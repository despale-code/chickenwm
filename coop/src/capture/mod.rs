//! Phase 2.1 & 2.2: Windows Graphics Capture (WGC) and DXGI zero-copy NT shared handle export.

use anyhow::{Context, Result};
use windows::core::{Interface, HSTRING};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE, HWND, RECT};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Resource, ID3D11Texture2D,
    D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX, D3D11_RESOURCE_MISC_SHARED_NTHANDLE, D3D11_SDK_VERSION,
    D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
};
use windows::Win32::Graphics::Dxgi::Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGIKeyedMutex, IDXGIResource1};
use windows::Win32::System::WinRT::Direct3D11::{
    CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess,
};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::Win32::System::WinRT::RoGetActivationFactory;
use windows::Win32::UI::WindowsAndMessaging::GetClientRect;

/// Shared D3D11 device context used across WGC capture sessions.
pub struct CaptureDevice {
    pub d3d_device: ID3D11Device,
    pub d3d_context: ID3D11DeviceContext,
    pub winrt_device: IDirect3DDevice,
}

impl CaptureDevice {
    pub fn new() -> Result<Self> {
        unsafe {
            let feature_levels = [D3D_FEATURE_LEVEL_11_0];
            let mut d3d_device: Option<ID3D11Device> = None;
            let mut d3d_context: Option<ID3D11DeviceContext> = None;

            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                None,
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&feature_levels),
                D3D11_SDK_VERSION,
                Some(&mut d3d_device),
                None,
                Some(&mut d3d_context),
            )
            .context("D3D11CreateDevice failed")?;

            let d3d_device = d3d_device.context("Missing ID3D11Device")?;
            let d3d_context = d3d_context.context("Missing ID3D11DeviceContext")?;
            let dxgi_device: IDXGIDevice = d3d_device.cast()?;
            let inspectable = CreateDirect3D11DeviceFromDXGIDevice(&dxgi_device)?;
            let winrt_device: IDirect3DDevice = inspectable.cast()?;

            Ok(Self {
                d3d_device,
                d3d_context,
                winrt_device,
            })
        }
    }

    /// Allocates a `B8G8R8A8_UNORM` texture configured with `SHARED_NTHANDLE | SHARED_KEYEDMUTEX`
    /// and exports its NT `HANDLE` via `IDXGIResource1::CreateSharedHandle`.
    pub fn create_shared_texture(
        &self,
        width: u32,
        height: u32,
    ) -> Result<(ID3D11Texture2D, IDXGIKeyedMutex, HANDLE)> {
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width.max(1),
                Height: height.max(1),
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_SHADER_RESOURCE.0 | D3D11_BIND_RENDER_TARGET.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: (D3D11_RESOURCE_MISC_SHARED_NTHANDLE.0
                    | D3D11_RESOURCE_MISC_SHARED_KEYEDMUTEX.0) as u32,
            };

            let mut texture: Option<ID3D11Texture2D> = None;
            self.d3d_device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .context("CreateTexture2D for shared NT handle failed")?;
            let texture = texture.context("Null shared ID3D11Texture2D")?;

            let dxgi_res: IDXGIResource1 = texture.cast()?;
            let nt_handle = dxgi_res
                .CreateSharedHandle(None, GENERIC_ALL.0, None)
                .context("IDXGIResource1::CreateSharedHandle failed")?;

            let keyed_mutex: IDXGIKeyedMutex = texture.cast()?;
            Ok((texture, keyed_mutex, nt_handle))
        }
    }
}

/// Creates a WinRT `GraphicsCaptureItem` targeting a specific `HWND`.
pub fn create_capture_item_for_hwnd(hwnd: HWND) -> Result<GraphicsCaptureItem> {
    unsafe {
        let class_id = HSTRING::from("Windows.Graphics.Capture.GraphicsCaptureItem");
        let interop: IGraphicsCaptureItemInterop = RoGetActivationFactory(&class_id)
            .context("RoGetActivationFactory for IGraphicsCaptureItemInterop failed")?;
        let item: GraphicsCaptureItem = interop
            .CreateForWindow(hwnd)
            .context("CreateForWindow failed for target HWND")?;
        Ok(item)
    }
}

/// Represents an active zero-copy window capture session.
pub struct WindowCaptureSession {
    pub hwnd: HWND,
    pub shared_handle: HANDLE,
    pub shared_texture: Option<ID3D11Texture2D>,
    pub keyed_mutex: Option<IDXGIKeyedMutex>,
    pub frame_pool: Option<Direct3D11CaptureFramePool>,
    pub session: Option<GraphicsCaptureSession>,
    pub width: u32,
    pub height: u32,
    pub paused: bool,
}

impl WindowCaptureSession {
    /// Starts capturing a Win32 `HWND` using `Windows.Graphics.Capture`.
    /// Configures `IsBorderRequired = false` and `IsCursorCaptureEnabled = false`,
    /// and exports an NT shared `HANDLE` for Vulkan external memory import.
    pub fn start(hwnd: HWND) -> Result<Self> {
        let dev = CaptureDevice::new()?;
        Self::start_with_device(hwnd, &dev)
    }

