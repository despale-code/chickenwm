//! Phase 10.1: Safe RAII wrappers around Win32 handles (`HANDLE`, `HHOOK`, `HDC`, `HICON`, `HWND`)
//! to guarantee zero GDI or kernel handle leaks across multi-day uptime.

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::Graphics::Gdi::{DeleteDC, ReleaseDC, HDC};
use windows::Win32::UI::WindowsAndMessaging::{
    DestroyIcon, DestroyWindow, UnhookWindowsHookEx, HHOOK, HICON,
};

/// RAII wrapper for a Win32 kernel `HANDLE` closed via `CloseHandle`.
#[derive(Debug)]
pub struct SafeHandle(pub HANDLE);

unsafe impl Send for SafeHandle {}
unsafe impl Sync for SafeHandle {}

impl SafeHandle {
    pub fn new(handle: HANDLE) -> Option<Self> {
        if handle.is_invalid() {
            None
        } else {
            Some(Self(handle))
        }
    }

    pub fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for SafeHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// RAII wrapper for a Win32 hook handle (`HHOOK`) released via `UnhookWindowsHookEx`.
#[derive(Debug)]
pub struct SafeHhook(pub HHOOK);

unsafe impl Send for SafeHhook {}

impl SafeHhook {
    pub fn new(hook: HHOOK) -> Option<Self> {
        if hook.is_invalid() {
            None
        } else {
            Some(Self(hook))
        }
    }

    pub fn raw(&self) -> HHOOK {
        self.0
    }
}

impl Drop for SafeHhook {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = UnhookWindowsHookEx(self.0);
            }
        }
    }
}

/// RAII wrapper for a Win32 `HICON` destroyed via `DestroyIcon`.
#[derive(Debug)]
pub struct SafeHicon(pub HICON);

unsafe impl Send for SafeHicon {}
unsafe impl Sync for SafeHicon {}

impl SafeHicon {
    pub fn new(icon: HICON) -> Option<Self> {
        if icon.is_invalid() {
            None
        } else {
            Some(Self(icon))
        }
    }

    pub fn raw(&self) -> HICON {
        self.0
    }
}

impl Drop for SafeHicon {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = DestroyIcon(self.0);
            }
        }
    }
}

/// RAII wrapper for a created GDI device context (`HDC`) deleted via `DeleteDC`,
/// or a window DC released via `ReleaseDC`.
#[derive(Debug)]
pub enum SafeHdc {
    Created(HDC),
    Window { hwnd: HWND, hdc: HDC },
}

impl SafeHdc {
    pub fn raw(&self) -> HDC {
        match *self {
            SafeHdc::Created(hdc) => hdc,
            SafeHdc::Window { hdc, .. } => hdc,
        }
    }
}

impl Drop for SafeHdc {
    fn drop(&mut self) {
        unsafe {
            match *self {
                SafeHdc::Created(hdc) => {
                    if !hdc.is_invalid() {
                        let _ = DeleteDC(hdc);
                    }
                }
                SafeHdc::Window { hwnd, hdc } => {
                    if !hdc.is_invalid() {
                        let _ = ReleaseDC(hwnd, hdc);
                    }
                }
            }
        }
    }
}

/// RAII wrapper for an owned top-level `HWND` destroyed via `DestroyWindow`.
#[derive(Debug)]
pub struct SafeOwnedHwnd(pub HWND);

impl SafeOwnedHwnd {
    pub fn new(hwnd: HWND) -> Option<Self> {
        if hwnd.0.is_null() {
            None
        } else {
            Some(Self(hwnd))
        }
    }

    pub fn raw(&self) -> HWND {
        self.0
    }
}

impl Drop for SafeOwnedHwnd {
    fn drop(&mut self) {
        if !self.0.0.is_null() {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
}
