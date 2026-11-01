//! The connector's guidance for the engineer (contract 4.4), as data.
//!
//! Rules about this simulator that the engineer's agent cannot know. The connector declares
//! them with `pitwall.guidance.declare` at every connection (idempotent, `ik`
//! `ams2/guidance/<sha12 of the items>`, last declaration wins); Pitwall lists the topics in its
//! MCP instructions and serves the texts through `get_connector_guidance`. The items' canonical JSON and hash are
//! pinned by `fixtures/announce_parity.json`.

use crate::announce::catalog::{canonical, sha12};
use serde_json::{Value, json};
use std::sync::OnceLock;

/// Timeout of `pitwall.guidance.declare`.
pub const DECLARE_TIMEOUT_S: f64 = 5.0;

static ITEMS: OnceLock<Value> = OnceLock::new();

/// The guidance items (a JSON array), built once.
pub fn items() -> &'static Value {
    ITEMS.get_or_init(|| {
        json!([
            {
                "id": "setup_from_capture",
                "topic": "setup",
                "when": "you read, log or compare a car setup",
                "text": "AMS2 encrypts its setup files (`*.vts`): never try to read them. A setup is read from a capture of the in-game setup screen. Ask the driver to open the setup tab you need and press the capture button (or call `capture_screen` yourself when he asks), then find the capture with `list_captures`, open it with `get_capture` and read the values from the image. One tab per capture: ask for the next tab when you need it. Do not guess an unreadable value: ask for that tab again. Log the values in `team/cars/<car>/setups/...`.",
            },
            {
                "id": "garage_screens",
                "topic": "capture",
                "when": "the driver captures a game screen",
                "text": "The capture button grabs the AMS2 window, wherever he presses it: a setup tab, the timing or results table, a championship page, a telemetry HUD page or a replay. Label each capture with `label_capture` for what it shows. Values on a capture are in the units the driver configured in the game (pressures, temperatures, speeds): read the unit from the screen and do not assume metric or imperial.",
            },
        ])
    })
}

/// The idempotency key of the declaration: `ams2/guidance/<sha12 of the items>`.
pub fn items_ik(items: &Value) -> String {
    format!(
        "{}/guidance/{}",
        crate::CONNECTOR_ID,
        sha12(&canonical(items))
    )
}
