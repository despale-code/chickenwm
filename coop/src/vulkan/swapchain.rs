//! Phase 3.1: Display enumeration, click-through transparent top-most window creation,
//! and independent per-monitor Vulkan swapchains + render/present loop at native max refresh rates.

use crate::vulkan::VulkanContext;
use anyhow::{Context, Result};
use ash::vk;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::DwmExtendFrameIntoClientArea;
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, DEVMODEW, ENUM_CURRENT_SETTINGS,
    HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassW, SetLayeredWindowAttributes,
    SetWindowDisplayAffinity, SetWindowPos, ShowWindow, HTTRANSPARENT, HWND_BOTTOM, HWND_TOPMOST,
    LWA_ALPHA, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SW_SHOWNA, WDA_EXCLUDEFROMCAPTURE,
    WM_NCHITTEST, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_EX_TRANSPARENT, WS_POPUP, WS_VISIBLE,
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
    if msg == WM_NCHITTEST {
        return LRESULT(HTTRANSPARENT as isize);
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Spawns a borderless, click-through, per-pixel alpha transparent Vulkan host window covering a monitor.
/// Configured with `WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE` and `WDA_EXCLUDEFROMCAPTURE`
/// so mouse clicks pass straight through to underlying apps and WGC never captures the canvas itself.
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
            WS_EX_TOPMOST
                | WS_EX_TOOLWINDOW
                | WS_EX_LAYERED
                | WS_EX_TRANSPARENT
                | WS_EX_NOACTIVATE,
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

        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        let margins = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        let _ = chickenwm_core::sys::set_overlay_capture_exclusion(hwnd);
        let _ = ShowWindow(hwnd, SW_SHOWNA);
        Ok(hwnd)
    }
}

