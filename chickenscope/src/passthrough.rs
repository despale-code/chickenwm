//! DirectFlip display passthrough, MMCSS process priority boost, and input unhooking.

use anyhow::Result;
use windows::Win32::Foundation::HWND;

/// Manages native game passthrough state.
pub struct PassthroughManager;

impl PassthroughManager {
    /// Enters Game Mode: sleeps coop (0% GPU), unhooks desktop hotkeys, boosts MMCSS priority.
    pub fn enter_game_mode(_game_hwnd: HWND) -> Result<()> {
        // Skeleton: To be implemented by Gemini 4 Argon
        // 1. Send IPC Sleep command to coop
        // 2. Unhook desktop hotkeys in chickenwm-binds (keep only master escape chord)
        // 3. SetPriorityClass(ABOVE_NORMAL_PRIORITY_CLASS)
        // 4. AvSetMmThreadCharacteristicsW("Games")
        Ok(())
    }

    /// Exits Game Mode: wakes coop, restores desktop hotkeys, normalizes priorities.
    pub fn exit_game_mode() -> Result<()> {
        // Skeleton: To be implemented by Gemini 4 Argon
        Ok(())
    }
}
