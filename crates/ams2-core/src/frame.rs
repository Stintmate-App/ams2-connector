//! AMS2 shared memory snapshot to the neutral live frame (PRY-036, PRY-037).
//!
//! The `connector.live_frame` answer of the recorder (application contract 5.3: the live frame
//! of Pitwall's connector protocol, schema `$defs/live_frame`). Built only when Pitwall asks,
//! from the recorder's last snapshot, on the connection's reader thread (never on the sampling
//! thread). Freshness (a frozen mapping) and the game's process are the caller's checks.
//!
//! Mapping:
//!
//! * game state and pit mode to `phase` ([`crate::garage::phase`]);
//! * session state to `session.type` (practice, test, time attack = `practice`; qualify =
//!   `qualifying`; race and formation lap = `race`; anything else = `other`);
//! * pit mode to Pitwall's pit codes (0 on track, 1 into the pits, 2 in the box, 3 driving out
//!   of the pits or the garage, 4 in the garage; an undefined value = 2); the player parked in
//!   the car in the garage is 4 (`in_garage`);
//! * units: speed km/h (the game gives m/s), fuel litres (a fraction of the tank), carcass
//!   temperature C (K), times s, the session duration s (minutes); tread temperatures as inner /
//!   middle / outer (on the left wheels the game's "left" is the outer edge);
//! * "not given" is `null`: lap times <= 0, a lap clock < 0, a time remaining <= 0, a race
//!   position 0, a sector < 0, a lap count 0, a session duration 0, an empty compound;
//! * the safety car is flagged (`is_safety_car`): named "Safety Car" or of class "SafetyCar".
//!
//! Why the lap is not timed or not valid (PRY-037): `session.started` (races only),
//! `car.penalty` (drive-through and stop-and-go from the pit schedule) and the tracker's view
//! of the open lap ([`LiveLap`]: `lap_type`, `invalid_cause`, `invalid_cause_certain`,
//! `invalid_at_m`).

use crate::codes::session_state;
use crate::garage;
use crate::round_to;
use crate::tracker::LiveLap;
use crate::{SIM, decode_str};
use ams2_shm::SharedMemory;
use serde_json::{Map, Value, json};

const KELVIN: f64 = 273.15;
/// Pitwall's code for the garage.
const PIT_IN_GARAGE: i64 = 4;
/// An undefined pit mode.
const PIT_UNKNOWN: i64 = 2;
/// Wheel names in the game's order.
pub const WHEELS: [&str; 4] = ["fl", "fr", "rl", "rr"];
const SCHEDULE_DRIVE_THROUGH: u32 = 5;
const SCHEDULE_STOP_GO: u32 = 6;
const RACE_NOT_STARTED: u32 = 1;

/// What the caller adds to a snapshot.
#[derive(Debug, Default, Clone)]
pub struct FrameOptions<'a> {
    /// `taken_at`: ISO 8601 with offset.
    pub taken_at: Option<&'a str>,
    /// Seconds since the game last changed the data.
    pub data_age_s: Option<f64>,
    /// The tracker's view of the open lap.
    pub lap_info: Option<&'a LiveLap>,
    /// The tracker says the player's car is in the garage (also while the game says "driving
    /// out of the garage" before the car moved): its pit mode is 4.
    pub in_garage: bool,
}

/// `session.started`: `false` before a race start, `true` once started, `None` when not known
/// (not a race, or the race state not given).
pub fn race_started(session: u32, race: u32) -> Option<bool> {
    if session == session_state::FORMATION_LAP {
        return Some(false);
    }
    if session != session_state::RACE {
        return None;
    }
    match race {
        RACE_NOT_STARTED => Some(false),
        r if r > RACE_NOT_STARTED => Some(true),
        _ => None,
    }
}

/// `session.type` of a session state.
pub fn session_type(state: u32) -> &'static str {
    match state {
        session_state::PRACTICE | session_state::TEST | session_state::TIME_ATTACK => "practice",
        session_state::QUALIFY => "qualifying",
        session_state::FORMATION_LAP | session_state::RACE => "race",
        _ => "other",
    }
}

