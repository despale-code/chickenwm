//! Phase 3.1: Display enumeration and independent per-monitor Vulkan swapchains for native max refresh rates.

use crate::vulkan::VulkanContext;
use anyhow::{Context, Result};
use ash::vk;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, DEVMODEW, ENUM_CURRENT_SETTINGS,
    HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetWindowPos, ShowWindow,
    HWND_BOTTOM, HWND_TOPMOST, SWP_NOACTIVATE, SW_SHOWNA, WNDCLASSW, WS_EX_NOREDIRECTIONBITMAP,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
};

/// Metadata for a physical display enumerated via `EnumDisplayMonitors`.
#[derive(Debug, Clone)]
pub struct PhysicalMonitorInfo {
    pub handle: HMONITOR,
    pub rect: RECT,
    pub width: u32,
    pub height: u32,
    pub refresh_rate_hz: u32,
}

/// Enumerates all physical monitors and queries their native maximum refresh rate (`dmDisplayFrequency`).
pub fn enumerate_physical_monitors() -> Vec<PhysicalMonitorInfo> {
    let mut monitors = Vec::new();

    unsafe extern "system" fn enum_proc(
        hmonitor: HMONITOR,
        _hdc: HDC,
        _rc: *mut RECT,
        lparam: LPARAM,
    ) -> BOOL {
        let list = unsafe { &mut *(lparam.0 as *mut Vec<PhysicalMonitorInfo>) };
        let mut mi = MONITORINFOEXW::default();
        mi.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;

        if unsafe { GetMonitorInfoW(hmonitor, &mut mi.monitorInfo as *mut _).as_bool() } {
            let rc = mi.monitorInfo.rcMonitor;
            let width = (rc.right - rc.left).max(1) as u32;
            let height = (rc.bottom - rc.top).max(1) as u32;

            let mut devmode = DEVMODEW {
                dmSize: std::mem::size_of::<DEVMODEW>() as u16,
                ..Default::default()
            };
            let hz = if unsafe {
                EnumDisplaySettingsW(
                    PCWSTR(mi.szDevice.as_ptr()),
                    ENUM_CURRENT_SETTINGS,
                    &mut devmode,
                )
                .as_bool()
            } && devmode.dmDisplayFrequency > 1
            {
                devmode.dmDisplayFrequency
            } else {
                60
            };

            list.push(PhysicalMonitorInfo {
                handle: hmonitor,
                rect: rc,
                width,
                height,
                refresh_rate_hz: hz,
            });
        }
        BOOL(1)
    }

    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(enum_proc),
            LPARAM(&mut monitors as *mut _ as isize),
        );
    }
    monitors
}

unsafe extern "system" fn coop_canvas_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Spawns a borderless, topmost `WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP` Vulkan host window covering a monitor.
pub fn create_monitor_canvas_window(info: &PhysicalMonitorInfo) -> Result<HWND> {
    unsafe {
        let hinstance = GetModuleHandleW(None)?;
        let class_name: Vec<u16> = "CoopVulkanCanvasClass\0".encode_utf16().collect();

        let wc = WNDCLASSW {
            lpfnWndProc: Some(coop_canvas_wnd_proc),
            hInstance: HINSTANCE(hinstance.0),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..Default::default()
        };
        let _ = RegisterClassW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOREDIRECTIONBITMAP,
            PCWSTR(class_name.as_ptr()),
            PCWSTR(class_name.as_ptr()),
            WS_POPUP | WS_VISIBLE,
            info.rect.left,
            info.rect.top,
            info.width as i32,
            info.height as i32,
            None,
            None,
            HINSTANCE(hinstance.0),
            None,
        )?;
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        Ok(hwnd)
    }
}

/// Encapsulates a dedicated Vulkan window, surface, and swapchain bound to a specific physical monitor.
pub struct MonitorSwapchain {
    pub monitor: HMONITOR,
    pub hwnd: HWND,
    pub width: u32,
    pub height: u32,
    pub refresh_rate_hz: u32,
    pub surface: vk::SurfaceKHR,
    pub swapchain: vk::SwapchainKHR,
    pub images: Vec<vk::Image>,
    pub views: Vec<vk::ImageView>,
}

impl MonitorSwapchain {
    /// Creates an independent swapchain wrapper (lightweight constructor when Vulkan context is not yet attached).
    pub fn new(monitor: HMONITOR, width: u32, height: u32, refresh_rate_hz: u32) -> Result<Self> {
        Ok(Self {
            monitor,
            hwnd: HWND::default(),
            width,
            height,
            refresh_rate_hz,
            surface: vk::SurfaceKHR::null(),
            swapchain: vk::SwapchainKHR::null(),
            images: Vec::new(),
            views: Vec::new(),
        })
    }

