//! The spotter: "Car left", "Car right", "In the middle", "Still right / left / middle", "Clear left /
//! right", "All clear", from the other cars' world positions in every frame.
//!
//! AMS2 has no proximity field: the geometry is CrewChief's, over `mWorldPosition`,
//! `mOrientation` and `mSpeeds` (research: relative positions jitter by
//! 7 mm p99, so no onset debounce is needed). Pure logic like the detector: the clock comes from
//! outside, nothing allocates, [`Spotter::update`] returns the code to send or none.
//!
//! * **Geometry.** In the player's frame (`theta = -yaw`) a car is `x'` to the left (positive)
//!   and `z'` behind (positive). A new overlap on a side: `|z'| < CAR_LENGTH` ahead,
//!   `z' < CAR_LENGTH + BEHIND` behind, and `|x'| > CAR_WIDTH` (alongside, not nose to tail);
//!   an overlap already held stays while `|z'| < CAR_LENGTH + GAP` (hysteresis). Two cars on one
//!   side count as one.
//! * **State.** One state machine (left 0/1, right 0/1) of what the driver was told; each change
//!   sends one call. A side empty for [`CLEAR_DELAY_S`] clears; an overlap back on a side within
//!   [`BOUNCE_S`] of its clear is called only if it still holds then; a side-specific still call every
//!   [`STILL_THERE_S`] after the last call while a called overlap holds; "All clear" instead of
//!   "Clear left / right" after "In the middle".
//! * **Gates.** Practice, test, qualifying and race (not time attack or the formation lap);
//!   not a private session; the player on track at [`MIN_SPEED`] or more; in a race from
//!   [`RACE_DELAY_S`] after the green; not under a full-course yellow. When a gate closes the
//!   state is reset silently (no "clear" on pit entry or at a standstill).

use super::detector::race;
use crate::codes::{pit_mode, session_state};
use ams2_shm::SharedMemory;

/// A car's length, metres (AMS2 does not expose the real one).
pub const CAR_LENGTH: f32 = 4.4;
/// A car's width, metres.
pub const CAR_WIDTH: f32 = 1.8;
/// Extra length an existing overlap keeps (hysteresis).
pub const CLEAR_GAP: f32 = 0.5;
/// Extra length a car behind needs to count as new (a nose that just passed).
pub const BEHIND_ALLOWANCE: f32 = 0.4;
/// Cars further than this on either axis are not looked at.
pub const SEARCH_RANGE: f32 = 20.0;
/// Closing speed (m/s) from which a car is passing, not alongside.
pub const MAX_CLOSING_SPEED: f32 = 12.0;
/// The player's speed (m/s) under which the spotter is silent.
pub const MIN_SPEED: f32 = 10.0;
/// A side must stay empty this long before its clear is called.
pub const CLEAR_DELAY_S: f64 = 0.15;
/// An overlap back within this long of a side's clear is not called at once.
pub const BOUNCE_S: f64 = 1.5;
/// A side-specific still call every this long since the last call.
pub const STILL_THERE_S: f64 = 3.0;
/// In a race, silent until this long after the green.
pub const RACE_DELAY_S: f64 = 20.0;
/// `mRaceState` racing.
const RACING: u32 = race::RACING as u32;

/// The codes (catalog).
pub const CAR_LEFT: &str = "spotter_car_left";
/// See [`CAR_LEFT`].
pub const CAR_RIGHT: &str = "spotter_car_right";
/// See [`CAR_LEFT`].
pub const IN_THE_MIDDLE: &str = "spotter_in_the_middle";
/// See [`CAR_LEFT`].
pub const STILL_RIGHT: &str = "spotter_still_right";
/// See [`CAR_LEFT`].
pub const STILL_LEFT: &str = "spotter_still_left";
/// See [`CAR_LEFT`].
pub const STILL_MIDDLE: &str = "spotter_still_middle";
/// See [`CAR_LEFT`].
pub const CLEAR_LEFT: &str = "spotter_clear_left";
/// See [`CAR_LEFT`].
pub const CLEAR_RIGHT: &str = "spotter_clear_right";
/// See [`CAR_LEFT`].
pub const ALL_CLEAR: &str = "spotter_all_clear";

