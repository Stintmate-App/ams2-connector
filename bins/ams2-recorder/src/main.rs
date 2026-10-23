//! `ams2-recorder.exe`: reads the game at 50 Hz and records through Pitwall.

use ams2_platform::location::{self, WORKSPACE_ERROR_EXIT, WorkspaceArgs};
use ams2_platform::logging::{self, LogConfig};
use ams2_platform::single::{Names, SingleInstance, StopEvent};
use ams2_platform::workspace::SystemProbes;
use ams2_recorder::recorder::{Config, Overrides, Recorder};

fn main() {
    let args = WorkspaceArgs::parse(std::env::args().skip(1));
    let mut hz = 50.0;
    let mut pitwall_pipe = None;
    let mut control_pipe = None;
    let mut it = args.rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--hz" => hz = it.next().and_then(|v| v.parse().ok()).unwrap_or(hz),
            "--pitwall-pipe" => pitwall_pipe = it.next().cloned(),
            "--control-pipe" => control_pipe = it.next().cloned(),
            other => {
                eprintln!("ams2-recorder: unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let loc = match location::resolve(args.workspace.as_deref(), args.allow_synced, &SystemProbes) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}", e.line());
            std::process::exit(WORKSPACE_ERROR_EXIT);
        }
    };
    let _ = std::fs::create_dir_all(&loc.data_dir);
    let _guard = logging::init(&LogConfig::new(&loc.data_dir, "recorder"));
    logging::install_panic_hook();
    loc.log();
    let names = Names::default();
    let instance = match SingleInstance::acquire(&names.recorder) {
        Ok(i) if !i.already_running() => i,
        _ => {
            tracing::error!("Another recorder is already running; exiting");
            std::process::exit(1);
        }
    };
    let Ok(stop) = StopEvent::create(&names.recorder_stop) else {
        tracing::error!("Cannot create the stop event");
        std::process::exit(1);
    };
    let cfg = Config {
        hz,
        workspace: loc.workspace.clone(),
        data_dir: loc.data_dir.clone(),
        pitwall_pipe,
        control_pipe,
        no_control: false,
        shm_name: None,
    };
    match Recorder::build(cfg, Overrides::default()) {
        Ok(mut rec) => rec.run(&|| stop.is_set()),
        Err(e) => {
            tracing::error!("Cannot start: {e}");
            std::process::exit(1);
        }
    }
    drop(instance);
}
