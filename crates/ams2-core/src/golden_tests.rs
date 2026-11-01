//! Golden check: the scripted session through the Rust tracker equals the recorded
//! output (`fixtures/scripted_session_golden.json`, made once by an earlier implementation of the
//! tracker that is not in this repository, run on the same scripted session with a recording sink,
//! the game clock of the replay reader and `now_iso` fixed to `ISO`).
//!
//! Compared field by field: the session start (catalogue and params), setups (snapshot and
//! params), pit exits, every lap's params (channels summarised by length, first, last and sum),
//! the rival batches, the session end and the events. Numbers within a relative 1e-5.

use crate::golden::{call_json as as_json, same};
use crate::testing::{RecordingSink, scripted_session};
use crate::tracker::SessionTracker;
use serde_json::Value;

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

#[test]
fn scripted_session_matches_the_golden() {
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
    // The shape of the scripted session in `testing.rs`: the session opens in the garage
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
