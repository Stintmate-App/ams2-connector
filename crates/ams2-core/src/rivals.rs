//! Rival timing (R2): one row per participant per lap counted by the game.
//!
//! Fed by the [`SessionTracker`](crate::tracker::SessionTracker) with every new shared memory
//! frame while a session is open. Every participant is tracked (AI cars and the player, so the
//! classification is built from one source), except the safety car.
//!
//! Semantics learned from a 2 Hz capture with 18 AI cars (Oulton Park practice):
//!
//! * `mCurrentSector` is 0-based and goes 2 -> 0 at *every* line crossing, including crossings in
//!   the pit lane and the first crossing of an out lap.
//! * `mLapsCompleted` only counts laps the game timed: it does not increment on the crossing that
//!   ends an out lap or on a pit-lane crossing. Those crossings start a new lap for us but
//!   produce no row.
//! * `mCurrentSector{1,2,3}Times[i]`: the value of the *current* sector is a running clock; it
//!   becomes final on the frame the sector index changes; the other values linger from the
//!   previous lap until overwritten. Unset times are -1 (and -123 for lap times). So S1 is read
//!   when the car goes from sector 2 to 3 and S2 at the line crossing; S3 = lap - S1 - S2.
//! * `mLastLapTimes[i]` may lag a few frames behind `mLapsCompleted`, so a counted lap waits until
//!   the value changes, or [`PENDING_TIMEOUT_S`] and then falls back to S1 + S2 + S3.
//! * `mRacePosition` in practice is the order by best lap; it updates on the crossing.
//!
//! Per-frame cost: a handful of integer comparisons per participant. Floats are read only at
//! sector changes and crossings. Rows are buffered and handed to the sink in batches.

use crate::messages::ParticipantLap;
use crate::tracker::Sink;
use crate::{decode_str, round_to, vocab};
use ams2_shm::SharedMemory;

/// A counted lap waits this long for the game's official time.
pub const PENDING_TIMEOUT_S: f64 = 3.0;
/// Two crossing signals (sector 2 -> 0, laps completed + 1) this close are the same crossing.
pub const SYNC_WINDOW_S: f64 = 3.0;
/// Ignore the invalid flag right after a crossing: it may still belong to the previous lap.
pub const INVALID_GRACE_S: f64 = 0.5;
/// Write buffered rows at least this often (they are also flushed with every player lap).
pub const FLUSH_INTERVAL_S: f64 = 30.0;
/// Names, cars and classes are re-read this often (and when the participant count changes).
pub const ROSTER_INTERVAL_S: f64 = 5.0;

/// The safety car is not a participant.
pub fn is_safety_car(name: &str, car_class: &str) -> bool {
    name.trim().to_lowercase() == "safety car"
        || car_class.replace(' ', "").to_lowercase() == "safetycar"
}

fn pos(v: f32) -> Option<f64> {
    let v = f64::from(v);
    (v > 0.0).then_some(v)
}

/// A lap that just ended on the line, waiting to be counted and timed.
#[derive(Debug, Clone)]
struct Closed {
    lap: u32,
    crossed_at: f64,
    s1: Option<f64>,
    s2: Option<f64>,
    invalid: bool,
    pit_modes: u64,
    pit_stops: u32,
    from_line: bool,
    since: f64,
}

#[derive(Debug, Clone)]
struct Car {
    name: String,
    car: String,
    car_class: String,
    lc: u32,
    sector: i32,
    pit: u32,
    /// `mLastLapTimes` value already used (or present at the lap start).
    baseline_last: f64,
    /// Tracker clock at the crossing that started the lap.
    lap_start: Option<f64>,
    invalid: bool,
    pit_modes: u64,
    pit_stops: u32,
    s1: Option<f64>,
    last_cross: Option<f64>,
    /// Crossing not (yet) counted by the game.
    held: Option<Closed>,
    /// Counted lap waiting for its official time.
    pending: Option<Closed>,
}

