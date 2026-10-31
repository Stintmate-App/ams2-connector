//! `export`: a capture through the tracker, the detector and the live frame, as JSON Lines.

use crate::sampler::Sampler;
use ams2_core::announce::detector::{Announcement, EventDetector};
use ams2_core::frame::{FrameOptions, live_frame};
use ams2_core::golden::call_json;
use ams2_core::testing::{Call, FakeClock, RecordingSink};
use ams2_core::tracker::SessionTracker;
use serde_json::{Value, json};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The live frame goes out at the first processed tick with index (capture time / 20 ms) at least
/// a multiple of this (10 s), counting from the first multiple.
pub const LIVE_EVERY: u64 = 500;

/// One lap as the summary shows it.
pub struct LapLine {
    /// Lap number.
    pub lap: u32,
    /// Lap time, when it has one.
    pub time_s: Option<f64>,
    /// Valid flag.
    pub valid: bool,
}

/// What an export did.
#[derive(Default)]
pub struct Summary {
    /// Records read.
    pub records: u64,
    /// Ticks processed.
    pub ticks: u64,
    /// Lines per kind: call, announce, live.
    pub lines: [u64; 3],
    /// Laps of each session, in order.
    pub sessions: Vec<Vec<LapLine>>,
    /// Seconds taken.
    pub seconds: f64,
}

/// The announcement line (`data` is null when the detector gave none; `variant` only when not 0,
/// so the goldens taken before the variant field was dropped still compare).
fn announce_json(t_ms: u64, a: &Announcement) -> Value {
    let mut v = json!({"t_ms": t_ms, "kind": "announce", "code": a.code, "params": a.params,
           "data": a.data});
    if a.variant != 0 {
        v["variant"] = json!(a.variant);
    }
    v
}

fn put(out: &mut impl Write, v: &Value) -> std::io::Result<()> {
    serde_json::to_writer(&mut *out, v)?;
    out.write_all(b"\n")
}

/// Streams `capture` into `out`.
pub fn run(capture: &Path, out: &mut impl Write) -> Result<Summary, String> {
    let started = Instant::now();
    let file = std::fs::File::open(capture).map_err(|e| format!("{}: {e}", capture.display()))?;
    let io = |e: std::io::Error| format!("write: {e}");
    let mut s = Summary::default();
    let Some(mut sampler) =
        Sampler::new(BufReader::with_capacity(1 << 20, file)).map_err(|e| e.to_string())?
    else {
        return Err("the capture holds no record".into());
    };
    let clock = FakeClock::new();
    let mut tracker = SessionTracker::new(Box::new(clock.clone()));
    let mut detector = EventDetector::new(clock.clone(), 2.0);
    let mut sink = RecordingSink::new();
    let mut last_ms = 0;
    let mut next_live = LIVE_EVERY;
    let t0_us = sampler.t0_us();
    while let Some(rel_us) = sampler.advance().map_err(|e| e.to_string())? {
        let t_ms = rel_us / 1000;
        last_ms = t_ms;
        clock.set((t0_us + rel_us) as f64 / 1e6);
        let frame = sampler.frame();
        tracker.process(frame, &mut sink);
        flush_calls(&mut sink, t_ms, out, &mut s).map_err(io)?;
        for a in detector.update(frame) {
            put(out, &announce_json(t_ms, &a)).map_err(io)?;
            s.lines[1] += 1;
        }
        let k = rel_us / crate::sampler::TICK_US;
        if k >= next_live {
            next_live = (k / LIVE_EVERY + 1) * LIVE_EVERY;
            let lap = tracker.live_lap();
            let opts = FrameOptions {
                taken_at: Some("ISO"),
                data_age_s: Some(0.0),
                lap_info: lap.as_ref(),
                in_garage: tracker.in_garage(),
            };
            let line = json!({"t_ms": t_ms, "kind": "live", "frame": live_frame(frame, &opts)});
            put(out, &line).map_err(io)?;
            s.lines[2] += 1;
        }
    }
    tracker.end_session(&mut sink);
    flush_calls(&mut sink, last_ms, out, &mut s).map_err(io)?;
    out.flush().map_err(io)?;
    s.records = sampler.records;
    s.ticks = sampler.ticks;
    s.seconds = started.elapsed().as_secs_f64();
    Ok(s)
}

fn flush_calls(
    sink: &mut RecordingSink,
    t_ms: u64,
    out: &mut impl Write,
    s: &mut Summary,
) -> std::io::Result<()> {
    for c in std::mem::take(&mut sink.calls) {
        let mut v = call_json(&c);
        match &c {
            Call::Start(_) => s.sessions.push(Vec::new()),
            Call::Lap { record, .. } => {
                if let Some(cur) = s.sessions.last_mut() {
                    cur.push(LapLine {
                        lap: record.lap_number,
                        time_s: record.lap_time_s,
                        valid: record.valid,
                    });
                }
            }
            _ => {}
        }
        let o = v.as_object_mut().expect("a call is an object");
        o.insert("t_ms".into(), json!(t_ms));
        o.insert("kind".into(), json!("call"));
        put(out, &v)?;
        s.lines[0] += 1;
    }
    Ok(())
}

/// The output file for a capture: `<dir>\<stem>.rust.jsonl`, `dir` defaulting to the dev
/// captures' `golden` folder.
pub fn default_out(capture: &Path, dir: Option<PathBuf>) -> PathBuf {
    let dir = dir.unwrap_or_else(|| {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("Stintmate")
            .join("dev-captures")
            .join("golden")
    });
    let stem = capture
        .file_stem()
        .map_or_else(|| "capture".into(), |s| s.to_string_lossy().into_owned());
    dir.join(format!("{stem}.rust.jsonl"))
}

/// Opens the output file (creating its folder) as a buffered writer.
pub fn create(path: &Path) -> Result<BufWriter<std::fs::File>, String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::File::create(path)
        .map(|f| BufWriter::with_capacity(1 << 20, f))
        .map_err(|e| format!("{}: {e}", path.display()))
}
