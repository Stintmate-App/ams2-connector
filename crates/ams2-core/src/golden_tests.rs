//! Golden check: the scripted session through the Rust tracker equals the Python tracker's
//! output (`fixtures/scripted_session_golden.json`, written by running the Python
//! `SessionTracker` on `replay_session.session_script()` with a recording sink, the game clock
//! of the replay reader and `now_iso` fixed to `ISO`).
//!
//! Compared field by field: the session start (catalogue and params), setups (snapshot and
//! params), pit exits, every lap's params (channels summarised by length, first, last and sum),
//! the rival batches, the session end and the events. Numbers within a relative 1e-5.

use crate::messages::{lap_message, session_start_message};
use crate::names::Names;
use crate::setup::setup_message;
use crate::testing::{Call, RecordingSink, scripted_session};
use crate::tracker::SessionTracker;
use serde_json::{Value, json};

const GOLDEN: &str = include_str!("../fixtures/scripted_session_golden.json");

fn play() -> RecordingSink {
    let frames = scripted_session();
    let clock = crate::testing::FakeClock::new();
    let mut tracker = SessionTracker::new(Box::new(clock.clone()));
    let mut sink = RecordingSink::new();
    for f in &frames {
        clock.set(f.game_time);
        tracker.process(&f.frame, &mut sink);
    }
    tracker.end_session(&mut sink);
    sink
}

fn as_json(call: &Call) -> Value {
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

fn same(a: &Value, b: &Value, path: &str) -> Result<(), String> {
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

#[test]
fn scripted_session_matches_the_python_golden() {
    let golden: Vec<Value> = serde_json::from_str(GOLDEN).unwrap();
    let sink = play();
    let got: Vec<Value> = sink.calls.iter().map(as_json).collect();
    let name = |v: &Value| -> String {
        let c = v["call"].as_str().unwrap();
        if c == "event" {
            v["type"].as_str().unwrap().to_owned()
        } else {
            c.to_owned()
        }
    };
    assert_eq!(
        got.iter().map(name).collect::<Vec<_>>(),
        golden.iter().map(name).collect::<Vec<_>>(),
        "the sequence of calls"
    );
    let errors: Vec<String> = got
        .iter()
        .zip(&golden)
        .enumerate()
        .filter_map(|(i, (g, p))| same(g, p, &format!("#{i} {}", name(g))).err())
        .collect();
    assert!(
        errors.is_empty(),
        "{} differences, first: {:#?}",
        errors.len(),
        &errors[..errors.len().min(8)]
    );
}

#[test]
fn scripted_session_shape() {
    // The shape described in replay_session.py's docstring: the session opens in the garage
    // (setup first), 8 laps are stored, two pit exits... (one at the garage exit, one at the
    // pit exit), and the session ends once.
    let sink = play();
    let laps = sink.laps();
    assert_eq!(laps.len(), 8);
    assert_eq!(sink.stored_sessions(), 1);
    let kinds: Vec<&str> = laps.iter().map(|l| l.kind).collect();
    assert_eq!(
        kinds,
        [
            "out", "flying", "flying", "flying", "in", "out", "flying", "in"
        ]
    );
    assert_eq!(laps.iter().filter(|l| !l.record.valid).count(), 1); // lap 4: off track, flagged at 2000 m
    let k = sink.kinds();
    assert_eq!(k[..3], ["catalog", "session_start", "setup"]);
    assert_eq!(k.iter().filter(|x| **x == "pit_exit").count(), 2);
    assert_eq!(*k.last().unwrap(), "session_end");
}