/// Rival timing for one session at a time.
pub struct RivalTracker {
    session_id: Option<u64>,
    t0: f64,
    cars: Vec<Option<Car>>,
    tracked: Vec<usize>,
    n: i32,
    roster_at: f64,
    player: i32,
    rows: Vec<ParticipantLap>,
    rows_session: Option<u64>,
    last_flush: f64,
    /// A private copy of the last frame (so a session end can read what a frame had).
    last_d: Option<Box<SharedMemory>>,
    last_d_valid: bool,
}

impl Default for RivalTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl RivalTracker {
    /// A tracker with nothing known.
    pub fn new() -> Self {
        Self {
            session_id: None,
            t0: 0.0,
            cars: vec![None; 64],
            tracked: Vec::with_capacity(64),
            n: -1,
            roster_at: 0.0,
            player: -1,
            rows: Vec::new(),
            rows_session: None,
            last_flush: 0.0,
            last_d: None,
            last_d_valid: false,
        }
    }

    /// Takes a frame of the open session `session_id`; `now` is the tracker clock.
    pub fn process(&mut self, d: &SharedMemory, session_id: u64, now: f64, sink: &mut dyn Sink) {
        match self.last_d.as_mut() {
            Some(b) => **b = *d,
            None => {
                let mut b = ams2_shm::new_frame();
                *b = *d;
                self.last_d = Some(b);
            }
        }
        self.last_d_valid = true;
        if self.session_id != Some(session_id) {
            self.end_session(now, sink);
            // end_session cleared the copy: it is the frame being processed
            if let Some(b) = self.last_d.as_mut() {
                **b = *d;
            }
            self.last_d_valid = true;
            self.session_id = Some(session_id);
            self.t0 = now;
            self.last_flush = now;
            self.n = -1;
        }
        let n = d.num_participants.clamp(0, d.participant_info.len() as i32);
        if n != self.n || now - self.roster_at > ROSTER_INTERVAL_S {
            self.refresh_roster(d, n, now);
        }

        for k in 0..self.tracked.len() {
            let i = self.tracked[k];
            let p = &d.participant_info[i];
            let (lc, sec, pit) = (p.laps_completed, p.current_sector, d.pit_modes[i]);
            if self.cars[i].as_ref().is_some_and(|c| c.pending.is_some()) {
                self.resolve(i, d, now);
            }
            let Some(c) = self.cars[i].as_mut() else {
                continue;
            };
            if lc == c.lc && sec == c.sector && pit == c.pit {
                if !c.invalid
                    && d.laps_invalidated[i] != 0
                    && c.lap_start.is_none_or(|s| now - s > INVALID_GRACE_S)
                {
                    c.invalid = true;
                }
                continue;
            }
            self.changed(i, d, lc, sec, pit, now);
        }

        if !self.rows.is_empty() && now - self.last_flush > FLUSH_INTERVAL_S {
            self.flush(now, sink);
        }
    }

    /// Hands the buffered rows to the sink (one batch).
    pub fn flush(&mut self, now: f64, sink: &mut dyn Sink) {
        self.last_flush = now;
        if !self.rows.is_empty() {
            let rows = std::mem::take(&mut self.rows);
            if let Some(id) = self.rows_session {
                sink.insert_participant_laps(id, rows);
            }
        }
    }

    /// Resolves the laps still waiting for a time (with what is known) and flushes.
    pub fn end_session(&mut self, now: f64, sink: &mut dyn Sink) {
        let d = if self.last_d_valid {
            self.last_d.take()
        } else {
            None
        };
        if self.session_id.is_some() {
            for i in 0..self.cars.len() {
                if self.cars[i].as_ref().is_some_and(|c| c.pending.is_some()) {
                    self.save(i, d.as_deref(), None);
                }
            }
        }
        self.flush(now, sink);
        self.session_id = None;
        self.cars.iter_mut().for_each(|c| *c = None);
        self.tracked.clear();
        self.last_d = d; // keep the allocation
        self.last_d_valid = false;
    }

