//! Raw capture format: a sequence of snapshots with timestamps, for replay and test fixtures.
//!
//! ```text
//! header (24 bytes): magic "A2RAWCAP" | format u32 | shm version u32 | frame size u32 | reserved u32
//! record:            timestamp_us u64 (since the capture started) | frame bytes (FRAME_SIZE)
//! ```
//! All integers little-endian. A file ends after a whole record; a partial record is an error.

use crate::{
    FRAME_SIZE, FrameSource, SHARED_MEMORY_VERSION, SharedMemory, ShmError, SnapshotOutcome,
};
use std::io::{Read, Write};
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
        Ok(Self { input })
    }

    /// Reads the next record into `out`; `None` at a clean end of file.
    pub fn next_frame(&mut self, out: &mut SharedMemory) -> Result<Option<u64>, ShmError> {
        let mut ts = [0u8; 8];
        let mut got = 0;
        while got < ts.len() {
            let n = self.input.read(&mut ts[got..])?;
            if n == 0 {
                return if got == 0 {
                    Ok(None)
                } else {
                    Err(ShmError::Capture("truncated record".into()))
                };
            }
            got += n;
        }
        self.input
            .read_exact(out.as_mut_bytes())
            .map_err(|_| ShmError::Capture("truncated record".into()))?;
        Ok(Some(u64::from_le_bytes(ts)))
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
    let start = Instant::now();
    let mut last_seq = None;
    let mut written = 0u64;
    while !stop.load(Ordering::Relaxed) && opts.max_frames.is_none_or(|m| written < m) {
        match source.snapshot(frame)? {
            SnapshotOutcome::Ended => break,
            SnapshotOutcome::Frame(seq) if last_seq != Some(seq) => {
                last_seq = Some(seq);
                writer.write_frame(start.elapsed().as_micros() as u64, frame)?;
                written += 1;
            }
            _ => {}
        }
        if !opts.interval.is_zero() {
            std::thread::sleep(opts.interval);
        }
    }
    Ok(written)
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
        bad.extend_from_slice(&[0u8; 100]); // partial record
        let mut r = CaptureReader::new(bad.as_slice()).unwrap();
        assert!(r.next_frame(&mut new_frame()).is_err());
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
