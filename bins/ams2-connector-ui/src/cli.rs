//! The command line and the process's life: `--register`, workspace errors, single instance,
//! the second launch's call, and the headless run until a UI (U1) is attached.

use crate::api::UiEvent;
use crate::app::{ControlApp, Environment};
use crate::autostart::{Autostart, WinRunKey};
use crate::endpoint::ServeConfig;
use crate::instance::CallFailure;
use crate::services::WinHost;
use crate::texts::Texts;
use ams2_platform::location::{self, WORKSPACE_ERROR_EXIT, WorkspaceArgs};
use ams2_platform::logging::{self, LogConfig};
use ams2_platform::single::{self, Names, SingleInstance};
use ams2_platform::workspace::SystemProbes;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};
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
    /// `--check` (with `--register`): only check that the descriptor can be written.
    pub check: bool,
    /// `--unregister`: remove the descriptor and exit.
    pub unregister: bool,
    /// `--quit`: ask the running instance to exit.
    pub quit: bool,
    /// `--version`: print the version and exit.
    pub version: bool,
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
            "--check" => a.check = true,
            "--unregister" => a.unregister = true,
            "--quit" => a.quit = true,
            "--version" => a.version = true,
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
    if a.check && !a.register {
        return Err("--check needs --register".into());
    }
    let commands = [
        ("--register", a.register),
        ("--unregister", a.unregister),
        ("--quit", a.quit),
        ("--version", a.version),
    ];
    let asked: Vec<&str> = commands
        .iter()
        .filter(|(_, on)| *on)
        .map(|(n, _)| *n)
        .collect();
    if asked.len() > 1 {
        return Err(format!("{} cannot be combined", asked.join(" and ")));
    }
    if let Some(which) = asked.first()
        && (a.managed || a.tray)
    {
        return Err(format!(
            "{which} starts nothing: not with --managed or --tray"
        ));
    }
    Ok(a)
}

/// The usage message: every option.
pub const USAGE: &str = "usage: ams2-connector-ui [--tray] [--managed --parent-pid <pid>] [--workspace <dir>] [--allow-synced]
       ams2-connector-ui --register   [--check] [--workspace <dir>] [--allow-synced]
       ams2-connector-ui --unregister [--workspace <dir>] [--allow-synced]
       ams2-connector-ui --quit
       ams2-connector-ui --version
  --tray            start hidden in the tray
  --managed         run for a managing app (no tray icon); needs --parent-pid <pid>
  --register        write <workspace>\\connectors\\ams2\\connector.json and exit
  --check           with --register: only check that the descriptor can be written
  --unregister      remove that descriptor (keeps logs, outbox and settings) and exit
  --quit            ask the running app to exit (exit 4 if a managing app owns it)
  --version         print the version and exit
  --workspace <dir> the Stintmate workspace; --allow-synced accepts one inside OneDrive
exit status: 0 done, 1 failed, 2 bad arguments, 3 workspace error, 4 managed (--quit)";

/// Exit status of `--quit` when the running app is managed: it must be stopped by its manager.
pub const QUIT_MANAGED_EXIT: i32 = 4;
/// How long `--quit` waits for the connector to stop.
pub const QUIT_WAIT: Duration = Duration::from_secs(10);

fn say(line: &str) {
    println!("{line}");
}

/// `--version`: the line printed (`<exe name> <version>`).
pub fn version_line() -> String {
    format!("{} {}", crate::APP_ID, crate::VERSION)
}

/// `--unregister`: removes this connector's descriptor and nothing else. 0: removed or nothing
/// to remove, 3: a workspace error, 1: the file is not this connector's or cannot be removed.
pub fn unregister(args: &Args) -> i32 {
    let loc = match location::resolve(args.workspace.as_deref(), args.allow_synced, &SystemProbes) {
        Ok(l) => l,
        Err(e) => {
            say(&e.line());
            return WORKSPACE_ERROR_EXIT;
        }
    };
    match crate::descriptor::remove(&loc.data_dir) {
        Ok(Some(p)) => {
            say(&format!("unregistered: {}", p.display()));
            0
        }
        Ok(None) => {
            say("nothing to remove: the connector is not registered in this workspace");
            0
        }
        Err(e) => {
            say(&format!("cannot remove the connector descriptor: {e}"));
            1
        }
    }
}

