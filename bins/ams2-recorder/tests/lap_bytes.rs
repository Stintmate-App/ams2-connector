//! Byte-level golden: the `p` of one lap equals the fixture for the same record and
//! rows (`tests/fixtures/lap_params_300.msgpack`, made once by `scripts/gen_lap_fixture.py` with an
//! earlier implementation of the connector that is not in this repository).
//! The channels are `bin` of little-endian float32 (4 bytes a sample), never f64 or arrays.

use ams2_core::channels::{N_CHANNELS, Row};
use ams2_core::messages::{LapRecord, lap_message};
use ams2_recorder::link::to_value;
use stintmate_ipc::codec::{self, Value};

const GOLDEN: &[u8] = include_bytes!("fixtures/lap_params_300.msgpack");
const ROWS: usize = 300;

fn rows(n: usize) -> Vec<Row> {
    (0..n)
        .map(|i| {
            let mut r = [0.0; N_CHANNELS];
            for (j, v) in r.iter_mut().enumerate() {
                *v = ((i * 37 + j * 11) % 1013) as f64 * 0.173;
            }
            r
        })
        .collect()
}

fn record(n: usize) -> LapRecord {
    LapRecord {
        session_id: 1,
        lap_number: 3,
        started_at: "2026-10-23T10:00:00.000+02:00".into(),
        lap_time_s: Some(90.123),
        measured_time_s: Some(90.1),
        sector1_s: Some(30.1),
        sector2_s: Some(29.9),
        sector3_s: Some(30.123),
        complete: true,
        valid: true,
        out_lap: false,
        in_lap: false,
        fuel_start_l: Some(50.0),
        fuel_end_l: Some(47.5),
        max_speed_kmh: Some(250.5),
        n_samples: n,
        on_track: true,
        game_lap: Some(3),
        lap_time_source: Some("official".into()),
        formation_lap: false,
        ambient_c: Some(21.0),
        track_c: Some(33.5),
        rain: Some(0.0),
        wind_speed: Some(2.5),
    }
}

fn params(n: usize) -> Value {
    let mut p = to_value(&lap_message(&record(n), &rows(n)));
    if let Value::Map(m) = &mut p {
        m.push((Value::from("session_key"), Value::from("0123456789abcdef")));
        m.push((Value::from("seq"), Value::from(3u64)));
    }
    p
}

#[test]
fn a_lap_params_are_byte_equal_to_the_fixture() {
    let bytes = codec::encode(&params(ROWS)).unwrap();
    if bytes != GOLDEN {
        let at = bytes.iter().zip(GOLDEN).position(|(a, b)| a != b);
        panic!(
            "lap params differ from the fixture: {} vs {} bytes, first difference at {at:?}",
            bytes.len(),
            GOLDEN.len()
        );
    }
}

#[test]
fn a_90_s_lap_is_about_1_3_mb() {
    let bytes = codec::encode(&params(4500)).unwrap();
    // About 1,298,649 bytes with the envelope; the envelope adds ~60 bytes.
    assert!(
        (1_298_000..1_299_000).contains(&bytes.len()),
        "{}",
        bytes.len()
    );
}

/// Timing of what the writer thread does for a 90 s lap (4500 rows): message, value, encode,
/// write + fsync + rename of the file. Run in release:
/// `cargo test --release -p ams2-recorder --test lap_bytes -- --ignored --nocapture`.
#[test]
#[ignore = "timing: run in release with --ignored"]
fn a_90_s_lap_message_encode_and_store_time() {
    let dir = tempfile::tempdir().unwrap();
    let rows = rows(4500);
    let mut ms = Vec::new();
    for k in 0..7 {
        let t0 = std::time::Instant::now();
        let msg = lap_message(&record(4500), &rows);
        let doc = codec::map([
            ("method", Value::from("lap")),
            ("v", Value::from(1u32)),
            ("ik", Value::from("x")),
            ("p", to_value(&msg)),
            ("at", Value::from("2026-10-23T10:00:00.000+02:00")),
        ]);
        let bytes = codec::encode(&doc).unwrap();
        let tmp = dir.path().join(format!("{k}.tmp"));
        let mut f = std::fs::File::create(&tmp).unwrap();
        std::io::Write::write_all(&mut f, &bytes).unwrap();
        f.sync_all().unwrap();
        drop(f);
        std::fs::rename(&tmp, dir.path().join(format!("{k}.msgpack"))).unwrap();
        ms.push((t0.elapsed().as_secs_f64() * 1e3, bytes.len()));
    }
    eprintln!("90 s lap message + encode + store (ms, bytes): {ms:?}");
}
