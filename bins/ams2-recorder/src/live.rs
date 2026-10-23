//! The live frame: a double buffer the loop publishes into and `connector.live_frame` reads
//! (plan 1.3). Two preallocated snapshots and an atomic index; the loop never takes a lock and
//! never allocates. Each slot is a seqlock (odd while it is written), so a reader that is slow
//! enough to be lapped by two publishes simply copies again.

use crate::link::to_value;
use ams2_core::codes::game_state;
use ams2_core::frame::{FrameOptions, live_frame};
use ams2_core::tracker::LiveLap;
use ams2_platform::logging::local_iso;
use ams2_shm::{SHARED_MEMORY_VERSION, SharedMemory, new_frame};
use std::cell::UnsafeCell;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering, fence};
use std::time::{Duration, Instant};
use stintmate_ipc::codec::Value;
use stintmate_ipc::error::RemoteError;

/// A game that plays but whose frames stop this long is "frozen" (or gone).
pub const FROZEN: Duration = Duration::from_secs(1);
const NONE: u8 = 2;
const TRIES: usize = 200;

/// What the loop knows about a snapshot besides the snapshot.
#[derive(Clone, Copy)]
pub struct Meta {
    /// The tracker's garage rule.
    pub in_garage: bool,
    /// The tracker's open lap.
    pub lap: Option<LiveLap>,
    /// When the game last changed the data.
    pub last_change: Instant,
}

struct Slot {
    seq: AtomicU32,
    meta: UnsafeCell<MaybeUninit<Meta>>,
    frame: UnsafeCell<Box<SharedMemory>>,
}

/// The double buffer: one writer (the loop), any number of readers.
pub struct LiveBuffer {
    slots: [Slot; 2],
    index: AtomicU8,
    bad_version: AtomicU32,
}

// SAFETY: the single writer and the readers synchronise through the slot's sequence number and
// the index (a seqlock); a reader keeps a copy only when the sequence did not change.
unsafe impl Sync for LiveBuffer {}
unsafe impl Send for LiveBuffer {}

impl Default for LiveBuffer {
    fn default() -> Self {
        let slot = || Slot {
            seq: AtomicU32::new(0),
            meta: UnsafeCell::new(MaybeUninit::uninit()),
            frame: UnsafeCell::new(new_frame()),
        };
        Self {
            slots: [slot(), slot()],
            index: AtomicU8::new(NONE),
            bad_version: AtomicU32::new(0),
        }
    }
}

impl LiveBuffer {
    /// Publishes one snapshot (the loop only: a single writer). No lock, no allocation.
    pub fn publish(&self, frame: &SharedMemory, meta: Meta) {
        let cur = self.index.load(Ordering::Relaxed);
        let w = if cur == 0 { 1 } else { 0 };
        let slot = &self.slots[usize::from(w)];
        slot.seq.fetch_add(1, Ordering::Release); // odd: being written
        fence(Ordering::Release);
        // SAFETY: only the writer writes to a slot, and it only writes the one the readers are
        // not directed to; a reader that still reads it discards its copy (odd/changed sequence).
        unsafe {
            core::ptr::copy_nonoverlapping(
                frame as *const SharedMemory,
                (*slot.frame.get()).as_mut() as *mut SharedMemory,
                1,
            );
            (*slot.meta.get()).write(meta);
        }
        slot.seq.fetch_add(1, Ordering::Release); // even: complete
        self.bad_version.store(0, Ordering::Relaxed);
        self.index.store(w, Ordering::Release);
    }

    /// Nothing to show any more: the game is gone or its mapping was closed.
    pub fn clear(&self) {
        self.index.store(NONE, Ordering::Release);
    }

    /// The game runs a shared memory version this connector does not read.
    pub fn unsupported(&self, version: u32) {
        self.index.store(NONE, Ordering::Release);
        self.bad_version.store(version, Ordering::Release);
    }

    /// The unsupported version in force, if any.
    pub fn bad_version(&self) -> Option<u32> {
        Some(self.bad_version.load(Ordering::Acquire)).filter(|v| *v != 0)
    }

    /// A consistent copy of the last published snapshot into `out`, with its meta; `None`
    /// before the first publish or after [`clear`](Self::clear).
    pub fn read(&self, out: &mut SharedMemory) -> Option<Meta> {
        for _ in 0..TRIES {
            let i = self.index.load(Ordering::Acquire);
            if i == NONE {
                return None;
            }
            let slot = &self.slots[usize::from(i)];
            let before = slot.seq.load(Ordering::Acquire);
            if before & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let mut meta = MaybeUninit::<Meta>::uninit();
            // SAFETY: the copy may race with the writer lapping this slot; it is used only if
            // the sequence is unchanged afterwards (so the bytes are a complete publish).
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (*slot.frame.get()).as_ref() as *const SharedMemory,
                    out as *mut SharedMemory,
                    1,
                );
                core::ptr::copy_nonoverlapping(
                    slot.meta.get() as *const Meta,
                    meta.as_mut_ptr(),
                    1,
                );
            }
            fence(Ordering::Acquire);
            if slot.seq.load(Ordering::Relaxed) == before {
                // SAFETY: a complete, unchanged publish wrote the meta.
                return Some(unsafe { meta.assume_init() });
            }
        }
        None
    }
}

