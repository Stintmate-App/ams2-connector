//! `ams2-measure`: what the recorder costs and how steady it is (plan section 3; port of
//! `scripts/measure_telemetry.py`). No game needed and nothing of the driver's touched: a unique
//! pipe, a throw-away key and workspace in the temp folder, a test mapping.
//!
//! ```text
//! ams2-measure [--seconds 20] [--rounds 3] [--fps 90] [--rate N] [--cars 30]
//! ```
//!
//! Two processes: this one plays Pitwall (a fake endpoint that subscribes to
//! `connector.telemetry` in alternate windows) and the game (a thread writing synthetic frames
//! into a test mapping, as AMS2 in VR does at `--fps`); the child (`--child`) is the real
//! recorder loop on that mapping at 50 Hz. The child's CPU is read by `QueryProcessCycleTime`
//! (exact, not tick-sampled) and calibrated against one core's cycles per second; the stream's
//! period comes from the samples' `t_ns` (when the recorder first saw each frame) and its lag
//! from the writer's own clock; the loop's period and tick time are recorded inside the child.

use ams2_core::testing::Sim;
use ams2_platform::game::GameWatch;
use ams2_platform::timer::WaitableTimer;
use ams2_recorder::recorder::{Config, Overrides, Recorder};
use ams2_shm::SharedMemory;
use ams2_shm::testing::{TestMapping, unique_name};
use serde_json::json;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use stintmate_ipc::blocking::Endpoint;
use stintmate_ipc::clock::now_ns;
use stintmate_ipc::codec::{self, Value};
use stintmate_ipc::handshake::PeerInfo;
use stintmate_ipc::keyfile::{Key, KeySource, format_key};
use stintmate_ipc::node::{Delivery, SubscribeOptions};
use stintmate_ipc::{MethodOpts, naming};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows_sys::Win32::System::Threading::{
    GetCurrentThread, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_VM_READ,
};
use windows_sys::Win32::System::WindowsProgramming::{QueryProcessCycleTime, QueryThreadCycleTime};

fn arg(args: &[String], flag: &str) -> Option<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(i) = args.iter().position(|a| a == "--child") {
        child(&args[i + 1..]);
        return;
    }
    let num = |f: &str, d: f64| arg(&args, f).and_then(|v| v.parse().ok()).unwrap_or(d);
    let rate = arg(&args, "--rate").and_then(|v| v.parse::<i64>().ok());
    parent(
        num("--seconds", 20.0),
        num("--rounds", 3.0) as usize,
        num("--fps", 90.0),
        rate,
        num("--cars", 30.0) as usize,
    );
}

// ---- the recorder process ----------------------------------------------------------------

/// `--child PIPE WORKSPACE SHM CAPACITY`: the real loop until stdin closes, then its timings.
fn child(a: &[String]) {
    let cap: usize = a[3].parse().unwrap_or(100_000);
    let ws = std::path::PathBuf::from(&a[1]);
    let cfg = Config {
        hz: 50.0,
        workspace: ws.clone(),
        data_dir: ws.join("connectors").join("ams2"),
        pitwall_pipe: Some(a[0].clone()),
        control_pipe: None,
        no_control: true,
        shm_name: Some(a[2].clone()),
    };
    let ov = Overrides {
        game: Some(GameWatch::with(
            || true,
            Instant::now,
            Duration::from_secs(5),
        )),
        game_check: Some(Arc::new(|| true)),
        ..Overrides::default()
    };
    let t0 = Instant::now();
    ams2_platform::proc::lower_process_priority(); // as the recorder's main does
    let mut rec = Recorder::build(cfg, ov).expect("the recorder builds");
    rec.enable_stats(cap);
    let stop = Arc::new(AtomicBool::new(false));
    let s = stop.clone();
    std::thread::spawn(move || {
        let _ = std::io::stdin().read_to_end(&mut Vec::new());
        s.store(true, Ordering::Relaxed);
    });
    rec.run(&|| stop.load(Ordering::Relaxed));
    let st = rec.loop_stats().cloned().unwrap_or_default();
    println!(
        "STATS {}",
        json!({"periods": st.periods, "ticks": st.ticks, "stages": st.stages,
               "first_frame_s": st.first_frame_at.map(|t| t.duration_since(t0).as_secs_f64())})
    );
}

// ---- the measuring process ---------------------------------------------------------------

fn random_key() -> Key {
    let mut key = [0u8; 32];
    for chunk in key.chunks_mut(8) {
        let mut h = RandomState::new().build_hasher();
        h.write_u64(now_ns() as u64);
        chunk.copy_from_slice(&h.finish().to_le_bytes());
    }
    key
}

