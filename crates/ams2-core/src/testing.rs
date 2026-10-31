//! Frame builders, a clock and a recording sink for tests, and the scripted session (the port
//! of `replay_session.py`'s `session_script`, frames built in memory).
//!
//! Nothing here touches the game's mapping; later tasks (replay tool, integration tests) play
//! [`scripted_session`] into a test mapping.

use crate::channels::Row;
use crate::codes::{game_state, pit_mode, race_state, session_state};
use crate::messages::{LapRecord, ParticipantLap, SessionInfo, lap_type};
use crate::round_to;
use crate::setup::SetupSnapshot;
use crate::tracker::{Clock, Event, SessionTracker, Sink};
use ams2_shm::{ParticipantInfo, SharedMemory, new_frame};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A clock set by hand; clones share the time. `iso()` is the fixed text `ISO`.
#[derive(Clone, Default)]
pub struct FakeClock(Arc<AtomicU64>);

impl FakeClock {
    /// A clock at 0.
    pub fn new() -> Self {
        Self::default()
    }
    /// Sets the time.
    pub fn set(&self, t: f64) {
        self.0.store(t.to_bits(), Ordering::Relaxed);
    }
    /// Moves the time on.
    pub fn advance(&self, dt: f64) {
        self.set(self.now() + dt);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }
    fn iso(&self) -> String {
        "ISO".to_owned()
    }
}

/// One call the tracker made to its sink.
#[derive(Debug, Clone)]
pub enum Call {
    /// `start_session`.
    Start(SessionInfo),
    /// `end_session`.
    End {
        /// Session id.
        session_id: u64,
        /// Timestamp.
        ended_at: String,
    },
    /// `insert_lap`.
    Lap {
        /// The record.
        record: LapRecord,
        /// The samples.
        rows: Vec<Row>,
    },
    /// `insert_setup`.
    Setup {
        /// Session id.
        session_id: u64,
        /// `session_start` or `pit_exit`.
        reason: String,
        /// The snapshot.
        snapshot: SetupSnapshot,
    },
    /// `insert_pit_exit`.
    PitExit {
        /// Session id.
        session_id: u64,
        /// Fuel at the exit.
        fuel_l: Option<f64>,
    },
    /// `insert_participant_laps`.
    Participants {
        /// Session id.
        session_id: u64,
        /// The rows.
        rows: Vec<ParticipantLap>,
    },
    /// An event.
    Event(Event),
}

/// A sink that keeps every call, in order.
#[derive(Default)]
pub struct RecordingSink {
    /// The calls.
    pub calls: Vec<Call>,
    next_id: u64,
}

/// A stored lap as the Writer would number it.
#[derive(Debug, Clone)]
pub struct StoredLap {
    /// Lap number (1..n).
    pub lap: u32,
    /// The game's lap counter.
    pub sim_lap: u32,
    /// `formation`, `out`, `in` or `flying`.
    pub kind: &'static str,
    /// The record.
    pub record: LapRecord,
    /// The samples.
    pub rows: Vec<Row>,
}

impl RecordingSink {
    /// An empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// The laps in order.
    pub fn laps(&self) -> Vec<StoredLap> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                Call::Lap { record, rows } => Some(StoredLap {
                    lap: record.lap_number,
                    sim_lap: record.game_lap.unwrap_or(0),
                    kind: lap_type(record),
                    record: record.clone(),
                    rows: rows.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    /// Sessions the Writer would store: those with at least one lap.
    pub fn stored_sessions(&self) -> usize {
        let mut ids: Vec<u64> = self
            .calls
            .iter()
            .filter_map(|c| match c {
                Call::Lap { record, .. } => Some(record.session_id),
                _ => None,
            })
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    }

    /// The message kinds in order (`catalog` + `session_start` per session start, `setup`,
    /// `lap`, `pit_exit`, `participant_laps`, `session_end`).
    pub fn kinds(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        for c in &self.calls {
            match c {
                Call::Start(_) => out.extend(["catalog", "session_start"]),
                Call::End { .. } => out.push("session_end"),
                Call::Lap { .. } => out.push("lap"),
                Call::Setup { .. } => out.push("setup"),
                Call::PitExit { .. } => out.push("pit_exit"),
                Call::Participants { .. } => out.push("participant_laps"),
                Call::Event(_) => {}
            }
        }
        out
    }

    /// The events in order.
    pub fn events(&self) -> Vec<&Event> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                Call::Event(e) => Some(e),
                _ => None,
            })
            .collect()
    }

    /// The setups: `(reason, snapshot)`.
    pub fn setups(&self) -> Vec<(&str, &SetupSnapshot)> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                Call::Setup {
                    reason, snapshot, ..
                } => Some((reason.as_str(), snapshot)),
                _ => None,
            })
            .collect()
    }

    /// The rival rows of every batch.
    pub fn rival_rows(&self) -> Vec<ParticipantLap> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                Call::Participants { rows, .. } => Some(rows.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }
}

