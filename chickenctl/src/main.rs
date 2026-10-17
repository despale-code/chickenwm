//! Phase 1.2: `chickenctl` CLI Control Utility for ChickenWM.
//! Sends IPC commands over `\\.\pipe\chickenwm-ipc` and inspects window/monitor/daemon state.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use chickenwm_core::failsafe::trigger_emergency_restore;
use chickenwm_core::ipc::{send_ipc_command, Action, IpcCommand, IpcResponse};
use chickenwm_core::monitor::enumerate_monitors;

#[derive(Parser, Debug)]
#[command(
    name = "chickenctl",
    version,
    about = "ChickenWM CLI Control & Inspection Utility (Hyprland-compatible)"
)]
struct Cli {
    /// Output response formatted as JSON (`-j` / `--json`).
    #[arg(short = 'j', long = "json", global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Hot-reload `~/.config/chickenwm/chickenwm.conf` and CSS stylesheets.
    Reload,
    /// Issue a window manager or shell dispatcher (`chickenctl dispatch <dispatcher> [arg]`).
    Dispatch {
        /// Dispatcher name (e.g. `exec`, `killactive`, `togglefloating`, `fullscreen`, `workspace`, `movefocus`).
        dispatcher: String,
        /// Optional argument passed to the dispatcher.
        #[arg(default_value = "")]
        arg: String,
    },
    /// List all managed top-level windows and their workspace/geometry state.
    Clients,
    /// List all connected monitors, resolutions, DPI scales, and native refresh rates (Hz).
    Monitors,
    /// List workspaces (1..10) and their active/occupied status (`roobar` state).
    Workspaces,
    /// Set or reset the target animation/render framerate (`0` = follow monitor native refresh rate).
    Fps {
        /// Target FPS (`0` for native monitor refresh rate, or custom Hz like `60`, `144`, `240`).
        fps: u32,
    },
    /// Summon or toggle `chickenwm-launcher`.
    Launcher,
    /// Query ChickenWM daemon, window manager, and module watchdog status.
    Status,
    /// Set wallpaper image or live `.wgsl` shader on `chickenwm-paper`.
    Wallpaper {
        /// Path to PNG/JPEG/WebP image or `.wgsl` shader file.
        path: String,
        /// Optional target monitor name (e.g. `\\.\DISPLAY1`).
        #[arg(short, long)]
        monitor: Option<String>,
        /// Transition style (`fade`, `wipe`, `outer`, `grow`).
        #[arg(short, long, default_value = "fade")]
        transition: String,
    },
    /// Emergency failsafe: immediately restore `explorer.exe` shell and stop ChickenWM.
    RestoreExplorer,
    /// Unhide and restore all desktop windows that were hidden on other workspaces.
    RestoreWindows,
    /// Show or change the system theme (`chickenctl theme`, `chickenctl theme "#242321" --mode dark`).
    Theme {
        /// New accent color (`#rrggbb` or `rgb(rrggbb)`). Omit to re-apply the current theme.
        accent: Option<String>,
        /// `light` or `dark`.
        #[arg(short, long)]
        mode: Option<String>,
        /// Icon theme: `system`, `chickenwm` or a folder name under `~/.config/chickenwm/icons`.
        #[arg(short, long)]
        icons: Option<String>,
    },
    /// Render the generated theme wallpaper to a PNG (defaults: current theme, 3840x2160).
    RenderWallpaper {
        /// Output PNG path.
        out: std::path::PathBuf,
        #[arg(long, default_value_t = 3840)]
        width: u32,
        #[arg(long, default_value_t = 2160)]
        height: u32,
        /// Accent to render with instead of the configured one.
        #[arg(short, long)]
        accent: Option<String>,
        /// `light` or `dark` instead of the configured mode.
        #[arg(short, long)]
        mode: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::RestoreExplorer => {
            let _ = send_ipc_command(&IpcCommand::RestoreExplorer).await;
            trigger_emergency_restore(false);
            println!("ok: explorer.exe restored as default shell and launched.");
            return Ok(());
        }
        Commands::Theme { accent, mode, icons } => {
            use chickenwm_core::theme;
            let (mut doc, cfg) = chickenwm_core::bootstrap_config_and_themes()?;
            let mut t = cfg.theme.clone();
            if let Some(a) = accent {
                t.accent = chickenwm_core::config::RgbaColor::parse(a.trim())
                    .map(|c| chickenwm_core::config::RgbaColor { a: 255, ..c })
                    .with_context(|| format!("not a color: {a}"))?;
            }
            if let Some(m) = mode {
                t.mode = theme::ThemeMode::parse(&m).with_context(|| format!("mode must be light or dark, not {m}"))?;
            }
            if let Some(i) = icons {
                t.icon_theme = i;
            }
            let palette = theme::apply_theme(&t)?;
            doc.set_category_key("theme", "accent", &theme::hypr_rgb(t.accent));
            doc.set_category_key("theme", "mode", t.mode.as_str());
            doc.set_category_key("theme", "icon_theme", &t.icon_theme);
            doc.set_category_key("general", "col.active_border", &theme::hypr_color(palette.active_border()));
            chickenwm_core::config::save_config_document(&doc)?;
            let _ = send_ipc_command(&IpcCommand::ReloadConfig).await;
            if cli.json {
                println!("{}", serde_json::json!({
                    "accent": theme::css(t.accent), "mode": t.mode.as_str(), "icon_theme": t.icon_theme,
                    "base": theme::css(palette.base), "surface": theme::css(palette.surface),
                    "fg": theme::css(palette.fg), "accent_fg": theme::css(palette.accent_fg),
                }));
            } else {
                println!("ok: theme {} ({}), icons {}", theme::css(t.accent), t.mode.as_str(), t.icon_theme);
                println!("    base {}  surface {}  text {}  accent {}  on-accent {}",
                    theme::css(palette.base), theme::css(palette.surface), theme::css(palette.fg),
                    theme::css(palette.accent), theme::css(palette.accent_fg));
            }
            return Ok(());
        }
        Commands::RenderWallpaper { out, width, height, accent, mode } => {
            use chickenwm_core::theme;
            let cfg = chickenwm_core::config::load_or_init_config().map(|(_, c)| c.theme).unwrap_or_default();
            let accent = match accent {
                Some(a) => chickenwm_core::config::RgbaColor::parse(a.trim()).with_context(|| format!("not a color: {a}"))?,
                None => cfg.accent,
            };
            let mode = match mode {
                Some(m) => theme::ThemeMode::parse(&m).with_context(|| format!("mode must be light or dark, not {m}"))?,
                None => cfg.mode,
            };
            let palette = theme::Palette::new(chickenwm_core::config::RgbaColor { a: 255, ..accent }, mode);
            chickenwm_core::wallpaper::save_png(&palette, width, height, &out)?;
            println!("ok: {}x{} {} wallpaper ({}) written to {}", width, height, mode.as_str(), theme::css(accent), out.display());
            return Ok(());
        }
        Commands::RestoreWindows => {
            let count = chickenwm_core::failsafe::restore_all_desktop_windows();
            println!("ok: restored {count} hidden desktop windows.");
            return Ok(());
        }
        Commands::Monitors => {
            match send_ipc_command(&IpcCommand::QueryMonitors).await {
                Ok(IpcResponse::Monitors(monitors)) => print_monitors(&monitors, cli.json)?,
                _ => {
                    // Fallback to direct Win32/DXGI monitor query if daemon is not running
                    let monitors = enumerate_monitors();
                    print_monitors(&monitors, cli.json)?;
                }
            }
            return Ok(());
        }
        _ => {}
    }

