//! B2 through real links into a fake Pitwall: the telemetry stream (ports of `test_telemetry.py`),
//! `connector.live_frame` (`test_frame.py`, server parts), `connector.action` (`test_actions.py`)
//! and the test hook (`test_connector_messages.py`). Random pipes, a temp workspace and key,
//! test mappings only: nothing of the game or the driver's apps is touched.

mod common;

use ams2_core::telemetry::{FRESH, PLAYER_VIEW, Sample};
use ams2_core::testing::Sim;
use ams2_platform::timer::WaitableTimer;
use ams2_recorder::cli::{self, AnnounceArgs};
use ams2_recorder::link::{PitwallLink, peer_info, to_value};
use ams2_recorder::live::Hints;
use ams2_recorder::recorder::{Config, Overrides, Recorder};
use ams2_recorder::serve::to_json;
use ams2_recorder::telemetry::{Telemetry, mapping_opener};
use ams2_shm::testing::{TestMapping, unique_name};
use common::*;
use serde_json::{Value as Json, json};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use stintmate_ipc::blocking::Peer;
use stintmate_ipc::connection::CallOptions;
use stintmate_ipc::node::{Delivery, SubscribeOptions};
use stintmate_ipc::{Error, codec};

type Got = Arc<Mutex<Vec<(i64, Sample)>>>;

/// The real-time test runs alone (a 4-vCPU CI machine shared with this file's other tests drops
/// a frame now and then); the others run in parallel with each other.
static REALTIME: RwLock<()> = RwLock::new(());

fn shared() -> RwLockReadGuard<'static, ()> {
    REALTIME.read().unwrap_or_else(|e| e.into_inner())
}

fn alone() -> RwLockWriteGuard<'static, ()> {
    REALTIME.write().unwrap_or_else(|e| e.into_inner())
}

fn wait_for(secs: u64, f: impl Fn() -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    f()
}

fn peer_of(fake: &FakePitwall) -> Peer {
    assert!(wait_for(10, || fake.peer().is_some()), "recorder connected");
    fake.peer().unwrap()
}

// ---- the telemetry stream ----------------------------------------------------------------

/// A "game" writing frames into a test mapping at `fps` until stopped.
struct Game {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Game {
    fn start(name: &str, fps: f64) -> Self {
        let mut map = TestMapping::create(name).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut sim = Sim::new();
            let timer = WaitableTimer::new().unwrap();
            timer
                .start_periodic(Duration::from_secs_f64(1.0 / fps))
                .unwrap();
            let mut t = 0.0;
            while !s.load(Ordering::Relaxed) {
                t += 1.0 / fps;
                map.write_frame(sim.frame(1, (t * 40.0) % 4000.0, t));
                timer.wait_timeout(Duration::from_millis(200));
            }
        });
        Self {
            stop,
            thread: Some(thread),
        }
    }
}

struct Rig {
    fake: FakePitwall,
    link: PitwallLink,
    telemetry: Telemetry,
    hints: Arc<Hints>,
    shm: String,
    _dir: tempfile::TempDir,
}

fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path(), &KEY);
    let pipe = pitwall_pipe(&unique_user());
    let fake = FakePitwall::start(&pipe);
    let link = PitwallLink::new(
        &pipe,
        peer_info("recorder"),
        ams2_recorder::link::key_source(&ws).unwrap(),
    )
    .unwrap();
    let hints = Arc::new(Hints::default());
    let shm = unique_name("b2tel");
    let telemetry = Telemetry::install(
        &link.client,
        hints.clone(),
        mapping_opener(Some(shm.clone())),
    )
    .unwrap();
    link.start();
    Rig {
        fake,
        link,
        telemetry,
        hints,
        shm,
        _dir: dir,
    }
}

impl Rig {
    fn running(&self) {
        self.hints.game_running.store(true, Ordering::Relaxed);
        self.hints.generation.fetch_add(1, Ordering::Relaxed);
    }

