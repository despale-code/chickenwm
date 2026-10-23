//! `chickenscope`: The Low-Latency Game Mode & In-Game Overlay Engine for ChickenWM.
//!
//! Automatically detects games, steps coop aside into 0% GPU sleep, grants native DirectFlip
//! display passthrough, boosts MMCSS process priority, and provides an in-game HUD overlay.

pub mod detect;
pub mod overlay;
pub mod passthrough;

#[cfg(test)]
mod tests {
    use crate::detect::{GameDetector, KNOWN_GAME_EXECUTABLES, MANUAL_GAME_MODE_OVERRIDE};
    use std::sync::atomic::Ordering;

    #[test]
    fn test_known_game_list_contains_competitive_titles() {
        assert!(KNOWN_GAME_EXECUTABLES.contains(&"cs2.exe"));
        assert!(KNOWN_GAME_EXECUTABLES.contains(&"valorant-win64-shipping.exe"));
        assert!(KNOWN_GAME_EXECUTABLES.contains(&"fortniteclient-win64-shipping.exe"));
    }

    #[test]
    fn test_manual_game_mode_toggle() {
        MANUAL_GAME_MODE_OVERRIDE.store(false, Ordering::Release);
        assert!(GameDetector::toggle_manual_override());
        assert!(!GameDetector::toggle_manual_override());
    }
}