/// `--quit`: asks the running control app (found by the mutex `names.ui`) to exit through its
/// endpoint's `control.quit`, and a recorder left running alone through its stop event, then
/// waits up to `wait` for both to be gone. 0: nothing runs anymore (or nothing ran), 1: still
/// running or unreachable, 4: the app is managed (it refuses; its managing app must stop it).
pub fn quit(names: &Names, pipe: &str, key: impl Fn() -> KeySource, wait: Duration) -> i32 {
    let ui = || single::is_running(&names.ui);
    let recorder = || single::is_running(&names.recorder);
    if !ui() && !recorder() {
        say("The AMS2 connector is not running.");
        return 0;
    }
    if ui() {
        let reply = crate::instance::call_checked(
            pipe,
            key,
            "control.quit",
            &serde_json::json!({}),
            wait.min(Duration::from_secs(3)),
        );
        match reply {
            Ok(_) => {}
            Err(CallFailure::Remote { code, message }) if code == "managed" => {
                say(&format!("not quit: {message}"));
                return QUIT_MANAGED_EXIT;
            }
            Err(CallFailure::Remote { message, .. }) => {
                say(&format!("the running app refused to quit: {message}"));
                return 1;
            }
            Err(CallFailure::Unreachable) => {
                say("the running app does not answer on its endpoint");
                return 1;
            }
        }
    }
    let deadline = Instant::now() + wait;
    let mut recorder_asked = false;
    loop {
        if !ui() && recorder() && !recorder_asked {
            // The app is gone but a recorder started on its own is not: its stop event.
            single::signal_stop(&names.recorder_stop);
            recorder_asked = true;
        }
        if !ui() && !recorder() {
            say("The AMS2 connector has stopped.");
            return 0;
        }
        if Instant::now() >= deadline {
            say("The AMS2 connector is still running.");
            return 1;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `--register --check`: resolves the workspace and checks the descriptor can be written (the
/// folder `connectors\ams2` is a folder or can be created, and a test file can be created and
/// removed there); writes nothing else. 0 ok, 3 a workspace error, 1 cannot write.
pub fn check_register(args: &Args) -> i32 {
    let loc = match location::resolve(args.workspace.as_deref(), args.allow_synced, &SystemProbes) {
        Ok(l) => l,
        Err(e) => {
            say(&e.line());
            return WORKSPACE_ERROR_EXIT;
        }
    };
    let probe = loc
        .data_dir
        .join(format!(".check-{}.tmp", std::process::id()));
    let result = std::fs::create_dir_all(&loc.data_dir)
        .and_then(|()| std::fs::write(&probe, b"check"))
        .and_then(|()| std::fs::remove_file(&probe));
    match result {
        Ok(()) => {
            say(&format!("ok: {}", loc.data_dir.display()));
            0
        }
        Err(e) => {
            let _ = std::fs::remove_file(&probe);
            say(&format!("cannot write in {}: {e}", loc.data_dir.display()));
            1
        }
    }
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
            attach_console();
            eprintln!("ams2-connector-ui: {m}\n{USAGE}");
            return 2;
        }
    };
    let names = Names::default();
    let ui_exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ams2-connector-ui.exe"));
    if args.version || args.register || args.unregister || args.quit {
        attach_console();
    }
    if args.version {
        say(&version_line());
        return 0;
    }
    if args.unregister {
        return unregister(&args);
    }
    if args.quit {
        return run_quit(&args, &names);
    }
    if args.register && args.check {
        return check_register(&args);
    }
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
        let pipe = ams2_platform::dev::pipe_path(crate::ENDPOINT_ID).unwrap_or_default();
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

/// Makes the output of the command-line options reach the console that started this
/// GUI-subsystem exe (when its output is not redirected).
fn attach_console() {
    #[cfg(windows)]
    ams2_platform::console::attach_parent_output();
}

/// `--quit` against the real names, the endpoint and the workspace's key.
fn run_quit(args: &Args, names: &Names) -> i32 {
    let workspace = location::resolve(args.workspace.as_deref(), args.allow_synced, &SystemProbes)
        .ok()
        .map(|l| l.workspace);
    let key_path = default_key_path(workspace.as_deref());
    let pipe = ams2_platform::dev::pipe_path(crate::ENDPOINT_ID).unwrap_or_default();
    quit(
        names,
        &pipe,
        || KeySource::File(KeyFile::open(key_path.clone().unwrap_or_default())),
        QUIT_WAIT,
    )
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
        assert!(p(&["--register", "--check"]).unwrap().check);
        assert!(p(&["--check"]).is_err());
        assert!(p(&["--tray"]).unwrap().tray);
        assert!(p(&["--bogus"]).is_err());
    }

    #[test]
    fn the_command_options_are_alternatives_and_start_nothing() {
        assert!(p(&["--unregister"]).unwrap().unregister);
        assert!(p(&["--quit"]).unwrap().quit);
        assert!(p(&["--version"]).unwrap().version);
        assert!(p(&["--unregister", "--workspace", "C:/w"]).is_ok());
        assert!(p(&["--register", "--quit"]).is_err());
        assert!(p(&["--unregister", "--managed", "--parent-pid", "5"]).is_err());
        assert!(p(&["--quit", "--tray"]).is_err());
        assert_eq!(
            version_line(),
            format!("ams2-connector-ui {}", env!("CARGO_PKG_VERSION"))
        );
    }
}
