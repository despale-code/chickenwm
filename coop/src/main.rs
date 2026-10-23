//! `coop` — Pure user-mode Vulkan viewspace compositor with WGC zero-copy texture import,
//! spring-damper physics, multi-monitor native refresh rate swapchains, and DPMS sleep/lock management.

#![windows_subsystem = "windows"]

use anyhow::Result;
use coop::capture::{CaptureDevice, WindowCaptureSession};
use coop::input::{
    enforce_english_keyboard_layout, register_high_polling_mouse, sync_hardware_cursor,
    sync_os_cursor_pos, GLOBAL_CURSOR,
};
use coop::power::{PowerManager, DISPLAY_SLEEPING, SESSION_LOCKED};
use coop::scene::hierarchy::HierarchyTracker;
use coop::scene::{Vec2, WindowQuad};
use coop::vulkan::pipeline::{CompositorPipeline, ExposeOverview, WallpaperLayer};
use coop::vulkan::swapchain::{enumerate_physical_monitors, MonitorSwapchain};
use coop::vulkan::{ImportedVulkanTexture, VulkanContext};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, GetClassNameW, GetForegroundWindow, GetShellWindow,
    GetWindowLongW, GetWindowRect, IsIconic, IsWindowVisible, PeekMessageW, TranslateMessage,
    GWL_EXSTYLE, MSG, PM_REMOVE, WS_EX_TOOLWINDOW,
};

struct ManagedWindow {
    quad: WindowQuad,
    capture: WindowCaptureSession,
    vk_tex: Option<ImportedVulkanTexture>,
}

/// Returns `true` if `hwnd` is cloaked by DWM (e.g. suspended UWP background apps).
fn is_window_cloaked(hwnd: HWND) -> bool {
    unsafe {
        let mut cloaked: u32 = 0;
        let _ = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        );
        cloaked != 0
    }
}

