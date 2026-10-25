//! Live race events from AMS2's shared memory, as edges (PRY-052).
//!
//! [`EventDetector`] is fed every snapshot the recorder takes (50 Hz) and returns the
//! announcements that fired on it, as codes of the catalog with their params; most ticks it
//! returns an empty list after a handful of integer comparisons. Pure logic: no I/O, no
//! threads, no clock of its own (injected through [`Clock`]). Sending, rate limits and
//! Pitwall's absence are the caller's.
//!
//! The rules are those of the research (`PRY-052`, section 2); none has been seen firing in a
//! real AMS2 race yet (`catalog::validation`).
//!
//! 1. **Gate.** Only while the game state is playing (never in replays, the front end,
//!    restarting; paused and menus freeze everything). Nothing is announced in the first
//!    [`QUIET_S`] seconds after a session load, a session-state change, or a player race state
//!    change through NOT_STARTED / INVALID; values are followed silently as the baseline.
//! 2. **Debounce.** A value counts once it has been the same for its hold time ([`HOLD_S`]).
//! 3. **Hysteresis.** A yellow re-arms only after [`YELLOW_REARM_S`] of NONE/GREEN;
//!    `flag_clear` only after YELLOW/DOUBLE_YELLOW to GREEN/NONE held [`CLEAR_HOLD_S`].
//! 4. **Cooldowns** per code ([`cooldown`]); the blue flag at most [`BLUE_MAX_PER_CAR`] times
//!    for the same lapping car per session. Once-per-session codes: [`ONCE`].
//! 5. **Rivals by identity** (name + car class), never by array index: inactive slots, names
//!    starting with NUL, the player's duplicates and the safety car are ignored; a DQ or
//!    retirement is announced once per session per identity; the arrays are read at
//!    `rival_hz` only; nothing while the leader is FINISHED.
//! 6. **Context.** Yellow, double yellow, blue, slow car and clear are not announced in the pit
//!    lane; blue, penalties, the race states only in race sessions (the caution also in the
//!    formation lap).
//! 7. **The player's own fields** (flag, race state, pit schedule) only while the viewed car is
//!    the player (its name is kept); viewing another car freezes them, and coming back re-takes
//!    their current values silently.

use crate::codes::{game_state, pit_mode, session_state};
use crate::tracker::Clock;
use ams2_shm::SharedMemory;
use serde_json::{Map, Value, json};
use std::borrow::Cow;
use std::collections::HashMap;

/// `mHighestFlagColour` (SharedMemory.h).
pub mod flag {
    /// No flag.
    pub const NONE: i64 = 0;
    /// Green.
    pub const GREEN: i64 = 1;
    /// Blue.
    pub const BLUE: i64 = 2;
    /// White: slow car ahead.
    pub const WHITE_SLOW_CAR: i64 = 3;
    /// White: final lap.
    pub const WHITE_FINAL_LAP: i64 = 4;
    /// Red.
    pub const RED: i64 = 5;
    /// Yellow.
    pub const YELLOW: i64 = 6;
    /// Double yellow.
    pub const DOUBLE_YELLOW: i64 = 7;
    /// Black and white.
    pub const BLACK_AND_WHITE: i64 = 8;
    /// Black with an orange circle.
    pub const BLACK_ORANGE_CIRCLE: i64 = 9;
    /// Black.
    pub const BLACK: i64 = 10;
    /// Chequered.
    pub const CHEQUERED: i64 = 11;
}

/// `mHighestFlagReason`.
pub mod reason {
    /// A car off on its own.
    pub const SOLO_CRASH: u32 = 1;
    /// A collision.
    pub const VEHICLE_CRASH: u32 = 2;
    /// A car blocking the track.
    pub const VEHICLE_OBSTRUCTION: u32 = 3;
}

/// `mYellowFlagState` (full-course yellow / safety car).
pub mod yellow {
    /// Invalid.
    pub const INVALID: i64 = -1;
    /// None.
    pub const NONE: i64 = 0;
    /// Pending.
    pub const PENDING: i64 = 1;
    /// Pits closed.
    pub const PITS_CLOSED: i64 = 2;
    /// Pits open for the lead lap.
    pub const PIT_LEAD_LAP: i64 = 3;
    /// Pits open.
    pub const PITS_OPEN: i64 = 4;
    /// Pits open (second value).
    pub const PITS_OPEN2: i64 = 5;
    /// Last lap.
    pub const LAST_LAP: i64 = 6;
    /// Resume.
    pub const RESUME: i64 = 7;
    /// Race halted.
    pub const RACE_HALT: i64 = 8;
}

/// Race states of a participant (`mRaceState`, `mRaceStates`).
pub mod race {
    /// Invalid.
    pub const INVALID: i64 = 0;
    /// Not started.
    pub const NOT_STARTED: i64 = 1;
    /// Racing.
    pub const RACING: i64 = 2;
    /// Finished.
    pub const FINISHED: i64 = 3;
    /// Disqualified.
    pub const DISQUALIFIED: i64 = 4;
    /// Retired.
    pub const RETIRED: i64 = 5;
    /// Did not finish.
    pub const DNF: i64 = 6;
}

/// `mPitSchedule` values that are penalties.
pub mod schedule {
    /// None.
    pub const NONE: i64 = 0;
    /// Drive-through.
    pub const DRIVE_THROUGH: i64 = 5;
    /// Stop-and-go.
    pub const STOP_GO: i64 = 6;
}

