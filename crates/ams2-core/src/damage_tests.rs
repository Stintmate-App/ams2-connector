//! The damage watch against rows of a real recording: a frontal hit, the game's damage pit
//! request, a side hit, the repair in the box, the request cleared, and the menu frames that
//! carry zeros. Each row below was read from a 20 Hz probe of the shared memory and holds until
//! the next one.

use crate::codes::game_state;
use crate::damage::{DamageReading, DamageWatch, HEALTHY, differs, pit_request_name};
use crate::testing::{Call, Harness};

/// `(time_s, game_state, pit_mode, reading)`.
type Row = (f64, u32, u32, DamageReading);

const fn reading(
    aero: f32,
    engine: f32,
    brake: [f32; 4],
    suspension: [f32; 4],
    tyre_flags: [u32; 4],
    pit_schedule: u32,
) -> DamageReading {
    DamageReading {
        aero,
        engine,
        brake,
        suspension,
        tyre_flags,
        pit_schedule,
    }
}

const ALL: [u32; 4] = [7; 4];
const B0: [f32; 4] = [0.0009, 0.0009, 0.0002, 0.0002];
const B1: [f32; 4] = [0.001, 0.001, 0.0002, 0.0002];
const B2: [f32; 4] = [0.0011, 0.0011, 0.0002, 0.0002];
const ZERO: [f32; 4] = [0.0; 4];
const S_FR: [f32; 4] = [0.0, 0.5272, 0.0, 0.0];

/// The rows of the recording (game state 2 is playing, 3 is a menu).
const RUN: [Row; 13] = [
    (3381.563, 2, 0, reading(0.0, 0.0, ZERO, ZERO, ALL, 0)),
    (3564.313, 2, 0, reading(0.0, 0.0005, B0, ZERO, ALL, 0)),
    (
        3564.344,
        2,
        0,
        reading(0.3037, 0.2005, B0, [0.0, 0.474, 0.0, 0.0], ALL, 0),
    ),
    (3564.438, 2, 0, reading(0.4066, 0.2005, B0, S_FR, ALL, 0)),
    // The rear wheels' "on ground" bit flickers during the hit.
    (
        3564.5,
        2,
        0,
        reading(0.4066, 0.2005, B0, S_FR, [7, 7, 7, 3], 0),
    ),
    (
        3564.594,
        2,
        0,
        reading(0.4066, 0.2005, B0, S_FR, [7, 7, 3, 3], 0),
    ),
    // The game asks the car to pit for repairs.
    (3568.5, 2, 0, reading(0.4066, 0.2005, B0, S_FR, ALL, 3)),
    // A side hit: aero only.
    (3644.907, 2, 0, reading(0.4489, 0.2007, B1, S_FR, ALL, 3)),
    // The car stopped in its box.
    (3705.407, 2, 2, reading(0.4489, 0.2009, B2, S_FR, ALL, 3)),
    // Repaired: every value 0, the request still set.
    (3705.5, 2, 2, reading(0.0, 0.0, ZERO, ZERO, ALL, 3)),
    // Driving out, the request cleared; the engine creeps again.
    (3824.938, 2, 0, reading(0.0, 0.0003, ZERO, ZERO, ALL, 0)),
    // Menu frames carry zeros and no tyre flags.
    (3893.407, 3, 0, reading(0.0, 0.0, ZERO, ZERO, [0; 4], 0)),
    (
        3893.454,
        3,
        0,
        reading(0.0, 0.0005, [0.0002, 0.0001, 0.0, 0.0], ZERO, ALL, 0),
    ),
];

