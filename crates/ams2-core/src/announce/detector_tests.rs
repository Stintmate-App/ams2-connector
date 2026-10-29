//! Edge detection of live race events (PRY-052) on synthetic shared memory snapshots: a port of
//! the Python connector's `test_announce_detector.py`, plus a replay of a seeded random run
//! whose announcements the Python detector produced (`fixtures/detector_replay.json.gz`).

use super::catalog::{check_params, messages};
use super::detector::{
    Announcement, BLUE_MAX_PER_CAR, BLUE_NEXT_GAP_S, BLUE_TRAIN_GRACE_S, EventDetector, flag, race,
    schedule, yellow,
};
use crate::codes::{game_state, pit_mode, session_state};
use crate::testing::FakeClock;
use ams2_shm::{SharedMemory, new_frame};
use serde_json::{Value, json};

const LENGTH: f32 = 4000.0;
const HZ: f64 = 50.0;

fn put(dst: &mut [u8], text: &str) {
    dst.fill(0);
    dst[..text.len()].copy_from_slice(text.as_bytes());
}

fn make_race(n: usize, session: u32, laps: u32) -> Box<SharedMemory> {
    let mut d = new_frame();
    d.version = 14;
    d.sequence_number = 2;
    d.game_state = game_state::INGAME_PLAYING;
    d.session_state = session;
    d.race_state = race::RACING as u32;
    d.track_length = LENGTH;
    d.laps_in_event = laps;
    d.session_duration = 0.0;
    d.viewed_participant_index = 0;
    d.num_participants = n as i32;
    d.highest_flag_colour = flag::GREEN as u32;
    d.yellow_flag_state = yellow::NONE as i32;
    d.speed = 50.0;
    for i in 0..n {
        let p = &mut d.participant_info[i];
        p.is_active = 1;
        put(
            &mut p.name,
            &if i == 0 {
                "Me".to_string()
            } else {
                format!("Rival {i}")
            },
        );
        p.race_position = if i > 0 { i as u32 + 1 } else { 3 };
        p.current_lap = 3;
        p.laps_completed = 2;
        p.current_lap_distance = 1000.0 + 100.0 * i as f32;
        p.current_sector = 1;
        put(&mut d.car_class_names[i], "GT3");
        d.race_states[i] = race::RACING as u32;
        d.speeds[i] = 40.0;
    }
    d.participant_info[1].race_position = 1;
    d.participant_info[2].race_position = 2;
    d
}

fn race6() -> Box<SharedMemory> {
    make_race(6, session_state::RACE, 10)
}

struct Run {
    clock: FakeClock,
    det: EventDetector,
    d: Box<SharedMemory>,
}

impl Run {
    fn new(d: Box<SharedMemory>) -> Self {
        let clock = FakeClock::new();
        clock.set(1000.0);
        let det = EventDetector::new(clock.clone(), 2.0);
        Self { clock, det, d }
    }

    fn for_s(&mut self, seconds: f64) -> Vec<Announcement> {
        let mut out = Vec::new();
        let ticks = ((seconds * HZ).round() as usize).max(1);
        for _ in 0..ticks {
            self.clock.advance(1.0 / HZ);
            self.d.sequence_number += 2;
            out.extend(self.det.update(&self.d));
        }
        for a in &out {
            // every announcement fits its declaration
            assert_eq!(
                check_params(a.code, &Value::Object(a.params.clone()), messages()),
                None,
                "{a:?}"
            );
        }
        out
    }

    fn codes(&mut self, seconds: f64) -> Vec<&'static str> {
        self.for_s(seconds).iter().map(|a| a.code).collect()
    }

    fn settle(&mut self) {
        assert!(self.codes(11.0).is_empty()); // the quiet period after the session load
    }
}

fn started() -> Run {
    let mut r = Run::new(race6());
    r.settle();
    r
}

fn in_session(session: u32) -> Run {
    let mut r = Run::new(make_race(6, session, 10));
    r.settle();
    r
}

// ------------------------------------------------------------------ flags

#[test]
fn yellow_fires_after_the_debounce_with_the_sector() {
    let mut r = started();
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert!(r.codes(0.4).is_empty());
    let out = r.for_s(0.2);
    assert_eq!(
        out.iter().map(|a| a.code).collect::<Vec<_>>(),
        ["flag_yellow_ahead"]
    );
    assert_eq!(Value::Object(out[0].params.clone()), json!({}));
    assert_eq!(
        Value::Object(out[0].data.clone().unwrap()),
        json!({"lap": 3, "sector": 2, "attribution": "unknown"})
    );
}

#[test]
fn a_one_frame_flicker_is_absorbed() {
    let mut r = started();
    r.d.highest_flag_colour = flag::YELLOW as u32;
    r.for_s(1.0 / HZ);
    r.d.highest_flag_colour = flag::GREEN as u32;
    assert!(r.codes(2.0).is_empty());
}

#[test]
fn yellow_cause_when_the_reason_is_given() {
    let mut r = started();
    r.d.highest_flag_colour = flag::YELLOW as u32;
    r.d.highest_flag_reason = 1;
    let out = r.for_s(1.0);
    let got: Vec<_> = out
        .iter()
        .map(|a| (a.code, Value::Object(a.params.clone())))
        .collect();
    assert_eq!(
        got,
        [
            ("flag_yellow_ahead", json!({})),
            ("flag_yellow_cause", json!({"cause": "solo_crash"}))
        ]
    );
}

