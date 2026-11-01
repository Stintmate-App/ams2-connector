//! `connector.telemetry` v1 (contract 5.5): the neutral 32-byte samples Pitwall's brake cue
//! scheduler runs on, and the mapping from AMS2's values and flags (pure).
//!
//! Layout (little endian, 32 bytes): `lap_distance_m f32`, `speed_ms f32`, `accel_long_ms2 f32`,
//! `track_length_m f32`, `lap_time_s f32`, `frame u32`, `sim_lap u16`, `session_type u8`,
//! `game_state u8`, `flags u16`, `reserved u16`.
//!
//! The mapping reads about a dozen fields: [`Fields`] holds exactly those, filled from a full
//! snapshot ([`Fields::from_snapshot`]) or, by the sampling thread, straight from its own view
//! of the mapping without copying the 20 kB struct. Sampling rules (rate cap, `t_ns`, keep-alive
//! repeats) belong to the thread, not to this module.

use crate::codes::{game_state, pit_mode, session_state};
use ams2_shm::SharedMemory;

/// The channel's id.
pub const CHANNEL: &str = "connector.telemetry";
/// Declared rate, Hz.
pub const RATE_HZ: u32 = 100;
/// Subscription option `rate_hz` range.
pub const RATE_MIN: u32 = 10;
/// Subscription option `rate_hz` range.
pub const RATE_MAX: u32 = 200;
/// Bytes of one sample.
pub const SAMPLE_SIZE: usize = 32;

/// Flag: the car is on track.
pub const ON_TRACK: u16 = 1 << 0;
/// Flag: in the pit lane.
pub const IN_PIT_LANE: u16 = 1 << 1;
/// Flag: in the garage.
pub const IN_GARAGE: u16 = 1 << 2;
/// Flag: the lap clock runs.
pub const TIMED: u16 = 1 << 3;
/// Flag: a flying lap (timed, on track, not an out lap).
pub const FLYING: u16 = 1 << 4;
/// Flag: the game flagged the lap invalid.
pub const LAP_INVALID: u16 = 1 << 5;
/// Flag: the viewed car is the player's (not a replay, a valid index).
pub const PLAYER_VIEW: u16 = 1 << 6;
/// Flag: a new game frame (not a keep-alive repeat).
pub const FRESH: u16 = 1 << 7;

/// The fields a sample needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fields {
    /// `mViewedParticipantIndex`.
    pub viewed: i32,
    /// `mNumParticipants`.
    pub num_participants: i32,
    /// `mTrackLength`.
    pub track_length: f32,
    /// `mCurrentTime`.
    pub current_time: f32,
    /// `mGameState`.
    pub game_state: u32,
    /// `mSessionState`.
    pub session_state: u32,
    /// `mPitMode`.
    pub pit_mode: u32,
    /// `mLapInvalidated`.
    pub lap_invalidated: bool,
    /// `mSpeed`, m/s.
    pub speed: f32,
    /// `mLocalAcceleration[2]`.
    pub accel_z: f32,
    /// The viewed car's `mCurrentLapDistance` (read only when the index is valid).
    pub lap_distance: f32,
    /// The viewed car's `mCurrentLap`.
    pub current_lap: u32,
}

impl Fields {
    /// The fields of a full snapshot.
    pub fn from_snapshot(d: &SharedMemory) -> Self {
        let p = usize::try_from(d.viewed_participant_index)
            .ok()
            .and_then(|i| d.participant_info.get(i));
        Self {
            viewed: d.viewed_participant_index,
            num_participants: d.num_participants,
            track_length: d.track_length,
            current_time: d.current_time,
            game_state: d.game_state,
            session_state: d.session_state,
            pit_mode: d.pit_mode,
            lap_invalidated: d.lap_invalidated != 0,
            speed: d.speed,
            accel_z: d.local_acceleration[2],
            lap_distance: p.map_or(0.0, |p| p.current_lap_distance),
            current_lap: p.map_or(0, |p| p.current_lap),
        }
    }
}