/// Seconds of silence after a load, a session-state change or a race start.
pub const QUIET_S: f64 = 10.0;
/// Hold times: flag, race state, pit schedule, caution.
pub const HOLD_S: Hold = Hold {
    flag: 0.5,
    race_state: 1.0,
    schedule: 0.5,
    fcy: 0.5,
};
/// A yellow re-arms after this long of NONE/GREEN.
pub const YELLOW_REARM_S: f64 = 5.0;
/// `flag_clear` after the yellow ended and this long held.
pub const CLEAR_HOLD_S: f64 = 3.0;
/// NONE to PITS_OPEN directly (CrewChief): "pits open" this much later.
pub const FCY_JUMP_DELAY_S: f64 = 8.0;
/// Blue flags for the same lapping car per session.
pub const BLUE_MAX_PER_CAR: u32 = 3;
/// Below this speed (m/s) yellow, blue and slow-car calls are skipped.
pub const STANDSTILL_MS: f32 = 1.0;
/// The lapping car is looked for this far behind on track.
pub const BLUE_RANGE_M: f64 = 500.0;
/// The player counts as slow (m/s, on track) below this: his own yellow is about him (PRY-090).
pub const SELF_SLOW_MS: f32 = 5.0;
/// A yellow edge within this many seconds of the player being slow is the player's own.
pub const SELF_WINDOW_S: f64 = 5.0;
/// Another car is a candidate cause of a yellow below this speed (m/s); AI cars slow to 12-15
/// m/s through a yellow zone and none went under 6.
pub const CAUSE_SLOW_MS: f32 = 6.0;
/// The cause is looked for this far ahead of the player on track.
pub const CAUSE_AHEAD_M: f64 = 1500.0;
/// The cause is looked for this far behind the player on track (the zone reaches past it).
pub const CAUSE_BEHIND_M: f64 = 350.0;
/// A black and white within this long of the lap being invalidated is the track-limits call.
pub const TRACK_LIMITS_WINDOW_S: f64 = 1.5;
/// A slow car is named when it is this close ahead on track.
pub const SLOW_CAR_RANGE_M: f64 = 500.0;
/// A slow car goes below this share of the player's speed.
pub const SLOW_CAR_RATIO: f32 = 0.6;

/// The debounce hold times.
#[derive(Debug, Clone, Copy)]
pub struct Hold {
    /// Flags.
    pub flag: f64,
    /// Race states.
    pub race_state: f64,
    /// Pit schedule.
    pub schedule: f64,
    /// The caution.
    pub fcy: f64,
}

/// Minimum seconds between two announcements of the same code (0: edge only).
pub fn cooldown(code: &str) -> f64 {
    match code {
        "flag_yellow"
        | "flag_double_yellow"
        | "flag_yellow_ahead"
        | "flag_double_yellow_ahead"
        | "flag_yellow_driver"
        | "flag_yellow_self"
        | "flag_yellow_cause"
        | "flag_red"
        | "flag_black_orange" => 30.0,
        "flag_clear"
        | "flag_blue"
        | "flag_blue_unknown"
        | "flag_slow_car"
        | "flag_slow_car_driver" => 20.0,
        "flag_black_white" | "flag_track_limits" => 15.0,
        _ => 0.0,
    }
}

/// Announced at most once per session.
pub const ONCE: [&str; 5] = [
    "final_lap",
    "disqualified",
    "retired",
    "chequered",
    "chequered_no_position",
];

fn is_yellow(f: i64) -> bool {
    f == flag::YELLOW || f == flag::DOUBLE_YELLOW
}
fn is_clear(f: Option<i64>) -> bool {
    matches!(f, Some(flag::NONE | flag::GREEN))
}
fn is_unsettled(r: i64) -> bool {
    r == race::NOT_STARTED || r == race::INVALID
}
fn is_out(r: i64) -> bool {
    r == race::RETIRED || r == race::DNF
}
fn flag_code(f: i64) -> Option<&'static str> {
    Some(match f {
        flag::YELLOW => "flag_yellow",
        flag::DOUBLE_YELLOW => "flag_double_yellow",
        flag::RED => "flag_red",
        flag::BLACK_ORANGE_CIRCLE => "flag_black_orange",
        _ => return None,
    })
}
fn is_finished_for_good(r: i64) -> bool {
    r == race::FINISHED || r == race::DISQUALIFIED || is_out(r)
}
fn cause(r: u32) -> Option<&'static str> {
    Some(match r {
        reason::SOLO_CRASH => "solo_crash",
        reason::VEHICLE_CRASH => "vehicle_crash",
        reason::VEHICLE_OBSTRUCTION => "obstruction",
        _ => return None,
    })
}
fn fcy_code_of(v: i64) -> Option<&'static str> {
    Some(match v {
        yellow::PIT_LEAD_LAP => "fcy_pits_lead_lap",
        yellow::PITS_OPEN | yellow::PITS_OPEN2 => "fcy_pits_open",
        yellow::LAST_LAP => "fcy_last_lap",
        yellow::RESUME => "fcy_restart",
        yellow::RACE_HALT => "fcy_halt",
        _ => return None,
    })
}
fn penalty_code(v: i64) -> Option<&'static str> {
    Some(match v {
        schedule::DRIVE_THROUGH => "penalty_drive_through",
        schedule::STOP_GO => "penalty_stop_go",
        _ => return None,
    })
}
fn fcy_off(v: i64) -> bool {
    v == yellow::NONE || v == yellow::INVALID
}

