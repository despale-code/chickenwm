//! Game detection heuristics (fullscreen geometry, borderless styles, process matchers).

use windows::Win32::Foundation::HWND;

/// Detects whether an active window is an exclusive or borderless fullscreen game.
pub struct GameDetector;

impl GameDetector {
    /// Evaluates whether the given HWND qualifies for Game Mode passthrough.
    pub fn is_game(_hwnd: HWND) -> bool {
        // Skeleton: To be implemented by Gemini 4 Argon
        // 1. Check window rect matches physical monitor bounds
        // 2. Verify window lacks standard caption (WS_POPUP or WS_THICKFRAME without WS_CAPTION)
        // 3. Match executable name against chickenwm.conf game_rules
        false
    }
}
