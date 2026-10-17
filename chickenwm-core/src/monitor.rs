//! Phase 3.1: Monitor & Refresh Rate Discovery (`EnumDisplayMonitors`, `EnumDisplaySettingsExW`,
//! `DXGI_OUTPUT_DESC`, `GetDpiForMonitor`, and `WM_DISPLAYCHANGE` listener).

use crate::ipc::MonitorInfo;

/// Height reserved at the top of the primary monitor for `roobar` (bar + margin).
pub const ROOBAR_RESERVED_TOP_PX: i32 = 44;

/// Resets every monitor's work area to its full bounds, reserving only the `roobar` strip on
/// the primary monitor. Windows keeps explorer's taskbar reservation after explorer exits, which
/// otherwise leaves a dead gap at the bottom of the screen. Broadcasts `WM_SETTINGCHANGE` so
/// running apps (and `chickenwm-wm`) pick up the new area. Explorer re-reserves its taskbar itself
/// when it comes back.
pub fn reserve_work_areas_without_taskbar() {
    use windows::Win32::UI::WindowsAndMessaging::{
        SystemParametersInfoW, SPIF_SENDCHANGE, SPI_SETWORKAREA,
    };
    for m in enumerate_monitors() {
        let top = if m.is_primary {
            ROOBAR_RESERVED_TOP_PX
        } else {
            0
        };
        let mut rect = RECT {
            left: m.x,
            top: m.y + top,
            right: m.x + m.width,
            bottom: m.y + m.height,
        };
        unsafe {
            let _ = SystemParametersInfoW(
                SPI_SETWORKAREA,
                0,
                Some(&mut rect as *mut RECT as *mut core::ffi::c_void),
                SPIF_SENDCHANGE,
            );
        }
    }
}
use std::mem::size_of;
use windows::core::PCWSTR;
use windows::Win32::Foundation::{BOOL, LPARAM, RECT};
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, EnumDisplaySettingsExW, GetMonitorInfoW, DEVMODEW,
    ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_FLAGS, HDC, HMONITOR, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

const MONITORINFOF_PRIMARY_FLAG: u32 = 1;

unsafe extern "system" fn monitor_enum_proc(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _lprect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    unsafe {
        let monitors = &mut *(lparam.0 as *mut Vec<MonitorInfo>);
        let mut mi_ex = MONITORINFOEXW::default();
        mi_ex.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;

        if GetMonitorInfoW(hmonitor, &mut mi_ex.monitorInfo).as_bool() {
            let len = mi_ex
                .szDevice
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(mi_ex.szDevice.len());
            let dev_name = String::from_utf16_lossy(&mi_ex.szDevice[..len]);

            // Query native refresh rate via EnumDisplaySettingsExW(ENUM_CURRENT_SETTINGS)
            let mut dev_mode = DEVMODEW::default();
            dev_mode.dmSize = size_of::<DEVMODEW>() as u16;
            let mut refresh_hz = 60u32;
            if EnumDisplaySettingsExW(
                PCWSTR(mi_ex.szDevice.as_ptr()),
                ENUM_CURRENT_SETTINGS,
                &mut dev_mode,
                ENUM_DISPLAY_SETTINGS_FLAGS(0),
            )
            .as_bool()
            {
                if dev_mode.dmDisplayFrequency > 1 {
                    refresh_hz = dev_mode.dmDisplayFrequency;
                }
            }

            // Query DPI scale factor via GetDpiForMonitor
            let mut dpi_x = 96u32;
            let mut dpi_y = 96u32;
            let _ = GetDpiForMonitor(hmonitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            let dpi_scale = (dpi_x as f32 / 96.0).max(1.0);

            let rc = mi_ex.monitorInfo.rcMonitor;
            let work = mi_ex.monitorInfo.rcWork;
            let is_primary = (mi_ex.monitorInfo.dwFlags & MONITORINFOF_PRIMARY_FLAG) != 0;
            let id = monitors.len();

            monitors.push(MonitorInfo {
                id,
                name: dev_name.clone(),
                adapter_name: dev_name,
                width: rc.right - rc.left,
                height: rc.bottom - rc.top,
                x: rc.left,
                y: rc.top,
                work_rect: [
                    work.left,
                    work.top,
                    work.right - work.left,
                    work.bottom - work.top,
                ],
                refresh_rate_hz: refresh_hz,
                dpi_scale,
                active_workspace_id: 1,
                is_primary,
            });
        }

        BOOL(1)
    }
}

/// Enumerates all connected monitors and queries their exact refresh rate in Hz,
/// DPI scale factor, bounds, and work area rect.
/// Also enriches adapter descriptions via DXGI (`CreateDXGIFactory1` / `IDXGIOutput`).
pub fn enumerate_monitors() -> Vec<MonitorInfo> {
    let mut monitors: Vec<MonitorInfo> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(monitor_enum_proc),
            LPARAM(&mut monitors as *mut Vec<MonitorInfo> as isize),
        );

        // Enrich with DXGI output descriptors when available
        if let Ok(factory) = CreateDXGIFactory1::<IDXGIFactory1>() {
            let mut adapter_idx = 0;
            while let Ok(adapter) = factory.EnumAdapters1(adapter_idx) {
                adapter_idx += 1;
                let adapter_desc_str = adapter
                    .GetDesc1()
                    .ok()
                    .map(|d| {
                        let end = d
                            .Description
                            .iter()
                            .position(|&c| c == 0)
                            .unwrap_or(d.Description.len());
                        String::from_utf16_lossy(&d.Description[..end])
                    })
                    .unwrap_or_default();

                let mut output_idx = 0;
                while let Ok(output) = adapter.EnumOutputs(output_idx) {
                    output_idx += 1;
                    if let Ok(out_desc) = output.GetDesc() {
                        let end = out_desc
                            .DeviceName
                            .iter()
                            .position(|&c| c == 0)
                            .unwrap_or(out_desc.DeviceName.len());
                        let out_name = String::from_utf16_lossy(&out_desc.DeviceName[..end]);
                        if let Some(m) = monitors.iter_mut().find(|m| m.name == out_name) {
                            if !adapter_desc_str.is_empty() {
                                m.adapter_name = adapter_desc_str.clone();
                            }
                        }
                    }
                }
            }
        }
    }

    if monitors.is_empty() {
        monitors.push(MonitorInfo {
            id: 0,
            name: r"\\.\DISPLAY1".to_string(),
            adapter_name: "Primary Display".to_string(),
            width: 1920,
            height: 1080,
            x: 0,
            y: 0,
            work_rect: [0, 0, 1920, 1080],
            refresh_rate_hz: 60,
            dpi_scale: 1.0,
            active_workspace_id: 1,
            is_primary: true,
        });
    }

    monitors
}

/// Returns the refresh rate (in Hz) of the monitor containing `(x, y)` or the primary monitor.
pub fn refresh_rate_at_point(x: i32, y: i32, monitors: &[MonitorInfo]) -> u32 {
    for m in monitors {
        if x >= m.x && x < m.x + m.width && y >= m.y && y < m.y + m.height {
            return m.refresh_rate_hz.max(30);
        }
    }
    monitors
        .iter()
        .find(|m| m.is_primary)
        .or_else(|| monitors.first())
        .map(|m| m.refresh_rate_hz.max(30))
        .unwrap_or(60)
}

/// Returns the maximum native refresh rate across all connected monitors.
pub fn max_native_refresh_rate(monitors: &[MonitorInfo]) -> u32 {
    monitors
        .iter()
        .map(|m| m.refresh_rate_hz)
        .max()
        .unwrap_or(60)
        .max(30)
}
