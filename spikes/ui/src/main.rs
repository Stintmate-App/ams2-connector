//! U0 UI spike (PRY-057 D11): eframe/egui + tray-icon + muda, driven by timers.
//!
//! Usage: `ams2-u0-ui-spike <results.json> <log-file> [idle_secs]`. The spike starts with the
//! window hidden, drives its own checks from a thread, writes the results as JSON and exits.
//! It only ever touches its own window and tray icon.

use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU64, Ordering::SeqCst};
use std::sync::{LazyLock, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

use eframe::egui;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use serde_json::{Map, Value, json};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};
use windows_sys::Win32::Foundation::{FILETIME, HWND, RECT};
use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
use windows_sys::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, IsWindowVisible, PostMessageW, WM_CLOSE, WM_COMMAND,
};

/// Command ids muda hands out sequentially from 1000 (Open, Log, Exit are created in that order).
const START_HIDDEN: bool = true;
const CMD_SHOW: usize = 1000;
const CMD_LOG: usize = 1001;
const CMD_EXIT: usize = 1002;

static HWND_RAW: AtomicIsize = AtomicIsize::new(0);
static TRAY_HWND: AtomicIsize = AtomicIsize::new(0);
static EXITING: AtomicBool = AtomicBool::new(false);
static FRAMES: AtomicU64 = AtomicU64::new(0);
static COUNTER: AtomicU64 = AtomicU64::new(0);
static PAINTED: AtomicU64 = AtomicU64::new(0);
static CTX: OnceLock<egui::Context> = OnceLock::new();
static RESULTS: LazyLock<Mutex<Map<String, Value>>> = LazyLock::new(|| Mutex::new(Map::new()));
static LOG_PATH: OnceLock<String> = OnceLock::new();
static LOG_TX: Mutex<Option<mpsc::Sender<Instant>>> = Mutex::new(None);

fn put(key: &str, value: Value) {
    RESULTS.lock().unwrap().insert(key.to_string(), value);
}

fn hwnd() -> HWND {
    HWND_RAW.load(SeqCst) as HWND
}

fn visible() -> bool {
    unsafe { IsWindowVisible(hwnd()) != 0 }
}

/// Kernel + user CPU time of this process, in milliseconds.
fn cpu_ms() -> f64 {
    let z = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let (mut c, mut e, mut k, mut u) = (z, z, z, z);
    unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) };
    let t = |f: FILETIME| {
        ((u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)) as f64 / 10_000.0
    };
    t(k) + t(u)
}

fn working_set_mb() -> f64 {
    let mut m: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
    m.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
    unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut m, m.cb) };
    m.WorkingSetSize as f64 / 1_048_576.0
}

fn post_cmd(id: usize) {
    unsafe { PostMessageW(TRAY_HWND.load(SeqCst) as HWND, WM_COMMAND, id, 0) };
}

fn finish(path: &str) {
    let m = RESULTS.lock().unwrap().clone();
    std::fs::write(
        path,
        serde_json::to_string_pretty(&Value::Object(m)).unwrap(),
    )
    .unwrap();
}

fn registry_light() -> Value {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let sub: Vec<u16> = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize\0"
        .encode_utf16()
        .collect();
    let name: Vec<u16> = "AppsUseLightTheme\0".encode_utf16().collect();
    let (mut v, mut n) = (0u32, 4u32);
    let r = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            sub.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut v as *mut u32).cast(),
            &mut n,
        )
    };
    if r == 0 { json!(v) } else { Value::Null }
}

