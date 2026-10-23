use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    chickenwm_core::init_tracing("chickenscope");
    tracing::info!("Starting chickenscope Game Mode daemon...");

    // Skeletons:
    // 1. Listen for window focus changes
    // 2. Evaluate game heuristics (fullscreen, borderless, process regex)
    // 3. Coordinate with coop (sleep compositor on game enter, wake on game exit)
    // 4. Listen for overlay toggle chord (Super + Tab)

    tracing::info!("chickenscope daemon running.");
    Ok(())
}
