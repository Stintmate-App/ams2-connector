//! Per-sample telemetry channels extracted from a shared memory snapshot.
//!
//! The tracker buffers one [`Row`] per sample ([`extract`], ordered as [`CHANNEL_NAMES`], AMS2's
//! own values: `<wheel>_terrain` is AMS2's terrain material and `pit_mode` AMS2's pit mode).
//! When a lap is written, [`lap_channels`] turns the rows into the lap message's channels,
//! with the channel catalogue's names and codes (`<wheel>_surface` and `pit_mode` through
//! [`crate::vocab`]). That runs on the sink's writer thread, never on the sampling thread.
//!
//! Conventions (verified in Phase 1, see FINDINGS):
//! * `g_lat` > 0 = cornering to the right (same sign as `steer`).
//! * `g_long` > 0 = accelerating, < 0 = braking.
//! * Tyre temps are stored as inner / middle / outer per wheel. The game's `Left`/`Right` are the
//!   car's sides: on left wheels Left = outer, on right wheels Left = inner.
//! * Pressures in kPa, temperatures in Celsius, suspension travel in mm, ride height in cm,
//!   fuel in litres.

use crate::codes::car_flags;
use crate::vocab;
use ams2_shm::{ParticipantInfo, SharedMemory};

const KELVIN: f64 = 273.15;
const G: f64 = 9.80665;

/// Number of channels of a sample.
pub const N_CHANNELS: usize = 72;

/// One sample, ordered as [`CHANNEL_NAMES`]. Doubles;
/// they become float32 when the lap is encoded.
pub type Row = [f64; N_CHANNELS];

macro_rules! channel_names {
    ($terrain:literal; $($w:literal),*) => {
        [
            "t", "dist", "speed", "throttle", "brake", "clutch", "steer", "gear", "rpm",
            "g_lat", "g_long", "g_vert", "x", "y", "z", "fuel", "abs", "tc", "invalid",
            "pit_mode",
            $(
                concat!($w, "_t_in"), concat!($w, "_t_mid"), concat!($w, "_t_out"),
                concat!($w, "_t_carcass"), concat!($w, "_t_air"), concat!($w, "_pressure"),
                concat!($w, "_wear"), concat!($w, "_brake_t"), concat!($w, "_susp"),
                concat!($w, "_susp_vel"), concat!($w, "_ride"), concat!($w, "_", $terrain),
                concat!($w, "_rps"),
            )*
        ]
    };
}

/// The sample channels, in row order.
pub const CHANNEL_NAMES: [&str; N_CHANNELS] = channel_names!("terrain"; "fl", "fr", "rl", "rr");
/// The lap message's channel names (the catalogue's): terrain materials become surface codes.
pub const LAP_CHANNEL_NAMES: [&str; N_CHANNELS] = channel_names!("surface"; "fl", "fr", "rl", "rr");

/// Column of the game lap clock (`mCurrentTime`), s.
pub const T: usize = 0;
/// Column of the lap distance, m.
pub const DIST: usize = 1;
/// Column of the speed, km/h.
pub const SPEED: usize = 2;
/// Column of the fuel, litres.
pub const FUEL: usize = 15;
/// Column of the pit mode (AMS2's).
pub const PIT: usize = 19;
/// Columns of the four `<wheel>_terrain` channels.
pub const TERRAIN: [usize; 4] = [31, 44, 57, 70];

/// Returns one sample for the viewed car.
pub fn extract(d: &SharedMemory, p: &ParticipantInfo) -> Row {
    let mut row = [0.0; N_CHANNELS];
    extract_into(d, p, &mut row);
    row
}