impl Sink for RecordingSink {
    fn start_session(&mut self, info: &SessionInfo) -> u64 {
        self.next_id += 1;
        self.calls.push(Call::Start(info.clone()));
        self.next_id
    }
    fn end_session(&mut self, session_id: u64, ended_at: &str) {
        self.calls.push(Call::End {
            session_id,
            ended_at: ended_at.to_owned(),
        });
    }
    fn insert_lap(&mut self, record: LapRecord, rows: Vec<Row>) {
        self.calls.push(Call::Lap { record, rows });
    }
    fn insert_setup(
        &mut self,
        session_id: u64,
        _taken_at: &str,
        reason: &str,
        snapshot: SetupSnapshot,
    ) {
        self.calls.push(Call::Setup {
            session_id,
            reason: reason.to_owned(),
            snapshot,
        });
    }
    fn insert_pit_exit(&mut self, session_id: u64, _at: &str, fuel_l: Option<f64>) {
        self.calls.push(Call::PitExit { session_id, fuel_l });
    }
    fn insert_participant_laps(&mut self, session_id: u64, rows: Vec<ParticipantLap>) {
        self.calls.push(Call::Participants { session_id, rows });
    }
    fn event(&mut self, event: Event) {
        self.calls.push(Call::Event(event));
    }
}

/// Builds successive shared memory frames for one car on track (the tracker tests' `Sim`).
pub struct Sim {
    seq: u32,
    /// The frame being built; changes persist from frame to frame, as in the game.
    pub d: Box<SharedMemory>,
}

fn put(dst: &mut [u8], text: &str) {
    dst.fill(0);
    dst[..text.len()].copy_from_slice(text.as_bytes());
}

impl Default for Sim {
    fn default() -> Self {
        Self::new()
    }
}

impl Sim {
    /// A practice session at Road Atlanta, one car, 51 L tank at half.
    pub fn new() -> Self {
        let mut d = new_frame();
        d.version = 14;
        d.game_state = game_state::INGAME_PLAYING;
        d.session_state = session_state::PRACTICE;
        d.viewed_participant_index = 0;
        d.num_participants = 1;
        put(&mut d.car_name, "MINI Cooper S 1965");
        put(&mut d.car_class_name, "TC60S");
        put(&mut d.track_location, "Road_Atlanta");
        put(&mut d.track_variation, "Full");
        d.track_length = 4088.0;
        d.fuel_capacity = 51.0;
        d.fuel_level = 0.5;
        d.brake_bias = 0.2;
        d.last_lap_time = -1.0;
        d.air_pressure = [174.0, 174.0, 192.0, 192.0];
        d.tyre_temp_left = [50.0, 60.0, 30.0, 33.0];
        d.tyre_temp_right = [60.0, 50.0, 33.0, 30.0];
        d.participant_info[0].is_active = 1;
        Self { seq: 0, d }
    }

    /// The next frame at 100 km/h.
    pub fn frame(&mut self, lap: u32, dist: f64, t: f64) -> &SharedMemory {
        self.frame_kmh(lap, dist, t, 100.0)
    }

