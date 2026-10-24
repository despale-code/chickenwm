//! Phase 2.3: Vulkan device initialization, external Win32 memory import, and zero-copy texture binding.

pub mod pipeline;
pub mod swapchain;

use anyhow::{Context, Result};
use ash::vk;
use windows::Win32::Foundation::HANDLE;

/// Represents a DXGI shared texture imported zero-copy into Vulkan memory.
pub struct ImportedVulkanTexture {
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub view: vk::ImageView,
    pub width: u32,
    pub height: u32,
}

impl ImportedVulkanTexture {
    pub unsafe fn destroy(&mut self, device: &ash::Device) {
        unsafe {
            if self.view != vk::ImageView::null() {
                device.destroy_image_view(self.view, None);
                self.view = vk::ImageView::null();
            }
            if self.image != vk::Image::null() {
                device.destroy_image(self.image, None);
                self.image = vk::Image::null();
            }
            if self.memory != vk::DeviceMemory::null() {
                device.free_memory(self.memory, None);
                self.memory = vk::DeviceMemory::null();
            }
        }
    }
}

/// Core Vulkan state, device handles, and Win32 external memory import capabilities.
pub struct VulkanContext {
    pub entry: ash::Entry,
    pub instance: ash::Instance,
    pub device: ash::Device,
    pub physical_device: vk::PhysicalDevice,
    pub graphics_queue_family: u32,
    pub graphics_queue: vk::Queue,
}

impl VulkanContext {
    /// Initializes Vulkan 1.3 with external memory & Win32 surface extensions:
    /// - `VK_KHR_surface`, `VK_KHR_win32_surface`
    /// - `VK_KHR_swapchain`
    /// - `VK_KHR_external_memory`, `VK_KHR_external_memory_win32`
    /// - `VK_KHR_external_semaphore`, `VK_KHR_external_semaphore_win32`
    pub fn new() -> Result<Self> {
        unsafe {
            let entry =
                ash::Entry::load().context("Failed to load Vulkan loader (vulkan-1.dll)")?;
            let app_info = vk::ApplicationInfo::default()
                .application_name(c"coop")
                .engine_name(c"ChickenWM-Coop")
                .api_version(vk::make_api_version(0, 1, 3, 0));

            let instance_extensions = [
                ash::khr::surface::NAME.as_ptr(),
                ash::khr::win32_surface::NAME.as_ptr(),
                ash::khr::external_memory_capabilities::NAME.as_ptr(),
                ash::khr::external_semaphore_capabilities::NAME.as_ptr(),
            ];

            let instance_create_info = vk::InstanceCreateInfo::default()
                .application_info(&app_info)
                .enabled_extension_names(&instance_extensions);

            let instance = entry
                .create_instance(&instance_create_info, None)
                .context("vkCreateInstance failed")?;

            let pdevices = instance
                .enumerate_physical_devices()
                .context("No Vulkan physical devices found")?;
            let physical_device = *pdevices
                .first()
                .context("Zero Vulkan-capable GPUs enumerated")?;

            let queue_families =
                instance.get_physical_device_queue_family_properties(physical_device);
            let graphics_queue_family = queue_families
                .iter()
                .position(|q| q.queue_flags.contains(vk::QueueFlags::GRAPHICS))
                .unwrap_or(0) as u32;

            let priorities = [1.0f32];
            let queue_info = [vk::DeviceQueueCreateInfo::default()
                .queue_family_index(graphics_queue_family)
                .queue_priorities(&priorities)];

            let device_extensions = [
                ash::khr::swapchain::NAME.as_ptr(),
                ash::khr::external_memory::NAME.as_ptr(),
                ash::khr::external_memory_win32::NAME.as_ptr(),
                ash::khr::external_semaphore::NAME.as_ptr(),
                ash::khr::external_semaphore_win32::NAME.as_ptr(),
            ];

            let device_create_info = vk::DeviceCreateInfo::default()
                .queue_create_infos(&queue_info)
                .enabled_extension_names(&device_extensions);

            let device = instance
                .create_device(physical_device, &device_create_info, None)
                .context("vkCreateDevice failed")?;

            let graphics_queue = device.get_device_queue(graphics_queue_family, 0);

            Ok(Self {
                entry,
                instance,
                device,
                physical_device,
                graphics_queue_family,
                graphics_queue,
            })
        }
    }