    let ipc_cmd = match cli.command {
        Commands::Reload => IpcCommand::ReloadConfig,
        Commands::Dispatch { dispatcher, arg } => {
            let action = Action::parse_dispatcher(&dispatcher, &arg).with_context(|| {
                format!("Unknown dispatcher '{dispatcher}' with argument '{arg}'")
            })?;
            IpcCommand::Dispatch(action)
        }
        Commands::Clients => IpcCommand::QueryClients,
        Commands::Workspaces => IpcCommand::QueryBarState,
        Commands::Fps { fps } => IpcCommand::SetFramerate(if fps == 0 { None } else { Some(fps) }),
        Commands::Launcher => IpcCommand::SummonLauncher,
        Commands::Status => IpcCommand::QueryStatus,
        Commands::Wallpaper {
            path,
            monitor,
            transition,
        } => IpcCommand::SetWallpaper {
            monitor,
            path,
            transition: Some(transition),
        },
        Commands::Monitors | Commands::RestoreExplorer | Commands::RestoreWindows | Commands::Theme { .. } | Commands::RenderWallpaper { .. } => unreachable!(),
    };

    let response = send_ipc_command(&ipc_cmd).await?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&response)?);
        return Ok(());
    }

    match response {
        IpcResponse::Ok { message } => println!("ok: {message}"),
        IpcResponse::Error { message } => {
            eprintln!("error: {message}");
            std::process::exit(1);
        }
        IpcResponse::Clients(clients) => {
            if clients.is_empty() {
                println!("No managed windows.");
            } else {
                for c in clients {
                    println!(
                        "Window 0x{:X} -> \"{}\":\n\tclass: {}\n\tprocess: {} (pid {})\n\tworkspace: {} ({})\n\tat: {},{}\n\tsize: {}x{}\n\tfloating: {} | fullscreen: {} | pseudo: {} | focused: {}\n\topacity: {:.2}\n",
                        c.hwnd,
                        c.title,
                        c.class,
                        c.process_name,
                        c.pid,
                        c.workspace_id,
                        c.monitor_name,
                        c.rect[0],
                        c.rect[1],
                        c.rect[2],
                        c.rect[3],
                        c.floating,
                        c.fullscreen,
                        c.pseudo,
                        c.focused,
                        c.opacity
                    );
                }
            }
        }
        IpcResponse::Monitors(monitors) => print_monitors(&monitors, false)?,
        IpcResponse::Status(st) => {
            println!("ChickenWM Daemon v{}", st.version);
            println!("  Uptime:              {}s", st.uptime_secs);
            println!(
                "  Framerate Mode:      {}",
                if st.fps_limit == 0 {
                    "Monitor Native Refresh Rate (Dynamic)".to_string()
                } else {
                    format!("Custom Cap ({} FPS)", st.fps_limit)
                }
            );
            println!("  VFR Idle Sleep:      {}", st.vfr_enabled);
            println!("  Active Layout:       {}", st.active_layout);
            println!("  Explorer Suppressed: {}", st.explorer_suppressed);
            println!("  Winlogon Shell Set:  {}", st.winlogon_shell_replaced);
            println!("  Monitors Connected:  {}", st.monitor_count);
            println!("  Managed Windows:     {}", st.client_count);
            println!("  Active Modules:      {}", st.modules_running.join(", "));
        }
        IpcResponse::BarState(bs) => {
            println!(
                "Active Workspace: {} | Layout: {} | Monitor: {} ({}Hz)",
                bs.active_workspace, bs.active_layout, bs.monitor_name, bs.monitor_refresh_hz
            );
            for w in bs.workspaces {
                println!(
                    "  workspace {:>2}{} -> {} window(s) {}",
                    w.id,
                    if w.active { " [ACTIVE]" } else { "         " },
                    w.client_count,
                    w.focused_title
                        .as_deref()
                        .map(|t| format!("(\"{t}\")"))
                        .unwrap_or_default()
                );
            }
        }
    }

    Ok(())
}

fn print_monitors(monitors: &[chickenwm_core::ipc::MonitorInfo], as_json: bool) -> Result<()> {
    if as_json {
        println!("{}", serde_json::to_string_pretty(monitors)?);
    } else {
        for m in monitors {
            println!(
                "Monitor {} (ID {}){}:\n\tadapter: {}\n\t{}x{} @ {}Hz at {}x{}\n\twork area: {},{} {}x{}\n\tdpi scale: {:.2}\n\tactive workspace: {}\n",
                m.name,
                m.id,
                if m.is_primary { " [PRIMARY]" } else { "" },
                m.adapter_name,
                m.width,
                m.height,
                m.refresh_rate_hz,
                m.x,
                m.y,
                m.work_rect[0],
                m.work_rect[1],
                m.work_rect[2],
                m.work_rect[3],
                m.dpi_scale,
                m.active_workspace_id
            );
        }
    }
    Ok(())
}
