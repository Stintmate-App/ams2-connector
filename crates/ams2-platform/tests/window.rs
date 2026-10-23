//! The capture target's window search (port of `test_capture_target.py`). Windows are made by
//! the test itself, never shown, and the listing is restricted to this process: no other
//! window is read.
#![cfg(windows)]

use ams2_platform::window::{Scope, WINDOW_CLASS, WindowInfo, find_game_window, list_windows};
use std::ptr::null;
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, WNDCLASSW, WS_OVERLAPPEDWINDOW,
};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

/// A hidden window of this process; destroyed on drop (same thread).
struct TestWindow(HWND);

impl TestWindow {
    fn new(class: &str, title: &str) -> Self {
        let (c, t) = (wide(class), wide(title));
        // SAFETY: plain window-class and window creation with NUL-terminated strings.
        unsafe {
            let hinst = GetModuleHandleW(null());
            let mut wc: WNDCLASSW = core::mem::zeroed();
            wc.lpfnWndProc = Some(DefWindowProcW);
            wc.hInstance = hinst;
            wc.lpszClassName = c.as_ptr();
            RegisterClassW(&wc); // a second registration of the same class fails: harmless
            let h = CreateWindowExW(
                0,
                c.as_ptr(),
                t.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                0,
                0,
                640,
                480,
                core::ptr::null_mut(),
                core::ptr::null_mut(),
                hinst,
                null(),
            );
            assert!(!h.is_null(), "test window");
            Self(h)
        }
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        // SAFETY: the window was created on this thread.
        unsafe { DestroyWindow(self.0) };
    }
}

fn info(exe: &str, title: &str, class: &str, area: i64, hwnd: i64) -> WindowInfo {
    WindowInfo {
        hwnd,
        pid: 1,
        exe: exe.into(),
        title: title.into(),
        class: class.into(),
        area,
        minimized: false,
    }
}

#[test]
fn a_window_with_the_games_title_and_class_is_listed_and_found() {
    let _s = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _decoy = TestWindow::new("Stintmate.TestDecoy", "Automobilista 2");
    let game = TestWindow::new(WINDOW_CLASS, "Automobilista 2");
    let all = list_windows(Scope::OwnProcess);
    assert!(all.iter().all(|w| w.pid == std::process::id()));
    let found = find_game_window(&all).expect("found by title and class");
    assert_eq!(found.hwnd, game.0 as isize as i64);
    assert_eq!(
        (found.title.as_str(), found.class.as_str()),
        ("Automobilista 2", WINDOW_CLASS)
    );
    assert!(found.area > 0 && !found.minimized);
}

#[test]
fn no_window_of_the_game_is_none() {
    let _s = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let _decoy = TestWindow::new("Stintmate.TestDecoy2", "Automobilista 2");
    assert!(find_game_window(&list_windows(Scope::OwnProcess)).is_none());
}

#[test]
fn the_process_name_wins_and_the_largest_area_breaks_ties() {
    let ws = vec![
        info("other.exe", "Automobilista 2", WINDOW_CLASS, 9_000_000, 1),
        info("ams2.exe", "x", "c", 100, 2),
        info("ams2avx.exe", "y", "c", 500, 3),
        info("ams2avx.exe", "z", "c", 500, 4),
    ];
    assert_eq!(find_game_window(&ws).unwrap().hwnd, 3); // by exe, largest, lowest handle
    // by title and class only when no process matches; the title is trimmed, any case
    let ws = vec![
        info("browser.exe", "Automobilista 2", "Chrome_WidgetWin_1", 9, 1),
        info("x.exe", "  automobilista 2 ", WINDOW_CLASS, 10, 2),
        info("y.exe", "Automobilista 2", WINDOW_CLASS, 20, 3),
    ];
    assert_eq!(find_game_window(&ws).unwrap().hwnd, 3);
    assert!(find_game_window(&[]).is_none());
}
