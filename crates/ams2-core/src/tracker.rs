//! Session / lap state machine.
//!
//! Feed it one shared memory snapshot per sample with [`SessionTracker::process`]. It opens
//! sessions, splits laps at the start/finish line, buffers samples in memory and hands each lap
//! to the [`Sink`] once it is finished. Time comes from an injected [`Clock`]; nothing here
//! reads the system clock.
//!
//! Line crossings: a lap ends when `mCurrentLap` increments *or* when the lap distance wraps
//! from the end of the track back to ~0 while on track. The second signal is needed because AMS2
//! does not increment `mCurrentLap` on the first crossing after leaving the garage: the out lap
//! and the first flying lap used to end up merged. Both signals usually arrive within a frame of
//! each other, so the second one inside [`SYNC_WINDOW_S`] of a lap started on the line only syncs
//! the game lap number. Laps are numbered 1..n per session by us; the game's number is kept as
//! `game_lap` (`sim_lap` in the model).
//!
//! Timing start: before a race start (grid, formation lap) the game lap clock is -1; when it
//! starts counting on track the lap is split there too, so the formation lap is stored on its own
//! and race lap 1 starts with the timing.
//!
//! Session start: when a race loads, the game can still report the previous session's lap
//! counter. A counter jump before the session has saved any lap restarts the lap instead of
//! opening a new session. A session that ends with no lap is never stored (the write API's rule).
//!
//! Garage: AMS2's garage screen is a menu with the pit mode "in the garage", so the
//! session is opened there already, with no lap and no sample; laps still start with the first
//! frame on track. A session opened in the garage and left with no lap is discarded.
//!
//! A lap that never moved is not a lap: if the session ends before the car moves, the
//! lap's top speed stays under [`MOVING_KMH`] and it is dropped.
//!
//! Return to garage from the menu: the tracker takes no samples while the game is paused
//! or in a menu, so in that branch an open lap is marked as an in lap as soon as the pit mode
//! shows the pits or the garage.
//!
//! The frame's `current_lap` is the game's counter (`sim_lap`), never ours.
//!
//! Lap time: the game updates `mLastLapTime` around the crossing (not necessarily on the same
//! frame), so a finished lap waits as "pending" until it changes, or falls back to the lap clock
//! at its last sample after [`PENDING_TIMEOUT_S`].
//!
//! Steady state: a tick allocates nothing (the lap's sample buffer is reserved when the lap
//! starts and grows by doubling only on very long laps; strings are built at lap and session
//! boundaries only).

use crate::channels::{FUEL, PIT, Row, SPEED, T, TERRAIN, extract_into};
use crate::codes::{game_state, pit_mode, session_state};
use crate::garage::{GarageSeat, MOVING_KMH};
use crate::messages::{LapRecord, ParticipantLap, SessionInfo};
use crate::rivals::RivalTracker;
use crate::setup::{COLD_TYRE_MARGIN_C, SetupSnapshot};
use crate::vocab;
use crate::{decode_str, round_to};
use ams2_shm::{ParticipantInfo, SharedMemory};
use serde::Serialize;

/// A finished lap waits this long for the game's official time.
pub const PENDING_TIMEOUT_S: f64 = 3.0;
/// Ignore the invalid flag right after crossing the line: it may still belong to the previous lap.
pub const INVALID_GRACE_S: f64 = 0.5;
/// Lap distance up to which a car on lap 1 in the last sector is still on the grid, behind the
/// line (the game holds the distance at 0 there).
const GRID_DIST_M: f64 = 0.5;
/// Partial laps shorter than this (e.g. a session ended right after a crossing) are dropped.
pub const MIN_PARTIAL_LAP_S: f64 = 5.0;
/// A lap starting this close to the line, with the lap clock near zero, counts as starting on it.
pub const LINE_START_DIST_M: f64 = 50.0;
/// Distance wrap window: from the last this-many metres of the track to the first.
pub const LINE_WINDOW_M: f64 = 200.0;
/// A second crossing signal this soon after a lap started on the line is the same crossing.
pub const SYNC_WINDOW_S: f64 = 5.0;
/// Looking back this far on the lap clock for the cause of an invalid flag.
pub const CAUSE_WINDOW_S: f64 = 10.0;
/// Four wheels on kerbs must last this many samples (~0.1 s).
pub const MIN_KERB_SAMPLES: usize = 5;
/// Aborted sector times end this close to the lap clock of the last timed sample.
pub const ABORTED_SECTOR_TOLERANCE_S: f64 = 0.15;
/// Sample rows reserved when a lap starts (about 80 s at 50 Hz).
pub const ROWS_RESERVED: usize = 4096;

const KELVIN: f64 = 273.15;

fn is_session_end_state(s: u32) -> bool {
    matches!(
        s,
        game_state::EXITED
            | game_state::FRONT_END
            | game_state::INGAME_RESTARTING
            | game_state::INGAME_REPLAY
            | game_state::FRONT_END_REPLAY
    )
}
fn is_pit_exit_mode(m: u32) -> bool {
    m == pit_mode::DRIVING_OUT_OF_PITS || m == pit_mode::DRIVING_OUT_OF_GARAGE
}
fn is_out_lap_mode(m: u32) -> bool {
    matches!(
        m,
        pit_mode::IN_PIT
            | pit_mode::DRIVING_OUT_OF_PITS
            | pit_mode::IN_GARAGE
            | pit_mode::DRIVING_OUT_OF_GARAGE
    )
}
/// Only counts once the car has been on track during the lap (it starts in the garage).
fn is_in_lap_mode(m: u32) -> bool {
    matches!(
        m,
        pit_mode::DRIVING_INTO_PITS | pit_mode::IN_PIT | pit_mode::IN_GARAGE
    )
}
/// The car may be refuelled or reset here, so these samples don't count for fuel used.
fn is_stopped_mode(m: f64) -> bool {
    m == f64::from(pit_mode::IN_PIT) || m == f64::from(pit_mode::IN_GARAGE)
}
fn is_race_type(name: &str) -> bool {
    name == "RACE" || name == "FORMATION_LAP"
}

