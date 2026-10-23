//! `coop` — Pure user-mode Vulkan viewspace compositor with WGC zero-copy texture import,
//! spring-damper physics, multi-monitor native refresh rate swapchains, and DPMS sleep/lock management.

#![windows_subsystem = "windows"]

use anyhow::Result;
use coop::capture::CaptureDevice;
use coop::input::{
    enforce_english_keyboard_layout, hit_test_quads, register_high_polling_mouse,
    sync_hardware_cursor, sync_os_cursor_pos, GLOBAL_CURSOR,
};
use coop::power::{PowerManager, DISPLAY_SLEEPING};
use coop::scene::hierarchy::HierarchyTracker;
use coop::scene::WindowQuad;
use coop::vulkan::pipeline::{CompositorPipeline, ExposeOverview, WallpaperLayer};
use coop::vulkan::swapchain::{enumerate_physical_monitors, MonitorSwapchain};
use coop::vulkan::VulkanContext;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
};

#[tokio::main]
async fn main() -> Result<()> {
    chickenwm_core::init_tracing("coop");
    chickenwm_core::failsafe::install_crash_handler();
    let _failsafe = chickenwm_core::failsafe::spawn_emergency_failsafe_thread();

    // Phase 1.2 & 8.3: Intercept Win + L and enforce English/Latin input
    let _ = chickenwm_core::failsafe::set_disable_lock_workstation(true);
    enforce_english_keyboard_layout();

    tracing::info!("Initializing coop Vulkan viewspace compositor...");

    // Phase 2 & 3: Initialize Vulkan context, pipeline, WGC capture device, and per-monitor swapchains
    let vk_ctx = VulkanContext::new().ok();
    let _capture_dev = CaptureDevice::new().ok();
    let mut pipeline = CompositorPipeline::new()?;
    if let Some(ref ctx) = vk_ctx {
        let _ = pipeline.init_layout(&ctx.device);
    }

    let monitors = enumerate_physical_monitors();
    let mut swapchains = Vec::with_capacity(monitors.len());
    let mut max_hz = 60u32;

    for mon in &monitors {
        max_hz = max_hz.max(mon.refresh_rate_hz);
        if let Some(ref ctx) = vk_ctx {
            if let Ok(sc) = MonitorSwapchain::new_with_vulkan(ctx, mon) {
                let _ = register_high_polling_mouse(sc.hwnd);
                swapchains.push(sc);
                continue;
            }
        }
        if let Ok(sc) = MonitorSwapchain::new(mon.handle, mon.width, mon.height, mon.refresh_rate_hz) {
            swapchains.push(sc);
        }
    }

    // Phase 4.2 & 5.3: Start WinEvent popup hierarchy tracker and DPMS/Lock hook
    let _hierarchy = HierarchyTracker::start();
    let _power_hook = PowerManager::start_hook();

    let mut wallpaper = WallpaperLayer::new();
    let _expose = ExposeOverview::new();
    let mut quads: Vec<WindowQuad> = Vec::new();

    let frame_interval = Duration::from_micros((1_000_000 / max_hz.max(60)) as u64);
    let mut last_tick = Instant::now();
    let idle_timeout_ms = 15 * 60 * 1000; // 15 min DPMS sleep timeout

    tracing::info!(
        "coop running across {} monitor(s) at up to {}Hz",
        swapchains.len(),
        max_hz
    );

    loop {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }

        // Phase 5.3: Check idle timeout -> sleep displays and drop GPU to 0 FPS
        if PowerManager::poll_idle_timeout(idle_timeout_ms, vk_ctx.as_ref())
            || DISPLAY_SLEEPING.load(Ordering::Acquire)
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
            last_tick = Instant::now();
            continue;
        }

        let now = Instant::now();
        let dt = (now - last_tick).as_secs_f32();
        last_tick = now;

        wallpaper.tick(dt);
        for q in &mut quads {
            q.step(dt);
        }

        // Phase 5.1 & 5.2: Evaluate hit-testing synchronously at VSync rate
        sync_os_cursor_pos();
        if let Some((cx, cy)) = GLOBAL_CURSOR.consume_for_vsync() {
            if let Some(top_quad) = hit_test_quads(&quads, cx as f32, cy as f32) {
                sync_hardware_cursor(top_quad.hwnd);
            }
        }

        let elapsed = last_tick.elapsed();
        if elapsed < frame_interval {
            tokio::time::sleep(frame_interval - elapsed).await;
        }
    }
}