    /// Phase 2.3: Imports a DXGI NT shared `HANDLE` (`D3D11_TEXTURE_BIT`)
    /// directly into a `VkImage` (`TRANSFER_SRC | SAMPLED`) and `VkDeviceMemory` with zero CPU copies.
    pub fn import_dxgi_texture(
        &self,
        nt_handle: HANDLE,
        width: u32,
        height: u32,
    ) -> Result<ImportedVulkanTexture> {
        unsafe {
            let handle_type = vk::ExternalMemoryHandleTypeFlags::D3D11_TEXTURE;
            let mut ext_img_info =
                vk::ExternalMemoryImageCreateInfo::default().handle_types(handle_type);

            let img_info = vk::ImageCreateInfo::default()
                .push_next(&mut ext_img_info)
                .image_type(vk::ImageType::TYPE_2D)
                .format(vk::Format::B8G8R8A8_UNORM)
                .extent(vk::Extent3D {
                    width: width.max(1),
                    height: height.max(1),
                    depth: 1,
                })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(
                    vk::ImageUsageFlags::TRANSFER_SRC
                        | vk::ImageUsageFlags::SAMPLED
                        | vk::ImageUsageFlags::TRANSFER_DST,
                )
                .sharing_mode(vk::SharingMode::EXCLUSIVE);

            let image = self
                .device
                .create_image(&img_info, None)
                .context("vkCreateImage for imported DXGI texture failed")?;

            let mem_reqs = self.device.get_image_memory_requirements(image);
            let ext_mem_fn =
                ash::khr::external_memory_win32::Device::new(&self.instance, &self.device);

            let mut handle_props = vk::MemoryWin32HandlePropertiesKHR::default();
            let _ = ext_mem_fn.get_memory_win32_handle_properties(
                handle_type,
                nt_handle.0 as isize,
                &mut handle_props,
            );

            let mem_props = self
                .instance
                .get_physical_device_memory_properties(self.physical_device);
            let mut allowed_bits = mem_reqs.memory_type_bits & handle_props.memory_type_bits;
            if allowed_bits == 0 {
                allowed_bits = mem_reqs.memory_type_bits;
            }

            let mem_type_index = (0..mem_props.memory_type_count)
                .find(|&i| {
                    (allowed_bits & (1 << i)) != 0
                        && mem_props.memory_types[i as usize]
                            .property_flags
                            .contains(vk::MemoryPropertyFlags::DEVICE_LOCAL)
                })
                .or_else(|| (0..mem_props.memory_type_count).find(|&i| (allowed_bits & (1 << i)) != 0))
                .unwrap_or(0);

            let mut dedicated_info = vk::MemoryDedicatedAllocateInfo::default().image(image);
            let mut import_info = vk::ImportMemoryWin32HandleInfoKHR::default()
                .handle_type(handle_type)
                .handle(nt_handle.0 as isize);

            let alloc_info = vk::MemoryAllocateInfo::default()
                .push_next(&mut dedicated_info)
                .push_next(&mut import_info)
                .allocation_size(mem_reqs.size)
                .memory_type_index(mem_type_index);

            let memory = self
                .device
                .allocate_memory(&alloc_info, None)
                .context("vkAllocateMemory with Win32 NT handle failed")?;

            self.device
                .bind_image_memory(image, memory, 0)
                .context("vkBindImageMemory failed")?;

            let view_info = vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(vk::Format::B8G8R8A8_UNORM)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });

            let view = self
                .device
                .create_image_view(&view_info, None)
                .context("vkCreateImageView failed")?;

            Ok(ImportedVulkanTexture {
                image,
                memory,
                view,
                width,
                height,
            })
        }
    }

    /// Records a hardware-filtered `vkCmdBlitImage` from an imported WGC window texture (`src_img`)
    /// onto the monitor swapchain (`dst_img`, currently in `TRANSFER_DST_OPTIMAL`) at the window quad's
    /// spring-animated monitor-local rectangle `(dst_x0, dst_y0)..(dst_x1, dst_y1)`.
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn record_blit_window(
        &self,
        cmd_buf: vk::CommandBuffer,
        src_img: vk::Image,
        src_w: u32,
        src_h: u32,
        dst_img: vk::Image,
        dst_x0: i32,
        dst_y0: i32,
        dst_x1: i32,
        dst_y1: i32,
        mon_w: u32,
        mon_h: u32,
    ) {
        let total_w = (dst_x1 - dst_x0).max(1) as f32;
        let total_h = (dst_y1 - dst_y0).max(1) as f32;

        let cx0 = dst_x0.clamp(0, mon_w as i32);
        let cy0 = dst_y0.clamp(0, mon_h as i32);
        let cx1 = dst_x1.clamp(0, mon_w as i32);
        let cy1 = dst_y1.clamp(0, mon_h as i32);

        if cx1 <= cx0 || cy1 <= cy0 {
            return;
        }

        let u0 = ((cx0 - dst_x0) as f32 / total_w).clamp(0.0, 1.0);
        let v0 = ((cy0 - dst_y0) as f32 / total_h).clamp(0.0, 1.0);
        let u1 = ((cx1 - dst_x0) as f32 / total_w).clamp(0.0, 1.0);
        let v1 = ((cy1 - dst_y0) as f32 / total_h).clamp(0.0, 1.0);

        let sx0 = (u0 * src_w as f32).round() as i32;
        let sy0 = (v0 * src_h as f32).round() as i32;
        let sx1 = (u1 * src_w as f32).round().max((sx0 + 1) as f32) as i32;
        let sy1 = (v1 * src_h as f32).round().max((sy0 + 1) as f32) as i32;

        unsafe {
            let src_barrier = [vk::ImageMemoryBarrier::default()
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ)
                .image(src_img)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                })];

            self.device.cmd_pipeline_barrier(
                cmd_buf,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &src_barrier,
            );

            let region = [vk::ImageBlit {
                src_subresource: vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                },
                src_offsets: [
                    vk::Offset3D {
                        x: sx0.min(src_w as i32),
                        y: sy0.min(src_h as i32),
                        z: 0,
                    },
                    vk::Offset3D {
                        x: sx1.min(src_w as i32),
                        y: sy1.min(src_h as i32),
                        z: 1,
                    },
                ],
                dst_subresource: vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                },
                dst_offsets: [
                    vk::Offset3D {
                        x: cx0,
                        y: cy0,
                        z: 0,
                    },
                    vk::Offset3D {
                        x: cx1,
                        y: cy1,
                        z: 1,
                    },
                ],
            }];

            self.device.cmd_blit_image(
                cmd_buf,
                src_img,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                dst_img,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &region,
                vk::Filter::LINEAR,
            );
        }
    }

    /// Phase 5.3: Halts GPU queues and drops rendering to 0 FPS during DPMS sleep or Game Mode passthrough.
    pub fn wait_idle(&self) {
        unsafe {
            let _ = self.device.device_wait_idle();
        }
    }
}

impl Drop for VulkanContext {
    fn drop(&mut self) {
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}
