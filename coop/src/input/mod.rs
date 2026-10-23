//! Phase 5.1, 5.2 & 8.3: Hardware cursor plane integration, 1000–8000Hz Raw Input atomic mouse decoupling,
//! `ClipCursor` hardware cursor confinement passthrough, and English-only input enforcement.

use crate::scene::WindowQuad;
use anyhow::{Context, Result};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ActivateKeyboardLayout, LoadKeyboardLayoutW, KLF_ACTIVATE,
};
use windows::Win32::UI::Input::{
    RegisterRawInputDevices, RAWINPUTDEVICE, RIDEV_INPUTSINK,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ClipCursor, GetCursorInfo, GetCursorPos, SendMessageTimeoutW, SetCursor, CURSORINFO,
    CURSOR_SHOWING, HTCLIENT, SMTO_ABORTIFHUNG, WM_MOUSEMOVE, WM_SETCURSOR,
};

/// Thread-safe lock-free cursor coordinate store decoupled from VSync hit-testing.
/// Absorbs 1000Hz–8000Hz raw mouse packets with zero mutex contention.
pub struct AtomicCursor {
    pub x: AtomicI32,
    pub y: AtomicI32,
    pub dirty: AtomicBool,
}

impl AtomicCursor {
    pub const fn new() -> Self {
        Self {
            x: AtomicI32::new(0),
            y: AtomicI32::new(0),
            dirty: AtomicBool::new(false),
        }
    }

    /// Lock-free coordinate update called from the high-frequency Raw Input / `WM_INPUT` handler.
    #[inline]
    pub fn update(&self, new_x: i32, new_y: i32) {
        self.x.store(new_x, Ordering::Relaxed);
        self.y.store(new_y, Ordering::Relaxed);
        self.dirty.store(true, Ordering::Release);
    }

    #[inline]
    pub fn get(&self) -> (i32, i32) {
        (
            self.x.load(Ordering::Relaxed),
            self.y.load(Ordering::Relaxed),
        )
    }

    /// Consumes the latest cursor position once per VSync frame (144–240Hz) for quad hit-testing.
    #[inline]
    pub fn consume_for_vsync(&self) -> Option<(i32, i32)> {
        if self.dirty.swap(false, Ordering::AcqRel) {
            Some(self.get())
        } else {
            None
        }
    }
}

pub static GLOBAL_CURSOR: AtomicCursor = AtomicCursor::new();

/// Phase 5.2: Registers the host window for high-polling (1000–8000Hz) Raw Input mouse packets.
pub fn register_high_polling_mouse(target_hwnd: HWND) -> Result<()> {
    unsafe {
        let device = RAWINPUTDEVICE {
            usUsagePage: 0x01, // HID_USAGE_PAGE_GENERIC
            usUsage: 0x02,     // HID_USAGE_GENERIC_MOUSE
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: target_hwnd,
        };
        RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32)
            .context("RegisterRawInputDevices for high-polling mouse failed")?;
    }
    Ok(())
}

/// Polls the OS cursor position into [`GLOBAL_CURSOR`].
pub fn sync_os_cursor_pos() -> (i32, i32) {
    let mut pt = POINT::default();
    unsafe {
        if GetCursorPos(&mut pt).is_ok() {
            GLOBAL_CURSOR.update(pt.x, pt.y);
        }
    }
    (pt.x, pt.y)
}

/// Phase 5.1: Hit-tests the scene quad tree in reverse Z-order to find the topmost window under `(px, py)`.
pub fn hit_test_quads(quads: &[WindowQuad], px: f32, py: f32) -> Option<&WindowQuad> {
    quads
        .iter()
        .filter(|q| q.hit_test(px, py))
        .max_by_key(|q| q.z_order)
}

/// Phase 5.1: Updates the Win32 hardware cursor plane (`WM_SETCURSOR` + `GetCursorInfo`)
/// to match the hovered window's cursor shape with zero software-cursor latency.
pub fn sync_hardware_cursor(target_hwnd: HWND) {
    if target_hwnd.0.is_null() {
        return;
    }
    unsafe {
        let lparam = ((WM_MOUSEMOVE as usize) << 16) | (HTCLIENT as usize);
        let mut result = 0usize;
        let _ = SendMessageTimeoutW(
            target_hwnd,
            WM_SETCURSOR,
            WPARAM(target_hwnd.0 as usize),
            LPARAM(lparam as isize),
            SMTO_ABORTIFHUNG,
            10,
            Some(&mut result),
        );

        let mut ci = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        if GetCursorInfo(&mut ci).is_ok() && (ci.flags.0 & CURSOR_SHOWING.0 != 0) {
            let _ = SetCursor(ci.hCursor);
        }
    }
}

/// Phase 8.3: Hardware cursor clipping (`ClipCursor`) passthrough.
/// Confines the physical hardware cursor to `quad`'s bounds when requested, or releases confinement when `None`.
pub fn apply_cursor_clip(quad: Option<&WindowQuad>) {
    unsafe {
        if let Some(q) = quad {
            let rc = RECT {
                left: q.physics.position.x.round() as i32,
                top: q.physics.position.y.round() as i32,
                right: (q.physics.position.x + q.width).round() as i32,
                bottom: (q.physics.position.y + q.height).round() as i32,
            };
            let _ = ClipCursor(Some(&rc));
        } else {
            let _ = ClipCursor(None);
        }
    }
}

/// Phase 8.3: Enforces English/Latin keyboard layout (`00000409` US English) and bypasses complex IME overhead.
pub fn enforce_english_keyboard_layout() {
    unsafe {
        let klid: Vec<u16> = "00000409\0".encode_utf16().collect();
        if let Ok(hkl) = LoadKeyboardLayoutW(PCWSTR(klid.as_ptr()), KLF_ACTIVATE) {
            let _ = ActivateKeyboardLayout(hkl, KLF_ACTIVATE);
        }
    }
}
