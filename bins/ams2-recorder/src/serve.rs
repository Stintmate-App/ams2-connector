//! The methods the recorder serves to Pitwall besides `connector.live_frame`
//! (`actions.py`, `capture_target.py`): `connector.capture_target` and `connector.action`.
//! Both run on the handler pool, never on the 50 Hz loop.

use crate::announcer::Announcer;
use crate::link::to_value;
use ams2_core::actions::{ACCEPT_TIMEOUT_S, TEST_ANNOUNCE, TEST_PING};
use ams2_core::announce::Announcement;
use ams2_core::announce::catalog::{self, check_params, parse_params};
use ams2_platform::window::{Scope, WindowInfo, find_game_window, list_windows};
use serde_json::{Map, Value as Json, json};
use std::sync::Arc;
use std::time::Duration;
use stintmate_ipc::codec::{self, Value};
use stintmate_ipc::error::RemoteError;

/// An IPC value as JSON (`bin` as an array of bytes, `ext` as null).
pub fn to_json(v: &Value) -> Json {
    match v {
        Value::Nil | Value::Ext(..) => Json::Null,
        Value::Boolean(b) => json!(b),
        Value::Integer(i) => i.as_i64().map_or_else(|| json!(i.as_u64()), |x| json!(x)),
        Value::F32(f) => json!(f64::from(*f)),
        Value::F64(f) => json!(f),
        Value::String(s) => json!(s.as_str().unwrap_or_default()),
        Value::Binary(b) => json!(b),
        Value::Array(a) => Json::Array(a.iter().map(to_json).collect()),
        Value::Map(m) => Json::Object(
            m.iter()
                .map(|(k, v)| (k.as_str().unwrap_or_default().to_owned(), to_json(v)))
                .collect(),
        ),
    }
}

/// `connector.capture_target {}` -> `{hwnd, pid, title}` among `windows`.
pub fn capture_target_of(windows: &[WindowInfo]) -> Result<Value, RemoteError> {
    let name = ams2_core::SIM_NAME;
    let w = find_game_window(windows).ok_or_else(|| {
        RemoteError::new(
            "game_not_running",
            format!("{name} is not running (no game window)."),
        )
    })?;
    if w.minimized {
        return Err(RemoteError::new(
            "window_minimized",
            format!("The {name} window is minimized: restore it (it needs no focus)."),
        ));
    }
    Ok(to_value(
        &json!({"hwnd": w.hwnd, "pid": w.pid, "title": w.title}),
    ))
}

/// `connector.capture_target` for the live desktop (EnumWindows, off the loop).
pub fn capture_target() -> Result<Value, RemoteError> {
    capture_target_of(&list_windows(Scope::AllVisible))
}

/// The static `&'static str` of a catalog code.
fn static_code(code: &str) -> Option<&'static str> {
    catalog::codes().into_iter().find(|c| *c == code)
}

/// Serves `connector.action` (`test.ping`, `test.announce`).
pub struct ActionServer {
    announcer: Arc<Announcer>,
}

impl ActionServer {
    /// A server announcing through `announcer` (its rate window bounds the radio).
    pub fn new(announcer: Arc<Announcer>) -> Self {
        Self { announcer }
    }

    /// One call: `{code, params?, variant?, data?, call_id?}`.
    pub fn call(&self, params: &Value) -> Result<Value, RemoteError> {
        let p = to_json(params);
        let code = p.get("code").and_then(Json::as_str).unwrap_or_default();
        let data = p.get("data").cloned().unwrap_or(Json::Null);
        tracing::info!(
            "Action {code} (call {})",
            p.get("call_id").and_then(Json::as_str).unwrap_or("-")
        );
        let out = match code {
            TEST_PING => self.ping(&data),
            TEST_ANNOUNCE => self.announce(&data)?,
            other => {
                return Err(RemoteError::new(
                    "unknown_action",
                    format!("this connector does not serve {other:?}"),
                ));
            }
        };
        Ok(to_value(&out))
    }

    fn send(&self, code: &'static str, variant: u8, params: Map<String, Json>) -> Json {
        let a = Announcement {
            code,
            variant,
            params,
            data: None,
        };
        let wait = Duration::from_secs_f64(ACCEPT_TIMEOUT_S);
        match self.announcer.announce_wait(&a, wait) {
            Ok(res) => {
                let mut out = json!({"announced": true});
                if let Some(id) = codec::get(&res, "message_id").and_then(Value::as_str)
                    && !id.is_empty()
                {
                    out["message_id"] = json!(id.chars().take(64).collect::<String>());
                }
                out
            }
            Err(why) => {
                let why = if why.is_empty() {
                    "not announced".into()
                } else {
                    why
                };
                json!({"announced": false, "refused": why.chars().take(300).collect::<String>()})
            }
        }
    }

