//! Windows helpers of the AMS2 connector, shared by both executables.
//!
//! - [`workspace`]: the Stintmate workspace resolver (contract v1) and [`location`], the
//!   connector's data folder;
//! - [`logging`]: `tracing` to `<data>\<name>.log`, the line format `date time LEVEL target: message`,
//!   non-blocking, with a size cap;
//! - [`dev`]: the development-only instance tag (`STINTMATE_DEV_INSTANCE`): names and pipes;
//! - [`single`]: the named mutex (single instance) and the named stop event;
//! - [`procinfo`]: is a PID alive, and its process chain (for logs);
//! - [`proc`]: process helpers (image name, start time, kill-on-close job object, spawn
//!   suspended) and [`game`], the "is AMS2 running" check;
//! - [`timer`]: the high-resolution waitable timer for the 50 Hz loop.

pub mod dev;
pub mod location;
pub mod logging;
pub mod workspace;

#[cfg(windows)]
pub mod console;
#[cfg(windows)]
pub mod game;
#[cfg(windows)]
pub mod proc;
#[cfg(windows)]
pub mod procinfo;
#[cfg(windows)]
pub mod single;
#[cfg(windows)]
pub mod timer;
#[cfg(windows)]
mod win;
#[cfg(windows)]
pub mod window;

#[cfg(windows)]
pub use win::{OwnedHandle, Win32Error};