    /// The next frame at `speed_kmh`.
    pub fn frame_kmh(&mut self, lap: u32, dist: f64, t: f64, speed_kmh: f64) -> &SharedMemory {
        self.seq += 2;
        self.d.sequence_number = self.seq;
        let p = &mut self.d.participant_info[0];
        p.current_lap = lap;
        p.current_lap_distance = dist as f32;
        self.d.current_time = t as f32;
        self.d.speed = (speed_kmh / 3.6) as f32;
        &self.d
    }
}

/// A tracker with its clock, sim and recording sink: the tests' fixture.
pub struct Harness {
    /// The tracker.
    pub tracker: SessionTracker,
    /// The frame builder.
    pub sim: Sim,
    /// The tracker's clock.
    pub clock: FakeClock,
    /// Everything the tracker wrote.
    pub sink: RecordingSink,
}

impl Default for Harness {
    fn default() -> Self {
        Self::new()
    }
}

impl Harness {
    /// A fresh fixture.
    pub fn new() -> Self {
        let clock = FakeClock::new();
        Self {
            tracker: SessionTracker::new(Box::new(clock.clone())),
            sim: Sim::new(),
            clock,
            sink: RecordingSink::new(),
        }
    }

    /// Builds a frame at 100 km/h and feeds it.
    pub fn feed(&mut self, lap: u32, dist: f64, t: f64) {
        self.feed_kmh(lap, dist, t, 100.0);
    }

    /// Builds a frame at `kmh` and feeds it.
    pub fn feed_kmh(&mut self, lap: u32, dist: f64, t: f64, kmh: f64) {
        self.sim.frame_kmh(lap, dist, t, kmh);
        self.tracker.process(&self.sim.d, &mut self.sink);
    }

    /// Feeds the frame as it is (after the test changed `sim.d`).
    pub fn feed_current(&mut self) {
        self.sim.seq += 2;
        self.sim.d.sequence_number = self.sim.seq;
        self.tracker.process(&self.sim.d, &mut self.sink);
    }

    /// Moves the clock on.
    pub fn tick(&self, secs: f64) {
        self.clock.advance(secs);
    }

    /// `drive_lap` of the Python tests: one frame per `step` s of the lap clock from `start_t`
    /// to `end_t`, the distance running from `start_dist` to the end of the track; the clock
    /// advances `step` after each frame.
    pub fn drive(&mut self, lap: u32, start_t: f64, end_t: f64, start_dist: f64) {
        self.drive_step(lap, start_t, end_t, start_dist, 1.0);
    }

    /// [`Harness::drive`] with a step.
    pub fn drive_step(&mut self, lap: u32, start_t: f64, end_t: f64, start_dist: f64, step: f64) {
        self.drive_core(lap, start_t, end_t, start_dist, step, 100.0);
    }

    /// [`Harness::drive`] at a speed.
    pub fn drive_kmh(&mut self, lap: u32, start_t: f64, end_t: f64, start_dist: f64, kmh: f64) {
        self.drive_core(lap, start_t, end_t, start_dist, 1.0, kmh);
    }

    fn drive_core(
        &mut self,
        lap: u32,
        start_t: f64,
        end_t: f64,
        start_dist: f64,
        step: f64,
        kmh: f64,
    ) {
        let mut t = start_t;
        while t < end_t {
            let dist = start_dist + (t - start_t) / (end_t - start_t) * (4088.0 - start_dist);
            self.feed_kmh(lap, dist, t, kmh);
            self.clock.advance(step);
            t += step;
        }
    }

    /// Ends the session (the recorder stops).
    pub fn end(&mut self) {
        self.tracker.end_session(&mut self.sink);
    }
}

// ------------------------------------------------------------------ the scripted session

/// Car and class of the scripted session.
pub const CAR: &str = "Ginetta G40 Cup";
/// Class code.
pub const CLASS: &str = "G40Cup";
/// Track location.
pub const TRACK: &str = "OultonPark";
/// Layout.
pub const LAYOUT: &str = "OultonPark_International";
/// Track length, m.
pub const LENGTH: f64 = 4316.9;
/// Sector 2 and sector 3 start (m).
pub const SECTORS: [f64; 2] = [1400.0, 3000.0];
/// Tank, litres.
pub const TANK: f64 = 40.0;
/// Game seconds per frame.
pub const DT: f64 = 0.1;
const FUEL_PER_M: f64 = 2.6 / LENGTH;
const GRASS: u32 = 7;
const ROAD: u32 = 0;
const KELVIN: f64 = 273.15;
/// The two AI cars: name, pace factor, start distance.
pub const RIVALS: [(&str, f64, f64); 2] = [
    ("Pol Riera", 1.015, 1200.0),
    ("Tyler Groves", 0.997, 2600.0),
];

