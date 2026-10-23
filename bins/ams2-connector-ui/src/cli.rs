//! The command line and the process's life: `--register`, workspace errors, single instance,
//! the second launch's call, and the headless run until a UI (U1) is attached.

use crate::api::UiEvent;
use crate::app::{ControlApp, Environment};
use crate::autostart::{Autostart, WinRunKey};
use crate::endpoint::ServeConfig;
use crate::services::WinHost;
use crate::texts::Texts;
use ams2_platform::location::{self, WORKSPACE_ERROR_EXIT, WorkspaceArgs};
use ams2_platform::logging::{self, LogConfig};
use ams2_platform::single::{Names, SingleInstance};
use ams2_platform::workspace::SystemProbes;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::Duration;
use stintmate_ipc::keyfile::{KeyFile, KeySource, default_key_path};

/// What the command line asked for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    /// `--tray`: start hidden in the tray.
    pub tray: bool,
    /// `--managed`.
    pub managed: bool,
    /// `--parent-pid <pid>`.
    pub parent_pid: Option<u32>,
    /// `--register`: write the descriptor and exit.
    pub register: bool,
    /// `--workspace`.
    pub workspace: Option<String>,
    /// `--allow-synced`.
    pub allow_synced: bool,
}

/// Parses the arguments (program name excluded); the error is a usage message.
pub fn parse<I: IntoIterator<Item = String>>(argv: I) -> Result<Args, String> {
    let ws = WorkspaceArgs::parse(argv);
    let mut a = Args {
        workspace: ws.workspace,
        allow_synced: ws.allow_synced,
        ..Args::default()
    };
    let mut it = ws.rest.into_iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--tray" => a.tray = true,
            "--managed" => a.managed = true,
            "--register" => a.register = true,
            "--parent-pid" => {
                let v = it.next().ok_or("--parent-pid needs a value")?;
                a.parent_pid = Some(v.parse().map_err(|_| "--parent-pid needs a number")?);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if a.managed && a.parent_pid.is_none_or(|p| p == 0) {
        return Err("--managed needs --parent-pid <pid>".into());
    }
    if a.parent_pid.is_some() && !a.managed {
        return Err("--parent-pid needs --managed".into());
    }
    if a.register && a.managed {
        return Err("--register starts nothing: not with --managed".into());
    }
    Ok(a)
}

fn say(line: &str) {
    println!("{line}");
}

/// `--register`: writes the descriptor and returns the exit status: 0 written (or already up
/// to date), 3 a workspace error (none chosen included), 1 the file cannot be written.
pub fn register(args: &Args, ui_exe: &std::path::Path, mutex: &str) -> i32 {
    let loc = match location::resolve(args.workspace.as_deref(), args.allow_synced, &SystemProbes) {
        Ok(l) => l,
        Err(e) => {
            say(&e.line());
            return WORKSPACE_ERROR_EXIT;
        }
    };
    match crate::descriptor::write(&loc.data_dir, ui_exe, mutex) {
        Ok(p) => {
            say(&format!("registered: {}", p.display()));
            0
        }
        Err(e) => {
            say(&format!("cannot write the connector descriptor: {e}"));
            1
        }
    }
}

/// The process's entry point: the exit status.
pub fn main(argv: Vec<String>) -> i32 {
    let args = match parse(argv) {
        Ok(a) => a,
        Err(m) => {
            eprintln!("ams2-connector-ui: {m}");
            return 2;
        }
    };
    let names = Names::default();
    let ui_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ams2-connector-ui.exe"));
    if args.register {
        return register(&args, &ui_exe, &names.ui);
    }
    let loc = match location::resolve(args.workspace.as_deref(), args.allow_synced, &SystemProbes) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}", e.line());
            #[cfg(windows)]
            if !args.managed {
                let t = Texts::english();
                let body = format!(
                    "{}

{}

{}",
                    t.s("startup.error.title"),
                    e.line(),
                    t.s("startup.error.hint")
                );
                crate::dialog::error(&t.s("app.name"), &body);
            }
            return WORKSPACE_ERROR_EXIT;
        }
    };
    let _ = std::fs::create_dir_all(&loc.data_dir);
    let _guard = logging::init(&LogConfig::new(&loc.data_dir, "ui"));
    logging::install_panic_hook();
    loc.log();
    let key_path = default_key_path(Some(&loc.workspace));
    let key =
        || -> KeySource { KeySource::File(KeyFile::open(key_path.clone().unwrap_or_default())) };

    // Single instance: a second launch asks the running one to show itself (or to be managed).
    let instance = SingleInstance::acquire(&names.ui);
    if !matches!(&instance, Ok(i) if !i.already_running()) {
        let pipe = stintmate_ipc::naming::pipe_path(crate::ENDPOINT_ID, None).unwrap_or_default();
        let (method, params) = match args.parent_pid.filter(|_| args.managed) {
            Some(pid) => ("control.managed", serde_json::json!({"parent_pid": pid})),
            None => ("control.show", serde_json::json!({})),
        };
        let reply = crate::instance::call(&pipe, key, method, &params, Duration::from_secs(3));
        tracing::info!("The AMS2 connector app is already running; {method} (reply {reply:?})");
        return 0;
    }
    crate::descriptor::refresh(&loc.data_dir, &ui_exe, &names.ui);

    let recorder_exe = ui_exe.with_file_name("ams2-recorder.exe");
    let app = ControlApp::new(Environment {
        workspace: loc.workspace.clone(),
        data_dir: loc.data_dir.clone(),
        data_source: loc.describe(),
        workspace_args: loc.child_args(),
        ui_exe: ui_exe.clone(),
        recorder_exe,
        recorder_mutex: names.recorder.clone(),
        recorder_stop: names.recorder_stop.clone(),
        recorder_extra_args: Vec::new(),
        stop_timeout: crate::services::STOP_TIMEOUT,
        host: Box::new(WinHost::default()),
        autostart: Autostart::new(Box::new(WinRunKey::default()), &ui_exe),
        texts: Texts::load(Some(&loc.workspace)),
        parent_watch: Box::new(crate::managed::ParentWatch::new),
        tray_available: tray_available(),
        time: None,
    });
    let events = app.take_events();
    let serve = ServeConfig {
        pipe: None,
        registry: None,
        key: key(),
    };
    if let Err(e) = app.serve(serve) {
        tracing::error!(
            "Cannot serve the connector's endpoint ({e}); a managing app and the services cannot reach this app"
        );
    }
    tracing::info!(
        "AMS2 connector app started (pid {}, managed={:?}, started by {})",
        std::process::id(),
        args.parent_pid.filter(|_| args.managed),
        ams2_platform::procinfo::describe_process(
            ams2_platform::procinfo::process_chain(std::process::id(), 2)
                .get(1)
                .map(|c| c.0),
            4
        )
    );
    app.start(args.parent_pid.filter(|_| args.managed));
    let ticker = app.spawn_ticker();
    if let Some(events) = events {
        run_ui(&app, events, &ui_exe, &loc, &args);
    }
    let _ = ticker.join();
    tracing::info!("AMS2 connector app exited");
    0
}

