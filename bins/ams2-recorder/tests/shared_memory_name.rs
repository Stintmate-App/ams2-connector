//! `ams2-recorder.exe --shared-memory-name`: the real binary reads a replayed session from a
//! test mapping (the game counts as running) and writes the laps to a fake Pitwall.

mod common;

use ams2_core::testing::scripted_session;
use ams2_shm::testing::{TestMapping, unique_name};
use common::*;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn recorder() -> Command {
    Command::new(env!("CARGO_BIN_EXE_ams2-recorder"))
}

#[test]
fn the_game_s_own_mapping_name_is_refused() {
    for name in ["$pcars2$", "Local-x-pcars2-y", ""] {
        let o = recorder()
            .args(["--shared-memory-name", name])
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{name}");
        assert!(String::from_utf8_lossy(&o.stderr).contains("test mapping"));
    }
}

#[test]
fn a_rate_that_is_not_a_positive_number_is_a_usage_error() {
    for hz in ["0", "-5", "abc", "inf", "NaN", ""] {
        let o = recorder().args(["--hz", hz]).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{hz}");
        assert!(String::from_utf8_lossy(&o.stderr).contains("--hz needs a positive number"));
    }
    let o = recorder().arg("--hz").output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("--hz needs a value"));
    assert_eq!(ams2_recorder::cli::parse_hz(Some("200")), Ok(200.0));
}

#[test]
fn a_replayed_session_drives_the_real_recorder() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(&dir.path().join("ws"), &KEY);
    std::fs::write(ws.join("stintmate.json"), r#"{"schema": 1}"#).unwrap();
    let pipe = pitwall_pipe(&unique_user());
    let pitwall = FakePitwall::start(&pipe);
    let shm = unique_name("opt");
    let mut map = TestMapping::create(&shm).unwrap();
    // the first frame is in place before the recorder opens the mapping (version check)
    let frames = scripted_session();
    map.write_frame(&frames[0].frame);
    let tag = format!("t{}", std::process::id());
    let mut child = recorder()
        .arg("--workspace")
        .arg(&ws)
        .args([
            "--pitwall-pipe",
            &pipe,
            "--shared-memory-name",
            &shm,
            "--hz",
            "200",
        ])
        .env("AMS2_CONNECTOR_DEV_INSTANCE", &tag)
        .env("STINTMATE_CONNECTORS_DIR", dir.path().join("connectors"))
        .env("STINTMATE_SETTINGS_FILE", dir.path().join("settings.json"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // let the recorder open the mapping, then replay the session faster than real time
    std::thread::sleep(Duration::from_secs(1));
    let mut last: Option<u32> = None;
    for f in frames {
        if last != Some(f.frame.sequence_number) {
            last = Some(f.frame.sequence_number);
            map.write_frame(&f.frame);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    while pitwall.count("pitwall.write.lap") < 3 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let laps = pitwall.count("pitwall.write.lap");
    let sessions = pitwall.count("pitwall.write.session_start");
    let _ = child.kill();
    let _ = child.wait();
    let log = std::fs::read_to_string(ws.join("connectors/ams2/recorder.log")).unwrap_or_default();
    assert!(
        sessions >= 1,
        "session_start writes: {sessions}
{log}"
    );
    assert!(
        laps >= 3,
        "lap writes: {laps}
{log}"
    );
}
