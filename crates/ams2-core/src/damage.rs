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
use stintmate_connector_wire::round_to;

/// A change of a reading smaller than this is not a change (the game's wear creeps by less).
pub const CHANGE: f32 = 0.005;
/// How long the readings must stay put before they are reported, in seconds.
pub const SETTLE_S: f64 = 1.0;

/// A damage reading as a number of the `damage` record: rounded to 3 decimals, or none when it is
/// not a number within the game's range (a reading outside it is left out, never clamped).
/// [`DamageReading::has_damage`] reads the values through it too, so the live frame and the
/// record never disagree.
pub fn record_value(v: f32) -> Option<f64> {
    let v = round_to(f64::from(v), 3);
    (v.is_finite() && (-0.001..=1.001).contains(&v)).then_some(v)
}

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

    /// Whether the car has damage as the `damage` record shows it: any of aero, engine, the four
    /// brakes or the four suspensions above 0 once rounded by [`record_value`], or any wheel not
    /// attached or not inflated. A value the record shows as 0 (a residual below 0.0005) is no
    /// damage.
    pub fn has_damage(&self) -> bool {
        self.floats()
            .iter()
            .any(|v| record_value(*v).is_some_and(|x| x > 0.0))
            || self
                .tyre_flags
                .iter()
                .any(|f| f & TYRE_FLAGS_USED != TYRE_FLAGS_USED)
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

/// How far a damage value must rise within [`SETTLE_S`] to count as a new hit while the request
/// is stale. The recorded wear creeps by about 0.0001 in 10 s (brakes); the smallest recorded hit
/// moved aero by 0.042 within one 20 Hz frame.
pub const NEW_HIT: f32 = 0.01;

/// Whether the game's damage pit request still means something. The game keeps
/// `PIT_SCHEDULE_DAMAGE_REQUESTED` after it reset the car (a return to the garage through the menu,
/// a repair in the box) until a stop is driven, while the car reads healthy and the game shows no
/// flag. The latch marks the request stale on the first playing frame that reads no damage
/// ([`DamageReading::has_damage`]) and keeps it stale while the wear creeps, until the game clears
/// the request or a new hit: a value up by more than [`NEW_HIT`] against readings at most
/// [`SETTLE_S`] old (so creep never adds up to a hit), or a wheel off or deflated.
#[derive(Debug, Clone, Copy, Default)]
pub struct RepairLatch {
    /// While stale: the readings compared with, and when they were taken.
    stale_ref: Option<(DamageReading, f64)>,
}

impl RepairLatch {
    /// Takes the readings of a playing frame at monotonic time `now`.
    pub fn observe(&mut self, r: &DamageReading, now: f64) {
        if r.pit_schedule != SCHEDULE_DAMAGE_REQUESTED {
            self.stale_ref = None;
            return;
        }
        match self.stale_ref {
            None if !r.has_damage() => self.stale_ref = Some((*r, now)),
            None => {}
            Some((base, _)) if new_hit(&base, r) => self.stale_ref = None,
            Some((_, since)) if now - since >= SETTLE_S => self.stale_ref = Some((*r, now)),
            Some(_) => {}
        }
    }

    /// The game requests the damage stop but the car was reset since and took no new hit.
    pub fn request_stale(&self) -> bool {
        self.stale_ref.is_some()
    }
}

/// A value rose by more than [`NEW_HIT`] above `base`, or a wheel came off or went flat.
fn new_hit(base: &DamageReading, r: &DamageReading) -> bool {
    base.floats()
        .iter()
        .zip(r.floats())
        .any(|(b, v)| v - b > NEW_HIT)
        || base.tyre_flags.iter().zip(r.tyre_flags).any(|(b, f)| {
            b & TYRE_FLAGS_USED == TYRE_FLAGS_USED && f & TYRE_FLAGS_USED != TYRE_FLAGS_USED
        })
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
