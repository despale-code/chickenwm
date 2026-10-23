//! Vulkan device initialization, extensions, and context.

pub mod pipeline;
pub mod swapchain;

use anyhow::Result;

/// Core Vulkan state, device handles, and memory import capabilities.
pub struct VulkanContext {
    pub instance: ash::Instance,
    pub device: ash::Device,
    pub physical_device: ash::vk::PhysicalDevice,
}

impl VulkanContext {
    /// Initializes Vulkan with external memory extensions:
    /// - VK_KHR_external_memory
    /// - VK_KHR_external_memory_win32
    /// - VK_KHR_external_semaphore_win32
    pub fn new() -> Result<Self> {
        // Skeleton: To be implemented by Gemini 4 Argon
        unsafe {
            let entry = ash::Entry::load()?;
            let app_info = ash::vk::ApplicationInfo::default()
                .application_name(c"coop")
                .api_version(ash::vk::make_api_version(0, 1, 3, 0));

            let instance_create_info = ash::vk::InstanceCreateInfo::default()
                .application_info(&app_info);

            let instance = entry.create_instance(&instance_create_info, None)?;
            
            // Dummy placeholder handles for skeleton compilation
            let physical_device = ash::vk::PhysicalDevice::null();
            let device = ash::Device::load(&instance.fp_v1_0(), ash::vk::Device::null());

            Ok(Self {
                instance,
                device,
                physical_device,
            })
        }
    }
}
