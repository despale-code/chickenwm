//! In-game transparent Vulkan HUD overlay.

use anyhow::Result;

/// Controls the in-game transparent HUD overlay.
pub struct GameOverlay;

impl GameOverlay {
    /// Toggles the overlay on/off without interrupting the running game.
    pub fn toggle() -> Result<()> {
        // Skeleton: To be implemented by Gemini 4 Argon
        // Triggered by Super + Tab: displays lightweight coop viewspace over game
        Ok(())
    }
}