/// Wordings of [`STILL_RIGHT`] (its text and two variants); the catalog declares the same.
pub const STILL_RIGHT_VARIANTS: u8 = 3;
/// Wordings of [`STILL_LEFT`].
pub const STILL_LEFT_VARIANTS: u8 = 3;
/// Wordings of [`STILL_MIDDLE`].
pub const STILL_MIDDLE_VARIANTS: u8 = 2;

/// A call: the catalog code and which of its wordings to say (0: the `text`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Call {
    /// The catalog code.
    pub code: &'static str,
    /// 0 for the `text`, 1.. for the `variants`.
    pub variant: u8,
}

impl Call {
    fn plain(code: &'static str) -> Self {
        Self { code, variant: 0 }
    }
}

/// splitmix64 step.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// One side of the player.
#[derive(Debug, Default, Clone, Copy)]
struct Side {
    /// A car overlaps (its clear delay included).
    tracked: bool,
    /// The driver was told.
    called: bool,
    /// Empty since (while `tracked`).
    empty_since: Option<f64>,
    /// No call before this time (after the side's clear).
    bounce_until: f64,
}

impl Side {
    /// Follows the geometry: whether the side is occupied now.
    fn follow(&mut self, occupied: bool, now: f64) {
        if occupied {
            self.tracked = true;
            self.empty_since = None;
            return;
        }
        if !self.tracked {
            return;
        }
        let since = *self.empty_since.get_or_insert(now);
        if now - since < CLEAR_DELAY_S {
            return;
        }
        self.tracked = false;
        self.empty_since = None;
        if self.called {
            self.bounce_until = now + BOUNCE_S;
        }
    }

    /// What the driver should be told about this side now.
    fn wanted(&self, now: f64) -> bool {
        self.tracked && now >= self.bounce_until
    }
}

/// The spotter's state. See the module docs.
#[derive(Debug, Default, Clone)]
pub struct Spotter {
    left: Side,
    right: Side,
    /// "In the middle" was said and the sides have not been both clear since.
    middle_said: bool,
    /// The time of the last call (for the still calls).
    last_call: f64,
    /// The race went green at this time (race sessions).
    green_at: Option<f64>,
    /// splitmix64 state of the variant choice.
    rng: u64,
    /// The last variant said per still code (right, left, middle).
    last_variant: [u8; 3],
}

impl Spotter {
    /// A spotter with nothing tracked.
    pub fn new() -> Self {
        Self::default()
    }

    /// A spotter whose variant choice starts from `seed` (any value, 0 included).
    pub fn seeded(seed: u64) -> Self {
        Self {
            rng: seed,
            ..Self::default()
        }
    }

    /// The variant of a still call: uniform over the wordings but the last one said.
    fn pick(&mut self, slot: usize, n: u8) -> u8 {
        if n <= 1 {
            return 0;
        }
        let r = (splitmix64(&mut self.rng) % u64::from(n - 1)) as u8;
        let v = if r >= self.last_variant[slot] {
            r + 1
        } else {
            r
        };
        self.last_variant[slot] = v;
        v
    }

    /// Forgets everything, silently.
    pub fn reset(&mut self) {
        *self = Self {
            rng: self.rng,
            last_variant: self.last_variant,
            ..Self::default()
        };
    }

    /// Whether the spotter has anything tracked (for tests).
    pub fn idle(&self) -> bool {
        !self.left.tracked && !self.right.tracked && !self.left.called && !self.right.called
    }