/// Time source of the tracker (injected so tests and replays control it).
pub trait Clock: Send {
    /// Monotonic seconds.
    fn now(&self) -> f64;
    /// The wall clock as an ISO 8601 timestamp with offset, whole seconds.
    fn iso(&self) -> String;
}

/// Where the tracker's records go. `session_id` is the id [`Sink::start_session`] returned.
pub trait Sink {
    /// A session starts; returns its id.
    fn start_session(&mut self, info: &SessionInfo) -> u64;
    /// A session ends (a session with no lap is never stored).
    fn end_session(&mut self, session_id: u64, ended_at: &str);
    /// A finished lap and its samples.
    fn insert_lap(&mut self, record: LapRecord, rows: Vec<Row>);
    /// A setup snapshot (`session_start` or `pit_exit`).
    fn insert_setup(
        &mut self,
        session_id: u64,
        taken_at: &str,
        reason: &str,
        snapshot: SetupSnapshot,
    );
    /// The car left the pits or the garage.
    fn insert_pit_exit(&mut self, session_id: u64, at: &str, fuel_l: Option<f64>);
    /// A batch of rival rows.
    fn insert_participant_laps(&mut self, session_id: u64, rows: Vec<ParticipantLap>);
    /// An event for tools and tests (the recorder ignores them).
    fn event(&mut self, _event: Event) {}
}

/// Events of the tracker (the hook stays for tests and tools).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// A session started.
    SessionStarted {
        /// Sink id.
        session_id: u64,
        /// Car.
        car: String,
        /// Class.
        car_class: String,
        /// Track.
        track: String,
        /// Layout.
        layout: String,
        /// Track length, m (1 decimal).
        track_length_m: f64,
        /// AMS2 session state name.
        session_type: String,
        /// Ambient, C (1 decimal).
        ambient_c: f64,
        /// Track, C (1 decimal).
        track_c: f64,
        /// Rain (2 decimals).
        rain: f64,
    },
    /// A session ended.
    SessionEnded {
        /// Sink id.
        session_id: u64,
        /// Laps saved.
        laps: u32,
        /// Ended with no lap: never stored.
        discarded: bool,
    },
    /// A lap was saved.
    LapCompleted(LapEvent),
}

/// A saved lap as published to the event hook.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LapEvent {
    /// Sink id.
    pub session_id: u64,
    /// Stored lap number.
    pub lap: u32,
    /// The game's lap counter.
    pub game_lap: Option<u32>,
    /// `formation`, `out`, `in`, `flying`.
    pub kind: &'static str,
    /// Reached the line.
    pub complete: bool,
    /// Valid for the game.
    pub valid: bool,
    /// Lap time (3 decimals).
    pub lap_time_s: Option<f64>,
    /// `official` or `measured`.
    pub time_source: Option<String>,
    /// Sectors (3 decimals).
    pub sectors_s: [Option<f64>; 3],
    /// Time minus the session's best clean lap.
    pub delta_session_best_s: Option<f64>,
    /// A new session best.
    pub session_best: bool,
    /// Fuel used (2 decimals).
    pub fuel_used_l: Option<f64>,
    /// Fuel left (2 decimals).
    pub fuel_left_l: Option<f64>,
    /// Fuel left over the average per flying lap (1 decimal).
    pub fuel_laps_left: Option<f64>,
    /// Top speed (1 decimal).
    pub max_speed_kmh: Option<f64>,
    /// Track temperature (1 decimal).
    pub track_c: Option<f64>,
    /// Ambient temperature (1 decimal).
    pub ambient_c: Option<f64>,
    /// Race position of the viewed car.
    pub position: Option<u32>,
}

/// The open lap for the live frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveLap {
    /// `formation`, `out`, `in` or `flying` as known so far.
    pub lap_type: &'static str,
    /// Once the game flagged the lap invalid: the probable cause.
    pub invalid_cause: Option<&'static str>,
    /// `false` when the cause is only probable (kerbs).
    pub invalid_cause_certain: bool,
    /// Lap distance where the flag came.
    pub invalid_at_m: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionKey {
    car: String,
    track: String,
    variation: String,
    state: u32,
}

impl SessionKey {
    fn of(d: &SharedMemory) -> Self {
        Self {
            car: decode_str(&d.car_name).into_owned(),
            track: decode_str(&d.track_location).into_owned(),
            variation: decode_str(&d.track_variation).into_owned(),
            state: d.session_state,
        }
    }

    /// Compares without allocating.
    fn matches(&self, d: &SharedMemory) -> bool {
        self.state == d.session_state
            && self.car == decode_str(&d.car_name)
            && self.track == decode_str(&d.track_location)
            && self.variation == decode_str(&d.track_variation)
    }
}

pub(crate) struct Session {
    pub(crate) id: u64,
    key: SessionKey,
    pub(crate) laps_saved: u32,
    /// A timed lap has run in this session: a race has started, so an untimed lap is no longer
    /// a formation lap.
    timed_seen: bool,
    /// The car has been on track since the last pit exit (a cheap flag).
    driven: bool,
    /// AMS2 session state name (`PRACTICE`, `UNKNOWN_9`...).
    pub(crate) session_type: String,
    /// Best complete, valid, flying lap so far.
    best_lap_s: Option<f64>,
    fuel_per_lap: Vec<f64>,
}

