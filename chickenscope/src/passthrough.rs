//! Phase 6.2, 6.3 & 6.4: DirectFlip display passthrough, window unhooking,
//! MMCSS `"Games"` thread priority boost, process priority tuning, and anti-cheat safe input unhooking.

use anyhow::Result;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::core::{PCSTR, PCWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::{
    OpenProcess, SetPriorityClass, ABOVE_NORMAL_PRIORITY_CLASS, BELOW_NORMAL_PRIORITY_CLASS,
    NORMAL_PRIORITY_CLASS, PROCESS_SET_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetWindowThreadProcessId, SetForegroundWindow, SetWindowPos, HWND_BOTTOM,
    HWND_TOPMOST, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
};

/// Global atomic flag indicating whether `chickenscope` Game Mode passthrough is currently active.
pub static GAME_MODE_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Stores the `HWND` of the currently active passthrough game.
pub static ACTIVE_GAME_HWND: AtomicIsize = AtomicIsize::new(0);

/// Stores the `avrt.dll` MMCSS handle returned by `AvSetMmThreadCharacteristicsW`.
static MMCSS_HANDLE: AtomicIsize = AtomicIsize::new(0);

const BACKGROUND_DAEMONS: [&str; 6] = [
    "chickenwm-wm.exe",
    "chickenwm-paper.exe",
    "roobar.exe",
    "chickenwm-notify.exe",
    "chickenwm-launcher.exe",
    "chickenwm-files.exe",
];

type AvSetMmThreadCharacteristicsWFn =
    unsafe extern "system" fn(PCWSTR, *mut u32) -> HANDLE;
type AvRevertMmThreadCharacteristicsFn =
    unsafe extern "system" fn(HANDLE) -> windows::Win32::Foundation::BOOL;

/// Phase 6.4: Boosts current scheduler thread via Multimedia Class Scheduler Service (`AvSetMmThreadCharacteristicsW("Games")`).
fn enable_mmcss_games_boost() {
    unsafe {
        let dll: Vec<u16> = "avrt.dll\0".encode_utf16().collect();
        let Ok(hmod) = LoadLibraryW(PCWSTR(dll.as_ptr())) else {
            return;
        };
        if let Some(proc) = GetProcAddress(hmod, PCSTR(c"AvSetMmThreadCharacteristicsW".as_ptr() as *const u8)) {
            let func: AvSetMmThreadCharacteristicsWFn = std::mem::transmute(proc);
            let task: Vec<u16> = "Games\0".encode_utf16().collect();
            let mut task_index = 0u32;
            let h = func(PCWSTR(task.as_ptr()), &mut task_index);
            if !h.is_invalid() && !h.0.is_null() {
                MMCSS_HANDLE.store(h.0 as isize, Ordering::Release);
            }
        }
    }
}

fn disable_mmcss_games_boost() {
    let raw = MMCSS_HANDLE.swap(0, Ordering::AcqRel);
    if raw == 0 {
        return;
    }
    unsafe {
        let dll: Vec<u16> = "avrt.dll\0".encode_utf16().collect();
        let Ok(hmod) = LoadLibraryW(PCWSTR(dll.as_ptr())) else {
            return;
        };
        if let Some(proc) = GetProcAddress(hmod, PCSTR(c"AvRevertMmThreadCharacteristics".as_ptr() as *const u8)) {
            let func: AvRevertMmThreadCharacteristicsFn = std::mem::transmute(proc);
            let _ = func(HANDLE(raw as *mut core::ffi::c_void));
        }
    }
}

fn set_process_priority_by_pid(pid: u32, priority: windows::Win32::System::Threading::PROCESS_CREATION_FLAGS) {
    if pid == 0 {
        return;
    }
    unsafe {
        if let Ok(proc) = OpenProcess(PROCESS_SET_INFORMATION, false, pid) {
            let _ = SetPriorityClass(proc, priority);
            let _ = CloseHandle(proc);
        }
    }
}

/// Drops or restores all `CoopVulkanCanvasClass` windows in Z-order so the game swapchain
/// can engage hardware DirectFlip (`DXGI_SWAP_EFFECT_FLIP_DISCARD`) with 0ms compositor overhead.
fn set_coop_canvas_yield(yield_to_game: bool) {
    unsafe {
        let class_name: Vec<u16> = "CoopVulkanCanvasClass\0".encode_utf16().collect();
        let mut child = HWND::default();
        let z = if yield_to_game { HWND_BOTTOM } else { HWND_TOPMOST };
        loop {
            let Ok(next) = FindWindowExW(None, child, PCWSTR(class_name.as_ptr()), None) else {
                break;
            };
            if next.0.is_null() {
                break;
            }
            let _ = SetWindowPos(
                next,
                z,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
            );
            child = next;
        }
    }
}

/// Manages native game passthrough state.
pub struct PassthroughManager;

impl PassthroughManager {
    /// Phase 6.2, 6.3 & 6.4: Enters Game Mode:
    /// 1. Unhooks all window chrome/subclasses on `game_hwnd` (`restore_managed_window`).
    /// 2. Yields `coop` canvas Z-order (`HWND_BOTTOM`) to grant native DirectFlip.
    /// 3. Ensures `game_hwnd` is the true OS foreground window (`SetForegroundWindow`).
    /// 4. Boosts game process to `ABOVE_NORMAL_PRIORITY_CLASS` + MMCSS `"Games"`, and lowers background daemons to `BELOW_NORMAL_PRIORITY_CLASS`.
    pub fn enter_game_mode(game_hwnd: HWND) -> Result<()> {
        if game_hwnd.0.is_null() {
            return Ok(());
        }
        let prev = GAME_MODE_ACTIVE.swap(true, Ordering::AcqRel);
        ACTIVE_GAME_HWND.store(game_hwnd.0 as isize, Ordering::Release);

        // Phase 6.2: Remove any borderless resize subclass or DWM cloak/acrylic attributes from the game HWND
        chickenwm_core::sys::restore_managed_window(game_hwnd);
        set_coop_canvas_yield(true);

        unsafe {
            let _ = SetForegroundWindow(game_hwnd);

            // Phase 6.4: Boost game priority
            let mut game_pid = 0u32;
            GetWindowThreadProcessId(game_hwnd, Some(&mut game_pid));
            set_process_priority_by_pid(game_pid, ABOVE_NORMAL_PRIORITY_CLASS);
        }

        if !prev {
            enable_mmcss_games_boost();
            for pid in chickenwm_core::failsafe::find_process_ids(&BACKGROUND_DAEMONS) {
                set_process_priority_by_pid(pid, BELOW_NORMAL_PRIORITY_CLASS);
            }
            tracing::info!("Entered Game Mode DirectFlip passthrough for HWND {:?}", game_hwnd);
        }
        Ok(())
    }

    /// Exits Game Mode: wakes `coop` canvas, restores desktop priorities, and reverts MMCSS boost.
    pub fn exit_game_mode() -> Result<()> {
        if !GAME_MODE_ACTIVE.swap(false, Ordering::AcqRel) {
            return Ok(());
        }
        let prev_hwnd = ACTIVE_GAME_HWND.swap(0, Ordering::AcqRel);
        if prev_hwnd != 0 {
            unsafe {
                let mut game_pid = 0u32;
                GetWindowThreadProcessId(
                    HWND(prev_hwnd as *mut core::ffi::c_void),
                    Some(&mut game_pid),
                );
                set_process_priority_by_pid(game_pid, NORMAL_PRIORITY_CLASS);
            }
        }

        disable_mmcss_games_boost();
        for pid in chickenwm_core::failsafe::find_process_ids(&BACKGROUND_DAEMONS) {
            set_process_priority_by_pid(pid, NORMAL_PRIORITY_CLASS);
        }
        set_coop_canvas_yield(false);

        tracing::info!("Exited Game Mode passthrough; restored coop viewspace.");
        Ok(())
    }

    pub fn is_active() -> bool {
        GAME_MODE_ACTIVE.load(Ordering::Acquire)
    }
}