    fn refresh_roster(&mut self, d: &SharedMemory, n: i32, now: f64) {
        self.n = n;
        self.roster_at = now;
        self.player = d.viewed_participant_index;
        self.tracked.clear();
        for i in 0..n as usize {
            let p = &d.participant_info[i];
            if p.is_active == 0 {
                continue;
            }
            let name = decode_str(&p.name);
            let car_class = decode_str(&d.car_class_names[i]);
            if is_safety_car(&name, &car_class) {
                continue;
            }
            let car = decode_str(&d.car_names[i]);
            match self.cars[i].as_mut() {
                Some(c) if c.name == name => {
                    if c.car != car {
                        c.car = car.into_owned();
                    }
                    if c.car_class != car_class {
                        c.car_class = car_class.into_owned();
                    }
                }
                _ => {
                    // new participant in this slot
                    self.cars[i] = Some(Car {
                        name: name.into_owned(),
                        car: car.into_owned(),
                        car_class: car_class.into_owned(),
                        lc: p.laps_completed,
                        sector: p.current_sector,
                        pit: d.pit_modes[i],
                        baseline_last: f64::from(d.last_lap_times[i]),
                        lap_start: None,
                        invalid: false,
                        pit_modes: 0,
                        pit_stops: 0,
                        s1: None,
                        last_cross: None,
                        held: None,
                        pending: None,
                    });
                }
            }
            self.tracked.push(i);
        }
        for i in 0..self.cars.len() {
            if self.cars[i].is_some() && !self.tracked.contains(&i) {
                self.cars[i] = None;
            }
        }
    }

    fn changed(&mut self, i: usize, d: &SharedMemory, lc: u32, sec: i32, pit: u32, now: f64) {
        let t0 = self.t0;
        let c = self.cars[i].as_mut().expect("tracked car");
        if pit != c.pit {
            if c.pit == 0 && pit != 0 {
                c.pit_stops += 1;
            }
            if pit != 0 {
                c.pit_modes |= 1 << pit.min(63);
            }
            c.pit = pit;
        }

        if lc < c.lc {
            // session restarted in the game: start over for this car
            c.lc = lc;
            c.sector = sec;
            c.lap_start = None;
            c.s1 = None;
            c.held = None;
            c.pending = None;
            c.invalid = false;
            c.pit_modes = 0;
            c.baseline_last = f64::from(d.last_lap_times[i]);
            return;
        }

        let line = c.sector == 2 && sec == 0;
        let counted = lc > c.lc;
        if sec != c.sector {
            if c.sector == 1 && sec == 2 {
                c.s1 = pos(d.current_sector1_times[i]);
            }
            c.sector = sec;
        }
        if !(line || counted) {
            return;
        }

        if c.last_cross.is_some_and(|x| now - x < SYNC_WINDOW_S) {
            // Second signal of a crossing already handled (the counter or the sector index
            // arrived a few frames later).
            if counted && let Some(held) = c.held.take() {
                self.count(i, d, held, lc, now);
            }
            if let Some(c) = self.cars[i].as_mut() {
                c.lc = lc;
            }
            return;
        }

        let closed = Closed {
            lap: lc,
            crossed_at: now - t0,
            s1: c.s1,
            s2: if c.s1.is_some() {
                pos(d.current_sector2_times[i])
            } else {
                None
            },
            invalid: c.invalid,
            pit_modes: c.pit_modes,
            pit_stops: c.pit_stops,
            from_line: c.lap_start.is_some(),
            since: 0.0,
        };
        c.last_cross = Some(now);
        c.lap_start = Some(now);
        c.s1 = None;
        c.invalid = false;
        c.pit_modes = if pit != 0 { 1 << pit.min(63) } else { 0 };
        if counted {
            c.held = None;
            self.count(i, d, closed, lc, now);
        } else {
            c.held = Some(closed); // out lap / pit lane: counted only if the counter follows
        }
        if let Some(c) = self.cars[i].as_mut() {
            c.lc = lc;
        }
    }

