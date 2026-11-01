//! The golden form of the tracker's sink calls, shared by the golden test and `tools/golden`:
//! [`call_json`] turns one [`Call`] into the JSON compared against the golden file, and
//! [`same`] compares two such values (numbers within a relative 1e-5, everything else exact).

use crate::messages::{lap_message, session_start_message};
use crate::names::Names;
use crate::setup::setup_message;
use crate::testing::Call;
use serde_json::{Value, json};

/// One sink call in the golden form (channels summarised as `{n, first, last, sum}`, wall-clock
/// times as `ISO`).
pub fn call_json(call: &Call) -> Value {
    match call {
        Call::Start(info) => {
            let (cat, start) = session_start_message(info, Names::embedded());
            json!({"call": "start_session", "catalog": cat, "params": start, "session_state": info.session_state})
        }
        Call::End { ended_at, .. } => {
            json!({"call": "end_session", "params": {"ended_at": ended_at}})
        }
        Call::Lap { record, rows } => {
            let mut msg = lap_message(record, rows);
            let channels = msg.channels.take().map(|c| {
                c.0.iter()
                    .map(|(name, col)| {
                        let sum: f64 = col.iter().map(|v| f64::from(*v)).sum();
                        (
                            (*name).to_owned(),
                            json!({"n": col.len(), "first": col[0], "last": col[col.len() - 1], "sum": sum}),
                        )
                    })
                    .collect::<serde_json::Map<_, _>>()
            });
            let mut params = serde_json::to_value(&msg).unwrap();
            if let Some(c) = channels {
                params["channels"] = Value::Object(c);
            }
            json!({"call": "insert_lap", "n_rows": rows.len(), "params": params})
        }
        Call::Setup {
            reason, snapshot, ..
        } => {
            json!({"call": "insert_setup", "snapshot": snapshot, "params": setup_message(snapshot, "ISO", reason)})
        }
        Call::PitExit { fuel_l, .. } => {
            let mut p = json!({"at": "ISO"});
            if let Some(f) = fuel_l {
                p["fuel_l"] = json!(f);
            }
            json!({"call": "insert_pit_exit", "params": p})
        }
        Call::Participants { rows, .. } => {
            json!({"call": "insert_participant_laps", "params": {"laps": rows}})
        }
        Call::Event(e) => {
            let mut v = serde_json::to_value(e).unwrap();
            let t = v.as_object_mut().unwrap().remove("type").unwrap();
            json!({"call": "event", "type": t, "fields": v})
        }
    }
}

/// `Ok` when `a` and `b` are equal (numbers within a relative 1e-5), else where they differ.
pub fn same(a: &Value, b: &Value, path: &str) -> Result<(), String> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            if (x - y).abs() <= 1e-5 * x.abs().max(y.abs()).max(1.0) {
                Ok(())
            } else {
                Err(format!("{path}: {x} != {y}"))
            }
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Err(format!("{path}: length {} != {}", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .try_for_each(|(i, (p, q))| same(p, q, &format!("{path}[{i}]")))
        }
        (Value::Object(x), Value::Object(y)) => {
            let kx: std::collections::BTreeSet<_> = x.keys().collect();
            let ky: std::collections::BTreeSet<_> = y.keys().collect();
            if kx != ky {
                return Err(format!(
                    "{path}: keys {:?} != {:?}",
                    kx.symmetric_difference(&ky),
                    "(rust vs golden)"
                ));
            }
            x.iter()
                .try_for_each(|(k, v)| same(v, &y[k], &format!("{path}.{k}")))
        }
        _ if a == b => Ok(()),
        _ => Err(format!("{path}: {a} != {b}")),
    }
}
