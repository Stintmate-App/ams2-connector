//! The command line of `ams2-connector-ui.exe` run as a process: `--version`, `--register`,
//! `--unregister`, the usage message and the exit statuses. Only temporary workspaces; the
//! options that start or stop the app (`--quit`) are tested through `cli::quit` with unique
//! names in `endpoint.rs`, never against the driver's real instance.

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ams2-connector-ui"))
        .args(args)
        .env_remove("STINTMATE_WORKSPACE")
        .env(
            "STINTMATE_SETTINGS_FILE",
            std::env::temp_dir().join("ams2-cli-test-no-settings.json"),
        )
        .output()
        .expect("run ams2-connector-ui")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn descriptor(ws: &Path) -> std::path::PathBuf {
    ws.join("connectors").join("ams2").join("connector.json")
}

#[test]
fn version_prints_the_name_and_the_version() {
    let o = run(&["--version"]);
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&o.stdout).trim(),
        format!("ams2-connector-ui {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn register_writes_and_unregister_removes_only_the_descriptor() {
    let dir = workspace();
    let ws = dir.path().to_str().unwrap();
    let o = run(&["--register", "--workspace", ws]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let d: Value = serde_json::from_slice(&std::fs::read(descriptor(dir.path())).unwrap()).unwrap();
    assert_eq!(d["id"], "ams2");
    assert!(
        d["launcher"]
            .as_str()
            .unwrap()
            .ends_with("ams2-connector-ui.exe")
    );
    // Data the connector keeps next to the descriptor stays.
    let data = dir.path().join("connectors").join("ams2");
    for f in ["ui.log", "ui_config.json"] {
        std::fs::write(data.join(f), "x").unwrap();
    }
    std::fs::create_dir_all(data.join("outbox")).unwrap();

    let o = run(&["--unregister", "--workspace", ws]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(!descriptor(dir.path()).exists());
    assert!(data.join("ui.log").exists() && data.join("ui_config.json").exists());
    assert!(data.join("outbox").is_dir());

    // Nothing left to remove: still 0.
    let o = run(&["--unregister", "--workspace", ws]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(text(&o).contains("nothing to remove"));
}

#[test]
fn unregister_leaves_a_descriptor_that_is_not_ours() {
    let dir = workspace();
    let path = descriptor(dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"id":"other"}"#).unwrap();
    let o = run(&["--unregister", "--workspace", dir.path().to_str().unwrap()]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
    assert!(path.exists());
}

#[test]
fn a_workspace_error_is_exit_3_for_register_and_unregister() {
    let missing = tempfile::tempdir().unwrap().path().join("not-there");
    let ws = missing.to_str().unwrap();
    for opt in ["--register", "--unregister"] {
        let o = run(&[opt, "--workspace", ws]);
        assert_eq!(o.status.code(), Some(3), "{opt}: {}", text(&o));
    }
    assert!(!missing.exists(), "nothing is created");
}

#[test]
fn register_check_writes_nothing_and_reports_the_three_outcomes() {
    let dir = workspace();
    let ws = dir.path().to_str().unwrap();
    let o = run(&["--register", "--check", "--workspace", ws]);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    assert!(!descriptor(dir.path()).exists(), "no descriptor written");
    let data = descriptor(dir.path()).parent().unwrap().to_path_buf();
    assert_eq!(
        std::fs::read_dir(&data).unwrap().count(),
        0,
        "no test file left"
    );

    let missing = tempfile::tempdir().unwrap();
    let missing_ws = missing.path().join("not-there");
    let o = run(&[
        "--register",
        "--check",
        "--workspace",
        missing_ws.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(3), "{}", text(&o));

    let blocked = workspace();
    let conn = blocked.path().join("connectors");
    std::fs::create_dir_all(&conn).unwrap();
    std::fs::write(conn.join("ams2"), "a file").unwrap();
    let o = run(&[
        "--register",
        "--check",
        "--workspace",
        blocked.path().to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(1), "{}", text(&o));
}

#[test]
fn bad_arguments_print_the_usage_with_every_option() {
    let o = run(&["--bogus"]);
    assert_eq!(o.status.code(), Some(2));
    let t = text(&o);
    for opt in [
        "--tray",
        "--managed",
        "--parent-pid",
        "--register",
        "--unregister",
        "--quit",
        "--version",
        "--workspace",
        "--allow-synced",
    ] {
        assert!(t.contains(opt), "usage lacks {opt}: {t}");
    }
    for combo in [
        &["--unregister", "--managed", "--parent-pid", "5"][..],
        &["--quit", "--managed", "--parent-pid", "5"][..],
        &["--register", "--unregister"][..],
        &["--version", "--quit"][..],
    ] {
        assert_eq!(run(combo).status.code(), Some(2), "{combo:?}");
    }
}

/// A temporary folder that is a workspace (`stintmate.json`).
fn workspace() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("stintmate.json"), r#"{"schema": 1}"#).unwrap();
    dir
}
