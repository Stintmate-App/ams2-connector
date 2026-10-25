//! Raw capture format: a sequence of snapshots with timestamps, for replay and test fixtures.
//!
//! ```text
//! header (24 bytes): magic "A2RAWCAP" | format u32 | shm version u32 | frame size u32 | reserved u32
//! record:            timestamp_us u64 (since the capture started) | frame bytes (FRAME_SIZE)
//! ```
//! All integers little-endian. A file ends after a whole record. A record cut short (the
//! capturing process crashed or was killed) is dropped by the reader, which reports it through
//! [`CaptureReader::truncated`]; the whole records before it are kept.

use crate::{
    FRAME_SIZE, FrameSource, SHARED_MEMORY_VERSION, SharedMemory, ShmError, SnapshotOutcome,
};
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use zerocopy::IntoBytes;

/// Magic bytes at the start of a capture.
pub const MAGIC: &[u8; 8] = b"A2RAWCAP";
/// Capture format version.
pub const FORMAT_VERSION: u32 = 1;
const HEADER_LEN: usize = 24;

/// Writes a capture.
pub struct CaptureWriter<W: Write> {
    out: W,
}

impl<W: Write> CaptureWriter<W> {
    /// Writes the header.
    pub fn new(mut out: W) -> Result<Self, ShmError> {
        let mut h = [0u8; HEADER_LEN];
        h[..8].copy_from_slice(MAGIC);
        h[8..12].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        h[12..16].copy_from_slice(&SHARED_MEMORY_VERSION.to_le_bytes());
        h[16..20].copy_from_slice(&(FRAME_SIZE as u32).to_le_bytes());
        out.write_all(&h)?;
        Ok(Self { out })
    }

    /// Appends one record.
    pub fn write_frame(&mut self, timestamp_us: u64, frame: &SharedMemory) -> Result<(), ShmError> {
        self.out.write_all(&timestamp_us.to_le_bytes())?;
        self.out.write_all(frame.as_bytes())?;
        Ok(())
    }

    /// Flushes and returns the inner writer.
    pub fn finish(mut self) -> Result<W, ShmError> {
        self.out.flush()?;
        Ok(self.out)
    }
}

/// Reads a capture.
pub struct CaptureReader<R: Read> {
    input: R,
    truncated: bool,
}

impl<R: Read> CaptureReader<R> {
    /// Reads and validates the header.
    pub fn new(mut input: R) -> Result<Self, ShmError> {
        let mut h = [0u8; HEADER_LEN];
        input
            .read_exact(&mut h)
            .map_err(|_| ShmError::Capture("truncated header".into()))?;
        if &h[..8] != MAGIC {
            return Err(ShmError::Capture("not a raw capture (bad magic)".into()));
        }
        let word = |i: usize| u32::from_le_bytes(h[i..i + 4].try_into().unwrap());
        if word(8) != FORMAT_VERSION {
            return Err(ShmError::Capture(format!("unsupported format {}", word(8))));
        }
        if word(12) != SHARED_MEMORY_VERSION || word(16) as usize != FRAME_SIZE {
            return Err(ShmError::Capture(format!(
                "shared memory version {} / frame size {} not supported",
                word(12),
                word(16)
            )));
        }
        Ok(Self {
            input,
            truncated: false,
        })
    }

    /// The file ended inside a record (seen once [`next_frame`](Self::next_frame) returned `None`).
    pub fn truncated(&self) -> bool {
        self.truncated
    }

