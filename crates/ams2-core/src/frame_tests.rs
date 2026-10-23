//! The live frame (`connector.live_frame`, contract 5.3): the conversion of every fixture
//! snapshot equals Pitwall's fixture, the pit codes, why the lap is not timed or not valid
//! (PRY-037). Every frame is checked against the protocol schema's `$defs/live_frame`. A port of
//! the Python connector's `test_frame.py`; the 307 snapshots come as specs in
//! `fixtures/live_frame_cases.json.gz` (written by `scripts/gen_announce_fixtures.py` from the
//! Python `live_cases`), the expected frames are Pitwall's.

use crate::codes::{game_state, pit_mode, session_state};
use crate::frame::{FrameOptions, live_frame};
use crate::schema_check;
use crate::tracker::LiveLap;
use ams2_shm::{SharedMemory, new_frame};
use serde_json::{Map, Value};

fn read_gz(path: &str) -> Value {
    use std::io::Read;
    let raw = std::fs::read(format!("{}/fixtures/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap();
    let mut text = String::new();
    flate2::read::GzDecoder::new(&raw[..])
        .read_to_string(&mut text)
        .unwrap();
    serde_json::from_str(&text).unwrap()
}

/// The protocol schema (first) and the write API schema it references.
fn schema() -> Vec<Value> {
    ["connector-protocol", "write-api"]
        .iter()
        .map(|n| {
            let raw = std::fs::read_to_string(format!(
                "{}/fixtures/{n}.schema.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .unwrap();
            serde_json::from_str(&raw).unwrap()
        })
        .collect()
}

fn put(dst: &mut [u8], text: &str) {
    dst.fill(0);
    dst[..text.len()].copy_from_slice(text.as_bytes());
}

fn arr4(v: &Value) -> [f32; 4] {
    let a = v.as_array().unwrap();
    std::array::from_fn(|i| a[i].as_f64().unwrap() as f32)
}

/// A snapshot from one spec of the fixture.
fn build(spec: &Value) -> Box<SharedMemory> {
    let mut d = new_frame();
    d.version = 14;
    d.sequence_number = 100;
    for (k, v) in spec["scalars"].as_object().unwrap() {
        let f = v.as_f64().unwrap();
        match k.as_str() {
            "game_state" => d.game_state = f as u32,
            "session_state" => d.session_state = f as u32,
            "race_state" => d.race_state = f as u32,
            "viewed_participant_index" => d.viewed_participant_index = f as i32,
            "num_participants" => d.num_participants = f as i32,
            "laps_in_event" => d.laps_in_event = f as u32,
            "track_length" => d.track_length = f as f32,
            "lap_invalidated" => d.lap_invalidated = f as u8,
            "best_lap_time" => d.best_lap_time = f as f32,
            "last_lap_time" => d.last_lap_time = f as f32,
            "current_time" => d.current_time = f as f32,
            "split_time_ahead" => d.split_time_ahead = f as f32,
            "split_time_behind" => d.split_time_behind = f as f32,
            "event_time_remaining" => d.event_time_remaining = f as f32,
            "pit_mode" => d.pit_mode = f as u32,
            "pit_schedule" => d.pit_schedule = f as u32,
            "fuel_level" => d.fuel_level = f as f32,
            "fuel_capacity" => d.fuel_capacity = f as f32,
            "speed" => d.speed = f as f32,
            "rpm" => d.rpm = f as f32,
            "gear" => d.gear = f as i32,
            "ambient_temperature" => d.ambient_temperature = f as f32,
            "track_temperature" => d.track_temperature = f as f32,
            "rain_density" => d.rain_density = f as f32,
            "wind_speed" => d.wind_speed = f as f32,
            "session_duration" => d.session_duration = f as f32,
            "session_additional_laps" => d.session_additional_laps = f as i32,
            other => panic!("unknown scalar {other}"),
        }
    }
    for (k, v) in spec["texts"].as_object().unwrap() {
        let t = v.as_str().unwrap();
        match k.as_str() {
            "car_name" => put(&mut d.car_name, t),
            "car_class_name" => put(&mut d.car_class_name, t),
            "track_location" => put(&mut d.track_location, t),
            "track_variation" => put(&mut d.track_variation, t),
            other => panic!("unknown text {other}"),
        }
    }
    for (k, v) in spec["wheels"].as_object().unwrap() {
        let a = arr4(v);
        match k.as_str() {
            "tyre_temp_left" => d.tyre_temp_left = a,
            "tyre_temp_right" => d.tyre_temp_right = a,
            "tyre_temp_center" => d.tyre_temp_center = a,
            "tyre_carcass_temp" => d.tyre_carcass_temp = a,
            "tyre_wear" => d.tyre_wear = a,
            "brake_temp_celsius" => d.brake_temp_celsius = a,
            "air_pressure" => d.air_pressure = a,
            other => panic!("unknown wheel array {other}"),
        }
    }
    for (i, row) in spec["participants"].as_array().unwrap().iter().enumerate() {
        let f = |k: &str| row[k].as_f64().unwrap();
        let p = &mut d.participant_info[i];
        p.is_active = f("is_active") as u8;
        put(&mut p.name, row["name"].as_str().unwrap());
        p.race_position = f("race_position") as u32;
        p.laps_completed = f("laps_completed") as u32;
        p.current_lap = f("current_lap") as u32;
        p.current_lap_distance = f("current_lap_distance") as f32;
        p.current_sector = f("current_sector") as i32;
        put(&mut d.car_names[i], row["car_name"].as_str().unwrap());
        put(
            &mut d.car_class_names[i],
            row["class_name"].as_str().unwrap(),
        );
        d.pit_modes[i] = f("pit_modes") as u32;
        d.last_lap_times[i] = f("last_lap_times") as f32;
        d.fastest_lap_times[i] = f("fastest_lap_times") as f32;
    }
    d
}

/// Python's `live_cases.shm(session)`: the first spec with another session state.
fn shm(session: u32) -> Box<SharedMemory> {
    let fx = read_gz("live_frame_cases.json.gz");
    let mut d = build(&fx["cases"][0]);
    d.session_state = session;
    d
}

/// Removes the PRY-037 fields; returns what was taken as `"group.field"`.
fn without_pry037(frame: &mut Value) -> Map<String, Value> {
    let mut taken = Map::new();
    for (group, fields) in [
        ("session", &["started"][..]),
        ("car", &["penalty"][..]),
        (
            "lap",
            &[
                "lap_type",
                "invalid_cause",
                "invalid_cause_certain",
                "invalid_at_m",
            ][..],
        ),
    ] {
        if let Some(g) = frame.get_mut(group).and_then(Value::as_object_mut) {
            for f in fields {
                if let Some(v) = g.remove(*f) {
                    taken.insert(format!("{group}.{f}"), v);
                }
            }
        }
    }
    taken
}

fn check_schema(frame: &Value) {
    if let Err(e) = schema_check::definition(&schema(), "live_frame", frame) {
        panic!("{e}\n{frame}");
    }
}

#[test]
fn live_frame_conversion_equals_the_fixture() {
    let fx = read_gz("live_frame_cases.json.gz");
    let (cases, frames) = (
        fx["cases"].as_array().unwrap(),
        fx["frames"].as_array().unwrap(),
    );
    assert_eq!((cases.len(), frames.len()), (307, 307));
    let schema = schema();
    let mut started = 0;
    for (i, (c, want)) in cases.iter().zip(frames).enumerate() {
        let d = build(c);
        let mut mine = live_frame(&d, &FrameOptions::default());
        schema_check::definition(&schema, "live_frame", &mine)
            .unwrap_or_else(|e| panic!("frame {i}: {e}"));
        let new = without_pry037(&mut mine);
        assert_eq!(&mine, want, "frame {i} differs");
        assert!(mine.get("labels").is_none());
        // PRY-037: no tracker view here, no pit schedule and no race state in the cases; only
        // the formation lap session state says "not started".
        if mine.get("session").is_some() {
            assert_eq!(new["car.penalty"], Value::Null, "{i}");
            let want_started = if d.session_state == session_state::FORMATION_LAP {
                Value::Bool(false)
            } else {
                Value::Null
            };
            assert_eq!(new["session.started"], want_started, "{i}");
            if new["session.started"] == Value::Bool(false) {
                started += 1;
            }
        }
        assert!(!new.keys().any(|k| k.starts_with("lap.")), "{i}");
    }
    assert!(started > 0); // some random cases are formation laps
}

#[test]
fn pit_mode_codes_in_the_frame() {
    let mut d = shm(session_state::PRACTICE);
    d.pit_mode = pit_mode::DRIVING_OUT_OF_GARAGE;
    assert_eq!(
        live_frame(&d, &FrameOptions::default())["car"]["pit_mode"],
        3
    );
    let opts = FrameOptions {
        in_garage: true,
        ..Default::default()
    };
    let parked = live_frame(&d, &opts); // in the car, in the garage, not moved
    assert_eq!(parked["car"]["pit_mode"], 4);
    let players: Vec<_> = parked["participants"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p["is_player"] == true)
        .map(|p| p["pit_mode"].clone())
        .collect();
    assert_eq!(players, [4]);
}

#[test]
fn frame_session_started() {
    use session_state::{FORMATION_LAP, PRACTICE, QUALIFY, RACE};
    // NOT_STARTED 1, RACING 2, FINISHED 3, INVALID 0
    for (session, race_state, started) in [
        (RACE, 1, Some(false)),
        (RACE, 2, Some(true)),
        (RACE, 3, Some(true)),
        (RACE, 0, None),
        (FORMATION_LAP, 2, Some(false)),
        (PRACTICE, 1, None),
        (QUALIFY, 2, None),
    ] {
        let mut d = shm(session);
        d.race_state = race_state;
        let f = live_frame(&d, &FrameOptions::default());
        assert_eq!(
            f["session"]["started"],
            started.map_or(Value::Null, Value::Bool)
        );
        check_schema(&f);
    }
}

#[test]
fn frame_penalty_from_the_pit_schedule() {
    // NONE 0, PLAYER_REQUESTED 1, ENGINEER 2, DAMAGE 3, MANDATORY 4, DRIVE_THROUGH 5, STOP_GO 6,
    // PITSPOT_OCCUPIED 7
    for (schedule, penalty) in [
        (0, None),
        (1, None),
        (2, None),
        (3, None),
        (4, None),
        (7, None),
        (5, Some("drive_through")),
        (6, Some("stop_go")),
    ] {
        let mut d = shm(session_state::PRACTICE);
        d.pit_schedule = schedule;
        let f = live_frame(&d, &FrameOptions::default());
        assert_eq!(
            f["car"]["penalty"],
            penalty.map_or(Value::Null, Value::from),
            "{schedule}"
        );
    }
}

#[test]
fn frame_lap_info_from_the_tracker() {
    let mut d = shm(session_state::PRACTICE);
    d.lap_invalidated = 1;
    let info = LiveLap {
        lap_type: "flying",
        invalid_cause: Some("track_limits"),
        invalid_cause_certain: false,
        invalid_at_m: Some(955.0),
    };
    let opts = FrameOptions {
        lap_info: Some(&info),
        ..Default::default()
    };
    let f = live_frame(&d, &opts);
    let lap = &f["lap"];
    assert_eq!(lap["valid"], false);
    assert_eq!(lap["lap_type"], "flying");
    assert_eq!(lap["invalid_cause"], "track_limits");
    assert_eq!(lap["invalid_cause_certain"], false);
    assert_eq!(lap["invalid_at_m"], 955.0);
    check_schema(&f);
    // A certain cause leaves `invalid_cause_certain` out; a valid lap has only its type.
    let certain = LiveLap {
        invalid_cause_certain: true,
        ..info
    };
    let opts = FrameOptions {
        lap_info: Some(&certain),
        ..Default::default()
    };
    assert!(
        live_frame(&d, &opts)["lap"]
            .get("invalid_cause_certain")
            .is_none()
    );
    let valid = LiveLap {
        lap_type: "out",
        invalid_cause: None,
        invalid_cause_certain: true,
        invalid_at_m: None,
    };
    let opts = FrameOptions {
        lap_info: Some(&valid),
        ..Default::default()
    };
    let lap = live_frame(&d, &opts)["lap"].clone();
    assert_eq!(lap["lap_type"], "out");
    assert!(lap.get("invalid_cause").is_none());
    // No player car: no lap group, the tracker's view is dropped with it.
    d.viewed_participant_index = 9;
    let opts = FrameOptions {
        lap_info: Some(&info),
        ..Default::default()
    };
    assert!(live_frame(&d, &opts).get("lap").is_none());
}

#[test]
fn menus_and_the_optional_header() {
    let mut d = shm(session_state::PRACTICE);
    d.game_state = game_state::FRONT_END;
    let opts = FrameOptions {
        taken_at: Some("2026-10-23T10:00:00.123+02:00"),
        data_age_s: Some(0.123_456),
        ..Default::default()
    };
    let f = live_frame(&d, &opts);
    assert_eq!(f["phase"], "menus");
    assert_eq!(f["data_age_s"], 0.123);
    assert_eq!(f.as_object().unwrap().len(), 4); // sim, phase, taken_at, data_age_s
    check_schema(&f);
}
