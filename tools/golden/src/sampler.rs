//! The contract's sampling: ticks every 20 ms of capture time from the first record, each
//! reading the newest record at or before the tick; a tick without a new record is skipped.

use ams2_shm::capture::CaptureReader;
use ams2_shm::{SharedMemory, ShmError, new_frame};
use std::io::Read;

/// Capture time between two ticks, in microseconds.
pub const TICK_US: u64 = 20_000;

/// Streams a capture holding two frames at a time.
pub struct Sampler<R: Read> {
    reader: CaptureReader<R>,
    cur: Box<SharedMemory>,
    next: Box<SharedMemory>,
    cur_ts: u64,
    next_ts: Option<u64>,
    t0: u64,
    k: u64,
    last: Option<u64>,
    /// Records read so far.
    pub records: u64,
    /// Ticks handed out so far.
    pub ticks: u64,
}

impl<R: Read> Sampler<R> {
    /// Opens the capture; `None` when it holds no record.
    pub fn new(input: R) -> Result<Option<Self>, ShmError> {
        let mut reader = CaptureReader::new(input)?;
        let mut cur = new_frame();
        let Some(t0) = reader.next_frame(&mut cur)? else {
            return Ok(None);
        };
        let mut next = new_frame();
        let next_ts = reader.next_frame(&mut next)?;
        Ok(Some(Self {
            reader,
            cur,
            next,
            cur_ts: t0,
            next_ts,
            t0,
            k: 0,
            last: None,
            records: 1 + u64::from(next_ts.is_some()),
            ticks: 0,
        }))
    }

    /// The first record's timestamp in microseconds.
    pub const fn t0_us(&self) -> u64 {
        self.t0
    }

    /// The frame of the last tick returned.
    pub fn frame(&self) -> &SharedMemory {
        &self.cur
    }

    /// The next processed tick: its time in microseconds since the first record, or `None`
    /// after the last record was processed.
    pub fn advance(&mut self) -> Result<Option<u64>, ShmError> {
        loop {
            let tick = self.t0 + self.k * TICK_US;
            self.k += 1;
            while let Some(nts) = self.next_ts {
                if nts > tick {
                    break;
                }
                std::mem::swap(&mut self.cur, &mut self.next);
                self.cur_ts = nts;
                self.next_ts = self.reader.next_frame(&mut self.next)?;
                self.records += u64::from(self.next_ts.is_some());
            }
            if self.last == Some(self.cur_ts) {
                if self.next_ts.is_none() {
                    return Ok(None);
                }
                continue;
            }
            self.last = Some(self.cur_ts);
            self.ticks += 1;
            return Ok(Some(tick - self.t0));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ams2_shm::capture::CaptureWriter;

    #[test]
    fn newest_record_per_tick_and_skipped_repeats() {
        let mut w = CaptureWriter::new(Vec::new()).unwrap();
        let f = new_frame();
        // records at 1.000 s, 1.010 s, 1.095 s: ticks at +0, +20, +40, +60, +80, +100 ms
        for ts in [1_000_000, 1_010_000, 1_095_000] {
            w.write_frame(ts, &f).unwrap();
        }
        let bytes = w.finish().unwrap();
        let mut s = Sampler::new(&bytes[..]).unwrap().unwrap();
        let mut got = Vec::new();
        while let Some(t) = s.advance().unwrap() {
            got.push(t);
        }
        // +0 -> record 1; +20 -> record 2; +40..+80 repeat record 2 (skipped); +100 -> record 3
        assert_eq!(got, [0, 20_000, 100_000]);
        assert_eq!(s.records, 3);
    }
}