#[test]
fn clear_after_the_hold_then_rearm_and_cooldown() {
    let mut r = started();
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert_eq!(r.codes(1.0), ["flag_yellow_ahead"]);
    r.d.highest_flag_colour = flag::GREEN as u32;
    assert!(r.codes(3.0).is_empty()); // 0.5 debounce + 3 s hold not reached yet
    assert_eq!(r.codes(1.0), ["flag_clear"]);
    // A new yellow 2 s later: re-armed (5 s of green) but the 30 s cooldown holds.
    r.codes(2.0);
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert!(r.codes(1.0).is_empty());
    r.d.highest_flag_colour = flag::GREEN as u32;
    r.codes(30.0);
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert_eq!(r.codes(1.0), ["flag_yellow_ahead"]);
}

#[test]
fn clear_only_after_an_announced_yellow() {
    let mut r = started();
    r.d.speed = 0.5; // stopped: the yellow is skipped
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert!(r.codes(1.0).is_empty());
    r.d.highest_flag_colour = flag::GREEN as u32;
    assert!(r.codes(6.0).is_empty()); // no "clear" for a yellow nobody heard
}

#[test]
fn yellow_blue_and_slow_car_are_skipped_when_stopped() {
    for colour in [
        flag::YELLOW,
        flag::DOUBLE_YELLOW,
        flag::BLUE,
        flag::WHITE_SLOW_CAR,
    ] {
        let mut r = started();
        r.d.speed = 0.5;
        r.d.highest_flag_colour = colour as u32;
        assert!(r.codes(1.0).is_empty(), "{colour}");
        r.d.highest_flag_colour = flag::GREEN as u32;
        r.codes(6.0);
        r.d.speed = 30.0;
        r.codes(6.0); // past the window in which the stop would make the flag his own
        r.d.highest_flag_colour = colour as u32;
        assert!(!r.codes(1.0).is_empty(), "{colour}");
    }
}

#[test]
fn yellow_not_rearmed_without_five_seconds_of_green() {
    let mut r = started();
    r.d.highest_flag_colour = flag::YELLOW as u32;
    r.codes(1.0);
    r.codes(40.0); // past the cooldown, still yellow
    r.d.highest_flag_colour = flag::GREEN as u32;
    r.codes(2.0); // green for less than 5 s
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert!(r.codes(1.0).is_empty());
}

#[test]
fn double_yellow_escalates_from_a_yellow() {
    let mut r = started();
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert_eq!(r.codes(1.0), ["flag_yellow_ahead"]);
    r.d.highest_flag_colour = flag::DOUBLE_YELLOW as u32;
    assert_eq!(r.codes(1.0), ["flag_double_yellow_ahead"]);
}

#[test]
fn flags_are_not_announced_in_the_pit_lane() {
    let mut r = started();
    r.d.pit_mode = pit_mode::DRIVING_INTO_PITS;
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert!(r.codes(2.0).is_empty());
    r.d.highest_flag_colour = flag::RED as u32; // a red flag still is
    assert_eq!(r.codes(1.0), ["flag_red"]);
}

#[test]
fn each_flag_colour() {
    for (colour, code) in [
        (flag::WHITE_SLOW_CAR, "flag_slow_car"),
        (flag::RED, "flag_red"),
        (flag::BLACK_AND_WHITE, "flag_black_white"),
        (flag::BLACK_ORANGE_CIRCLE, "flag_black_orange"),
        (flag::BLACK, "disqualified"),
        (flag::WHITE_FINAL_LAP, "final_lap"),
    ] {
        let mut r = started();
        r.d.highest_flag_colour = colour as u32;
        assert_eq!(r.codes(1.0), [code], "{colour}");
    }
}

#[test]
fn black_white_cooldown() {
    let mut r = started();
    for expected in [vec!["flag_black_white"], vec![], vec!["flag_black_white"]] {
        r.d.highest_flag_colour = flag::BLACK_AND_WHITE as u32;
        assert_eq!(r.codes(1.0), expected);
        r.d.highest_flag_colour = flag::GREEN as u32;
        r.codes(if expected.is_empty() { 10.0 } else { 6.0 });
    }
}

#[test]
fn chequered_once_with_the_position() {
    let mut r = started();
    r.d.highest_flag_colour = flag::CHEQUERED as u32;
    let out = r.for_s(1.0);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].code, "chequered");
    assert_eq!(Value::Object(out[0].params.clone()), json!({"position": 3}));
    r.d.race_state = race::FINISHED as u32;
    assert!(r.codes(2.0).is_empty());
}

#[test]
fn chequered_from_the_race_state_without_a_position() {
    let mut r = started();
    r.d.participant_info[0].race_position = 0;
    r.d.race_state = race::FINISHED as u32;
    assert_eq!(r.codes(2.0), ["chequered_no_position"]);
}

// ------------------------------------------------------------------ blue

#[test]
fn blue_names_the_lapping_car_behind() {
    let mut d = race6();
    let lapper = &mut d.participant_info[4];
    lapper.current_lap = 4; // 100 m behind, a lap ahead
    lapper.current_lap_distance = 900.0;
    let mut r = Run::new(d);
    r.settle();
    r.d.highest_flag_colour = flag::BLUE as u32;
    let out = r.for_s(1.0);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].code, "flag_blue");
    assert_eq!(
        Value::Object(out[0].params.clone()),
        json!({"driver": "Rival 4"})
    );
}

#[test]
fn blue_without_a_lapping_car_and_at_most_three_per_car() {
    let mut r = started();
    let mut seen = Vec::new();
    for _ in 0..BLUE_MAX_PER_CAR + 2 {
        r.d.highest_flag_colour = flag::BLUE as u32;
        seen.extend(r.codes(1.0));
        r.d.highest_flag_colour = flag::GREEN as u32;
        r.codes(25.0);
    }
    assert_eq!(seen, vec!["flag_blue_unknown"; BLUE_MAX_PER_CAR as usize]);
}