pub(crate) struct Lap {
    /// Expected stored number (1..n); final when saved.
    pub(crate) number: u32,
    /// The game's `mCurrentLap`.
    pub(crate) game_lap: u32,
    started_at: String,
    /// Tracker clock.
    created_at: f64,
    from_line: bool,
    pub(crate) out_lap: bool,
    prev_last_lap_time: f64,
    /// Sector times present when the lap started: possibly left over from the previous lap.
    s1_at_start: f64,
    s2_at_start: f64,
    pub(crate) rows: Vec<Row>,
    invalid: bool,
    invalid_cause: Option<&'static str>,
    invalid_cause_certain: bool,
    invalid_at_m: Option<f64>,
    pub(crate) in_lap: bool,
    been_on_track: bool,
    visited_garage: bool,
    sector1: Option<f64>,
    sector2: Option<f64>,
    first_sample_at: f64,
    last_sample_at: f64,
    /// The game lap clock ran (>= 0) on track during this lap.
    timed: bool,
    /// ambient, track, rain, cloud, wind at the last sample.
    weather: Option<[f64; 5]>,
}

struct Pending {
    lap: Lap,
    session_id: u64,
    measured_time: Option<f64>,
    since: f64,
}

/// For a lap that did not reach the line: when the lap is aborted (garage via the menu, session
/// ended, out of fuel) the game writes the running sector time, which then ends exactly at the
/// lap clock of the last timed sample. Such values are not real sector times.
pub fn drop_aborted_sectors(
    s1: Option<f64>,
    s2: Option<f64>,
    rows: &[Row],
) -> (Option<f64>, Option<f64>) {
    let Some(last_t) = rows
        .iter()
        .filter(|r| r[PIT] == 0.0 && r[T] > 0.0)
        .map(|r| r[T])
        .next_back()
    else {
        return (None, None);
    };
    let aborted = |cumulative: f64| {
        (cumulative - last_t).abs() < ABORTED_SECTOR_TOLERANCE_S
            || cumulative > last_t + ABORTED_SECTOR_TOLERANCE_S
    };
    if s1.is_some_and(aborted) {
        return (None, None);
    }
    let mut s2 = s2;
    if let (Some(a), Some(b)) = (s1, s2)
        && aborted(a + b)
    {
        s2 = None;
    }
    (s1, s2)
}

/// The probable cause of the invalid flag raised at the last of `rows` (the lap's samples so
/// far): `(cause, certain)`. Looks back [`CAUSE_WINDOW_S`] on the lap clock; samples in the
/// pits are skipped (AMS2 reports the pit lane as run-off).
pub fn invalid_cause(rows: &[Row]) -> (&'static str, bool) {
    let Some(last) = rows.last() else {
        return ("unknown", true);
    };
    let t_flag = last[T];
    let (mut off_track, mut strip) = (false, false);
    let (mut kerb_run, mut best_kerb_run) = (0usize, 0usize);
    for row in rows.iter().rev() {
        let t = row[T];
        if t < 0.0 || t_flag - t > CAUSE_WINDOW_S {
            break;
        }
        if row[PIT] != 0.0 {
            kerb_run = 0;
            continue;
        }
        let surfaces = TERRAIN.map(|c| vocab::surface(row[c]));
        let off = surfaces.iter().filter(|s| (2..=8).contains(*s)).count();
        if off >= 2 {
            let all_strip = surfaces
                .iter()
                .filter(|s| (2..=8).contains(*s))
                .all(|s| *s == vocab::SURFACE_STRIP);
            if all_strip {
                strip = true;
            } else {
                off_track = true;
            }
        }
        let kerbs = surfaces
            .iter()
            .filter(|s| **s == vocab::SURFACE_KERB)
            .count();
        if kerbs > 0 && kerbs + off == 4 {
            kerb_run += 1;
            best_kerb_run = best_kerb_run.max(kerb_run);
        } else {
            kerb_run = 0;
        }
    }
    if off_track {
        ("off_track", true)
    } else if strip {
        ("track_limits", true)
    } else if best_kerb_run >= MIN_KERB_SAMPLES {
        ("track_limits", false)
    } else {
        ("unknown", true)
    }
}

fn positive(v: f64) -> Option<f64> {
    (v > 0.0).then_some(v)
}

fn nonzero(v: Option<f64>) -> Option<f64> {
    v.filter(|x| *x != 0.0)
}

/// The open session as the status shows it ([`SessionTracker::status_view`]).
#[derive(Debug, Clone, PartialEq)]
pub struct StatusView {
    /// The sink's id.
    pub session_id: u64,
    /// Car id.
    pub car: String,
    /// Track id.
    pub track: String,
    /// Layout id.
    pub layout: String,
    /// AMS2 session state name.
    pub session_type: String,
    /// The open lap's stored number.
    pub lap_number: Option<u32>,
}

/// [`StatusView`] borrowed from the tracker: no allocation ([`SessionTracker::status_ref`]).
#[derive(Debug, Clone, Copy)]
pub struct StatusRef<'a> {
    /// The sink's id.
    pub session_id: u64,
    /// Car id.
    pub car: &'a str,
    /// Track id.
    pub track: &'a str,
    /// Layout id.
    pub layout: &'a str,
    /// AMS2 session state name.
    pub session_type: &'a str,
    /// The open lap's stored number.
    pub lap_number: Option<u32>,
}