fn cycles(h: HANDLE) -> u64 {
    let mut c = 0u64;
    // SAFETY: a valid process handle and out pointer.
    unsafe { QueryProcessCycleTime(h, &mut c) };
    c
}

/// One core's cycles per second, from a 0.5 s busy loop on this thread.
fn cycles_per_second() -> f64 {
    let (mut a, mut b) = (0u64, 0u64);
    // SAFETY: the pseudo handle of this thread and out pointers.
    unsafe {
        let me = GetCurrentThread();
        QueryThreadCycleTime(me, &mut a);
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(500) {
            std::hint::spin_loop();
        }
        QueryThreadCycleTime(me, &mut b);
        (b - a) as f64 / t0.elapsed().as_secs_f64()
    }
}

fn memory(h: HANDLE) -> (f64, f64) {
    // SAFETY: zeroed plain data, sized for the call.
    let mut m: PROCESS_MEMORY_COUNTERS_EX = unsafe { core::mem::zeroed() };
    m.cb = size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32;
    unsafe { GetProcessMemoryInfo(h, &mut m as *mut _ as *mut _, m.cb) };
    (m.PrivateUsage as f64 / 1e6, m.WorkingSetSize as f64 / 1e6)
}

fn pct(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() as f64 * p) as usize).min(sorted.len() - 1)]
}

fn sorted(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(f64::total_cmp);
    v
}

fn median(v: &[f64]) -> f64 {
    pct(&sorted(v.to_vec()), 0.5)
}

fn parent(seconds: f64, rounds: usize, fps: f64, rate: Option<i64>, cars: usize) {
    let key = random_key();
    let dir = std::env::temp_dir().join(format!("ams2-measure-{}", std::process::id()));
    let ipc = dir.join("ipc");
    std::fs::create_dir_all(&ipc).expect("a temp workspace");
    std::fs::write(ipc.join("key"), format_key(&key)).expect("a temp key");
    let user = format!("m{}x{}", std::process::id(), now_ns() % 100_000);
    let pipe = naming::pipe_path_for("pitwall", None, &user).expect("a pipe name");

    // Pitwall: accepts the writes, subscribes in alternate windows.
    let endpoint = Endpoint::builder(PeerInfo::new("pitwall"), KeySource::Bytes(key))
        .pipe(&pipe)
        .build()
        .expect("the fake Pitwall");
    for m in [
        "pitwall.write.catalog",
        "pitwall.write.session_start",
        "pitwall.write.lap",
        "pitwall.write.setup",
        "pitwall.write.pit_exit",
        "pitwall.write.participant_laps",
        "pitwall.write.session_end",
        "pitwall.catalog.declare",
        "pitwall.guidance.declare",
        "pitwall.actions.declare",
        "pitwall.announce",
    ] {
        endpoint
            .method(
                m,
                MethodOpts::default().ordered(true).idempotent(true),
                |_c, _p| {
                    Ok(codec::map([
                        ("session_id", Value::from(1u32)),
                        ("lap", Value::from(1u32)),
                        ("stint", Value::from(1u32)),
                        ("message_id", Value::from("m")),
                    ]))
                },
            )
            .expect("a method");
    }
    endpoint.start().expect("the endpoint starts");

    // The game: synthetic on-track frames at `fps`; the writer's clock is kept for the lag.
    let shm = unique_name("measure");
    let mut map = TestMapping::create(&shm).expect("a test mapping");
    let writes: Arc<Mutex<Vec<i64>>> = Arc::new(Mutex::new(Vec::with_capacity(
        (fps * (seconds + 2.0) * rounds as f64 * 2.0 + fps * 20.0) as usize,
    )));
    let stop = Arc::new(AtomicBool::new(false));
    let game = {
        let (stop, writes) = (stop.clone(), writes.clone());
        std::thread::spawn(move || {
            let mut sim = Sim::new();
            let timer = WaitableTimer::new().expect("a timer");
            timer
                .start_periodic(Duration::from_secs_f64(1.0 / fps))
                .expect("a periodic timer");
            let t0 = Instant::now();
            while !stop.load(Ordering::Relaxed) {
                let t = t0.elapsed().as_secs_f64();
                let mut f = Box::new(*sim.frame(1, (t * 40.0) % 4300.0, t));
                add_traffic(&mut f, cars);
                map.write_frame(&f);
                writes.lock().unwrap().push(now_ns());
                timer.wait_timeout(Duration::from_millis(100));
            }
        })
    };

    let cap = ((seconds + 2.0) * rounds as f64 * 2.0 * 50.0 + 4000.0) as usize;
    let mut proc = Command::new(std::env::current_exe().expect("own path"))
        .args([
            "--child",
            &pipe,
            &dir.to_string_lossy(),
            &shm,
            &cap.to_string(),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("the recorder process starts");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        measure(&endpoint, &proc, seconds, rounds, fps, rate, &writes)
    }));
    drop(proc.stdin.take()); // the child's stop signal
    let mut out = String::new();
    if let Some(so) = proc.stdout.take() {
        for line in BufReader::new(so).lines().map_while(Result::ok) {
            out.push_str(&line);
        }
    }
    let end = Instant::now() + Duration::from_secs(30);
    while proc.try_wait().ok().flatten().is_none() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = proc.kill(); // our own child, by its handle
    stop.store(true, Ordering::Relaxed);
    let _ = game.join();
    endpoint.stop();
    let _ = std::fs::remove_dir_all(&dir);
    let Ok((cpu, stream)) = result else {
        eprintln!("measurement failed");
        std::process::exit(1);
    };
    let stats: serde_json::Value = out
        .strip_prefix("STATS ")
        .and_then(|j| serde_json::from_str(j).ok())
        .unwrap_or_default();
    report(seconds, rounds, fps, rate, &cpu, &stream, &stats);
}

