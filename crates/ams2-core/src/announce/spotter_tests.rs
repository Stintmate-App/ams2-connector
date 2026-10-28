//! The spotter on synthetic frames (PRY-098): the player is car 0 at the origin heading along
//! -z at yaw 0 (`x'` = world x to the left, `z'` = world z behind), at 50 m/s.

use super::catalog::{check_params, is_spotter, messages};
use super::detector::EventDetector;
use super::rate::{RATE_WINDOW_S, RateWindow, SPOTTER_RATE_LIMIT};
use super::spotter::*;
use crate::codes::{game_state, pit_mode, session_state};
use crate::testing::FakeClock;
use ams2_shm::{SharedMemory, new_frame};
use serde_json::Value;

const HZ: f64 = 50.0;

fn frame(session: u32) -> Box<SharedMemory> {
    let mut d = new_frame();
    d.game_state = game_state::INGAME_PLAYING;
    d.session_state = session;
    d.race_state = 2;
    d.viewed_participant_index = 0;
    d.num_participants = 3;
    d.speed = 50.0;
    for i in 0..3 {
        d.participant_info[i].is_active = 1;
        d.participant_info[i].name[..2].copy_from_slice(if i == 0 { b"Me" } else { b"Ri" });
        d.speeds[i] = 50.0;
    }
    d
}

/// Puts car `i` at player-frame (`x`, `z`) for the player's yaw (`orientation[1]`).
fn place(d: &mut SharedMemory, i: usize, x: f32, z: f32) {
    let theta = -d.orientation[1];
    let (s, c) = theta.sin_cos();
    let p = &mut d.participant_info[i].world_position;
    p[0] = c * x - s * z;
    p[2] = s * x + c * z;
}

fn away(d: &mut SharedMemory, i: usize) {
    place(d, i, 40.0, 40.0);
}

struct Run {
    sp: Spotter,
    d: Box<SharedMemory>,
    t: f64,
    view_ok: bool,
}

impl Run {
    fn new(session: u32) -> Self {
        let mut d = frame(session);
        away(&mut d, 1);
        away(&mut d, 2);
        Self {
            sp: Spotter::seeded(7),
            d,
            t: 1000.0,
            view_ok: true,
        }
    }

    /// Runs `seconds` of frames; the codes with their times.
    fn run(&mut self, seconds: f64) -> Vec<(f64, &'static str)> {
        let mut out = Vec::new();
        for _ in 0..((seconds * HZ).round() as usize).max(1) {
            self.t += 1.0 / HZ;
            if let Some(c) = self.sp.update(&self.d, self.t, self.view_ok, false) {
                out.push((self.t, c.code));
            }
        }
        out
    }

    fn codes(&mut self, seconds: f64) -> Vec<&'static str> {
        self.run(seconds).into_iter().map(|(_, c)| c).collect()
    }
}

fn practice() -> Run {
    Run::new(session_state::PRACTICE)
}

#[test]
fn a_car_passing_on_the_left_is_called_then_cleared_after_the_delay() {
    let mut r = practice();
    assert!(r.codes(1.0).is_empty());
    place(&mut r.d, 1, 3.0, 0.0);
    assert_eq!(r.codes(0.1), [CAR_LEFT]);
    away(&mut r.d, 1);
    let left_at = r.t;
    let ev = r.run(0.5);
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0].1, CLEAR_LEFT);
    assert!((ev[0].0 - left_at - CLEAR_DELAY_S).abs() < 0.05, "{ev:?}");
    assert!(r.sp.idle());
}

#[test]
fn a_car_on_the_right_is_the_mirror() {
    let mut r = practice();
    place(&mut r.d, 1, -3.0, 2.0);
    assert_eq!(r.codes(0.1), [CAR_RIGHT]);
    away(&mut r.d, 1);
    assert_eq!(r.codes(0.5), [CLEAR_RIGHT]);
}

#[test]
fn three_wide_from_each_side_and_back_ends_in_all_clear() {
    let mut r = practice();
    place(&mut r.d, 1, 3.0, 0.0);
    assert_eq!(r.codes(0.1), [CAR_LEFT]);
    place(&mut r.d, 2, -3.0, 0.0);
    assert_eq!(r.codes(0.1), [IN_THE_MIDDLE]);
    // the right one drops: "Car left" (not a clear)
    away(&mut r.d, 2);
    assert_eq!(r.codes(0.5), [CAR_LEFT]);
    // then the left one: all clear, because the middle was said
    away(&mut r.d, 1);
    assert_eq!(r.codes(0.5), [ALL_CLEAR]);

    // from clear straight to both sides (after the bounce window), then both go at once
    assert!(r.codes(2.0).is_empty());
    place(&mut r.d, 1, 3.0, 0.0);
    place(&mut r.d, 2, -3.0, 0.0);
    assert_eq!(r.codes(0.1), [IN_THE_MIDDLE]);
    away(&mut r.d, 1);
    away(&mut r.d, 2);
    assert_eq!(r.codes(0.5), [ALL_CLEAR]);

    // mirror: the left one drops first
    assert!(r.codes(2.0).is_empty());
    place(&mut r.d, 1, 3.0, 0.0);
    place(&mut r.d, 2, -3.0, 0.0);
    assert_eq!(r.codes(0.1), [IN_THE_MIDDLE]);
    away(&mut r.d, 1);
    assert_eq!(r.codes(0.5), [CAR_RIGHT]);
    away(&mut r.d, 2);
    assert_eq!(r.codes(0.5), [ALL_CLEAR]);
}