fn speed_kmh(dist: f64, factor: f64) -> f64 {
    (172.0 + 45.0 * (2.0 * std::f64::consts::PI * 3.0 * dist / LENGTH).sin()) / factor
}

fn sector_index(dist: f64) -> i32 {
    if dist < SECTORS[0] {
        0
    } else if dist < SECTORS[1] {
        1
    } else {
        2
    }
}

/// One frame of the scripted session as the replay reader would hand it out.
pub struct ScriptedFrame {
    /// The frame, its sequence number numbered by the reader (it moves only when the game
    /// wrote a new frame).
    pub frame: Box<SharedMemory>,
    /// Game seconds played: the tracker's clock (`sequence / 2 * DT`).
    pub game_time: f64,
}

struct Script {
    seq: u32,
    d: Box<SharedMemory>,
    rival_clock: [f64; 2],
    out: Vec<Box<SharedMemory>>,
}

impl Script {
    fn new() -> Self {
        let mut d = new_frame();
        d.version = ams2_shm::SHARED_MEMORY_VERSION;
        d.game_state = game_state::FRONT_END;
        d.session_state = session_state::PRACTICE;
        d.viewed_participant_index = 0;
        d.num_participants = 1 + RIVALS.len() as i32;
        put(&mut d.car_name, CAR);
        put(&mut d.car_class_name, CLASS);
        put(&mut d.track_location, TRACK);
        put(&mut d.track_variation, LAYOUT);
        d.track_length = LENGTH as f32;
        d.num_sectors = 3;
        d.fuel_capacity = TANK as f32;
        d.fuel_level = (20.0 / TANK) as f32;
        d.brake_bias = 0.42;
        d.anti_lock_setting = -1;
        d.traction_control_setting = -1;
        d.ambient_temperature = 18.0;
        d.track_temperature = 26.0;
        d.session_duration = 30.0;
        d.event_time_remaining = 1800.0;
        d.last_lap_time = -1.0;
        d.best_lap_time = -1.0;
        d.current_time = -1.0;
        d.current_sector1_time = -1.0;
        d.current_sector2_time = -1.0;
        d.current_sector3_time = -1.0;
        d.air_pressure = [165.0, 165.0, 160.0, 160.0];
        d.tyre_carcass_temp = [(KELVIN + 19.0) as f32; 4];
        d.tyre_temp_left = [20.0; 4];
        d.tyre_temp_center = [20.0; 4];
        d.tyre_temp_right = [20.0; 4];
        d.brake_temp_celsius = [60.0; 4];
        for k in 0..4 {
            put(&mut d.tyre_compound[k], "Slick");
        }
        d.gear = 0;
        d.rpm = 900.0;
        let names: [(&str, f64); 3] = [
            ("leonardo", 150.0),
            (RIVALS[0].0, RIVALS[0].2),
            (RIVALS[1].0, RIVALS[1].2),
        ];
        for (i, (name, dist)) in names.iter().enumerate() {
            let p = &mut d.participant_info[i];
            p.is_active = 1;
            put(&mut p.name, name);
            p.current_lap = 1;
            p.laps_completed = 0;
            p.current_lap_distance = *dist as f32;
            p.current_sector = sector_index(*dist);
            p.race_position = i as u32 + 1;
            put(&mut d.car_names[i], CAR);
            put(&mut d.car_class_names[i], CLASS);
            d.last_lap_times[i] = -1.0;
            d.fastest_lap_times[i] = -1.0;
            d.current_sector1_times[i] = -1.0;
            d.current_sector2_times[i] = -1.0;
            d.current_sector3_times[i] = -1.0;
            d.race_states[i] = race_state::RACING;
        }
        Self {
            seq: 0,
            d,
            rival_clock: [0.0; 2],
            out: Vec::new(),
        }
    }