#[test]
fn blue_only_in_races() {
    let mut r = in_session(session_state::PRACTICE);
    r.d.highest_flag_colour = flag::BLUE as u32;
    assert!(r.codes(2.0).is_empty());
}

/// A race where cars 3, 4 and 5 are a lap ahead and behind the player (100, 300 and 400 m).
fn train() -> Run {
    let mut d = race6();
    for (i, dist) in [(4, 900.0), (3, 700.0), (5, 600.0)] {
        d.participant_info[i].current_lap = 4;
        d.participant_info[i].current_lap_distance = dist;
    }
    let mut r = Run::new(d);
    r.settle();
    r
}

/// Runs `seconds` in 0.25 s steps; (time since `from`, code, driver) of each call.
fn blue_calls(r: &mut Run, from: f64, seconds: f64) -> Vec<(f64, &'static str, String)> {
    let mut out = Vec::new();
    let mut t = 0.0;
    while t < seconds {
        for a in r.for_s(0.25) {
            let who = a.params.get("driver").and_then(Value::as_str).unwrap_or("");
            out.push((from + t, a.code, who.to_string()));
        }
        t += 0.25;
    }
    out
}

/// Car `i` has gone by: it is now ahead of the player on the track.
fn passes(r: &mut Run, i: usize) {
    r.d.participant_info[i].current_lap_distance = 1150.0;
}

#[test]
fn a_train_of_three_cars_under_one_held_flag_is_called_car_by_car() {
    let mut r = train();
    r.d.highest_flag_colour = flag::BLUE as u32;
    let mut calls = blue_calls(&mut r, 0.0, 2.0);
    passes(&mut r, 4);
    calls.extend(blue_calls(&mut r, 2.0, 5.0)); // car 3 is the nearest now
    passes(&mut r, 3);
    calls.extend(blue_calls(&mut r, 7.0, 6.0)); // then car 5
    passes(&mut r, 5);
    calls.extend(blue_calls(&mut r, 13.0, 30.0));
    let codes: Vec<_> = calls.iter().map(|c| c.1).collect();
    assert_eq!(
        codes,
        ["flag_blue", "flag_blue_next", "flag_blue_next"],
        "{calls:?}"
    );
    let who: Vec<_> = calls.iter().map(|c| c.2.as_str()).collect();
    assert_eq!(who, ["Rival 4", "Rival 3", "Rival 5"]);
    assert!(calls[0].0 <= 1.0, "{calls:?}");
    assert!(
        calls[1].0 - calls[0].0 >= BLUE_NEXT_GAP_S - 0.5,
        "{calls:?}"
    );
    assert!(
        calls[2].0 - calls[1].0 >= BLUE_NEXT_GAP_S - 0.5,
        "{calls:?}"
    );
}

#[test]
fn a_new_flag_after_the_old_one_dropped_starts_a_new_episode() {
    let mut r = train();
    r.d.highest_flag_colour = flag::BLUE as u32;
    assert_eq!(r.codes(1.0), ["flag_blue"]);
    r.d.highest_flag_colour = flag::GREEN as u32;
    r.codes(BLUE_TRAIN_GRACE_S + 2.0);
    passes(&mut r, 4);
    r.d.highest_flag_colour = flag::BLUE as u32;
    assert_eq!(r.codes(1.0), ["flag_blue"]); // car 3: a flag off past the grace is a new train
}

#[test]
fn a_flag_that_drops_for_12_s_between_two_cars_is_one_train() {
    let mut r = train();
    r.d.highest_flag_colour = flag::BLUE as u32;
    assert_eq!(r.codes(1.0), ["flag_blue"]); // car 4
    passes(&mut r, 4);
    r.d.highest_flag_colour = flag::GREEN as u32;
    assert!(r.codes(12.0).is_empty());
    r.d.highest_flag_colour = flag::BLUE as u32;
    let out = r.for_s(1.0);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].code, "flag_blue_next");
    assert_eq!(out[0].params["driver"], "Rival 3");
}

#[test]
fn a_car_is_named_once_per_train_even_if_the_flag_drops_and_returns() {
    let mut r = train();
    r.d.highest_flag_colour = flag::BLUE as u32;
    assert_eq!(r.codes(1.0), ["flag_blue"]); // car 4
    for _ in 0..3 {
        r.d.highest_flag_colour = flag::GREEN as u32;
        assert!(r.codes(5.0).is_empty());
        r.d.highest_flag_colour = flag::BLUE as u32;
        // Car 4 is still the nearest lapping car, 20 s have passed: only the next car is named.
        let seen = r.codes(10.0);
        assert!(!seen.contains(&"flag_blue"), "{seen:?}");
    }
    let calls = blue_calls(&mut r, 0.0, 30.0);
    assert!(calls.iter().all(|c| c.2 != "Rival 4"), "{calls:?}");
}

#[test]
fn a_blue_flag_raised_in_the_pit_lane_is_called_when_the_car_leaves_it() {
    let mut r = train();
    r.d.pit_mode = pit_mode::DRIVING_INTO_PITS;
    r.d.highest_flag_colour = flag::BLUE as u32;
    assert!(r.codes(3.0).is_empty());
    r.d.pit_mode = pit_mode::NONE;
    let out = r.for_s(1.0);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].code, "flag_blue"); // the first call of the episode
    assert_eq!(out[0].params["driver"], "Rival 4");
}

#[test]
fn a_held_blue_flag_is_not_called_at_a_standstill() {
    let mut r = train();
    r.d.speed = 0.5;
    r.d.highest_flag_colour = flag::BLUE as u32;
    assert!(r.codes(6.0).is_empty());
    r.d.speed = 30.0;
    assert_eq!(r.codes(1.0), ["flag_blue"]);
}