/// What the telemetry thread needs from the loop, all atomics.
#[derive(Default)]
pub struct Hints {
    /// The game's process runs and its mapping is open.
    pub game_running: AtomicBool,
    /// +1 at every (re)mapping.
    pub generation: AtomicU64,
    /// The tracker's garage rule.
    pub in_garage: AtomicBool,
    /// The open lap is not an out lap.
    pub flying: AtomicBool,
}

impl Hints {
    /// (running, generation).
    pub fn game(&self) -> (bool, u64) {
        (
            self.game_running.load(Ordering::Relaxed),
            self.generation.load(Ordering::Relaxed),
        )
    }
    /// (in the garage, the lap is not an out lap).
    pub fn lap_state(&self) -> (bool, bool) {
        (
            self.in_garage.load(Ordering::Relaxed),
            self.flying.load(Ordering::Relaxed),
        )
    }
}

/// Answers `connector.live_frame` from the buffer (the connection's reader thread: memory only,
/// except the process check when the data froze).
pub struct LiveServer {
    /// The buffer.
    pub buffer: Arc<LiveBuffer>,
    /// "Does the game's process exist?"
    pub game_running: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The clock of ages.
    pub now: Arc<dyn Fn() -> Instant + Send + Sync>,
}

impl LiveServer {
    /// The neutral live frame (contract 5.1), or the protocol's errors.
    pub fn answer(&self) -> Result<Value, RemoteError> {
        let not_running = || {
            RemoteError::new(
                "game_not_running",
                format!("{} is not running.", ams2_core::SIM_NAME),
            )
        };
        if let Some(v) = self.buffer.bad_version() {
            return Err(RemoteError::new(
                "internal",
                format!(
                    "Unsupported shared memory version {v} (expected {SHARED_MEMORY_VERSION})."
                ),
            ));
        }
        let mut d = new_frame();
        let meta = self.buffer.read(&mut d).ok_or_else(not_running)?;
        let age = (self.now)().saturating_duration_since(meta.last_change);
        if d.game_state == game_state::INGAME_PLAYING && age > FROZEN {
            if !(self.game_running)() {
                return Err(not_running());
            }
            return Err(RemoteError::new(
                "game_not_running",
                format!(
                    "{}'s shared memory is not being updated (the game is frozen).",
                    ams2_core::SIM_NAME
                ),
            ));
        }
        let taken = local_iso(true);
        let opts = FrameOptions {
            taken_at: Some(&taken),
            data_age_s: Some(age.as_secs_f64()),
            lap_info: meta.lap.as_ref(),
            in_garage: meta.in_garage,
        };
        Ok(to_value(&live_frame(&d, &opts)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn stamp(f: &mut SharedMemory, n: u32) {
        f.sequence_number = n;
        f.race_state = n;
        f.speed = n as f32;
        f.car_name[..4].copy_from_slice(&n.to_le_bytes());
        f.participant_info[63].current_lap = n;
    }

    fn meta(n: u32, base: Instant) -> Meta {
        Meta {
            in_garage: n.is_multiple_of(2),
            lap: None,
            last_change: base + Duration::from_millis(u64::from(n)),
        }
    }

    #[test]
    fn nothing_before_the_first_publish_and_after_clear() {
        let b = LiveBuffer::default();
        let mut out = new_frame();
        assert!(b.read(&mut out).is_none());
        let mut f = new_frame();
        stamp(&mut f, 7);
        b.publish(&f, meta(7, Instant::now()));
        assert!(!b.read(&mut out).unwrap().in_garage);
        assert_eq!(out.sequence_number, 7);
        b.clear();
        assert!(b.read(&mut out).is_none());
        b.unsupported(13);
        assert_eq!(b.bad_version(), Some(13));
        b.publish(&f, meta(8, Instant::now()));
        assert_eq!(b.bad_version(), None);
    }

    /// A reader never sees a half-written snapshot, whatever the writer does meanwhile.
    #[test]
    fn readers_never_see_a_torn_snapshot() {
        let b = Arc::new(LiveBuffer::default());
        let stop = Arc::new(AtomicBool::new(false));
        let base = Instant::now();
        let w = {
            let (b, stop) = (b.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut f = new_frame();
                let mut n = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    n += 2;
                    stamp(&mut f, n);
                    b.publish(&f, meta(n, base));
                }
                n
            })
        };
        let readers: Vec<_> = (0..3)
            .map(|_| {
                let (b, stop) = (b.clone(), stop.clone());
                std::thread::spawn(move || {
                    let mut out = new_frame();
                    let (mut reads, mut last) = (0u32, 0u32);
                    while !stop.load(Ordering::Relaxed) {
                        let Some(m) = b.read(&mut out) else { continue };
                        let n = out.sequence_number;
                        assert_eq!(out.race_state, n);
                        assert_eq!(out.speed, n as f32);
                        assert_eq!(out.car_name[..4], n.to_le_bytes());
                        assert_eq!(out.participant_info[63].current_lap, n);
                        assert_eq!(m.last_change, base + Duration::from_millis(u64::from(n)));
                        assert!(n >= last, "time does not go back");
                        (reads, last) = (reads + 1, n);
                    }
                    reads
                })
            })
            .collect();
        std::thread::sleep(Duration::from_millis(700));
        stop.store(true, Ordering::Relaxed);
        let published = w.join().unwrap() / 2;
        let reads: u32 = readers.into_iter().map(|r| r.join().unwrap()).sum();
        assert!(
            published > 1000 && reads > 100,
            "{published} publishes, {reads} reads"
        );
    }
}
