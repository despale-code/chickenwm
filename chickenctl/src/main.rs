//! Phase 1.2: `chickenctl` CLI Control Utility for ChickenWM.
//! Sends IPC commands over `\\.\pipe\chickenwm-ipc` and inspects window/monitor/daemon state.

mod doctor;

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
    /// Check the setup (shell, admin task, modules, IPC, DWM hook, failsafe hotkey, logs) and
    /// say what to fix. Pass `--fix` to repair safe issues automatically.
    Doctor {
        /// Automatically repair safe problems (broken shell path, duplicate modules, missing theme/config files, DWM hook injection).
        #[arg(long)]
        fix: bool,
    },
    /// List the system tray icons rooster hosts.
    Tray,
    /// Check chickenwm.conf for mistakes (unknown settings, bad values, unclosed sections) and
    /// print them with line numbers.
    CheckConfig,
    /// Show each windowrulev2 rule, why it can never apply (if so), and which open windows it
    /// matches. Matching is re-run here, so it reflects the current titles.
    #[command(alias = "windowrules")]
    WindowRules,
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
        /// Color scheme: dracula, catppuccin, nord, gruvbox, tokyonight, rosepine, solarized, one,
        /// everforest, kanagawa; `none` = generate from the accent.
        #[arg(short, long)]
        preset: Option<String>,
        /// Set one app's light/dark instead: `--app browser --app-mode dark` (`system` = follow).
        #[arg(long, requires = "app_mode")]
        app: Option<String>,
        #[arg(long)]
        app_mode: Option<String>,
        /// Save the current theme to a shareable file (`--export night.chickentheme`).
        #[arg(long)]
        export: Option<std::path::PathBuf>,
        /// Load and apply a theme file.
        #[arg(long)]
        import: Option<std::path::PathBuf>,
        /// Take the accent colour from the current wallpaper image.
        #[arg(long)]
        from_wallpaper: bool,
        /// Switch light/dark by the clock: `off`, `times` (with --light-at / --dark-at) or `sun`
        /// (sunset to sunrise at --location).
        #[arg(long)]
        auto_dark: Option<String>,
        #[arg(long)]
        light_at: Option<String>,
        #[arg(long)]
        dark_at: Option<String>,
        /// `latitude,longitude` for `--auto-dark sun` (e.g. `41.01,28.97`).
        #[arg(long, allow_hyphen_values = true)]
        location: Option<String>,
    },
    /// Show a notification (`chickenctl notify "Backup done" "42 files copied" --app Robocopy`).
    Notify {
        title: String,
        #[arg(default_value = "")]
        body: String,
        #[arg(short, long, default_value = "chickenctl")]
        app: String,
        /// Seconds on screen (0 = until dismissed).
        #[arg(short, long)]
        timeout: Option<u32>,
        /// Stays until dismissed and ignores do-not-disturb.
        #[arg(long)]
        critical: bool,
    },
    /// Show / hide the notification center (same as Super+N).
    Notifications,
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
        Commands::CheckConfig => {
            let issues = chickenwm_core::config_check::validate_user_config()?;
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&issues)?);
            } else if issues.is_empty() {
                println!("ok: chickenwm.conf has no problems");
            } else {
                for i in &issues {
                    let tag = if i.severity == chickenwm_core::config_check::Severity::Error { "error" } else { "warning" };
                    println!("chickenwm.conf:{}: {tag}: {}", i.line, i.message);
                }
            }
            let errors = issues.iter().any(|i| i.severity == chickenwm_core::config_check::Severity::Error);
            std::process::exit(if errors { 1 } else { 0 });
        }
        Commands::WindowRules => {
            use chickenwm_core::window_rules::{compile, WindowFacts};
            let (_, cfg) = chickenwm_core::config::load_or_init_config()?;
            let rules = compile(&cfg.window_rules);
            // Without a running WM the rules are still checked; only the window matches are skipped.
            let clients = match send_ipc_command(&IpcCommand::QueryClients).await {
                Ok(IpcResponse::Clients(c)) => Some(c),
                _ => None,
            };
            let offline = clients.is_none();
            let clients = clients.unwrap_or_default();
            if cli.json {
                let rows: Vec<serde_json::Value> = rules
                    .iter()
                    .map(|r| {
                        let hits: Vec<u64> = clients
                            .iter()
                            .filter(|c| r.matches(&WindowFacts { class: &c.class, title: &c.title, process: &c.process_name }))
                            .map(|c| c.hwnd)
                            .collect();
                        serde_json::json!({ "effect": r.rule.effect, "matcher": r.rule.matcher, "problems": r.problems(), "windows": hits })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&rows)?);
                return Ok(());
            }
            if rules.is_empty() {
                println!("No window rules in chickenwm.conf.");
            }
            for r in &rules {
                println!("{}, {}", r.rule.effect, r.rule.matcher);
                for p in r.problems() {
                    println!("    ! {p}");
                }
                let mut any = false;
                for c in &clients {
                    if r.matches(&WindowFacts { class: &c.class, title: &c.title, process: &c.process_name }) {
                        println!("    -> 0x{:X} {} \"{}\" (ws {})", c.hwnd, c.process_name, c.title, c.workspace_id);
                        any = true;
                    }
                }
                if offline {
                    println!("    (ChickenWM isn't running: open windows not checked)");
                } else if !any {
                    println!("    (no open window matches)");
                }
            }
            return Ok(());
        }
        Commands::Doctor { fix } => {
            let checks = doctor::run(fix);
            if cli.json {
                println!("{}", serde_json::to_string_pretty(&checks)?);
            } else {
                doctor::print(&checks);
            }
            let failed = checks.iter().any(|c| c.level == doctor::Level::Fail);
            std::process::exit(if failed { 1 } else { 0 });
        }
        Commands::RestoreExplorer => {
            let _ = send_ipc_command(&IpcCommand::RestoreExplorer).await;
            trigger_emergency_restore(false);
            println!("ok: explorer.exe restored as default shell and launched.");
            return Ok(());
        }
        Commands::Theme { accent, mode, icons, preset, app, app_mode, export, import, from_wallpaper, auto_dark, light_at, dark_at, location } => {
            use chickenwm_core::theme;
            if let (Some(app), Some(m)) = (app, app_mode) {
                let m = theme::AppMode::parse(&m).with_context(|| format!("app mode must be system, light or dark, not {m}"))?;
                if theme::app_file(&app).is_none() {
                    anyhow::bail!("unknown app {app}; one of: {}", theme::APPS.iter().map(|a| a.0).collect::<Vec<_>>().join(", "));
                }
                let t = theme::set_app_mode(&app, m)?;
                println!("ok: {app} → {} (uses {})", m.as_str(), t.mode_for(&app).as_str());
                return Ok(());
            }
            use chickenwm_core::theme_tools;
            if let Some(path) = import {
                let name = theme_tools::import_and_apply(&path)?;
                println!("ok: theme \"{name}\" imported and applied");
                return Ok(());
            }
            let (mut doc, cfg) = chickenwm_core::bootstrap_config_and_themes()?;
            if let Some(path) = export {
                let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "My theme".into());
                std::fs::write(&path, theme_tools::export_theme(&cfg.theme, &name))?;
                println!("ok: theme saved to {}", path.display());
                return Ok(());
            }
            let mut t = cfg.theme.clone();
            if from_wallpaper {
                let wp = cfg.wallpapers.first().context("no wallpaper image is set")?;
                let path = chickenwm_core::config::expand_home(&wp.path);
                t.accent = theme_tools::accent_from_image(&path).with_context(|| format!("couldn't find a colourful accent in {}", path.display()))?;
                t.preset.clear();
            }
            let auto_changed = auto_dark.is_some() || light_at.is_some() || dark_at.is_some() || location.is_some();
            if let Some(a) = auto_dark {
                anyhow::ensure!(matches!(a.as_str(), "off" | "times" | "sun"), "--auto-dark must be off, times or sun");
                t.auto_dark.mode = a;
            }
            if let Some(v) = light_at {
                theme_tools::parse_hm(&v).with_context(|| format!("--light-at wants HH:MM, not {v}"))?;
                t.auto_dark.light_at = v;
            }
            if let Some(v) = dark_at {
                theme_tools::parse_hm(&v).with_context(|| format!("--dark-at wants HH:MM, not {v}"))?;
                t.auto_dark.dark_at = v;
            }
            if let Some(loc) = location {
                let mut it = loc.split(',').map(|s| s.trim().parse::<f64>());
                match (it.next(), it.next()) {
                    (Some(Ok(lat)), Some(Ok(lon))) => {
                        t.auto_dark.latitude = Some(lat);
                        t.auto_dark.longitude = Some(lon);
                    }
                    _ => anyhow::bail!("--location wants latitude,longitude"),
                }
            }
            if auto_changed {
                theme_tools::write_auto_dark_keys(&mut doc, &t.auto_dark);
                let (y, m, d, minute, offset) = theme_tools::local_clock();
                if let Some(want) = theme_tools::desired_mode(&t.auto_dark, y, m, d, minute, offset) {
                    t.mode = want;
                }
            }
            if let Some(a) = &accent {
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
            if let Some(p) = preset {
                t.preset = if p.eq_ignore_ascii_case("none") || p.eq_ignore_ascii_case("chickenwm") {
                    String::new()
                } else {
                    theme::theme_preset_id(&p).with_context(|| {
                        format!("unknown preset {p}; one of: {}", chickenwm_core::theme_presets::PRESETS.iter().map(|p| p.id).collect::<Vec<_>>().join(", "))
                    })?
                };
            } else if accent.is_some() {
                // A new accent means the generated theme.
                t.preset.clear();
            }
            let palette = theme::apply_theme(&t)?;
            theme::write_theme_keys(&mut doc, &t, &palette);
            chickenwm_core::config::save_config_document(&doc)?;
            let _ = send_ipc_command(&IpcCommand::ReloadConfig).await;
            if cli.json {
                println!("{}", serde_json::json!({
                    "accent": theme::css(t.accent), "mode": t.mode.as_str(), "preset": t.preset, "icon_theme": t.icon_theme,
                    "base": theme::css(palette.base), "surface": theme::css(palette.surface),
                    "fg": theme::css(palette.fg), "accent_fg": theme::css(palette.accent_fg),
                }));
            } else {
                let scheme = if t.preset.is_empty() { theme::css(t.accent) } else { t.preset.clone() };
                println!("ok: theme {scheme} ({}), icons {}", t.mode.as_str(), t.icon_theme);
                println!("    base {}  surface {}  text {}  accent {}  on-accent {}",
                    theme::css(palette.base), theme::css(palette.surface), theme::css(palette.fg),
                    theme::css(palette.accent), theme::css(palette.accent_fg));
            }
            return Ok(());
        }
        Commands::Notify { title, body, app, timeout, critical } => {
            let note = chickenwm_core::ipc::Notification {
                app,
                title,
                body,
                timeout_secs: timeout,
                urgency: critical.then(|| "critical".to_string()),
            };
            let resp = chickenwm_core::ipc::send_ipc_command_to(chickenwm_core::ipc::PIPE_NAME_NOTIFY, &IpcCommand::Notify(note))
                .await
                .context("chickenwm-notify isn't running")?;
            println!("{resp:?}");
            return Ok(());
        }
        Commands::Notifications => {
            chickenwm_core::ipc::send_ipc_command_to(chickenwm_core::ipc::PIPE_NAME_NOTIFY, &IpcCommand::ToggleNotificationCenter)
                .await
                .context("chickenwm-notify isn't running")?;
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
        Commands::Tray => IpcCommand::QueryTray,
        Commands::Wallpaper {
            path,
            monitor,
            transition,
        } => IpcCommand::SetWallpaper {
            monitor,
            path,
            transition: Some(transition),
        },
        Commands::Monitors | Commands::RestoreExplorer | Commands::RestoreWindows | Commands::Theme { .. } | Commands::RenderWallpaper { .. } | Commands::Notify { .. } | Commands::Notifications | Commands::Doctor { .. } | Commands::CheckConfig | Commands::WindowRules => unreachable!(),
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
        IpcResponse::NotificationStatus { unread, dnd } => {
            println!("{unread} unread notification(s){}", if dnd { ", do not disturb is on" } else { "" });
        }
        IpcResponse::Tray(icons) => {
            if icons.is_empty() {
                println!("No tray icons (rooster hosts the tray only while Explorer's taskbar isn't running).");
            }
            for i in icons {
                println!("{:>6}  {}{}", i.pid, if i.tip.is_empty() { "(no tooltip)" } else { &i.tip }, if i.hidden { "  [hidden]" } else { "" });
            }
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
