//! `capture-raw` (dev builds only, feature `capture-raw`): records every changed
//! frame of the game's shared memory to a file, for the offline comparison of the connectors and
//! for test fixtures. Started by hand or by `scripts/capture/start-capture.ps1`; never by the
//! control app.
//!
//! ```text
//! capture-raw FILE [--seconds N] [--stop-file PATH] [--shm-name NAME]
//! ```
//!
//! * Waits for the game's mapping if it does not exist yet (up to the cap).
//! * Ends cleanly (file finished, summary written) on: the stop file existing, Ctrl+C or
//!   Ctrl+Break, the game process exiting, or `--seconds` (default 3 h, a safety cap).
//! * `FILE.json` is a summary, rewritten about every 10 s while recording and at the end.
//! * Cheap by design: below-normal priority, a 4 MB buffered writer, no allocation per frame.
//!   Every changed frame is kept.

use ams2_platform::console::interrupt_flag;
use ams2_platform::game::GameProbe;
use ams2_platform::proc::{lower_process_priority, process_cpu_time};
use ams2_shm::capture::{
    CaptureSummary, CaptureWriter, RecordOptions, StopReason, Stopper, record_with,
};
use ams2_shm::{Mapping, ShmError, new_frame};
use serde_json::{Value, json};
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The safety cap when `--seconds` is not given: three hours.
pub const DEFAULT_SECONDS: f64 = 3.0 * 3600.0;
/// Size of the write buffer.
pub const BUFFER_BYTES: usize = 4 * 1024 * 1024;
/// How often the summary file is refreshed while recording.
pub const PROGRESS_EVERY_US: u64 = 10_000_000;

/// The command line of `capture-raw`.
#[derive(Debug, PartialEq)]
pub struct Args {
    /// The capture file.
    pub file: PathBuf,
    /// The safety cap in seconds.
    pub seconds: f64,
    /// Stop when this file exists.
    pub stop_file: Option<PathBuf>,
    /// A test mapping instead of the game's.
    pub shm_name: Option<String>,
}

/// Parses `capture-raw`'s arguments.
pub fn parse(args: &[String]) -> Result<Args, String> {
    let (mut file, mut seconds, mut stop_file, mut shm_name) = (None, DEFAULT_SECONDS, None, None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--seconds" => {
                seconds = it
                    .next()
                    .and_then(|v| v.parse::<f64>().ok())
                    .filter(|s| s.is_finite() && *s > 0.0)
                    .ok_or("--seconds needs a positive number")?;
            }
            "--stop-file" => {
                stop_file = Some(PathBuf::from(it.next().ok_or("--stop-file needs a path")?));
            }
            "--shm-name" => shm_name = Some(it.next().ok_or("--shm-name needs a name")?.clone()),
            s if !s.starts_with("--") && file.is_none() => file = Some(PathBuf::from(s)),
            other => return Err(format!("unexpected argument {other}")),
        }
    }
    Ok(Args {
        file: file.ok_or("capture-raw needs a FILE")?,
        seconds,
        stop_file,
        shm_name,
    })
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Writes `value` next to `path` and renames it over, so a reader never sees half a file.
fn write_json(path: &Path, value: &Value) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, serde_json::to_vec_pretty(value).unwrap_or_default())?;
    std::fs::rename(&tmp, path)
}

/// The facts of a run outside the frames.
struct Run<'a> {
    file: &'a Path,
    started: Instant,
    started_unix: u64,
    waited_s: f64,
}

impl Run<'_> {
    fn json(&self, summary: &CaptureSummary, state: &str, reason: Option<&str>) -> Value {
        summary.to_json(json!({
            "state": state,
            "stop_reason": reason,
            "file": self.file.file_name().map(|n| n.to_string_lossy()),
            "file_bytes": std::fs::metadata(self.file).map(|m| m.len()).ok(),
            "started_unix": self.started_unix,
            "updated_unix": unix_now(),
            "wall_seconds": self.started.elapsed().as_secs_f64(),
            "waited_for_game_seconds": self.waited_s,
            "cpu_seconds": process_cpu_time(std::process::id()).map(|d| d.as_secs_f64()),
            "pid": std::process::id(),
        }))
    }
}

