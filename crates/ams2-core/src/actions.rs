//! The actions the engineer can send to this connector (PRY-066, contract 4.8 and 5.1), as data.
//!
//! The connector declares them with `pitwall.actions.declare` at every connection (idempotent,
//! `ik` `ams2/actions/<sha12 of the actions>`, last declaration wins) and serves them through
//! `connector.action` off the 50 Hz loop. This module is the constant declaration and its key;
//! the handler and the calls are the recorder's. The declaration equals the Python connector's
//! (same canonical JSON, same hash: `fixtures/announce_parity.json`).

use crate::announce::catalog::{canonical, codes, sha12};
use serde_json::{Value, json};
use std::sync::OnceLock;

/// `test.ping`: radio round trip.
pub const TEST_PING: &str = "test.ping";
/// `test.announce`: one announcement of the connector's own catalog.
pub const TEST_ANNOUNCE: &str = "test.announce";
/// How long an action waits for Pitwall to accept its announcement.
pub const ACCEPT_TIMEOUT_S: f64 = 2.0;
/// Timeout of `pitwall.actions.declare`.
pub const DECLARE_TIMEOUT_S: f64 = 5.0;

static ACTIONS: OnceLock<Value> = OnceLock::new();

/// The declared actions (a JSON array), built once.
pub fn actions() -> &'static Value {
    ACTIONS.get_or_init(|| {
        json!([
            {
                "code": TEST_PING,
                "description": "Radio round trip: the connector answers and says its radio check on the driver's radio. Use it when the driver asks for a radio test, or to prove that the connector is alive.",
                "timeout_s": 5,
                "params": {"type": "object", "additionalProperties": false, "properties": {
                    "say": {"type": "boolean",
                            "description": "Also play the radio check on the driver's radio (default true)."}}},
                "result": {"type": "object", "additionalProperties": false,
                           "required": ["pong", "announced"], "properties": {
                    "pong": {"type": "string", "maxLength": 64, "description": "Always 'pong'."},
                    "announced": {"type": "boolean",
                                  "description": "Pitwall accepted the radio check announcement."},
                    "message_id": {"type": "string", "maxLength": 64,
                                   "description": "The announcement's id (list_connector_messages), when announced."}}},
                "example": {"say": true},
            },
            {
                "code": TEST_ANNOUNCE,
                "description": "For desk tests of the radio calls: sends one announcement of the connector's own catalog with the given params; the driver hears the driver-audience codes. Param values are text and are converted by the code's param kinds (a lap time in seconds, a count as digits); a missing or unknown param is refused with bad_request naming it.",
                "timeout_s": 5,
                "params": {"type": "object", "additionalProperties": false,
                           "required": ["code"], "properties": {
                    "code": {"type": "string", "enum": codes(),
                             "description": "The catalog code to announce (list_connector_messages)."},
                    "params": {"type": "array", "maxItems": 8,
                               "description": "The code's params, one entry per param it declares.",
                               "items": {"type": "object", "additionalProperties": false,
                                         "required": ["name", "value"],
                                         "description": "One param of the announcement.",
                                         "properties": {
                                   "name": {"type": "string", "maxLength": 32,
                                            "description": "The param's name, as the catalog declares it."},
                                   "value": {"type": "string", "maxLength": 200,
                                             "description": "The value as text."}}}}}},
                "result": {"type": "object", "additionalProperties": false,
                           "required": ["announced"], "properties": {
                    "announced": {"type": "boolean",
                                  "description": "Pitwall accepted the announcement."},
                    "message_id": {"type": "string", "maxLength": 64,
                                   "description": "The announcement's id, when announced."},
                    "refused": {"type": "string", "maxLength": 300,
                                "description": "Why it was not announced (rate limit, Pitwall's refusal), when it was not."}}},
                "example": {"code": "engineer_test",
                            "params": [{"name": "text", "value": "radio test"}]},
            },
        ])
    })
}

/// The idempotency key of the declaration: `ams2/actions/<sha12 of the actions>`.
pub fn actions_ik(actions: &Value) -> String {
    format!(
        "{}/actions/{}",
        crate::CONNECTOR_ID,
        sha12(&canonical(actions))
    )
}