#[test]
fn nose_to_tail_and_out_of_reach_are_not_called() {
    let mut r = practice();
    place(&mut r.d, 1, 0.5, -4.0); // directly ahead
    place(&mut r.d, 2, -1.0, 3.0); // directly behind
    assert!(r.codes(1.0).is_empty());
    place(&mut r.d, 1, 3.0, -4.5); // alongside but a nose ahead
    place(&mut r.d, 2, -3.0, 5.0); // alongside but a tail behind
    assert!(r.codes(1.0).is_empty());
    place(&mut r.d, 1, 25.0, 0.0); // beyond the search range
    assert!(r.codes(1.0).is_empty());
}

#[test]
fn the_new_overlap_edges_ahead_and_behind() {
    let mut r = practice();
    place(&mut r.d, 1, 3.0, -4.3);
    assert_eq!(r.codes(0.1), [CAR_LEFT]);
    let mut r = practice();
    place(&mut r.d, 1, 3.0, 4.7);
    assert_eq!(r.codes(0.1), [CAR_LEFT]);
}

#[test]
fn the_hysteresis_keeps_a_held_overlap_a_little_longer_and_without_the_width_test() {
    let mut r = practice();
    place(&mut r.d, 1, 3.0, 0.0);
    assert_eq!(r.codes(0.1), [CAR_LEFT]);
    // drifts to 4.8 m ahead and 1 m across: still the same overlap (no clear, no new call)
    place(&mut r.d, 1, 1.0, -4.8);
    assert!(r.codes(1.0).is_empty());
    place(&mut r.d, 1, 1.0, -5.0);
    assert_eq!(r.codes(0.5), [CLEAR_LEFT]);
}

#[test]
fn a_car_closing_faster_than_12_ms_is_passing_not_alongside() {
    let mut r = practice();
    place(&mut r.d, 1, 3.0, 0.0);
    r.d.speeds[1] = 35.0; // 15 m/s slower
    assert!(r.codes(1.0).is_empty());
    r.d.speeds[1] = 40.0; // 10 m/s
    assert_eq!(r.codes(0.1), [CAR_LEFT]);
}

#[test]
fn an_overlap_back_within_the_clear_delay_cancels_the_clear() {
    let mut r = practice();
    place(&mut r.d, 1, 3.0, 0.0);
    assert_eq!(r.codes(0.1), [CAR_LEFT]);
    away(&mut r.d, 1);
    assert!(r.codes(0.1).is_empty());
    place(&mut r.d, 1, 3.0, 0.0);
    assert!(r.codes(0.5).is_empty());
}

#[test]
fn a_bounce_within_1_5_s_of_the_clear_is_called_only_if_it_holds() {
    let mut r = practice();
    place(&mut r.d, 1, 3.0, 0.0);
    assert_eq!(r.codes(0.1), [CAR_LEFT]);
    away(&mut r.d, 1);
    assert_eq!(r.codes(0.3), [CLEAR_LEFT]);
    // it comes back 0.3 s later and stays: called at 1.5 s after the clear, not before
    place(&mut r.d, 1, 3.0, 0.0);
    let ev = r.run(2.0);
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert_eq!(ev[0].1, CAR_LEFT);

    // a bounce that leaves again before 1.5 s: no call and no clear
    away(&mut r.d, 1);
    assert_eq!(r.codes(0.5), [CLEAR_LEFT]);
    place(&mut r.d, 1, 3.0, 0.0);
    assert!(r.codes(0.5).is_empty());
    away(&mut r.d, 1);
    assert!(r.codes(3.0).is_empty());
    assert!(r.sp.idle());
}

#[test]
fn still_right_every_three_seconds_while_the_overlap_holds() {
    let mut r = practice();
    place(&mut r.d, 1, -3.0, 0.0);
    let ev = r.run(7.0);
    let codes: Vec<_> = ev.iter().map(|e| e.1).collect();
    assert_eq!(codes, [CAR_RIGHT, STILL_RIGHT, STILL_RIGHT]);
    assert!((ev[1].0 - ev[0].0 - STILL_THERE_S).abs() < 0.05);
    assert!((ev[2].0 - ev[1].0 - STILL_THERE_S).abs() < 0.05);
}

