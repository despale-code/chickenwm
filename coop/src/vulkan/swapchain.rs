//! Independent per-monitor swapchains to guarantee maximum native refresh rates.

use anyhow::Result;
use windows::Win32::Graphics::Gdi::HMONITOR;

/// Encapsulates a Vulkan swapchain bound to a specific physical monitor.
pub struct MonitorSwapchain {
    pub monitor: HMONITOR,
    pub width: u32,
    pub height: u32,
    pub refresh_rate_hz: u32,
}

impl MonitorSwapchain {
    /// Spawns an independent swapchain running at the physical display's maximum native refresh rate.
    pub fn new(monitor: HMONITOR, width: u32, height: u32, refresh_rate_hz: u32) -> Result<Self> {
        // Skeleton: To be implemented by Gemini 4 Argon
        Ok(Self {
            monitor,
            width,
            height,
            refresh_rate_hz,
        })
    }

    /// Recreates the swapchain upon monitor wake (`WM_POWERBROADCAST`) or resolution change.
    pub fn recreate(&mut self, _new_width: u32, _new_height: u32) -> Result<()> {
        // Skeleton: To be implemented by Gemini 4 Argon
        Ok(())
    }
}
