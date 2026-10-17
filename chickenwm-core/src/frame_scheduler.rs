//! Phase 3.2: Adaptive Frame Scheduler (Default Monitor Sync + Custom FPS Override + Idle VFR).
//! Uses `CreateWaitableTimerExW(CREATE_WAITABLE_TIMER_HIGH_RESOLUTION)`, `QueryPerformanceCounter`,
//! and `DwmFlush()` for sub-millisecond frame pacing synced to the active monitor's native refresh rate
//! or a user-configured `fps_limit`.

use crate::handles::SafeHandle;
use crate::monitor::{enumerate_monitors, max_native_refresh_rate};
use parking_lot::{Condvar, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    CreateWaitableTimerExW, SetWaitableTimer, WaitForSingleObject,
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS,
};

/// Shared controller allowing any thread (WM, IPC, Binds, Settings) to wake the frame loop,
/// update the monitor native refresh rate, or set a custom `fps_limit` / `vfr` mode.
#[derive(Debug)]
pub struct FramePacerControl {
    /// `0` = follow monitor native refresh rate dynamically; `>0` = custom FPS cap.
    fps_limit: AtomicU32,
    /// Detected monitor native refresh rate in Hz (e.g., 60, 144, 165, 240).
    monitor_native_hz: AtomicU32,
    /// Variable Frame Rate: if true, sleep indefinitely when `active_animations == false`.
    vfr_enabled: AtomicBool,
    /// Signal wakeup when new animations or mouse drags begin.
    wake_mutex: Mutex<bool>,
    wake_cv: Condvar,
}

impl FramePacerControl {
    pub fn new(fps_limit: u32, vfr_enabled: bool) -> Arc<Self> {
        let monitors = enumerate_monitors();
        let native_hz = max_native_refresh_rate(&monitors);
        Arc::new(Self {
            fps_limit: AtomicU32::new(fps_limit),
            monitor_native_hz: AtomicU32::new(native_hz),
            vfr_enabled: AtomicBool::new(vfr_enabled),
            wake_mutex: Mutex::new(true),
            wake_cv: Condvar::new(),
        })
    }

    /// Refreshes the cached monitor native refresh rate (called on startup and on `WM_DISPLAYCHANGE`).
    pub fn refresh_monitors(&self) -> u32 {
        let monitors = enumerate_monitors();
        let hz = max_native_refresh_rate(&monitors);
        self.monitor_native_hz.store(hz, Ordering::SeqCst);
        self.wake();
        hz
    }

    pub fn set_monitor_native_hz(&self, hz: u32) {
        self.monitor_native_hz.store(hz.max(30), Ordering::SeqCst);
    }

    /// Sets `fps_limit` (`0` or `None` = follow monitor native refresh rate; `Some(n)` = lock to `n` FPS).
    pub fn set_fps_limit(&self, fps_limit: Option<u32>) {
        self.fps_limit
            .store(fps_limit.unwrap_or(0), Ordering::SeqCst);
        self.wake();
    }

    pub fn fps_limit(&self) -> u32 {
        self.fps_limit.load(Ordering::Relaxed)
    }

    pub fn set_vfr(&self, enabled: bool) {
        self.vfr_enabled.store(enabled, Ordering::SeqCst);
        self.wake();
    }

    pub fn vfr_enabled(&self) -> bool {
        self.vfr_enabled.load(Ordering::Relaxed)
    }

    /// Returns the effective target framerate in Hz (either custom `fps_limit` if `> 0`,
    /// or the active monitor's native refresh rate).
    pub fn effective_target_fps(&self) -> u32 {
        let custom = self.fps_limit.load(Ordering::Relaxed);
        if custom > 0 {
            custom.clamp(15, 1000)
        } else {
            self.monitor_native_hz.load(Ordering::Relaxed).max(30)
        }
    }

    /// Target frame budget in seconds (`1.0 / effective_target_fps()`).
    pub fn target_frame_duration_secs(&self) -> f64 {
        1.0 / (self.effective_target_fps() as f64)
    }

    /// Wakes the frame scheduler immediately from VFR idle sleep.
    pub fn wake(&self) {
        let mut active = self.wake_mutex.lock();
        *active = true;
        self.wake_cv.notify_all();
    }

    /// Blocks the caller until `wake()` is called if VFR is enabled.
    pub fn wait_until_woken(&self) {
        if !self.vfr_enabled.load(Ordering::Relaxed) {
            return;
        }
        let mut active = self.wake_mutex.lock();
        while !*active {
            self.wake_cv.wait(&mut active);
        }
        *active = false;
    }
}