    fn subscribe(
        &self,
        rate: Option<codec::Value>,
    ) -> Result<(stintmate_ipc::node::Subscription, Got), Error> {
        let got: Got = Arc::default();
        let g = got.clone();
        let opts = SubscribeOptions {
            options: rate.map(|r| codec::map([("rate_hz", r)])),
            ..SubscribeOptions::default()
        };
        let sub = peer_of(&self.fake).subscribe("connector.telemetry", opts, move |d| {
            if let Delivery::Sample(s) = d {
                let b: [u8; 32] = s.body.as_ref().try_into().unwrap();
                g.lock().unwrap().push((s.t_ns, Sample::from_bytes(&b)));
            }
        })?;
        Ok((sub, got))
    }

    fn close(self) {
        self.telemetry.close();
        self.link.close();
        self.fake.stop();
    }
}

fn median_ms(ts: &[i64]) -> f64 {
    let mut d: Vec<f64> = ts.windows(2).map(|w| (w[1] - w[0]) as f64 / 1e6).collect();
    d.sort_by(f64::total_cmp);
    d[d.len() / 2]
}

#[test]
fn one_sample_per_game_frame_phase_locked() {
    let _rt = alone();
    // Real-time: one sample per 90 Hz frame. Shared GitHub-hosted runners skip frames under
    // load (the workflow sets STINTMATE_SHARED_RUNNER there); runs on a PC or our own runner.
    if std::env::var_os("STINTMATE_SHARED_RUNNER").is_some_and(|v| !v.is_empty()) {
        eprintln!("skipped on a shared GitHub-hosted runner: frame timing is not reliable there");
        return;
    }
    let r = rig();
    let mut game = Game::start(&r.shm, 90.0);
    r.running();
    assert!(!r.telemetry.active(), "nothing runs before a subscription");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(r.telemetry.stats().sent, 0);
    let (sub, got) = r.subscribe(None).unwrap();
    std::thread::sleep(Duration::from_millis(2500));
    assert!(r.telemetry.active());
    sub.close();
    let v = got.lock().unwrap().clone();
    assert!(
        v.len() > 140 && v.len() < 280,
        "{} samples in 2.5 s at 90 fps",
        v.len()
    );
    assert!(
        v.iter()
            .all(|(_, s)| s.flags & FRESH != 0 && s.flags & PLAYER_VIEW != 0)
    );
    assert!(v.windows(2).all(|w| w[1].0 > w[0].0), "t_ns increases");
    assert!(
        v.windows(2)
            .all(|w| w[1].1.frame == w[0].1.frame.wrapping_add(1)),
        "the frame counter advances by one per sample"
    );
    let med = median_ms(&v.iter().map(|x| x.0).collect::<Vec<_>>());
    assert!(
        (9.0..13.5).contains(&med),
        "median period {med} ms at 90 fps"
    );
    assert!(
        v.iter()
            .all(|(_, s)| (s.speed_ms - 100.0 / 3.6).abs() < 0.01)
    );
    // unsubscribed: the thread winds down and the count stops growing
    assert!(wait_for(5, || !r.telemetry.active()));
    std::thread::sleep(Duration::from_millis(200));
    let sent = r.telemetry.stats().sent;
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(r.telemetry.stats().sent, sent);
    game.stop.store(true, Ordering::Relaxed);
    game.thread.take().unwrap().join().unwrap();
    r.close();
}

#[test]
fn the_rate_option_lowers_the_cap_and_bad_values_are_refused() {
    let _rt = shared();
    let r = rig();
    let mut game = Game::start(&r.shm, 90.0);
    r.running();
    for bad in [
        codec::Value::from(5),
        codec::Value::from(500),
        codec::Value::from(true),
        codec::Value::from("fast"),
    ] {
        match r.subscribe(Some(bad.clone())) {
            Err(Error::Remote(e)) => assert_eq!(e.code, "bad_request", "{bad:?}"),
            other => panic!("{bad:?}: {:?}", other.map(|_| ())),
        }
    }
    assert!(!r.telemetry.active());
    let (sub, got) = r.subscribe(Some(codec::Value::from(30))).unwrap();
    std::thread::sleep(Duration::from_millis(2000));
    sub.close();
    let n = got.lock().unwrap().len();
    assert!((40..=80).contains(&n), "{n} samples in 2 s at a 30 Hz cap");
    game.stop.store(true, Ordering::Relaxed);
    game.thread.take().unwrap().join().unwrap();
    r.close();
}