/// The session / lap state machine (see the module docs).
pub struct SessionTracker {
    clock: Box<dyn Clock>,
    rivals: RivalTracker,
    /// The player's `mRacePosition`, last frame.
    position: Option<u32>,
    /// A player lap was saved: write the rival rows too.
    flush_rivals: bool,
    pub(crate) session: Option<Session>,
    pub(crate) lap: Option<Lap>,
    pending: Option<Pending>,
    last_seq: Option<u32>,
    last_pit_mode: u32,
    prev_dist: Option<f64>,
    prev_t: Option<f64>,
    garage: GarageSeat,
}

impl SessionTracker {
    /// A tracker on `clock`.
    pub fn new(clock: Box<dyn Clock>) -> Self {
        Self {
            clock,
            rivals: RivalTracker::new(),
            position: None,
            flush_rivals: false,
            session: None,
            lap: None,
            pending: None,
            last_seq: None,
            last_pit_mode: pit_mode::NONE,
            prev_dist: None,
            prev_t: None,
            garage: GarageSeat::new(),
        }
    }

    /// The viewed car is in the garage: the garage screen, or the driver sitting in the car
    /// before it moved (pit mode "driving out of the garage" at 0 km/h).
    pub fn in_garage(&self) -> bool {
        self.garage.in_garage
    }

    /// What the recorder's status shows of the open session (allocates: ask twice a second).
    pub fn status_view(&self) -> Option<StatusView> {
        let s = self.session.as_ref()?;
        Some(StatusView {
            session_id: s.id,
            car: s.key.car.clone(),
            track: s.key.track.clone(),
            layout: s.key.variation.clone(),
            session_type: s.session_type.clone(),
            lap_number: self.lap.as_ref().map(|l| l.number),
        })
    }