/// Pitwall's pit code of the game's pit mode (driving out of the garage = driving out).
pub fn pit_code(v: u32) -> i64 {
    match v {
        0 => 0,
        1 => 1,
        2 => 2,
        3 | 5 => 3,
        4 => 4,
        _ => PIT_UNKNOWN,
    }
}

/// The safety car: named "Safety Car" or of class "SafetyCar".
pub fn is_safety_car(name: &str, car_class: &str) -> bool {
    name.trim().to_lowercase() == "safety car"
        || car_class.replace(' ', "").to_lowercase() == "safetycar"
}

fn pos(v: f32) -> Value {
    if v > 0.0 {
        json!(f64::from(v))
    } else {
        Value::Null
    }
}

fn text(raw: &[u8]) -> Value {
    let s = decode_str(raw);
    if s.is_empty() { Value::Null } else { json!(s) }
}

fn sector(v: i32) -> Value {
    if v >= 0 { json!(v + 1) } else { Value::Null }
}

/// The neutral live frame of one consistent snapshot.
pub fn live_frame(d: &SharedMemory, opts: &FrameOptions<'_>) -> Value {
    let phase = garage::phase(d.game_state, d.pit_mode, opts.in_garage);
    let mut frame = Map::new();
    frame.insert("sim".into(), json!(SIM));
    frame.insert("phase".into(), json!(phase));
    if let Some(t) = opts.taken_at {
        frame.insert("taken_at".into(), json!(t));
    }
    if let Some(age) = opts.data_age_s {
        frame.insert("data_age_s".into(), json!(round_to(age, 3)));
    }
    if phase == "menus" {
        return Value::Object(frame);
    }

    let n = usize::try_from(d.num_participants)
        .unwrap_or(0)
        .min(d.participant_info.len());
    let me = d.viewed_participant_index;
    let me_idx = usize::try_from(me).ok().filter(|&i| i < n);

    frame.insert(
        "session".into(),
        json!({
            "type": session_type(d.session_state),
            "time_remaining_s": pos(d.event_time_remaining),
            "laps_in_event": if d.laps_in_event != 0 { json!(d.laps_in_event) } else { Value::Null },
            "duration_s": if d.session_duration > 0.0 {
                json!(f64::from(d.session_duration) * 60.0)
            } else { Value::Null },
            "additional_laps": if d.session_additional_laps >= 0 {
                json!(d.session_additional_laps)
            } else { Value::Null },
            "started": race_started(d.session_state, d.race_state),
        }),
    );
    frame.insert(
        "track".into(),
        json!({
            "track_id": decode_str(&d.track_location),
            "layout_id": decode_str(&d.track_variation),
            "length_m": if d.track_length > 0.0 { json!(f64::from(d.track_length)) } else { Value::Null },
        }),
    );

    let mut wheels = Map::new();
    for (k, w) in WHEELS.iter().enumerate() {
        let (left, right) = (
            f64::from(d.tyre_temp_left[k]),
            f64::from(d.tyre_temp_right[k]),
        );
        let (inner, outer) = if k == 0 || k == 2 {
            (right, left)
        } else {
            (left, right)
        };
        wheels.insert(
            (*w).into(),
            json!({
                "tread_in_c": inner,
                "tread_mid_c": f64::from(d.tyre_temp_center[k]),
                "tread_out_c": outer,
                "pressure_kpa": f64::from(d.air_pressure[k]),
                "carcass_c": f64::from(d.tyre_carcass_temp[k]) - KELVIN,
                "wear": f64::from(d.tyre_wear[k]),
                "compound": text(&d.tyre_compound[k]),
                "brake_c": f64::from(d.brake_temp_celsius[k]),
            }),
        );
    }
    let penalty = match d.pit_schedule {
        SCHEDULE_DRIVE_THROUGH => json!("drive_through"),
        SCHEDULE_STOP_GO => json!("stop_go"),
        _ => Value::Null,
    };
    frame.insert(
        "car".into(),
        json!({
            "car_id": decode_str(&d.car_name),
            "class_id": text(&d.car_class_name),
            "pit_mode": if opts.in_garage { PIT_IN_GARAGE } else { pit_code(d.pit_mode) },
            "speed_kmh": f64::from(d.speed) * 3.6,
            "gear": d.gear,
            "rpm": f64::from(d.rpm),
            "fuel_l": f64::from(d.fuel_level) * f64::from(d.fuel_capacity),
            "fuel_capacity_l": if d.fuel_capacity > 0.0 { json!(f64::from(d.fuel_capacity)) } else { Value::Null },
            "penalty": penalty,
            "wheels": Value::Object(wheels),
        }),
    );
    if let Some(i) = me_idx {
        let p = &d.participant_info[i];
        let mut lap = Map::new();
        lap.insert("current_lap".into(), json!(p.current_lap));
        lap.insert("laps_completed".into(), json!(p.laps_completed));
        lap.insert(
            "distance_m".into(),
            json!(f64::from(p.current_lap_distance)),
        );
        lap.insert(
            "time_s".into(),
            if d.current_time >= 0.0 {
                json!(f64::from(d.current_time))
            } else {
                Value::Null
            },
        );
        lap.insert("valid".into(), json!(d.lap_invalidated == 0));
        lap.insert("sector".into(), sector(p.current_sector));
        lap.insert("last_lap_s".into(), pos(d.last_lap_time));
        lap.insert("best_lap_s".into(), pos(d.best_lap_time));
        if let Some(info) = opts.lap_info {
            lap.insert("lap_type".into(), json!(info.lap_type));
            // The tracker's dictionary has the cause keys only once the game flagged the lap
            // (and `invalid_cause_certain` only when the cause is probable).
            if info.invalid_cause.is_some() || info.invalid_at_m.is_some() {
                lap.insert("invalid_cause".into(), json!(info.invalid_cause));
                if !info.invalid_cause_certain {
                    lap.insert("invalid_cause_certain".into(), json!(false));
                }
                lap.insert("invalid_at_m".into(), json!(info.invalid_at_m));
            }
        }
        frame.insert("lap".into(), Value::Object(lap));
    }
    frame.insert(
        "splits".into(),
        json!({"ahead_s": pos(d.split_time_ahead), "behind_s": pos(d.split_time_behind)}),
    );
    frame.insert(
        "conditions".into(),
        json!({
            "ambient_c": f64::from(d.ambient_temperature),
            "track_c": f64::from(d.track_temperature),
            "rain": f64::from(d.rain_density).clamp(0.0, 1.0),
            "wind_speed_ms": f64::from(d.wind_speed).max(0.0),
        }),
    );
    let mut participants = Vec::with_capacity(n);
    for i in 0..n {
        let q = &d.participant_info[i];
        let name = decode_str(&q.name);
        let car_class = decode_str(&d.car_class_names[i]);
        let pit = if opts.in_garage && Some(i) == usize::try_from(me).ok() {
            PIT_IN_GARAGE
        } else {
            pit_code(d.pit_modes[i])
        };
        participants.push(json!({
            "index": i,
            "name": &*name,
            "is_player": Some(i) == usize::try_from(me).ok(),
            "is_safety_car": is_safety_car(&name, &car_class),
            "active": q.is_active != 0,
            "position": if q.race_position != 0 { json!(q.race_position) } else { Value::Null },
            "laps_completed": q.laps_completed,
            "current_lap": q.current_lap,
            "distance_m": f64::from(q.current_lap_distance),
            "sector": sector(q.current_sector),
            "pit_mode": pit,
            "car_id": text(&d.car_names[i]),
            "class_id": if car_class.is_empty() { Value::Null } else { json!(&*car_class) },
            "last_lap_s": pos(d.last_lap_times[i]),
            "best_lap_s": pos(d.fastest_lap_times[i]),
        }));
    }
    frame.insert("participants".into(), Value::Array(participants));
    Value::Object(frame)
}