#[test]
fn the_same_car_is_called_once_per_train_and_at_most_three_times_a_session() {
    let mut r = train();
    let mut calls = Vec::new();
    for _ in 0..5 {
        r.d.highest_flag_colour = flag::BLUE as u32;
        calls.extend(blue_calls(&mut r, 0.0, 30.0)); // car 4 first; 3 and 5 only if they are nearest
        r.d.highest_flag_colour = flag::GREEN as u32;
        r.codes(BLUE_TRAIN_GRACE_S + 5.0);
    }
    let count = |n: &str| calls.iter().filter(|c| c.2 == n).count();
    assert_eq!(count("Rival 4"), BLUE_MAX_PER_CAR as usize, "{calls:?}");
}

// ------------------------------------------------------------------ race states, penalties

#[test]
fn disqualified_once_for_the_flag_and_the_race_state() {
    let mut r = started();
    r.d.highest_flag_colour = flag::BLACK as u32;
    assert_eq!(r.codes(1.0), ["disqualified"]);
    r.d.race_state = race::DISQUALIFIED as u32;
    assert!(r.codes(2.0).is_empty());
}

#[test]
fn retired_once() {
    for state in [race::RETIRED, race::DNF] {
        let mut r = started();
        r.d.race_state = state as u32;
        assert!(r.codes(0.8).is_empty()); // race states hold 1 s
        assert_eq!(r.codes(0.5), ["retired"]);
    }
}

#[test]
fn drive_through_then_served_after_the_pit_lane() {
    let mut r = started();
    r.d.pit_schedule = schedule::DRIVE_THROUGH as u32;
    assert_eq!(r.codes(1.0), ["penalty_drive_through"]);
    r.d.pit_mode = pit_mode::DRIVING_INTO_PITS;
    r.codes(5.0);
    r.d.pit_mode = pit_mode::DRIVING_OUT_OF_PITS;
    r.d.pit_schedule = schedule::NONE as u32;
    assert_eq!(r.codes(1.0), ["penalty_served"]);
}

#[test]
fn a_penalty_cleared_without_a_pit_pass_is_not_served() {
    let mut r = started();
    r.d.pit_schedule = schedule::STOP_GO as u32;
    assert_eq!(r.codes(1.0), ["penalty_stop_go"]);
    r.d.pit_schedule = schedule::NONE as u32;
    assert!(r.codes(2.0).is_empty());
}

#[test]
fn penalties_only_in_races_and_pit_spot_is_not_one() {
    let mut r = in_session(session_state::QUALIFY);
    r.d.pit_schedule = schedule::DRIVE_THROUGH as u32;
    assert!(r.codes(2.0).is_empty());
    let mut r = started();
    r.d.pit_schedule = 7; // PITSPOT_OCCUPIED
    assert!(r.codes(2.0).is_empty());
}

// ------------------------------------------------------------------ the caution

#[test]
fn fcy_straight_to_pits_open_then_the_sequence() {
    let mut r = started();
    r.d.yellow_flag_state = yellow::PITS_OPEN as i32;
    assert_eq!(r.codes(1.0), ["fcy_start"]);
    assert!(r.codes(7.0).is_empty());
    assert_eq!(r.codes(1.0), ["fcy_pits_open"]);
    r.d.yellow_flag_state = yellow::LAST_LAP as i32;
    assert_eq!(r.codes(1.0), ["fcy_last_lap"]);
    r.d.yellow_flag_state = yellow::RESUME as i32;
    assert_eq!(r.codes(1.0), ["fcy_restart"]);
    r.d.yellow_flag_state = yellow::NONE as i32;
    assert!(r.codes(1.0).is_empty());
    r.d.yellow_flag_state = yellow::PENDING as i32; // a second caution: said again
    assert_eq!(r.codes(1.0), ["fcy_start"]);
    r.d.yellow_flag_state = yellow::PIT_LEAD_LAP as i32;
    assert_eq!(r.codes(1.0), ["fcy_pits_lead_lap"]);
}

#[test]
fn fcy_jump_superseded_before_the_delay() {
    let mut r = started();
    r.d.yellow_flag_state = yellow::PITS_OPEN as i32;
    assert_eq!(r.codes(1.0), ["fcy_start"]);
    r.d.yellow_flag_state = yellow::LAST_LAP as i32;
    assert_eq!(r.codes(10.0), ["fcy_last_lap"]);
}

#[test]
fn fcy_halt_and_none_in_practice() {
    let mut r = started();
    r.d.yellow_flag_state = yellow::RACE_HALT as i32;
    assert_eq!(r.codes(1.0), ["fcy_halt"]);
    let mut r = in_session(session_state::PRACTICE);
    r.d.yellow_flag_state = yellow::PITS_OPEN as i32;
    assert!(r.codes(10.0).is_empty());
}

// ------------------------------------------------------------------ final lap from the leader

#[test]
fn final_lap_when_the_leader_starts_the_last_lap_once() {
    let mut r = started();
    r.d.participant_info[1].current_lap = 10;
    assert_eq!(r.codes(1.0), ["final_lap"]);
    r.d.highest_flag_colour = flag::WHITE_FINAL_LAP as u32;
    assert!(r.codes(1.0).is_empty());
}

#[test]
fn no_final_lap_from_the_leader_in_a_timed_race() {
    let mut r = Run::new(race6());
    r.d.session_duration = 30.0;
    r.settle();
    r.d.participant_info[1].current_lap = 10;
    assert!(r.codes(1.0).is_empty());
}

// ------------------------------------------------------------------ the gate

#[test]
fn nothing_in_the_first_ten_seconds_after_the_load() {
    let mut r = Run::new(race6());
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert!(r.codes(9.0).is_empty());
    r.d.highest_flag_colour = flag::GREEN as u32;
    r.codes(6.0);
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert_eq!(r.codes(1.0), ["flag_yellow_ahead"]);
}