    /// Reads the next record into `out`; `None` at the end of the file, clean or cut inside a
    /// record (then [`truncated`](Self::truncated) is true and the partial record is dropped).
    pub fn next_frame(&mut self, out: &mut SharedMemory) -> Result<Option<u64>, ShmError> {
        let mut ts = [0u8; 8];
        let mut got = 0;
        while got < ts.len() {
            let n = self.input.read(&mut ts[got..])?;
            if n == 0 {
                self.truncated = got > 0;
                return Ok(None);
            }
            got += n;
        }
        match self.input.read_exact(out.as_mut_bytes()) {
            Ok(()) => Ok(Some(u64::from_le_bytes(ts))),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                self.truncated = true;
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }
}

/// Options of [`record`].
#[derive(Debug, Clone)]
pub struct RecordOptions {
    /// Pause between polls.
    pub interval: Duration,
    /// Stop after this many frames written (`None`: until `stop` or the source ends).
    pub max_frames: Option<u64>,
}

/// The core of `capture-raw`: polls `source` and writes a record each time the sequence number
/// changes, until `stop` is set, `max_frames` is reached or the source ends. Returns the frames
/// written. `frame` is the preallocated work buffer.
pub fn record<S: FrameSource, W: Write>(
    source: &mut S,
    writer: &mut CaptureWriter<W>,
    frame: &mut SharedMemory,
    opts: &RecordOptions,
    stop: &AtomicBool,
) -> Result<u64, ShmError> {
    record_with(
        source,
        writer,
        frame,
        opts,
        |_, _| {},
        || stop.load(Ordering::Relaxed),
    )
    .map(|o| o.frames)
}

/// How [`record_with`] ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordOutcome {
    /// Frames written.
    pub frames: u64,
    /// The source ended (as opposed to `should_stop` or `max_frames`).
    pub source_ended: bool,
}

/// [`record`] with a per-frame observer (`timestamp_us`, frame; called after the write) and a
/// stop predicate polled on every pass of the loop (it must be cheap: see [`Stopper`], which
/// rate-limits its checks). Nothing is allocated per frame.
pub fn record_with<S: FrameSource, W: Write>(
    source: &mut S,
    writer: &mut CaptureWriter<W>,
    frame: &mut SharedMemory,
    opts: &RecordOptions,
    mut on_frame: impl FnMut(u64, &SharedMemory),
    mut should_stop: impl FnMut() -> bool,
) -> Result<RecordOutcome, ShmError> {
    let start = Instant::now();
    let mut last_seq = None;
    let mut written = 0u64;
    let mut source_ended = false;
    while !should_stop() && opts.max_frames.is_none_or(|m| written < m) {
        match source.snapshot(frame)? {
            SnapshotOutcome::Ended => {
                source_ended = true;
                break;
            }
            SnapshotOutcome::Frame(seq) if last_seq != Some(seq) => {
                last_seq = Some(seq);
                let us = start.elapsed().as_micros() as u64;
                writer.write_frame(us, frame)?;
                written += 1;
                on_frame(us, frame);
            }
            _ => {}
        }
        if !opts.interval.is_zero() {
            std::thread::sleep(opts.interval);
        }
    }
    Ok(RecordOutcome {
        frames: written,
        source_ended,
    })
}

/// Why a capture ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The stop file appeared.
    StopFile,
    /// Ctrl+C or Ctrl+Break.
    Interrupted,
    /// The game process exited.
    GameExited,
    /// The `--seconds` cap.
    TimeLimit,
    /// The source ended.
    SourceEnded,
}

impl StopReason {
    /// The name written in the summary.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::StopFile => "stop-file",
            Self::Interrupted => "interrupted",
            Self::GameExited => "game-exited",
            Self::TimeLimit => "time-limit",
            Self::SourceEnded => "source-ended",
        }
    }
}

/// The stop conditions of a capture. The stop file, the game probe and the cap are checked at
/// most every `interval` (a filesystem call, a handle wait); the interrupt flag on every call.
pub struct Stopper<'a> {
    stop_file: Option<PathBuf>,
    deadline: Instant,
    interrupt: Option<&'a AtomicBool>,
    game_exited: Option<Box<dyn FnMut() -> bool + 'a>>,
    interval: Duration,
    next_check: Instant,
    reason: Option<StopReason>,
}

impl<'a> Stopper<'a> {
    /// Stops `limit` from now. Checks every 0.5 s.
    pub fn new(limit: Duration) -> Self {
        let now = Instant::now();
        Self {
            stop_file: None,
            deadline: now + limit,
            interrupt: None,
            game_exited: None,
            interval: Duration::from_millis(500),
            next_check: now,
            reason: None,
        }
    }

    /// Stops when this file exists.
    pub fn stop_file(mut self, path: impl Into<PathBuf>) -> Self {
        self.stop_file = Some(path.into());
        self
    }

