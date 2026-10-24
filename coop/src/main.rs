//! `coop` — Pure user-mode Vulkan viewspace compositor with WGC zero-copy texture import,
//! spring-damper physics, multi-monitor native refresh rate swapchains, and DPMS sleep/lock management.

#![windows_subsystem = "windows"]

use anyhow::Result;
use ash::vk;
use coop::capture::{CaptureDevice, WindowCaptureSession};
use coop::input::{
    enforce_english_keyboard_layout, register_high_polling_mouse, sync_hardware_cursor,
    sync_os_cursor_pos, GLOBAL_CURSOR,
};
use coop::power::{PowerManager, DISPLAY_SLEEPING, SESSION_LOCKED};
use coop::scene::hierarchy::HierarchyTracker;
use coop::scene::{Vec2, WindowQuad};
use coop::vulkan::pipeline::{
    CompositorPipeline, ExposeOverview, MacBorderColors, MacBorderTheme, WallpaperLayer,
};
use coop::vulkan::swapchain::{enumerate_physical_monitors, MonitorSwapchain};
use coop::vulkan::{ImportedVulkanTexture, VulkanContext};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{
    DwmExtendFrameIntoClientArea, DwmGetWindowAttribute, DwmSetWindowAttribute, DWMWA_BORDER_COLOR,
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DWMWA_SYSTEMBACKDROP_TYPE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWINDOWATTRIBUTE,
};
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, EnumWindows, GetClassNameW, GetForegroundWindow, GetPropW, GetShellWindow,
    GetWindow, GetWindowLongW, GetWindowRect, GetWindowTextW, IsIconic, IsWindowVisible,
    PeekMessageW, SetWindowLongW, SetWindowPos, TranslateMessage, GWL_EXSTYLE, GWL_STYLE, GW_OWNER,
    MSG, PM_REMOVE, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    WS_CAPTION, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP,
    WS_SYSMENU, WS_THICKFRAME,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowRole {
    Desktop,
    Roobar,
    TransparentOverlay,
    Floating,
    Tiled,
}

struct ManagedWindow {
    quad: WindowQuad,
    capture: WindowCaptureSession,
    vk_tex: Option<ImportedVulkanTexture>,
    descriptor_set: Option<vk::DescriptorSet>,
    role: WindowRole,
}

fn get_window_title(hwnd: HWND) -> String {
    unsafe {
        let mut buf = [0u16; 256];
        let len = GetWindowTextW(hwnd, &mut buf);
        if len > 0 {
            String::from_utf16_lossy(&buf[..len as usize])
        } else {
            String::new()
        }
    }
}

fn get_window_class(hwnd: HWND) -> String {
    unsafe {
        let mut buf = [0u16; 128];
        let len = GetClassNameW(hwnd, &mut buf);
        if len > 0 {
            String::from_utf16_lossy(&buf[..len as usize])
        } else {
            String::new()
        }
    }
}

fn classify_window_role(hwnd: HWND) -> WindowRole {
    unsafe {
        if hwnd == GetShellWindow() {
            return WindowRole::Desktop;
        }
        let class = get_window_class(hwnd);
        if class == "Progman" || class == "WorkerW" || class == "ChickenWMPaperClass" {
            return WindowRole::Desktop;
        }

        let title = get_window_title(hwnd);
        if title == "ChickenWM Bar — roobar" || title.starts_with("ChickenWM Bar") {
            return WindowRole::Roobar;
        }
        if title.starts_with("roobar ")
            || title == "ChickenWM Power"
            || title == "ChickenWM Launcher"
            || title == "ChickenWM Notifications"
            || class == "#32768"
        {
            return WindowRole::TransparentOverlay;
        }

        // Check authoritative ChickenWM_Managed / ChickenWM_Tiled window properties set by chickenwm-wm
        let managed = GetPropW(hwnd, windows::core::w!("ChickenWM_Managed"));
        let tiled = GetPropW(hwnd, windows::core::w!("ChickenWM_Tiled"));
        if !managed.0.is_null() {
            return if !tiled.0.is_null() {
                WindowRole::Tiled
            } else {
                WindowRole::Floating
            };
        }

        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let has_owner = GetWindow(hwnd, GW_OWNER)
            .map(|h| !h.0.is_null())
            .unwrap_or(false);

        if (style & WS_POPUP.0) != 0
            || has_owner
            || (ex_style & (WS_EX_TOPMOST.0 | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0)) != 0
        {
            return WindowRole::Floating;
        }

        if (style & (WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0)) == 0
            && (ex_style & WS_EX_LAYERED.0) != 0
        {
            return WindowRole::TransparentOverlay;
        }

        WindowRole::Tiled
    }
}