#[test]
fn nothing_unless_playing() {
    for state in [
        game_state::INGAME_PAUSED,
        game_state::INGAME_REPLAY,
        game_state::FRONT_END,
        game_state::INGAME_RESTARTING,
    ] {
        let mut r = started();
        r.d.game_state = state;
        r.d.highest_flag_colour = flag::RED as u32;
        assert!(r.codes(3.0).is_empty(), "{state}");
    }
}

#[test]
fn resume_from_pause_keeps_the_session_but_a_reload_is_quiet() {
    let mut r = started();
    r.d.game_state = game_state::INGAME_PAUSED;
    r.codes(2.0);
    r.d.game_state = game_state::INGAME_PLAYING;
    r.d.highest_flag_colour = flag::RED as u32;
    assert_eq!(r.codes(1.0), ["flag_red"]);
    r.d.game_state = game_state::FRONT_END;
    r.codes(2.0);
    r.d.game_state = game_state::INGAME_PLAYING;
    r.d.highest_flag_colour = flag::BLACK_AND_WHITE as u32;
    assert!(r.codes(9.0).is_empty());
}

#[test]
fn spectating_another_car_freezes_the_players_fields() {
    let mut r = started();
    r.d.viewed_participant_index = 2; // watching Rival 2
    r.d.highest_flag_colour = flag::RED as u32; // its flag, not ours
    assert!(r.codes(2.0).is_empty());
    r.d.viewed_participant_index = 0; // back: today's value taken silently
    assert!(r.codes(2.0).is_empty());
    r.d.highest_flag_colour = flag::BLACK_AND_WHITE as u32;
    assert_eq!(r.codes(1.0), ["flag_black_white"]);
}

#[test]
fn session_state_change_is_a_new_session() {
    let mut r = started();
    r.d.highest_flag_colour = flag::CHEQUERED as u32;
    assert_eq!(r.codes(1.0), ["chequered"]);
    r.d.session_state = session_state::PRACTICE; // a new weekend: once-per-session resets
    r.d.highest_flag_colour = flag::GREEN as u32;
    r.codes(11.0);
    r.d.session_state = session_state::RACE;
    r.codes(11.0);
    r.d.highest_flag_colour = flag::CHEQUERED as u32;
    assert_eq!(r.codes(1.0), ["chequered"]);
}

#[test]
fn nothing_fires_at_the_practice_end_reset() {
    let mut r = in_session(session_state::PRACTICE);
    for state in [race::NOT_STARTED, race::INVALID] {
        r.d.race_state = state as u32;
        for i in 0..r.d.num_participants as usize {
            r.d.race_states[i] = state as u32;
        }
        assert!(r.codes(2.0).is_empty());
    }
    r.d.highest_flag_colour = flag::NONE as u32;
    assert!(r.codes(5.0).is_empty());
}

#[test]
fn race_start_is_quiet_for_ten_seconds() {
    let mut d = race6();
    d.race_state = race::NOT_STARTED as u32;
    let mut r = Run::new(d);
    r.settle();
    r.d.race_state = race::RACING as u32;
    r.codes(2.0);
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert!(r.codes(5.0).is_empty());
}

// ------------------------------------------------------------------ rivals

#[test]
fn rival_out_and_disqualified_once_by_identity_across_a_reshuffle() {
    let mut r = started();
    r.d.race_states[3] = race::RETIRED as u32;
    let out = r.for_s(2.0);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].code, "rival_out");
    assert_eq!(
        Value::Object(out[0].params.clone()),
        json!({"driver": "Rival 3"})
    );
    // The arrays reshuffle: Rival 3 moves to slot 5 (still retired), Rival 5 to slot 3.
    put(&mut r.d.participant_info[3].name, "Rival 5");
    put(&mut r.d.participant_info[5].name, "Rival 3");
    r.d.race_states[3] = race::RACING as u32;
    r.d.race_states[5] = race::RETIRED as u32;
    assert!(r.codes(3.0).is_empty());
    r.d.race_states[4] = race::DISQUALIFIED as u32;
    let out = r.for_s(2.0);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].code, "rival_disqualified");
    assert_eq!(
        Value::Object(out[0].params.clone()),
        json!({"driver": "Rival 4"})
    );
    assert_eq!(
        Value::Object(out[0].data.clone().unwrap()),
        json!({"car_class": "GT3"})
    );
}

#[test]
fn rivals_ignore_the_safety_car_inactive_slots_and_the_players_duplicate() {
    let mut r = started();
    put(&mut r.d.participant_info[3].name, "Safety Car");
    r.d.race_states[3] = race::RETIRED as u32;
    r.d.participant_info[4].is_active = 0;
    r.d.race_states[4] = race::DNF as u32;
    put(&mut r.d.participant_info[5].name, "Me");
    r.d.race_states[5] = race::DISQUALIFIED as u32;
    assert!(r.codes(3.0).is_empty());
}

#[test]
fn rivals_silent_once_the_leader_finished() {
    let mut r = started();
    r.d.race_states[1] = race::FINISHED as u32; // the leader
    r.codes(2.0);
    r.d.race_states[3] = race::DNF as u32;
    assert!(r.codes(3.0).is_empty());
}

#[test]
fn rivals_only_in_races() {
    let mut r = in_session(session_state::QUALIFY);
    r.d.race_states[3] = race::RETIRED as u32;
    assert!(r.codes(3.0).is_empty());
}

#[test]
fn reset_forgets_everything() {
    let mut r = started();
    r.d.highest_flag_colour = flag::BLACK as u32;
    assert_eq!(r.codes(1.0), ["disqualified"]);
    r.det.reset();
    r.d.highest_flag_colour = flag::GREEN as u32;
    r.settle();
    r.d.highest_flag_colour = flag::BLACK as u32;
    assert_eq!(r.codes(1.0), ["disqualified"]);
}

