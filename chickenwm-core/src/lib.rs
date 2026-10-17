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
pub mod elevation;
pub mod failsafe;
pub mod fonts;
pub mod frame_scheduler;
pub mod handles;
pub mod icon_theme;
pub mod instance;
pub mod ipc;
pub mod monitor;
pub mod theme;
pub mod theme_presets;
pub mod wallpaper;

use anyhow::Result;

/// Initializes structured `tracing` logging for a ChickenWM binary.
///
/// Logs go to stdout and to `~/.config/chickenwm/logs/<module>.log` (the modules have no console
/// when rooster starts them, so without the file a crash leaves no trace). Panics are written
/// there too, with a backtrace.
pub fn init_tracing(module_name: &str) {
    use tracing_subscriber::fmt::writer::MakeWriterExt;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(format!("info,{module_name}=debug")));
    let file = open_log_file(module_name);
    let file_writer = file.map(|f| std::sync::Mutex::new(f));
    match file_writer {
        Some(w) => {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(false)
                .with_ansi(false)
                .with_writer(std::io::stdout.and(w))
                .try_init();
        }
        None => {
            let _ = tracing_subscriber::fmt().with_env_filter(filter).with_target(false).try_init();
        }
    }
    install_panic_log(module_name);
}

/// `logs/<module>.log`, appended; rotated to `.old` past 2 MB so it can't grow forever.
fn open_log_file(module_name: &str) -> Option<std::fs::File> {
    let dir = config::config_dir().join("logs");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{module_name}.log"));
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 2 * 1024 * 1024) {
        let _ = std::fs::rename(&path, dir.join(format!("{module_name}.log.old")));
    }
    std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
}

/// Records panics (message, location, backtrace) in the module's log before the default hook
/// runs, then also in `logs/crashes.log`, one place to look when a module keeps dying.
fn install_panic_log(module_name: &str) {
    let module = module_name.to_string();
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        let thread = std::thread::current().name().unwrap_or("unnamed").to_string();
        let report = format!("PANIC in {module} (thread '{thread}'): {info}\n{backtrace}");
        tracing::error!("{report}");
        let dir = config::config_dir().join("logs");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("crashes.log")) {
            use std::io::Write;
            let _ = writeln!(f, "==== {:?}\n{report}", std::time::SystemTime::now());
        }
        default(info);
    }));
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
    let palette = cfg.theme.palette(cfg.theme.mode);
    for f in missing {
        let _ = theme::apply_palette_to_file(&palette, &dir.join(f));
    }
    if !icon_theme::icons_root().join("chickenwm").join("folder.png").exists() {
        let _ = icon_theme::generate_builtin(&palette);
    }
    Ok((doc, cfg))
}