/// Returns the actual visible frame bounds of `hwnd`, excluding invisible Win10/Win11 drop-shadow padding.
fn get_window_extended_bounds(hwnd: HWND) -> Option<RECT> {
    unsafe {
        let mut rc = RECT::default();
        if DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut rc as *mut _ as *mut core::ffi::c_void,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok()
        {
            if (rc.right - rc.left) > 0 && (rc.bottom - rc.top) > 0 {
                return Some(rc);
            }
        }
        if GetWindowRect(hwnd, &mut rc).is_ok() {
            if (rc.right - rc.left) > 0 && (rc.bottom - rc.top) > 0 {
                return Some(rc);
            }
        }
        None
    }
}

/// Hides Windows 11's native 1px DWM border (`DWMWA_BORDER_COLOR = DWMWA_COLOR_NONE`), clears any
/// bogus white `DWMWA_CAPTION_COLOR` override, heals frameless transparent overlays (`roobar`),
/// and restores `WS_CAPTION | WS_THICKFRAME` only on standard overlapped windows (`WS_SYSMENU`).
fn heal_native_window_frame(hwnd: HWND, role: WindowRole) {
    unsafe {
        if role == WindowRole::Desktop {
            return;
        }
        let none: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE (valid only for DWMWA_BORDER_COLOR)
        let default_color: u32 = 0xFFFF_FFFF; // DWMWA_COLOR_DEFAULT
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_BORDER_COLOR,
            &none as *const u32 as *const core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWINDOWATTRIBUTE(35), // DWMWA_CAPTION_COLOR
            &default_color as *const u32 as *const core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        );
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWINDOWATTRIBUTE(36), // DWMWA_TEXT_COLOR
            &default_color as *const u32 as *const core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        );

        let cur = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        let frame = WS_CAPTION.0 | WS_THICKFRAME.0;

        if matches!(role, WindowRole::Roobar | WindowRole::TransparentOverlay) {
            let no_round = DWMWCP_DONOTROUND;
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_WINDOW_CORNER_PREFERENCE,
                &no_round as *const _ as *const core::ffi::c_void,
                std::mem::size_of_val(&no_round) as u32,
            );
            let no_border: u32 = 0xFFFF_FFFE; // DWMWA_COLOR_NONE
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR,
                &no_border as *const u32 as *const core::ffi::c_void,
                std::mem::size_of::<u32>() as u32,
            );
            let no_backdrop: u32 = 1; // DWMSBT_NONE
            let _ = DwmSetWindowAttribute(
                hwnd,
                DWMWA_SYSTEMBACKDROP_TYPE,
                &no_backdrop as *const u32 as *const core::ffi::c_void,
                std::mem::size_of::<u32>() as u32,
            );
            let margins = MARGINS {
                cxLeftWidth: -1,
                cxRightWidth: -1,
                cyTopHeight: -1,
                cyBottomHeight: -1,
            };
            let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
            return;
        }

        // Restore WS_CAPTION | WS_THICKFRAME on standard non-popup windows so DWM never falls back to Win95 titlebars
        if (cur & WS_POPUP.0) == 0 && (cur & frame) != frame {
            let _ = SetWindowLongW(hwnd, GWL_STYLE, (cur | frame) as i32);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
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
                || class_name == "ChickenWMBorderOverlayClass"
                || class_name == "Progman"
                || class_name == "WorkerW"
            {
                return BOOL(1);
            }

            let title = get_window_title(hwnd);
            let is_chicken_ui = title.starts_with("ChickenWM") || title.starts_with("roobar");

            let ex_style = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
            if (ex_style & WS_EX_TOOLWINDOW.0) != 0 && class_name != "#32768" && !is_chicken_ui {
                return BOOL(1);
            }

            if let Some(rc) = get_window_extended_bounds(hwnd) {
                let w = rc.right - rc.left;
                let h = rc.bottom - rc.top;
                if w > 16 && h > 16 {
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
            if let Some(rc) = get_window_extended_bounds(shell_hwnd) {
                if (rc.right - rc.left) > 100 && (rc.bottom - rc.top) > 100 {
                    top_to_bottom.insert(0, (shell_hwnd, rc));
                }
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
                let _ = pipeline.init_border_pipeline(&ctx.device, sc.render_pass);
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

    let initial_cfg = chickenwm_core::config::load_or_init_config().map(|(_doc, c)| c).ok();
    let mut anim_speed = initial_cfg.as_ref().map(|c| c.animations.speed).unwrap_or(2.2);
    let config_fps_limit = initial_cfg.as_ref().map(|c| c.misc.fps_limit).unwrap_or(0);
    let mut corner_radius = initial_cfg.as_ref().map(|c| c.decoration.rounding).unwrap_or(12);
    let mut theme_mode = initial_cfg
        .as_ref()
        .map(|c| if c.theme.mode == chickenwm_core::theme::ThemeMode::Light { MacBorderTheme::Light } else { MacBorderTheme::Dark })
        .unwrap_or(MacBorderTheme::Dark);
    let mut last_cfg_check = Instant::now();

    let mut wallpaper = WallpaperLayer::new();
    let _expose = ExposeOverview::new();
    let mut managed_windows: HashMap<isize, ManagedWindow> = HashMap::new();
    let mut failed_hwnds: HashSet<isize> = HashSet::new();
    let mut z_ordered_keys: Vec<isize> = Vec::new();
    let mut last_enum = Instant::now() - Duration::from_secs(1);

    let pacer = chickenwm_core::frame_scheduler::FramePacerControl::new(config_fps_limit, false);
    pacer.set_monitor_native_hz(max_hz);
    let mut scheduler = chickenwm_core::frame_scheduler::AdaptiveFrameScheduler::new(pacer);
    let mut dt = 1.0 / (max_hz.max(60) as f32);

    let idle_timeout_ms = 15 * 60 * 1000; // 15 min DPMS sleep timeout

    let mut fps_report = Instant::now();
    let mut frame_count = 0u32;

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
            dt = scheduler.sample_delta_seconds();
            continue;
        }

        let now = Instant::now();

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
                let role = classify_window_role(hwnd);

                if let Some(entry) = managed_windows.get_mut(&key) {
                    entry.quad.physics.target = Vec2::new(x, y);
                    entry.quad.size_physics.target = Vec2::new(w, h);
                    entry.quad.z_order = z_idx as i32;
                    entry.role = role;
                } else if !failed_hwnds.contains(&key) {
                    heal_native_window_frame(hwnd, role);
                    if let Some(ref cdev) = capture_dev {
                        match WindowCaptureSession::start_with_device(hwnd, cdev) {
                            Ok(cap) => {
                                let vk_tex = vk_ctx.as_ref().and_then(|ctx| {
                                    match ctx.import_dxgi_texture(
                                        cap.shared_handle,
                                        cap.capacity_w,
                                        cap.capacity_h,
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
                                let descriptor_set =
                                    vk_ctx.as_ref().zip(vk_tex.as_ref()).and_then(|(ctx, t)| {
                                        pipeline.allocate_texture_descriptor_set(&ctx.device, t.view)
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
                                        descriptor_set,
                                        role,
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
                    if let Some(ref ctx) = vk_ctx {
                        if let Some(ds) = entry.descriptor_set.take() {
                            unsafe {
                                pipeline.free_texture_descriptor_set(&ctx.device, ds);
                            }
                        }
                        if let Some(mut tex) = entry.vk_tex.take() {
                            unsafe {
                                tex.destroy(&ctx.device);
                            }
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
                if new_cfg.misc.fps_limit != scheduler.control().fps_limit() {
                    scheduler.control().set_fps_limit(Some(new_cfg.misc.fps_limit));
                }
                corner_radius = new_cfg.decoration.rounding;
                theme_mode = if new_cfg.theme.mode == chickenwm_core::theme::ThemeMode::Light { MacBorderTheme::Light } else { MacBorderTheme::Dark };
            }
        }

        // Poll latest WGC frames and step spring-damper physics for all tracked windows
        let fg_hwnd = unsafe { GetForegroundWindow() };
        let lbutton_down = unsafe {
            GetAsyncKeyState(VK_LBUTTON.0 as i32) < 0 || GetAsyncKeyState(VK_RBUTTON.0 as i32) < 0
        };

        if !lbutton_down {
            if let Some(fg_entry) = managed_windows.get_mut(&(fg_hwnd.0 as isize)) {
                if let Some(rc) = get_window_extended_bounds(fg_hwnd) {
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
            if lbutton_down && entry.role != WindowRole::Desktop {
                if let Some(rc) = get_window_extended_bounds(entry.quad.hwnd) {
                    let target_pos = Vec2::new(rc.left as f32, rc.top as f32);
                    let target_size = Vec2::new(
                        (rc.right - rc.left).max(1) as f32,
                        (rc.bottom - rc.top).max(1) as f32,
                    );
                    entry.quad.physics.target = target_pos;
                    entry.quad.size_physics.target = target_size;
                }
            }

            if let Some(ref cdev) = capture_dev {
                let _ = entry.capture.poll_next_frame(cdev);
                if entry.capture.handle_recreated {
                    entry.capture.handle_recreated = false;
                    if let Some(ref ctx) = vk_ctx {
                        if let Some(ds) = entry.descriptor_set.take() {
                            unsafe {
                                pipeline.free_texture_descriptor_set(&ctx.device, ds);
                            }
                        }
                        if let Some(mut old_tex) = entry.vk_tex.take() {
                            unsafe {
                                ctx.wait_idle();
                                old_tex.destroy(&ctx.device);
                            }
                        }
                        entry.vk_tex = ctx
                            .import_dxgi_texture(
                                entry.capture.shared_handle,
                                entry.capture.capacity_w,
                                entry.capture.capacity_h,
                            )
                            .ok();
                        entry.descriptor_set = entry.vk_tex.as_ref().and_then(|t| {
                            pipeline.allocate_texture_descriptor_set(&ctx.device, t.view)
                        });
                    }
                }
            }

            if lbutton_down
                || matches!(entry.role, WindowRole::Roobar | WindowRole::TransparentOverlay | WindowRole::Desktop)
            {
                // Keep locked to target position during real-time mouse drag/resize or for roobar/overlays
                entry.quad.physics.position = entry.quad.physics.target;
                entry.quad.physics.velocity = Vec2::default();
                entry.quad.size_physics.position = entry.quad.size_physics.target;
                entry.quad.size_physics.velocity = Vec2::default();
                entry.quad.width = entry.quad.size_physics.target.x;
                entry.quad.height = entry.quad.size_physics.target.y;
            } else {
                entry.quad.step(dt);
            }
        }

        // Render and present across all monitor Vulkan swapchains
        if let Some(ref ctx) = vk_ctx {
            let locked = SESSION_LOCKED.load(Ordering::Acquire);
            let clear_rgba = if locked {
                [0.04, 0.04, 0.07, 1.0]
            } else {
                [0.0, 0.0, 0.0, 0.0] // Transparent clear so desktop wallpaper & transparent surfaces never turn black
            };

            let active_popups = HierarchyTracker::active_popups();

            for (idx, sc) in swapchains.iter_mut().enumerate() {
                let (mon_x, mon_y) = monitor_origins.get(idx).copied().unwrap_or((0, 0));
                let mon_w = sc.width;
                let mon_h = sc.height;

                let _ = sc.render_and_present(
                    ctx,
                    clear_rgba,
                    |cmd_buf, _dst_img| {
                        if !locked {
                            // Transition all captured window textures to SHADER_READ_ONLY_OPTIMAL for alpha-blended sampling
                            for key in &z_ordered_keys {
                                if let Some(entry) = managed_windows.get(key) {
                                    if entry.capture.has_frame {
                                        if let Some(ref tex) = entry.vk_tex {
                                            unsafe {
                                                ctx.record_transition_for_sampling(cmd_buf, tex.image);
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
                            let lock_colors = MacBorderColors::for_state(theme_mode, true);
                            pipeline.record_mac_dual_rim_border(
                                device,
                                cmd_buf,
                                &lock_quad,
                                mon_x,
                                mon_y,
                                mon_w,
                                mon_h,
                                lock_colors,
                                corner_radius,
                            );
                        } else {
                            let unfocused_colors = MacBorderColors::for_state(theme_mode, false);
                            let focused_colors = MacBorderColors::for_state(theme_mode, true);
                            let popup_colors = MacBorderColors::for_state(theme_mode, true);

                            // Draw all windows in back-to-front Z-order with premultiplied alpha blending
                            for key in &z_ordered_keys {
                                if let Some(entry) = managed_windows.get(key) {
                                    if !entry.capture.has_frame {
                                        continue;
                                    }
                                    let is_focused = *key == (fg_hwnd.0 as isize);
                                    let is_tiled = entry.role == WindowRole::Tiled;
                                    // Forced border radius is ONLY for tiled windows!
                                    let quad_radius = if is_tiled { corner_radius } else { 0 };
                                    let force_opaque =
                                        matches!(entry.role, WindowRole::Tiled | WindowRole::Desktop);

                                    if let Some(ds) = entry.descriptor_set {
                                        let uv_scale = if let Some(ref tex) = entry.vk_tex {
                                            [
                                                (entry.capture.width as f32
                                                    / tex.width.max(1) as f32)
                                                    .clamp(0.0, 1.0),
                                                (entry.capture.height as f32
                                                    / tex.height.max(1) as f32)
                                                    .clamp(0.0, 1.0),
                                            ]
                                        } else {
                                            [1.0, 1.0]
                                        };
                                        pipeline.record_textured_quad(
                                            device,
                                            cmd_buf,
                                            ds,
                                            &entry.quad,
                                            mon_x,
                                            mon_y,
                                            mon_w,
                                            mon_h,
                                            quad_radius,
                                            force_opaque,
                                            uv_scale,
                                        );
                                    }

                                    match entry.role {
                                        WindowRole::Desktop
                                        | WindowRole::TransparentOverlay
                                        | WindowRole::Floating => {}
                                        WindowRole::Roobar => {
                                            // Roobar, when not floating (docked full monitor width),
                                            // only renders border on bottom and has 0 border radius.
                                            let bar_w = entry.quad.width.round() as i32;
                                            let bar_x = (entry.quad.physics.position.x.round() as i32) - mon_x;
                                            let is_docked = bar_x <= 2 && bar_w >= (mon_w as i32 - 4);
                                            if is_docked {
                                                pipeline.record_mac_dual_rim_border_ex(
                                                    device,
                                                    cmd_buf,
                                                    &entry.quad,
                                                    mon_x,
                                                    mon_y,
                                                    mon_w,
                                                    mon_h,
                                                    unfocused_colors,
                                                    0,
                                                    true,
                                                );
                                            }
                                        }
                                        WindowRole::Tiled => {
                                            let colors = if is_focused {
                                                focused_colors
                                            } else {
                                                unfocused_colors
                                            };
                                            pipeline.record_mac_dual_rim_border(
                                                device,
                                                cmd_buf,
                                                &entry.quad,
                                                mon_x,
                                                mon_y,
                                                mon_w,
                                                mon_h,
                                                colors,
                                                corner_radius,
                                            );
                                            if is_focused {
                                                for popup in &active_popups {
                                                    let pquad =
                                                        HierarchyTracker::compute_relative_popup_quad(
                                                            popup,
                                                            &entry.quad,
                                                        );
                                                    pipeline.record_mac_dual_rim_border(
                                                        device,
                                                        cmd_buf,
                                                        &pquad,
                                                        mon_x,
                                                        mon_y,
                                                        mon_w,
                                                        mon_h,
                                                        popup_colors,
                                                        0,
                                                    );
                                                }
                                            }
                                        }
                                    }
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

        frame_count += 1;
        if fps_report.elapsed() >= Duration::from_secs(1) {
            let elapsed_secs = fps_report.elapsed().as_secs_f32();
            let measured_fps = (frame_count as f32 / elapsed_secs).round() as u32;
            let target_fps = scheduler.control().effective_target_fps();
            tracing::info!("coop: {} FPS (target: {}Hz)", measured_fps, target_fps);
            frame_count = 0;
            fps_report = Instant::now();
        }

        dt = scheduler.wait_for_next_frame_timer(true);
    }

    if let Some(ref ctx) = vk_ctx {
        ctx.wait_idle();
        for entry in managed_windows.values_mut() {
            if let Some(ds) = entry.descriptor_set.take() {
                unsafe {
                    pipeline.free_texture_descriptor_set(&ctx.device, ds);
                }
            }
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
        unsafe {
            pipeline.destroy(&ctx.device);
        }
    }
    Ok(())
}
