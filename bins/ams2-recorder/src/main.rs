//! `ams2-recorder.exe`: reads the game at 50 Hz and records through Pitwall.

use ams2_platform::game::GameWatch;
use ams2_platform::location::{self, WORKSPACE_ERROR_EXIT, WorkspaceArgs};
use ams2_platform::logging::{self, LogConfig};
use ams2_platform::single::{Names, SingleInstance, StopEvent};
use ams2_platform::workspace::SystemProbes;
use ams2_recorder::cli;
use ams2_recorder::recorder::{Config, Overrides, Recorder};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        println!("ams2-recorder {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let args = WorkspaceArgs::parse(std::env::args().skip(1));
    if let Some(sub) = args
        .rest
        .first()
        .filter(|s| cli::SUBCOMMANDS.contains(&s.as_str()))
    {
        std::process::exit(subcommand(sub, &args));
    }
    let mut hz = 50.0;
    let mut pitwall_pipe = None;
    let mut control_pipe = None;
    let mut shm_name: Option<String> = None;
    let mut it = args.rest.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--hz" => hz = it.next().and_then(|v| v.parse().ok()).unwrap_or(hz),
            "--pitwall-pipe" => pitwall_pipe = it.next().cloned(),
            "--control-pipe" => control_pipe = it.next().cloned(),
            "--shared-memory-name" => shm_name = it.next().cloned(),
            other => {
                eprintln!("ams2-recorder: unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    if let Some(n) = &shm_name
        && (n.is_empty() || n == ams2_shm::MAPPING_NAME || n.contains("pcars2"))
    {
        eprintln!("ams2-recorder: --shared-memory-name needs a test mapping, not the game's");
        std::process::exit(2);
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
        shm_name: shm_name.clone(),
    };
    // A replayed session has no game process: with a test mapping the game counts as running.
    let overrides = if shm_name.is_some() {
        Overrides {
            game: Some(GameWatch::with(
                || true,
                Instant::now,
                Duration::from_secs(5),
            )),
            game_check: Some(Arc::new(|| true)),
            ..Overrides::default()
        }
    } else {
        Overrides::default()
    };
    ams2_platform::proc::lower_process_priority();
    match Recorder::build(cfg, overrides) {
        Ok(mut rec) => rec.run(&|| stop.is_set()),
        Err(e) => {
            tracing::error!("Cannot start: {e}");
            std::process::exit(1);
        }
    }
    drop(instance);
}

/// `announce`, `codes`, `diagnose`, `capture-raw`: print to stdout, exit with the code.
fn subcommand(sub: &str, args: &WorkspaceArgs) -> i32 {
    let rest = &args.rest[1..];
    let mut out = |line: String| println!("{line}");
    let shm_name = || {
        rest.iter()
            .position(|a| a == "--shm-name")
            .and_then(|i| rest.get(i + 1))
            .cloned()
    };
    match sub {
        "codes" => {
            cli::codes(&mut out);
            0
        }
        "diagnose" => cli::diagnose(shm_name().as_deref(), &mut out),
        "announce" => {
            match location::resolve(args.workspace.as_deref(), args.allow_synced, &SystemProbes) {
                Ok(loc) => cli::announce_main(rest, &loc.workspace, &mut out),
                Err(e) => {
                    eprintln!("{}", e.line());
                    WORKSPACE_ERROR_EXIT
                }
            }
        }
        #[cfg(feature = "capture-raw")]
        "capture-raw" => ams2_recorder::capture_raw::run(rest, &mut out),
        _ => {
            eprintln!("ams2-recorder: {sub} is not available in this build");
            2
        }
    }
}
