use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    chickenwm_core::init_tracing("coop");
    tracing::info!("Initializing coop Vulkan viewspace compositor...");

    // Skeletons:
    // 1. Initialize Vulkan instance and multi-monitor swapchains
    // 2. Start Windows Graphics Capture session manager
    // 3. Start WinEvent hierarchy tracker (#32768 popups)
    // 4. Start input hit-test thread with hardware cursor sync
    // 5. Main presentation loop

    tracing::info!("coop initialized successfully.");
    Ok(())
}
