//! The session clock: the few fields of the shared memory that tell whether two moments belong to
//! the same game session (BUG-128).
//!
//! A recorder that stops while the game runs leaves the clock of its last frame in
//! `session-resume.json`; the next recorder, and the control app while no recorder runs, compare
//! it with the game's current one. Fields (from `SharedMemory.h` v14 and `docs/FINDINGS.md`):
//!
//! * `mGameState`: front end / exited mean the session is over (the driver went to the menus).
//! * `mSessionState` and a hash of `mCarName`, `mTrackLocation`, `mTrackVariation`: the kind of
//!   session, car and place must be identical.
//! * `mEventTimeRemaining` (seconds, despite the header; `-1` unset): counts down while a timed
//!   session runs and stands still while paused or in the garage menu. In the same session it can
//!   only be lower than or equal to its value at the stop; a new session of the same kind starts
//!   again at the session total, higher.
//! * `mLapsInEvent` and `mSessionDuration` (the scheduled length): fixed for a session.
//! * The player's `mLapsCompleted` (`participant_info[mViewedParticipantIndex]`): only grows in a
//!   session; a new one starts at 0.
//!
//! Not decidable from these: a new session of the same kind started with no time elapsed and no
//! lap in between (both clocks read the same). That case is resumed.

use crate::{Mapping, ParticipantInfo, SharedMemory};
use core::mem::{offset_of, size_of};

/// `GAME_EXITED` of `mGameState`.
const GAME_EXITED: u32 = 0;
/// `GAME_FRONT_END`.
const GAME_FRONT_END: u32 = 1;
/// `GAME_FRONT_END_REPLAY`.
const GAME_FRONT_END_REPLAY: u32 = 7;
/// How much the remaining time may rise between two readings of one session, in seconds.
pub const REMAINING_TOLERANCE_S: f32 = 2.0;

/// What the clock says of the game right now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionClock {
    /// `mGameState`.
    pub game_state: u32,
    /// `mSessionState`.
    pub session_state: u32,
    /// [`identity`] of car, track and layout.
    pub identity: u64,
    /// `mEventTimeRemaining`, s; negative when unset.
    pub time_remaining_s: f32,
    /// `mLapsInEvent`; 0 when unset.
    pub laps_in_event: u32,
    /// `mSessionDuration`, minutes; 0 when unset.
    pub duration_min: f32,
    /// The viewed participant's `mLapsCompleted`.
    pub laps_completed: u32,
}

/// What the control app does with a stored session while no recorder runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The same session may still continue.
    Keep,
    /// The session is over or another one began: forget the stored one.
    Drop,
}

fn text(b: &[u8]) -> &[u8] {
    &b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())]
}