/// The 20 Hz frames of the playing rows: `(time_s, row)`, each row held until the next.
fn playing_frames() -> Vec<(f64, &'static Row)> {
    let playing: Vec<&Row> = RUN
        .iter()
        .filter(|r| r.1 == game_state::INGAME_PLAYING)
        .collect();
    let end = RUN
        .iter()
        .find(|r| r.1 != game_state::INGAME_PLAYING)
        .unwrap()
        .0;
    let mut out = Vec::new();
    for (i, row) in playing.iter().enumerate() {
        let next = playing.get(i + 1).map_or(end, |n| n.0);
        let n = ((next - row.0) / 0.05).round() as usize;
        out.extend((0..n.max(1)).map(|k| (row.0 + k as f64 * 0.05, *row)));
    }
    out
}

/// What the watch reports over the whole run: `(time_s, reading)`.
fn reported() -> Vec<(f64, DamageReading)> {
    let mut w = DamageWatch::new();
    playing_frames()
        .into_iter()
        .filter_map(|(t, row)| w.observe(row.3, t).map(|r| (t, r)))
        .collect()
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-6
}

#[test]
fn a_healthy_first_frame_writes_nothing() {
    let mut w = DamageWatch::new();
    for k in 0..200 {
        assert_eq!(w.observe(RUN[0].3, 3381.563 + f64::from(k) * 0.05), None);
    }
    assert!(!differs(&RUN[0].3, &HEALTHY));
}

#[test]
fn the_whole_run_gives_five_readings() {
    let got = reported();
    assert_eq!(got.len(), 5, "{got:?}");
    assert!(got.windows(2).all(|p| p[0].0 < p[1].0));
}

#[test]
fn a_hit_is_reported_once_its_values_settled() {
    let (t, r) = reported()[0];
    // Not when the first value moved (3564.344): the second change restarted the wait.
    assert!((3565.40..3565.60).contains(&t), "{t}");
    assert!(near(r.aero, 0.4066) && near(r.engine, 0.2005));
    assert!(near(r.suspension[1], 0.5272));
    assert_eq!(r.pit_schedule, 0);
}

#[test]
fn the_rear_ground_flag_flicker_moves_nothing() {
    assert!(!differs(&RUN[3].3, &RUN[4].3));
    assert!(!differs(&RUN[3].3, &RUN[5].3));
    assert_eq!(reported()[0].1.tyre_flags, ALL);
}

#[test]
fn the_damage_pit_request_is_a_change() {
    let (t, r) = reported()[1];
    assert!((3569.40..3569.60).contains(&t), "{t}");
    assert_eq!(r.pit_schedule, 3);
    assert!(near(r.aero, 0.4066));
}

#[test]
fn a_side_hit_moves_only_the_aero() {
    let got = reported();
    let (t, r) = got[2];
    assert!((3645.80..3646.00).contains(&t), "{t}");
    assert!(near(r.aero, 0.4489));
    let before = got[1].1;
    for i in 0..4 {
        assert!((r.brake[i] - before.brake[i]).abs() < 0.005);
        assert!((r.suspension[i] - before.suspension[i]).abs() < 0.005);
    }
}

#[test]
fn engine_creep_alone_writes_nothing() {
    let a = reading(0.0, 0.2005, B0, ZERO, ALL, 0);
    let b = reading(0.0, 0.2009, B2, ZERO, ALL, 0);
    assert!(!differs(&a, &b));
    let mut w = DamageWatch::new();
    let mut got = 0;
    for k in 0..400 {
        let r = if k % 2 == 0 { a } else { b };
        got += usize::from(w.observe(r, f64::from(k) * 0.05).is_some());
    }
    // The first values (a damaged engine against a healthy car) are the one record; the creep
    // after them is none.
    assert_eq!(got, 1);
}

#[test]
fn a_repair_in_the_box_is_a_reading_of_zeros_with_the_request_still_set() {
    let (t, r) = reported()[3];
    assert!((3706.40..3706.60).contains(&t), "{t}");
    assert!(r.aero == 0.0 && r.engine == 0.0);
    assert!(r.brake.iter().chain(&r.suspension).all(|v| *v == 0.0));
    assert_eq!(r.pit_schedule, 3);
}

