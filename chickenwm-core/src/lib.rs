//! `chickenwm-core` — Shared core library for the ChickenWM Windows Shell Replacement Suite.
//! Includes:
//! - Asynchronous & blocking Windows Named Pipe IPC (`ipc`)
//! - Emergency Failsafe (`Ctrl+Alt+Shift+Esc`) & Shell Registry Manager (`failsafe`)
//! - `winnow`-based Hyprland `chickenwm.conf` parser & lossless 2-way AST serializer (`config`)
//! - Hot-reloading CSS Theme Engine & `notify` file system watcher (`css`)
//! - Per-monitor refresh rate, DPI, and DXGI discovery (`monitor`)
//! - High-resolution `CreateWaitableTimerExW` + `DwmFlush` Adaptive Frame Scheduler (`frame_scheduler`)
//! - Safe RAII Win32 handle wrappers (`handles`)

pub mod config;
pub mod css;
pub mod failsafe;
pub mod frame_scheduler;
pub mod handles;
pub mod icon_theme;
pub mod ipc;
pub mod monitor;
pub mod theme;
pub mod wallpaper;

use anyhow::Result;

/// Initializes structured `tracing` logging for a ChickenWM binary.
pub fn init_tracing(module_name: &str) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(format!("info,{module_name}=debug")));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

/// Phase 10.3: Ensures `~/.config/chickenwm/chickenwm.conf` and the default dark-mode CSS themes
/// (`launcher.css`, `files.css`, `settings.css`) exist on disk, and returns the parsed config.
pub fn bootstrap_config_and_themes() -> Result<(config::ConfigDocument, config::HyprConfig)> {
    let dir = config::config_dir();
    let missing: Vec<&str> = theme::THEMED_CSS_FILES
        .into_iter()
        .filter(|f| !dir.join(f).exists())
        .collect();
    css::ensure_default_css_files()?;
    let (doc, cfg) = config::load_or_init_config()?;
    // Freshly created CSS files start from the configured theme, not the stock defaults.
    let palette = theme::Palette::new(cfg.theme.accent, cfg.theme.mode);
    for f in missing {
        let _ = theme::apply_palette_to_file(&palette, &dir.join(f));
    }
    if !icon_theme::icons_root().join("chickenwm").join("folder.png").exists() {
        let _ = icon_theme::generate_builtin(&palette);
    }
    Ok((doc, cfg))
}
