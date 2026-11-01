//! The scripted session through the real recorder, outbox and links into a fake Pitwall
//!: the writes equal the golden of the scripted session, keys and declarations are the fixed ones.

mod common;

use ams2_core::actions::{actions, actions_ik};
use ams2_core::announce::catalog::{catalog, catalog_ik};
use ams2_core::guidance::{items, items_ik};
use ams2_core::testing::scripted_session;
use ams2_platform::game::GameWatch;
use ams2_recorder::link::{LinkState, PitwallLink, peer_info};
use ams2_recorder::recorder::{Config, Overrides, Recorder};
use ams2_shm::testing::{TestMapping, unique_name};
use common::*;
use serde_json::{Value as Json, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use stintmate_ipc::codec;
use stintmate_ipc::keyfile::KeySource;

const GOLDEN: &str =
    include_str!("../../../crates/ams2-core/fixtures/scripted_session_golden.json");

struct Run {
    rec: Recorder,
    map: TestMapping,
    clock: Arc<AtomicU64>,
    last_seq: Option<u32>,
    _dir: tempfile::TempDir,
}

fn start(pipe: &str) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path(), &KEY);
    let shm = unique_name("b1");
    let map = TestMapping::create(&shm).unwrap();
    let clock = Arc::new(AtomicU64::new(0f64.to_bits()));
    let c = clock.clone();
    let cfg = Config {
        hz: 50.0,
        workspace: ws.clone(),
        data_dir: ws.join("connectors").join("ams2"),
        pitwall_pipe: Some(pipe.to_owned()),
        control_pipe: None,
        no_control: true,
        shm_name: Some(shm),
    };
    let ov = Overrides {
        game: Some(GameWatch::with(
            || true,
            Instant::now,
            Duration::from_secs(5),
        )),
        clock: Some((
            Arc::new(move || f64::from_bits(c.load(Ordering::Relaxed))),
            "ISO".into(),
        )),
        now: None,
        ..Overrides::default()
    };
    let mut rec = Recorder::build(cfg, ov).unwrap();
    rec.start_links();
    rec.connect();
    assert!(rec.mapped());
    Run {
        rec,
        map,
        clock,
        last_seq: None,
        _dir: dir,
    }
}

impl Run {
    fn play_all(&mut self) {
        for f in scripted_session() {
            if self.last_seq != Some(f.frame.sequence_number) {
                self.last_seq = Some(f.frame.sequence_number);
                self.map.write_frame(&f.frame);
            }
            self.clock.store(f.game_time.to_bits(), Ordering::Relaxed);
            self.rec.tick();
        }
    }
}

/// The received writes in the shape of the golden's calls.
fn received(calls: &[Call]) -> Vec<Json> {
    let mut out = Vec::new();
    let mut pending_catalog: Option<Json> = None;
    for c in calls {
        let Some(kind) = c.method.strip_prefix("pitwall.write.") else {
            continue;
        };
        let mut p = to_json(&c.params);
        let obj = p.as_object_mut().unwrap();
        obj.remove("session_key");
        obj.remove("seq");
        match kind {
            "catalog" => {
                pending_catalog = Some(p);
                continue;
            }
            "session_start" => {
                out.push(
                    json!({"call": "start_session", "catalog": pending_catalog.take(), "params": p}),
                );
            }
            "lap" => {
                let ch = codec::get(&c.params, "channels").unwrap();
                let summary: serde_json::Map<_, _> = ch
                    .as_map()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| {
                        let bin = codec::get(v, "data").unwrap().as_slice().unwrap();
                        assert_eq!(codec::get(v, "encoding").unwrap().as_str(), Some("f32"));
                        let col: Vec<f32> = bin
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|b| f32::from_le_bytes(*b))
                            .collect();
                        let sum: f64 = col.iter().map(|v| f64::from(*v)).sum();
                        (
                            k.as_str().unwrap().to_owned(),
                            json!({"n": col.len(), "first": col[0], "last": col[col.len() - 1], "sum": sum}),
                        )
                    })
                    .collect();
                p["channels"] = Json::Object(summary);
                out.push(json!({"call": "insert_lap", "params": p}));
            }
            "setup" => out.push(json!({"call": "insert_setup", "params": p})),
            "pit_exit" => out.push(json!({"call": "insert_pit_exit", "params": p})),
            "participant_laps" => {
                out.push(json!({"call": "insert_participant_laps", "params": p}));
            }
            "session_end" => out.push(json!({"call": "end_session", "params": p})),
            other => panic!("unexpected write {other}"),
        }
        pending_catalog = None;
    }
    out
}