/// One 32-byte sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// Lap distance, m (-1: unknown).
    pub lap_distance_m: f32,
    /// Speed, m/s.
    pub speed_ms: f32,
    /// Longitudinal acceleration, m/s2 (braking is negative).
    pub accel_long_ms2: f32,
    /// Track length, m (0: unknown).
    pub track_length_m: f32,
    /// Lap clock, s (-1: not timed).
    pub lap_time_s: f32,
    /// Frame counter.
    pub frame: u32,
    /// The game's lap counter.
    pub sim_lap: u16,
    /// 0 unknown, 1 practice, 2 test, 3 qualifying, 4 warmup, 5 race, 6 other.
    pub session_type: u8,
    /// 0 not running, 1 menus, 2 loading, 3 playing, 4 paused, 5 replay.
    pub game_state: u8,
    /// The `ON_TRACK` .. `FRESH` bits.
    pub flags: u16,
    /// Reserved, 0.
    pub reserved: u16,
}

impl Sample {
    /// The wire bytes (little endian).
    pub fn to_bytes(&self) -> [u8; SAMPLE_SIZE] {
        let mut b = [0u8; SAMPLE_SIZE];
        b[0..4].copy_from_slice(&self.lap_distance_m.to_le_bytes());
        b[4..8].copy_from_slice(&self.speed_ms.to_le_bytes());
        b[8..12].copy_from_slice(&self.accel_long_ms2.to_le_bytes());
        b[12..16].copy_from_slice(&self.track_length_m.to_le_bytes());
        b[16..20].copy_from_slice(&self.lap_time_s.to_le_bytes());
        b[20..24].copy_from_slice(&self.frame.to_le_bytes());
        b[24..26].copy_from_slice(&self.sim_lap.to_le_bytes());
        b[26] = self.session_type;
        b[27] = self.game_state;
        b[28..30].copy_from_slice(&self.flags.to_le_bytes());
        b[30..32].copy_from_slice(&self.reserved.to_le_bytes());
        b
    }

    /// Reads a sample from its wire bytes.
    pub fn from_bytes(b: &[u8; SAMPLE_SIZE]) -> Self {
        let f = |i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        Self {
            lap_distance_m: f(0),
            speed_ms: f(4),
            accel_long_ms2: f(8),
            track_length_m: f(12),
            lap_time_s: f(16),
            frame: u32::from_le_bytes([b[20], b[21], b[22], b[23]]),
            sim_lap: u16::from_le_bytes([b[24], b[25]]),
            session_type: b[26],
            game_state: b[27],
            flags: u16::from_le_bytes([b[28], b[29]]),
            reserved: u16::from_le_bytes([b[30], b[31]]),
        }
    }
}

/// `session_type` of a session state.
pub fn session_type(state: u32) -> u8 {
    match state {
        session_state::PRACTICE | session_state::TIME_ATTACK => 1,
        session_state::TEST => 2,
        session_state::QUALIFY => 3,
        session_state::FORMATION_LAP | session_state::RACE => 5,
        session_state::INVALID => 0,
        _ => 6,
    }
}

/// `game_state` code of the game's state.
pub fn game_state_code(state: u32) -> u8 {
    match state {
        game_state::EXITED => 0,
        game_state::INGAME_PLAYING => 3,
        game_state::INGAME_PAUSED | game_state::INGAME_INMENU_TIME_TICKING => 4,
        game_state::INGAME_RESTARTING => 2,
        game_state::INGAME_REPLAY | game_state::FRONT_END_REPLAY => 5,
        _ => 1, // front end and anything unknown
    }
}

fn is_pit_lane(pit: u32) -> bool {
    matches!(
        pit,
        pit_mode::DRIVING_INTO_PITS
            | pit_mode::IN_PIT
            | pit_mode::DRIVING_OUT_OF_PITS
            | pit_mode::DRIVING_OUT_OF_GARAGE
    )
}

