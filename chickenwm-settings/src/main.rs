//! Phase 9: Tabbed GUI Settings Menu (`chickenwm-settings`)
//! - 9.1: Live 2-Way AST Serializer (`ConfigDocument`) + IPC Sync & `settings.css` Styling
//! - 9.2: Tab 1 — General & Shell (`explorer.exe` Suppressor, `Winlogon\Shell`, Emergency Restore, `exec-once` Manager)
//! - 9.3: Tab 2 — Display, Framerate & Performance (Per-Monitor Hz/DPI, Monitor Sync vs Custom `fps_limit` 30–500 FPS, `vfr`, & Anti-Cheat Safe Switch for `chickenwm-dwm-hook`)
//! - 9.4: Tab 3 — Tiling & Layouts (`Dwindle` vs `Master`, `gaps_in`, `gaps_out`, `border_size`, `split_ratio`, `smart_gaps`, `preserve_split`)
//! - 9.5: Tab 4 — Appearance, Borders & Live CSS Editor (`col.active_border`, `rounding`, `active_opacity`, Blur/Acrylic/Mica, `launcher.css` / `files.css` / `settings.css`)
//! - 9.6: Tab 5 — Animations & Interactive Visual Cubic-Bezier Curve Editor (Draggable control points + Live Animated Preview)
//! - 9.7: Tab 6 — Keybinds (`whkd` Manager, Searchable Table & Interactive "Record Keybind" Modal)
//! - 9.8: Tab 7 — Window Rules (`windowrulev2` Builder & Crosshair Window Inspector via `GetCursorPos` + `WindowFromPoint`)
//! - 9.9: Tab 8 — Wallpaper, Launcher & File Explorer (`chickenwm-paper` Picker, Transition Selector, WGSL Shader Loader)

mod theme_ui;

use anyhow::Result;
use eframe::egui;
use chickenwm_core::bootstrap_config_and_themes;
use chickenwm_core::config::{
    config_dir, save_config_document, BezierCurve, BindKind, BindRule, ConfigDocument, HyprConfig,
    RgbaColor, WindowRuleV2,
};
use chickenwm_core::css::{load_css_sheet, spawn_config_watcher, ConfigWatchEvent, CssStyleSheet};
use chickenwm_core::failsafe::{
    is_files_handler_registered, register_files_shell_handler, spawn_emergency_failsafe_thread,
    trigger_emergency_restore,
};
use chickenwm_core::init_tracing;
use chickenwm_core::ipc::{
    send_ipc_command_blocking, IpcCommand, MonitorInfo, PIPE_NAME_DAEMON, PIPE_NAME_PAPER,
    PIPE_NAME_WM,
};
use chickenwm_core::monitor::{enumerate_monitors, max_native_refresh_rate};
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetCursorPos, GetWindowTextW, WindowFromPoint, GA_ROOT,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsTab {
    GeneralShell,
    DisplayFramerate,
    TilingLayouts,
    AppearanceCss,
    AnimationsBezier,
    Keybinds,
    WindowRules,
    WallpaperModules,
}

impl SettingsTab {
    fn all() -> &'static [(SettingsTab, &'static str)] {
        &[
            (SettingsTab::GeneralShell, "1. General & Shell"),
            (SettingsTab::DisplayFramerate, "2. Display & Framerate"),
            (SettingsTab::TilingLayouts, "3. Tiling & Layouts"),
            (SettingsTab::AppearanceCss, "4. Appearance & CSS"),
            (SettingsTab::AnimationsBezier, "5. Animations & Bezier"),
            (SettingsTab::Keybinds, "6. Keybinds (whkd)"),
            (SettingsTab::WindowRules, "7. Window Rules"),
            (SettingsTab::WallpaperModules, "8. Wallpaper & Suite"),
        ]
    }
}

struct SettingsApp {
    active_tab: SettingsTab,
    doc: ConfigDocument,
    cfg: HyprConfig,
    monitors: Vec<MonitorInfo>,
    css: Arc<RwLock<CssStyleSheet>>,
    status_banner: String,

    // Tab 1: exec-once input
    new_exec_once: String,

    // Tab 4: Live CSS Editor state
    selected_css_file: String,
    css_editor_buffer: String,

    // Tab 5: Cubic-Bezier visual editor & animated preview
    selected_bezier_idx: usize,
    bezier_preview_t: f32,
    last_tick: Instant,

    // Tab 6: Keybind search & Record Keybind Modal
    bind_search: String,
    recording_keybind: bool,
    recorded_mods: String,
    recorded_key: String,
    new_bind_dispatcher: String,
    new_bind_arg: String,

    // Tab 7: Window Rules & Crosshair Inspector
    inspecting_crosshair: bool,
    inspected_class: String,
    inspected_title: String,
    new_rule_effect: String,

    // Tab 8: Wallpaper & Suite
    wallpaper_path_input: String,
    wallpaper_transition: String,

    // Default file manager registry state (cached; re-read after every change)
    files_handler_active: bool,

    // Tab 4: system theme editor
    theme_editor: theme_ui::ThemeEditor,
    /// settings.css the current egui visuals were built from.
    visuals_from: Option<CssStyleSheet>,
}

