//! Read-only view of a named file mapping.

use crate::{
    FRAME_SIZE, FrameSource, MAPPING_NAME, MAX_RETRIES, SharedMemory, ShmError, SnapshotOutcome,
};
use core::ffi::c_void;
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
use windows_sys::Win32::System::Memory::{
    FILE_MAP_READ, MEMORY_BASIC_INFORMATION, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile,
    OpenFileMappingW, UnmapViewOfFile, VirtualQuery,
};

pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// A read-only mapped view; reads consistent snapshots with the seqlock.
pub struct Mapping {
    handle: HANDLE,
    view: *const u8,
    region_size: usize,
}

// SAFETY: the view is read-only memory valid until Drop; the type is used from one thread at a time.
unsafe impl Send for Mapping {}

impl Mapping {
    /// Opens the game's mapping (`$pcars2$`). Never used by tests.
    pub fn open_game() -> Result<Self, ShmError> {
        Self::open(MAPPING_NAME)
    }

    /// Opens an existing mapping by name, read-only. `OpenFileMappingW` (not create) so that a
    /// missing game is an error and no empty mapping is made.
    pub fn open(name: &str) -> Result<Self, ShmError> {
        let w = wide(name);
        // SAFETY: `w` is NUL-terminated and outlives the call.
        let handle = unsafe { OpenFileMappingW(FILE_MAP_READ, 0, w.as_ptr()) };
        if handle.is_null() {
            return Err(ShmError::NotFound(name.to_owned()));
        }
        // SAFETY: valid mapping handle.
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_READ, 0, 0, 0) };
        if view.Value.is_null() {
            // SAFETY: plain calls on a handle we own.
            let code = unsafe { GetLastError() };
            unsafe { CloseHandle(handle) };
            return Err(ShmError::Os {
                call: "MapViewOfFile",
                code,
            });
        }
        // From here Drop releases both.
        let mut this = Self {
            handle,
            view: view.Value as *const u8,
            region_size: 0,
        };
        // SAFETY: all-zero is a valid MEMORY_BASIC_INFORMATION.
        let mut mbi: MEMORY_BASIC_INFORMATION = unsafe { core::mem::zeroed() };
        // SAFETY: `view` is a mapped address; mbi is a valid out buffer.
        unsafe {
            VirtualQuery(
                this.view as *const c_void,
                &mut mbi,
                size_of::<MEMORY_BASIC_INFORMATION>(),
            )
        };
        this.region_size = mbi.RegionSize;
        if this.region_size < FRAME_SIZE {
            return Err(ShmError::TooSmall {
                size: this.region_size,
                expected: FRAME_SIZE,
            });
        }
        Ok(this)
    }

    /// Size of the mapped region (rounded up to the page size by Windows).
    pub fn region_size(&self) -> usize {
        self.region_size
    }

    /// The sequence number, read atomically (4 bytes; odd while the game writes).
    pub fn sequence(&self) -> u32 {
        // SAFETY: the view is at least FRAME_SIZE bytes, page aligned; the offset is 4-aligned.
        unsafe { &*(self.view.add(crate::SEQUENCE_OFFSET) as *const core::sync::atomic::AtomicU32) }
            .load(core::sync::atomic::Ordering::Acquire)
    }

    /// Reads one field straight from the view (a volatile read: no copy of the frame). The
    /// caller checks the sequence number before and after, as for a snapshot.
    ///
    /// # Panics
    /// When the field would not lie inside the frame or `offset` is not aligned for `T`.
    pub fn read_field<T: zerocopy::FromBytes + Copy>(&self, offset: usize) -> T {
        assert!(
            offset + size_of::<T>() <= FRAME_SIZE && offset.is_multiple_of(align_of::<T>()),
            "field outside the frame"
        );
        // SAFETY: inside the mapped, page-aligned view and aligned for `T` (checked above);
        // every bit pattern is valid for a `FromBytes` type.
        unsafe { core::ptr::read_volatile(self.view.add(offset) as *const T) }
    }

    /// Seqlock snapshot with an explicit retry bound.
    pub fn snapshot_with(&self, out: &mut SharedMemory, max_retries: u32) -> SnapshotOutcome {
        // SAFETY: the view is at least FRAME_SIZE bytes (checked in open) and page aligned.
        match unsafe { crate::seqlock_copy(self.view, out, max_retries) } {
            Some(seq) => SnapshotOutcome::Frame(seq),
            None => SnapshotOutcome::Torn,
        }
    }
}

impl FrameSource for Mapping {
    /// Allocation-free: copies into the caller's preallocated frame.
    fn snapshot(&mut self, out: &mut SharedMemory) -> Result<SnapshotOutcome, ShmError> {
        Ok(self.snapshot_with(out, MAX_RETRIES))
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: view and handle were obtained in open and are released once.
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view as *mut c_void,
            });
            CloseHandle(self.handle);
        }
    }
}