#[test]
fn keep_alive_repeats_with_fresh_zero_when_no_frame_comes() {
    let _rt = shared();
    let r = rig();
    let mut game = Game::start(&r.shm, 60.0);
    r.running();
    let (sub, got) = r.subscribe(None).unwrap();
    std::thread::sleep(Duration::from_millis(600));
    game.stop.store(true, Ordering::Relaxed); // the game stops writing (a pause, a menu)
    game.thread.take().unwrap().join().unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let before = got.lock().unwrap().len();
    std::thread::sleep(Duration::from_millis(1300));
    sub.close();
    let v = got.lock().unwrap()[before..].to_vec();
    assert!((3..=7).contains(&v.len()), "{} repeats in 1.3 s", v.len());
    assert!(v.iter().all(|(_, s)| s.flags & FRESH == 0));
    assert!(
        v.windows(2).all(|w| w[1].1.frame == w[0].1.frame),
        "same frame counter"
    );
    let gap = median_ms(&v.iter().map(|x| x.0).collect::<Vec<_>>());
    assert!((200.0..320.0).contains(&gap), "keep-alive every {gap} ms");
    r.close();
}

#[test]
fn without_the_game_idle_samples_say_not_running() {
    let _rt = shared();
    let r = rig(); // hints: game not running
    let (sub, got) = r.subscribe(None).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    sub.close();
    let v = got.lock().unwrap().clone();
    assert!((3..=6).contains(&v.len()), "{} idle samples", v.len());
    assert!(
        v.iter()
            .all(|(_, s)| s.game_state == 0 && s.flags == 0 && s.lap_distance_m == -1.0)
    );
    r.close();
}

// ---- the recorder: live frame and actions ------------------------------------------------

struct Run {
    rec: Recorder,
    fake: FakePitwall,
    map: TestMapping,
    sim: Sim,
    now: Arc<Mutex<Instant>>,
    running: Arc<AtomicBool>,
    clock: Arc<AtomicU64>,
    _dir: tempfile::TempDir,
}

fn run() -> Run {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path(), &KEY);
    let pipe = pitwall_pipe(&unique_user());
    let fake = FakePitwall::start(&pipe);
    let shm = unique_name("b2rec");
    let map = TestMapping::create(&shm).unwrap();
    let base = Instant::now();
    let now = Arc::new(Mutex::new(base));
    let n = now.clone();
    let running = Arc::new(AtomicBool::new(true));
    let (r1, r2) = (running.clone(), running.clone());
    let clock = Arc::new(AtomicU64::new(0f64.to_bits()));
    let c = clock.clone();
    let cfg = Config {
        hz: 50.0,
        workspace: ws.clone(),
        data_dir: ws.join("connectors").join("ams2"),
        pitwall_pipe: Some(pipe),
        control_pipe: None,
        no_control: true,
        shm_name: Some(shm),
    };
    let ov = Overrides {
        game: Some(ams2_platform::game::GameWatch::with(
            move || r1.load(Ordering::Relaxed),
            Instant::now,
            Duration::from_secs(5),
        )),
        clock: Some((
            Arc::new(move || f64::from_bits(c.load(Ordering::Relaxed))),
            "ISO".into(),
        )),
        now: Some(Arc::new(move || *n.lock().unwrap())),
        game_check: Some(Arc::new(move || r2.load(Ordering::Relaxed))),
    };
    let mut rec = Recorder::build(cfg, ov).unwrap();
    rec.start_links();
    rec.connect();
    assert!(rec.mapped());
    Run {
        rec,
        fake,
        map,
        sim: Sim::new(),
        now,
        running,
        clock,
        _dir: dir,
    }
}

impl Run {
    fn play(&mut self, t: f64) {
        self.map.write_frame(self.sim.frame(1, 100.0 + t, t));
        self.rec.tick();
    }

    fn call(&self, method: &str, params: Json) -> Result<Json, Error> {
        peer_of(&self.fake)
            .call(method, to_value(&params), CallOptions::default())
            .map(|v| to_json(&v))
    }

    fn finish(mut self) {
        self.rec.shutdown();
        self.fake.stop();
    }
}

fn remote_code(r: Result<Json, Error>) -> String {
    match r {
        Err(Error::Remote(e)) => e.code,
        other => panic!("expected a remote error, got {other:?}"),
    }
}