impl SettingsApp {
    fn new(doc: ConfigDocument, cfg: HyprConfig, css: Arc<RwLock<CssStyleSheet>>) -> Self {
        let monitors = enumerate_monitors();
        let initial_css_text =
            std::fs::read_to_string(config_dir().join("launcher.css")).unwrap_or_default();
        let cfg_theme = cfg.theme.clone();
        let wp_default = cfg
            .wallpapers
            .first()
            .map(|w| w.path.clone())
            .unwrap_or_default();

        Self {
            active_tab: SettingsTab::GeneralShell,
            doc,
            cfg,
            monitors,
            css,
            status_banner: "Connected to ~/.config/chickenwm/chickenwm.conf (2-way lossless AST sync)"
                .to_string(),
            new_exec_once: String::new(),
            selected_css_file: "launcher.css".to_string(),
            css_editor_buffer: initial_css_text,
            selected_bezier_idx: 0,
            bezier_preview_t: 0.0,
            last_tick: Instant::now(),
            bind_search: String::new(),
            recording_keybind: false,
            recorded_mods: "SUPER".to_string(),
            recorded_key: "Return".to_string(),
            new_bind_dispatcher: "exec".to_string(),
            new_bind_arg: "wt.exe".to_string(),
            inspecting_crosshair: false,
            inspected_class: "Taskmgr".to_string(),
            inspected_title: String::new(),
            new_rule_effect: "float".to_string(),
            wallpaper_path_input: wp_default,
            wallpaper_transition: "fade".to_string(),
            files_handler_active: is_files_handler_registered(),
            theme_editor: theme_ui::ThemeEditor::new(&cfg_theme),
            visuals_from: None,
        }
    }

    /// Saves the updated `ConfigDocument` AST losslessly to `~/.config/chickenwm/chickenwm.conf`
    /// and notifies running daemons via IPC.
    fn sync_config_and_notify(&mut self) {
        if let Err(e) = save_config_document(&self.doc) {
            self.status_banner = format!("Error saving chickenwm.conf: {e}");
            return;
        }
        let _ = send_ipc_command_blocking(PIPE_NAME_DAEMON, &IpcCommand::ReloadConfig);
        let _ = send_ipc_command_blocking(PIPE_NAME_WM, &IpcCommand::ReloadConfig);
        self.status_banner =
            "Saved changes losslessly to chickenwm.conf and applied live via IPC.".to_string();
    }

    /// Writes (or removes) the default-file-manager registry keys directly, so the toggle works
    /// even when the daemon isn't running, then persists the choice and tells the daemon.
    fn apply_files_handler(&mut self, enable: bool) {
        if let Err(e) = register_files_shell_handler(enable, None) {
            self.status_banner = format!("Could not change default file manager: {e}");
            self.files_handler_active = is_files_handler_registered();
            return;
        }
        self.files_handler_active = is_files_handler_registered();
        self.cfg.misc.register_files_handler = enable;
        self.doc.set_category_key(
            "misc",
            "register_files_handler",
            if enable { "true" } else { "false" },
        );
        self.sync_config_and_notify();
        let _ = send_ipc_command_blocking(
            PIPE_NAME_DAEMON,
            &IpcCommand::SetShellConfig {
                replace_winlogon_shell: self.cfg.misc.replace_winlogon_shell,
                suppress_explorer: self.cfg.misc.suppress_explorer,
                register_files_handler: enable,
            },
        );
        self.status_banner = if enable {
            "chickenwm-files is now the default file manager.".to_string()
        } else {
            "explorer.exe restored as the default file manager.".to_string()
        };
    }