    fn me(&mut self) -> &mut ParticipantInfo {
        &mut self.d.participant_info[0]
    }

    fn emit(&mut self, advance: bool) {
        if advance {
            self.seq += 2;
            self.d.sequence_number = self.seq;
        }
        let mut f = new_frame();
        *f = *self.d;
        self.out.push(f);
    }

    fn tick_world(&mut self) {
        let d = &mut self.d;
        d.event_time_remaining = (f64::from(d.event_time_remaining) - DT).max(0.0) as f32;
        for (k, (_name, factor, _start)) in RIVALS.iter().enumerate() {
            let i = k + 1;
            self.rival_clock[k] += DT;
            let dist0 = f64::from(d.participant_info[i].current_lap_distance);
            d.participant_info[i].current_lap_distance =
                (dist0 + speed_kmh(dist0, *factor) / 3.6 * DT) as f32;
            let before = d.participant_info[i].current_sector;
            if f64::from(d.participant_info[i].current_lap_distance) >= LENGTH {
                let nd = f64::from(d.participant_info[i].current_lap_distance) - LENGTH;
                d.participant_info[i].current_lap_distance = nd as f32;
                let lap_s = round_to(self.rival_clock[k], 3);
                if d.participant_info[i].laps_completed > 0 {
                    // the first lap started mid-track: not timed
                    d.last_lap_times[i] = lap_s as f32;
                    let best = f64::from(d.fastest_lap_times[i]);
                    d.fastest_lap_times[i] =
                        if best <= 0.0 { lap_s } else { best.min(lap_s) } as f32;
                    d.current_sector3_times[i] = round_to(
                        lap_s
                            - f64::from(d.current_sector1_times[i])
                            - f64::from(d.current_sector2_times[i]),
                        3,
                    ) as f32;
                }
                d.participant_info[i].laps_completed += 1;
                d.participant_info[i].current_lap += 1;
                self.rival_clock[k] = 0.0;
            }
            let sec = sector_index(f64::from(d.participant_info[i].current_lap_distance));
            d.participant_info[i].current_sector = sec;
            if before == 0 && sec == 1 {
                d.current_sector1_times[i] = round_to(self.rival_clock[k], 3) as f32;
            } else if before == 1 && sec == 2 {
                d.current_sector2_times[i] = round_to(
                    self.rival_clock[k] - f64::from(d.current_sector1_times[i]),
                    3,
                ) as f32;
            }
        }
        self.positions();
    }