/// Encapsulates a dedicated Vulkan window, surface, swapchain, render pass, and command buffers for a monitor.
pub struct MonitorSwapchain {
    pub monitor: HMONITOR,
    pub hwnd: HWND,
    pub width: u32,
    pub height: u32,
    pub refresh_rate_hz: u32,
    pub surface: vk::SurfaceKHR,
    pub swapchain: vk::SwapchainKHR,
    pub render_pass: vk::RenderPass,
    pub images: Vec<vk::Image>,
    pub views: Vec<vk::ImageView>,
    pub framebuffers: Vec<vk::Framebuffer>,
    pub command_pool: vk::CommandPool,
    pub command_buffer: vk::CommandBuffer,
    pub image_available_sem: vk::Semaphore,
    pub render_finished_sem: vk::Semaphore,
    pub in_flight_fence: vk::Fence,
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
            render_pass: vk::RenderPass::null(),
            images: Vec::new(),
            views: Vec::new(),
            framebuffers: Vec::new(),
            command_pool: vk::CommandPool::null(),
            command_buffer: vk::CommandBuffer::null(),
            image_available_sem: vk::Semaphore::null(),
            render_finished_sem: vk::Semaphore::null(),
            in_flight_fence: vk::Fence::null(),
        })
    }

    /// Initializes the Win32 top-most canvas window, `VkSurfaceKHR`, `VkRenderPass`, `VkSwapchainKHR`,
    /// framebuffers, command buffer, and sync primitives for the display.
    pub fn new_with_vulkan(ctx: &VulkanContext, info: &PhysicalMonitorInfo) -> Result<Self> {
        let hwnd = create_monitor_canvas_window(info)?;
        unsafe {
            let hinstance = GetModuleHandleW(None)?;
            let win32_surface_fn =
                ash::khr::win32_surface::Instance::new(&ctx.entry, &ctx.instance);
            let surface_info = vk::Win32SurfaceCreateInfoKHR::default()
                .hinstance(hinstance.0 as isize)
                .hwnd(hwnd.0 as isize);
            let surface = win32_surface_fn
                .create_win32_surface(&surface_info, None)
                .context("vkCreateWin32SurfaceKHR failed")?;

            // Create RenderPass that preserves blitted window textures (LOAD) and transitions to PRESENT_SRC_KHR
            let attachment = [vk::AttachmentDescription::default()
                .format(vk::Format::B8G8R8A8_UNORM)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::LOAD)
                .store_op(vk::AttachmentStoreOp::STORE)
                .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
                .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
                .initial_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .final_layout(vk::ImageLayout::PRESENT_SRC_KHR)];

            let color_ref = [vk::AttachmentReference::default()
                .attachment(0)
                .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];

            let subpass = [vk::SubpassDescription::default()
                .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
                .color_attachments(&color_ref)];

            let rp_info = vk::RenderPassCreateInfo::default()
                .attachments(&attachment)
                .subpasses(&subpass);
            let render_pass = ctx.device.create_render_pass(&rp_info, None)?;

            let pool_info = vk::CommandPoolCreateInfo::default()
                .queue_family_index(ctx.graphics_queue_family)
                .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
            let command_pool = ctx.device.create_command_pool(&pool_info, None)?;

            let alloc_info = vk::CommandBufferAllocateInfo::default()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1);
            let command_buffer = ctx.device.allocate_command_buffers(&alloc_info)?[0];

            let sem_info = vk::SemaphoreCreateInfo::default();
            let image_available_sem = ctx.device.create_semaphore(&sem_info, None)?;
            let render_finished_sem = ctx.device.create_semaphore(&sem_info, None)?;

            let fence_info =
                vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED);
            let in_flight_fence = ctx.device.create_fence(&fence_info, None)?;

            let mut sc = Self {
                monitor: info.handle,
                hwnd,
                width: info.width,
                height: info.height,
                refresh_rate_hz: info.refresh_rate_hz,
                surface,
                swapchain: vk::SwapchainKHR::null(),
                render_pass,
                images: Vec::new(),
                views: Vec::new(),
                framebuffers: Vec::new(),
                command_pool,
                command_buffer,
                image_available_sem,
                render_finished_sem,
                in_flight_fence,
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

    /// Recreates the underlying Vulkan swapchain, image views, and framebuffers at the target resolution.
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
            let surface_fn = ash::khr::surface::Instance::new(&ctx.entry, &ctx.instance);

            for &fb in &self.framebuffers {
                if fb != vk::Framebuffer::null() {
                    ctx.device.destroy_framebuffer(fb, None);
                }
            }
            self.framebuffers.clear();

            for &v in &self.views {
                if v != vk::ImageView::null() {
                    ctx.device.destroy_image_view(v, None);
                }
            }
            self.views.clear();

            let caps = surface_fn
                .get_physical_device_surface_capabilities(ctx.physical_device, self.surface)?;
            let composite_alpha = if caps
                .supported_composite_alpha
                .contains(vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED)
            {
                vk::CompositeAlphaFlagsKHR::PRE_MULTIPLIED
            } else if caps
                .supported_composite_alpha
                .contains(vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED)
            {
                vk::CompositeAlphaFlagsKHR::POST_MULTIPLIED
            } else {
                vk::CompositeAlphaFlagsKHR::OPAQUE
            };

            let present_modes = surface_fn
                .get_physical_device_surface_present_modes(ctx.physical_device, self.surface)?;
            let present_mode = if present_modes.contains(&vk::PresentModeKHR::MAILBOX) {
                vk::PresentModeKHR::MAILBOX
            } else {
                vk::PresentModeKHR::FIFO
            };

            let min_images = (caps.min_image_count + 1).max(2);
            let image_count = if caps.max_image_count > 0 {
                min_images.min(caps.max_image_count)
            } else {
                min_images
            };

            let old_swapchain = self.swapchain;
            let create_info = vk::SwapchainCreateInfoKHR::default()
                .surface(self.surface)
                .min_image_count(image_count)
                .image_format(vk::Format::B8G8R8A8_UNORM)
                .image_color_space(vk::ColorSpaceKHR::SRGB_NONLINEAR)
                .image_extent(vk::Extent2D {
                    width: self.width,
                    height: self.height,
                })
                .image_array_layers(1)
                .image_usage(
                    vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST,
                )
                .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
                .pre_transform(vk::SurfaceTransformFlagsKHR::IDENTITY)
                .composite_alpha(composite_alpha)
                .present_mode(present_mode)
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
                let view = ctx.device.create_image_view(&view_info, None)?;
                self.views.push(view);

                let attachments = [view];
                let fb_info = vk::FramebufferCreateInfo::default()
                    .render_pass(self.render_pass)
                    .attachments(&attachments)
                    .width(self.width)
                    .height(self.height)
                    .layers(1);
                let fb = ctx.device.create_framebuffer(&fb_info, None)?;
                self.framebuffers.push(fb);
            }
        }
        Ok(())
    }

    /// Acquires the next swapchain image, transitions it to `TRANSFER_DST_OPTIMAL`, clears to `clear_rgba`,
    /// invokes `record_blits` to blit captured window textures, then runs the render pass (`record_draws`)
    /// for borders/HUD and presents to the display.
    pub fn render_and_present<B, F>(
        &mut self,
        ctx: &VulkanContext,
        clear_rgba: [f32; 4],
        record_blits: B,
        record_draws: F,
    ) -> Result<()>
    where
        B: FnOnce(vk::CommandBuffer, vk::Image),
        F: FnOnce(&ash::Device, vk::CommandBuffer),
    {
        if self.swapchain == vk::SwapchainKHR::null() {
            return Ok(());
        }
        unsafe {
            let fences = [self.in_flight_fence];
            ctx.device.wait_for_fences(&fences, true, 100_000_000)?;

            let swapchain_fn = ash::khr::swapchain::Device::new(&ctx.instance, &ctx.device);
            let acquire = swapchain_fn.acquire_next_image(
                self.swapchain,
                100_000_000,
                self.image_available_sem,
                vk::Fence::null(),
            );

            let image_index = match acquire {
                Ok((idx, _suboptimal)) => idx,
                Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => {
                    let w = self.width;
                    let h = self.height;
                    self.recreate_with_vulkan(ctx, w, h)?;
                    return Ok(());
                }
                Err(_) => return Ok(()),
            };

            ctx.device.reset_fences(&fences)?;
            ctx.device
                .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())?;

            let begin_info = vk::CommandBufferBeginInfo::default()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
            ctx.device
                .begin_command_buffer(self.command_buffer, &begin_info)?;

            let dst_image = self.images[image_index as usize];
            let subresource = vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            };

            // 1. Transition swapchain image UNDEFINED -> TRANSFER_DST_OPTIMAL
            let to_transfer_dst = [vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .image(dst_image)
                .subresource_range(subresource)];

            ctx.device.cmd_pipeline_barrier(
                self.command_buffer,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &to_transfer_dst,
            );

            // 2. Clear background (dark slate desktop or lock screen backdrop)
            let clear_color = vk::ClearColorValue {
                float32: clear_rgba,
            };
            ctx.device.cmd_clear_color_image(
                self.command_buffer,
                dst_image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &clear_color,
                &[subresource],
            );

            // 3. Blit all captured WGC window textures (and wallpaper) in back-to-front Z-order
            record_blits(self.command_buffer, dst_image);

            // 4. Begin RenderPass (preserves blitted images via LOAD) to draw borders & HUD accents
            let rp_begin = vk::RenderPassBeginInfo::default()
                .render_pass(self.render_pass)
                .framebuffer(self.framebuffers[image_index as usize])
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: vk::Extent2D {
                        width: self.width,
                        height: self.height,
                    },
                });

            ctx.device.cmd_begin_render_pass(
                self.command_buffer,
                &rp_begin,
                vk::SubpassContents::INLINE,
            );

            let viewports = [vk::Viewport {
                x: 0.0,
                y: 0.0,
                width: self.width as f32,
                height: self.height as f32,
                min_depth: 0.0,
                max_depth: 1.0,
            }];
            let scissors = [vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: vk::Extent2D {
                    width: self.width,
                    height: self.height,
                },
            }];
            ctx.device
                .cmd_set_viewport(self.command_buffer, 0, &viewports);
            ctx.device
                .cmd_set_scissor(self.command_buffer, 0, &scissors);

            record_draws(&ctx.device, self.command_buffer);

            ctx.device.cmd_end_render_pass(self.command_buffer);
            ctx.device.end_command_buffer(self.command_buffer)?;

            let wait_sems = [self.image_available_sem];
            let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
            let signal_sems = [self.render_finished_sem];
            let cmd_bufs = [self.command_buffer];

            let submit_info = [vk::SubmitInfo::default()
                .wait_semaphores(&wait_sems)
                .wait_dst_stage_mask(&wait_stages)
                .command_buffers(&cmd_bufs)
                .signal_semaphores(&signal_sems)];

            ctx.device
                .queue_submit(ctx.graphics_queue, &submit_info, self.in_flight_fence)?;

            let swapchains = [self.swapchain];
            let indices = [image_index];
            let present_info = vk::PresentInfoKHR::default()
                .wait_semaphores(&signal_sems)
                .swapchains(&swapchains)
                .image_indices(&indices);

            let _ = swapchain_fn.queue_present(ctx.graphics_queue, &present_info);
        }
        Ok(())
    }

    /// Phase 6.2: Drops or restores the canvas Z-order so a fullscreen game can acquire DirectFlip.
    pub fn set_passthrough_yield(&self, yield_to_game: bool) {
        if self.hwnd.0.is_null() {
            return;
        }
        unsafe {
            let z = if yield_to_game {
                HWND_BOTTOM
            } else {
                HWND_TOPMOST
            };
            let _ = SetWindowPos(
                self.hwnd,
                z,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
        }
    }

    pub unsafe fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            ctx.wait_idle();
            let swapchain_fn = ash::khr::swapchain::Device::new(&ctx.instance, &ctx.device);
            let surface_fn = ash::khr::surface::Instance::new(&ctx.entry, &ctx.instance);
            for &fb in &self.framebuffers {
                if fb != vk::Framebuffer::null() {
                    ctx.device.destroy_framebuffer(fb, None);
                }
            }
            self.framebuffers.clear();
            for &v in &self.views {
                if v != vk::ImageView::null() {
                    ctx.device.destroy_image_view(v, None);
                }
            }
            self.views.clear();
            if self.in_flight_fence != vk::Fence::null() {
                ctx.device.destroy_fence(self.in_flight_fence, None);
                self.in_flight_fence = vk::Fence::null();
            }
            if self.image_available_sem != vk::Semaphore::null() {
                ctx.device.destroy_semaphore(self.image_available_sem, None);
                self.image_available_sem = vk::Semaphore::null();
            }
            if self.render_finished_sem != vk::Semaphore::null() {
                ctx.device.destroy_semaphore(self.render_finished_sem, None);
                self.render_finished_sem = vk::Semaphore::null();
            }
            if self.command_pool != vk::CommandPool::null() {
                ctx.device.destroy_command_pool(self.command_pool, None);
                self.command_pool = vk::CommandPool::null();
            }
            if self.render_pass != vk::RenderPass::null() {
                ctx.device.destroy_render_pass(self.render_pass, None);
                self.render_pass = vk::RenderPass::null();
            }
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
