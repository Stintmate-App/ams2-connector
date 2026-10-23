//! The connector descriptor `<workspace>\connectors\ams2\connector.json`.
//!
//! How a managing app finds the connectors installed for a workspace without knowing any
//! simulator (workspace.md section 5.5). Written by this connector only, into its own folder:
//! by the control app at every start (only when something but `written_at` changed, atomically)
//! and by `ams2-connector-ui --register`, which writes it and exits.

use crate::config::write_json_atomic;
use crate::texts::Texts;
use serde_json::{Value, json};
use std::io;
use std::path::{Path, PathBuf};

/// The descriptor's schema version.
pub const SCHEMA: u64 = 1;
/// The control protocol the hub requires.
pub const CONTROL_PROTOCOL: &str = "stintmate-ipc/2";
/// The file name.
pub const FILE_NAME: &str = "connector.json";

/// Where the descriptor goes for `data_dir` (`<workspace>\connectors\ams2`).
pub fn path_for(data_dir: &Path) -> PathBuf {
    data_dir.join(FILE_NAME)
}

/// The descriptor. `launcher`: this exe; `mutex`: the control app's single-instance mutex.
pub fn build(launcher: &Path, mutex: &str) -> Value {
    json!({
        "schema": SCHEMA,
        "id": crate::CONNECTOR_ID,
        "name": crate::CONNECTOR_NAME,
        "sim": crate::SIM,
        "sim_name": crate::SIM_NAME,
        "version": crate::VERSION,
        "launcher": launcher.display().to_string(),
        "args": [],
        "control_protocol": CONTROL_PROTOCOL,
        "endpoint": crate::ENDPOINT_ID,
        "mutex": mutex,
        // A file other apps read: the names stay English (data, not this app's UI).
        "services": crate::SERVICES.iter().map(|s| json!({
            "id": s,
            "name": Texts::english_text(&format!("service.{s}")),
        })).collect::<Vec<_>>(),
        "logs": ["recorder.log", "ui.log"],
        "written_at": ams2_platform::logging::local_iso(false),
        "written_by": format!("{} {}", crate::APP_ID, crate::VERSION),
    })
}

fn same(a: &Value, b: &Value) -> bool {
    let strip = |v: &Value| {
        let mut v = v.clone();
        if let Some(o) = v.as_object_mut() {
            o.remove("written_at");
        }
        v
    };
    a.is_object() && strip(a) == strip(b)
}

/// Writes (or refreshes) the descriptor in `data_dir`; returns its path. Left untouched when
/// nothing but the write time would change.
pub fn write(data_dir: &Path, launcher: &Path, mutex: &str) -> io::Result<PathBuf> {
    let target = path_for(data_dir);
    let doc = build(launcher, mutex);
    let current = std::fs::read(&target)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    if current.as_ref().is_some_and(|c| same(c, &doc)) {
        return Ok(target);
    }
    write_json_atomic(&target, &doc)?;
    tracing::info!("Connector descriptor written: {}", target.display());
    Ok(target)
}

/// [`write`] for the app's start: a failure is logged, never fatal.
pub fn refresh(data_dir: &Path, launcher: &Path, mutex: &str) -> Option<PathBuf> {
    match write(data_dir, launcher, mutex) {
        Ok(p) => Some(p),
        Err(e) => {
            tracing::warn!("Cannot write the connector descriptor: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_field_of_workspace_md_5_5() {
        let d = build(Path::new(r"C:\x\ams2-connector-ui.exe"), r"Local\M");
        for k in [
            "schema",
            "id",
            "name",
            "sim",
            "sim_name",
            "version",
            "launcher",
            "args",
            "control_protocol",
            "endpoint",
            "mutex",
            "services",
            "logs",
            "written_at",
            "written_by",
        ] {
            assert!(d.get(k).is_some(), "{k}");
        }
        assert_eq!(d["id"], "ams2");
        assert_eq!(d["endpoint"], "connector-ams2");
        assert_eq!(d["control_protocol"], "stintmate-ipc/2");
        assert_eq!(
            d["services"],
            json!([{"id": "recorder", "name": "Recorder"}])
        );
        assert_eq!(d["logs"], json!(["recorder.log", "ui.log"]));
        assert_eq!(d["args"], json!([]));
    }

    #[test]
    fn written_once_and_left_alone_until_something_changes() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("connectors").join("ams2");
        let exe = Path::new(r"C:\x\ams2-connector-ui.exe");
        let p = write(&data, exe, r"Local\M").unwrap();
        let first = std::fs::read(&p).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(1100));
        write(&data, exe, r"Local\M").unwrap();
        assert_eq!(
            std::fs::read(&p).unwrap(),
            first,
            "only written_at would differ"
        );
        write(
            &data,
            Path::new(r"D:\moved\ams2-connector-ui.exe"),
            r"Local\M",
        )
        .unwrap();
        let after: Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        assert_eq!(after["launcher"], r"D:\moved\ams2-connector-ui.exe");
    }
}
