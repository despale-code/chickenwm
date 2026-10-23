//! Windows Graphics Capture (WGC) and DXGI zero-copy texture export.

use anyhow::Result;
use windows::Win32::Foundation::{HANDLE, HWND};

/// Represents an active window capture session.
pub struct WindowCaptureSession {
    pub hwnd: HWND,
    pub shared_handle: HANDLE,
    pub width: u32,
    pub height: u32,
}

impl WindowCaptureSession {
    /// Starts capturing a Win32 HWND using Windows.Graphics.Capture.
    /// Configures `IsBorderRequired = false` and `IsCursorCaptureEnabled = false`.
    pub fn start(_hwnd: HWND) -> Result<Self> {
        // Skeleton: To be implemented by Gemini 4 Argon
        // 1. Create GraphicsCaptureItem from HWND
        // 2. Initialize Direct3D11CaptureFramePool in B8G8R8A8_UNORM
        // 3. Export ID3D11Texture2D to NT HANDLE via IDXGIResource1::CreateSharedHandle
        Ok(Self {
            hwnd: _hwnd,
            shared_handle: HANDLE(std::ptr::null_mut()),
            width: 0,
            height: 0,
        })
    }

    /// Stops the capture session and releases DXGI resources.
    pub fn stop(&mut self) {
        // Skeleton: To be implemented by Gemini 4 Argon
    }
}