/// One event: a code of the catalog, its params, and optional facts for the engineer.
#[derive(Debug, Clone, PartialEq)]
pub struct Announcement {
    /// The catalog code.
    pub code: &'static str,
    /// Its params, as the catalog declares them.
    pub params: Map<String, Value>,
    /// Facts for the engineer (`pitwall.announce` `data`).
    pub data: Option<Map<String, Value>>,
}

/// A debounced value: [`feed`](Self::feed) returns the previous stable value when the value
/// changed (the first stable value returns `Some(None)`: the baseline), else `None`.
#[derive(Debug, Default, Clone)]
struct Stable {
    value: Option<i64>,
    cand: Option<i64>,
    since: f64,
}

impl Stable {
    fn feed(&mut self, v: i64, now: f64, hold: f64) -> Option<Option<i64>> {
        if Some(v) != self.cand {
            self.cand = Some(v);
            self.since = now;
        }
        if self.cand != self.value && now - self.since >= hold {
            let old = self.value;
            self.value = self.cand;
            return Some(old);
        }
        None
    }

    /// Takes `v` as the stable value, silently.
    fn set(&mut self, v: i64) {
        self.value = Some(v);
        self.cand = Some(v);
    }
}

/// A NUL-terminated name, trimmed; empty when it starts with NUL.
fn name_of(raw: &[u8]) -> Cow<'_, str> {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    if end == 0 {
        return Cow::Borrowed("");
    }
    match String::from_utf8_lossy(&raw[..end]) {
        Cow::Borrowed(s) => Cow::Borrowed(s.trim()),
        Cow::Owned(s) => Cow::Owned(s.trim().to_owned()),
    }
}

fn is_safety_car(name: &str, car_class: &str) -> bool {
    name.to_lowercase() == "safety car" || car_class.replace(' ', "").to_lowercase() == "safetycar"
}

type Ident = (String, String);

/// Whose a yellow flag is (PRY-090).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Attribution {
    /// The player ran off and is slow or off the surface.
    SelfOffTrack,
    /// The player is stopped (or crawling) on the surface.
    SelfStopped,
    /// Another participant (index in the arrays) is the likely cause.
    Car(usize),
    /// Nobody could be named.
    Unknown,
}

/// See the module docs. The clock is monotonic seconds; `rival_hz` is how often the
/// per-participant arrays are read.
pub struct EventDetector {
    clock: Box<dyn Clock>,
    rival_period: f64,
    session: Option<u32>,
    game_state: Option<u32>,
    quiet_until: f64,
    last: HashMap<&'static str, f64>,
    once: Vec<&'static str>,
    flag: Stable,
    race_state: Stable,
    schedule: Stable,
    fcy: Stable,
    yellow_armed: bool,
    yellow_said: bool,
    /// A yellow (old, new) edge skipped because the car was at a standstill: said once the car
    /// moves, if the yellow still holds.
    /// The attribution is made at the edge and carried with it (always the player's own).
    yellow_deferred: Option<(i64, i64, Attribution)>,
    /// Off in the parity replay against the Python detector, which never defers (BUG-130).
    defer_yellow: bool,
    /// Off in the parity replay: the Python detector knew nothing of whose a flag is (PRY-090).
    attribute_flags: bool,
    /// The last time the player was on track below [`SELF_SLOW_MS`].
    player_slow_at: f64,
    /// `mLapInvalidated` at the previous tick.
    inval_prev: bool,
    /// The last time `mLapInvalidated` went from 0 to 1.
    inval_rose_at: f64,
    /// The yellow being held was the player's own and closed silently.
    self_yellow: bool,
    clear_since: Option<f64>,
    clear_pending: Option<f64>,
    penalty: Option<i64>,
    penalty_pit: bool,
    fcy_said: Vec<&'static str>,
    fcy_due: Option<(f64, i64)>,
    sector: i32,
    blue_count: HashMap<Option<Ident>, u32>,
    rivals: HashMap<Ident, Stable>,
    rivals_said: Vec<Ident>,
    seen: Vec<Ident>,
    next_rival_scan: f64,
    leader_finished: bool,
    chequered_said: bool,
    player_name: Option<String>,
    view_index: Option<i32>,
    view_ok: bool,
    rebaseline: bool,
}

impl EventDetector {
    /// The Python detector never deferred a standstill yellow: the parity replay turns it off.
    #[cfg(test)]
    pub(crate) fn without_deferred_yellow(mut self) -> Self {
        self.defer_yellow = false;
        self.attribute_flags = false;
        self
    }

    /// A detector reading time from `clock`.
    pub fn new(clock: impl Clock + 'static, rival_hz: f64) -> Self {
        let mut d = Self {
            clock: Box::new(clock),
            rival_period: if rival_hz > 0.0 { 1.0 / rival_hz } else { 0.5 },
            session: None,
            game_state: None,
            quiet_until: 0.0,
            last: HashMap::new(),
            once: Vec::new(),
            flag: Stable::default(),
            race_state: Stable::default(),
            schedule: Stable::default(),
            fcy: Stable::default(),
            yellow_armed: true,
            yellow_said: false,
            yellow_deferred: None,
            defer_yellow: true,
            attribute_flags: true,
            player_slow_at: f64::NEG_INFINITY,
            inval_prev: false,
            inval_rose_at: f64::NEG_INFINITY,
            self_yellow: false,
            clear_since: None,
            clear_pending: None,
            penalty: None,
            penalty_pit: false,
            fcy_said: Vec::new(),
            fcy_due: None,
            sector: 1,
            blue_count: HashMap::new(),
            rivals: HashMap::new(),
            rivals_said: Vec::new(),
            seen: Vec::new(),
            next_rival_scan: 0.0,
            leader_finished: false,
            chequered_said: false,
            player_name: None,
            view_index: None,
            view_ok: false,
            rebaseline: false,
        };
        d.reset();
        d
    }