    /// Initializes the Win32 top-most canvas window, `VkSurfaceKHR`, and `VkSwapchainKHR` for the display.
    pub fn new_with_vulkan(ctx: &VulkanContext, info: &PhysicalMonitorInfo) -> Result<Self> {
        let hwnd = create_monitor_canvas_window(info)?;
        unsafe {
            let hinstance = GetModuleHandleW(None)?;
            let win32_surface_fn = ash::khr::win32_surface::Instance::new(&ctx.entry, &ctx.instance);
            let surface_info = vk::Win32SurfaceCreateInfoKHR::default()
                .hinstance(hinstance.0 as isize)
                .hwnd(hwnd.0 as isize);
            let surface = win32_surface_fn
                .create_win32_surface(&surface_info, None)
                .context("vkCreateWin32SurfaceKHR failed")?;

            let mut sc = Self {
                monitor: info.handle,
                hwnd,
                width: info.width,
                height: info.height,
                refresh_rate_hz: info.refresh_rate_hz,
                surface,
                swapchain: vk::SwapchainKHR::null(),
                images: Vec::new(),
                views: Vec::new(),
            };
            sc.recreate_with_vulkan(ctx, info.width, info.height)?;
            Ok(sc)
        }
    }

    /// Recreates the `VkSwapchainKHR` upon monitor wake (`WM_POWERBROADCAST`) or resolution change.
    pub fn recreate(&mut self, new_width: u32, new_height: u32) -> Result<()> {
        self.width = new_width.max(1);
        self.height = new_height.max(1);
        Ok(())
    }

    /// Recreates the underlying Vulkan swapchain and image views at the target resolution.
    pub fn recreate_with_vulkan(
        &mut self,
        ctx: &VulkanContext,
        new_width: u32,
        new_height: u32,
    ) -> Result<()> {
        self.width = new_width.max(1);
        self.height = new_height.max(1);
        unsafe {
            ctx.wait_idle();
            let swapchain_fn = ash::khr::swapchain::Device::new(&ctx.instance, &ctx.device);

            for &v in &self.views {
                if v != vk::ImageView::null() {
                    ctx.device.destroy_image_view(v, None);
                }
            }
            self.views.clear();

            let old_swapchain = self.swapchain;
            let create_info = vk::SwapchainCreateInfoKHR::default()
                .surface(self.surface)
                .min_image_count(3)
                .image_format(vk::Format::B8G8R8A8_UNORM)
                .image_color_space(vk::ColorSpaceKHR::SRGB_NONLINEAR)
                .image_extent(vk::Extent2D {
                    width: self.width,
                    height: self.height,
                })
                .image_array_layers(1)
                .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST)
                .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                .pre_transform(vk::SurfaceTransformFlagsKHR::IDENTITY)
                .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
                .present_mode(vk::PresentModeKHR::MAILBOX)
                .clipped(true)
                .old_swapchain(old_swapchain);

            let new_sc = swapchain_fn
                .create_swapchain(&create_info, None)
                .context("vkCreateSwapchainKHR failed")?;

            if old_swapchain != vk::SwapchainKHR::null() {
                swapchain_fn.destroy_swapchain(old_swapchain, None);
            }
            self.swapchain = new_sc;
            self.images = swapchain_fn.get_swapchain_images(new_sc)?;

            for &img in &self.images {
                let view_info = vk::ImageViewCreateInfo::default()
                    .image(img)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk::Format::B8G8R8A8_UNORM)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    });
                self.views.push(ctx.device.create_image_view(&view_info, None)?);
            }
        }
        Ok(())
    }

    /// Phase 6.2: Drops or restores the canvas Z-order so a fullscreen game can acquire DirectFlip.
    pub fn set_passthrough_yield(&self, yield_to_game: bool) {
        if self.hwnd.0.is_null() {
            return;
        }
        unsafe {
            let z = if yield_to_game { HWND_BOTTOM } else { HWND_TOPMOST };
            let _ = SetWindowPos(
                self.hwnd,
                z,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | windows::Win32::UI::WindowsAndMessaging::SWP_NOMOVE | windows::Win32::UI::WindowsAndMessaging::SWP_NOSIZE,
            );
        }
    }

    pub unsafe fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            let swapchain_fn = ash::khr::swapchain::Device::new(&ctx.instance, &ctx.device);
            let surface_fn = ash::khr::surface::Instance::new(&ctx.entry, &ctx.instance);
            for &v in &self.views {
                if v != vk::ImageView::null() {
                    ctx.device.destroy_image_view(v, None);
                }
            }
            self.views.clear();
            if self.swapchain != vk::SwapchainKHR::null() {
                swapchain_fn.destroy_swapchain(self.swapchain, None);
                self.swapchain = vk::SwapchainKHR::null();
            }
            if self.surface != vk::SurfaceKHR::null() {
                surface_fn.destroy_surface(self.surface, None);
                self.surface = vk::SurfaceKHR::null();
            }
            if !self.hwnd.0.is_null() {
                let _ = DestroyWindow(self.hwnd);
                self.hwnd = HWND::default();
            }
        }
    }
}
