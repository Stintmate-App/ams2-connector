//! The connector's logic without Windows or IPC: sessions, laps, announcements, live frame.
//!
//! Part I (PRY-057 A3): everything pure and clocked from outside.
//!
//! * [`tracker`]: the session / lap state machine ([`tracker::SessionTracker`]), fed one
//!   shared memory frame per call; its records go to a [`tracker::Sink`].
//! * [`rivals`]: one row per participant and lap.
//! * [`garage`]: the garage rule and the protocol's `phase`.
//! * [`channels`], [`vocab`], [`codes`]: the 72 channels, AMS2 values to Pitwall's codes.
//! * [`setup`], [`names`], [`messages`]: the write API messages and how they are built.
//! * [`testing`]: frame builders and the scripted session (also used by later tasks).
//!
//! Part II (PRY-057 A4):
//!
//! * [`frame`]: the neutral live frame (`connector.live_frame`, with the PRY-037 fields).
//! * [`telemetry`]: the 32-byte `connector.telemetry` sample and its flags.
//! * [`announce`]: the 28-code catalog, the event detector and the rate window.
//! * [`actions`], [`guidance`]: the declared actions and guidance, as data.
//!
//! Nothing here reads a clock or a file: time comes in through [`tracker::Clock`].

pub mod actions;
pub mod announce;
pub mod channels;
pub mod codes;
pub mod frame;
pub mod garage;
pub mod golden;
pub mod guidance;
pub mod messages;
pub mod names;
pub mod rivals;
pub mod setup;
pub mod telemetry;
pub mod testing;
pub mod tracker;
pub mod vocab;

#[cfg(test)]
mod frame_tests;
#[cfg(test)]
mod golden_tests;
#[cfg(test)]
mod parity_tests;
#[cfg(test)]
mod rivals_tests;
#[cfg(test)]
mod schema_check;
#[cfg(test)]
mod tracker_tests;

/// Connector id on the wire (`ams2/...` idempotency keys).
pub const CONNECTOR_ID: &str = "ams2";
/// Simulator id.
pub const SIM: &str = "ams2";
/// Simulator display name.
pub const SIM_NAME: &str = "Automobilista 2";

/// Python-compatible `round(x, ndigits)`: correctly rounded, ties to even on the exact value.
pub fn round_to(x: f64, ndigits: usize) -> f64 {
    if !x.is_finite() {
        return x;
    }
    format!("{x:.ndigits$}").parse().unwrap_or(x)
}

/// A NUL-terminated `char[]` field as text (invalid UTF-8 replaced); borrows when it can.
pub fn decode_str(raw: &[u8]) -> std::borrow::Cow<'_, str> {
    let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
    String::from_utf8_lossy(&raw[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_matches_python() {
        assert_eq!(round_to(2.675, 2), 2.67); // the exact value is below the tie
        assert_eq!(round_to(0.5, 0), 0.0);
        assert_eq!(round_to(1.5, 0), 2.0);
        assert_eq!(round_to(25.5, 2), 25.5);
    }

    #[test]
    fn decode_stops_at_nul() {
        assert_eq!(decode_str(b"abc\0def"), "abc");
        assert_eq!(decode_str(b"abc"), "abc");
        assert_eq!(decode_str(b"\0"), "");
    }
}
