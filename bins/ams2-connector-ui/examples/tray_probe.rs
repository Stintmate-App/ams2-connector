//! Measures the tray process: runs the real tray on a random pipe in a temporary workspace
//! (never the driver's endpoint, mutex or workspace), idles, then opens the window process and
//! measures that too. Prints JSON.
//!
//! `tray_probe <idle_secs> <window-exe> [window_secs]`; run it from a release build. Windows only.

#[cfg(windows)]
fn main() {
    use ams2_connector_ui::api::Command;
    use ams2_connector_ui::app::{ControlApp, Environment};
    use ams2_connector_ui::autostart::{Autostart, MemRunKey};
    use ams2_connector_ui::endpoint::ServeConfig;
    use ams2_connector_ui::managed::ParentWatch;
    use ams2_connector_ui::services::WinHost;
    use ams2_connector_ui::texts::Texts;
    use ams2_connector_ui::tray::{self, TrayRun};
    use ams2_connector_ui::window_host::WindowHost;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Duration;
    use stintmate_ipc::keyfile::{Key, KeySource, format_key};
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    const KEY: Key = [5u8; 32];
    let argv: Vec<String> = std::env::args().collect();
    let idle: u64 = argv.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
    let window_exe = PathBuf::from(argv.get(2).expect("window exe"));
    let window_secs: u64 = argv.get(3).and_then(|s| s.parse().ok()).unwrap_or(10);

    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_path_buf();
    std::fs::create_dir_all(ws.join("ipc")).unwrap();
    let key_file = ws.join("ipc").join("key");
    std::fs::write(&key_file, format_key(&KEY)).unwrap();
    std::fs::write(ws.join("stintmate.json"), r#"{"schema": 1}"#).unwrap();
    let user = format!("probe{}", std::process::id());
    let pipe = stintmate_ipc::naming::pipe_path_for("connector-ams2", None, &user).unwrap();
    let exe = ws.join("ams2-connector-ui.exe");
    let app = ControlApp::new(Environment {
        workspace: ws.clone(),
        data_dir: ws.join("connectors").join("ams2"),
        data_source: "probe".into(),
        workspace_args: vec![],
        ui_exe: exe.clone(),
        recorder_exe: ws.join("ams2-recorder.exe"),
        recorder_mutex: format!("Local\\Probe.{user}.R"),
        recorder_stop: format!("Local\\Probe.{user}.R.Stop"),
        recorder_extra_args: vec![],
        stop_timeout: Duration::from_secs(1),
        host: Box::new(WinHost::default()),
        autostart: Autostart::new(Box::new(MemRunKey::default()), &exe),
        texts: Texts::english(),
        parent_watch: Box::new(ParentWatch::new),
        tray_available: tray::tray_available(),
        time: None,
    });
    let events = app.take_events().unwrap();
    app.serve(ServeConfig {
        pipe: Some(pipe.clone()),
        registry: Some(ws.join("ipc").join("endpoints")),
        key: KeySource::Bytes(KEY),
    })
    .unwrap();
    app.start(None);
    let ticker = app.spawn_ticker();
    let args: Vec<String> = [
        "--workspace",
        &ws.display().to_string(),
        "--pipe",
        &pipe,
        "--key-file",
        &key_file.display().to_string(),
        "--mutex",
        &format!("Local\\Probe.{user}.Window"),
    ]
    .map(String::from)
    .to_vec();
    let host = Arc::new(WindowHost::new(window_exe, args));

    fn cpu_ms(h: windows_sys::Win32::Foundation::HANDLE) -> f64 {
        let z = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut c, mut e, mut k, mut u) = (z, z, z, z);
        unsafe { GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) };
        let t = |f: FILETIME| {
            ((u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)) as f64 / 10_000.0
        };
        t(k) + t(u)
    }
    fn ws_mb(h: windows_sys::Win32::Foundation::HANDLE) -> f64 {
        let mut m: PROCESS_MEMORY_COUNTERS = unsafe { std::mem::zeroed() };
        m.cb = size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        unsafe { K32GetProcessMemoryInfo(h, &mut m, m.cb) };
        m.WorkingSetSize as f64 / 1_048_576.0
    }

    let (app2, host2) = (app.clone(), host.clone());
    let driver = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(3));
        // SAFETY: the pseudo handle of this process.
        let me = unsafe { GetCurrentProcess() };
        let (c0, w0) = (cpu_ms(me), std::time::Instant::now());
        std::thread::sleep(Duration::from_secs(idle));
        let wall = w0.elapsed().as_secs_f64() * 1000.0;
        let tray_cpu = cpu_ms(me) - c0;
        let tray_ws = ws_mb(me);
        // Open the window through the tray's own path (a menu click would send this command).
        app2.dispatch(Command::ShowWindow).unwrap();
        std::thread::sleep(Duration::from_secs(3));
        let pid = host2.pid().unwrap_or(0);
        // SAFETY: a handle to our own child, closed below.
        let h = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        let (wc0, ww0) = (cpu_ms(h), std::time::Instant::now());
        std::thread::sleep(Duration::from_secs(window_secs));
        let wwall = ww0.elapsed().as_secs_f64() * 1000.0;
        let out = serde_json::json!({
            "tray_idle_secs": idle,
            "tray_cpu_ms": tray_cpu,
            "tray_cpu_percent_of_one_core": tray_cpu / wall * 100.0,
            "tray_working_set_mb": tray_ws,
            "window_pid": pid,
            "window_working_set_mb": ws_mb(h),
            "window_idle_cpu_percent_of_one_core": (cpu_ms(h) - wc0) / wwall * 100.0,
            "tray_working_set_mb_with_window_open": ws_mb(me),
        });
        println!("{out}");
        unsafe { CloseHandle(h) };
        app2.exit("probe done");
    });
    tray::run(TrayRun {
        app: app.clone(),
        events,
        texts: Texts::english(),
        host,
        show_at_start: false,
    });
    let _ = driver.join();
    let _ = ticker.join();
}

#[cfg(not(windows))]
fn main() {}