#[cfg(windows)]
fn tray_available() -> bool {
    crate::tray::tray_available()
}

#[cfg(not(windows))]
fn tray_available() -> bool {
    false
}

/// The UI of the process: the tray on this thread until the control app quits; the window is
/// another process, started on demand.
#[cfg(windows)]
fn run_ui(
    app: &std::sync::Arc<ControlApp>,
    events: Receiver<UiEvent>,
    ui_exe: &std::path::Path,
    loc: &location::Location,
    args: &Args,
) {
    use crate::window_host::WindowHost;
    let host = std::sync::Arc::new(WindowHost::new(
        WindowHost::sibling_exe(ui_exe),
        loc.child_args(),
    ));
    let show_at_start = !args.tray && !app.config().start_minimized && !args.managed;
    crate::tray::run(crate::tray::TrayRun {
        app: app.clone(),
        events,
        texts: Texts::load(Some(&loc.workspace)),
        host,
        show_at_start,
    });
}

/// Without a tray (not Windows) the events are only drained.
#[cfg(not(windows))]
fn run_ui(
    _app: &std::sync::Arc<ControlApp>,
    events: Receiver<UiEvent>,
    _ui_exe: &std::path::Path,
    _loc: &location::Location,
    _args: &Args,
) {
    for ev in events {
        if ev == UiEvent::Quit {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(a: &[&str]) -> Result<Args, String> {
        parse(a.iter().map(|s| (*s).to_string()))
    }

    #[test]
    fn managed_needs_a_parent_and_the_other_way_round() {
        assert!(p(&["--managed"]).is_err());
        assert!(p(&["--parent-pid", "5"]).is_err());
        assert!(p(&["--managed", "--parent-pid", "0"]).is_err());
        let a = p(&[
            "--managed",
            "--parent-pid",
            "5",
            "--workspace",
            "C:\\w",
            "--allow-synced",
        ])
        .unwrap();
        assert!(a.managed && a.allow_synced);
        assert_eq!(
            (a.parent_pid, a.workspace.as_deref()),
            (Some(5), Some("C:\\w"))
        );
        assert!(p(&["--register", "--managed", "--parent-pid", "5"]).is_err());
        assert!(p(&["--register"]).unwrap().register);
        assert!(p(&["--tray"]).unwrap().tray);
        assert!(p(&["--bogus"]).is_err());
    }
}