    /// Practice order: best lap, cars without one last (in slot order).
    fn positions(&mut self) {
        let d = &mut self.d;
        let n = d.num_participants as usize;
        let best: Vec<f64> = (0..n)
            .map(|i| {
                let f = f64::from(d.fastest_lap_times[i]);
                if f > 0.0 { f } else { f64::INFINITY }
            })
            .collect();
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by(|a, b| best[*a].total_cmp(&best[*b]).then(a.cmp(b)));
        for (pos, i) in order.into_iter().enumerate() {
            d.participant_info[i].race_position = pos as u32 + 1;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn drive(
        &mut self,
        to: f64,
        factor: f64,
        pit: u32,
        speed: Option<f64>,
        invalid_at: Option<f64>,
        off_track: Option<(f64, f64)>,
        crossing_counts: bool,
    ) {
        self.d.game_state = game_state::INGAME_PLAYING;
        self.d.pit_mode = pit;
        self.d.pit_modes[0] = pit;
        let mut travelled = 0.0;
        let target = to - f64::from(self.me().current_lap_distance);
        while travelled < target {
            let me_dist = f64::from(self.me().current_lap_distance);
            let kmh = speed.unwrap_or_else(|| speed_kmh(me_dist, factor));
            let step = kmh / 3.6 * DT;
            travelled += step;
            let mut dist = me_dist + step;
            if self.d.current_time >= 0.0 {
                self.d.current_time = (f64::from(self.d.current_time) + DT) as f32;
            }
            let sector_before = self.me().current_sector;
            if dist >= LENGTH {
                dist -= LENGTH;
                self.cross_line(crossing_counts, pit == pit_mode::NONE);
            }
            self.me().current_lap_distance = dist as f32;
            self.me().current_sector = sector_index(dist);
            let cur_sector = self.me().current_sector;
            let ct = f64::from(self.d.current_time);
            if ct > 0.0 && sector_before == 0 && cur_sector == 1 {
                let v = round_to(ct, 3) as f32;
                self.d.current_sector1_time = v;
                self.d.current_sector1_times[0] = v;
            } else if ct > 0.0 && sector_before == 1 && cur_sector == 2 {
                let s2 = round_to(ct - f64::from(self.d.current_sector1_time), 3) as f32;
                self.d.current_sector2_time = s2;
                self.d.current_sector2_times[0] = s2;
            }
            let off = off_track.is_some_and(|(a, b)| a <= dist && dist <= b);
            let terrain = if off { GRASS } else { ROAD };
            self.d.terrain[0] = terrain;
            self.d.terrain[2] = terrain;
            if invalid_at.is_some_and(|a| dist >= a) && cur_sector >= 1 {
                self.d.lap_invalidated = 1;
                self.d.laps_invalidated[0] = 1;
            }
            self.d.speed = (kmh / 3.6) as f32;
            self.d.gear = ((kmh / 35.0).floor() as i32).clamp(1, 6);
            self.d.rpm = (4000.0 + 30.0 * (kmh % 35.0)) as f32;
            self.d.throttle = 0.9;
            self.d.fuel_level =
                (f64::from(self.d.fuel_level) - FUEL_PER_M * step / TANK).max(0.0) as f32;
            let hot = (70.0 + kmh / 10.0) as f32;
            self.d.tyre_temp_left = [hot; 4];
            self.d.tyre_temp_center = [hot; 4];
            self.d.tyre_temp_right = [hot; 4];
            self.d.tyre_carcass_temp = [(KELVIN + f64::from(hot) - 5.0) as f32; 4];
            self.d.brake_temp_celsius = [(350.0 + kmh) as f32; 4];
            self.tick_world();
            self.emit(true);
        }
    }

    fn cross_line(&mut self, counts: bool, on_track: bool) {
        if on_track {
            if self.d.current_time > 0.0 {
                // a timed lap ends: the game publishes its time
                let lap_s = round_to(f64::from(self.d.current_time), 3) as f32;
                self.d.last_lap_time = lap_s;
                self.d.last_lap_times[0] = lap_s;
                let best = self.d.best_lap_time;
                if self.d.lap_invalidated == 0 && (best <= 0.0 || lap_s < best) {
                    self.d.best_lap_time = lap_s;
                    self.d.fastest_lap_times[0] = lap_s;
                }
                if self.d.current_sector1_time > 0.0 && self.d.current_sector2_time > 0.0 {
                    self.d.current_sector3_time = round_to(
                        f64::from(lap_s)
                            - f64::from(self.d.current_sector1_time)
                            - f64::from(self.d.current_sector2_time),
                        3,
                    ) as f32;
                }
            }
            self.d.current_time = 0.0; // the lap clock (re)starts at the line
        }
        if counts {
            self.me().current_lap += 1;
            self.me().laps_completed += 1;
        }
        self.d.lap_invalidated = 0;
        self.d.laps_invalidated[0] = 0;
        self.d.current_sector1_time = -1.0;
        self.d.current_sector2_time = -1.0;
    }

    fn stand(&mut self, seconds: f64, state: u32, pit: Option<u32>, advance: bool) {
        self.d.game_state = state;
        if let Some(p) = pit {
            self.d.pit_mode = p;
            self.d.pit_modes[0] = p;
        }
        self.d.speed = 0.0;
        self.d.gear = 0;
        self.d.throttle = 0.0;
        for _ in 0..((seconds / DT).round() as usize).max(1) {
            if advance {
                self.tick_world();
            }
            self.emit(advance);
        }
    }

    fn go(&mut self, to: f64) {
        self.drive(to, 1.0, pit_mode::NONE, None, None, None, true);
    }
}

/// The scripted session of `replay_session.py` (menus, garage as a menu, out lap, three flying
/// laps with one off track and invalid, a pit stop, a lap after it, return to garage mid lap,
/// menus), as the frames the replay reader would hand out, with the game clock of each.
pub fn scripted_session() -> Vec<ScriptedFrame> {
    use game_state::{FRONT_END, INGAME_INMENU_TIME_TICKING as MENU, INGAME_PAUSED};
    let mut s = Script::new();
    s.stand(1.0, FRONT_END, None, false);
    s.stand(2.0, MENU, Some(pit_mode::IN_GARAGE), true);
    s.stand(1.0, MENU, Some(pit_mode::IN_GARAGE), true);
    s.drive(
        300.0,
        1.0,
        pit_mode::DRIVING_OUT_OF_GARAGE,
        Some(60.0),
        None,
        None,
        true,
    );
    // Out lap (AMS2 flags it invalid, with no lap clock) and three flying laps (lap 4 off
    // track and invalid in sector 2).
    s.d.lap_invalidated = 1;
    s.d.laps_invalidated[0] = 1;
    s.go(1500.0);
    s.drive(LENGTH + 1.0, 1.0, pit_mode::NONE, None, None, None, false); // out lap: the clock starts
    s.go(LENGTH + 1.0); // flying lap 2
    s.drive(1700.0, 0.99, pit_mode::NONE, None, None, None, true); // lap 3, mid lap
    s.drive(LENGTH + 1.0, 0.99, pit_mode::NONE, None, None, None, true);
    s.drive(
        2300.0,
        1.01,
        pit_mode::NONE,
        None,
        Some(2000.0),
        Some((1950.0, 1990.0)),
        true,
    ); // lap 4
    s.drive(
        LENGTH + 1.0,
        1.01,
        pit_mode::NONE,
        None,
        Some(2000.0),
        None,
        true,
    );
    // In lap: pit entry, across the line in the pit lane, the box, the pit exit (stint 2).
    s.go(4150.0);
    s.drive(
        LENGTH + 120.0,
        1.0,
        pit_mode::DRIVING_INTO_PITS,
        Some(60.0),
        None,
        None,
        false,
    );
    s.d.current_time = -1.0;
    s.d.lap_invalidated = 1; // pit entry flags the lap (FINDINGS)
    s.d.laps_invalidated[0] = 1;
    s.stand(
        1.0,
        game_state::INGAME_PLAYING,
        Some(pit_mode::IN_PIT),
        true,
    );
    s.d.fuel_level = (f64::from(s.d.fuel_level) + 10.0 / TANK).min(1.0) as f32;
    s.stand(
        5.0,
        game_state::INGAME_PLAYING,
        Some(pit_mode::IN_PIT),
        true,
    );
    s.drive(
        260.0,
        1.0,
        pit_mode::DRIVING_OUT_OF_PITS,
        Some(60.0),
        None,
        None,
        true,
    );
    s.go(LENGTH + 1.0); // stint 2 out lap
    s.drive(LENGTH + 1.0, 0.995, pit_mode::NONE, None, None, None, true); // stint 2 flying lap
    // Mid lap: pause, "return to garage", then the main menu.
    s.go(3000.0);
    s.stand(0.5, INGAME_PAUSED, None, true);
    s.stand(1.0, INGAME_PAUSED, Some(pit_mode::IN_GARAGE), true);
    s.stand(0.5, FRONT_END, None, true);

    // The reader numbers the frames itself: it moves only when the game wrote a new one.
    let (mut script_seq, mut reader_seq) = (None, 0u32);
    s.out
        .into_iter()
        .map(|mut f| {
            if script_seq != Some(f.sequence_number) {
                script_seq = Some(f.sequence_number);
                reader_seq += 2;
            }
            f.sequence_number = reader_seq;
            ScriptedFrame {
                game_time: f64::from(reader_seq) / 2.0 * DT,
                frame: f,
            }
        })
        .collect()
}