#[test]
fn in_the_middle_restarts_the_still_timer() {
    let mut r = practice();
    place(&mut r.d, 1, 3.0, 0.0);
    assert_eq!(r.codes(2.5), [CAR_LEFT]);
    place(&mut r.d, 2, -3.0, 0.0);
    assert_eq!(r.codes(0.1), [IN_THE_MIDDLE]);
    assert!(r.codes(2.5).is_empty());
    assert_eq!(r.codes(1.0), [STILL_MIDDLE]);
}

#[test]
fn still_left_and_still_middle_say_the_side() {
    let mut r = practice();
    place(&mut r.d, 1, 3.0, 0.0);
    assert_eq!(r.codes(7.0), [CAR_LEFT, STILL_LEFT, STILL_LEFT]);
    place(&mut r.d, 2, -3.0, 0.0);
    assert_eq!(r.codes(0.1), [IN_THE_MIDDLE]);
    assert_eq!(r.codes(3.1), [STILL_MIDDLE]);
}

#[test]
fn still_variants_never_repeat_and_all_appear() {
    for (code, n, x) in [
        (STILL_RIGHT, STILL_RIGHT_VARIANTS, -3.0f32),
        (STILL_LEFT, STILL_LEFT_VARIANTS, 3.0),
    ] {
        let mut sp = Spotter::seeded(0);
        let mut r = practice();
        place(&mut r.d, 1, x, 0.0);
        let mut got = Vec::new();
        let mut t = 1000.0;
        while got.len() < 300 {
            t += 1.0 / HZ;
            if let Some(c) = sp.update(&r.d, t, true, false)
                && c.code == code
            {
                got.push(c.variant);
            }
        }
        assert!(got.windows(2).all(|w| w[0] != w[1]), "{code}: {got:?}");
        assert!((0..n).all(|v| got.contains(&v)), "{code}: {got:?}");
        assert!(got.iter().all(|v| *v < n), "{code}");
    }
    let mut sp = Spotter::seeded(u64::MAX);
    let mut r = practice();
    place(&mut r.d, 1, 3.0, 0.0);
    place(&mut r.d, 2, -3.0, 0.0);
    let mut got = Vec::new();
    let mut t = 1000.0;
    while got.len() < 300 {
        t += 1.0 / HZ;
        if let Some(c) = sp.update(&r.d, t, true, false)
            && c.code == STILL_MIDDLE
        {
            got.push(c.variant);
        }
    }
    assert!(got.windows(2).all(|w| w[0] != w[1]), "{got:?}");
    assert!(
        (0..STILL_MIDDLE_VARIANTS).all(|v| got.contains(&v)),
        "{got:?}"
    );
}

#[test]
fn every_gate_keeps_it_silent_and_closing_one_resets_without_a_clear() {
    type Edit = fn(&mut Run);
    let gates: [(&str, Edit); 8] = [
        ("player in the pit lane", |r| {
            r.d.pit_mode = pit_mode::DRIVING_INTO_PITS
        }),
        ("full-course yellow", |r| r.d.yellow_flag_state = 4),
        ("private session", |r| r.d.session_is_private = 1),
        ("below 10 m/s", |r| {
            r.d.speed = 9.0;
            r.d.speeds[1] = 9.0;
        }),
        ("viewed car is not the player", |r| r.view_ok = false),
        ("time attack", |r| {
            r.d.session_state = session_state::TIME_ATTACK
        }),
        ("formation lap", |r| {
            r.d.session_state = session_state::FORMATION_LAP
        }),
        ("invalid session", |r| r.d.session_state = 0),
    ];
    for (name, close) in gates {
        let mut r = practice();
        place(&mut r.d, 1, 3.0, 0.0);
        assert_eq!(r.codes(0.1), [CAR_LEFT], "{name}");
        close(&mut r);
        assert!(r.codes(1.0).is_empty(), "{name}: closing it is silent");
        assert!(r.sp.idle(), "{name}");
    }
    // the other car in the pit lane is not a candidate
    let mut r = practice();
    place(&mut r.d, 1, 3.0, 0.0);
    r.d.pit_modes[1] = pit_mode::IN_PIT;
    assert!(r.codes(1.0).is_empty());
    // reopening a gate calls an overlap that is still there
    let mut r = practice();
    place(&mut r.d, 1, 3.0, 0.0);
    r.d.pit_mode = pit_mode::DRIVING_INTO_PITS;
    assert!(r.codes(0.5).is_empty());
    r.d.pit_mode = pit_mode::NONE;
    assert_eq!(r.codes(0.1), [CAR_LEFT]);
}

