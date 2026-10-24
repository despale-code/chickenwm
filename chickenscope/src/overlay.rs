//! Phase 7.1 & 7.2: In-game transparent Vulkan HUD overlay (`Super + Tab`)
//! with click interception when active and instant zero-latency raw focus return to game on dismiss.

use crate::passthrough::ACTIVE_GAME_HWND;
use anyhow::Result;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT, MOD_SHIFT, MOD_WIN, VK_G,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, GetSystemMetrics, GetWindowLongW, RegisterClassW,
    SetForegroundWindow, SetLayeredWindowAttributes, SetWindowLongW, SetWindowPos, ShowWindow,
    GWL_EXSTYLE, HWND_TOPMOST, LWA_ALPHA, SM_CXSCREEN, SM_CYSCREEN, SWP_NOMOVE, SWP_NOSIZE,
    SW_HIDE, SW_SHOW, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT,
    WS_POPUP,
};

pub const HOTKEY_ID_OVERLAY_TOGGLE: i32 = 0x7E01; // Super + Shift + G
pub const HOTKEY_ID_GAMEMODE_TOGGLE: i32 = 0x7E02; // Super + G

pub static OVERLAY_VISIBLE: AtomicBool = AtomicBool::new(false);
static OVERLAY_HWND: AtomicIsize = AtomicIsize::new(0);

unsafe extern "system" fn hud_overlay_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Controls the in-game transparent HUD overlay.
pub struct GameOverlay;

impl GameOverlay {
    /// Phase 7.1: Registers `Super + Shift + G` (HUD overlay toggle) and `Super + G` (manual Game Mode toggle)
    /// and initializes the hidden topmost overlay window.
    pub fn init() -> Result<()> {
        unsafe {
            let _ = RegisterHotKey(
                None,
                HOTKEY_ID_OVERLAY_TOGGLE,
                MOD_WIN | MOD_SHIFT | MOD_NOREPEAT,
                VK_G.0 as u32,
            );
            let _ = RegisterHotKey(
                None,
                HOTKEY_ID_GAMEMODE_TOGGLE,
                MOD_WIN | MOD_NOREPEAT,
                VK_G.0 as u32,
            );

            let hinstance = GetModuleHandleW(None)?;
            let class_name: Vec<u16> = "ChickenScopeHudOverlayClass\0".encode_utf16().collect();
            let wc = WNDCLASSW {
                lpfnWndProc: Some(hud_overlay_wnd_proc),
                hInstance: HINSTANCE(hinstance.0),
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            let _ = RegisterClassW(&wc);

            let w = GetSystemMetrics(SM_CXSCREEN).max(1920);
            let h = GetSystemMetrics(SM_CYSCREEN).max(1080);

            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW,
                PCWSTR(class_name.as_ptr()),
                PCWSTR(class_name.as_ptr()),
                WS_POPUP,
                0,
                0,
                w,
                h,
                None,
                None,
                HINSTANCE(hinstance.0),
                None,
            )?;
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 235, LWA_ALPHA);
            chickenwm_core::sys::apply_acrylic(hwnd, Some(0xBB18_1414));
            OVERLAY_HWND.store(hwnd.0 as isize, Ordering::Release);
        }
        Ok(())
    }

    /// Phase 7.2: Toggles the in-game HUD overlay on/off:
    /// - When toggled ON: removes `WS_EX_TRANSPARENT` to capture mouse/keyboard input on the HUD and prevent clicks leaking into the game.
    /// - When toggled OFF: restores `WS_EX_TRANSPARENT`, hides the HUD, and immediately returns raw foreground focus (`SetForegroundWindow`) to the active game with 0ms Alt-Tab penalty.
    pub fn toggle() -> Result<()> {
        let raw_hwnd = OVERLAY_HWND.load(Ordering::Acquire);
        if raw_hwnd == 0 {
            Self::init()?;
        }
        let hwnd = HWND(OVERLAY_HWND.load(Ordering::Acquire) as *mut core::ffi::c_void);
        if hwnd.0.is_null() {
            return Ok(());
        }

        let now_visible = !OVERLAY_VISIBLE.fetch_xor(true, Ordering::AcqRel);
        unsafe {
            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if now_visible {
                let active_style = (ex_style & !WS_EX_TRANSPARENT.0) as i32;
                SetWindowLongW(hwnd, GWL_EXSTYLE, active_style);
                let _ = ShowWindow(hwnd, SW_SHOW);
                let _ = SetWindowPos(hwnd, HWND_TOPMOST, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE);
                let _ = SetForegroundWindow(hwnd);
                tracing::info!("chickenscope HUD overlay activated (input captured by HUD)");
            } else {
                let passthrough_style = (ex_style | WS_EX_TRANSPARENT.0) as i32;
                SetWindowLongW(hwnd, GWL_EXSTYLE, passthrough_style);
                let _ = ShowWindow(hwnd, SW_HIDE);

                let game_raw = ACTIVE_GAME_HWND.load(Ordering::Acquire);
                if game_raw != 0 {
                    let game_hwnd = HWND(game_raw as *mut core::ffi::c_void);
                    let _ = SetForegroundWindow(game_hwnd);
                }
                tracing::info!("chickenscope HUD overlay dismissed (instant raw focus returned to game)");
            }
        }
        Ok(())
    }

    pub fn shutdown() {
        unsafe {
            let _ = UnregisterHotKey(None, HOTKEY_ID_OVERLAY_TOGGLE);
            let _ = UnregisterHotKey(None, HOTKEY_ID_GAMEMODE_TOGGLE);
        }
    }
}