/// The sample of one snapshot. `in_garage`: the tracker's garage rule (the garage screen, or
/// parked in the car before it moved); `flying_lap`: the tracker's lap is not an out lap.
pub fn sample_values(
    f: &Fields,
    frame: u32,
    fresh: bool,
    in_garage: bool,
    flying_lap: bool,
) -> Sample {
    let valid = usize::try_from(f.viewed)
        .is_ok_and(|i| i < usize::try_from(f.num_participants).unwrap_or(0).min(64));
    let (dist, sim_lap) = if valid {
        (f.lap_distance, f.current_lap.min(0xFFFF) as u16)
    } else {
        (-1.0, 0)
    };
    let timed = f.current_time >= 0.0;
    let mut flags = if fresh { FRESH } else { 0 };
    if in_garage || f.pit_mode == pit_mode::IN_GARAGE {
        flags |= IN_GARAGE;
    } else if is_pit_lane(f.pit_mode) {
        flags |= IN_PIT_LANE;
    } else if f.pit_mode == pit_mode::NONE {
        flags |= ON_TRACK;
    }
    if timed {
        flags |= TIMED;
        if flying_lap && flags & ON_TRACK != 0 {
            flags |= FLYING;
        }
    }
    if f.lap_invalidated {
        flags |= LAP_INVALID;
    }
    let replay = matches!(
        f.game_state,
        game_state::INGAME_REPLAY | game_state::FRONT_END_REPLAY
    );
    if valid && !replay {
        flags |= PLAYER_VIEW;
    }
    Sample {
        lap_distance_m: dist,
        speed_ms: f.speed,
        accel_long_ms2: -f.accel_z,
        track_length_m: if f.track_length > 0.0 {
            f.track_length
        } else {
            0.0
        },
        lap_time_s: if timed { f.current_time } else { -1.0 },
        frame,
        sim_lap,
        session_type: session_type(f.session_state),
        game_state: game_state_code(f.game_state),
        flags,
        reserved: 0,
    }
}

