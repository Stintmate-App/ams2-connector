//! A writer for a mapping under a test name, using the game's sequence protocol.
//! It refuses the game's real name, so tests can never touch `$pcars2$`.

use crate::mapping::wide;
use crate::{FRAME_SIZE, MAPPING_NAME, SEQUENCE_OFFSET, SharedMemory, ShmError};
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, AtomicU64, Ordering, fence};
use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READWRITE,
    UnmapViewOfFile,
};
use zerocopy::IntoBytes;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns a process-unique mapping name for a test.
pub fn unique_name(tag: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("Local\\ams2-shm-test-{}-{}-{}", std::process::id(), tag, n)
}

/// Owner of a writable test mapping.
pub struct TestMapping {
    handle: HANDLE,
    view: *mut u8,
    sequence: u32,
}

// SAFETY: the view is valid until Drop and only used by its owner.
unsafe impl Send for TestMapping {}

impl TestMapping {
    /// Creates a zeroed mapping of one frame (sequence 0) under `name`.
    pub fn create(name: &str) -> Result<Self, ShmError> {
        if name == MAPPING_NAME || name.contains("pcars2") {
            return Err(ShmError::RefusedName(name.to_owned()));
        }
        let w = wide(name);
        // SAFETY: `w` is NUL-terminated; pagefile-backed mapping.
        let handle = unsafe {
            CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                core::ptr::null(),
                PAGE_READWRITE,
                0,
                FRAME_SIZE as u32,
                w.as_ptr(),
            )
        };
        if handle.is_null() {
            // SAFETY: plain call.
            let code = unsafe { GetLastError() };
            return Err(ShmError::Os {
                call: "CreateFileMappingW",
                code,
            });
        }
        // SAFETY: valid handle.
        let view = unsafe { MapViewOfFile(handle, FILE_MAP_WRITE, 0, 0, 0) };
        if view.Value.is_null() {
            // SAFETY: plain calls on a handle we own.
            let code = unsafe { GetLastError() };
            unsafe { CloseHandle(handle) };
            return Err(ShmError::Os {
                call: "MapViewOfFile",
                code,
            });
        }
        Ok(Self {
            handle,
            view: view.Value as *mut u8,
            sequence: 0,
        })
    }

    fn seq(&self) -> &AtomicU32 {
        // SAFETY: the view is FRAME_SIZE bytes and page aligned; the offset is 4-aligned.
        unsafe { &*(self.view.add(SEQUENCE_OFFSET) as *const AtomicU32) }
    }

    /// Writes a frame like the game: sequence goes odd, the body is copied, sequence goes even.
    /// The frame's own `sequence_number` is ignored; the writer counts by two.
    pub fn write_frame(&mut self, frame: &SharedMemory) {
        let bytes = frame.as_bytes();
        let odd = self.sequence.wrapping_add(1);
        self.seq().store(odd, Ordering::Release);
        fence(Ordering::SeqCst);
        // SAFETY: ranges inside the view; the sequence word itself is skipped.
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.view, SEQUENCE_OFFSET);
            core::ptr::copy_nonoverlapping(
                bytes.as_ptr().add(SEQUENCE_OFFSET + 4),
                self.view.add(SEQUENCE_OFFSET + 4),
                FRAME_SIZE - SEQUENCE_OFFSET - 4,
            );
        }
        self.sequence = self.sequence.wrapping_add(2);
        self.seq().store(self.sequence, Ordering::Release);
    }

    /// Leaves the mapping in the "game is writing" state (odd sequence), to test retries.
    pub fn begin_write_and_hold(&mut self) {
        self.seq()
            .store(self.sequence.wrapping_add(1), Ordering::Release);
    }

    /// Sequence of the last completed write.
    pub fn sequence(&self) -> u32 {
        self.sequence
    }
}

impl Drop for TestMapping {
    fn drop(&mut self) {
        // SAFETY: released once.
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS {
                Value: self.view as *mut c_void,
            });
            CloseHandle(self.handle);
        }
    }
}