    fn ping(&self, data: &Json) -> Json {
        if data.get("say") == Some(&Json::Bool(false)) {
            return json!({"pong": "pong", "announced": false});
        }
        let mut sent = self.send("radio_check", 0, Map::new());
        sent.as_object_mut().map(|m| m.remove("refused")); // the ping's answer has no reason field
        let mut out = json!({"pong": "pong"});
        out.as_object_mut()
            .expect("an object")
            .extend(sent.as_object().cloned().unwrap_or_default());
        out
    }

    fn announce(&self, data: &Json) -> Result<Json, RemoteError> {
        let bad = |m: String| RemoteError::new("bad_request", m);
        let code = data.get("code").and_then(Json::as_str).unwrap_or_default();
        let mut pairs = Vec::new();
        for item in data
            .get("params")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let (Some(n), Some(v)) = (
                item.get("name").and_then(Json::as_str),
                item.get("value").and_then(Json::as_str),
            ) else {
                return Err(bad("params: each entry needs name and value".into()));
            };
            pairs.push(format!("{n}={v}"));
        }
        let params = parse_params(code, &pairs).map_err(|e| bad(format!("params: {e}")))?;
        if let Some(problem) =
            check_params(code, &Json::Object(params.clone()), catalog::messages())
        {
            return Err(bad(problem));
        }
        let variant = match data.get("variant") {
            None | Some(Json::Null) => 0,
            Some(v) => v
                .as_u64()
                .and_then(|n| u8::try_from(n).ok())
                .ok_or_else(|| bad("variant: must be an integer 0..255".into()))?,
        };
        let code = static_code(code).ok_or_else(|| bad(format!("unknown code {code:?}")))?;
        Ok(self.send(code, variant, params))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ams2_platform::window::{WINDOW_CLASS, WindowInfo};

    fn win(minimized: bool) -> WindowInfo {
        WindowInfo {
            hwnd: 4242,
            pid: 77,
            exe: "ams2avx.exe".into(),
            title: "Automobilista 2".into(),
            class: WINDOW_CLASS.into(),
            area: if minimized { 0 } else { 1920 * 1080 },
            minimized,
        }
    }

    fn err(r: Result<Value, RemoteError>) -> (String, String) {
        let e = r.unwrap_err();
        (e.code, e.message)
    }

    #[test]
    fn capture_target_is_hwnd_pid_title_or_the_protocols_errors() {
        let v = to_json(&capture_target_of(&[win(false)]).unwrap());
        assert_eq!(
            v,
            json!({"hwnd": 4242, "pid": 77, "title": "Automobilista 2"})
        );
        let (code, msg) = err(capture_target_of(&[]));
        assert_eq!(code, "game_not_running");
        assert!(msg.contains("no game window"), "{msg}");
        let (code, msg) = err(capture_target_of(&[win(true)]));
        assert_eq!(code, "window_minimized");
        assert!(msg.contains("restore it"), "{msg}");
    }

    #[test]
    fn params_are_typed_by_the_catalogs_kinds() {
        let mut checked = 0;
        for (code, m) in catalog::messages() {
            for (name, spec) in m["params"].as_object().into_iter().flatten() {
                let kind = spec.as_str().or_else(|| spec["kind"].as_str());
                let (value, want_int) = match kind {
                    Some("int" | "ordinal" | "laps") => ("3", Some(true)),
                    Some(
                        "decimal" | "gap" | "lap_time" | "distance_m" | "speed" | "temperature"
                        | "litres",
                    ) => ("1.5", Some(false)),
                    _ => continue,
                };
                let p = parse_params(code, &[format!("{name}={value}")]).unwrap();
                assert_eq!(p[name].is_i64(), want_int.unwrap(), "{code}.{name}");
                assert!(parse_params(code, &[format!("{name}=abc")]).is_err());
                checked += 1;
            }
        }
        assert!(checked >= 3, "{checked} typed params");
        assert!(parse_params("engineer_test", &["=x".into()]).is_err());
    }
}