    /// Stops when this flag is set (Ctrl+C).
    pub fn interrupt(mut self, flag: &'a AtomicBool) -> Self {
        self.interrupt = Some(flag);
        self
    }

    /// Stops when `f` returns true (the game process has exited).
    pub fn game_exited(mut self, f: impl FnMut() -> bool + 'a) -> Self {
        self.game_exited = Some(Box::new(f));
        self
    }

    /// Changes the check interval (tests).
    pub fn interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// The reason, once a condition held (sticky). Cheap between checks.
    pub fn poll(&mut self) -> Option<StopReason> {
        if self.reason.is_some() {
            return self.reason;
        }
        if self.interrupt.is_some_and(|f| f.load(Ordering::Relaxed)) {
            self.reason = Some(StopReason::Interrupted);
            return self.reason;
        }
        let now = Instant::now();
        if now < self.next_check {
            return None;
        }
        self.next_check = now + self.interval;
        self.reason = if self.stop_file.as_deref().is_some_and(|p| p.exists()) {
            Some(StopReason::StopFile)
        } else if self.game_exited.as_mut().is_some_and(|f| f()) {
            Some(StopReason::GameExited)
        } else if now >= self.deadline {
            Some(StopReason::TimeLimit)
        } else {
            None
        };
        self.reason
    }
}

/// Session types of `mSessionState`, by name.
pub fn session_name(state: u32) -> String {
    match state {
        0 => "invalid".into(),
        1 => "practice".into(),
        2 => "test".into(),
        3 => "qualify".into(),
        4 => "formation-lap".into(),
        5 => "race".into(),
        6 => "time-attack".into(),
        n => format!("unknown({n})"),
    }
}

/// Distinct values kept per list of the summary (a bound nobody reaches).
const MAX_SEEN: usize = 64;

