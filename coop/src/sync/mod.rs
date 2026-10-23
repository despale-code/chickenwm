//! Phase 2.4: Cross-API synchronization between Direct3D 11 (`IDXGIKeyedMutex` / `ID3D11Fence`) and Vulkan.

use anyhow::{Context, Result};
use ash::vk;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Dxgi::IDXGIKeyedMutex;

/// Manages cross-API synchronization for shared DXGI/Vulkan surfaces.
pub struct CrossApiSync {
    pub shared_fence_handle: Option<HANDLE>,
    pub keyed_mutex: Option<IDXGIKeyedMutex>,
    pub keyed_mutex_key: u64,
    pub timeout_ms: u32,
    pub vk_semaphore: vk::Semaphore,
}

impl CrossApiSync {
    /// Creates a new synchronization wrapper for a captured surface.
    pub fn new(shared_fence: Option<HANDLE>) -> Self {
        Self {
            shared_fence_handle: shared_fence,
            keyed_mutex: None,
            keyed_mutex_key: 0,
            timeout_ms: 5,
            vk_semaphore: vk::Semaphore::null(),
        }
    }

    /// Creates a synchronization wrapper bound to a DXGI keyed mutex.
    pub fn with_keyed_mutex(mutex: IDXGIKeyedMutex, key: u64) -> Self {
        Self {
            shared_fence_handle: None,
            keyed_mutex: Some(mutex),
            keyed_mutex_key: key,
            timeout_ms: 5,
            vk_semaphore: vk::Semaphore::null(),
        }
    }

    /// Acquires ownership of the shared texture for Vulkan rendering via `IDXGIKeyedMutex::AcquireSync`.
    pub fn acquire_for_vulkan(&mut self) -> Result<()> {
        if let Some(mutex) = &self.keyed_mutex {
            unsafe {
                mutex
                    .AcquireSync(self.keyed_mutex_key, self.timeout_ms)
                    .context("IDXGIKeyedMutex::AcquireSync failed for Vulkan pass")?;
            }
        }
        Ok(())
    }

    /// Releases texture ownership back to Direct3D/WGC after Vulkan fragment pass completes.
    pub fn release_from_vulkan(&mut self) -> Result<()> {
        if let Some(mutex) = &self.keyed_mutex {
            unsafe {
                mutex
                    .ReleaseSync(self.keyed_mutex_key)
                    .context("IDXGIKeyedMutex::ReleaseSync failed after Vulkan pass")?;
            }
        }
        Ok(())
    }

    /// Imports an NT fence/semaphore handle into a Vulkan `VkSemaphore` using `VK_KHR_external_semaphore_win32`.
    pub fn import_win32_semaphore(
        instance: &ash::Instance,
        device: &ash::Device,
        nt_handle: HANDLE,
    ) -> Result<vk::Semaphore> {
        unsafe {
            let mut export_info = vk::ExportSemaphoreCreateInfo::default()
                .handle_types(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_WIN32);
            let sem_info = vk::SemaphoreCreateInfo::default().push_next(&mut export_info);
            let semaphore = device
                .create_semaphore(&sem_info, None)
                .context("vkCreateSemaphore failed")?;

            let ext_sem_fn = ash::khr::external_semaphore_win32::Device::new(instance, device);
            let import_info = vk::ImportSemaphoreWin32HandleInfoKHR::default()
                .semaphore(semaphore)
                .handle_type(vk::ExternalSemaphoreHandleTypeFlags::OPAQUE_WIN32)
                .handle(nt_handle.0 as isize);

            ext_sem_fn
                .import_semaphore_win32_handle(&import_info)
                .context("vkImportSemaphoreWin32HandleKHR failed")?;

            Ok(semaphore)
        }
    }
}