#[test]
fn every_announced_code_is_in_the_catalog() {
    let src = include_str!("detector.rs");
    let msgs = messages();
    let mut used = 0;
    for token in src.split('"').skip(1).step_by(2) {
        let is_code = token.starts_with("flag_")
            || token.starts_with("fcy_")
            || token.starts_with("penalty_")
            || token.starts_with("rival_")
            || matches!(
                token,
                "final_lap" | "disqualified" | "retired" | "chequered" | "chequered_no_position"
            );
        if is_code && token.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            assert!(msgs.contains_key(token), "{token}");
            used += 1;
        }
    }
    assert!(used > 20);
}

// ------------------------------------------------------------------ the Python detector's run

fn read_gz(path: &str) -> Value {
    use std::io::Read;
    let raw = std::fs::read(format!("{}/fixtures/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let mut text = String::new();
    flate2::read::GzDecoder::new(&raw[..])
        .read_to_string(&mut text)
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

fn apply(d: &mut SharedMemory, name: &str, v: &Value) {
    let f = v.as_f64();
    let u = || v.as_u64().unwrap() as u32;
    let pair = || {
        let a = v.as_array().unwrap();
        (a[0].as_u64().unwrap() as usize, &a[1])
    };
    match name {
        "highest_flag_colour" => d.highest_flag_colour = u(),
        "highest_flag_reason" => d.highest_flag_reason = u(),
        "yellow_flag_state" => d.yellow_flag_state = v.as_i64().unwrap() as i32,
        "pit_mode" => d.pit_mode = u(),
        "pit_schedule" => d.pit_schedule = u(),
        "speed" => d.speed = f.unwrap() as f32,
        "race_state" => d.race_state = u(),
        "game_state" => d.game_state = u(),
        "session_state" => d.session_state = u(),
        "viewed_participant_index" => d.viewed_participant_index = v.as_i64().unwrap() as i32,
        "laps_in_event" => d.laps_in_event = u(),
        "session_duration" => d.session_duration = f.unwrap() as f32,
        "race_states" => {
            let (i, x) = pair();
            d.race_states[i] = x.as_u64().unwrap() as u32;
        }
        "lap" => {
            let (i, x) = pair();
            d.participant_info[i].current_lap = x.as_u64().unwrap() as u32;
        }
        "dist" => {
            let (i, x) = pair();
            d.participant_info[i].current_lap_distance = x.as_f64().unwrap() as f32;
        }
        "is_active" => {
            let (i, x) = pair();
            d.participant_info[i].is_active = u8::from(x.as_u64().unwrap() != 0);
        }
        "rename" => {
            let (i, x) = pair();
            put(&mut d.participant_info[i].name, x.as_str().unwrap());
        }
        "sector" => d.participant_info[0].current_sector = v.as_i64().unwrap() as i32,
        other => panic!("unknown mutation {other}"),
    }
}

#[test]
fn replay_of_a_seeded_random_run_equals_the_python_detectors_announcements() {
    let fx = read_gz("detector_replay.json.gz");
    let ticks = fx["ticks"].as_u64().unwrap();
    let mut steps = std::collections::HashMap::new();
    for s in fx["steps"].as_array().unwrap() {
        steps.insert(s[0].as_u64().unwrap(), s[1].as_array().unwrap());
    }
    let mut r = Run::new(make_race(8, session_state::RACE, 6));
    r.det = EventDetector::new(r.clock.clone(), 2.0).without_deferred_yellow();
    let mut got = Vec::new();
    for tick in 0..ticks {
        for m in steps.get(&tick).into_iter().flat_map(|v| v.iter()) {
            let (name, v) = (m[0].as_str().unwrap(), &m[1]);
            if name == "reset" {
                r.det.reset();
            } else {
                apply(&mut r.d, name, v);
            }
        }
        r.clock.advance(1.0 / HZ);
        r.d.sequence_number += 2;
        for a in r.det.update(&r.d) {
            if a.code.starts_with("flag_blue") {
                continue; // BUG-139: the blue flag is followed now; tested apart
            }
            got.push(json!({
                "tick": tick, "code": a.code, "params": a.params,
                "data": a.data.map_or(Value::Null, Value::Object),
            }));
        }
    }
    let want: Vec<_> = fx["events"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| !e["code"].as_str().unwrap().starts_with("flag_blue"))
        .cloned()
        .collect();
    assert!(want.len() >= 50, "the fixture has {} events", want.len());
    assert_eq!(got.len(), want.len());
    for (g, w) in got.iter().zip(&want) {
        assert_eq!(g, w);
    }
}

#[test]
fn yellow_raised_while_stopped_is_said_when_the_car_moves() {
    // BUG-130: a contact stops the car and the game raises the yellow a moment later; the
    // yellow still holds when the car rolls on, so it is said then (once).
    let mut r = started();
    r.d.speed = 0.1;
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert!(r.codes(2.0).is_empty());
    r.d.speed = 4.0;
    assert_eq!(r.codes(0.2), ["flag_yellow_self"]);
    assert!(r.codes(5.0).is_empty());
    // A yellow that ended before the car moved is never said.
    let mut r = started();
    r.d.speed = 0.1;
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert!(r.codes(2.0).is_empty());
    r.d.highest_flag_colour = flag::GREEN as u32;
    assert!(r.codes(1.0).is_empty());
    r.d.speed = 30.0;
    assert!(r.codes(6.0).is_empty());
}

// ------------------------------------------------------------------ PRY-090: whose flag

fn params_of(a: &Announcement) -> Value {
    Value::Object(a.params.clone())
}

fn data_of(a: &Announcement) -> Value {
    Value::Object(a.data.clone().unwrap())
}

/// A stopped car at `distance` metres into the lap.
fn stop_car(r: &mut Run, i: usize, distance: f32) {
    r.d.speeds[i] = 0.0;
    r.d.participant_info[i].current_lap_distance = distance;
}

#[test]
fn self_off_track_says_the_note_not_the_yellow() {
    let mut r = started();
    r.d.crash_state = 1;
    r.d.speed = 2.0;
    r.codes(2.0);
    r.d.highest_flag_colour = flag::DOUBLE_YELLOW as u32;
    r.d.highest_flag_reason = 3;
    let out = r.for_s(1.0);
    assert_eq!(
        out.iter().map(|a| a.code).collect::<Vec<_>>(),
        ["flag_yellow_self"]
    );
    assert_eq!(params_of(&out[0]), json!({"situation": "off_track"}));
    assert_eq!(
        data_of(&out[0]),
        json!({"lap": 3, "sector": 2, "attribution": "self"})
    );
    // the flag clears: no "track clear" for a flag the radio did not announce
    r.d.highest_flag_colour = flag::GREEN as u32;
    assert!(r.codes(8.0).is_empty());
}

#[test]
fn self_stopped_on_track_after_a_contact() {
    // The race shape: the surface under the car, 0.1 m/s, a single yellow with a reason.
    let mut r = started();
    r.d.speed = 0.1;
    r.d.highest_flag_colour = flag::YELLOW as u32;
    r.d.highest_flag_reason = 3;
    assert!(r.codes(2.0).is_empty());
    r.d.speed = 4.0; // he drives off: the note is useful now
    let out = r.for_s(0.2);
    assert_eq!(
        out.iter().map(|a| a.code).collect::<Vec<_>>(),
        ["flag_yellow_self"]
    );
    assert_eq!(params_of(&out[0]), json!({"situation": "stopped"}));
    assert!(r.codes(5.0).is_empty());
    r.d.highest_flag_colour = flag::GREEN as u32;
    assert!(r.codes(8.0).is_empty()); // and no "track clear" after it
}

#[test]
fn self_from_the_window() {
    for (wait, expected) in [
        (4.0, vec!["flag_yellow_self"]),
        (6.0, vec!["flag_yellow_ahead"]),
    ] {
        let mut r = started();
        r.d.speed = 2.0;
        r.codes(0.2);
        r.d.speed = 6.0;
        r.codes(wait);
        r.d.highest_flag_colour = flag::YELLOW as u32;
        assert_eq!(r.codes(1.0), expected, "{wait}");
    }
}

#[test]
fn escalation_of_a_self_yellow_is_silent() {
    let mut r = started();
    r.d.speed = 2.0;
    r.codes(0.2);
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert_eq!(r.codes(1.0), ["flag_yellow_self"]);
    r.d.highest_flag_colour = flag::DOUBLE_YELLOW as u32;
    assert!(r.codes(2.0).is_empty());
}

#[test]
fn other_car_stopped_ahead_names_sector_and_driver() {
    let mut r = started();
    stop_car(&mut r, 3, 1400.0);
    r.d.participant_info[3].current_sector = 2;
    r.d.highest_flag_colours[3] = flag::DOUBLE_YELLOW as u32;
    r.d.highest_flag_colour = flag::DOUBLE_YELLOW as u32;
    r.d.highest_flag_reason = 3;
    let out = r.for_s(1.0);
    assert_eq!(
        out.iter().map(|a| a.code).collect::<Vec<_>>(),
        ["flag_double_yellow", "flag_yellow_driver"]
    );
    assert_eq!(params_of(&out[0]), json!({"sector": 3}));
    assert_eq!(
        params_of(&out[1]),
        json!({"driver": "Rival 3", "cause": "obstruction"})
    );
    assert_eq!(
        data_of(&out[0]),
        json!({"lap": 3, "sector": 2, "attribution": "car",
               "incident_sector": 3, "driver": "Rival 3"})
    );
}

#[test]
fn a_car_without_a_reason_is_slow_on_track() {
    let mut r = started();
    stop_car(&mut r, 2, 1200.0);
    r.d.highest_flag_colour = flag::YELLOW as u32;
    let out = r.for_s(1.0);
    assert_eq!(
        out.iter().map(|a| a.code).collect::<Vec<_>>(),
        ["flag_yellow", "flag_yellow_driver"]
    );
    assert_eq!(
        params_of(&out[1]),
        json!({"driver": "Rival 2", "cause": "unknown"})
    );
}

#[test]
fn other_car_wins_over_a_fast_off() {
    let mut r = started();
    r.d.crash_state = 1;
    r.d.speed = 30.0;
    stop_car(&mut r, 3, 1400.0);
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert_eq!(r.codes(1.0), ["flag_yellow", "flag_yellow_driver"]);
}

#[test]
fn a_fast_off_with_nobody_stopped_is_still_an_off() {
    let mut r = started();
    r.d.crash_state = 1;
    r.d.speed = 30.0;
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert_eq!(r.codes(1.0), ["flag_yellow_self"]);
}

#[test]
fn own_flag_preferred() {
    let mut r = make_run(8);
    stop_car(&mut r, 2, 1200.0);
    stop_car(&mut r, 4, 1700.0);
    r.d.highest_flag_colours[4] = flag::YELLOW as u32;
    r.d.highest_flag_colour = flag::YELLOW as u32;
    let out = r.for_s(1.0);
    assert_eq!(params_of(&out[1])["driver"], "Rival 4");
}

fn make_run(n: usize) -> Run {
    let mut r = Run::new(make_race(n, session_state::RACE, 10));
    r.settle();
    r
}

#[test]
fn candidate_filters() {
    let mut r = make_run(12);
    stop_car(&mut r, 1, 1100.0);
    r.d.pit_modes[1] = pit_mode::IN_PIT; // in the pit lane
    stop_car(&mut r, 2, 1200.0);
    put(&mut r.d.participant_info[2].name, "Safety Car"); // the Safety Car
    stop_car(&mut r, 3, 1300.0);
    r.d.participant_info[3].is_active = 0; // an inactive slot
    stop_car(&mut r, 4, 1400.0);
    put(&mut r.d.participant_info[4].name, "Me"); // the player's duplicate
    stop_car(&mut r, 5, 600.0); // 400 m behind
    stop_car(&mut r, 6, 2600.0); // 1600 m ahead
    r.d.speeds[7] = 7.0; // too fast to be stopped
    r.d.race_states[8] = race::FINISHED as u32; // done for good
    stop_car(&mut r, 8, 1800.0);
    r.d.highest_flag_colour = flag::YELLOW as u32;
    r.d.highest_flag_reason = 1;
    assert_eq!(r.codes(1.0), ["flag_yellow_ahead", "flag_yellow_cause"]);
}

#[test]
fn candidates_just_inside_the_range() {
    // 350 m behind and 1500 m ahead count.
    for (i, distance) in [(5, 650.0), (6, 2500.0)] {
        let mut r = make_run(8);
        stop_car(&mut r, i, distance);
        r.d.highest_flag_colour = flag::YELLOW as u32;
        assert_eq!(
            r.codes(1.0),
            ["flag_yellow", "flag_yellow_driver"],
            "{distance}"
        );
    }
}

#[test]
fn gap_wraps_at_the_line() {
    // 100 m before the line, a car 200 m after it: 300 m ahead; and the reverse, 300 m behind.
    for (mine, theirs) in [(3900.0, 200.0), (200.0, 3900.0)] {
        let mut r = make_run(6);
        r.d.participant_info[0].current_lap_distance = mine;
        stop_car(&mut r, 3, theirs);
        r.d.highest_flag_colour = flag::YELLOW as u32;
        assert_eq!(
            r.codes(1.0),
            ["flag_yellow", "flag_yellow_driver"],
            "{mine}"
        );
    }
}

#[test]
fn no_track_length_no_candidate() {
    let mut r = started();
    stop_car(&mut r, 3, 1400.0);
    r.d.track_length = 0.0;
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert_eq!(r.codes(1.0), ["flag_yellow_ahead"]);
}

#[test]
fn only_the_declared_participants_are_read() {
    let mut r = started();
    r.d.num_participants = 3;
    stop_car(&mut r, 4, 1400.0); // beyond mNumParticipants
    r.d.highest_flag_colour = flag::YELLOW as u32;
    assert_eq!(r.codes(1.0), ["flag_yellow_ahead"]);
}

#[test]
fn track_limits_after_an_invalidation() {
    let mut r = started();
    r.d.lap_invalidated = 1;
    r.codes(0.02);
    r.d.highest_flag_colour = flag::BLACK_AND_WHITE as u32;
    let out = r.for_s(1.0);
    assert_eq!(
        out.iter().map(|a| a.code).collect::<Vec<_>>(),
        ["flag_track_limits"]
    );
    assert_eq!(data_of(&out[0]), json!({"lap": 3, "sector": 2}));
}

#[test]
fn black_white_without_an_invalidation() {
    let mut r = started();
    r.d.highest_flag_colour = flag::BLACK_AND_WHITE as u32;
    assert_eq!(r.codes(1.0), ["flag_black_white"]);
    // an invalidation long before is not a cut now
    let mut r = started();
    r.d.lap_invalidated = 1;
    r.codes(5.0);
    r.d.highest_flag_colour = flag::BLACK_AND_WHITE as u32;
    assert_eq!(r.codes(1.0), ["flag_black_white"]);
}

#[test]
fn black_white_on_an_invalid_out_lap() {
    // The level is already 1 when the session starts: it never rises while playing.
    let mut r = Run::new(race6());
    r.d.lap_invalidated = 1;
    r.settle();
    r.d.highest_flag_colour = flag::BLACK_AND_WHITE as u32;
    assert_eq!(r.codes(1.0), ["flag_black_white"]);
}

#[test]
fn slow_car_names_the_car_ahead() {
    let mut r = started();
    r.d.speeds[1] = 10.0; // 100 m ahead, under 60 % of 50 m/s
    r.d.speeds[2] = 29.0; // 200 m ahead, also slow: the nearest wins
    r.d.highest_flag_colour = flag::WHITE_SLOW_CAR as u32;
    let out = r.for_s(1.0);
    assert_eq!(
        out.iter().map(|a| a.code).collect::<Vec<_>>(),
        ["flag_slow_car_driver"]
    );
    assert_eq!(params_of(&out[0]), json!({"driver": "Rival 1"}));
}

#[test]
fn slow_car_self_is_silent() {
    let mut r = started();
    r.d.speed = 3.0;
    r.d.speeds[1] = 1.0;
    r.d.highest_flag_colour = flag::WHITE_SLOW_CAR as u32;
    assert!(r.codes(1.0).is_empty());
}

#[test]
fn slow_car_unknown() {
    // A slow car behind, or too far ahead, or in the pit lane, is not named.
    let mut r = started();
    r.d.speeds[1] = 10.0;
    r.d.participant_info[1].current_lap_distance = 400.0; // behind
    r.d.speeds[2] = 10.0;
    r.d.participant_info[2].current_lap_distance = 1600.0; // 600 m ahead
    r.d.speeds[3] = 10.0;
    r.d.pit_modes[3] = pit_mode::IN_PIT;
    r.d.highest_flag_colour = flag::WHITE_SLOW_CAR as u32;
    assert_eq!(r.codes(1.0), ["flag_slow_car"]);
}