fn text(raw: &[u8]) -> String {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

/// What a capture contains, built frame by frame without allocating per frame (strings are
/// decoded only when the game's value changes). Holds nothing but what the game frames hold.
#[derive(Debug, Default, Clone)]
pub struct CaptureSummary {
    frames: u64,
    first_us: Option<u64>,
    last_us: u64,
    cars: BTreeSet<String>,
    tracks: BTreeSet<String>,
    sessions: BTreeSet<u32>,
    game_states: BTreeSet<u32>,
    last_car: Option<[u8; 64]>,
    last_track: Option<([u8; 64], [u8; 64])>,
    last_session: Option<u32>,
    last_game: Option<u32>,
}

impl CaptureSummary {
    /// Counts one frame.
    pub fn observe(&mut self, timestamp_us: u64, f: &SharedMemory) {
        self.frames += 1;
        self.first_us.get_or_insert(timestamp_us);
        self.last_us = timestamp_us;
        if self.last_car != Some(f.car_name) {
            self.last_car = Some(f.car_name);
            if self.cars.len() < MAX_SEEN {
                self.cars.insert(text(&f.car_name));
            }
        }
        let track = (f.track_location, f.track_variation);
        if self.last_track != Some(track) {
            self.last_track = Some(track);
            if self.tracks.len() < MAX_SEEN {
                self.tracks.insert(format!(
                    "{} / {}",
                    text(&f.track_location),
                    text(&f.track_variation)
                ));
            }
        }
        if self.last_session != Some(f.session_state) {
            self.last_session = Some(f.session_state);
            self.sessions.insert(f.session_state);
        }
        if self.last_game != Some(f.game_state) {
            self.last_game = Some(f.game_state);
            self.game_states.insert(f.game_state);
        }
    }

    /// Frames counted.
    pub const fn frames(&self) -> u64 {
        self.frames
    }

    /// The summary as JSON; the members of `extra` (an object) are merged in.
    pub fn to_json(&self, extra: serde_json::Value) -> serde_json::Value {
        let mut v = serde_json::json!({
            "frames": self.frames,
            "first_timestamp_us": self.first_us,
            "last_timestamp_us": self.first_us.map(|_| self.last_us),
            "capture_seconds": self.first_us.map_or(0.0, |f| (self.last_us - f) as f64 / 1e6),
            "cars": self.cars,
            "tracks": self.tracks,
            "session_types": self.sessions.iter().map(|&s| session_name(s)).collect::<Vec<_>>(),
            "game_states": self.game_states,
        });
        if let (Some(a), serde_json::Value::Object(b)) = (v.as_object_mut(), extra) {
            a.extend(b);
        }
        v
    }

    /// The summary as lines for stdout.
    pub fn lines(&self) -> Vec<String> {
        let join = |s: &BTreeSet<String>| s.iter().cloned().collect::<Vec<_>>().join("; ");
        vec![
            format!("frames: {}", self.frames),
            format!(
                "timestamps: first {} us, last {} us",
                self.first_us.unwrap_or(0),
                self.last_us
            ),
            format!("cars: {}", join(&self.cars)),
            format!("tracks: {}", join(&self.tracks)),
            format!(
                "session types: {}",
                self.sessions
                    .iter()
                    .map(|&s| session_name(s))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MemorySource, new_frame};

    fn frames(n: u32) -> Vec<Box<SharedMemory>> {
        (0..n)
            .map(|i| {
                let mut f = new_frame();
                f.sequence_number = 2 * (i + 1);
                f.speed = i as f32;
                f.car_name[..3].copy_from_slice(b"abc");
                f
            })
            .collect()
    }

    #[test]
    fn round_trip() {
        let mut w = CaptureWriter::new(Vec::new()).unwrap();
        let fs = frames(3);
        for (i, f) in fs.iter().enumerate() {
            w.write_frame(1000 * i as u64, f).unwrap();
        }
        let bytes = w.finish().unwrap();
        assert_eq!(bytes.len(), HEADER_LEN + 3 * (8 + FRAME_SIZE));
        let mut r = CaptureReader::new(bytes.as_slice()).unwrap();
        let mut out = new_frame();
        for (i, f) in fs.iter().enumerate() {
            assert_eq!(r.next_frame(&mut out).unwrap(), Some(1000 * i as u64));
            assert_eq!(out.as_bytes(), f.as_bytes());
        }
        assert_eq!(r.next_frame(&mut out).unwrap(), None);
    }

    #[test]
    fn rejects_bad_files() {
        assert!(CaptureReader::new(&b"short"[..]).is_err());
        let mut bad = vec![0u8; HEADER_LEN];
        assert!(CaptureReader::new(bad.as_slice()).is_err());
        let w = CaptureWriter::new(Vec::new()).unwrap();
        bad = w.finish().unwrap();
        bad.extend_from_slice(&[0u8; 100]);
        assert!(CaptureReader::new(&bad[..HEADER_LEN - 1]).is_err());
    }

    #[test]
    fn truncated_last_record_keeps_the_whole_ones() {
        let fs = frames(3);
        let mut w = CaptureWriter::new(Vec::new()).unwrap();
        for (i, f) in fs.iter().enumerate() {
            w.write_frame(i as u64, f).unwrap();
        }
        let bytes = w.finish().unwrap();
        let third = HEADER_LEN + 2 * (8 + FRAME_SIZE);
        // Cut inside the third record's frame, inside its timestamp, and after its timestamp.
        for cut in [bytes.len() - 5, third + 3, third + 8] {
            let mut r = CaptureReader::new(&bytes[..cut]).unwrap();
            let mut out = new_frame();
            assert_eq!(r.next_frame(&mut out).unwrap(), Some(0));
            assert_eq!(r.next_frame(&mut out).unwrap(), Some(1));
            assert!(!r.truncated());
            assert_eq!(r.next_frame(&mut out).unwrap(), None);
            assert!(r.truncated(), "cut at {cut}");
        }
        let mut r = CaptureReader::new(&bytes[..third]).unwrap();
        let mut out = new_frame();
        while r.next_frame(&mut out).unwrap().is_some() {}
        assert!(!r.truncated(), "a cut between records is clean");
    }

    #[test]
    fn stopper_sees_the_stop_file_the_flag_the_game_and_the_cap() {
        let dir = std::env::temp_dir().join(format!("ams2-stopper-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("stop");
        let _ = std::fs::remove_file(&file);
        let mut s = Stopper::new(Duration::from_secs(3600))
            .stop_file(&file)
            .interval(Duration::ZERO);
        assert_eq!(s.poll(), None);
        std::fs::write(&file, b"").unwrap();
        assert_eq!(s.poll(), Some(StopReason::StopFile));
        std::fs::remove_file(&file).unwrap();
        assert_eq!(s.poll(), Some(StopReason::StopFile), "sticky");
        let _ = std::fs::remove_dir_all(&dir);

        let flag = AtomicBool::new(false);
        let mut s = Stopper::new(Duration::from_secs(3600)).interrupt(&flag);
        assert_eq!(s.poll(), None);
        flag.store(true, Ordering::Relaxed);
        assert_eq!(s.poll(), Some(StopReason::Interrupted));

        let mut s = Stopper::new(Duration::ZERO).interval(Duration::ZERO);
        assert_eq!(s.poll(), Some(StopReason::TimeLimit));

        let mut n = 0;
        let mut s = Stopper::new(Duration::from_secs(3600))
            .game_exited(|| {
                n += 1;
                n > 2
            })
            .interval(Duration::ZERO);
        assert_eq!(s.poll(), None);
        assert_eq!(s.poll(), None);
        assert_eq!(s.poll(), Some(StopReason::GameExited));
    }

    #[test]
    fn stopper_rate_limits_its_checks() {
        let mut calls = 0;
        {
            let mut s = Stopper::new(Duration::from_secs(3600)).game_exited(|| {
                calls += 1;
                false
            });
            for _ in 0..1000 {
                s.poll();
            }
        }
        assert_eq!(calls, 1);
    }

    #[test]
    fn summary_lists_what_the_frames_hold() {
        let mut s = CaptureSummary::default();
        let mut f = new_frame();
        f.car_name[..3].copy_from_slice(b"GT3");
        f.track_location[..5].copy_from_slice(b"Imola");
        f.track_variation[..3].copy_from_slice(b"GP1");
        f.session_state = 1;
        s.observe(100, &f);
        s.observe(200, &f);
        f.session_state = 5;
        f.game_state = 2;
        s.observe(900, &f);
        let j = s.to_json(serde_json::json!({"stop_reason": "stop-file"}));
        assert_eq!(j["frames"], 3);
        assert_eq!(j["first_timestamp_us"], 100);
        assert_eq!(j["last_timestamp_us"], 900);
        assert_eq!(j["cars"], serde_json::json!(["GT3"]));
        assert_eq!(j["tracks"], serde_json::json!(["Imola / GP1"]));
        assert_eq!(j["session_types"], serde_json::json!(["practice", "race"]));
        assert_eq!(j["game_states"], serde_json::json!([0, 2]));
        assert_eq!(j["stop_reason"], "stop-file");
        assert!(s.lines().iter().any(|l| l == "frames: 3"));
        let empty = CaptureSummary::default().to_json(serde_json::json!({}));
        assert_eq!(empty["frames"], 0);
        assert!(empty["first_timestamp_us"].is_null());
    }

    #[test]
    fn record_with_observes_and_stops() {
        let mut src = MemorySource::new(frames(5));
        let mut w = CaptureWriter::new(Vec::new()).unwrap();
        let opts = RecordOptions {
            interval: Duration::ZERO,
            max_frames: None,
        };
        let mut seen = 0;
        let mut polls = 0;
        let o = record_with(
            &mut src,
            &mut w,
            &mut new_frame(),
            &opts,
            |_, _| seen += 1,
            || {
                polls += 1;
                polls > 3
            },
        )
        .unwrap();
        assert_eq!((o.frames, seen, o.source_ended), (3, 3, false));
    }

    #[test]
    fn record_writes_each_new_sequence_once() {
        let mut fs = frames(3);
        fs.insert(1, fs[0].clone()); // repeated sequence: skipped
        let mut src = MemorySource::new(fs);
        let mut w = CaptureWriter::new(Vec::new()).unwrap();
        let opts = RecordOptions {
            interval: Duration::ZERO,
            max_frames: None,
        };
        let n = record(
            &mut src,
            &mut w,
            &mut new_frame(),
            &opts,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(n, 3);
    }
}
