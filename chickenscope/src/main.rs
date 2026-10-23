//! `chickenscope` — Game Mode & DirectFlip passthrough engine with MMCSS priority tuning
//! and in-game transparent HUD overlay (`Super + Tab`).

#![windows_subsystem = "windows"]

use anyhow::Result;
use chickenscope::detect::GameDetector;
use chickenscope::overlay::{
    GameOverlay, HOTKEY_ID_GAMEMODE_TOGGLE, HOTKEY_ID_OVERLAY_TOGGLE, OVERLAY_VISIBLE,
};
use chickenscope::passthrough::PassthroughManager;
use std::sync::atomic::Ordering;
use std::time::Duration;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetForegroundWindow, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    WM_HOTKEY,
};

#[tokio::main]
async fn main() -> Result<()> {
    chickenwm_core::init_tracing("chickenscope");
    chickenwm_core::failsafe::install_crash_handler();
    let _failsafe = chickenwm_core::failsafe::spawn_emergency_failsafe_thread();

    tracing::info!("Initializing chickenscope Game Mode & DirectFlip passthrough engine...");

    let _ = GameOverlay::init();

    loop {
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_HOTKEY {
                    match msg.wParam.0 as i32 {
                        HOTKEY_ID_OVERLAY_TOGGLE => {
                            let _ = GameOverlay::toggle();
                        }
                        HOTKEY_ID_GAMEMODE_TOGGLE => {
                            let active = GameDetector::toggle_manual_override();
                            if active {
                                let fg = GetForegroundWindow();
                                let _ = PassthroughManager::enter_game_mode(fg);
                            } else {
                                let _ = PassthroughManager::exit_game_mode();
                            }
                        }
                        _ => {}
                    }
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            if !OVERLAY_VISIBLE.load(Ordering::Acquire) {
                let fg = GetForegroundWindow();
                if GameDetector::is_game(fg) {
                    if !PassthroughManager::is_active() {
                        let _ = PassthroughManager::enter_game_mode(fg);
                    }
                } else if PassthroughManager::is_active() {
                    let _ = PassthroughManager::exit_game_mode();
                }
            }
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}