    fn render_tab_general(&mut self, ui: &mut egui::Ui) {
        ui.heading("Tab 1 — General & Windows Shell Control");
        ui.add_space(6.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(
                egui::RichText::new("Windows Shell & Explorer.exe Suppressor")
                    .strong()
                    .size(15.0),
            );
            ui.add_space(4.0);

            let mut changed_shell = false;
            if ui
                .checkbox(
                    &mut self.cfg.misc.replace_winlogon_shell,
                    "Set ChickenWM as Default Windows Shell (HKCU\\...\\Winlogon\\Shell)",
                )
                .changed()
            {
                self.doc.set_category_key(
                    "misc",
                    "replace_winlogon_shell",
                    if self.cfg.misc.replace_winlogon_shell {
                        "true"
                    } else {
                        "false"
                    },
                );
                changed_shell = true;
            }

            if ui
                .checkbox(
                    &mut self.cfg.misc.suppress_explorer,
                    "Suppress explorer.exe (Immediately terminate unauthorized explorer.exe instances)",
                )
                .changed()
            {
                self.doc.set_category_key(
                    "misc",
                    "suppress_explorer",
                    if self.cfg.misc.suppress_explorer {
                        "true"
                    } else {
                        "false"
                    },
                );
                changed_shell = true;
            }

            if changed_shell {
                self.sync_config_and_notify();
                let _ = send_ipc_command_blocking(
                    PIPE_NAME_DAEMON,
                    &IpcCommand::SetShellConfig {
                        replace_winlogon_shell: self.cfg.misc.replace_winlogon_shell,
                        suppress_explorer: self.cfg.misc.suppress_explorer,
                        register_files_handler: self.cfg.misc.register_files_handler,
                    },
                );
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let btn = egui::Button::new(
                    egui::RichText::new("🚨 Emergency Restore explorer.exe")
                        .strong()
                        .color(egui::Color32::WHITE),
                )
                .fill(egui::Color32::from_rgb(210, 55, 75));
                if ui.add(btn).clicked() {
                    let _ =
                        send_ipc_command_blocking(PIPE_NAME_DAEMON, &IpcCommand::RestoreExplorer);
                    trigger_emergency_restore(false);
                    self.status_banner =
                        "Emergency Restore triggered: explorer.exe restored and launched."
                            .to_string();
                }
                ui.label("Hardware Failsafe Hotkey: Ctrl + Alt + Shift + Escape");
            });
        });

        ui.add_space(12.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(
                egui::RichText::new("Default File Manager")
                    .strong()
                    .size(15.0),
            );
            ui.label(
                "Opens folders, drives, Win + E and the Start / taskbar \"File Explorer\" entry in \
                 chickenwm-files instead of explorer.exe. Per-user (HKCU), no admin needed; \
                 Emergency Restore reverts it.",
            );
            ui.add_space(4.0);

            let mut want = self.cfg.misc.register_files_handler;
            if ui
                .checkbox(
                    &mut want,
                    "Make chickenwm-files the system-wide default file manager",
                )
                .changed()
            {
                self.apply_files_handler(want);
            }

            ui.horizontal(|ui| {
                if self.files_handler_active {
                    ui.label(
                        egui::RichText::new("● Active")
                            .color(egui::Color32::from_rgb(90, 200, 120)),
                    );
                } else {
                    ui.label(
                        egui::RichText::new("○ Not active — explorer.exe opens folders")
                            .color(egui::Color32::GRAY),
                    );
                    if self.cfg.misc.register_files_handler {
                        ui.label(
                            egui::RichText::new("Enabled in config but registry keys are missing")
                                .color(egui::Color32::from_rgb(230, 170, 60)),
                        );
                        if ui.button("Repair").clicked() {
                            self.apply_files_handler(true);
                        }
                    }
                }
                if ui.button("Open chickenwm-files").clicked() {
                    if let Ok(mut exe) = std::env::current_exe() {
                        exe.set_file_name("chickenwm-files.exe");
                        if let Err(e) = std::process::Command::new(&exe).spawn() {
                            self.status_banner = format!("Failed to launch {}: {e}", exe.display());
                        }
                    }
                }
            });
        });

        ui.add_space(12.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(
                egui::RichText::new("Startup Applications (exec-once)")
                    .strong()
                    .size(15.0),
            );
            let mut remove_idx = None;
            for (idx, cmd) in self.cfg.exec_once.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.monospace(format!("exec-once = {}", cmd));
                    if ui.small_button("✖ Remove").clicked() {
                        remove_idx = Some(idx);
                    }
                });
            }
            if let Some(ri) = remove_idx {
                self.cfg.exec_once.remove(ri);
                let vals = self.cfg.exec_once.clone();
                self.doc.replace_directives("exec-once", &vals);
                self.sync_config_and_notify();
            }

            ui.horizontal(|ui| {
                ui.text_edit_singleline(&mut self.new_exec_once);
                if ui.button("+ Add Startup Program").clicked()
                    && !self.new_exec_once.trim().is_empty()
                {
                    let cmd = self.new_exec_once.trim().to_string();
                    self.cfg.exec_once.push(cmd.clone());
                    self.doc.add_directive("exec-once", &cmd);
                    self.new_exec_once.clear();
                    self.sync_config_and_notify();
                }
            });
        });
    }

    fn render_tab_display_framerate(&mut self, ui: &mut egui::Ui) {
        ui.heading("Tab 2 — Display, Framerate & Compositor Engine");
        ui.add_space(6.0);

        // Connected Monitors Table
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Connected Displays").strong().size(15.0));
                if ui.button("⟳ Refresh Monitors").clicked() {
                    self.monitors = enumerate_monitors();
                }
            });
            for m in &self.monitors {
                ui.monospace(format!(
                    "• {} ({}) — {}x{} @ {} Hz | Position: {},{} | Scale: {:.0}% {}",
                    m.name,
                    m.adapter_name,
                    m.width,
                    m.height,
                    m.refresh_rate_hz,
                    m.x,
                    m.y,
                    m.dpi_scale * 100.0,
                    if m.is_primary { "[PRIMARY]" } else { "" }
                ));
            }
        });

        ui.add_space(12.0);
        // Frame Pacing & Refresh Rate Mode
        egui::Frame::group(ui.style()).show(ui, |ui| {
            let native_hz = max_native_refresh_rate(&self.monitors);
            ui.label(
                egui::RichText::new(format!(
                    "Animation & Resize Framerate Pacing (Detected Monitor Native: {} Hz)",
                    native_hz
                ))
                .strong()
                .size(15.0),
            );

            let mut follow_native = self.cfg.misc.fps_limit == 0;
            let mut fps_changed = false;

            if ui
                .radio_value(
                    &mut follow_native,
                    true,
                    format!(
                        "Follow Monitor Native Refresh Rate Dynamically (Default — {} Hz + DwmFlush VBlank)",
                        native_hz
                    ),
                )
                .changed()
                && follow_native
            {
                self.cfg.misc.fps_limit = 0;
                fps_changed = true;
            }
            if ui
                .radio_value(&mut follow_native, false, "Custom Framerate Cap (30–500 FPS)")
                .changed()
                && !follow_native
            {
                if self.cfg.misc.fps_limit == 0 {
                    self.cfg.misc.fps_limit = native_hz;
                }
                fps_changed = true;
            }

            if !follow_native {
                ui.horizontal(|ui| {
                    ui.label("Custom Target FPS:");
                    if ui
                        .add(egui::Slider::new(&mut self.cfg.misc.fps_limit, 30..=500).suffix(" FPS"))
                        .changed()
                    {
                        fps_changed = true;
                    }
                    for preset in [60u32, 120, 144, 165, 240, 360, 500] {
                        if ui.small_button(format!("{preset}Hz")).clicked() {
                            self.cfg.misc.fps_limit = preset;
                            fps_changed = true;
                        }
                    }
                });
            }

            if ui
                .checkbox(
                    &mut self.cfg.misc.vfr,
                    "Variable Frame Rate (VFR — automatically sleep tick loop at 0% CPU/GPU when animations settle)",
                )
                .changed()
            {
                self.doc.set_category_key(
                    "misc",
                    "vfr",
                    if self.cfg.misc.vfr { "true" } else { "false" },
                );
                self.sync_config_and_notify();
            }

            if fps_changed {
                let val_str = self.cfg.misc.fps_limit.to_string();
                self.doc.set_category_key("misc", "fps_limit", &val_str);
                self.sync_config_and_notify();
                let ipc_fps = if self.cfg.misc.fps_limit == 0 {
                    None
                } else {
                    Some(self.cfg.misc.fps_limit)
                };
                let _ =
                    send_ipc_command_blocking(PIPE_NAME_DAEMON, &IpcCommand::SetFramerate(ipc_fps));
            }
        });

        ui.add_space(12.0);
        // Phase 1.1 & 9.3: Compositor Engine Selector (Anti-Cheat Safe Switch)
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(
                egui::RichText::new("Compositor Engine Selector & Anti-Cheat Safe Switch")
                    .strong()
                    .size(15.0),
            );
            let mut mode_changed = false;
            if ui
                .radio_value(
                    &mut self.cfg.misc.dwm_hook_enabled,
                    false,
                    "Standard Win32 / DirectComposition Overlay Mode (Anti-Cheat Safe — Recommended)",
                )
                .changed()
            {
                mode_changed = true;
            }
            if ui
                .radio_value(
                    &mut self.cfg.misc.dwm_hook_enabled,
                    true,
                    "Experimental chickenwm-dwm-hook Mode (uDWM.dll / dwmcore.dll PDB Symbol Injection)",
                )
                .changed()
            {
                mode_changed = true;
            }

            if mode_changed {
                self.doc.set_category_key(
                    "misc",
                    "dwm_hook_enabled",
                    if self.cfg.misc.dwm_hook_enabled {
                        "true"
                    } else {
                        "false"
                    },
                );
                self.sync_config_and_notify();
                let _ = send_ipc_command_blocking(
                    PIPE_NAME_DAEMON,
                    &IpcCommand::SetCompositorMode {
                        dwm_hook_enabled: self.cfg.misc.dwm_hook_enabled,
                    },
                );
            }
        });
    }

    fn render_tab_tiling_layouts(&mut self, ui: &mut egui::Ui) {
        ui.heading("Tab 3 — Tiling & Layouts");
        ui.add_space(6.0);

        let mut changed = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Active Tiling Layout:");
                if ui
                    .selectable_value(&mut self.cfg.general.layout, "dwindle".to_string(), "Dwindle (BSP Tree)")
                    .changed()
                {
                    self.doc.set_category_key("general", "layout", "dwindle");
                    changed = true;
                }
                if ui
                    .selectable_value(&mut self.cfg.general.layout, "master".to_string(), "Master-Stack")
                    .changed()
                {
                    self.doc.set_category_key("general", "layout", "master");
                    changed = true;
                }
            });

            if ui
                .add(egui::Slider::new(&mut self.cfg.general.gaps_in, 0..=40).text("gaps_in (Inner Gaps px)"))
                .changed()
            {
                self.doc
                    .set_category_key("general", "gaps_in", &self.cfg.general.gaps_in.to_string());
                changed = true;
            }
            if ui
                .add(egui::Slider::new(&mut self.cfg.general.gaps_out, 0..=60).text("gaps_out (Outer Gaps px)"))
                .changed()
            {
                self.doc.set_category_key(
                    "general",
                    "gaps_out",
                    &self.cfg.general.gaps_out.to_string(),
                );
                changed = true;
            }
            if ui
                .add(egui::Slider::new(&mut self.cfg.general.border_size, 0..=12).text("border_size (px)"))
                .changed()
            {
                self.doc.set_category_key(
                    "general",
                    "border_size",
                    &self.cfg.general.border_size.to_string(),
                );
                changed = true;
            }
            if ui
                .add(
                    egui::Slider::new(&mut self.cfg.dwindle.default_split_ratio, 0.4..=1.6)
                        .text("Dwindle split_ratio"),
                )
                .changed()
            {
                self.doc.set_category_key(
                    "dwindle",
                    "default_split_ratio",
                    &format!("{:.2}", self.cfg.dwindle.default_split_ratio),
                );
                changed = true;
            }
            if ui
                .add(egui::Slider::new(&mut self.cfg.master.mfact, 0.25..=0.75).text("Master mfact"))
                .changed()
            {
                self.doc.set_category_key(
                    "master",
                    "mfact",
                    &format!("{:.2}", self.cfg.master.mfact),
                );
                changed = true;
            }
            if ui
                .checkbox(
                    &mut self.cfg.dwindle.smart_gaps,
                    "smart_gaps (Remove gaps when only 1 tiled window is on workspace)",
                )
                .changed()
            {
                let v = if self.cfg.dwindle.smart_gaps {
                    "true"
                } else {
                    "false"
                };
                self.cfg.master.smart_gaps = self.cfg.dwindle.smart_gaps;
                self.doc.set_category_key("dwindle", "smart_gaps", v);
                self.doc.set_category_key("master", "smart_gaps", v);
                changed = true;
            }
            if ui
                .checkbox(&mut self.cfg.dwindle.preserve_split, "preserve_split")
                .changed()
            {
                self.doc.set_category_key(
                    "dwindle",
                    "preserve_split",
                    if self.cfg.dwindle.preserve_split {
                        "true"
                    } else {
                        "false"
                    },
                );
                changed = true;
            }
            if ui
                .checkbox(
                    &mut self.cfg.general.no_titlebars,
                    "Strip Native Windows Titlebars (WS_CAPTION)",
                )
                .changed()
            {
                self.doc.set_category_key(
                    "general",
                    "no_titlebars",
                    if self.cfg.general.no_titlebars {
                        "true"
                    } else {
                        "false"
                    },
                );
                changed = true;
            }
        });

        if changed {
            self.sync_config_and_notify();
        }
    }

    /// Applies the edited theme everywhere and records it in chickenwm.conf.
    fn apply_system_theme(&mut self) {
        let theme = self.theme_editor.draft.clone();
        match chickenwm_core::theme::apply_theme(&theme) {
            Ok(palette) => {
                self.doc.set_category_key("theme", "accent", &chickenwm_core::theme::hypr_rgb(theme.accent));
                self.doc.set_category_key("theme", "mode", theme.mode.as_str());
                self.doc.set_category_key("theme", "icon_theme", &theme.icon_theme);
                let border = chickenwm_core::theme::hypr_color(palette.active_border());
                self.doc.set_category_key("general", "col.active_border", &border);
                self.cfg.general.col_active_border = chickenwm_core::config::BorderColorSpec::parse(&border);
                self.cfg.theme = theme;
                self.sync_config_and_notify();
                self.theme_editor.invalidate_icons();
                self.status_banner = "Theme applied to the whole system.".to_string();
            }
            Err(e) => self.status_banner = format!("Couldn't apply theme: {e}"),
        }
    }

    fn render_tab_appearance_css(&mut self, ui: &mut egui::Ui) {
        ui.heading("Tab 4 — Appearance, Animated Gradient Borders & Live CSS Editor");
        ui.add_space(6.0);
        let applied = self.cfg.theme.clone();
        if self.theme_editor.ui(ui, &applied) {
            self.apply_system_theme();
        }
        ui.add_space(10.0);

        let mut changed = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Active Border Primary Color:");
                let mut c1 = [
                    self.cfg.general.col_active_border.primary.r,
                    self.cfg.general.col_active_border.primary.g,
                    self.cfg.general.col_active_border.primary.b,
                    self.cfg.general.col_active_border.primary.a,
                ];
                if ui.color_edit_button_srgba_unmultiplied(&mut c1).changed() {
                    self.cfg.general.col_active_border.primary = RgbaColor {
                        r: c1[0],
                        g: c1[1],
                        b: c1[2],
                        a: c1[3],
                    };
                    changed = true;
                }

                ui.label("Secondary Gradient Color:");
                let sec = self
                    .cfg
                    .general
                    .col_active_border
                    .secondary
                    .unwrap_or(self.cfg.general.col_active_border.primary);
                let mut c2 = [sec.r, sec.g, sec.b, sec.a];
                if ui.color_edit_button_srgba_unmultiplied(&mut c2).changed() {
                    self.cfg.general.col_active_border.secondary = Some(RgbaColor {
                        r: c2[0],
                        g: c2[1],
                        b: c2[2],
                        a: c2[3],
                    });
                    changed = true;
                }

                if ui
                    .add(
                        egui::Slider::new(
                            &mut self.cfg.general.col_active_border.angle_deg,
                            0.0..=360.0,
                        )
                        .suffix("°"),
                    )
                    .changed()
                {
                    changed = true;
                }
            });

            if changed {
                let spec_str = self.cfg.general.col_active_border.to_hypr_string();
                self.doc
                    .set_category_key("general", "col.active_border", &spec_str);
            }

            if ui
                .add(
                    egui::Slider::new(&mut self.cfg.decoration.rounding, 0..=28)
                        .text("Window Corner Rounding (px)"),
                )
                .changed()
            {
                self.doc.set_category_key(
                    "decoration",
                    "rounding",
                    &self.cfg.decoration.rounding.to_string(),
                );
                changed = true;
            }
            if ui
                .add(
                    egui::Slider::new(&mut self.cfg.decoration.active_opacity, 0.3..=1.0)
                        .text("active_opacity"),
                )
                .changed()
            {
                self.doc.set_category_key(
                    "decoration",
                    "active_opacity",
                    &format!("{:.2}", self.cfg.decoration.active_opacity),
                );
                changed = true;
            }
            if ui
                .add(
                    egui::Slider::new(&mut self.cfg.decoration.inactive_opacity, 0.3..=1.0)
                        .text("inactive_opacity"),
                )
                .changed()
            {
                self.doc.set_category_key(
                    "decoration",
                    "inactive_opacity",
                    &format!("{:.2}", self.cfg.decoration.inactive_opacity),
                );
                changed = true;
            }
            if ui
                .add(
                    egui::Slider::new(&mut self.cfg.decoration.border_anim_speed, 0.0..=5.0)
                        .text("Border Gradient Rotation Speed"),
                )
                .changed()
            {
                self.doc.set_category_key(
                    "decoration",
                    "border_anim_speed",
                    &format!("{:.2}", self.cfg.decoration.border_anim_speed),
                );
                changed = true;
            }
            if ui
                .checkbox(
                    &mut self.cfg.decoration.blur_enabled,
                    "Enable DWM Backdrop Blur / Acrylic / Mica",
                )
                .changed()
            {
                self.doc.set_category_key(
                    "decoration",
                    "blur_enabled",
                    if self.cfg.decoration.blur_enabled {
                        "true"
                    } else {
                        "false"
                    },
                );
                changed = true;
            }
        });

        if changed {
            self.sync_config_and_notify();
        }

        ui.add_space(10.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new("Live CSS Theme Editor:").strong());
                for fname in ["roobar.css", "launcher.css", "files.css", "settings.css"] {
                    if ui
                        .selectable_label(self.selected_css_file == fname, fname)
                        .clicked()
                    {
                        self.selected_css_file = fname.to_string();
                        self.css_editor_buffer =
                            std::fs::read_to_string(config_dir().join(fname)).unwrap_or_default();
                    }
                }
                if ui.button("💾 Save & Hot-Reload CSS").clicked() {
                    let path = config_dir().join(&self.selected_css_file);
                    if std::fs::write(&path, &self.css_editor_buffer).is_ok() {
                        let _ =
                            send_ipc_command_blocking(PIPE_NAME_DAEMON, &IpcCommand::ReloadConfig);
                        self.status_banner =
                            format!("Saved and hot-reloaded {}", self.selected_css_file);
                    }
                }
            });
            egui::ScrollArea::vertical()
                .max_height(220.0)
                .show(ui, |ui| {
                    ui.add_sized(
                        [ui.available_width(), 200.0],
                        egui::TextEdit::multiline(&mut self.css_editor_buffer)
                            .code_editor()
                            .desired_rows(12),
                    );
                });
        });
    }

    fn render_tab_animations_bezier(&mut self, ui: &mut egui::Ui, dt: f32) {
        ui.heading("Tab 5 — Cubic-Bezier Curve Editor & Animation Rules");
        ui.add_space(6.0);

        if self.cfg.beziers.is_empty() {
            self.cfg.beziers.push(BezierCurve {
                name: "myBezier".to_string(),
                x0: 0.05,
                y0: 0.9,
                x1: 0.1,
                y1: 1.05,
            });
        }
        if self.selected_bezier_idx >= self.cfg.beziers.len() {
            self.selected_bezier_idx = 0;
        }

        let mut changed = false;
        ui.columns(2, |cols| {
            // Left column: Control Point Sliders + Draggable 2D Curve Canvas
            cols[0].label(egui::RichText::new("Cubic-Bezier Curve Controls").strong());
            cols[0].horizontal(|ui| {
                for (i, b) in self.cfg.beziers.iter().enumerate() {
                    if ui
                        .selectable_label(self.selected_bezier_idx == i, &b.name)
                        .clicked()
                    {
                        self.selected_bezier_idx = i;
                    }
                }
            });

            let curve = &mut self.cfg.beziers[self.selected_bezier_idx];
            changed |= cols[0]
                .add(egui::Slider::new(&mut curve.x0, 0.0..=1.0).text("P1.x (x0)"))
                .changed();
            changed |= cols[0]
                .add(egui::Slider::new(&mut curve.y0, -0.5..=1.5).text("P1.y (y0)"))
                .changed();
            changed |= cols[0]
                .add(egui::Slider::new(&mut curve.x1, 0.0..=1.0).text("P2.x (x1)"))
                .changed();
            changed |= cols[0]
                .add(egui::Slider::new(&mut curve.y1, -0.5..=1.5).text("P2.y (y1)"))
                .changed();

            // Interactive 2D Bezier Curve Canvas
            let (rect, _resp) =
                cols[0].allocate_exact_size(egui::vec2(220.0, 160.0), egui::Sense::hover());
            let painter = cols[0].painter_at(rect);
            painter.rect_filled(rect, 6.0, egui::Color32::from_rgb(17, 17, 27));

            let to_screen = |u: f32, v: f32| -> egui::Pos2 {
                egui::pos2(
                    rect.left() + 16.0 + u * (rect.width() - 32.0),
                    rect.bottom() - 24.0 - v.clamp(-0.3, 1.3) * (rect.height() - 48.0),
                )
            };

            let mut pts = Vec::with_capacity(41);
            for step in 0..=40 {
                let t = (step as f32) / 40.0;
                let y = curve.sample(t);
                pts.push(to_screen(t, y));
            }
            painter.add(egui::Shape::line(
                pts,
                egui::Stroke::new(2.5_f32, egui::Color32::from_rgb(51, 204, 255)),
            ));

            // Right column: Live Animated Window Preview + Animation Table
            cols[1].label(egui::RichText::new("Live Animated Preview").strong());
            self.bezier_preview_t = (self.bezier_preview_t + dt * 0.9) % 1.0;
            let sample_y = curve.sample(self.bezier_preview_t);
            let (prev_rect, _) =
                cols[1].allocate_exact_size(egui::vec2(260.0, 70.0), egui::Sense::hover());
            let p_paint = cols[1].painter_at(prev_rect);
            p_paint.rect_filled(prev_rect, 6.0, egui::Color32::from_rgb(17, 17, 27));
            let box_x = prev_rect.left()
                + 14.0
                + sample_y.clamp(0.0, 1.15) * (prev_rect.width() - 75.0);
            let anim_box = egui::Rect::from_min_size(
                egui::pos2(box_x, prev_rect.top() + 15.0),
                egui::vec2(42.0, 40.0),
            );
            p_paint.rect_filled(anim_box, 6.0, egui::Color32::from_rgb(0, 255, 153));

            cols[1].add_space(8.0);
            cols[1].label(egui::RichText::new("Per-Event Animation Rules").strong());
            for rule in &self.cfg.animation_rules {
                cols[1].monospace(format!(
                    "• {:<12} | speed={:.0} | curve={} | {}",
                    rule.name,
                    rule.speed,
                    rule.curve,
                    rule.style.as_deref().unwrap_or("default")
                ));
            }
        });

        if changed {
            let bezier_lines: Vec<String> = self
                .cfg
                .beziers
                .iter()
                .map(|b| format!("{}, {:.2}, {:.2}, {:.2}, {:.2}", b.name, b.x0, b.y0, b.x1, b.y1))
                .collect();
            self.doc.replace_directives("bezier", &bezier_lines);
            self.sync_config_and_notify();
        }
    }

    fn render_tab_keybinds(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.heading("Tab 6 — Keybinds (whkd Manager)");
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label("Search Keybinds:");
            ui.text_edit_singleline(&mut self.bind_search);
            if ui
                .button(if self.recording_keybind {
                    "🔴 Listening for Key... (Click to Cancel)"
                } else {
                    "🎙 Record New Keybind"
                })
                .clicked()
            {
                self.recording_keybind = !self.recording_keybind;
            }
        });

        if self.recording_keybind {
            ctx.input(|i| {
                let mut mods = Vec::new();
                if i.modifiers.command || i.modifiers.mac_cmd {
                    mods.push("SUPER");
                }
                if i.modifiers.ctrl {
                    mods.push("CTRL");
                }
                if i.modifiers.alt {
                    mods.push("ALT");
                }
                if i.modifiers.shift {
                    mods.push("SHIFT");
                }
                for ev in &i.events {
                    if let egui::Event::Key {
                        key, pressed: true, ..
                    } = ev
                    {
                        if !mods.is_empty() {
                            self.recorded_mods = mods.join(" ");
                        }
                        self.recorded_key = format!("{:?}", key);
                        self.recording_keybind = false;
                    }
                }
            });
        }

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Mods:");
                ui.add_sized([100.0, 22.0], egui::TextEdit::singleline(&mut self.recorded_mods));
                ui.label("Key:");
                ui.add_sized([80.0, 22.0], egui::TextEdit::singleline(&mut self.recorded_key));
                ui.label("Dispatcher:");
                ui.add_sized(
                    [110.0, 22.0],
                    egui::TextEdit::singleline(&mut self.new_bind_dispatcher),
                );
                ui.label("Arg:");
                ui.add_sized([120.0, 22.0], egui::TextEdit::singleline(&mut self.new_bind_arg));
                if ui.button("+ Add Bind").clicked() {
                    let rule = BindRule {
                        kind: BindKind::Press,
                        mods: self.recorded_mods.clone(),
                        key: self.recorded_key.clone(),
                        dispatcher: self.new_bind_dispatcher.clone(),
                        arg: self.new_bind_arg.clone(),
                    };
                    self.doc.add_directive(
                        "bind",
                        &format!(
                            "{}, {}, {}, {}",
                            rule.mods, rule.key, rule.dispatcher, rule.arg
                        ),
                    );
                    self.cfg.binds.push(rule);
                    self.sync_config_and_notify();
                }
            });
        });

        ui.add_space(6.0);
        let q = self.bind_search.trim().to_ascii_lowercase();
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                for b in &self.cfg.binds {
                    if !q.is_empty()
                        && !b.key.to_ascii_lowercase().contains(&q)
                        && !b.dispatcher.to_ascii_lowercase().contains(&q)
                        && !b.arg.to_ascii_lowercase().contains(&q)
                    {
                        continue;
                    }
                    ui.monospace(format!(
                        "{:<14} + {:<10} -> {:<18} {}",
                        b.mods, b.key, b.dispatcher, b.arg
                    ));
                }
            });
    }

    fn render_tab_window_rules(&mut self, ui: &mut egui::Ui) {
        ui.heading("Tab 7 — Window Rules (windowrulev2) & Crosshair Inspector");
        ui.add_space(6.0);

        // Phase 9.8: Crosshair Window Inspector
        if self.inspecting_crosshair {
            unsafe {
                if (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 {
                    let mut pt = POINT::default();
                    if GetCursorPos(&mut pt).is_ok() {
                        let raw_hwnd = WindowFromPoint(pt);
                        let root_hwnd = GetAncestor(raw_hwnd, GA_ROOT);
                        let target = if !root_hwnd.0.is_null() {
                            root_hwnd
                        } else {
                            raw_hwnd
                        };
                        if !target.0.is_null() {
                            let mut cbuf = [0u16; 128];
                            let clen = GetClassNameW(target, &mut cbuf);
                            if clen > 0 {
                                self.inspected_class =
                                    String::from_utf16_lossy(&cbuf[..clen as usize]);
                            }
                            let mut tbuf = [0u16; 256];
                            let tlen = GetWindowTextW(target, &mut tbuf);
                            if tlen > 0 {
                                self.inspected_title =
                                    String::from_utf16_lossy(&tbuf[..tlen as usize]);
                            }
                        }
                    }
                    self.inspecting_crosshair = false;
                }
            }
        }

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button(if self.inspecting_crosshair {
                        "🎯 Click Any Open Window Now..."
                    } else {
                        "🎯 Crosshair Window Inspector"
                    })
                    .clicked()
                {
                    self.inspecting_crosshair = !self.inspecting_crosshair;
                }
                ui.label(format!(
                    "Detected Class: '{}' | Title: '{}'",
                    self.inspected_class, self.inspected_title
                ));
            });

            ui.horizontal(|ui| {
                ui.label("Effect (e.g. float, workspace 3, opacity 0.9):");
                ui.text_edit_singleline(&mut self.new_rule_effect);
                ui.label("Target Class:");
                ui.text_edit_singleline(&mut self.inspected_class);
                if ui.button("+ Add windowrulev2").clicked()
                    && !self.inspected_class.trim().is_empty()
                {
                    let matcher = format!("class:^({})$", self.inspected_class.trim());
                    let raw = format!("{}, {}", self.new_rule_effect.trim(), matcher);
                    self.doc.add_directive("windowrulev2", &raw);
                    self.cfg.window_rules.push(WindowRuleV2 {
                        effect: self.new_rule_effect.trim().to_string(),
                        matcher,
                    });
                    self.sync_config_and_notify();
                }
            });
        });

        ui.add_space(8.0);
        for r in &self.cfg.window_rules {
            ui.monospace(format!("windowrulev2 = {}, {}", r.effect, r.matcher));
        }
    }

    fn render_tab_wallpaper_modules(&mut self, ui: &mut egui::Ui) {
        ui.heading("Tab 8 — Wallpaper, WGSL Shaders, Launcher & File Explorer");
        ui.add_space(6.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(
                egui::RichText::new("chickenwm-paper Background & Live WGSL Shader Loader")
                    .strong(),
            );
            ui.horizontal(|ui| {
                ui.label("Image or .wgsl Path:");
                ui.text_edit_singleline(&mut self.wallpaper_path_input);
            });
            ui.horizontal(|ui| {
                ui.label("Transition Style:");
                for tr in ["fade", "wipe", "outer", "grow"] {
                    ui.selectable_value(&mut self.wallpaper_transition, tr.to_string(), tr);
                }
                if ui.button("Apply Wallpaper / Shader").clicked() {
                    let _ = send_ipc_command_blocking(
                        PIPE_NAME_PAPER,
                        &IpcCommand::SetWallpaper {
                            monitor: None,
                            path: self.wallpaper_path_input.clone(),
                            transition: Some(self.wallpaper_transition.clone()),
                        },
                    );
                    self.doc.replace_directives(
                        "wallpaper",
                        &[format!(
                            ", {}, {}",
                            self.wallpaper_path_input, self.wallpaper_transition
                        )],
                    );
                    self.sync_config_and_notify();
                }
            });
        });
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;

        if self.active_tab == SettingsTab::AnimationsBezier || self.inspecting_crosshair {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        let sheet = self.css.read().clone();
        if self.visuals_from.as_ref() != Some(&sheet) {
            theme_ui::apply_visuals(ctx, &sheet);
            self.visuals_from = Some(sheet.clone());
        }
        let bg = sheet.var_color(
            "--bg-color",
            RgbaColor {
                r: 24,
                g: 24,
                b: 37,
                a: 255,
            },
        );
        let accent = sheet.var_color(
            "--accent-color",
            RgbaColor {
                r: 51,
                g: 204,
                b: 255,
                a: 255,
            },
        );

        egui::TopBottomPanel::bottom("settings_status_footer").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(
                    egui::Color32::from_rgb(accent.r, accent.g, accent.b),
                    "● Live AST Sync:",
                );
                ui.label(&self.status_banner);
            });
        });

        egui::SidePanel::left("settings_sidebar")
            .resizable(false)
            .default_width(210.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.colored_label(
                    egui::Color32::from_rgb(accent.r, accent.g, accent.b),
                    egui::RichText::new("⚙ ChickenWM Settings").size(18.0).strong(),
                );
                ui.separator();
                for &(tab, label) in SettingsTab::all() {
                    if ui
                        .selectable_label(self.active_tab == tab, label)
                        .clicked()
                    {
                        self.active_tab = tab;
                    }
                }
            });

        let panel_frame = egui::Frame::NONE
            .fill(egui::Color32::from_rgb(bg.r, bg.g, bg.b))
            .inner_margin(egui::Margin::same(16));

        egui::CentralPanel::default()
            .frame(panel_frame)
            .show(ctx, |ui| match self.active_tab {
                SettingsTab::GeneralShell => self.render_tab_general(ui),
                SettingsTab::DisplayFramerate => self.render_tab_display_framerate(ui),
                SettingsTab::TilingLayouts => self.render_tab_tiling_layouts(ui),
                SettingsTab::AppearanceCss => self.render_tab_appearance_css(ui),
                SettingsTab::AnimationsBezier => self.render_tab_animations_bezier(ui, dt),
                SettingsTab::Keybinds => self.render_tab_keybinds(ui, ctx),
                SettingsTab::WindowRules => self.render_tab_window_rules(ui),
                SettingsTab::WallpaperModules => self.render_tab_wallpaper_modules(ui),
            });
    }
}

fn main() -> Result<()> {
    init_tracing("chickenwm_settings");
    let _failsafe = spawn_emergency_failsafe_thread();

    let (doc, cfg) = bootstrap_config_and_themes()?;
    let css = Arc::new(RwLock::new(
        load_css_sheet("settings.css").unwrap_or_default(),
    ));
    let css_watcher_ref = Arc::clone(&css);
    let _watcher = spawn_config_watcher(move |ev| {
        if let ConfigWatchEvent::CssChanged { file_name, .. } = ev {
            if file_name.eq_ignore_ascii_case("settings.css") {
                if let Ok(new_sheet) = load_css_sheet("settings.css") {
                    *css_watcher_ref.write() = new_sheet;
                }
            }
        }
    });

    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ChickenWM Control Center (chickenwm-settings)")
            .with_inner_size([1020.0, 680.0]),
        ..Default::default()
    };

    eframe::run_native(
        "ChickenWM Settings",
        native_options,
        Box::new(move |_cc| Ok(Box::new(SettingsApp::new(doc, cfg, css)))),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))?;

    Ok(())
}
