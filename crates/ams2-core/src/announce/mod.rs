//! Live race announcements (PRY-052): the `connector.ams2` catalog the connector declares to
//! Pitwall ([`catalog`]), the pure edge detection over shared memory snapshots that decides
//! when each code fires ([`detector`]) and the contract's rate window ([`rate`]). Sending is
//! the recorder's job.

pub mod catalog;
pub mod detector;
pub mod rate;

#[cfg(test)]
mod catalog_tests;
#[cfg(test)]
mod detector_tests;

pub use catalog::{
    CATALOG_ID, catalog, catalog_ik, check_params, parse_params, source_hash, validate_catalog,
};
pub use detector::{Announcement, EventDetector};
pub use rate::{RATE_LIMIT, RATE_WINDOW_S, RateWindow};
