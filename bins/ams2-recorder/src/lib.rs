//! The AMS2 recorder (PRY-057 B1): the 50 Hz loop, the outbox and the links to Pitwall.
//!
//! * [`recorder`]: the loop, the game watch, the status.
//! * [`sink`], [`outbox`]: tracker records to write API messages to files to Pitwall.
//! * [`link`]: the two links on Stintmate IPC's blocking façade, the status publisher.
//! * [`announcer`]: announcements and the per-connection declarations.

pub mod announcer;
#[cfg(feature = "capture-raw")]
pub mod capture_raw;
pub mod cli;
pub mod link;
pub mod live;
pub mod outbox;
pub mod recorder;
pub mod serve;
pub mod sink;
pub mod status;
pub mod telemetry;