    /// [`status_view`](Self::status_view) without copying: no allocation, for the loop.
    pub fn status_ref(&self) -> Option<StatusRef<'_>> {
        let s = self.session.as_ref()?;
        Some(StatusRef {
            session_id: s.id,
            car: &s.key.car,
            track: &s.key.track,
            layout: &s.key.variation,
            session_type: &s.session_type,
            lap_number: self.lap.as_ref().map(|l| l.number),
        })
    }

    /// The open lap is not an out lap (for the telemetry flag `FLYING`); no allocation.
    pub fn lap_not_out(&self) -> bool {
        self.lap.as_ref().is_some_and(|l| !l.out_lap)
    }

    /// Forgets the garage seat (the game is gone or a new mapping).
    pub fn reset_garage(&mut self) {
        self.garage.reset();
    }

    /// The sink's id of the open session.
    pub fn session_id(&self) -> Option<u64> {
        self.session.as_ref().map(|s| s.id)
    }

    /// Takes one shared memory snapshot (a frame with the same sequence number as the last one is
    /// ignored).
    pub fn process(&mut self, d: &SharedMemory, sink: &mut dyn Sink) {
        if self.last_seq == Some(d.sequence_number) {
            return; // game has not written a new frame
        }
        self.last_seq = Some(d.sequence_number);
        self.garage.update(d.pit_mode, f64::from(d.speed));

        let state = d.game_state;
        if is_session_end_state(state) {
            self.end_session(sink);
            return;
        }

        let viewed = d.viewed_participant_index;
        if viewed >= 0 && (viewed as usize) < d.participant_info.len() {
            // before any lap of this frame is saved
            self.position =
                Some(d.participant_info[viewed as usize].race_position).filter(|p| *p != 0);
        }
        self.resolve_pending(d, sink);
        if state != game_state::INGAME_PLAYING {
            self.mark_in_lap(d.pit_mode); // "return to garage" from the menu
            self.open_in_garage(d, sink); // the session exists before leaving the garage
            if self.session.is_some() {
                self.feed_rivals(d, sink); // AI cars keep lapping while the player is in a menu
            }
            return; // paused / in menu: keep the lap open, take no samples
        }

        let idx = d.viewed_participant_index;
        let n = d.num_participants.clamp(0, d.participant_info.len() as i32);
        if !(0..n).contains(&idx) {
            return;
        }
        let p = d.participant_info[idx as usize];

        if self.session.as_ref().is_some_and(|s| !s.key.matches(d)) {
            self.end_session(sink);
        }
        if self.session.is_none() {
            self.start_session(d, SessionKey::of(d), sink);
        }

        let pit = d.pit_mode;
        if is_pit_exit_mode(pit) && !is_pit_exit_mode(self.last_pit_mode) {
            self.on_pit_exit(d, sink);
        }
        self.last_pit_mode = pit;

        let lap_no = p.current_lap;
        let dist = f64::from(p.current_lap_distance);
        let wrapped = self.distance_wrapped(d, dist);
        self.prev_dist = Some(dist);
        // Race start: the lap clock starts counting on track (it was -1 on the grid / formation).
        let current_time = f64::from(d.current_time);
        let timing_started =
            self.prev_t.is_some_and(|t| t < 0.0) && current_time >= 0.0 && pit == pit_mode::NONE;
        self.prev_t = Some(current_time);

        match self.lap.as_ref().map(|l| l.game_lap) {
            None => self.start_lap(d, &p, false),
            Some(game_lap) if lap_no == game_lap.saturating_add(1) => {
                if self.just_started_on_line() {
                    // counter caught up with a crossing already handled
                    if let Some(l) = self.lap.as_mut() {
                        l.game_lap = lap_no;
                    }
                } else {
                    self.finish_lap(d, sink);
                    self.start_lap(d, &p, true);
                }
            }
            Some(game_lap) if lap_no != game_lap => {
                if pit != pit_mode::NONE {
                    // The game may reset the lap counter while the car is in the pits/garage
                    // (also the first frame after a "return to garage" from the menu).
                    self.mark_in_lap(pit);
                    self.close_partial_lap(sink);
                    self.start_lap(d, &p, false);
                } else if self.session.as_ref().is_some_and(|s| s.laps_saved == 0)
                    && self.pending.is_none()
                {
                    // A race loading still reports the previous session's counter: same session.
                    self.close_partial_lap(sink);
                    self.start_lap(d, &p, false);
                } else {
                    // The counter jumped on track: a new session.
                    self.end_session(sink);
                    self.start_session(d, SessionKey::of(d), sink);
                    self.start_lap(d, &p, false);
                }
            }
            Some(_) => {
                if (wrapped || timing_started) && !self.just_started_on_line() {
                    // Crossing without a lap counter change (first crossing after leaving the
                    // garage), or the race start (timing begins on track after the grid).
                    self.finish_lap(d, sink);
                    self.start_lap(d, &p, true);
                }
            }
        }

        self.sample(d, &p);
        self.feed_rivals(d, sink);
    }

    /// In AMS2 the garage is a menu (game state "in menu, time ticking" or paused, pit
    /// mode "in the garage"), so the playing branch only saw the session when the car left the
    /// garage. Open it here, with no lap and no sample.
    fn open_in_garage(&mut self, d: &SharedMemory, sink: &mut dyn Sink) {
        if d.pit_mode != pit_mode::IN_GARAGE
            || !matches!(
                d.game_state,
                game_state::INGAME_PAUSED | game_state::INGAME_INMENU_TIME_TICKING
            )
        {
            return;
        }
        if d.session_state == session_state::INVALID {
            return;
        }
        let n = d.num_participants.clamp(0, d.participant_info.len() as i32);
        if !(0..n).contains(&d.viewed_participant_index) {
            return;
        }
        let key = SessionKey::of(d);
        if key.car.is_empty() || key.track.is_empty() || key.variation.is_empty() {
            return; // car, track or layout not given yet (still loading)
        }
        if let Some(s) = &self.session {
            if s.key == key {
                return;
            }
            // The event moved on while in the garage (e.g. practice -> qualifying).
            self.end_session(sink);
        }
        self.start_session(d, key, sink);
    }

    /// The open lap ends in the pits or the garage: an in lap. A lap started in the
    /// garage stays an out lap (out wins over in, as for every lap).
    fn mark_in_lap(&mut self, pit: u32) {
        let Some(lap) = self.lap.as_mut() else { return };
        if !is_in_lap_mode(pit) {
            return;
        }
        lap.in_lap = true;
        if pit == pit_mode::IN_GARAGE {
            lap.visited_garage = true;
        }
    }

    fn feed_rivals(&mut self, d: &SharedMemory, sink: &mut dyn Sink) {
        let Some(id) = self.session_id() else { return };
        let now = self.clock.now();
        self.rivals.process(d, id, now, sink);
        if self.flush_rivals {
            // After process(), so the batch includes the crossing that ended the lap.
            self.flush_rivals = false;
            self.rivals.flush(now, sink);
        }
    }

    fn distance_wrapped(&self, d: &SharedMemory, dist: f64) -> bool {
        let length = f64::from(d.track_length);
        self.prev_dist.is_some_and(|prev| {
            length > 0.0
                && d.pit_mode == pit_mode::NONE
                && prev > length - LINE_WINDOW_M
                && dist < LINE_WINDOW_M
        })
    }

    fn just_started_on_line(&self) -> bool {
        self.lap
            .as_ref()
            .is_some_and(|l| l.from_line && self.clock.now() - l.created_at < SYNC_WINDOW_S)
    }

    /// Ends the open session (the recorder stopped, the game left, the event changed).
    pub fn end_session(&mut self, sink: &mut dyn Sink) {
        if self.pending.is_some() {
            self.save_pending(None, sink);
        }
        if self.session.is_none() {
            return;
        }
        self.close_partial_lap(sink);
        let now = self.clock.now();
        self.rivals.end_session(now, sink); // rival rows go before the session_end
        let Some(s) = self.session.take() else { return };
        // A session with no lap is never stored (the Writer drops it at session_end).
        sink.end_session(s.id, &self.clock.iso());
        sink.event(Event::SessionEnded {
            session_id: s.id,
            laps: s.laps_saved,
            discarded: s.laps_saved == 0,
        });
        self.lap = None;
    }

    // ----------------------------------------------------------------- session

    fn start_session(&mut self, d: &SharedMemory, key: SessionKey, sink: &mut dyn Sink) {
        let session_type = session_state::NAMES
            .get(key.state as usize)
            .map_or_else(|| format!("UNKNOWN_{}", key.state), |s| (*s).to_owned());
        let car_class = decode_str(&d.car_class_name).into_owned();
        let session_id = sink.start_session(&SessionInfo {
            started_at: self.clock.iso(),
            car: key.car.clone(),
            car_class: car_class.clone(),
            track: key.track.clone(),
            variation: key.variation.clone(),
            session_state: session_type.clone(),
            length_m: Some(f64::from(d.track_length)),
            fuel_capacity_l: Some(f64::from(d.fuel_capacity)),
            ambient_c: Some(f64::from(d.ambient_temperature)),
            track_c: Some(f64::from(d.track_temperature)),
            rain: Some(f64::from(d.rain_density)),
            wind_speed: Some(f64::from(d.wind_speed)),
        });
        let event = Event::SessionStarted {
            session_id,
            car: key.car.clone(),
            car_class,
            track: key.track.clone(),
            layout: key.variation.clone(),
            track_length_m: round_to(f64::from(d.track_length), 1),
            session_type: session_type.clone(),
            ambient_c: round_to(f64::from(d.ambient_temperature), 1),
            track_c: round_to(f64::from(d.track_temperature), 1),
            rain: round_to(f64::from(d.rain_density), 2),
        };
        self.session = Some(Session {
            id: session_id,
            key,
            laps_saved: 0,
            timed_seen: false,
            driven: false,
            session_type,
            best_lap_s: None,
            fuel_per_lap: Vec::new(),
        });
        self.last_pit_mode = d.pit_mode;
        self.prev_dist = None;
        self.prev_t = None;
        self.snapshot_setup(d, "session_start", sink);
        sink.event(event);
    }

    /// The car left the pits or the garage: tell Pitwall, which decides whether that starts a
    /// stint. Every lap that ended before the exit goes first (the pending one with what it has,
    /// the open one closed as partial when the car had been on track), then the exit, then the
    /// setup snapshot (it belongs to the stint the exit starts).
    fn on_pit_exit(&mut self, d: &SharedMemory, sink: &mut dyn Sink) {
        if self.pending.is_some() {
            self.save_pending(None, sink);
        }
        let Some(s) = self.session.as_mut() else {
            return;
        };
        let (id, driven) = (s.id, s.driven);
        if driven {
            s.driven = false;
            self.close_partial_lap(sink);
        }
        let fuel = round_to(f64::from(d.fuel_level) * f64::from(d.fuel_capacity), 2);
        sink.insert_pit_exit(id, &self.clock.iso(), Some(fuel));
        self.snapshot_setup(d, "pit_exit", sink);
    }

    fn snapshot_setup(&mut self, d: &SharedMemory, reason: &str, sink: &mut dyn Sink) {
        let Some(id) = self.session_id() else { return };
        let bias = f64::from(d.brake_bias);
        let wings = d.wings.map(f64::from);
        let carcass_max = d
            .tyre_carcass_temp
            .iter()
            .map(|k| f64::from(*k))
            .fold(f64::MIN, f64::max);
        let snapshot = SetupSnapshot {
            brake_bias_front_pct: (bias >= 0.0).then(|| round_to((1.0 - bias) * 100.0, 1)),
            abs_setting: d.anti_lock_setting,
            tc_setting: d.traction_control_setting,
            wings: [round_to(wings[0], 3), round_to(wings[1], 3)],
            tyre_compound: std::array::from_fn(|i| decode_str(&d.tyre_compound[i]).into_owned()),
            tyre_pressure_kpa: d.air_pressure.map(|v| round_to(f64::from(v), 1)),
            // Pressures are only "cold" setup values if the tyres are at ambient temperature
            // (AMS2 pre-heats them at a race start).
            tyre_carcass_c: d
                .tyre_carcass_temp
                .map(|k| round_to(f64::from(k) - KELVIN, 1)),
            ambient_c: round_to(f64::from(d.ambient_temperature), 1),
            pressures_cold: carcass_max - KELVIN
                < f64::from(d.ambient_temperature) + COLD_TYRE_MARGIN_C,
            fuel_l: round_to(f64::from(d.fuel_level) * f64::from(d.fuel_capacity), 2),
            fuel_capacity_l: f64::from(d.fuel_capacity),
        };
        sink.insert_setup(id, &self.clock.iso(), reason, snapshot);
    }

    // -------------------------------------------------------------------- laps

    fn start_lap(&mut self, d: &SharedMemory, p: &ParticipantInfo, crossed_line: bool) {
        let on_line = crossed_line
            || (f64::from(p.current_lap_distance) < LINE_START_DIST_M
                && f64::from(d.current_time) < 1.0);
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let waiting = self
            .pending
            .as_ref()
            .is_some_and(|p| p.session_id == session.id);
        self.lap = Some(Lap {
            number: session.laps_saved + 1 + u32::from(waiting),
            game_lap: p.current_lap,
            started_at: self.clock.iso(),
            created_at: self.clock.now(),
            from_line: on_line,
            out_lap: !on_line || is_out_lap_mode(d.pit_mode),
            prev_last_lap_time: f64::from(d.last_lap_time),
            s1_at_start: f64::from(d.current_sector1_time),
            s2_at_start: f64::from(d.current_sector2_time),
            rows: Vec::with_capacity(ROWS_RESERVED),
            invalid: false,
            invalid_cause: None,
            invalid_cause_certain: true,
            invalid_at_m: None,
            in_lap: false,
            been_on_track: false,
            visited_garage: false,
            sector1: None,
            sector2: None,
            first_sample_at: 0.0,
            last_sample_at: 0.0,
            timed: false,
            weather: None,
        });
    }

    fn sample(&mut self, d: &SharedMemory, p: &ParticipantInfo) {
        let now = self.clock.now();
        let (Some(lap), Some(session)) = (self.lap.as_mut(), self.session.as_mut()) else {
            return;
        };
        if lap.rows.is_empty() {
            lap.first_sample_at = now;
        }
        lap.last_sample_at = now;
        let mut row = [0.0; crate::channels::N_CHANNELS];
        extract_into(d, p, &mut row);
        lap.rows.push(row);

        let current_time = f64::from(d.current_time);
        // On a standing start the game raises mLapInvalidated while the car sits on the grid
        // (lap 1, last sector, lap distance still 0) and clears it when the car crosses the line
        //: that is not the driver's fault, so it does not latch.
        let on_grid = p.current_lap <= 1
            && p.current_sector == 2
            && f64::from(p.current_lap_distance) <= GRID_DIST_M;
        if d.lap_invalidated != 0 && current_time > INVALID_GRACE_S && !on_grid {
            if !lap.invalid {
                let (cause, certain) = invalid_cause(&lap.rows);
                lap.invalid_cause = Some(cause);
                lap.invalid_cause_certain = certain;
                lap.invalid_at_m = Some(f64::from(p.current_lap_distance));
            }
            lap.invalid = true;
        }
        let pit = d.pit_mode;
        if current_time >= 0.0 && pit == pit_mode::NONE {
            lap.timed = true;
            session.timed_seen = true;
        }
        lap.weather = Some([
            f64::from(d.ambient_temperature),
            f64::from(d.track_temperature),
            f64::from(d.rain_density),
            f64::from(d.cloud_brightness),
            f64::from(d.wind_speed),
        ]);
        if pit == pit_mode::NONE {
            lap.been_on_track = true;
            session.driven = true;
        } else if is_in_lap_mode(pit) && lap.been_on_track {
            lap.in_lap = true;
        }
        if pit == pit_mode::IN_GARAGE {
            lap.visited_garage = true;
        }
        if is_out_lap_mode(pit) && lap.rows.len() == 1 {
            lap.out_lap = true;
        }
        // Only while the lap is being timed on track (the game writes a bogus "aborted" sector
        // time when teleporting to the garage), and only values that changed since the lap
        // started (older ones belong to the previous lap).
        if pit == pit_mode::NONE && current_time > 0.0 {
            let (s1, s2) = (
                f64::from(d.current_sector1_time),
                f64::from(d.current_sector2_time),
            );
            if s1 > 0.0 && s1 != lap.s1_at_start {
                lap.sector1 = Some(s1);
            }
            if s2 > 0.0 && s2 != lap.s2_at_start {
                lap.sector2 = Some(s2);
            }
        }
    }

    /// The open lap for the live frame: `lap_type` as known so far and, once the game
    /// flagged it, the cause. `None` with no open lap.
    pub fn live_lap(&self) -> Option<LiveLap> {
        let (lap, s) = (self.lap.as_ref()?, self.session.as_ref()?);
        let (formation, box_lap) = untimed_race_lap(s, lap);
        let lap_type = if formation {
            "formation"
        } else if lap.out_lap {
            "out"
        } else if lap.in_lap || box_lap {
            "in"
        } else {
            "flying"
        };
        Some(LiveLap {
            lap_type,
            invalid_cause: lap.invalid.then_some(lap.invalid_cause).flatten(),
            invalid_cause_certain: lap.invalid_cause_certain,
            invalid_at_m: if lap.invalid { lap.invalid_at_m } else { None },
        })
    }

    /// Save the open lap as incomplete (e.g. returning to the pits) and drop it.
    fn close_partial_lap(&mut self, sink: &mut dyn Sink) {
        if let Some(lap) = self.lap.take()
            && let Some(id) = self.session_id()
        {
            self.save_lap(lap, id, false, None, None, sink);
        }
    }

    fn finish_lap(&mut self, d: &SharedMemory, sink: &mut dyn Sink) {
        if self.pending.is_some() {
            self.save_pending(None, sink); // previous lap still waiting: give up on its official time
        }
        let (Some(lap), Some(session_id)) = (self.lap.take(), self.session_id()) else {
            return;
        };
        if !lap.timed {
            // grid / formation lap: there is no lap time to wait for
            self.save_lap(lap, session_id, false, None, None, sink);
            return;
        }
        let measured = if lap.from_line {
            lap.rows.last().map(|r| r[T])
        } else {
            None
        };
        self.pending = Some(Pending {
            lap,
            session_id,
            measured_time: measured,
            since: self.clock.now(),
        });
        self.resolve_pending(d, sink);
    }

    fn resolve_pending(&mut self, d: &SharedMemory, sink: &mut dyn Sink) {
        let Some(pending) = self.pending.as_ref() else {
            return;
        };
        let last = f64::from(d.last_lap_time);
        let official = if last > 0.0 && last != pending.lap.prev_last_lap_time {
            Some(last)
        } else if self.clock.now() - pending.since > PENDING_TIMEOUT_S {
            None
        } else {
            return;
        };
        self.save_pending(official, sink);
        // The lap now open must not mistake the time just published for its own.
        if let Some(lap) = self.lap.as_mut() {
            lap.prev_last_lap_time = last;
        }
    }

    fn save_pending(&mut self, official: Option<f64>, sink: &mut dyn Sink) {
        let Some(pending) = self.pending.take() else {
            return;
        };
        let lap = pending.lap;
        let complete = lap.from_line && lap.timed && !lap.visited_garage;
        self.save_lap(
            lap,
            pending.session_id,
            complete,
            official,
            pending.measured_time,
            sink,
        );
    }

    fn save_lap(
        &mut self,
        lap: Lap,
        session_id: u64,
        complete: bool,
        official: Option<f64>,
        measured: Option<f64>,
        sink: &mut dyn Sink,
    ) {
        if lap.rows.is_empty() {
            return;
        }
        // Use our own clock: the game's lap clock resets (to -1) in the pits/garage.
        if !complete && lap.last_sample_at - lap.first_sample_at < MIN_PARTIAL_LAP_S {
            return; // fragment (e.g. a few metres between pit exit and the line)
        }
        let top_speed = lap
            .rows
            .iter()
            .map(|r| r[SPEED])
            .fold(lap.rows[0][SPEED], f64::max);
        if top_speed < MOVING_KMH {
            return; // the car never moved (sat in the garage): not a lap
        }
        let (lap_time, source) = if complete && nonzero(official).is_some() {
            (official, Some("official"))
        } else if complete && measured.is_some_and(|m| m > 0.0) {
            (measured, Some("measured"))
        } else {
            (None, None)
        };
        let (mut s1, mut s2) = (lap.sector1, lap.sector2);
        if !complete {
            (s1, s2) = drop_aborted_sectors(s1, s2, &lap.rows);
        }
        let s3 = match (nonzero(lap_time), nonzero(s1), nonzero(s2)) {
            (Some(t), Some(a), Some(b)) => Some(t - a - b),
            _ => None,
        };
        let rows = &lap.rows;
        let fuel_end = rows
            .iter()
            .rev()
            .find(|r| !is_stopped_mode(r[PIT]))
            .unwrap_or(&rows[rows.len() - 1])[FUEL];
        let session = self.session.as_ref().filter(|s| s.id == session_id);
        let (formation, box_lap) = session.map_or((false, false), |s| untimed_race_lap(s, &lap));
        let [ambient, track_c, rain, _cloud, wind] = match lap.weather {
            Some(w) => w.map(Some),
            None => [None; 5],
        };
        let record = LapRecord {
            session_id,
            lap_number: session.map_or(lap.number, |s| s.laps_saved + 1),
            started_at: lap.started_at.clone(),
            lap_time_s: lap_time,
            measured_time_s: measured,
            sector1_s: s1,
            sector2_s: s2,
            sector3_s: s3.and_then(positive),
            complete,
            valid: !lap.invalid,
            out_lap: lap.out_lap,
            in_lap: lap.in_lap || box_lap,
            fuel_start_l: Some(rows[0][FUEL]),
            fuel_end_l: Some(fuel_end),
            max_speed_kmh: Some(top_speed),
            n_samples: rows.len(),
            on_track: lap.been_on_track,
            game_lap: Some(lap.game_lap),
            lap_time_source: source.map(str::to_owned),
            formation_lap: formation,
            ambient_c: ambient,
            track_c,
            rain,
            wind_speed: wind,
        };
        let has_session = session.is_some();
        sink.insert_lap(record.clone(), lap.rows);
        if has_session {
            let position = self.position;
            if let Some(s) = self.session.as_mut() {
                s.laps_saved += 1;
                let event = lap_event(s, &record, position);
                sink.event(Event::LapCompleted(event));
            }
        }
        self.flush_rivals = true; // rival rows ride along with the player's lap write
    }
}