/// Enumerates all visible top-level windows in back-to-front Z-order (bottom-most first, topmost last),
/// starting with the desktop wallpaper host (`Progman` / `GetShellWindow`) at index 0.
fn enumerate_visible_windows_back_to_front(canvas_hwnds: &[HWND]) -> Vec<(HWND, RECT)> {
    let mut top_to_bottom: Vec<(HWND, RECT)> = Vec::with_capacity(32);

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let list = unsafe { &mut *(lparam.0 as *mut Vec<(HWND, RECT)>) };
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                return BOOL(1);
            }
            if is_window_cloaked(hwnd) {
                return BOOL(1);
            }

            let mut class_buf = [0u16; 64];
            let len = GetClassNameW(hwnd, &mut class_buf) as usize;
            let class_name = String::from_utf16_lossy(&class_buf[..len]);
            if class_name == "CoopVulkanCanvasClass"
                || class_name == "ChickenScopeHudOverlayClass"
                || class_name == "Progman"
                || class_name == "WorkerW"
            {
                return BOOL(1);
            }

            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if (ex_style & WS_EX_TOOLWINDOW.0) != 0 && class_name != "#32768" {
                return BOOL(1);
            }

            let mut rc = RECT::default();
            if GetWindowRect(hwnd, &mut rc).is_ok() {
                let w = rc.right - rc.left;
                let h = rc.bottom - rc.top;
                if w > 24 && h > 24 {
                    list.push((hwnd, rc));
                }
            }
        }
        BOOL(1)
    }

    unsafe {
        let _ = EnumWindows(
            Some(enum_proc),
            LPARAM(&mut top_to_bottom as *mut _ as isize),
        );
    }

    top_to_bottom.retain(|(h, _)| !canvas_hwnds.contains(h));
    // Reverse so painter's algorithm draws back-to-front (bottom windows first, active top window last)
    top_to_bottom.reverse();

    // Prepend desktop shell wallpaper window (`Progman`) at the very bottom if available
    unsafe {
        let shell_hwnd = GetShellWindow();
        if !shell_hwnd.0.is_null() && IsWindowVisible(shell_hwnd).as_bool() {
            let mut rc = RECT::default();
            if GetWindowRect(shell_hwnd, &mut rc).is_ok()
                && (rc.right - rc.left) > 100
                && (rc.bottom - rc.top) > 100
            {
                top_to_bottom.insert(0, (shell_hwnd, rc));
            }
        }
    }

    top_to_bottom
}

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
    let capture_dev = CaptureDevice::new().ok();
    let mut pipeline = CompositorPipeline::new()?;
    if let Some(ref ctx) = vk_ctx {
        let _ = pipeline.init_layout(&ctx.device);
    }

    let monitors = enumerate_physical_monitors();
    let mut swapchains = Vec::with_capacity(monitors.len());
    let mut monitor_origins = Vec::with_capacity(monitors.len());
    let mut canvas_hwnds = Vec::with_capacity(monitors.len());
    let mut max_hz = 60u32;

    for mon in &monitors {
        max_hz = max_hz.max(mon.refresh_rate_hz);
        monitor_origins.push((mon.rect.left, mon.rect.top));
        if let Some(ref ctx) = vk_ctx {
            if let Ok(sc) = MonitorSwapchain::new_with_vulkan(ctx, mon) {
                let _ = register_high_polling_mouse(sc.hwnd);
                canvas_hwnds.push(sc.hwnd);
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

    let mut anim_speed = chickenwm_core::config::load_or_init_config()
        .map(|(_doc, c)| c.animations.speed)
        .unwrap_or(2.2);
    let mut last_cfg_check = Instant::now();

    let mut wallpaper = WallpaperLayer::new();
    let _expose = ExposeOverview::new();
    let mut managed_windows: HashMap<isize, ManagedWindow> = HashMap::new();
    let mut failed_hwnds: HashSet<isize> = HashSet::new();
    let mut z_ordered_keys: Vec<isize> = Vec::new();
    let mut last_enum = Instant::now() - Duration::from_secs(1);

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

        // Refresh window list and Z-order every ~32ms, and update positions every frame
        if last_enum.elapsed() >= Duration::from_millis(32) {
            last_enum = now;
            let visible = enumerate_visible_windows_back_to_front(&canvas_hwnds);
            let mut seen = HashSet::with_capacity(visible.len());
            z_ordered_keys.clear();

            for (z_idx, (hwnd, rc)) in visible.into_iter().enumerate() {
                let key = hwnd.0 as isize;
                seen.insert(key);
                z_ordered_keys.push(key);

                let x = rc.left as f32;
                let y = rc.top as f32;
                let w = (rc.right - rc.left).max(1) as f32;
                let h = (rc.bottom - rc.top).max(1) as f32;

                if let Some(entry) = managed_windows.get_mut(&key) {
                    entry.quad.physics.target = Vec2::new(x, y);
                    entry.quad.size_physics.target = Vec2::new(w, h);
                    entry.quad.z_order = z_idx as i32;
                } else if !failed_hwnds.contains(&key) {
                    if let Some(ref cdev) = capture_dev {
                        match WindowCaptureSession::start_with_device(hwnd, cdev) {
                            Ok(cap) => {
                                let vk_tex = vk_ctx.as_ref().and_then(|ctx| {
                                    match ctx.import_dxgi_texture(
                                        cap.shared_handle,
                                        cap.width,
                                        cap.height,
                                    ) {
                                        Ok(t) => Some(t),
                                        Err(e) => {
                                            tracing::warn!(
                                                "Vulkan import_dxgi_texture failed for HWND {:?}: {:?}",
                                                hwnd,
                                                e
                                            );
                                            None
                                        }
                                    }
                                });
                                let mut quad = WindowQuad::new(hwnd, Vec2::new(x, y), w, h);
                                quad.z_order = z_idx as i32;
                                quad.set_speed(anim_speed);
                                managed_windows.insert(
                                    key,
                                    ManagedWindow {
                                        quad,
                                        capture: cap,
                                        vk_tex,
                                    },
                                );
                            }
                            Err(e) => {
                                tracing::debug!("WGC capture skipped for HWND {:?}: {:?}", hwnd, e);
                                failed_hwnds.insert(key);
                            }
                        }
                    }
                }
            }

            // Clean up closed or hidden windows
            managed_windows.retain(|k, entry| {
                if seen.contains(k) {
                    true
                } else {
                    if let (Some(ctx), Some(mut tex)) = (vk_ctx.as_ref(), entry.vk_tex.take()) {
                        unsafe {
                            tex.destroy(&ctx.device);
                        }
                    }
                    false
                }
            });
        }

        if last_cfg_check.elapsed() >= Duration::from_millis(500) {
            last_cfg_check = now;
            if let Ok((_doc, new_cfg)) = chickenwm_core::config::load_or_init_config() {
                if (new_cfg.animations.speed - anim_speed).abs() > 0.05 {
                    anim_speed = new_cfg.animations.speed;
                    for entry in managed_windows.values_mut() {
                        entry.quad.set_speed(anim_speed);
                    }
                }
            }
        }

        // Poll latest WGC frames and step spring-damper physics for all tracked windows
        let fg_hwnd = unsafe { GetForegroundWindow() };
        if let Some(fg_entry) = managed_windows.get_mut(&(fg_hwnd.0 as isize)) {
            unsafe {
                let mut rc = RECT::default();
                if GetWindowRect(fg_hwnd, &mut rc).is_ok() {
                    fg_entry.quad.physics.target = Vec2::new(rc.left as f32, rc.top as f32);
                    fg_entry.quad.size_physics.target = Vec2::new(
                        (rc.right - rc.left).max(1) as f32,
                        (rc.bottom - rc.top).max(1) as f32,
                    );
                }
            }
        }

        wallpaper.tick(dt);
        for entry in managed_windows.values_mut() {
            if let Some(ref cdev) = capture_dev {
                let _ = entry.capture.poll_next_frame(cdev);
                if entry.capture.handle_recreated {
                    entry.capture.handle_recreated = false;
                    if let Some(ref ctx) = vk_ctx {
                        if let Some(mut old_tex) = entry.vk_tex.take() {
                            unsafe {
                                ctx.wait_idle();
                                old_tex.destroy(&ctx.device);
                            }
                        }
                        entry.vk_tex = ctx
                            .import_dxgi_texture(
                                entry.capture.shared_handle,
                                entry.capture.width,
                                entry.capture.height,
                            )
                            .ok();
                    }
                }
            }
            entry.quad.step(dt);
        }

        // Render and present across all monitor Vulkan swapchains
        if let Some(ref ctx) = vk_ctx {
            let locked = SESSION_LOCKED.load(Ordering::Acquire);
            let clear_rgba = if locked {
                [0.04, 0.04, 0.07, 1.0]
            } else {
                [0.08, 0.09, 0.12, 1.0] // Sleek dark slate fallback behind wallpaper/windows
            };

            let active_popups = HierarchyTracker::active_popups();

            for (idx, sc) in swapchains.iter_mut().enumerate() {
                let (mon_x, mon_y) = monitor_origins.get(idx).copied().unwrap_or((0, 0));
                let mon_w = sc.width;
                let mon_h = sc.height;

                let _ = sc.render_and_present(
                    ctx,
                    clear_rgba,
                    |cmd_buf, dst_img| {
                        if !locked {
                            // Blit every captured window texture in back-to-front Z-order
                            for key in &z_ordered_keys {
                                if let Some(entry) = managed_windows.get(key) {
                                    if entry.capture.has_frame {
                                        if let Some(ref tex) = entry.vk_tex {
                                            let x0 =
                                                (entry.quad.physics.position.x.round() as i32) - mon_x;
                                            let y0 =
                                                (entry.quad.physics.position.y.round() as i32) - mon_y;
                                            let x1 = x0 + (entry.quad.width.round() as i32);
                                            let y1 = y0 + (entry.quad.height.round() as i32);

                                            unsafe {
                                                ctx.record_blit_window(
                                                    cmd_buf, tex.image, tex.width, tex.height,
                                                    dst_img, x0, y0, x1, y1, mon_w, mon_h,
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    },
                    |device, cmd_buf| {
                        if locked {
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
                        } else if let Some(fg_entry) = managed_windows.get(&(fg_hwnd.0 as isize)) {
                            let shell_hwnd = unsafe { GetShellWindow() };
                            if fg_hwnd != shell_hwnd {
                                pipeline.record_quad_border(
                                    device,
                                    cmd_buf,
                                    &fg_entry.quad,
                                    mon_x,
                                    mon_y,
                                    mon_w,
                                    mon_h,
                                    [0.95, 0.45, 0.15, 0.95],
                                    3,
                                );

                                for popup in &active_popups {
                                    let pquad = HierarchyTracker::compute_relative_popup_quad(
                                        popup,
                                        &fg_entry.quad,
                                    );
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
                        }
                    },
                );
            }
        }

        // Phase 5.1 & 5.2: Evaluate hit-testing synchronously at VSync rate
        sync_os_cursor_pos();
        if let Some((cx, cy)) = GLOBAL_CURSOR.consume_for_vsync() {
            if let Some(top_key) = z_ordered_keys.iter().rev().find(|&&k| {
                managed_windows
                    .get(&k)
                    .is_some_and(|e| e.quad.hit_test(cx as f32, cy as f32))
            }) {
                if let Some(entry) = managed_windows.get(top_key) {
                    sync_hardware_cursor(entry.quad.hwnd);
                }
            }
        }

        let elapsed = last_tick.elapsed();
        if elapsed < frame_interval {
            tokio::time::sleep(frame_interval - elapsed).await;
        }
    }

    if let Some(ref ctx) = vk_ctx {
        ctx.wait_idle();
        for entry in managed_windows.values_mut() {
            if let Some(mut tex) = entry.vk_tex.take() {
                unsafe {
                    tex.destroy(&ctx.device);
                }
            }
        }
        for sc in &mut swapchains {
            unsafe {
                sc.destroy(ctx);
            }
        }
    }
    Ok(())
}