    /// Starts capturing using an existing shared [`CaptureDevice`].
    pub fn start_with_device(hwnd: HWND, dev: &CaptureDevice) -> Result<Self> {
        let mut rc = RECT::default();
        unsafe {
            let _ = GetClientRect(hwnd, &mut rc);
        }
        let width = (rc.right - rc.left).max(1) as u32;
        let height = (rc.bottom - rc.top).max(1) as u32;

        let (shared_tex, keyed_mutex, shared_handle) = dev.create_shared_texture(width, height)?;

        let item = create_capture_item_for_hwnd(hwnd)?;
        let item_size = item.Size().unwrap_or(SizeInt32 {
            Width: width as i32,
            Height: height as i32,
        });

        let frame_pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &dev.winrt_device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            2,
            item_size,
        )
        .context("Direct3D11CaptureFramePool::CreateFreeThreaded failed")?;

        let session = frame_pool
            .CreateCaptureSession(&item)
            .context("CreateCaptureSession failed")?;

        // Phase 2.1: Disable yellow capture border and OS cursor baking into texture
        let _ = session.SetIsBorderRequired(false);
        let _ = session.SetIsCursorCaptureEnabled(false);
        session.StartCapture().context("StartCapture failed")?;

        Ok(Self {
            hwnd,
            shared_handle,
            shared_texture: Some(shared_tex),
            keyed_mutex: Some(keyed_mutex),
            frame_pool: Some(frame_pool),
            session: Some(session),
            width: item_size.Width.max(1) as u32,
            height: item_size.Height.max(1) as u32,
            paused: false,
        })
    }

    /// Polls the next captured WGC frame and copies it into the shared NT-handle texture
    /// under `IDXGIKeyedMutex` synchronization. Returns `Ok(true)` if a new frame was latched,
    /// and updates `width`/`height` dynamically if the window resized.
    pub fn poll_next_frame(&mut self, dev: &CaptureDevice) -> Result<bool> {
        if self.paused {
            return Ok(false);
        }
        let Some(pool) = &self.frame_pool else {
            return Ok(false);
        };
        let Ok(frame) = pool.TryGetNextFrame() else {
            return Ok(false);
        };

        let content_size = frame.ContentSize()?;
        let new_w = content_size.Width.max(1) as u32;
        let new_h = content_size.Height.max(1) as u32;

        if new_w != self.width || new_h != self.height {
            self.resize_pool(dev, new_w, new_h)?;
        }

        let surface = frame.Surface()?;
        let access: IDirect3DDxgiInterfaceAccess = surface.cast()?;
        unsafe {
            let src_tex: ID3D11Texture2D = access.GetInterface()?;
            if let (Some(dst_tex), Some(mutex)) = (&self.shared_texture, &self.keyed_mutex) {
                if mutex.AcquireSync(0, 5).is_ok() {
                    let dst_res: ID3D11Resource = dst_tex.cast()?;
                    let src_res: ID3D11Resource = src_tex.cast()?;
                    dev.d3d_context.CopyResource(&dst_res, &src_res);
                    let _ = mutex.ReleaseSync(0);
                }
            }
        }
        Ok(true)
    }

    /// Dynamically resizes the frame pool and shared NT texture during live interactive resize (Phase 3.4).
    pub fn resize_pool(&mut self, dev: &CaptureDevice, new_w: u32, new_h: u32) -> Result<()> {
        let w = new_w.max(1);
        let h = new_h.max(1);
        if let Some(pool) = &self.frame_pool {
            pool.Recreate(
                &dev.winrt_device,
                DirectXPixelFormat::B8G8R8A8UIntNormalized,
                2,
                SizeInt32 {
                    Width: w as i32,
                    Height: h as i32,
                },
            )?;
        }
        if !self.shared_handle.is_invalid() && !self.shared_handle.0.is_null() {
            unsafe {
                let _ = CloseHandle(self.shared_handle);
            }
        }
        let (tex, mutex, handle) = dev.create_shared_texture(w, h)?;
        self.shared_texture = Some(tex);
        self.keyed_mutex = Some(mutex);
        self.shared_handle = handle;
        self.width = w;
        self.height = h;
        Ok(())
    }

    /// Phase 4.1: Pauses or resumes capture for occlusion culling on inactive workspaces.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    /// Stops the capture session and releases DXGI resources.
    pub fn stop(&mut self) {
        if let Some(session) = self.session.take() {
            let _ = session.Close();
        }
        if let Some(pool) = self.frame_pool.take() {
            let _ = pool.Close();
        }
        self.shared_texture = None;
        self.keyed_mutex = None;
        if !self.shared_handle.is_invalid() && !self.shared_handle.0.is_null() {
            unsafe {
                let _ = CloseHandle(self.shared_handle);
            }
            self.shared_handle = HANDLE(std::ptr::null_mut());
        }
    }
}

impl Drop for WindowCaptureSession {
    fn drop(&mut self) {
        self.stop();
    }
}
