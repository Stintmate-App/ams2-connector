//! The single-instance named mutex and the named stop event.
//!
//! Names are parameters: [`Names::default`] has the production names, tests use
//! [`Names::unique`] so they never touch the driver's running connector.

use crate::win::{OwnedHandle, Win32Error, wide};
use std::time::Duration;
use windows_sys::Win32::Foundation::{
    ERROR_ALREADY_EXISTS, GetLastError, SetLastError, WAIT_ABANDONED, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, OpenEventW, OpenMutexW, ReleaseMutex,
    ResetEvent, SetEvent, WaitForSingleObject,
};

const SYNCHRONIZE: u32 = 0x0010_0000;
/// How long a second instance waits for a mutex that exists but whose owner is exiting.
const ACQUIRE_GRACE_MS: u32 = 250;

/// The kernel object names of the connector (`Local\` namespace: per logon session).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    /// The control app's single-instance mutex.
    pub ui: String,
    /// The recorder's single-instance mutex.
    pub recorder: String,
    /// The recorder's stop event.
    pub recorder_stop: String,
}

impl Default for Names {
    fn default() -> Self {
        Self {
            ui: "Local\\Ams2Connector.UI".into(),
            recorder: "Local\\Ams2Connector.Recorder".into(),
            recorder_stop: "Local\\Ams2Connector.Recorder.Stop".into(),
        }
    }
}

impl Names {
    /// Names that no other process uses (process id, clock and a counter in them), for tests.
    pub fn unique(tag: &str) -> Self {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let id = format!(
            "Test.{tag}.{}.{nanos}.{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        );
        Self {
            ui: format!("Local\\Ams2Connector.{id}.UI"),
            recorder: format!("Local\\Ams2Connector.{id}.Recorder"),
            recorder_stop: format!("Local\\Ams2Connector.{id}.Recorder.Stop"),
        }
    }
}

/// A named mutex owned by this process while it runs.
///
/// A mutex is owned by the creating **thread**: create it on the main thread and keep the
/// object for the process's life (Windows releases it when the process exits). The type is
/// `!Send` for that reason.
#[derive(Debug)]
pub struct SingleInstance {
    handle: Option<OwnedHandle>,
    held: bool,
    name: String,
    _thread_affine: std::marker::PhantomData<*const ()>,
}

impl SingleInstance {
    /// Create or open the mutex `name` and try to own it.
    pub fn acquire(name: &str) -> Result<Self, Win32Error> {
        let w = wide(name);
        // SAFETY: `w` is NUL-terminated; null attributes make the handle non-inheritable.
        let handle = unsafe {
            SetLastError(0);
            CreateMutexW(std::ptr::null(), 1, w.as_ptr())
        };
        // SAFETY: no preconditions; read straight after the call.
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let handle = OwnedHandle::new(handle).ok_or_else(|| Win32Error::last("CreateMutexW"))?;
        if !existed {
            return Ok(Self {
                handle: Some(handle),
                held: true,
                name: name.into(),
                _thread_affine: std::marker::PhantomData,
            });
        }
        // It exists: owned by a running instance, or only kept alive by someone probing it
        // (`is_running`) or by an instance that is exiting. Try to take it.
        // SAFETY: valid handle.
        let r = unsafe { WaitForSingleObject(handle.raw(), ACQUIRE_GRACE_MS) };
        if r == WAIT_OBJECT_0 || r == WAIT_ABANDONED {
            Ok(Self {
                handle: Some(handle),
                held: true,
                name: name.into(),
                _thread_affine: std::marker::PhantomData,
            })
        } else {
            Ok(Self {
                handle: None,
                held: false,
                name: name.into(),
                _thread_affine: std::marker::PhantomData,
            })
        }
    }

    /// True when another live process owns the mutex.
    pub fn already_running(&self) -> bool {
        !self.held
    }

    /// The mutex name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for SingleInstance {
    fn drop(&mut self) {
        if let (true, Some(h)) = (self.held, &self.handle) {
            // SAFETY: we own it on this thread; a failure on another thread just abandons it.
            unsafe { ReleaseMutex(h.raw()) };
        }
    }
}

/// True if a process holds the named mutex (the service is running). It only checks that the
/// object exists and never takes ownership, so it cannot get in the way of a starting service.
pub fn is_running(name: &str) -> bool {
    let w = wide(name);
    // SAFETY: `w` is NUL-terminated.
    OwnedHandle::new(unsafe { OpenMutexW(SYNCHRONIZE, 0, w.as_ptr()) }).is_some()
}

/// A named manual-reset stop event created by a service. Another process signals it with
/// [`signal_stop`]; the service polls [`StopEvent::is_set`] or waits with [`StopEvent::wait`].
#[derive(Debug)]
pub struct StopEvent {
    handle: OwnedHandle,
}

impl StopEvent {
    /// Create the event, not signalled (a signalled leftover of an exiting instance is reset).
    pub fn create(name: &str) -> Result<Self, Win32Error> {
        let w = wide(name);
        // SAFETY: as in `SingleInstance::acquire`.
        let raw = unsafe {
            SetLastError(0);
            CreateEventW(std::ptr::null(), 1, 0, w.as_ptr())
        };
        // SAFETY: read straight after the call.
        let existed = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
        let handle = OwnedHandle::new(raw).ok_or_else(|| Win32Error::last("CreateEventW"))?;
        if existed {
            // SAFETY: valid handle.
            unsafe { ResetEvent(handle.raw()) };
        }
        Ok(Self { handle })
    }

    /// A stop was requested (one syscall, no blocking).
    pub fn is_set(&self) -> bool {
        // SAFETY: valid handle.
        unsafe { WaitForSingleObject(self.handle.raw(), 0) == WAIT_OBJECT_0 }
    }

    /// Sleep up to `timeout` or until a stop is requested; true if stopped.
    pub fn wait(&self, timeout: Duration) -> bool {
        let ms = u32::try_from(timeout.as_millis()).unwrap_or(u32::MAX - 1);
        // SAFETY: valid handle.
        unsafe { WaitForSingleObject(self.handle.raw(), ms) == WAIT_OBJECT_0 }
    }

    /// Signal the event (also visible to other processes, tests and signal handlers).
    pub fn set(&self) {
        // SAFETY: valid handle.
        unsafe { SetEvent(self.handle.raw()) };
    }

    /// The raw handle, to put it in a `WaitForMultipleObjects` with other waits.
    pub fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.handle.raw()
    }
}

/// Ask the service owning the named stop event to stop. False if there is no such event.
pub fn signal_stop(name: &str) -> bool {
    let w = wide(name);
    // SAFETY: `w` is NUL-terminated.
    let Some(h) = OwnedHandle::new(unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, w.as_ptr()) }) else {
        return false;
    };
    // SAFETY: valid handle.
    unsafe { SetEvent(h.raw()) != 0 }
}