    /// One frame at `now`. `view_ok`: the viewed car is the player; `quiet`: the detector's
    /// quiet time after a session load. Returns the code to send, or none.
    pub fn update(
        &mut self,
        d: &SharedMemory,
        now: f64,
        view_ok: bool,
        quiet: bool,
    ) -> Option<Call> {
        if !self.gates(d, now, view_ok, quiet) {
            let green_at = self.green_at;
            self.reset();
            self.green_at = green_at;
            return None;
        }
        let (on_left, on_right) = self.scan(d);
        self.left.follow(on_left, now);
        self.right.follow(on_right, now);
        let (want_l, want_r) = (self.left.wanted(now), self.right.wanted(now));
        let (was_l, was_r) = (self.left.called, self.right.called);
        self.left.called = want_l;
        self.right.called = want_r;
        let code = match (was_l, was_r, want_l, want_r) {
            (a, b, c, e) if (a, b) == (c, e) => {
                if (c || e) && now - self.last_call >= STILL_THERE_S {
                    let (code, slot, n) = match (c, e) {
                        (true, true) => (STILL_MIDDLE, 2, STILL_MIDDLE_VARIANTS),
                        (true, false) => (STILL_LEFT, 1, STILL_LEFT_VARIANTS),
                        _ => (STILL_RIGHT, 0, STILL_RIGHT_VARIANTS),
                    };
                    let variant = self.pick(slot, n);
                    return self.finish(Call { code, variant }, now);
                }
                return None;
            }
            (_, _, true, true) => {
                self.middle_said = true;
                IN_THE_MIDDLE
            }
            (_, _, true, false) => CAR_LEFT,
            (_, _, false, true) => CAR_RIGHT,
            (_, _, false, false) => {
                let all = self.middle_said || (was_l && was_r);
                self.middle_said = false;
                if all {
                    ALL_CLEAR
                } else if was_l {
                    CLEAR_LEFT
                } else {
                    CLEAR_RIGHT
                }
            }
        };
        self.finish(Call::plain(code), now)
    }

    fn finish(&mut self, call: Call, now: f64) -> Option<Call> {
        self.last_call = now;
        Some(call)
    }

    /// The gates of the module docs.
    fn gates(&mut self, d: &SharedMemory, now: f64, view_ok: bool, quiet: bool) -> bool {
        let session = d.session_state;
        let race = session == session_state::RACE;
        if race && d.race_state == RACING {
            self.green_at.get_or_insert(now);
        } else {
            self.green_at = None;
        }
        if !view_ok || quiet || d.session_is_private != 0 {
            return false;
        }
        let kind_ok = matches!(
            session,
            session_state::PRACTICE
                | session_state::TEST
                | session_state::QUALIFY
                | session_state::RACE
        );
        if !kind_ok || d.pit_mode != pit_mode::NONE || d.speed < MIN_SPEED {
            return false;
        }
        // Full-course yellow (and the safety car's whole sequence): anything but none / invalid.
        if d.yellow_flag_state > 0 {
            return false;
        }
        if race {
            return self.green_at.is_some_and(|g| now - g >= RACE_DELAY_S);
        }
        true
    }

    /// The geometry: whether a car overlaps on the left and on the right. Allocates nothing.
    fn scan(&self, d: &SharedMemory) -> (bool, bool) {
        let me = d.viewed_participant_index;
        let n = usize::try_from(d.num_participants)
            .unwrap_or(0)
            .min(d.participant_info.len());
        let Some(mine) = usize::try_from(me)
            .ok()
            .and_then(|i| d.participant_info.get(i))
        else {
            return (false, false);
        };
        let theta = -d.orientation[1];
        let (s, c) = theta.sin_cos();
        let (px, pz) = (mine.world_position[0], mine.world_position[2]);
        let (mut left, mut right) = (false, false);
        for i in 0..n {
            let p = &d.participant_info[i];
            if p.is_active == 0 || usize::try_from(me).is_ok_and(|m| m == i) {
                continue;
            }
            if d.pit_modes[i] != pit_mode::NONE
                || (d.speed - d.speeds[i]).abs() >= MAX_CLOSING_SPEED
            {
                continue;
            }
            let (dx, dz) = (p.world_position[0] - px, p.world_position[2] - pz);
            if dx.abs() > SEARCH_RANGE || dz.abs() > SEARCH_RANGE {
                continue;
            }
            let x = c * dx + s * dz;
            let z = c * dz - s * dx;
            let is_left = x > 0.0;
            let side = if is_left { &self.left } else { &self.right };
            let hit = if side.tracked {
                z.abs() < CAR_LENGTH + CLEAR_GAP
            } else {
                let along = if z < 0.0 {
                    -z < CAR_LENGTH
                } else {
                    z < CAR_LENGTH + BEHIND_ALLOWANCE
                };
                along && x.abs() > CAR_WIDTH
            };
            if hit {
                if is_left {
                    left = true;
                } else {
                    right = true;
                }
                if left && right {
                    break;
                }
            }
        }
        (left, right)
    }
}
