//! Hardware cursor plane integration and high-polling mouse input decoupling.

use std::sync::atomic::{AtomicI32, Ordering};
use windows::Win32::Foundation::HWND;

/// Thread-safe cursor coordinate store decoupled from hit-testing.
pub struct AtomicCursor {
    pub x: AtomicI32,
    pub y: AtomicI32,
}

impl AtomicCursor {
    pub const fn new() -> Self {
        Self {
            x: AtomicI32::new(0),
            y: AtomicI32::new(0),
        }
    }

    pub fn update(&self, new_x: i32, new_y: i32) {
        self.x.store(new_x, Ordering::Relaxed);
        self.y.store(new_y, Ordering::Relaxed);
    }

    pub fn get(&self) -> (i32, i32) {
        (self.x.load(Ordering::Relaxed), self.y.load(Ordering::Relaxed))
    }
}

pub static GLOBAL_CURSOR: AtomicCursor = AtomicCursor::new();

/// Updates the Win32 hardware cursor plane to match the hovered window's cursor shape.
pub fn sync_hardware_cursor(_target_hwnd: HWND) {
    // Skeleton: To be implemented by Gemini 4 Argon
    // Query WM_SETCURSOR or GetCursorInfo, call SetCursor()
}
