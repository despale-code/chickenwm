//! Phase 5.3 & 5.4: Custom DPMS monitor sleep (`SC_MONITORPOWER`), 0 FPS GPU idle,
//! instant input wake, `Win + L` interception, and Vulkan custom lock screen state.

use crate::vulkan::VulkanContext;
use parking_lot::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, GetLastInputInfo, LASTINPUTINFO, VK_LWIN, VK_RETURN, VK_RWIN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, SendMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, HWND_BROADCAST,
    KBDLLHOOKSTRUCT, SC_MONITORPOWER, WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSCOMMAND, WM_SYSKEYDOWN,
};

/// Atomic flag indicating whether displays are currently powered off via DPMS.
pub static DISPLAY_SLEEPING: AtomicBool = AtomicBool::new(false);

/// Atomic flag indicating whether the custom ChickenWM Vulkan lock screen is active.
pub static SESSION_LOCKED: AtomicBool = AtomicBool::new(false);

/// State for the fullscreen Vulkan lock screen canvas (Phase 5.4).
#[derive(Debug, Clone, Default)]
pub struct LockScreenCanvas {
    pub blur_radius: f32,
    pub pin_buffer: String,
    pub status_message: String,
}

pub static LOCK_SCREEN_STATE: Mutex<LockScreenCanvas> = Mutex::new(LockScreenCanvas {
    blur_radius: 24.0,
    pin_buffer: String::new(),
    status_message: String::new(),
});

unsafe extern "system" fn lock_and_wake_kb_hook(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code >= 0 {
        // Phase 5.3: Any keyboard activity wakes sleeping displays immediately
        if DISPLAY_SLEEPING.load(Ordering::Acquire) {
            PowerManager::wake_displays();
        }

        let msg = wparam.0 as u32;
        if msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN {
            let kb = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            let vk = kb.vkCode as u16;

            // Phase 5.4: Intercept Win + L (with DisableLockWorkstation = 1 active)
            let win_down = unsafe {
                (GetAsyncKeyState(VK_LWIN.0 as i32) as u16 & 0x8000) != 0
                    || (GetAsyncKeyState(VK_RWIN.0 as i32) as u16 & 0x8000) != 0
            };
            if win_down && vk == b'L' as u16 {
                PowerManager::lock_session();
                return LRESULT(1);
            }

            // While locked, swallow keystrokes into the lock canvas prompt
            if SESSION_LOCKED.load(Ordering::Acquire) {
                if vk == VK_RETURN.0 {
                    PowerManager::unlock_session();
                } else if (0x20..=0x7E).contains(&vk) {
                    let mut st = LOCK_SCREEN_STATE.lock();
                    st.pin_buffer.push(vk as u8 as char);
                }
                return LRESULT(1);
            }
        }
    }
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Controls physical display power states via DPMS and manages the custom Vulkan lock screen.
pub struct PowerManager {
    kb_hook: Option<HHOOK>,
}

impl PowerManager {
    /// Installs the low-level input listener for instant DPMS wake and `Win + L` lock interception.
    pub fn start_hook() -> Self {
        let kb_hook = unsafe {
            SetWindowsHookExW(WH_KEYBOARD_LL, Some(lock_and_wake_kb_hook), None, 0).ok()
        };
        Self { kb_hook }
    }

    /// Returns the number of milliseconds elapsed since the last user mouse or keyboard input.
    pub fn idle_duration_ms() -> u32 {
        unsafe {
            let mut lii = LASTINPUTINFO {
                cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
                dwTime: 0,
            };
            if GetLastInputInfo(&mut lii).as_bool() {
                GetTickCount().wrapping_sub(lii.dwTime)
            } else {
                0
            }
        }
    }

    /// Phase 5.3: Checks system idle timeout. If idle exceeds `timeout_ms`, sleeps displays and halts Vulkan queues.
    pub fn poll_idle_timeout(timeout_ms: u32, vk_ctx: Option<&VulkanContext>) -> bool {
        let idle = Self::idle_duration_ms();
        if idle >= timeout_ms && !DISPLAY_SLEEPING.load(Ordering::Acquire) {
            if let Some(ctx) = vk_ctx {
                ctx.wait_idle();
            }
            Self::sleep_displays();
            true
        } else if idle < 250 && DISPLAY_SLEEPING.load(Ordering::Acquire) {
            Self::wake_displays();
            false
        } else {
            DISPLAY_SLEEPING.load(Ordering::Acquire)
        }
    }

    /// Phase 5.3: Powers off all connected displays via DPMS (`SC_MONITORPOWER = 2`) and drops rendering to 0 FPS.
    pub fn sleep_displays() {
        DISPLAY_SLEEPING.store(true, Ordering::Release);
        unsafe {
            let _ = SendMessageW(
                HWND_BROADCAST,
                WM_SYSCOMMAND,
                WPARAM(SC_MONITORPOWER as usize),
                LPARAM(2), // 2 = power off
            );
        }
    }

    /// Phase 5.3: Powers on displays instantly (`SC_MONITORPOWER = -1`) upon detecting mouse or keyboard input.
    pub fn wake_displays() {
        if DISPLAY_SLEEPING.swap(false, Ordering::AcqRel) {
            unsafe {
                let _ = SendMessageW(
                    HWND_BROADCAST,
                    WM_SYSCOMMAND,
                    WPARAM(SC_MONITORPOWER as usize),
                    LPARAM(-1), // -1 = power on
                );
            }
        }
    }

    /// Phase 5.4: Triggers the custom ChickenWM lock screen in Vulkan (`Win + L`).
    pub fn lock_session() {
        let mut st = LOCK_SCREEN_STATE.lock();
        st.pin_buffer.clear();
        st.status_message = "Locked — Press Enter to Unlock".to_string();
        SESSION_LOCKED.store(true, Ordering::Release);
    }

    /// Unlocks the custom lock screen session.
    pub fn unlock_session() {
        let mut st = LOCK_SCREEN_STATE.lock();
        st.pin_buffer.clear();
        st.status_message.clear();
        SESSION_LOCKED.store(false, Ordering::Release);
    }
}

impl Drop for PowerManager {
    fn drop(&mut self) {
        if let Some(h) = self.kb_hook.take() {
            unsafe {
                let _ = UnhookWindowsHookEx(h);
            }
        }
    }
}
