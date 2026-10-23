//! The write API messages the connector sends (Pitwall's `write-api.md`), as typed structs.
//!
//! Each message serializes to the **params** of its IPC write method (`pitwall.write.<type>`):
//! the message without `type` and without `session_id` (the connector names its session by its
//! own `session_key`, which the sink adds), `None` fields left out. A lap's channels are
//! `{"encoding": "f32", "data": <bin: little-endian float32>}`; serializing them uses
//! `serialize_bytes`, so a msgpack serializer writes `bin` (the JSON form is only a debugging
//! aid). Encoding to the wire is the IPC layer's job (task B1).
//!
//! Also here: the records the tracker hands to its sink ([`LapRecord`], [`SessionInfo`]) and the
//! functions that map them to messages ([`lap_message`], [`session_start_message`]).

use crate::channels::{LapColumns, Row, lap_channels};
use crate::names::{self, Names};
use crate::vocab;
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Little-endian float32 samples as raw bytes (`bin` on the IPC wire).
pub fn f32_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 4);
    for v in values {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Bytes serialized as a byte string (`bin` in msgpack).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bin(pub Vec<u8>);

impl Serialize for Bin {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.0)
    }
}

#[derive(Serialize)]
struct EncodedChannel {
    encoding: &'static str,
    data: Bin,
}

/// A lap's channels in catalogue order, serialized as `{name: {encoding: "f32", data: bin}}`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LapChannels(pub LapColumns);

impl Serialize for LapChannels {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (name, col) in &self.0 {
            map.serialize_entry(
                name,
                &EncodedChannel {
                    encoding: "f32",
                    data: Bin(f32_bytes(col)),
                },
            )?;
        }
        map.end()
    }
}

/// `Some(v)` only for finite numbers (the wire never carries NaN or infinity).
pub fn finite(v: Option<f64>) -> Option<f64> {
    v.filter(|x| x.is_finite())
}

/// A car class of the catalogue.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CarClass {
    /// Class id.
    pub class_id: String,
    /// Display name.
    pub name: String,
}

/// A car of the catalogue.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Car {
    /// Car id.
    pub car_id: String,
    /// Display name.
    pub name: String,
    /// Class id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_id: Option<String>,
    /// Tank capacity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_capacity_l: Option<f64>,
}

/// A track of the catalogue.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Track {
    /// Track id.
    pub track_id: String,
    /// Display name.
    pub name: String,
}

/// A layout of the catalogue.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Layout {
    /// Track id.
    pub track_id: String,
    /// Layout id.
    pub layout_id: String,
    /// Display name.
    pub name: String,
    /// Length.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length_m: Option<f64>,
}

/// `pitwall.write.catalog`.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Catalog {
    /// Classes.
    pub classes: Vec<CarClass>,
    /// Cars.
    pub cars: Vec<Car>,
    /// Tracks.
    pub tracks: Vec<Track>,
    /// Layouts.
    pub layouts: Vec<Layout>,
}

/// Weather at a session start or the end of a lap.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Conditions {
    /// Ambient temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambient_c: Option<f64>,
    /// Track temperature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_c: Option<f64>,
    /// Rain density.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rain: Option<f64>,
    /// Wind speed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wind_speed_ms: Option<f64>,
    /// Cloud cover (AMS2 gives brightness, not a cover: never set).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cloud: Option<f64>,
}

/// `Some` unless every value is missing.
pub fn conditions(
    ambient: Option<f64>,
    track: Option<f64>,
    rain: Option<f64>,
    wind: Option<f64>,
) -> Option<Conditions> {
    let c = Conditions {
        ambient_c: finite(ambient),
        track_c: finite(track),
        rain: finite(rain),
        wind_speed_ms: finite(wind),
        cloud: None,
    };
    (c.ambient_c.is_some() || c.track_c.is_some() || c.rain.is_some() || c.wind_speed_ms.is_some())
        .then_some(c)
}

/// `pitwall.write.session_start`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionStart {
    /// ISO timestamp.
    pub started_at: String,
    /// `practice`, `qualifying`, `race` or `other`.
    pub session_type: String,
    /// Car id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub car_id: Option<String>,
    /// Track id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track_id: Option<String>,
    /// Layout id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub layout_id: Option<String>,
    /// Outside reference.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_ref: Option<String>,
    /// Weather.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Conditions>,
}