    /// Forget everything (the game is gone, the mapping was closed).
    pub fn reset(&mut self) {
        self.session = None;
        self.game_state = None;
        self.new_session(0.0, false);
    }

    /// Takes one snapshot; returns what fired on it.
    pub fn update(&mut self, d: &SharedMemory) -> Vec<Announcement> {
        let game = d.game_state;
        let previous = self.game_state;
        self.game_state = Some(game);
        let mut out = Vec::new();
        if game != game_state::INGAME_PLAYING {
            return out;
        }
        let now = self.clock.now();
        let session = d.session_state;
        let resumed = matches!(
            previous,
            Some(
                game_state::INGAME_PLAYING
                    | game_state::INGAME_PAUSED
                    | game_state::INGAME_INMENU_TIME_TICKING
            )
        );
        if Some(session) != self.session || !resumed {
            // A new session state, or a session (re)loaded (not a resume from pause/menu).
            self.session = Some(session);
            self.new_session(now, true);
        }
        let quiet = now < self.quiet_until;
        let race = session == session_state::RACE;

        if now >= self.next_rival_scan {
            self.next_rival_scan = now + self.rival_period;
            self.scan(d, now, quiet, race, &mut out);
        }

        let me = d.viewed_participant_index;
        if Some(me) != self.view_index {
            self.view_index = Some(me);
            self.view_ok = self.is_player(d, me);
            if self.view_ok {
                self.rebaseline = true;
            }
        }
        if self.player_name.is_none() && !quiet {
            self.take_player(d, me);
        }
        if self.view_ok {
            if self.rebaseline {
                self.rebaseline = false;
                self.flag.set(i64::from(d.highest_flag_colour));
                self.race_state.set(i64::from(d.race_state));
                self.schedule.set(i64::from(d.pit_schedule));
                self.inval_prev = d.lap_invalidated != 0;
            } else {
                self.player_fields(d, now, quiet, race, &mut out);
            }
        }

        if session == session_state::RACE || session == session_state::FORMATION_LAP {
            self.caution(d, now, quiet, &mut out);
        }
        out
    }

    // ---------------------------------------------------------------- session

    fn new_session(&mut self, now: f64, quiet: bool) {
        self.quiet_until = if quiet { now + QUIET_S } else { 0.0 };
        self.last.clear();
        self.once.clear();
        self.flag = Stable::default();
        self.race_state = Stable::default();
        self.schedule = Stable::default();
        self.fcy = Stable::default();
        self.yellow_armed = true;
        self.yellow_said = false;
        self.yellow_deferred = None;
        self.player_slow_at = f64::NEG_INFINITY;
        self.inval_prev = false;
        self.inval_rose_at = f64::NEG_INFINITY;
        self.self_yellow = false;
        self.clear_since = None;
        self.clear_pending = None;
        self.penalty = None;
        self.penalty_pit = false;
        self.fcy_said.clear();
        self.fcy_due = None;
        self.sector = 1;
        self.blue_count.clear();
        self.rivals.clear();
        self.rivals_said.clear();
        self.next_rival_scan = now;
        self.leader_finished = false;
        self.chequered_said = false;
        self.player_name = None;
        self.view_index = None;
        self.view_ok = false;
        self.rebaseline = false;
    }

    fn quiet_now(&mut self, now: f64) {
        self.quiet_until = self.quiet_until.max(now + QUIET_S);
    }

    // ---------------------------------------------------------------- the player

    fn take_player(&mut self, d: &SharedMemory, me: i32) {
        if let Some(p) = usize::try_from(me)
            .ok()
            .and_then(|i| d.participant_info.get(i))
        {
            let name = name_of(&p.name);
            if !name.is_empty() {
                self.player_name = Some(name.into_owned());
                self.view_ok = true;
            }
        }
    }

    fn is_player(&self, d: &SharedMemory, me: i32) -> bool {
        let Some(p) = usize::try_from(me)
            .ok()
            .and_then(|i| d.participant_info.get(i))
        else {
            return false;
        };
        match &self.player_name {
            None => true, // not known yet: the viewed car is taken for the player
            Some(n) => name_of(&p.name) == n.as_str(),
        }
    }