struct Cpu {
    off: Vec<f64>,
    on: Vec<f64>,
    private_mb: f64,
    working_set_mb: f64,
    ghz: f64,
}

struct Stream {
    rate: f64,
    periods_ms: Vec<f64>,
    lag_ms: Vec<f64>,
}

fn measure(
    endpoint: &Endpoint,
    proc: &std::process::Child,
    seconds: f64,
    rounds: usize,
    fps: f64,
    rate: Option<i64>,
    writes: &Arc<Mutex<Vec<i64>>>,
) -> (Cpu, Stream) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while endpoint.peers().is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let peer = endpoint
        .peers()
        .into_iter()
        .next()
        .expect("the recorder connects");
    // SAFETY: plain call; the child outlives it.
    let h = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_VM_READ,
            0,
            proc.id(),
        )
    };
    assert!(!h.is_null(), "a handle to the child");
    std::thread::sleep(Duration::from_secs(3)); // let it settle
    let hz = cycles_per_second();
    let window = || {
        let c0 = cycles(h);
        std::thread::sleep(Duration::from_secs_f64(seconds));
        (cycles(h) - c0) as f64 / hz / seconds * 100.0
    };
    let (mut off, mut on, mut periods, mut lags) = (vec![], vec![], vec![], vec![]);
    let mut samples_per_s = 0.0;
    for _ in 0..rounds {
        off.push(window());
        let got: Arc<Mutex<Vec<i64>>> = Arc::default();
        let g = got.clone();
        let opts = SubscribeOptions {
            options: rate.map(|r| codec::map([("rate_hz", Value::from(r))])),
            ..SubscribeOptions::default()
        };
        let sub = peer
            .subscribe("connector.telemetry", opts, move |d| {
                if let Delivery::Sample(s) = d {
                    g.lock().unwrap().push(s.t_ns);
                }
            })
            .expect("the subscription");
        std::thread::sleep(Duration::from_millis(500));
        let n0 = got.lock().unwrap().len();
        on.push(window());
        sub.close();
        let ts: Vec<i64> = got.lock().unwrap()[n0..].to_vec();
        periods.extend(ts.windows(2).map(|w| (w[1] - w[0]) as f64 / 1e6));
        let w = writes.lock().unwrap();
        for t in &ts {
            // the writer's last write at or before the moment the recorder first saw the frame
            let i = w.partition_point(|x| x <= t);
            if i > 0 {
                lags.push((t - w[i - 1]) as f64 / 1e6);
            }
        }
        samples_per_s = ts.len() as f64 / seconds;
        std::thread::sleep(Duration::from_millis(500));
    }
    let (private_mb, working_set_mb) = memory(h);
    // SAFETY: our own handle.
    unsafe { CloseHandle(h) };
    let _ = fps;
    (
        Cpu {
            off,
            on,
            private_mb,
            working_set_mb,
            ghz: hz / 1e9,
        },
        Stream {
            rate: samples_per_s,
            periods_ms: periods,
            lag_ms: lags,
        },
    )
}