/// One sector of a lap.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Sector {
    /// Time (absent when unknown).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_s: Option<f64>,
}

/// `pitwall.write.lap`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Lap {
    /// `formation`, `out`, `in` or `flying`.
    pub lap_type: String,
    /// The car was on the track (outside the pit lane) during the lap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_track: Option<bool>,
    /// Lap time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_s: Option<f64>,
    /// Valid for the game.
    pub valid: bool,
    /// `official` or `measured`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_source: Option<String>,
    /// The lap clock at the last sample.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_time_s: Option<f64>,
    /// ISO timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    /// The game's lap counter.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sim_lap: Option<u32>,
    /// Fuel at the start.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_start_l: Option<f64>,
    /// Fuel used.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_used_l: Option<f64>,
    /// Top speed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_speed_kmh: Option<f64>,
    /// Weather at the end of the lap.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conditions: Option<Conditions>,
    /// The three sectors.
    pub sectors: Vec<Sector>,
    /// The samples.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<LapChannels>,
}

/// An extra setup value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ExtraEntry {
    /// Label.
    pub label: String,
    /// Value.
    pub value: ExtraValue,
    /// Group.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    /// Unit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// The value of an [`ExtraEntry`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged)]
pub enum ExtraValue {
    /// Text.
    Text(String),
    /// Number.
    Number(f64),
    /// Flag.
    Bool(bool),
}

/// `pitwall.write.setup` (source `telemetry`).
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct Setup {
    /// ISO timestamp.
    pub taken_at: String,
    /// `session_start` or `pit_exit`.
    pub reason: String,
    /// Always `telemetry`.
    pub source: String,
    /// Brake bias, % front.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub brake_bias_front_pct: Option<f64>,
    /// Front compound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compound_front: Option<String>,
    /// Rear compound.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compound_rear: Option<String>,
    /// Cold pressure FL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure_fl_kpa: Option<f64>,
    /// Cold pressure FR.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure_fr_kpa: Option<f64>,
    /// Cold pressure RL.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure_rl_kpa: Option<f64>,
    /// Cold pressure RR.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure_rr_kpa: Option<f64>,
    /// Fuel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_l: Option<f64>,
    /// Front wing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wing_front: Option<f64>,
    /// Rear wing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wing_rear: Option<f64>,
    /// ABS setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub abs: Option<f64>,
    /// TC setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tc: Option<f64>,
    /// Extra entries.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, ExtraEntry>,
}

/// One row of `participant_laps`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParticipantLap {
    /// Participant index.
    pub participant: u32,
    /// Lap (the game's count).
    pub lap: u32,
    /// Driver name.
    pub name: String,
    /// Car id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub car_id: Option<String>,
    /// Class id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class_id: Option<String>,
    /// The viewed car.
    pub is_player: bool,
    /// Lap time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_s: Option<f64>,
    /// `official` or `sectors`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_source: Option<String>,
    /// Sector times (S1, S2, S3) or empty.
    pub sectors: Vec<Option<f64>>,
    /// Not invalidated.
    pub valid: bool,
    /// A pit mode was seen during the lap.
    pub pit: bool,
    /// Pit stops so far.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pit_stops: Option<u32>,
    /// The lap started on a line crossing.
    pub from_line: bool,
    /// Race position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<u32>,
    /// Seconds since the session began.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub crossed_at_s: Option<f64>,
    /// The game's best lap of this car.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sim_best_lap_s: Option<f64>,
    /// Race state label.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

/// `pitwall.write.pit_exit`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PitExit {
    /// ISO timestamp.
    pub at: String,
    /// Fuel at the exit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fuel_l: Option<f64>,
}

/// `pitwall.write.participant_laps`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ParticipantLaps {
    /// The rows.
    pub laps: Vec<ParticipantLap>,
}

/// `pitwall.write.session_end`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SessionEnd {
    /// ISO timestamp.
    pub ended_at: String,
}

