//! The [`FrameSource`] trait and the in-memory source.

use crate::{SharedMemory, ShmError};

/// Result of one snapshot attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotOutcome {
    /// A consistent frame was copied; carries its sequence number.
    Frame(u32),
    /// No consistent copy within the retry bound: skip this tick.
    Torn,
    /// The source has no more frames (in-memory sources only).
    Ended,
}

/// Where the recorder loop reads frames from.
pub trait FrameSource {
    /// Copies the current frame into `out` without allocating.
    fn snapshot(&mut self, out: &mut SharedMemory) -> Result<SnapshotOutcome, ShmError>;
}

/// A source that plays a fixed list of frames, one per call, then reports [`SnapshotOutcome::Ended`].
pub struct MemorySource {
    frames: std::vec::IntoIter<Box<SharedMemory>>,
}

impl MemorySource {
    /// Builds the source; each frame's own `sequence_number` is reported.
    pub fn new(frames: Vec<Box<SharedMemory>>) -> Self {
        Self {
            frames: frames.into_iter(),
        }
    }
}

impl FrameSource for MemorySource {
    fn snapshot(&mut self, out: &mut SharedMemory) -> Result<SnapshotOutcome, ShmError> {
        Ok(match self.frames.next() {
            Some(f) => {
                *out = *f;
                SnapshotOutcome::Frame(out.sequence_number)
            }
            None => SnapshotOutcome::Ended,
        })
    }
}