struct App {
    first_frame_done: bool,
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let n = FRAMES.fetch_add(1, SeqCst);
        if n < 3 && START_HIDDEN {
            // eframe 0.37 shows the window after the first painted frame whatever the builder said.
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        PAINTED.store(COUNTER.load(SeqCst), SeqCst);
        if !EXITING.load(SeqCst) && ctx.input(|i| i.viewport().close_requested()) {
            // Close-to-tray: keep the process, hide the window.
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        if !self.first_frame_done {
            self.first_frame_done = true;
            let (native, ppp) =
                ctx.input(|i| (i.viewport().native_pixels_per_point, i.pixels_per_point));
            put(
                "first_frame",
                json!({
                    "pixels_per_point": ppp,
                    "native_pixels_per_point": native,
                    "win32_dpi": unsafe { GetDpiForWindow(hwnd()) },
                    "system_theme": format!("{:?}", ctx.system_theme()),
                    "dark_mode_visuals": ctx.global_style().visuals.dark_mode,
                    "registry_apps_use_light_theme": registry_light(),
                }),
            );
        }
        ui.heading("U0 spike");
        ui.label(format!("counter {}", COUNTER.load(SeqCst)));
        ui.label(format!("frames {}", FRAMES.load(SeqCst)));
    }
}

fn idle_block(label: &str, idle: Duration) {
    let (c0, f0) = (cpu_ms(), FRAMES.load(SeqCst));
    let w = Instant::now();
    std::thread::sleep(idle);
    let wall = w.elapsed().as_secs_f64() * 1000.0;
    let c1 = cpu_ms();
    put(
        label,
        json!({ "wall_ms": wall, "cpu_ms": c1 - c0, "cpu_percent_of_one_core": (c1 - c0) / wall * 100.0,
                "egui_frames": FRAMES.load(SeqCst) - f0, "working_set_mb": working_set_mb() }),
    );
}

fn driver(results_path: String, idle: Duration) {
    // Wait for the creation context to publish the HWND.
    while HWND_RAW.load(SeqCst) == 0 || CTX.get().is_none() || TRAY_HWND.load(SeqCst) == 0 {
        std::thread::sleep(Duration::from_millis(1));
    }
    let ctx = CTX.get().unwrap().clone();
    // 1. No flash: poll visibility every 2 ms for 1.5 s.
    let t0 = Instant::now();
    let mut visible_polls = 0u32;
    let mut rect_when_visible = Value::Null;
    while t0.elapsed() < Duration::from_millis(1500) {
        if visible() {
            visible_polls += 1;
            let mut r: RECT = unsafe { std::mem::zeroed() };
            unsafe { GetWindowRect(hwnd(), &mut r) };
            rect_when_visible = json!([r.left, r.top, r.right, r.bottom]);
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    put(
        "hidden_start",
        json!({ "visible_polls_of_2ms_in_first_1500ms": visible_polls, "window_rect_when_visible": rect_when_visible, "update_frames_so_far": FRAMES.load(SeqCst) }),
    );

    // 2. Tray menu events while hidden (WM_COMMAND posted to the tray's own window).
    let (tx, rx) = mpsc::channel::<Instant>();
    *LOG_TX.lock().unwrap() = Some(tx);
    let observer = std::thread::spawn(move || {
        let mut lat = Vec::new();
        while let Ok(sent) = rx.recv_timeout(Duration::from_millis(500)) {
            lat.push(sent.elapsed().as_micros() as u64);
        }
        lat
    });
    let frames_before = FRAMES.load(SeqCst);
    for _ in 0..5 {
        post_cmd(CMD_LOG);
        std::thread::sleep(Duration::from_millis(100));
    }
    let lat = observer.join().unwrap();
    let logged = std::fs::read_to_string(LOG_PATH.get().unwrap())
        .unwrap_or_default()
        .lines()
        .count();
    put(
        "tray_events_while_hidden",
        json!({ "posted": 5, "observed_by_background_thread": lat.len(), "log_lines": logged,
                "handler_to_observer_us": lat,
                "egui_frames_during": FRAMES.load(SeqCst) - frames_before, "window_visible": visible() }),
    );

    // 3. Idle cost, hidden.
    idle_block("idle_hidden", idle);

    // 4. Show from the tray menu while no frame runs.
    let s = Instant::now();
    post_cmd(CMD_SHOW);
    while !visible() && s.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(1));
    }
    put(
        "show_from_tray_while_hidden",
        json!({ "visible": visible(), "latency_ms": s.elapsed().as_secs_f64() * 1000.0,
                "frames_after": FRAMES.load(SeqCst) }),
    );
    std::thread::sleep(Duration::from_millis(500));

    // 5. request_repaint from another thread.
    let f1 = FRAMES.load(SeqCst);
    let ticker = std::thread::spawn({
        let ctx = ctx.clone();
        move || {
            for _ in 0..60 {
                COUNTER.fetch_add(1, SeqCst);
                ctx.request_repaint();
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    });
    ticker.join().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    put(
        "request_repaint_from_thread",
        json!({ "ticks": COUNTER.load(SeqCst), "last_painted_counter": PAINTED.load(SeqCst),
                "frames": FRAMES.load(SeqCst) - f1 }),
    );

    // 6. Idle cost, shown.
    std::thread::sleep(Duration::from_secs(1));
    idle_block("idle_shown", idle);

    // 7. Close-to-tray: WM_CLOSE on our own window.
    unsafe { PostMessageW(hwnd(), WM_CLOSE, 0, 0) };
    let s = Instant::now();
    while visible() && s.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(5));
    }
    put(
        "close_to_tray",
        json!({ "hidden_after_close": !visible(), "latency_ms": s.elapsed().as_secs_f64() * 1000.0,
                "process_alive": true }),
    );

    // 8. Exit from the tray menu while hidden; force-exit if the event loop ignores it.
    finish(&results_path);
    post_cmd(CMD_EXIT);
    std::thread::sleep(Duration::from_secs(3));
    put("exit_while_hidden_needed_force", json!(true));
    finish(&results_path);
    std::process::exit(0);
}

fn tray_icon() -> Icon {
    let mut rgba = Vec::new();
    for _ in 0..16 * 16 {
        rgba.extend_from_slice(&[0x20, 0x90, 0xe0, 0xff]);
    }
    Icon::from_rgba(rgba, 16, 16).unwrap()
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let results_path = args.get(1).cloned().expect("results path");
    LOG_PATH
        .set(args.get(2).cloned().expect("log path"))
        .unwrap();
    let idle = Duration::from_secs(args.get(3).and_then(|s| s.parse().ok()).unwrap_or(60));
    std::fs::write(LOG_PATH.get().unwrap(), "").unwrap();
    put(
        "renderer",
        json!(if cfg!(feature = "wgpu") {
            "wgpu"
        } else {
            "glow"
        }),
    );
    put("pid", json!(std::process::id()));

    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_visible(false)
            .with_position([-32000.0, -32000.0])
            .with_inner_size([420.0, 240.0])
            .with_title("U0 spike"),
        ..Default::default()
    };
    let rp = results_path.clone();
    let res = eframe::run_native(
        "u0-spike",
        opts,
        Box::new(move |cc| {
            if let Ok(h) = cc.window_handle()
                && let RawWindowHandle::Win32(w) = h.as_raw()
            {
                HWND_RAW.store(w.hwnd.get(), SeqCst);
            }
            #[cfg(feature = "glow")]
            put("gl_context_exists_while_hidden", json!(cc.gl.is_some()));
            #[cfg(feature = "wgpu")]
            put(
                "gpu_context_exists_while_hidden",
                json!(cc.wgpu_render_state.is_some()),
            );
            let ctx = cc.egui_ctx.clone();
            CTX.set(ctx.clone()).ok();

            let open = MenuItem::new("Open", true, None);
            let log = MenuItem::new("Log", true, None);
            let exit = MenuItem::new("Exit", true, None);
            let menu = Menu::new();
            menu.append_items(&[&open, &log, &exit]).unwrap();
            let (open_id, log_id) = (open.id().clone(), log.id().clone());
            MenuEvent::set_event_handler(Some(move |ev: MenuEvent| {
                if ev.id == open_id {
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                        200.0, 200.0,
                    )));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    ctx.request_repaint();
                } else if ev.id == log_id {
                    let mut f = std::fs::OpenOptions::new()
                        .append(true)
                        .open(LOG_PATH.get().unwrap())
                        .unwrap();
                    writeln!(f, "menu item Log").unwrap();
                    if let Some(tx) = LOG_TX.lock().unwrap().as_ref() {
                        let _ = tx.send(Instant::now());
                    }
                } else {
                    EXITING.store(true, SeqCst);
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    ctx.request_repaint();
                }
            }));
            let tray = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("U0 spike")
                .with_icon(tray_icon())
                .build()
                .unwrap();
            TRAY_HWND.store(tray.window_handle() as isize, SeqCst);
            std::mem::forget(tray); // lives for the whole process
            std::thread::spawn(move || driver(rp, idle));
            Ok(Box::new(App {
                first_frame_done: false,
            }))
        }),
    );
    // Reached only when the loop stopped on its own.
    put("exit_while_hidden_needed_force", json!(false));
    finish(&results_path);
    res
}