/// One finished lap as the tracker saw it (AMS2 semantics; [`lap_message`] maps it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LapRecord {
    /// The sink's session id.
    pub session_id: u64,
    /// The number the Writer gives it (laps are numbered 1..n as stored).
    pub lap_number: u32,
    /// ISO timestamp.
    pub started_at: String,
    /// Lap time.
    pub lap_time_s: Option<f64>,
    /// Lap clock at the last sample.
    pub measured_time_s: Option<f64>,
    /// Sector 1.
    pub sector1_s: Option<f64>,
    /// Sector 2.
    pub sector2_s: Option<f64>,
    /// Sector 3.
    pub sector3_s: Option<f64>,
    /// The lap reached the line.
    pub complete: bool,
    /// Valid for the game.
    pub valid: bool,
    /// Out lap.
    pub out_lap: bool,
    /// In lap.
    pub in_lap: bool,
    /// Fuel at the first sample.
    pub fuel_start_l: Option<f64>,
    /// Fuel at the last sample on track.
    pub fuel_end_l: Option<f64>,
    /// Top speed.
    pub max_speed_kmh: Option<f64>,
    /// Number of samples.
    pub n_samples: usize,
    /// The car was on the track during the lap.
    pub on_track: bool,
    /// The game's lap counter.
    pub game_lap: Option<u32>,
    /// `official` or `measured`.
    pub lap_time_source: Option<String>,
    /// A formation lap.
    pub formation_lap: bool,
    /// Weather at the last sample.
    pub ambient_c: Option<f64>,
    /// Track temperature.
    pub track_c: Option<f64>,
    /// Rain.
    pub rain: Option<f64>,
    /// Wind.
    pub wind_speed: Option<f64>,
}

/// What the tracker knows when a session starts (AMS2's own strings and values).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionInfo {
    /// ISO timestamp.
    pub started_at: String,
    /// `mCarName`.
    pub car: String,
    /// `mCarClassName`.
    pub car_class: String,
    /// `mTrackLocation`.
    pub track: String,
    /// `mTrackVariation`.
    pub variation: String,
    /// AMS2 session state name, e.g. `PRACTICE` ([`vocab::session_type_of_name`] maps it).
    pub session_state: String,
    /// Track length.
    pub length_m: Option<f64>,
    /// Tank capacity.
    pub fuel_capacity_l: Option<f64>,
    /// Ambient temperature.
    pub ambient_c: Option<f64>,
    /// Track temperature.
    pub track_c: Option<f64>,
    /// Rain density.
    pub rain: Option<f64>,
    /// Wind speed.
    pub wind_speed: Option<f64>,
}

fn positive(v: Option<f64>) -> Option<f64> {
    v.filter(|x| *x > 0.0)
}

/// The old flags' precedence (formation > out > in > flying), as the importer maps them.
pub fn lap_type(r: &LapRecord) -> &'static str {
    if r.formation_lap {
        "formation"
    } else if r.out_lap {
        "out"
    } else if r.in_lap {
        "in"
    } else {
        "flying"
    }
}

/// A lap record and its sample rows to the `lap` message.
pub fn lap_message(r: &LapRecord, rows: &[Row]) -> Lap {
    let timed = r.lap_time_s.is_some_and(|t| t > 0.0);
    let used = match (r.fuel_start_l, r.fuel_end_l) {
        (Some(a), Some(b)) => Some(a - b),
        _ => None,
    };
    let channels = lap_channels(rows);
    Lap {
        lap_type: lap_type(r).to_owned(),
        on_track: Some(r.on_track),
        time_s: finite(r.lap_time_s.filter(|_| timed)),
        time_source: timed.then(|| {
            r.lap_time_source
                .clone()
                .unwrap_or_else(|| "official".into())
        }),
        measured_time_s: finite(positive(r.measured_time_s)),
        valid: r.valid,
        started_at: Some(r.started_at.clone()),
        sim_lap: r.game_lap,
        fuel_start_l: finite(r.fuel_start_l.filter(|v| *v >= 0.0)),
        fuel_used_l: finite(used),
        max_speed_kmh: finite(r.max_speed_kmh.filter(|v| *v >= 0.0)),
        conditions: conditions(r.ambient_c, r.track_c, r.rain, r.wind_speed),
        sectors: [r.sector1_s, r.sector2_s, r.sector3_s]
            .into_iter()
            .map(|s| Sector {
                time_s: finite(positive(s)),
            })
            .collect(),
        channels: (!channels.is_empty()).then_some(LapChannels(channels)),
    }
}