#[test]
fn the_request_cleared_is_a_reading() {
    let (t, r) = reported()[4];
    assert!((3825.80..3826.00).contains(&t), "{t}");
    assert_eq!(r.pit_schedule, 0);
    assert_eq!(pit_request_name(r.pit_schedule), "PIT_SCHEDULE_NONE");
}

#[test]
fn pit_request_names_follow_the_header() {
    assert_eq!(pit_request_name(3), "PIT_SCHEDULE_DAMAGE_REQUESTED");
    assert_eq!(pit_request_name(7), "PIT_SCHEDULE_PITSPOT_OCCUPIED");
    assert_eq!(pit_request_name(8), "unknown");
}

#[test]
fn a_flag_flip_is_a_change() {
    let a = reading(0.0, 0.0, ZERO, ZERO, ALL, 0);
    assert!(differs(&a, &reading(0.0, 0.0, ZERO, ZERO, [7, 7, 7, 5], 0)));
    assert!(differs(&a, &reading(0.0, 0.0, ZERO, ZERO, [7, 7, 7, 6], 0)));
    // Other schedule values are not a change on their own.
    assert!(!differs(&a, &reading(0.0, 0.0, ZERO, ZERO, ALL, 5)));
}

// ---------------------------------------------------------------- through the tracker

fn feed(h: &mut Harness, t: f64, state: u32, pit_mode: u32, r: &DamageReading) {
    let d = &mut h.sim.d;
    d.game_state = state;
    d.pit_mode = pit_mode;
    d.aero_damage = r.aero;
    d.engine_damage = r.engine;
    d.brake_damage = r.brake;
    d.suspension_damage = r.suspension;
    d.tyre_flags = r.tyre_flags;
    d.pit_schedule = r.pit_schedule;
    h.clock.set(t);
    h.feed_current();
}

#[test]
fn the_tracker_passes_five_readings_in_order() {
    let mut h = Harness::new();
    for (t, row) in playing_frames() {
        feed(&mut h, t, row.1, row.2, &row.3);
    }
    // Then the menu frames: they carry zeros and no tyre flags and must change nothing.
    let mut t = 3893.407;
    for k in 0..200 {
        let row = &RUN[11 + k % 2];
        feed(&mut h, t, row.1, row.2, &row.3);
        t += 0.05;
    }
    let got = h.sink.damages();
    assert_eq!(got.len(), 5, "{got:?}");
    assert!(near(got[0].aero, 0.4066) && got[0].pit_schedule == 0);
    assert_eq!(got[1].pit_schedule, 3);
    assert!(near(got[2].aero, 0.4489));
    assert!(got[3].aero == 0.0 && got[3].pit_schedule == 3);
    assert_eq!(got[4].pit_schedule, 0);
}

#[test]
fn menu_frames_between_playing_frames_change_nothing() {
    let mut h = Harness::new();
    let hit = RUN[3].3;
    let menu = RUN[11].3;
    for k in 0..60 {
        let t = 100.0 + f64::from(k) * 0.05;
        feed(&mut h, t, game_state::INGAME_PLAYING, 0, &hit);
        feed(&mut h, t + 0.025, 3, 0, &menu);
    }
    let got = h.sink.damages();
    assert_eq!(got.len(), 1, "{got:?}");
    assert!(near(got[0].aero, 0.4066));
}

#[test]
fn a_new_session_starts_from_a_healthy_car() {
    let mut h = Harness::new();
    let hit = RUN[3].3;
    for k in 0..60 {
        feed(&mut h, 10.0 + f64::from(k) * 0.05, 2, 0, &hit);
    }
    assert_eq!(h.sink.damages().len(), 1);
    h.end();
    for k in 0..60 {
        feed(&mut h, 20.0 + f64::from(k) * 0.05, 2, 0, &hit);
    }
    let ids: Vec<u64> = h
        .sink
        .calls
        .iter()
        .filter_map(|c| match c {
            Call::Damage { session_id, .. } => Some(*session_id),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
}
