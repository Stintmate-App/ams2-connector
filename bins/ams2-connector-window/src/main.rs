//! `ams2-connector-window.exe`: the AMS2 connector's window, one process at most.
#![windows_subsystem = "windows"]

use ams2_connector_ui::texts::Texts;
use ams2_connector_window::gui::{Page, Window};
use ams2_connector_window::link::Link;
use ams2_platform::location::{self, WORKSPACE_ERROR_EXIT, WorkspaceArgs};
use ams2_platform::logging::{self, LogConfig};
use ams2_platform::single::{Names, SingleInstance};
use ams2_platform::workspace::SystemProbes;
use eframe::egui;
use stintmate_ipc::keyfile::{KeyFile, KeySource, default_key_path};

/// `--page logs`, and the test overrides `--pipe <path>`, `--key-file <path>` and
/// `--mutex <name>` (a test never touches a running connector's endpoint or mutex).
#[derive(Default)]
struct Args {
    page: Option<String>,
    pipe: Option<String>,
    key_file: Option<String>,
    mutex: Option<String>,
}

fn parse(rest: Vec<String>) -> Result<Args, String> {
    let mut a = Args::default();
    let mut it = rest.into_iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or(format!("{name} needs a value"));
        match arg.as_str() {
            "--page" => a.page = Some(value("--page")?),
            "--pipe" => a.pipe = Some(value("--pipe")?),
            "--key-file" => a.key_file = Some(value("--key-file")?),
            "--mutex" => a.mutex = Some(value("--mutex")?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(a)
}

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--version") {
        ams2_platform::console::attach_parent_output();
        println!("ams2-connector-window {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    std::process::exit(run());
}

fn run() -> i32 {
    let ws = WorkspaceArgs::parse(std::env::args().skip(1));
    let args = match parse(ws.rest.clone()) {
        Ok(a) => a,
        Err(m) => {
            eprintln!("ams2-connector-window: {m}");
            return 2;
        }
    };
    let loc = match location::resolve(ws.workspace.as_deref(), ws.allow_synced, &SystemProbes) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("{}", e.line());
            return WORKSPACE_ERROR_EXIT;
        }
    };
    let _ = std::fs::create_dir_all(&loc.data_dir);
    let _guard = logging::init(&LogConfig::new(&loc.data_dir, "window"));
    logging::install_panic_hook();
    let mutex = args
        .mutex
        .clone()
        .unwrap_or_else(|| Names::default().window);
    let instance = SingleInstance::acquire(&mutex);
    if !matches!(&instance, Ok(i) if !i.already_running()) {
        // The control app tells the running window to come forward (`window_request`).
        tracing::info!("The window is already open");
        return 0;
    }
    let pipe = args.pipe.clone().unwrap_or_else(|| {
        ams2_platform::dev::pipe_path(ams2_connector_ui::ENDPOINT_ID).unwrap_or_default()
    });
    let key_path = args
        .key_file
        .clone()
        .map(std::path::PathBuf::from)
        .or_else(|| default_key_path(Some(&loc.workspace)).ok())
        .unwrap_or_default();
    let key = KeySource::File(KeyFile::open(key_path));
    let ctx_cell: std::sync::Arc<std::sync::OnceLock<egui::Context>> = Default::default();
    let wake_cell = ctx_cell.clone();
    let link = match Link::connect(&pipe, key, move || {
        if let Some(ctx) = wake_cell.get() {
            ctx.request_repaint();
        }
    }) {
        Ok(l) => l,
        Err(e) => {
            tracing::error!("Cannot connect to the control app: {e}");
            return 1;
        }
    };
    let texts = Texts::load(Some(&loc.workspace));
    let page = if args.page.as_deref() == Some("logs") {
        Page::Logs
    } else {
        Page::Services
    };
    let title = texts.s("app.name");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([680.0, 460.0])
            .with_min_inner_size([560.0, 380.0])
            .with_title(&title)
            .with_icon(std::sync::Arc::new(egui::IconData {
                rgba: ams2_connector_ui::icons::artwork_rgba(ams2_connector_ui::icons::WINDOW_SIZE),
                width: ams2_connector_ui::icons::WINDOW_SIZE,
                height: ams2_connector_ui::icons::WINDOW_SIZE,
            })),
        ..Default::default()
    };
    let result = eframe::run_native(
        &title,
        options,
        Box::new(move |cc| {
            ams2_connector_window::theme::apply(&cc.egui_ctx);
            let _ = ctx_cell.set(cc.egui_ctx.clone());
            Ok(Box::new(Window::new(link, texts, page)))
        }),
    );
    if let Err(e) = result {
        tracing::error!("The window failed: {e}");
        return 1;
    }
    0
}
