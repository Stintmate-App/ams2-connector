//! The car's damage readings and when they are worth a record.
//!
//! The game reports damage as fractions from 0 to 1 (`mAeroDamage`, `mEngineDamage`,
//! `mBrakeDamage[4]`, `mSuspensionDamage[4]`), each wheel's tyre flags (`mTyreFlags[4]`) and the
//! pit schedule (`mPitSchedule`). The values move in steps (a hit) and creep slowly (wear), and
//! a hit moves several of them over a few frames. [`DamageWatch`] waits for the values to hold
//! still for [`SETTLE_S`] seconds and then reports them once, so a record carries the settled
//! state and never a half-applied hit.

use crate::frame::SCHEDULE_DAMAGE_REQUESTED;
use ams2_shm::SharedMemory;

/// A change of a reading smaller than this is not a change (the game's wear creeps by less).
pub const CHANGE: f32 = 0.005;
/// How long the readings must stay put before they are reported, in seconds.
pub const SETTLE_S: f64 = 1.0;

/// `TYRE_ATTACHED` and `TYRE_INFLATED` of the game's tyre flags. `TYRE_IS_ON_GROUND` (bit 2)
/// flickers during a hit and is not used.
pub const TYRE_FLAGS_USED: u32 = 0b11;

/// The damage-related values of one frame. `Copy`, no heap: one is built per frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DamageReading {
    /// `mAeroDamage`.
    pub aero: f32,
    /// `mEngineDamage`.
    pub engine: f32,
    /// `mBrakeDamage`: front left, front right, rear left, rear right.
    pub brake: [f32; 4],
    /// `mSuspensionDamage`, in the same order.
    pub suspension: [f32; 4],
    /// `mTyreFlags`, raw, in the same order.
    pub tyre_flags: [u32; 4],
    /// `mPitSchedule`, raw.
    pub pit_schedule: u32,
}

/// What a car with no damage reads: nothing is reported until the values leave it.
pub const HEALTHY: DamageReading = DamageReading {
    aero: 0.0,
    engine: 0.0,
    brake: [0.0; 4],
    suspension: [0.0; 4],
    tyre_flags: [TYRE_FLAGS_USED; 4],
    pit_schedule: 0,
};

impl DamageReading {
    /// The readings of a frame.
    pub fn of(d: &SharedMemory) -> Self {
        Self {
            aero: d.aero_damage,
            engine: d.engine_damage,
            brake: d.brake_damage,
            suspension: d.suspension_damage,
            tyre_flags: d.tyre_flags,
            pit_schedule: d.pit_schedule,
        }
    }

    fn floats(&self) -> [f32; 10] {
        let [b0, b1, b2, b3] = self.brake;
        let [s0, s1, s2, s3] = self.suspension;
        [self.aero, self.engine, b0, b1, b2, b3, s0, s1, s2, s3]
    }
}

/// Whether `a` and `b` differ enough to be a change: a value moved by more than [`CHANGE`], a
/// tyre flag (attached, inflated) flipped, or the damage pit request was set or cleared.
pub fn differs(a: &DamageReading, b: &DamageReading) -> bool {
    a.floats()
        .iter()
        .zip(b.floats())
        .any(|(x, y)| (x - y).abs() > CHANGE)
        || a.tyre_flags
            .iter()
            .zip(b.tyre_flags)
            .any(|(x, y)| (x ^ y) & TYRE_FLAGS_USED != 0)
        || (a.pit_schedule == SCHEDULE_DAMAGE_REQUESTED)
            != (b.pit_schedule == SCHEDULE_DAMAGE_REQUESTED)
}

/// The header's name of a pit schedule value (`PIT_SCHEDULE_*` of the game's `SharedMemory.h`),
/// `"unknown"` for any other number.
pub fn pit_request_name(schedule: u32) -> &'static str {
    match schedule {
        0 => "PIT_SCHEDULE_NONE",
        1 => "PIT_SCHEDULE_PLAYER_REQUESTED",
        2 => "PIT_SCHEDULE_ENGINEER_REQUESTED",
        3 => "PIT_SCHEDULE_DAMAGE_REQUESTED",
        4 => "PIT_SCHEDULE_MANDATORY",
        5 => "PIT_SCHEDULE_DRIVE_THROUGH",
        6 => "PIT_SCHEDULE_STOP_GO",
        7 => "PIT_SCHEDULE_PITSPOT_OCCUPIED",
        _ => "unknown",
    }
}

/// Decides when the readings are reported: once they settled on values that differ from the last
/// ones reported (from a healthy car when none were).
#[derive(Debug, Clone, Copy, Default)]
pub struct DamageWatch {
    sent: Option<DamageReading>,
    /// The values seen and when they were first seen. A creeping value is compared to these, not
    /// to the previous frame, so slow wear adds up to a change in the end.
    candidate: Option<(DamageReading, f64)>,
}

impl DamageWatch {
    /// A watch that has reported nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes the readings of a playing frame at monotonic time `now`; returns them when they
    /// settled on new values.
    pub fn observe(&mut self, r: DamageReading, now: f64) -> Option<DamageReading> {
        match self.candidate {
            Some((c, _)) if !differs(&r, &c) => {}
            _ => {
                self.candidate = Some((r, now));
                return None;
            }
        }
        let (c, since) = self.candidate?;
        if now - since >= SETTLE_S && differs(&c, &self.sent.unwrap_or(HEALTHY)) {
            self.sent = Some(c);
            return Some(c);
        }
        None
    }
}