/// Classify an untimed, non-out lap of a race as `(formation, box_lap)`. Before the first timed
/// lap of the session it is the formation lap; afterwards it is a lap spent waiting in the pit
/// box (the box has no lap clock), part of the stop that began with the `in` lap, so it is
/// stored as `in`.
fn untimed_race_lap(s: &Session, lap: &Lap) -> (bool, bool) {
    let untimed_race = is_race_type(&s.session_type) && !lap.timed && !lap.out_lap;
    (untimed_race && !s.timed_seen, untimed_race && s.timed_seen)
}

fn lap_event(s: &mut Session, r: &LapRecord, position: Option<u32>) -> LapEvent {
    let kind = crate::messages::lap_type(r);
    let clean = r.complete && r.valid && kind == "flying" && r.lap_time_s.is_some();
    let t = r.lap_time_s;
    let delta = |best: Option<f64>| match (t, best) {
        (Some(t), Some(b)) => Some(round_to(t - b, 3)),
        _ => None,
    };
    let used = match (r.fuel_start_l, r.fuel_end_l) {
        (Some(a), Some(b)) => Some(a - b),
        _ => None,
    };
    if kind == "flying"
        && r.complete
        && let Some(u) = used.filter(|u| *u > 0.0)
    {
        s.fuel_per_lap.push(u);
    }
    let avg_fuel = (!s.fuel_per_lap.is_empty())
        .then(|| s.fuel_per_lap.iter().sum::<f64>() / s.fuel_per_lap.len() as f64);
    let event = LapEvent {
        session_id: s.id,
        lap: r.lap_number,
        game_lap: r.game_lap,
        kind,
        complete: r.complete,
        valid: r.valid,
        lap_time_s: t.map(|t| round_to(t, 3)),
        time_source: r.lap_time_source.clone(),
        sectors_s: [r.sector1_s, r.sector2_s, r.sector3_s].map(|x| x.map(|x| round_to(x, 3))),
        delta_session_best_s: if clean { delta(s.best_lap_s) } else { None },
        session_best: clean
            && match (t, s.best_lap_s) {
                (Some(t), Some(b)) => t < b,
                _ => true,
            },
        fuel_used_l: used.map(|u| round_to(u, 2)),
        fuel_left_l: r.fuel_end_l.map(|f| round_to(f, 2)),
        fuel_laps_left: match (r.fuel_end_l, nonzero(avg_fuel)) {
            (Some(f), Some(a)) => Some(round_to(f / a, 1)),
            _ => None,
        },
        max_speed_kmh: r.max_speed_kmh.map(|v| round_to(v, 1)),
        track_c: r.track_c.map(|v| round_to(v, 1)),
        ambient_c: r.ambient_c.map(|v| round_to(v, 1)),
        position,
    };
    if clean && let Some(t) = t {
        s.best_lap_s = Some(s.best_lap_s.map_or(t, |b| b.min(t)));
    }
    event
}
