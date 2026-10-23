//! `coop` — Pure user-mode Vulkan viewspace compositor with WGC zero-copy texture import,
//! spring-damper physics, multi-monitor native refresh rate swapchains, and DPMS sleep/lock management.

#![windows_subsystem = "windows"]

use anyhow::Result;
use coop::capture::CaptureDevice;
use coop::input::{
    enforce_english_keyboard_layout, hit_test_quads, register_high_polling_mouse,
    sync_hardware_cursor, sync_os_cursor_pos, GLOBAL_CURSOR,
};
use coop::power::{PowerManager, DISPLAY_SLEEPING, SESSION_LOCKED};
use coop::scene::hierarchy::HierarchyTracker;
use coop::scene::{Vec2, WindowQuad};
use coop::vulkan::pipeline::{CompositorPipeline, ExposeOverview, WallpaperLayer};
use coop::vulkan::swapchain::{enumerate_physical_monitors, MonitorSwapchain};
use coop::vulkan::VulkanContext;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetForegroundWindow, GetWindowRect, IsWindowVisible, PeekMessageW,
    TranslateMessage, MSG, PM_REMOVE,
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
    let mut monitor_origins = Vec::with_capacity(monitors.len());
    let mut max_hz = 60u32;

    for mon in &monitors {
        max_hz = max_hz.max(mon.refresh_rate_hz);
        monitor_origins.push((mon.rect.left, mon.rect.top));
        if let Some(ref ctx) = vk_ctx {
            if let Ok(sc) = MonitorSwapchain::new_with_vulkan(ctx, mon) {
                let _ = register_high_polling_mouse(sc.hwnd);
                swapchains.push(sc);
                continue;
            }
        }
        if let Ok(sc) =
            MonitorSwapchain::new(mon.handle, mon.width, mon.height, mon.refresh_rate_hz)
        {
            swapchains.push(sc);
        }
    }

    // Phase 4.2 & 5.3: Start WinEvent popup hierarchy tracker and DPMS/Lock hook
    let _hierarchy = HierarchyTracker::start();
    let _power_hook = PowerManager::start_hook();

    let mut wallpaper = WallpaperLayer::new();
    let _expose = ExposeOverview::new();
    let mut focus_quad = WindowQuad::new(HWND::default(), Vec2::new(100.0, 100.0), 800.0, 600.0);
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
        if chickenwm_core::failsafe::FAILSAFE_TRIGGERED.load(Ordering::Relaxed) {
            break;
        }

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

        // Track active foreground window geometry with spring-damper physics
        unsafe {
            let fg = GetForegroundWindow();
            let is_canvas = swapchains.iter().any(|s| s.hwnd == fg);
            if !fg.0.is_null() && !is_canvas && IsWindowVisible(fg).as_bool() {
                let mut rc = RECT::default();
                if GetWindowRect(fg, &mut rc).is_ok() {
                    let w = (rc.right - rc.left) as f32;
                    let h = (rc.bottom - rc.top) as f32;
                    if w > 32.0 && h > 32.0 {
                        focus_quad.hwnd = fg;
                        focus_quad.physics.target = Vec2::new(rc.left as f32, rc.top as f32);
                        focus_quad.size_physics.target = Vec2::new(w, h);
                    }
                }
            }
        }

        wallpaper.tick(dt);
        focus_quad.step(dt);
        for q in &mut quads {
            q.step(dt);
        }

        // Render and present across all monitor Vulkan swapchains
        if let Some(ref ctx) = vk_ctx {
            let locked = SESSION_LOCKED.load(Ordering::Acquire);
            let clear_rgba = if locked {
                [0.04, 0.04, 0.07, 0.94] // Dark translucent lock canvas
            } else {
                [0.0, 0.0, 0.0, 0.0] // 100% transparent click-through desktop canvas
            };

            let active_popups = HierarchyTracker::active_popups();

            for (idx, sc) in swapchains.iter_mut().enumerate() {
                let (mon_x, mon_y) = monitor_origins.get(idx).copied().unwrap_or((0, 0));
                let mon_w = sc.width;
                let mon_h = sc.height;

                let _ = sc.render_and_present(ctx, clear_rgba, |device, cmd_buf| {
                    if locked {
                        // Draw centered lock indicator card on the Vulkan lock screen
                        let lock_quad = WindowQuad::new(
                            HWND::default(),
                            Vec2::new(
                                mon_x as f32 + (mon_w as f32 - 420.0) * 0.5,
                                mon_y as f32 + (mon_h as f32 - 180.0) * 0.5,
                            ),
                            420.0,
                            180.0,
                        );
                        pipeline.record_quad_border(
                            device,
                            cmd_buf,
                            &lock_quad,
                            mon_x,
                            mon_y,
                            mon_w,
                            mon_h,
                            [0.95, 0.45, 0.15, 1.0],
                            4,
                        );
                    } else if !focus_quad.hwnd.0.is_null() {
                        // Draw fluid spring-animated Vulkan focus frame around active window
                        pipeline.record_quad_border(
                            device,
                            cmd_buf,
                            &focus_quad,
                            mon_x,
                            mon_y,
                            mon_w,
                            mon_h,
                            [0.95, 0.45, 0.15, 0.92],
                            3,
                        );

                        // Highlight any tracked #32768 popup menus relative to parent quad
                        for popup in &active_popups {
                            let pquad =
                                HierarchyTracker::compute_relative_popup_quad(popup, &focus_quad);
                            pipeline.record_quad_border(
                                device,
                                cmd_buf,
                                &pquad,
                                mon_x,
                                mon_y,
                                mon_w,
                                mon_h,
                                [0.98, 0.68, 0.25, 0.90],
                                2,
                            );
                        }
                    }
                });
            }
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

    if let Some(ref ctx) = vk_ctx {
        for sc in &mut swapchains {
            unsafe {
                sc.destroy(ctx);
            }
        }
    }
    Ok(())
}
