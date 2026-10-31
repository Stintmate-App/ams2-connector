//! A 90 s lap encoded and stored by the writer thread (target 5 ms). Timing, so it
//! is ignored by default; run it from a release build:
//! `cargo test --release -p ams2-recorder --test lap_write -- --ignored --nocapture`.
//! A test mapping and a temporary workspace; Pitwall's pipe is a random one nobody serves, so
//! the lap stays in the outbox. Its synthetic frames do not close laps like the game, so the
//! stored file holds more than 4500 samples: the size and the encode time of a real 90 s lap
//! are in `lap_bytes.rs`.

mod common;
use ams2_core::testing::Sim;
use ams2_platform::game::GameWatch;
use ams2_recorder::recorder::{Config, Overrides, Recorder};
use ams2_shm::testing::{TestMapping, unique_name};
use common::*;
use std::time::{Duration, Instant};

fn pending(rec: &Recorder) -> u64 {
    rec.protocol_status()["outbox"]["pending"]
        .as_u64()
        .unwrap_or(0)
}

#[test]
#[ignore = "timing: run in release with --ignored"]
fn a_90_s_lap_encoded_and_stored_time() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path(), &KEY);
    let shm = unique_name("lapw");
    let mut map = TestMapping::create(&shm).unwrap();
    let cfg = Config {
        hz: 50.0,
        workspace: ws.clone(),
        data_dir: ws.join("connectors").join("ams2"),
        pitwall_pipe: Some(pitwall_pipe(&unique_user())),
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
        ..Overrides::default()
    };
    let mut rec = Recorder::build(cfg, ov).unwrap();
    rec.connect();
    let mut sim = Sim::new();
    let mut times = Vec::new();
    let mut t = 0.0f64;
    for lap in 1..=4u32 {
        for i in 200..4500 {
            t += 0.02;
            map.write_frame(sim.frame(lap, f64::from(i) * 0.88, t));
            rec.tick();
        }
        // The lap is saved once the game reports its time (a few frames after the crossing):
        // tick the next lap's frames and time the tick that hands it to the writer, up to the
        // file in the outbox.
        let before = pending(&rec);
        for i in 0..200 {
            t += 0.02;
            map.write_frame(sim.frame(lap + 1, f64::from(i) * 0.88, t));
            let t0 = Instant::now();
            rec.tick();
            while pending(&rec) == before && t0.elapsed() < Duration::from_millis(20) {
                std::hint::spin_loop();
            }
            if pending(&rec) != before {
                if lap > 1 {
                    times.push(t0.elapsed().as_secs_f64() * 1e3); // lap 1 also opened the session
                }
                break;
            }
        }
    }
    let files: Vec<u64> = std::fs::read_dir(ws.join("connectors").join("ams2").join("outbox"))
        .map(|d| {
            d.flatten()
                .filter_map(|e| e.metadata().ok())
                .map(|m| m.len())
                .collect()
        })
        .unwrap_or_default();
    times.sort_by(f64::total_cmp);
    eprintln!("90 s lap, crossing to stored (ms): {times:?}; outbox file sizes {files:?}");
    assert!(!times.is_empty(), "a lap reached the outbox"); // the 5 ms target: BENCHMARKS.md
    rec.shutdown();
}