/// The keep-alive sample while the game is not running (`game_state` 0).
pub fn idle_values(frame: u32) -> Sample {
    Sample {
        lap_distance_m: -1.0,
        speed_ms: 0.0,
        accel_long_ms2: 0.0,
        track_length_m: 0.0,
        lap_time_s: -1.0,
        frame,
        sim_lap: 0,
        session_type: 0,
        game_state: 0,
        flags: 0,
        reserved: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ams2_shm::new_frame;

    /// A frame filled in by `f`.
    fn snap(f: impl FnOnce(&mut SharedMemory)) -> Box<SharedMemory> {
        let mut d = new_frame();
        d.version = 14;
        d.game_state = game_state::INGAME_PLAYING;
        d.session_state = session_state::PRACTICE;
        d.num_participants = 2;
        d.viewed_participant_index = 1;
        d.track_length = 4316.9;
        d.current_time = 61.25;
        d.speed = 55.5;
        d.local_acceleration[2] = 9.5; // braking: the car's -z
        let p = &mut d.participant_info[1];
        p.is_active = 1;
        p.current_lap_distance = 1234.5;
        p.current_lap = 7;
        f(&mut d);
        d
    }

    fn sample(d: &SharedMemory, frame: u32, fresh: bool, garage: bool, flying: bool) -> Sample {
        sample_values(&Fields::from_snapshot(d), frame, fresh, garage, flying)
    }

    #[test]
    fn layout_is_the_contracts() {
        assert_eq!(SAMPLE_SIZE, 32);
        let s = Sample {
            lap_distance_m: 1.0,
            speed_ms: 2.0,
            accel_long_ms2: 3.0,
            track_length_m: 4.0,
            lap_time_s: 5.0,
            frame: 6,
            sim_lap: 7,
            session_type: 8,
            game_state: 9,
            flags: 10,
            reserved: 0,
        };
        let b = s.to_bytes();
        // Layout: `<5fIH2B2H`, little-endian
        assert_eq!(&b[0..4], &1.0f32.to_le_bytes());
        assert_eq!(&b[16..20], &5.0f32.to_le_bytes());
        assert_eq!(&b[20..24], &6u32.to_le_bytes());
        assert_eq!(&b[24..26], &7u16.to_le_bytes());
        assert_eq!((b[26], b[27]), (8, 9));
        assert_eq!(&b[28..30], &10u16.to_le_bytes());
        assert_eq!(Sample::from_bytes(&b), s);
    }

    #[test]
    fn values_on_a_flying_lap() {
        let v = sample(&snap(|_| {}), 42, true, false, true);
        assert_eq!(
            (v.lap_distance_m, v.speed_ms, v.accel_long_ms2),
            (1234.5, 55.5, -9.5)
        );
        assert!((v.track_length_m - 4316.9).abs() < 1e-3 && v.lap_time_s == 61.25);
        assert_eq!(
            (v.frame, v.sim_lap, v.session_type, v.game_state, v.reserved),
            (42, 7, 1, 3, 0)
        );
        assert_eq!(v.flags, ON_TRACK | TIMED | FLYING | PLAYER_VIEW | FRESH);
    }

    #[test]
    fn flags() {
        type Case = (Box<dyn Fn(&mut SharedMemory)>, bool, u16, u16);
        let cases: Vec<Case> = vec![
            (
                Box::new(|d| d.pit_mode = pit_mode::DRIVING_INTO_PITS),
                false,
                IN_PIT_LANE,
                ON_TRACK | FLYING,
            ),
            (
                Box::new(|d| d.pit_mode = pit_mode::IN_GARAGE),
                false,
                IN_GARAGE,
                ON_TRACK,
            ),
            (
                Box::new(|d| d.pit_mode = pit_mode::DRIVING_OUT_OF_GARAGE),
                true,
                IN_GARAGE,
                IN_PIT_LANE,
            ),
            (
                Box::new(|d| d.current_time = -1.0),
                false,
                0,
                TIMED | FLYING,
            ),
            (Box::new(|d| d.lap_invalidated = 1), false, LAP_INVALID, 0),
            (
                Box::new(|d| d.game_state = game_state::INGAME_REPLAY),
                false,
                0,
                PLAYER_VIEW,
            ),
            (
                Box::new(|d| d.viewed_participant_index = -1),
                false,
                0,
                PLAYER_VIEW,
            ),
        ];
        for (i, (change, garage, want, not_want)) in cases.iter().enumerate() {
            let flags = sample(&snap(change), 1, false, *garage, true).flags;
            assert!(flags & want == *want, "case {i}: {flags:#x}");
            assert!(flags & not_want == 0, "case {i}: {flags:#x}");
            assert!(flags & FRESH == 0, "case {i}");
        }
    }

    #[test]
    fn unknowns() {
        let d = snap(|d| {
            d.viewed_participant_index = 9;
            d.track_length = 0.0;
            d.current_time = -1.0;
        });
        let v = sample(&d, 3, true, false, false);
        assert_eq!(
            (v.lap_distance_m, v.track_length_m, v.lap_time_s, v.sim_lap),
            (-1.0, 0.0, -1.0, 0)
        );
        let idle = idle_values(5);
        assert_eq!(idle.frame, 5);
        assert_eq!(
            (
                idle.lap_distance_m,
                idle.lap_time_s,
                idle.game_state,
                idle.flags
            ),
            (-1.0, -1.0, 0, 0)
        );
    }

    #[test]
    fn session_types() {
        for (state, code) in [
            (session_state::PRACTICE, 1),
            (session_state::TEST, 2),
            (session_state::QUALIFY, 3),
            (session_state::FORMATION_LAP, 5),
            (session_state::RACE, 5),
            (session_state::INVALID, 0),
            (session_state::TIME_ATTACK, 1),
            (99, 6),
        ] {
            let d = snap(|d| d.session_state = state);
            assert_eq!(
                sample(&d, 1, true, false, false).session_type,
                code,
                "{state}"
            );
        }
    }

    #[test]
    fn game_states() {
        for (state, code) in [
            (game_state::FRONT_END, 1),
            (game_state::INGAME_RESTARTING, 2),
            (game_state::INGAME_PLAYING, 3),
            (game_state::INGAME_PAUSED, 4),
            (game_state::INGAME_INMENU_TIME_TICKING, 4),
            (game_state::INGAME_REPLAY, 5),
            (game_state::EXITED, 0),
            (99, 1),
        ] {
            let d = snap(|d| d.game_state = state);
            assert_eq!(
                sample(&d, 1, true, false, false).game_state,
                code,
                "{state}"
            );
        }
    }

    #[test]
    fn frame_counter_wraps_and_lap_is_clamped() {
        let d = snap(|d| d.participant_info[1].current_lap = 100_000);
        let v = sample(&d, u32::MAX, true, false, false);
        assert_eq!((v.frame, v.sim_lap), (u32::MAX, 0xFFFF));
    }
}