/// `capture-raw FILE [--seconds N] [--stop-file PATH] [--shm-name NAME]`: prints its summary
/// and returns the exit code (0: ended by a stop condition, 1: error, 2: usage).
pub fn run(args: &[String], out: &mut dyn FnMut(String)) -> i32 {
    let args = match parse(args) {
        Ok(a) => a,
        Err(e) => {
            out(format!("error: {e}"));
            return 2;
        }
    };
    lower_process_priority();
    let json_path = {
        let mut p = args.file.as_os_str().to_owned();
        p.push(".json");
        PathBuf::from(p)
    };
    if let Some(dir) = args.file.parent().filter(|d| !d.as_os_str().is_empty()) {
        let _ = std::fs::create_dir_all(dir);
    }
    let writer = std::fs::File::create(&args.file)
        .map(|f| BufWriter::with_capacity(BUFFER_BYTES, f))
        .map_err(ShmError::from)
        .and_then(CaptureWriter::new);
    let mut writer = match writer {
        Ok(w) => w,
        Err(e) => {
            out(format!("error: {}: {e}", args.file.display()));
            return 1;
        }
    };
    let mut run = Run {
        file: &args.file,
        started: Instant::now(),
        started_unix: unix_now(),
        waited_s: 0.0,
    };
    let mut summary = CaptureSummary::default();
    let _ = write_json(&json_path, &run.json(&summary, "waiting", None));

    let mut stopper =
        Stopper::new(Duration::from_secs_f64(args.seconds)).interrupt(interrupt_flag());
    if let Some(f) = &args.stop_file {
        stopper = stopper.stop_file(f);
    }
    if args.shm_name.is_none() {
        // The mapping outlives the game, so the game's process says when it is over; only
        // after it has been seen (a capture may start before the game).
        let (mut probe, mut seen) = (GameProbe::new(), false);
        stopper = stopper.game_exited(move || {
            if probe.running() {
                seen = true;
                false
            } else {
                seen
            }
        });
    }

    // Wait for the mapping.
    let mut source = loop {
        let opened = match &args.shm_name {
            Some(n) => Mapping::open(n),
            None => Mapping::open_game(),
        };
        match opened {
            Ok(m) => break Some(m),
            Err(ShmError::NotFound(_)) => {
                if stopper.poll().is_some() {
                    break None;
                }
                std::thread::sleep(Duration::from_millis(250));
            }
            Err(e) => {
                out(format!("error: {e}"));
                let _ = write_json(&json_path, &run.json(&summary, "error", Some("error")));
                return 1;
            }
        }
    };
    run.waited_s = run.started.elapsed().as_secs_f64();

    let mut error = None;
    let mut ended = false;
    if let Some(source) = source.as_mut() {
        let opts = RecordOptions {
            interval: Duration::from_millis(2),
            max_frames: None,
        };
        let mut next_progress = PROGRESS_EVERY_US;
        let res = record_with(
            source,
            &mut writer,
            &mut new_frame(),
            &opts,
            |us, frame| {
                summary.observe(us, frame);
                if us >= next_progress {
                    next_progress = us + PROGRESS_EVERY_US;
                    let _ = write_json(&json_path, &run.json(&summary, "recording", None));
                }
            },
            || stopper.poll().is_some(),
        );
        match res {
            Ok(o) => ended = o.source_ended,
            Err(e) => error = Some(e),
        }
    }
    if let Err(e) = writer.finish() {
        error.get_or_insert(e);
    }
    drop(source);

    let reason = if error.is_some() {
        "error"
    } else if ended {
        StopReason::SourceEnded.as_str()
    } else {
        stopper.poll().map_or("unknown", StopReason::as_str)
    };
    let state = if error.is_some() { "error" } else { "finished" };
    let json = run.json(&summary, state, Some(reason));
    let wrote = write_json(&json_path, &json);
    for line in summary.lines() {
        out(line);
    }
    out(format!(
        "duration: {:.1} s, cpu: {:.2} s, file: {} bytes",
        run.started.elapsed().as_secs_f64(),
        json["cpu_seconds"].as_f64().unwrap_or(0.0),
        json["file_bytes"].as_u64().unwrap_or(0)
    ));
    out(format!("stop reason: {reason}"));
    out(format!("capture: {}", args.file.display()));
    match (&error, wrote) {
        (Some(e), _) => {
            out(format!("error: {e}"));
            1
        }
        (None, Err(e)) => {
            out(format!("error: summary {}: {e}", json_path.display()));
            1
        }
        (None, Ok(())) => {
            out(format!("summary: {}", json_path.display()));
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_the_options() {
        let p = parse(&a(&[
            "f.a2raw",
            "--stop-file",
            "s",
            "--seconds",
            "5",
            "--shm-name",
            "n",
        ]))
        .unwrap();
        assert_eq!(p.file, PathBuf::from("f.a2raw"));
        assert_eq!(p.stop_file, Some(PathBuf::from("s")));
        assert_eq!((p.seconds, p.shm_name.as_deref()), (5.0, Some("n")));
        assert_eq!(parse(&a(&["f"])).unwrap().seconds, DEFAULT_SECONDS);
        assert_eq!(DEFAULT_SECONDS, 10_800.0);
        assert!(parse(&a(&[])).is_err());
        assert!(parse(&a(&["f", "--seconds", "x"])).is_err());
        assert!(parse(&a(&["f", "--seconds"])).is_err());
        assert!(parse(&a(&["f", "--bogus"])).is_err());
    }

    #[test]
    fn captures_a_test_mapping_until_the_stop_file_and_writes_the_summary() {
        use ams2_shm::capture::CaptureReader;
        use ams2_shm::testing::{TestMapping, unique_name};
        let dir = std::env::temp_dir().join(format!("ams2-capraw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (file, stop) = (dir.join("c.a2raw"), dir.join("c.stop"));
        let name = unique_name("capraw");
        let mut map = TestMapping::create(&name).unwrap();
        let map = std::thread::scope(|s| {
            let stop_path = stop.clone();
            let h = s.spawn(move || {
                let mut f = new_frame();
                f.car_name[..2].copy_from_slice(b"F1");
                f.session_state = 5;
                for _ in 0..60 {
                    map.write_frame(&f);
                    std::thread::sleep(Duration::from_millis(10));
                }
                std::fs::write(stop_path, b"").unwrap();
                map
            });
            let mut lines = Vec::new();
            let code = run(
                &a(&[
                    file.to_str().unwrap(),
                    "--stop-file",
                    stop.to_str().unwrap(),
                    "--shm-name",
                    &name,
                ]),
                &mut |l| lines.push(l),
            );
            assert_eq!(code, 0, "{lines:?}");
            assert!(
                lines.iter().any(|l| l == "stop reason: stop-file"),
                "{lines:?}"
            );
            h.join().unwrap()
        });
        drop(map);
        let json: Value =
            serde_json::from_slice(&std::fs::read(dir.join("c.a2raw.json")).unwrap()).unwrap();
        assert_eq!(json["state"], "finished");
        assert_eq!(json["stop_reason"], "stop-file");
        assert_eq!(json["cars"], json!(["F1"]));
        assert_eq!(json["session_types"], json!(["race"]));
        let frames = json["frames"].as_u64().unwrap();
        assert!(frames > 10, "{frames}");
        let mut r = CaptureReader::new(std::fs::File::open(&file).unwrap()).unwrap();
        let mut f = new_frame();
        let mut n = 0;
        while r.next_frame(&mut f).unwrap().is_some() {
            n += 1;
        }
        assert!(!r.truncated());
        assert_eq!(n, frames);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_mapping_is_waited_for_until_the_stop_file() {
        let dir = std::env::temp_dir().join(format!("ams2-capwait-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (file, stop) = (dir.join("w.a2raw"), dir.join("w.stop"));
        std::fs::write(&stop, b"").unwrap();
        let mut lines = Vec::new();
        let code = run(
            &a(&[
                file.to_str().unwrap(),
                "--stop-file",
                stop.to_str().unwrap(),
                "--shm-name",
                &ams2_shm::testing::unique_name("absent"),
            ]),
            &mut |l| lines.push(l),
        );
        assert_eq!(code, 0, "{lines:?}");
        let json: Value =
            serde_json::from_slice(&std::fs::read(dir.join("w.a2raw.json")).unwrap()).unwrap();
        assert_eq!(json["frames"], 0);
        assert_eq!(json["stop_reason"], "stop-file");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