/// The catalogue and the `session_start` message of a session (what the sink queues when a
/// session starts); the catalogue is `None` when the session names no car and no layout.
pub fn session_start_message(info: &SessionInfo, names: &Names) -> (Option<Catalog>, SessionStart) {
    let trim = |s: &str| {
        let t = s.trim();
        (!t.is_empty()).then(|| t.to_owned())
    };
    let car = trim(&info.car);
    let (mut track, mut layout) = (trim(&info.track), trim(&info.variation));
    if track.is_none() || layout.is_none() {
        track = None;
        layout = None;
    }
    let class = (!info.car_class.is_empty()).then(|| info.car_class.clone());
    let mut sessions = Vec::new();
    if let (Some(c), Some(t), Some(l)) = (&car, &track, &layout) {
        sessions.push(names::session_ids(
            c,
            class.as_deref(),
            t,
            l,
            info.length_m,
            info.fuel_capacity_l,
        ));
    }
    let mut cat = names::catalog(&sessions, &[], names);
    if let (Some(c), None) = (&car, &track) {
        cat = names::catalog(&[], &[(c.clone(), class.clone())], names);
        if let Some(cap) = positive(info.fuel_capacity_l) {
            cat.cars[0].fuel_capacity_l = Some(cap);
        }
    }
    let start = SessionStart {
        started_at: info.started_at.clone(),
        session_type: vocab::session_type_of_name(&info.session_state).to_owned(),
        car_id: car,
        track_id: track,
        layout_id: layout,
        external_ref: None,
        conditions: conditions(info.ambient_c, info.track_c, info.rain, info.wind_speed),
    };
    (
        (!cat.cars.is_empty() || !cat.layouts.is_empty()).then_some(cat),
        start,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_bytes_are_little_endian() {
        assert_eq!(
            f32_bytes(&[1.0, -2.5]),
            [0, 0, 0x80, 0x3f, 0, 0, 0x20, 0xc0]
        );
        assert!(f32_bytes(&[]).is_empty());
    }

    #[test]
    fn params_leave_out_missing_fields() {
        let m = PitExit {
            at: "T".into(),
            fuel_l: None,
        };
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!({"at": "T"})
        );
        let s = Setup {
            source: "telemetry".into(),
            ..Setup::default()
        };
        let v = serde_json::to_value(&s).unwrap();
        assert!(v.get("extra").is_none() && v.get("abs").is_none());
        assert_eq!(conditions(None, None, None, None), None);
        assert_eq!(finite(Some(f64::NAN)), None);
    }

    #[test]
    fn lap_channels_serialize_as_encoded_f32() {
        let ch = LapChannels(vec![("speed", vec![100.0])]);
        let v = serde_json::to_value(&ch).unwrap();
        assert_eq!(v["speed"]["encoding"], "f32");
        assert_eq!(v["speed"]["data"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn lap_without_time_has_no_source() {
        let r = LapRecord {
            session_id: 1,
            lap_number: 1,
            started_at: "T".into(),
            lap_time_s: None,
            measured_time_s: Some(0.0),
            sector1_s: None,
            sector2_s: Some(-1.0),
            sector3_s: None,
            complete: false,
            valid: true,
            out_lap: false,
            in_lap: true,
            fuel_start_l: Some(20.0),
            fuel_end_l: Some(17.0),
            max_speed_kmh: Some(100.0),
            n_samples: 0,
            on_track: true,
            game_lap: Some(2),
            lap_time_source: None,
            formation_lap: false,
            ambient_c: None,
            track_c: None,
            rain: None,
            wind_speed: None,
        };
        let m = lap_message(&r, &[]);
        assert_eq!(
            (m.lap_type.as_str(), m.time_source, m.time_s),
            ("in", None, None)
        );
        assert_eq!(m.measured_time_s, None);
        assert_eq!(m.fuel_used_l, Some(3.0));
        assert_eq!(m.sectors, vec![Sector::default(); 3]);
        assert!(m.channels.is_none());
    }
}