/// FNV-1a over car, track and layout names (the bytes up to the first NUL of each).
pub fn identity(car: &[u8], track: &[u8], variation: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for part in [car, track, variation] {
        for &c in text(part).iter().chain(&[0]) {
            h = (h ^ u64::from(c)).wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

impl SessionClock {
    /// The clock of a snapshot.
    pub fn from_frame(f: &SharedMemory) -> Self {
        let laps = usize::try_from(f.viewed_participant_index)
            .ok()
            .and_then(|i| f.participant_info.get(i))
            .map_or(0, |p| p.laps_completed);
        Self {
            game_state: f.game_state,
            session_state: f.session_state,
            identity: identity(&f.car_name, &f.track_location, &f.track_variation),
            time_remaining_s: f.event_time_remaining,
            laps_in_event: f.laps_in_event,
            duration_min: f.session_duration,
            laps_completed: laps,
        }
    }

    /// Reads the clock straight from the mapping: a dozen fields, no frame copy. `None` when the
    /// game was writing (the sequence number was odd or changed): ask again later.
    pub fn read(m: &Mapping) -> Option<Self> {
        let seq = m.sequence();
        if seq % 2 == 1 {
            return None;
        }
        let viewed: i32 = m.read_field(offset_of!(SharedMemory, viewed_participant_index));
        let laps = match usize::try_from(viewed) {
            Ok(i) if i < 64 => m.read_field(
                offset_of!(SharedMemory, participant_info)
                    + i * size_of::<ParticipantInfo>()
                    + offset_of!(ParticipantInfo, laps_completed),
            ),
            _ => 0,
        };
        let car: [u8; 64] = m.read_field(offset_of!(SharedMemory, car_name));
        let track: [u8; 64] = m.read_field(offset_of!(SharedMemory, track_location));
        let var: [u8; 64] = m.read_field(offset_of!(SharedMemory, track_variation));
        let clock = Self {
            game_state: m.read_field(offset_of!(SharedMemory, game_state)),
            session_state: m.read_field(offset_of!(SharedMemory, session_state)),
            identity: identity(&car, &track, &var),
            time_remaining_s: m.read_field(offset_of!(SharedMemory, event_time_remaining)),
            laps_in_event: m.read_field(offset_of!(SharedMemory, laps_in_event)),
            duration_min: m.read_field(offset_of!(SharedMemory, session_duration)),
            laps_completed: laps,
        };
        (m.sequence() == seq).then_some(clock)
    }

    /// The game is in the front end (menus) or gone.
    pub fn in_menus(&self) -> bool {
        matches!(
            self.game_state,
            GAME_EXITED | GAME_FRONT_END | GAME_FRONT_END_REPLAY
        )
    }

    /// `now` can be the same session as `self` (the clock at the stop) continuing: same kind,
    /// car, place and scheduled length, the remaining time not higher (a timed session that
    /// went on has less left; one that began again has the total) and no lap fewer.
    pub fn continues_as(&self, now: &Self) -> bool {
        let timed = self.time_remaining_s >= 0.0;
        self.session_state == now.session_state
            && self.identity == now.identity
            && self.laps_in_event == now.laps_in_event
            && self.duration_min.to_bits() == now.duration_min.to_bits()
            && now.laps_completed >= self.laps_completed
            && (!timed
                || (now.time_remaining_s >= 0.0
                    && now.time_remaining_s <= self.time_remaining_s + REMAINING_TOLERANCE_S))
    }

    /// The control app's decision for a stored session given the game now.
    pub fn watch(&self, now: &Self) -> Verdict {
        if now.in_menus() || !self.continues_as(now) {
            Verdict::Drop
        } else {
            Verdict::Keep
        }
    }

    /// For `session-resume.json`.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "game_state": self.game_state, "session_state": self.session_state,
            "identity": self.identity, "time_remaining_s": f64::from(self.time_remaining_s),
            "laps_in_event": self.laps_in_event, "duration_min": f64::from(self.duration_min),
            "laps_completed": self.laps_completed,
        })
    }

    /// Reads [`to_json`](Self::to_json) back; `None` when a field is missing or malformed.
    pub fn from_json(v: &serde_json::Value) -> Option<Self> {
        let u = |k: &str| v.get(k)?.as_u64();
        let f = |k: &str| v.get(k)?.as_f64();
        Some(Self {
            game_state: u32::try_from(u("game_state")?).ok()?,
            session_state: u32::try_from(u("session_state")?).ok()?,
            identity: u("identity")?,
            time_remaining_s: f("time_remaining_s")? as f32,
            laps_in_event: u32::try_from(u("laps_in_event")?).ok()?,
            duration_min: f("duration_min")? as f32,
            laps_completed: u32::try_from(u("laps_completed")?).ok()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn practice() -> SessionClock {
        SessionClock {
            game_state: 2,
            session_state: 1,
            identity: identity(b"Car", b"Track", b"GP"),
            time_remaining_s: 1500.0,
            laps_in_event: 0,
            duration_min: 30.0,
            laps_completed: 3,
        }
    }

    #[test]
    fn the_same_session_after_a_stay_in_the_garage_continues() {
        let stop = practice();
        let garage = SessionClock {
            game_state: 4,
            ..stop
        };
        assert!(stop.continues_as(&garage));
        assert_eq!(stop.watch(&garage), Verdict::Keep);
        let later = SessionClock {
            time_remaining_s: 900.0,
            laps_completed: 6,
            ..stop
        };
        assert!(stop.continues_as(&later));
    }

    #[test]
    fn a_new_session_of_the_same_kind_does_not() {
        let stop = practice();
        let reset = SessionClock {
            time_remaining_s: 1800.0,
            laps_completed: 0,
            ..stop
        };
        assert!(!stop.continues_as(&reset), "clock back at the total");
        let no_laps = SessionClock {
            laps_completed: 0,
            ..stop
        };
        assert!(!stop.continues_as(&no_laps), "laps back to 0");
        let unset = SessionClock {
            time_remaining_s: -1.0,
            ..stop
        };
        assert!(!stop.continues_as(&unset));
        let other = SessionClock {
            identity: identity(b"Car", b"Track", b"Short"),
            ..stop
        };
        assert!(!stop.continues_as(&other));
        let longer = SessionClock {
            duration_min: 45.0,
            ..stop
        };
        assert!(!stop.continues_as(&longer));
    }

    #[test]
    fn menus_and_changes_drop_the_stored_session() {
        let stop = practice();
        for gs in [0, 1, 7] {
            let menus = SessionClock {
                game_state: gs,
                ..stop
            };
            assert_eq!(stop.watch(&menus), Verdict::Drop, "game state {gs}");
        }
        let other_kind = SessionClock {
            session_state: 3,
            ..stop
        };
        assert_eq!(stop.watch(&other_kind), Verdict::Drop);
        let restarted = SessionClock {
            time_remaining_s: 1790.0,
            laps_completed: 0,
            ..stop
        };
        assert_eq!(stop.watch(&restarted), Verdict::Drop);
    }

    #[test]
    fn a_lap_limited_session_uses_the_laps_only() {
        let stop = SessionClock {
            time_remaining_s: -1.0,
            duration_min: 0.0,
            laps_in_event: 10,
            ..practice()
        };
        assert!(stop.continues_as(&SessionClock {
            laps_completed: 4,
            ..stop
        }));
        assert!(!stop.continues_as(&SessionClock {
            laps_completed: 0,
            ..stop
        }));
    }

    #[test]
    fn the_clock_of_a_frame_reads_the_viewed_participant_and_round_trips() {
        let mut f = crate::new_frame();
        f.viewed_participant_index = 2;
        f.participant_info[2].laps_completed = 5;
        f.event_time_remaining = 123.5;
        f.car_name[..3].copy_from_slice(b"Car");
        let c = SessionClock::from_frame(&f);
        assert_eq!((c.laps_completed, c.time_remaining_s), (5, 123.5));
        assert_eq!(c.identity, identity(b"Car", b"", b""));
        assert_eq!(SessionClock::from_json(&c.to_json()), Some(c));
        f.viewed_participant_index = -1;
        assert_eq!(SessionClock::from_frame(&f).laps_completed, 0);
    }
}