/// High-precision QPC & `CreateWaitableTimerExW(CREATE_WAITABLE_TIMER_HIGH_RESOLUTION)` frame scheduler.
pub struct AdaptiveFrameScheduler {
    control: Arc<FramePacerControl>,
    timer_handle: Option<SafeHandle>,
    qpc_freq: i64,
    last_qpc: i64,
}

impl AdaptiveFrameScheduler {
    pub fn new(control: Arc<FramePacerControl>) -> Self {
        let mut freq = 0i64;
        let mut now = 0i64;
        let timer_handle = unsafe {
            let _ = QueryPerformanceFrequency(&mut freq);
            let _ = QueryPerformanceCounter(&mut now);
            CreateWaitableTimerExW(
                None,
                None,
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
            .ok()
            .and_then(SafeHandle::new)
        };

        Self {
            control,
            timer_handle,
            qpc_freq: freq.max(1),
            last_qpc: now,
        }
    }

    pub fn control(&self) -> Arc<FramePacerControl> {
        Arc::clone(&self.control)
    }

    /// Returns elapsed delta-time (`dt` in seconds) since the previous frame tick using `QueryPerformanceCounter`.
    pub fn sample_delta_seconds(&mut self) -> f32 {
        let mut now = 0i64;
        unsafe {
            let _ = QueryPerformanceCounter(&mut now);
        }
        let delta_ticks = (now - self.last_qpc).max(0);
        self.last_qpc = now;
        let dt = (delta_ticks as f64) / (self.qpc_freq as f64);
        // Clamp huge dt jumps after VFR idle sleep to 1 target frame interval
        let max_dt = (self.control.target_frame_duration_secs() * 2.5).max(0.05);
        dt.min(max_dt) as f32
    }

    /// Paces the current thread to the next frame boundary:
    /// - If `has_active_work` is `false` and `vfr = true`, enters zero-CPU VFR sleep until woken.
    /// - If `fps_limit == 0` (Default Monitor Native Sync Mode), synchronizes with DWM vblank via
    ///   `DwmFlush()` and enforces the active monitor's native refresh period (`1000.0 / Hz` ms).
    /// - If `fps_limit == N` (Custom Override Mode), sleeps using the high-resolution waitable timer
    ///   for the remaining `1000.0 / N` ms budget.
    pub fn wait_for_next_frame(&mut self, has_active_work: bool) -> f32 {
        if !has_active_work && self.control.vfr_enabled() {
            self.control.wait_until_woken();
            // Reset QPC baseline on wake so the first frame of a new animation starts cleanly
            unsafe {
                let _ = QueryPerformanceCounter(&mut self.last_qpc);
            }
            return self.control.target_frame_duration_secs() as f32;
        }

        let custom_fps = self.control.fps_limit();
        let target_secs = self.control.target_frame_duration_secs();

        if custom_fps == 0 {
            // Default Mode: Sync with DWM composition / VBlank first
            let dwm_ok = unsafe { DwmFlush().is_ok() };
            if !dwm_ok {
                self.sleep_high_precision(target_secs);
            } else {
                // Ensure we don't spin faster than the monitor's native refresh rate if DwmFlush returns early
                let elapsed = self.peek_elapsed_seconds();
                if elapsed < target_secs * 0.85 {
                    self.sleep_high_precision(target_secs - elapsed);
                }
            }
        } else {
            // Custom Override Mode (`fps_limit = N`): exact 1000.0 / N ms pacing
            let elapsed = self.peek_elapsed_seconds();
            if elapsed < target_secs {
                self.sleep_high_precision(target_secs - elapsed);
            }
        }

        self.sample_delta_seconds()
    }

    fn peek_elapsed_seconds(&self) -> f64 {
        let mut now = 0i64;
        unsafe {
            let _ = QueryPerformanceCounter(&mut now);
        }
        ((now - self.last_qpc).max(0) as f64) / (self.qpc_freq as f64)
    }

    fn sleep_high_precision(&self, seconds: f64) {
        if seconds <= 0.000_05 {
            return;
        }
        // Negative 100-nanosecond intervals for relative SetWaitableTimer
        let hundred_ns = -((seconds * 10_000_000.0) as i64).max(1);
        if let Some(ref timer) = self.timer_handle {
            unsafe {
                if SetWaitableTimer(timer.raw(), &hundred_ns, 0, None, None, false).is_ok() {
                    let timeout_ms = ((seconds * 1000.0) as u32).saturating_add(5);
                    let _ = WaitForSingleObject(timer.raw(), timeout_ms);
                    return;
                }
            }
        }
        std::thread::sleep(Duration::from_secs_f64(seconds));
    }
}