#[test]
fn live_frame_answers_from_the_last_snapshot() {
    let _rt = shared();
    let mut r = run();
    // nothing published yet
    assert_eq!(
        remote_code(r.call("connector.live_frame", json!({}))),
        "game_not_running"
    );
    r.play(10.0);
    r.play(10.1);
    let f = r.call("connector.live_frame", json!({})).unwrap();
    assert_eq!(f["sim"], "ams2");
    assert!(f["taken_at"].is_string() && f["phase"].is_string());
    assert!(f["data_age_s"].as_f64().unwrap() >= 0.0);
    assert!(f.get("labels").is_none(), "no labels");
    r.finish();
}

#[test]
fn live_frame_errors_frozen_game_and_unsupported_version() {
    let _rt = shared();
    let mut r = run();
    r.play(10.0);
    r.play(10.1);
    assert!(r.call("connector.live_frame", json!({})).is_ok());
    // the data froze while playing: 2 s without a new frame; the process is still there
    *r.now.lock().unwrap() += Duration::from_secs(2);
    let e = r.call("connector.live_frame", json!({}));
    match &e {
        Err(Error::Remote(x)) => {
            assert_eq!(x.code, "game_not_running");
            assert!(x.message.contains("frozen"), "{}", x.message);
        }
        other => panic!("{other:?}"),
    }
    // ... and the process is gone
    r.running.store(false, Ordering::Relaxed);
    match r.call("connector.live_frame", json!({})) {
        Err(Error::Remote(x)) => assert!(x.message.ends_with("is not running."), "{}", x.message),
        other => panic!("{other:?}"),
    }
    r.running.store(true, Ordering::Relaxed);
    // a version other than 14
    r.sim.d.version = 13;
    r.play(11.0);
    assert_eq!(
        remote_code(r.call("connector.live_frame", json!({}))),
        "internal"
    );
    assert_eq!(
        r.rec.protocol_status()["error_code"],
        "unsupported_sim_version"
    );
    r.finish();
}

fn ready(r: &Run) {
    assert!(
        wait_for(10, || r.rec.announcer().declared()),
        "catalog declared"
    );
}

#[test]
fn actions_ping_announce_and_errors() {
    let _rt = shared();
    let r = run();
    ready(&r);
    let ping = r
        .call("connector.action", json!({"code": "test.ping", "data": {}}))
        .unwrap();
    assert_eq!(
        ping,
        json!({"pong": "pong", "announced": true, "message_id": "m1"})
    );
    let quiet = r
        .call(
            "connector.action",
            json!({"code": "test.ping", "data": {"say": false}}),
        )
        .unwrap();
    assert_eq!(quiet, json!({"pong": "pong", "announced": false}));
    let said = r
        .call(
            "connector.action",
            json!({"code": "test.announce", "data": {"code": "engineer_test",
                   "params": [{"name": "text", "value": "radio test"}]}}),
        )
        .unwrap();
    assert_eq!(said, json!({"announced": true, "message_id": "m1"}));
    let calls = r.fake.calls();
    let last = calls
        .iter()
        .rev()
        .find(|c| c.method == "pitwall.announce")
        .unwrap();
    let p = to_json(&last.params);
    assert_eq!(
        (p["code"].as_str(), p["params"]["text"].as_str()),
        (Some("engineer_test"), Some("radio test"))
    );
    // errors
    assert_eq!(
        remote_code(r.call("connector.action", json!({"code": "nope", "data": {}}))),
        "unknown_action"
    );
    let missing = r.call(
        "connector.action",
        json!({"code": "test.announce", "data": {"code": "engineer_test", "params": []}}),
    );
    assert_eq!(remote_code(missing), "bad_request");
    let unknown = r.call(
        "connector.action",
        json!({"code": "test.announce", "data": {"code": "no_such_code"}}),
    );
    assert_eq!(remote_code(unknown), "bad_request");
    r.finish();
}

#[test]
fn actions_wait_through_the_announcers_rate_window() {
    let _rt = shared();
    let r = run();
    ready(&r);
    let started = Instant::now();
    let mut refused = None;
    for _ in 0..14 {
        let res = r
            .call(
                "connector.action",
                json!({"code": "test.announce", "data": {"code": "engineer_test",
                       "params": [{"name": "text", "value": "x"}]}}),
            )
            .unwrap();
        if res["announced"] == false {
            refused = Some(res);
            break;
        }
    }
    let refused = refused.expect("the 11th announcement in the window is refused");
    assert!(
        refused["refused"].as_str().unwrap().contains("rate limit"),
        "{refused}"
    );
    assert!(started.elapsed() < Duration::from_secs(10));
    r.clock.store(0f64.to_bits(), Ordering::Relaxed);
    r.finish();
}