fn report(
    seconds: f64,
    rounds: usize,
    fps: f64,
    rate: Option<i64>,
    cpu: &Cpu,
    s: &Stream,
    stats: &serde_json::Value,
) {
    let list = |v: &[f64]| {
        v.iter()
            .map(|x| format!("{x:.3}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    println!(
        "ams2-measure: {rounds} rounds of {seconds} s, game {fps} frames/s, one core = {:.2} GHz",
        cpu.ghz
    );
    println!("recorder CPU, % of one core (QueryProcessCycleTime):");
    println!(
        "  without the subscription: {} (median {:.3})",
        list(&cpu.off),
        median(&cpu.off)
    );
    println!(
        "  with the subscription:    {} (median {:.3})",
        list(&cpu.on),
        median(&cpu.on)
    );
    println!(
        "  the stream adds {:.3} %",
        median(&cpu.on) - median(&cpu.off)
    );
    println!(
        "recorder private working set {:.1} MB (working set {:.1} MB)",
        cpu.private_mb, cpu.working_set_mb
    );
    if !s.periods_ms.is_empty() {
        let p = sorted(s.periods_ms.clone());
        let n = p.len() as f64;
        let mean = p.iter().sum::<f64>() / n;
        let sd = (p.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt();
        println!(
            "stream: {:.1} samples/s{}; period (ms): median {:.2}, mean {:.2}, sd {:.2}, p1 {:.2}, p99 {:.2}, max {:.2} (game frame {:.2})",
            s.rate,
            rate.map_or(String::new(), |r| format!(" (o.rate_hz {r})")),
            pct(&p, 0.5),
            mean,
            sd,
            pct(&p, 0.01),
            pct(&p, 0.99),
            p[p.len() - 1],
            1000.0 / fps
        );
    }
    if !s.lag_ms.is_empty() {
        let l = sorted(s.lag_ms.clone());
        println!(
            "stream t_ns after the frame was written (ms): median {:.3}, p99 {:.3}, max {:.3}",
            pct(&l, 0.5),
            pct(&l, 0.99),
            l[l.len() - 1]
        );
    }
    let get = |k: &str| -> Vec<f64> {
        stats[k]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_f64()).collect())
            .unwrap_or_default()
    };
    let (periods, ticks) = (sorted(get("periods")), sorted(get("ticks")));
    if !periods.is_empty() {
        let mean = periods.iter().sum::<f64>() / periods.len() as f64;
        let sd =
            (periods.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / periods.len() as f64).sqrt();
        println!(
            "loop period (ms): median {:.2}, sd {:.2}, p99 {:.2}, max {:.2} ({} ticks)",
            pct(&periods, 0.5) * 1e3,
            sd * 1e3,
            pct(&periods, 0.99) * 1e3,
            periods[periods.len() - 1] * 1e3,
            periods.len()
        );
    }
    if !ticks.is_empty() {
        println!(
            "tick (snapshot + tracker + detector + publish), us: median {:.1}, p99 {:.1}, max {:.1}",
            pct(&ticks, 0.5) * 1e6,
            pct(&ticks, 0.99) * 1e6,
            ticks[ticks.len() - 1] * 1e6
        );
    }
    let stages: Vec<Vec<f64>> = stats["stages"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| serde_json::from_value(x.clone()).ok())
                .collect()
        })
        .unwrap_or_default();
    if !stages.is_empty() {
        let names = ["snapshot", "tracker", "live publish", "detector + status"];
        let parts: Vec<String> = names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let v = sorted(stages.iter().filter_map(|s| s.get(i).copied()).collect());
                format!("{n} {:.1}", pct(&v, 0.5) * 1e6)
            })
            .collect();
        println!("tick stages (median), us: {}", parts.join(", "));
    }
    if let Some(f) = stats["first_frame_s"].as_f64() {
        println!(
            "start (build + links + mapping) to the first snapshot: {:.1} ms",
            f * 1e3
        );
    }
}

/// Fills the frame with `cars` participants around the player (PRY-098): one alongside on each
/// side (the spotter calls them, "In the middle", then "Still there" every 3 s) and the rest
/// ahead and behind, some within the spotter's 20 m search range but not alongside.
fn add_traffic(f: &mut SharedMemory, cars: usize) {
    let n = cars.clamp(1, f.participant_info.len());
    f.num_participants = f.num_participants.max(n as i32);
    f.orientation = [0.0; 3];
    let (px, pz) = (
        f.participant_info[0].world_position[0],
        f.participant_info[0].world_position[2],
    );
    for i in 1..n {
        let (dx, dz) = match i {
            1 => (3.0, 0.0),
            2 => (-3.0, 1.0),
            _ => {
                let k = (i - 3) as f32;
                let side = if i % 2 == 0 { 7.0 } else { -7.0 };
                let dz = if i % 4 < 2 {
                    -(8.0 + 9.0 * k)
                } else {
                    8.0 + 9.0 * k
                };
                (side, dz)
            }
        };
        f.participant_info[i].is_active = 1;
        f.participant_info[i].world_position = [px + dx, 0.0, pz + dz];
        f.speeds[i] = f.speed;
    }
}