fn golden() -> Vec<Json> {
    let all: Vec<Json> = serde_json::from_str(GOLDEN).unwrap();
    all.into_iter()
        .filter(|g| g["call"] != "event")
        .map(|mut g| {
            let o = g.as_object_mut().unwrap();
            o.remove("session_state");
            o.remove("n_rows");
            o.remove("snapshot");
            g
        })
        .collect()
}

fn same(a: &Json, b: &Json, path: &str) -> Result<(), String> {
    match (a, b) {
        (Json::Number(x), Json::Number(y)) => {
            let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
            if (x - y).abs() <= 1e-5 * x.abs().max(y.abs()).max(1.0) {
                Ok(())
            } else {
                Err(format!("{path}: {x} != {y}"))
            }
        }
        (Json::Array(x), Json::Array(y)) => {
            if x.len() != y.len() {
                return Err(format!("{path}: length {} != {}", x.len(), y.len()));
            }
            x.iter()
                .zip(y)
                .enumerate()
                .try_for_each(|(i, (p, q))| same(p, q, &format!("{path}[{i}]")))
        }
        (Json::Object(x), Json::Object(y)) => {
            let kx: std::collections::BTreeSet<_> = x.keys().collect();
            let ky: std::collections::BTreeSet<_> = y.keys().collect();
            if kx != ky {
                return Err(format!("{path}: keys {kx:?} != {ky:?}"));
            }
            x.iter()
                .try_for_each(|(k, v)| same(v, &y[k], &format!("{path}.{k}")))
        }
        _ if a == b => Ok(()),
        _ => Err(format!("{path}: {a} != {b}")),
    }
}

fn assert_golden(calls: &[Call]) {
    let (got, want) = (received(calls), golden());
    let name = |v: &Json| v["call"].as_str().unwrap().to_owned();
    assert_eq!(
        got.iter().map(name).collect::<Vec<_>>(),
        want.iter().map(name).collect::<Vec<_>>(),
        "the sequence of writes"
    );
    let errors: Vec<String> = got
        .iter()
        .zip(&want)
        .enumerate()
        .filter_map(|(i, (g, w))| same(g, w, &format!("#{i} {}", name(g))).err())
        .collect();
    assert!(
        errors.is_empty(),
        "{} differences, first: {:#?}",
        errors.len(),
        &errors[..errors.len().min(8)]
    );
}

