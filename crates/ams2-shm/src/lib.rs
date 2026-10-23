//! Automobilista 2 shared memory (version 14): the layout, the mapping and consistent snapshots.
//!
//! * [`SharedMemory`]: the `#[repr(C)]` port of `SharedMemory.h` v14 (generated from the Python
//!   layout, checked at compile time and against `fixtures/offsets_v14.json`).
//! * [`Mapping`]: the read-only view of the game's mapping, with the seqlock snapshot.
//! * [`FrameSource`]: what the recorder loop reads through ([`Mapping`] or [`MemorySource`]).
//! * [`testing::TestMapping`]: writes frames into a mapping under a test name, with the game's
//!   sequence protocol.
//! * [`capture`]: the raw capture file format.

pub mod capture;
mod layout;
mod mapping;
mod source;
pub mod testing;

pub use layout::{FIELD_TABLE, ParticipantInfo, SharedMemory};
pub use mapping::Mapping;
pub use source::{FrameSource, MemorySource, SnapshotOutcome};

/// Shared memory version this crate understands.
pub const SHARED_MEMORY_VERSION: u32 = 14;
/// Name of the game's mapping. Tests never open it: they use [`testing::TestMapping`] names.
pub const MAPPING_NAME: &str = "$pcars2$";
/// Size in bytes of the v14 struct.
pub const FRAME_SIZE: usize = size_of::<SharedMemory>();
/// Byte offset of `mSequenceNumber` (odd while the game is writing).
pub const SEQUENCE_OFFSET: usize = core::mem::offset_of!(SharedMemory, sequence_number);
/// Default bound of seqlock retries per snapshot (as the Python reader).
pub const MAX_RETRIES: u32 = 10;

/// Errors of the shared memory layer.
#[derive(Debug, thiserror::Error)]
pub enum ShmError {
    /// The mapping does not exist (the game is not running or shared memory is off).
    #[error("mapping {0:?} not found (is AMS2 running with Shared Memory = Project CARS 2?)")]
    NotFound(String),
    /// A Windows call failed.
    #[error("{call} failed with Win32 error {code}")]
    Os {
        /// The failing call.
        call: &'static str,
        /// `GetLastError` value.
        code: u32,
    },
    /// The mapping is smaller than the v14 struct.
    #[error("mapping is {size} bytes, smaller than the expected {expected}; layout mismatch")]
    TooSmall {
        /// Size found.
        size: usize,
        /// Size expected.
        expected: usize,
    },
    /// Refused name (the game's real mapping for writers).
    #[error("refused mapping name: {0}")]
    RefusedName(String),
    /// Capture file problem.
    #[error("capture file: {0}")]
    Capture(String),
    /// I/O error.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Allocates a zeroed frame on the heap (the 20 kB struct never lives on the stack).
pub fn new_frame() -> Box<SharedMemory> {
    use zerocopy::FromZeros;
    SharedMemory::new_box_zeroed().expect("allocation of one frame")
}

/// Copies `FRAME_SIZE` bytes from `src` into `dst` with the seqlock rule (as the Python reader):
/// the sequence number must be even before the copy and equal after it; up to `max_retries`
/// attempts. Returns the sequence of the consistent copy, or `None`.
/// Never allocates: it only touches the caller's preallocated `dst`.
///
/// # Safety
/// `src` must be valid for reads of `FRAME_SIZE` bytes and 4-byte aligned for the whole call.
pub(crate) unsafe fn seqlock_copy(
    src: *const u8,
    dst: &mut SharedMemory,
    max_retries: u32,
) -> Option<u32> {
    use core::sync::atomic::{AtomicU32, Ordering, fence};
    use zerocopy::IntoBytes;
    // SAFETY: caller guarantees validity and alignment (offset 7320 is a multiple of 4).
    let seq = unsafe { &*(src.add(SEQUENCE_OFFSET) as *const AtomicU32) };
    for _ in 0..max_retries {
        let before = seq.load(Ordering::Acquire);
        if before & 1 == 1 {
            continue;
        }
        // The bytes may change under us while the game writes; a racing copy is discarded by the
        // check below, so only a copy with an unchanged even sequence is ever returned.
        // SAFETY: both ranges are FRAME_SIZE bytes and do not overlap (dst is private).
        unsafe { core::ptr::copy_nonoverlapping(src, dst.as_mut_bytes().as_mut_ptr(), FRAME_SIZE) };
        fence(Ordering::Acquire);
        if seq.load(Ordering::Relaxed) == before {
            return Some(before);
        }
    }
    None
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn fields_match_the_python_fixture() {
        let doc: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/offsets_v14.json")).unwrap();
        assert_eq!(doc["shared_memory_version"], SHARED_MEMORY_VERSION);
        assert_eq!(doc["mapping_name"], MAPPING_NAME);
        assert_eq!(doc["size"].as_u64().unwrap() as usize, FRAME_SIZE);
        let fields = doc["fields"].as_object().unwrap();
        for (name, off, size) in FIELD_TABLE {
            let f = &fields[*name];
            assert_eq!(
                f["offset"].as_u64().unwrap() as usize,
                *off,
                "{name} offset"
            );
            assert_eq!(f["size"].as_u64().unwrap() as usize, *size, "{name} size");
        }
        // every fixture field (but the two struct totals) is in the table
        assert_eq!(fields.len(), FIELD_TABLE.len() + 2);
        assert_eq!(FRAME_SIZE, 20700);
        assert_eq!(SEQUENCE_OFFSET, 7320);
    }
}