    fn count(&mut self, i: usize, d: &SharedMemory, mut closed: Closed, lc: u32, now: f64) {
        if self.cars[i].as_ref().is_some_and(|c| c.pending.is_some()) {
            // previous lap never got its time: give up waiting
            self.save(i, Some(d), None);
        }
        closed.lap = lc;
        closed.since = now;
        if let Some(c) = self.cars[i].as_mut() {
            c.pending = Some(closed);
        }
        self.resolve(i, d, now);
    }

    fn resolve(&mut self, i: usize, d: &SharedMemory, now: f64) {
        let Some(c) = self.cars[i].as_ref() else {
            return;
        };
        let Some(p) = c.pending.as_ref() else { return };
        let last = f64::from(d.last_lap_times[i]);
        if last > 0.0 && last != c.baseline_last {
            self.save(i, Some(d), Some(last));
        } else if now - p.since > PENDING_TIMEOUT_S {
            self.save(i, Some(d), None);
        }
    }

    fn save(&mut self, i: usize, d: Option<&SharedMemory>, official: Option<f64>) {
        let player = self.player;
        let Some(c) = self.cars[i].as_mut() else {
            return;
        };
        let Some(lap) = c.pending.take() else { return };
        let (s1, s2) = (lap.s1, lap.s2);
        let (lap_time, source) = if let Some(o) = official {
            (Some(o), Some("official"))
        } else {
            let s3 = d.and_then(|d| pos(d.current_sector3_times[i]));
            match (nonzero(s1), nonzero(s2), s3) {
                (Some(a), Some(b), Some(s3)) => (Some(a + b + s3), Some("sectors")),
                _ => (None, None),
            }
        };
        let (position, fastest, race_state) = match d {
            Some(d) => {
                c.baseline_last = f64::from(d.last_lap_times[i]);
                let position = Some(d.participant_info[i].race_position).filter(|p| *p > 0);
                (
                    position,
                    pos(d.fastest_lap_times[i]),
                    Some(d.race_states[i]),
                )
            }
            None => (None, None, None),
        };
        let s3 = match (nonzero(lap_time), nonzero(s1), nonzero(s2)) {
            (Some(t), Some(a), Some(b)) => Some(t - a - b),
            _ => None,
        }
        .filter(|s| *s > 0.0);
        let sectors = if s1.is_some() || s2.is_some() || s3.is_some() {
            vec![s1, s2, s3]
        } else {
            Vec::new()
        };
        let timed = lap_time.filter(|t| *t > 0.0);
        let row = ParticipantLap {
            participant: i as u32,
            lap: lap.lap,
            name: if c.name.trim().is_empty() {
                format!("Participant {i}")
            } else {
                c.name.clone()
            },
            car_id: (!c.car.trim().is_empty()).then(|| c.car.clone()),
            class_id: (!c.car_class.trim().is_empty()).then(|| c.car_class.clone()),
            is_player: i as i32 == player,
            time_s: timed,
            time_source: timed.and(source).map(str::to_owned),
            sectors,
            valid: !lap.invalid,
            pit: lap.pit_modes != 0,
            pit_stops: Some(lap.pit_stops),
            from_line: lap.from_line,
            position,
            crossed_at_s: Some(round_to(lap.crossed_at, 3).max(0.0)),
            sim_best_lap_s: fastest,
            state: race_state.map(vocab::race_state),
        };
        self.rows_session = self.session_id;
        self.rows.push(row);
    }
}

fn nonzero(v: Option<f64>) -> Option<f64> {
    v.filter(|x| *x != 0.0)
}
