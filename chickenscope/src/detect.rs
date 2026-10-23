//! Phase 6.1: Game detection heuristics (fullscreen monitor geometry, borderless styles,
//! executable process matchers, and `Super + G` manual toggle override).

use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{CloseHandle, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::ProcessStatus::K32GetModuleBaseNameW;
use windows::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_INFORMATION, PROCESS_VM_READ};
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsWindowVisible, GWL_STYLE,
    WS_CAPTION, WS_POPUP,
};

/// Manual Game Mode toggle flag controlled via `Super + G`.
pub static MANUAL_GAME_MODE_OVERRIDE: AtomicBool = AtomicBool::new(false);

/// Default competitive & AAA game executables automatically matched for DirectFlip passthrough.
pub const KNOWN_GAME_EXECUTABLES: [&str; 12] = [
    "cs2.exe",
    "valorant-win64-shipping.exe",
    "fortniteclient-win64-shipping.exe",
    "r5apex.exe",
    "overwatch.exe",
    "dota2.exe",
    "league of legends.exe",
    "eldenring.exe",
    "cyberpunk2077.exe",
    "baldur's gate 3.exe",
    "bg3_dx11.exe",
    "hl2.exe",
];

/// Queries the lowercase executable file name (e.g. `"cs2.exe"`) of the process owning `hwnd`.
pub fn get_window_exe_name(hwnd: HWND) -> Option<String> {
    if hwnd.0.is_null() {
        return None;
    }
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }
        let proc = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid).ok()?;
        let mut buf = [0u16; 260];
        let len = K32GetModuleBaseNameW(proc, None, &mut buf) as usize;
        let _ = CloseHandle(proc);
        if len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len]).to_ascii_lowercase())
    }
}

/// Detects whether an active window is an exclusive or borderless fullscreen game.
pub struct GameDetector;

impl GameDetector {
    /// Toggles the manual `Super + G` Game Mode override and returns the new state.
    pub fn toggle_manual_override() -> bool {
        let prev = MANUAL_GAME_MODE_OVERRIDE.fetch_xor(true, Ordering::AcqRel);
        !prev
    }

    /// Evaluates whether `hwnd` qualifies for Game Mode DirectFlip passthrough via:
    /// 1. Manual `Super + G` override (`MANUAL_GAME_MODE_OVERRIDE`).
    /// 2. Executable name matching against `KNOWN_GAME_EXECUTABLES` or user `custom_rules`.
    /// 3. Fullscreen borderless geometry (`GetWindowRect == rcMonitor`) without `WS_CAPTION`.
    pub fn is_game(hwnd: HWND) -> bool {
        Self::is_game_with_rules(hwnd, &[])
    }

    pub fn is_game_with_rules(hwnd: HWND, custom_rules: &[String]) -> bool {
        if hwnd.0.is_null() {
            return false;
        }
        if MANUAL_GAME_MODE_OVERRIDE.load(Ordering::Acquire) {
            return true;
        }
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() {
                return false;
            }

            // Heuristic 2: Process name match
            if let Some(exe) = get_window_exe_name(hwnd) {
                if exe.starts_with("chicken")
                    || exe == "coop.exe"
                    || exe == "roobar.exe"
                    || exe == "henbrowser.exe"
                    || exe == "explorer.exe"
                {
                    return false;
                }
                if KNOWN_GAME_EXECUTABLES.iter().any(|&k| k == exe) {
                    return true;
                }
                if custom_rules
                    .iter()
                    .any(|r| exe.contains(&r.to_ascii_lowercase()))
                {
                    return true;
                }
            }

            // Heuristic 1: Window rect equals physical monitor bounds without standard caption
            let mut win_rc = RECT::default();
            if GetWindowRect(hwnd, &mut win_rc).is_err() {
                return false;
            }

            let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
            let mut mi = MONITORINFO {
                cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                ..Default::default()
            };
            if !GetMonitorInfoW(hmon, &mut mi).as_bool() {
                return false;
            }

            let mon_rc = mi.rcMonitor;
            let covers_monitor = win_rc.left == mon_rc.left
                && win_rc.top == mon_rc.top
                && win_rc.right == mon_rc.right
                && win_rc.bottom == mon_rc.bottom;

            if !covers_monitor {
                return false;
            }

            let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
            let has_caption = (style & WS_CAPTION.0) == WS_CAPTION.0;
            let is_popup = (style & WS_POPUP.0) != 0;

            !has_caption || is_popup
        }
    }
}