    fn player_fields(
        &mut self,
        d: &SharedMemory,
        now: f64,
        mut quiet: bool,
        race: bool,
        out: &mut Vec<Announcement>,
    ) {
        let in_lane = d.pit_mode != pit_mode::NONE;
        let idx = self.view_index.unwrap_or(0);
        let part = usize::try_from(idx)
            .ok()
            .and_then(|i| d.participant_info.get(i));
        if let Some(p) = part
            && p.current_sector >= 0
        {
            self.sector = (p.current_sector + 1).min(3);
        }

        // Race state first: a transition through NOT_STARTED / INVALID silences the rest.
        if let Some(old) = self
            .race_state
            .feed(i64::from(d.race_state), now, HOLD_S.race_state)
        {
            let new = self.race_state.value.unwrap_or(-1);
            let old_unsettled = old.is_some_and(is_unsettled);
            if is_unsettled(new) || old_unsettled {
                self.quiet_now(now);
                quiet = true;
            } else if old.is_some() && !quiet && race {
                if new == race::DISQUALIFIED {
                    self.emit(out, now, "disqualified", None, None);
                } else if is_out(new) {
                    self.emit(out, now, "retired", None, None);
                } else if new == race::FINISHED {
                    self.chequered(d, now, out);
                }
            }
        }

        if !in_lane && d.speed < SELF_SLOW_MS && !quiet {
            self.player_slow_at = now;
        }
        let invalid = d.lap_invalidated != 0;
        if invalid && !self.inval_prev {
            self.inval_rose_at = now;
        }
        self.inval_prev = invalid;

        let old = self
            .flag
            .feed(i64::from(d.highest_flag_colour), now, HOLD_S.flag);
        let stable = self.flag.value;
        if is_clear(stable) {
            let since = *self.clear_since.get_or_insert(now);
            if now - since >= YELLOW_REARM_S {
                self.yellow_armed = true;
            }
        } else {
            self.clear_since = None;
            self.clear_pending = None;
        }
        if let (Some(Some(old)), Some(new), false) = (old, stable, quiet) {
            self.flag_edge(d, old, new, now, in_lane, race, out);
        }
        if let Some((old, new, att)) = self.yellow_deferred {
            if stable != Some(new) {
                self.yellow_deferred = None;
            } else if !quiet && !in_lane && d.speed >= STANDSTILL_MS {
                self.yellow_deferred = None;
                self.yellow_edge(d, old, new, att, now, out);
            }
        }
        if let Some(pending) = self.clear_pending
            && is_clear(stable)
            && now - pending >= CLEAR_HOLD_S
        {
            self.clear_pending = None;
            if !quiet && !in_lane {
                self.emit(out, now, "flag_clear", None, None);
            }
        }

        if in_lane && self.penalty.is_some() {
            self.penalty_pit = true;
        }
        if let Some(Some(old)) = self
            .schedule
            .feed(i64::from(d.pit_schedule), now, HOLD_S.schedule)
        {
            let new = self.schedule.value.unwrap_or(-1);
            if let Some(code) = penalty_code(new) {
                if !quiet && race && Some(new) != self.penalty {
                    self.emit(out, now, code, None, None);
                }
                self.penalty = Some(new);
                self.penalty_pit = in_lane;
            } else if penalty_code(old).is_some() {
                let served = new == schedule::NONE && self.penalty_pit;
                self.penalty = None;
                self.penalty_pit = false;
                if served && !quiet && race {
                    self.emit(out, now, "penalty_served", None, None);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn flag_edge(
        &mut self,
        d: &SharedMemory,
        old: i64,
        new: i64,
        now: f64,
        in_lane: bool,
        race: bool,
        out: &mut Vec<Announcement>,
    ) {
        if is_yellow(old) && is_clear(Some(new)) {
            if self.yellow_said {
                // "clear" only closes a yellow that was announced
                self.clear_pending = Some(now);
            }
            self.yellow_said = false;
            self.self_yellow = false;
            return;
        }
        let pit_lane_filtered = is_yellow(new) || new == flag::BLUE || new == flag::WHITE_SLOW_CAR;
        if pit_lane_filtered && in_lane {
            return;
        }
        let standstill_muted = new == flag::BLUE || new == flag::WHITE_SLOW_CAR;
        if (is_yellow(new) || standstill_muted) && d.speed < STANDSTILL_MS {
            // A contact stops the car right as the game raises the yellow: say it once the car
            // moves, if the yellow still holds (BUG-130).
            if is_yellow(new) && self.defer_yellow {
                let att = self.attribute(d, now);
                self.yellow_deferred = Some((old, new, att));
            }
            return;
        }
        self.yellow_deferred = None;
        if is_yellow(new) {
            let att = self.attribute(d, now);
            self.yellow_edge(d, old, new, att, now, out);
        } else if new == flag::BLUE {
            if race {
                self.blue(d, now, out);
            }
        } else if new == flag::WHITE_FINAL_LAP {
            self.emit(out, now, "final_lap", None, None);
        } else if new == flag::BLACK {
            if race {
                self.emit(out, now, "disqualified", None, None);
            }
        } else if new == flag::CHEQUERED {
            self.chequered(d, now, out);
        } else if new == flag::BLACK_AND_WHITE {
            let track_limits =
                self.attribute_flags && now - self.inval_rose_at <= TRACK_LIMITS_WINDOW_S;
            let code = if track_limits {
                "flag_track_limits"
            } else {
                "flag_black_white"
            };
            let data = self.flag_data(d);
            self.emit(out, now, code, None, Some(data));
        } else if new == flag::WHITE_SLOW_CAR {
            self.slow_car(d, now, out);
        } else if let Some(code) = flag_code(new) {
            let data = self.flag_data(d);
            self.emit(out, now, code, None, Some(data));
        }
    }

    /// The facts every flag call carries: the lap and the player's sector.
    fn flag_data(&self, d: &SharedMemory) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("lap".into(), json!(self.player_lap(d)));
        m.insert("sector".into(), json!(self.sector));
        m
    }

    /// Says a yellow (or the escalation to a double yellow) once, by whose it is.
    fn yellow_edge(
        &mut self,
        d: &SharedMemory,
        old: i64,
        new: i64,
        att: Attribution,
        now: f64,
        out: &mut Vec<Announcement>,
    ) {
        let double = new == flag::DOUBLE_YELLOW;
        let escalation = old == flag::YELLOW && double;
        if !(self.yellow_armed || escalation) {
            return;
        }
        let mut data = self.flag_data(d);
        let reason = cause(d.highest_flag_reason);
        if !self.attribute_flags {
            let code = flag_code(new).expect("a yellow has a code");
            let mut params = Map::new();
            params.insert("sector".into(), json!(self.sector));
            if self.emit(out, now, code, Some(params), Some(data.clone())) {
                self.yellow_armed = false;
                self.yellow_said = true;
                if let Some(c) = reason {
                    let mut params = Map::new();
                    params.insert("cause".into(), json!(c));
                    self.emit(out, now, "flag_yellow_cause", Some(params), Some(data));
                }
            }
            return;
        }
        match att {
            Attribution::SelfOffTrack | Attribution::SelfStopped => {
                if escalation && self.self_yellow {
                    return; // the same incident, already closed silently
                }
                let situation = if att == Attribution::SelfOffTrack {
                    "off_track"
                } else {
                    "stopped"
                };
                data.insert("attribution".into(), json!("self"));
                let mut params = Map::new();
                params.insert("situation".into(), json!(situation));
                if self.emit(out, now, "flag_yellow_self", Some(params), Some(data)) {
                    self.yellow_armed = false;
                    self.self_yellow = true;
                }
            }
            Attribution::Car(i) => {
                let Some(name) = self.rival_ok(d, i) else {
                    return;
                };
                let name = name.into_owned();
                let incident = (d.participant_info[i].current_sector + 1).clamp(1, 3);
                data.insert("attribution".into(), json!("car"));
                data.insert("incident_sector".into(), json!(incident));
                data.insert("driver".into(), json!(name));
                let code = if double {
                    "flag_double_yellow"
                } else {
                    "flag_yellow"
                };
                let mut params = Map::new();
                params.insert("sector".into(), json!(incident));
                if self.emit(out, now, code, Some(params), Some(data.clone())) {
                    self.yellow_armed = false;
                    self.yellow_said = true;
                    self.self_yellow = false;
                    let mut params = Map::new();
                    params.insert("driver".into(), json!(name));
                    params.insert("cause".into(), json!(reason.unwrap_or("unknown")));
                    self.emit(out, now, "flag_yellow_driver", Some(params), Some(data));
                }
            }
            Attribution::Unknown => {
                data.insert("attribution".into(), json!("unknown"));
                let code = if double {
                    "flag_double_yellow_ahead"
                } else {
                    "flag_yellow_ahead"
                };
                if self.emit(out, now, code, None, Some(data.clone())) {
                    self.yellow_armed = false;
                    self.yellow_said = true;
                    self.self_yellow = false;
                    if let Some(c) = reason {
                        let mut params = Map::new();
                        params.insert("cause".into(), json!(c));
                        self.emit(out, now, "flag_yellow_cause", Some(params), Some(data));
                    }
                }
            }
        }
    }

    /// Whose a yellow is: the player's own recent speed first, then the other cars.
    fn attribute(&self, d: &SharedMemory, now: f64) -> Attribution {
        if !self.attribute_flags {
            return Attribution::Unknown;
        }
        let off_track = d.crash_state != 0;
        if self.player_is_slow(now) {
            return if off_track {
                Attribution::SelfOffTrack
            } else {
                Attribution::SelfStopped
            };
        }
        let me = usize::try_from(self.view_index.unwrap_or(0)).unwrap_or(usize::MAX);
        if let Some(i) = self.cause_car(d, me) {
            Attribution::Car(i)
        } else if off_track {
            Attribution::SelfOffTrack
        } else {
            Attribution::Unknown
        }
    }

    fn player_is_slow(&self, now: f64) -> bool {
        now - self.player_slow_at <= SELF_WINDOW_S
    }

    /// The name of participant `i` when it is a rival worth naming: active, with a name, not
    /// the player, not the Safety Car.
    fn rival_ok<'a>(&self, d: &'a SharedMemory, i: usize) -> Option<Cow<'a, str>> {
        let q = d.participant_info.get(i)?;
        if q.is_active == 0 {
            return None;
        }
        let name = name_of(&q.name);
        if name.is_empty() || self.player_name.as_deref() == Some(&*name) {
            return None;
        }
        let car_class = name_of(&d.car_class_names[i]);
        if is_safety_car(&name, &car_class) {
            return None;
        }
        Some(name)
    }

    /// The gap in metres from the player (`me`) to participant `i` along the track, wrapped to
    /// (-length/2, length/2]; positive: ahead.
    fn gap_to(d: &SharedMemory, me: usize, i: usize, length: f64) -> f64 {
        let delta = f64::from(d.participant_info[i].current_lap_distance)
            - f64::from(d.participant_info[me].current_lap_distance);
        let mut g = delta.rem_euclid(length);
        if g > length / 2.0 {
            g -= length;
        }
        g
    }

    /// The most likely cause of a yellow among the other cars: slow on track near the player,
    /// preferring the one that carries a yellow itself, then the nearest (ahead first).
    fn cause_car(&self, d: &SharedMemory, me: usize) -> Option<usize> {
        let length = f64::from(d.track_length);
        let n = usize::try_from(d.num_participants)
            .unwrap_or(0)
            .min(d.participant_info.len());
        if length <= 0.0 || me >= n {
            return None;
        }
        let mut best: Option<(usize, bool, f64)> = None;
        for i in 0..n {
            if i == me
                || d.pit_modes[i] != pit_mode::NONE
                || d.speeds[i] >= CAUSE_SLOW_MS
                || is_finished_for_good(i64::from(d.race_states[i]))
            {
                continue;
            }
            let g = Self::gap_to(d, me, i, length);
            if !(-CAUSE_BEHIND_M..=CAUSE_AHEAD_M).contains(&g) {
                continue;
            }
            if self.rival_ok(d, i).is_none() {
                continue;
            }
            let own = is_yellow(i64::from(d.highest_flag_colours[i]));
            let cost = if g >= 0.0 { g } else { -2.0 * g };
            let better = match best {
                None => true,
                Some((_, b_own, b_cost)) => (own && !b_own) || (own == b_own && cost < b_cost),
            };
            if better {
                best = Some((i, own, cost));
            }
        }
        best.map(|(i, _, _)| i)
    }

    /// The white flag: a slow car ahead. Silent when the player is the slow one.
    fn slow_car(&mut self, d: &SharedMemory, now: f64, out: &mut Vec<Announcement>) {
        let data = self.flag_data(d);
        if !self.attribute_flags {
            self.emit(out, now, "flag_slow_car", None, Some(data));
            return;
        }
        if self.player_is_slow(now) {
            return;
        }
        let length = f64::from(d.track_length);
        let n = usize::try_from(d.num_participants)
            .unwrap_or(0)
            .min(d.participant_info.len());
        let me = usize::try_from(self.view_index.unwrap_or(0)).unwrap_or(usize::MAX);
        let mut who: Option<(String, f64)> = None;
        if length > 0.0 && me < n {
            for i in 0..n {
                if i == me
                    || d.pit_modes[i] != pit_mode::NONE
                    || d.speeds[i] >= SLOW_CAR_RATIO * d.speed
                {
                    continue;
                }
                let g = Self::gap_to(d, me, i, length);
                if g > 0.0
                    && g <= SLOW_CAR_RANGE_M
                    && who.as_ref().is_none_or(|(_, best)| g < *best)
                    && let Some(name) = self.rival_ok(d, i)
                {
                    who = Some((name.into_owned(), g));
                }
            }
        }
        match who {
            Some((name, _)) => {
                let mut params = Map::new();
                params.insert("driver".into(), json!(name));
                self.emit(out, now, "flag_slow_car_driver", Some(params), Some(data));
            }
            None => {
                self.emit(out, now, "flag_slow_car", None, Some(data));
            }
        }
    }

    fn player_lap(&self, d: &SharedMemory) -> u32 {
        usize::try_from(self.view_index.unwrap_or(0))
            .ok()
            .and_then(|i| d.participant_info.get(i))
            .map_or(0, |p| p.current_lap)
    }

    fn chequered(&mut self, d: &SharedMemory, now: f64, out: &mut Vec<Announcement>) {
        if self.chequered_said {
            return;
        }
        self.chequered_said = true;
        let pos = usize::try_from(self.view_index.unwrap_or(0))
            .ok()
            .and_then(|i| d.participant_info.get(i))
            .map_or(0, |p| p.race_position);
        if pos >= 1 {
            let mut params = Map::new();
            params.insert("position".into(), json!(pos));
            self.emit(out, now, "chequered", Some(params), None);
        } else {
            self.emit(out, now, "chequered_no_position", None, None);
        }
    }

    /// The nearest car behind on track that is ahead in the race (lapping us).
    fn blue(&mut self, d: &SharedMemory, now: f64, out: &mut Vec<Announcement>) {
        let me = self.view_index.unwrap_or(0);
        let length = f64::from(d.track_length);
        let n = usize::try_from(d.num_participants)
            .unwrap_or(0)
            .min(d.participant_info.len());
        let mut who: Option<Ident> = None;
        if length > 0.0
            && let Ok(me) = usize::try_from(me)
            && me < n
        {
            let p = &d.participant_info[me];
            let mine = f64::from(p.current_lap) * length + f64::from(p.current_lap_distance);
            let mut best = BLUE_RANGE_M;
            for i in 0..n {
                if i == me {
                    continue;
                }
                let q = &d.participant_info[i];
                let Some(name) = self.rival_ok(d, i) else {
                    continue;
                };
                let car_class = name_of(&d.car_class_names[i]);
                let gap = (f64::from(p.current_lap_distance) - f64::from(q.current_lap_distance))
                    .rem_euclid(length);
                let theirs = f64::from(q.current_lap) * length + f64::from(q.current_lap_distance);
                if 0.0 < gap && gap < best && theirs > mine {
                    best = gap;
                    who = Some((name.into_owned(), car_class.into_owned()));
                }
            }
        }
        let count = self.blue_count.get(&who).copied().unwrap_or(0);
        if count >= BLUE_MAX_PER_CAR {
            return;
        }
        let sent = match &who {
            Some((name, _)) => {
                let mut params = Map::new();
                params.insert("driver".into(), json!(name));
                self.emit(out, now, "flag_blue", Some(params), None)
            }
            None => self.emit(out, now, "flag_blue_unknown", None, None),
        };
        if sent {
            self.blue_count.insert(who, count + 1);
        }
    }

    // ---------------------------------------------------------------- the caution

    fn caution(&mut self, d: &SharedMemory, now: f64, quiet: bool, out: &mut Vec<Announcement>) {
        let value = match d.yellow_flag_state {
            v @ -1..=8 => i64::from(v),
            _ => yellow::INVALID,
        };
        if let Some(old) = self.fcy.feed(value, now, HOLD_S.fcy) {
            let new = self.fcy.value.unwrap_or(yellow::INVALID);
            if old.is_none() || quiet {
                if fcy_off(new) {
                    self.fcy_said.clear();
                }
                self.fcy_due = None;
            } else if fcy_off(new) {
                self.fcy_said.clear();
                self.fcy_due = None;
            } else if old.is_some_and(fcy_off) {
                self.fcy_said.clear();
                if new == yellow::RACE_HALT {
                    self.fcy_code(new, now, out);
                } else {
                    self.fcy_say(out, now, "fcy_start");
                    if fcy_code_of(new).is_some() {
                        // skipped the intermediate states (CrewChief)
                        self.fcy_due = Some((now + FCY_JUMP_DELAY_S, new));
                    }
                }
            } else {
                self.fcy_due = None;
                self.fcy_code(new, now, out);
            }
        }
        if let Some((when, state)) = self.fcy_due
            && now >= when
        {
            self.fcy_due = None;
            if self.fcy.value == Some(state) && !quiet {
                self.fcy_code(state, now, out);
            }
        }
    }

    fn fcy_code(&mut self, value: i64, now: f64, out: &mut Vec<Announcement>) {
        if let Some(code) = fcy_code_of(value) {
            self.fcy_say(out, now, code);
        }
    }

    fn fcy_say(&mut self, out: &mut Vec<Announcement>, now: f64, code: &'static str) {
        if !self.fcy_said.contains(&code) {
            // once per caution period
            self.fcy_said.push(code);
            self.emit(out, now, code, None, None);
        }
    }

    // ---------------------------------------------------------------- rivals (rival_hz)

    fn scan(
        &mut self,
        d: &SharedMemory,
        now: f64,
        quiet: bool,
        race: bool,
        out: &mut Vec<Announcement>,
    ) {
        let n = usize::try_from(d.num_participants)
            .unwrap_or(0)
            .min(d.participant_info.len());
        let me = d.viewed_participant_index;
        if self.player_name.is_some() && Some(me) == self.view_index {
            // The arrays may be reshuffled under the same index: check the name again.
            let ok = self.is_player(d, me);
            if ok && !self.view_ok {
                self.rebaseline = true;
            }
            self.view_ok = ok;
        }
        let mut leader_finished = false;
        let mut leader_lap = 0u32;
        self.seen.clear();
        let mut changed: Vec<(Ident, i64)> = Vec::new();
        for i in 0..n {
            let p = &d.participant_info[i];
            if p.is_active == 0 {
                continue;
            }
            if p.race_position == 1 {
                leader_lap = p.current_lap;
                leader_finished = i64::from(d.race_states[i]) == race::FINISHED;
            }
            if !race || usize::try_from(me).is_ok_and(|m| m == i) {
                continue;
            }
            let name = name_of(&p.name);
            if name.is_empty() || self.player_name.as_deref() == Some(&*name) {
                continue;
            }
            let car_class = name_of(&d.car_class_names[i]);
            if is_safety_car(&name, &car_class) {
                continue;
            }
            let ident: Ident = (name.into_owned(), car_class.into_owned());
            if self.seen.contains(&ident) {
                continue; // a duplicate entry of the same car
            }
            self.seen.push(ident.clone());
            let st = self.rivals.entry(ident.clone()).or_default();
            if let Some(Some(_)) = st.feed(i64::from(d.race_states[i]), now, HOLD_S.race_state) {
                changed.push((ident, st.value.unwrap_or(-1)));
            }
        }
        self.leader_finished = leader_finished;
        if quiet || leader_finished {
            return;
        }
        for (ident, state) in changed {
            if self.rivals_said.contains(&ident) {
                continue;
            }
            let code = if state == race::DISQUALIFIED {
                "rival_disqualified"
            } else if is_out(state) {
                "rival_out"
            } else {
                continue;
            };
            let mut params = Map::new();
            params.insert("driver".into(), json!(ident.0));
            let mut data = Map::new();
            data.insert("car_class".into(), json!(ident.1));
            self.rivals_said.push(ident);
            self.emit(out, now, code, Some(params), Some(data));
        }
        let laps = d.laps_in_event;
        if race
            && laps > 0
            && d.session_duration <= 0.0
            && leader_lap >= laps
            && !is_unsettled(i64::from(d.race_state))
        {
            self.emit(out, now, "final_lap", None, None);
        }
    }

    // ---------------------------------------------------------------- emit

    fn emit(
        &mut self,
        out: &mut Vec<Announcement>,
        now: f64,
        code: &'static str,
        params: Option<Map<String, Value>>,
        data: Option<Map<String, Value>>,
    ) -> bool {
        if ONCE.contains(&code) {
            if self.once.contains(&code) {
                return false;
            }
            self.once.push(code);
        }
        let cool = cooldown(code);
        if cool > 0.0
            && let Some(&last) = self.last.get(code)
            && now - last < cool
        {
            return false;
        }
        self.last.insert(code, now);
        out.push(Announcement {
            code,
            params: params.unwrap_or_default(),
            data,
        });
        true
    }
}