// ---- the test hook and diagnose ----------------------------------------------------------

fn collect(f: impl FnOnce(&mut dyn FnMut(String)) -> i32) -> (i32, Vec<String>) {
    let mut lines = Vec::new();
    let code = f(&mut |l| lines.push(l));
    (code, lines)
}

#[test]
fn announce_sends_through_a_message_cli_connection() {
    let _rt = shared();
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path(), &KEY);
    let pipe = pitwall_pipe(&unique_user());
    let fake = FakePitwall::start(&pipe);
    let args: Vec<String> = [
        "engineer_test",
        "--param",
        "text=hello",
        "--data",
        "{\"k\":1}",
        "--pitwall-pipe",
        &pipe,
    ]
    .map(String::from)
    .to_vec();
    let (code, lines) = collect(|out| cli::announce_main(&args, &ws, out));
    assert_eq!(
        (code, lines.as_slice()),
        (0, &["sent: m1 (T)".to_string()][..])
    );
    let calls = fake.calls();
    let a = calls
        .iter()
        .find(|c| c.method == "pitwall.announce")
        .unwrap();
    let p = to_json(&a.params);
    assert_eq!(
        (
            p["code"].clone(),
            p["params"]["text"].clone(),
            p["data"]["k"].clone()
        ),
        (json!("engineer_test"), json!("hello"), json!(1))
    );
    assert!(calls.iter().any(|c| c.method == "pitwall.catalog.declare"));
    assert!(
        fake.peers
            .lock()
            .unwrap()
            .iter()
            .all(|p| p.process.as_deref() == Some("message-cli"))
    );
    fake.stop();
}

#[test]
fn announce_checks_its_arguments_before_connecting() {
    let _rt = shared();
    let parse =
        |a: &[&str]| cli::parse_announce(&a.iter().map(|s| s.to_string()).collect::<Vec<_>>());
    assert!(parse(&[]).is_err());
    assert!(
        parse(&["x", "--data", "[1]"])
            .unwrap_err()
            .contains("JSON object")
    );
    assert!(parse(&["x", "--param"]).is_err());
    assert_eq!(
        parse(&["a", "--param", "n=v"]).unwrap(),
        AnnounceArgs {
            code: "a".into(),
            params: vec!["n=v".into()],
            ..AnnounceArgs::default()
        }
    );
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path(), &KEY);
    for args in [
        vec!["engineer_test"],                      // missing param
        vec!["engineer_test", "--param", "nope=1"], // undeclared param
        vec!["engineer_test", "--param", "novalue"],
    ] {
        let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let (code, lines) = collect(|out| cli::announce_main(&a, &ws, out));
        assert_eq!(code, 2, "{args:?}: {lines:?}");
    }
    let (_, lines) = collect(|out| {
        cli::codes(out);
        0
    });
    assert!(
        lines.iter().any(|l| l.starts_with("engineer_test  [agent")),
        "{lines:?}"
    );
}

#[test]
fn diagnose_reads_one_snapshot_of_a_test_mapping() {
    let _rt = shared();
    let name = unique_name("b2diag");
    let mut map = TestMapping::create(&name).unwrap();
    let mut sim = Sim::new();
    map.write_frame(sim.frame(1, 50.0, 12.0));
    let (code, lines) = collect(|out| cli::diagnose(Some(&name), out));
    assert_eq!(code, 0, "{lines:?}");
    let text = lines.join("\n");
    assert!(
        text.contains("version: 14 (supported)") && text.contains("Road_Atlanta"),
        "{text}"
    );
    sim.d.version = 13;
    map.write_frame(&sim.d);
    assert_eq!(collect(|out| cli::diagnose(Some(&name), out)).0, 2);
    let (code, lines) = collect(|out| cli::diagnose(Some(&unique_name("b2none")), out));
    assert_eq!(code, 1);
    assert!(lines[0].starts_with("error: mapping"), "{lines:?}");
}