#[test]
fn the_detectors_quiet_time_gates_it() {
    let mut r = practice();
    place(&mut r.d, 1, 3.0, 0.0);
    assert_eq!(r.sp.update(&r.d, 1000.0, true, true), None);
    assert_eq!(
        r.sp.update(&r.d, 1000.02, true, false).map(|c| c.code),
        Some(CAR_LEFT)
    );
}

#[test]
fn in_a_race_it_waits_20_s_after_the_green_and_not_before() {
    let mut r = Run::new(session_state::RACE);
    r.d.race_state = 1; // not started
    place(&mut r.d, 1, 3.0, 0.0);
    assert!(r.codes(5.0).is_empty());
    r.d.race_state = 2;
    let ev = r.run(25.0);
    assert_eq!(ev.first().map(|e| e.1), Some(CAR_LEFT));
    assert!(ev[0].0 - 1005.0 >= RACE_DELAY_S - 0.05, "{ev:?}");
    assert!(ev[0].0 - 1005.0 < RACE_DELAY_S + 0.1);
    // practice has no delay
    let mut p = practice();
    place(&mut p.d, 1, 3.0, 0.0);
    assert_eq!(p.codes(0.1), [CAR_LEFT]);
}

#[test]
fn the_heading_at_any_yaw_gives_the_same_sides() {
    let pi = std::f32::consts::PI;
    for yaw in [0.0, pi / 2.0, -pi / 2.0, pi, -pi, 0.7, -2.4] {
        let mut r = practice();
        r.d.orientation[1] = yaw;
        place(&mut r.d, 1, 3.0, -1.0);
        place(&mut r.d, 2, -3.0, 2.0);
        assert_eq!(r.codes(0.1), [IN_THE_MIDDLE], "yaw {yaw}");
        away(&mut r.d, 2);
        assert_eq!(r.codes(0.5), [CAR_LEFT], "yaw {yaw}");
    }
}

#[test]
fn the_spotter_budget_is_120_per_minute_in_its_own_window() {
    let mut w = RateWindow::new(SPOTTER_RATE_LIMIT, RATE_WINDOW_S);
    for i in 0..120 {
        assert!(w.admit(10.0 + f64::from(i) * 0.1).is_some());
    }
    assert!(w.admit(30.0).is_none());
    assert!(w.admit(70.0).is_some());
}

#[test]
fn the_nine_calls_are_spotter_level_with_one_slot_and_no_params() {
    let codes = [
        CAR_LEFT,
        CAR_RIGHT,
        IN_THE_MIDDLE,
        STILL_RIGHT,
        STILL_LEFT,
        STILL_MIDDLE,
        CLEAR_LEFT,
        CLEAR_RIGHT,
        ALL_CLEAR,
    ];
    for c in codes {
        let m = &messages()[c];
        assert!(is_spotter(c), "{c}");
        assert_eq!(m["group"], "spotter", "{c}");
        assert_eq!(m["slot"], "spotter", "{c}");
        assert_eq!(m["audience"], "driver", "{c}");
        assert_eq!(m["kind"], "announcement", "{c}");
        assert!(m.get("params").is_none(), "{c}");
        assert!(m["max_wait_s"].as_f64().unwrap() <= 3.0, "{c}");
    }
    assert!(!is_spotter("flag_red"));
    assert!(!is_spotter("nope"));
}

#[test]
fn the_detector_sends_the_calls_with_empty_params_and_can_turn_it_off() {
    for on in [true, false] {
        let clock = FakeClock::new();
        clock.set(1000.0);
        let mut det = EventDetector::new(clock.clone(), 2.0);
        if !on {
            det = det.without_deferred_yellow();
        }
        let mut d = frame(session_state::PRACTICE);
        d.track_length = 4000.0;
        away(&mut d, 2);
        place(&mut d, 1, 3.0, 0.0);
        let mut got = Vec::new();
        for _ in 0..(HZ as usize * 12) {
            clock.advance(1.0 / HZ);
            d.sequence_number += 2;
            got.extend(det.update(&d));
        }
        let spot: Vec<_> = got
            .iter()
            .filter(|a| a.code.starts_with("spotter_"))
            .collect();
        if on {
            // silent through the 10 s quiet time after the load, then the car left
            assert_eq!(spot.len(), 1, "{got:?}");
            assert_eq!(spot[0].code, CAR_LEFT);
            assert!(spot[0].params.is_empty());
            assert_eq!(
                check_params(
                    spot[0].code,
                    &Value::Object(spot[0].params.clone()),
                    messages()
                ),
                None
            );
        } else {
            assert!(spot.is_empty());
        }
    }
}