fn check_keys(calls: &[Call]) {
    let key_of = |m: &str| -> Vec<(String, String)> {
        calls
            .iter()
            .filter(|c| c.method == m)
            .map(|c| {
                let sk = codec::get(&c.params, "session_key")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_owned();
                (sk, c.ik.clone().unwrap())
            })
            .collect()
    };
    let start = key_of("pitwall.write.session_start");
    let sk = &start[0].0;
    assert_eq!(sk.len(), 16);
    assert!(sk.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(start[0].1, format!("ams2/{sk}/start"));
    let laps = key_of("pitwall.write.lap");
    assert_eq!(laps.len(), 8);
    for (i, (_, ik)) in laps.iter().enumerate() {
        assert_eq!(ik, &format!("ams2/{sk}/lap/{}", i + 1));
    }
    assert_eq!(
        key_of("pitwall.write.session_end")[0].1,
        format!("ams2/{sk}/end")
    );
    for (_, ik) in key_of("pitwall.write.setup") {
        assert!(ik.starts_with(&format!("ams2/{sk}/setup/")));
    }
    for (_, ik) in key_of("pitwall.write.pit_exit") {
        assert!(ik.starts_with(&format!("ams2/{sk}/pit_exit/")));
    }
    for (_, ik) in key_of("pitwall.write.catalog") {
        assert_eq!(ik.strip_prefix("ams2/catalog/").unwrap().len(), 12);
    }
}

#[test]
fn scripted_session_with_pitwall_up_matches_the_golden() {
    let user = unique_user();
    let fake = FakePitwall::start(&pitwall_pipe(&user));
    let mut run = start(&fake.pipe);
    assert!(fake.wait_for(
        |c| c.iter().any(|x| x.method == "pitwall.actions.declare"),
        10
    ));
    run.play_all();
    run.rec.shutdown();
    let calls = fake.calls();
    assert_golden(&calls);
    check_keys(&calls);

    // Hello and the per-connection declarations, with their idempotency keys.
    let info = fake.peers.lock().unwrap()[0].clone();
    assert_eq!(info.app, "connector-ams2");
    assert_eq!(info.process.as_deref(), Some("recorder"));
    let meta = to_json(info.meta.as_ref().unwrap());
    assert_eq!(
        meta,
        json!({"connector_id": "ams2", "connector_name": "AMS2 connector", "sim": "ams2",
               "sim_name": "Automobilista 2", "service": "recorder"})
    );
    let ik = |m: &str| {
        calls
            .iter()
            .find(|c| c.method == m)
            .unwrap()
            .ik
            .clone()
            .unwrap()
    };
    assert_eq!(ik("pitwall.catalog.declare"), catalog_ik(catalog()));
    assert_eq!(ik("pitwall.guidance.declare"), items_ik(items()));
    assert_eq!(ik("pitwall.actions.declare"), actions_ik(actions()));
    assert!(ik("pitwall.catalog.declare").starts_with("ams2/textcatalog/"));
    assert!(ik("pitwall.guidance.declare").starts_with("ams2/guidance/"));
    assert!(ik("pitwall.actions.declare").starts_with("ams2/actions/"));
    let order: Vec<&str> = calls.iter().map(|c| c.method.as_str()).take(3).collect();
    assert_eq!(
        order,
        [
            "pitwall.catalog.declare",
            "pitwall.guidance.declare",
            "pitwall.actions.declare"
        ]
    );
    fake.stop();
}

#[test]
fn pitwall_opened_after_the_session_gets_the_outbox_in_order() {
    let user = unique_user();
    let pipe = pitwall_pipe(&user);
    let mut run = start(&pipe);
    run.play_all();
    assert!(run.rec.sink().queued(Duration::from_secs(10)));
    let summary = run.rec.sink().outbox_summary();
    assert!(summary.pending > 20, "{summary:?}");
    let files = std::fs::read_dir(run.rec.sink().outbox_dir())
        .unwrap()
        .count();
    assert_eq!(files, summary.pending);
    // Now Pitwall starts: the backlog goes oldest first.
    let fake = FakePitwall::start(&pipe);
    assert!(
        run.rec.sink().outbox().flush(Duration::from_secs(30)),
        "{:?}",
        run.rec.sink().outbox_summary()
    );
    run.rec.shutdown();
    let calls = fake.calls();
    assert_golden(&calls);
    check_keys(&calls);
    fake.stop();
}

#[test]
fn a_refused_rival_batch_is_split_row_by_row_and_rejected_is_kept() {
    let user = unique_user();
    let fake = FakePitwall::start(&pitwall_pipe(&user));
    fake.rules.lock().unwrap().refuse_next_batch = true;
    let mut run = start(&fake.pipe);
    assert!(fake.wait_for(
        |c| c.iter().any(|x| x.method == "pitwall.actions.declare"),
        10
    ));
    run.play_all();
    assert!(run.rec.sink().flush(Duration::from_secs(30)));
    let rejected_dir = run.rec.sink().outbox_dir().join("rejected");
    assert_eq!(std::fs::read_dir(&rejected_dir).unwrap().count(), 1);
    assert_eq!(run.rec.sink().outbox_summary().rejected, 1);
    let rows: usize = fake
        .calls()
        .iter()
        .filter(|c| c.method == "pitwall.write.participant_laps")
        .map(|c| {
            codec::get(&c.params, "laps")
                .unwrap()
                .as_array()
                .unwrap()
                .len()
        })
        .sum();
    let want: usize = golden()
        .iter()
        .filter(|g| g["call"] == "insert_participant_laps")
        .map(|g| g["params"]["laps"].as_array().unwrap().len())
        .sum();
    assert_eq!(
        rows, want,
        "every row delivered once, singly after the refusal"
    );
    run.rec.shutdown();
    fake.stop();
}

#[test]
fn a_wrong_key_is_refused_and_pitwall_not_running_is_waiting() {
    let user = unique_user();
    let pipe = pitwall_pipe(&user);
    let link = PitwallLink::new(&pipe, peer_info("recorder"), KeySource::Bytes([9u8; 32])).unwrap();
    link.start();
    let end = Instant::now() + Duration::from_secs(5);
    while link.state().1.as_deref() != Some("Pitwall is not running") && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        link.state(),
        (LinkState::Waiting, Some("Pitwall is not running".into()))
    );
    let fake = FakePitwall::start(&pipe);
    let end = Instant::now() + Duration::from_secs(15);
    while link.state().0 != LinkState::Refused && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
    let (state, msg) = link.state();
    assert_eq!(state, LinkState::Refused, "{msg:?}");
    assert!(msg.unwrap().contains("unauthorized"));
    link.close();
    fake.stop();
}