/// Writes one sample into `row` (no allocation).
pub fn extract_into(d: &SharedMemory, p: &ParticipantInfo, row: &mut Row) {
    let acc = d.local_acceleration.map(f64::from);
    let pos = p.world_position;
    let flags = d.car_flags;
    let head: [f64; 20] = [
        f64::from(d.current_time),
        f64::from(p.current_lap_distance),
        f64::from(d.speed) * 3.6,
        f64::from(d.unfiltered_throttle),
        f64::from(d.unfiltered_brake),
        f64::from(d.unfiltered_clutch),
        f64::from(d.unfiltered_steering),
        f64::from(d.gear),
        f64::from(d.rpm),
        -acc[0] / G,
        -acc[2] / G,
        acc[1] / G,
        f64::from(pos[0]),
        f64::from(pos[1]),
        f64::from(pos[2]),
        f64::from(d.fuel_level) * f64::from(d.fuel_capacity),
        f64::from(u8::from(
            d.anti_lock_active != 0 || flags & car_flags::ABS != 0,
        )),
        f64::from(u8::from(flags & car_flags::TCS != 0)),
        f64::from(u8::from(d.lap_invalidated != 0)),
        f64::from(d.pit_mode),
    ];
    row[..20].copy_from_slice(&head);
    for i in 0..4 {
        let (left, right) = (d.tyre_temp_left[i], d.tyre_temp_right[i]);
        let (inner, outer) = if i == 0 || i == 2 {
            (right, left)
        } else {
            (left, right)
        };
        let wheel: [f64; 13] = [
            f64::from(inner),
            f64::from(d.tyre_temp_center[i]),
            f64::from(outer),
            f64::from(d.tyre_carcass_temp[i]) - KELVIN,
            f64::from(d.tyre_internal_air_temp[i]) - KELVIN,
            f64::from(d.air_pressure[i]),
            f64::from(d.tyre_wear[i]),
            f64::from(d.brake_temp_celsius[i]),
            f64::from(d.suspension_travel[i]) * 1000.0,
            f64::from(d.suspension_velocity[i]),
            f64::from(d.ride_height[i]),
            f64::from(d.terrain[i]),
            f64::from(d.tyre_rps[i]),
        ];
        row[20 + i * 13..33 + i * 13].copy_from_slice(&wheel);
    }
}

/// A lap's channels as the lap message carries them: one float32 column per catalogue channel.
pub type LapColumns = Vec<(&'static str, Vec<f32>)>;

/// A lap's sample rows as the lap message's channels: one float32 column per catalogue
/// channel, terrain and pit mode translated to Pitwall's codes. No rows, no channels.
pub fn lap_channels(rows: &[Row]) -> LapColumns {
    if rows.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::with_capacity(N_CHANNELS);
    for (i, name) in LAP_CHANNEL_NAMES.iter().enumerate() {
        let is_terrain = CHANNEL_NAMES[i].ends_with("_terrain");
        let col: Vec<f32> = rows
            .iter()
            .map(|r| {
                let v = r[i];
                if is_terrain {
                    // a non-integer material is road (the detector truncates)
                    if v.fract() == 0.0 {
                        f32::from(vocab::surface(v))
                    } else {
                        f32::from(vocab::SURFACE_ROAD)
                    }
                } else if i == PIT {
                    f32::from(vocab::pit_mode(v))
                } else {
                    v as f32
                }
            })
            .collect();
        out.push((*name, col));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_indexes() {
        assert_eq!(CHANNEL_NAMES.len(), 72);
        assert_eq!(CHANNEL_NAMES[T], "t");
        assert_eq!(CHANNEL_NAMES[DIST], "dist");
        assert_eq!(CHANNEL_NAMES[SPEED], "speed");
        assert_eq!(CHANNEL_NAMES[FUEL], "fuel");
        assert_eq!(CHANNEL_NAMES[PIT], "pit_mode");
        for (w, idx) in ["fl", "fr", "rl", "rr"].iter().zip(TERRAIN) {
            assert_eq!(CHANNEL_NAMES[idx], format!("{w}_terrain"));
            assert_eq!(LAP_CHANNEL_NAMES[idx], format!("{w}_surface"));
        }
        assert_eq!(CHANNEL_NAMES[20], "fl_t_in");
        assert_eq!(CHANNEL_NAMES[71], "rr_rps");
        let mut seen = std::collections::HashSet::new();
        assert!(LAP_CHANNEL_NAMES.iter().all(|n| seen.insert(*n)));
    }

    #[test]
    fn tyre_orientation_and_units() {
        let mut d = ams2_shm::new_frame();
        d.tyre_temp_left = [50.0, 60.0, 30.0, 33.0];
        d.tyre_temp_right = [60.0, 50.0, 33.0, 30.0];
        d.speed = 100.0 / 3.6;
        d.local_acceleration = [9.80665, 0.0, -9.80665];
        d.tyre_carcass_temp = [373.15; 4];
        let p = d.participant_info[0];
        let row = extract(&d, &p);
        // Left wheels: game Right = inner. Right wheels: game Left = inner.
        assert_eq!(
            row[CHANNEL_NAMES.iter().position(|n| *n == "fl_t_in").unwrap()],
            60.0
        );
        assert_eq!(
            row[CHANNEL_NAMES.iter().position(|n| *n == "fl_t_out").unwrap()],
            50.0
        );
        assert_eq!(
            row[CHANNEL_NAMES.iter().position(|n| *n == "fr_t_in").unwrap()],
            60.0
        );
        assert!((row[SPEED] - 100.0).abs() < 1e-4);
        assert!((row[9] + 1.0).abs() < 1e-6 && (row[10] - 1.0).abs() < 1e-6);
        assert!((row[23] - 100.0).abs() < 1e-4); // carcass in Celsius
    }
}
