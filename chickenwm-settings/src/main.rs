//! `chickenwm-settings`: the ChickenWM settings app.
//!
//! Laid out like macOS System Settings: a searchable sidebar of sections with colored icon
//! tiles, and pages made of grouped cards whose rows hold a label on the left and a control on
//! the right. Every change is written straight into `chickenwm.conf` through the lossless
//! `ConfigDocument` AST and applied live over IPC. The window follows the system theme
//! (`settings.css`).
//!
//! Pages: General (shell, default file manager, startup apps, recovery), Appearance (theme
//! gallery, light/dark, accent, icons, per-app modes, window looks, stylesheets), Wallpaper,
//! Displays (monitors, frame rate, compositor), Tiling, Animations (bezier editor), Keyboard
//! (shortcuts) and Window Rules (with a click-to-pick window inspector).

// Release builds are windowed apps: no console window when launched from the shell, a
// hotkey or the launcher. Debug builds keep the console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod kit;
mod theme_ui;

use anyhow::Result;
use chickenwm_core::bootstrap_config_and_themes;
use chickenwm_core::config::{
    config_dir, save_config_document, BezierCurve, BindKind, BindRule, ConfigDocument, HyprConfig, PaperLayerKind,
    RgbaColor, WindowRuleV2,
};
use chickenwm_core::css::{load_css_sheet, spawn_config_watcher, ConfigWatchEvent, CssStyleSheet};
use chickenwm_core::failsafe::{
    is_files_handler_registered, register_files_shell_handler, spawn_emergency_failsafe_thread, trigger_emergency_restore,
};
use chickenwm_core::init_tracing;
use chickenwm_core::ipc::{send_ipc_command_blocking, IpcCommand, MonitorInfo, PIPE_NAME_DAEMON, PIPE_NAME_PAPER, PIPE_NAME_WM};
use chickenwm_core::monitor::{enumerate_monitors, max_native_refresh_rate};
use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, Vec2};
use kit::Kit;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{GetAncestor, GetClassNameW, GetCursorPos, GetWindowTextW, WindowFromPoint, GA_ROOT};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Page {
    General,
    Appearance,
    Wallpaper,
    Displays,
    Tiling,
    Animations,
    Keyboard,
    WindowRules,
}

struct PageInfo {
    page: Page,
    glyph: &'static str,
    tile: Color32,
    title: &'static str,
    subtitle: &'static str,
    /// Extra words the sidebar search matches.
    keywords: &'static str,
}

const PAGES: [PageInfo; 8] = [
    PageInfo {
        page: Page::General,
        glyph: "⚙",
        tile: Color32::from_rgb(142, 142, 147),
        title: "General",
        subtitle: "Shell, default file manager and startup apps.",
        keywords: "shell explorer winlogon startup exec-once file manager files recovery restore",
    },
    PageInfo {
        page: Page::Appearance,
        glyph: "🎨",
        tile: Color32::from_rgb(88, 86, 214),
        title: "Appearance",
        subtitle: "Theme, light and dark, accent color, icons and window looks.",
        keywords: "theme dark light mode accent color icons dracula catppuccin nord gruvbox tokyo rose pine solarized one everforest kanagawa border rounding opacity blur css",
    },
    PageInfo {
        page: Page::Wallpaper,
        glyph: "🖼",
        tile: Color32::from_rgb(52, 170, 220),
        title: "Wallpaper",
        subtitle: "The desktop background and its layers.",
        keywords: "background paper image shader wgsl layer chroma key transition",
    },
    PageInfo {
        page: Page::Displays,
        glyph: "🖥",
        tile: Color32::from_rgb(0, 122, 255),
        title: "Displays",
        subtitle: "Monitors, frame rate and the compositor.",
        keywords: "monitor refresh hz fps framerate vfr compositor dwm anti-cheat",
    },
    PageInfo {
        page: Page::Tiling,
        glyph: "◫",
        tile: Color32::from_rgb(52, 199, 89),
        title: "Tiling",
        subtitle: "How windows are arranged.",
        keywords: "layout dwindle master gaps border split smart gaps titlebars",
    },
    PageInfo {
        page: Page::Animations,
        glyph: "✨",
        tile: Color32::from_rgb(255, 149, 0),
        title: "Animations",
        subtitle: "Easing curves and animation speeds.",
        keywords: "bezier curve easing speed motion",
    },
    PageInfo {
        page: Page::Keyboard,
        glyph: "⌨",
        tile: Color32::from_rgb(99, 99, 102),
        title: "Keyboard",
        subtitle: "Keyboard shortcuts.",
        keywords: "keybinds binds shortcuts hotkeys keys",
    },
    PageInfo {
        page: Page::WindowRules,
        glyph: "▣",
        tile: Color32::from_rgb(255, 59, 48),
        title: "Window Rules",
        subtitle: "Per-app rules: float, workspace, opacity and more.",
        keywords: "windowrule windowrulev2 float class inspector crosshair",
    },
];

fn info(page: Page) -> &'static PageInfo {
    PAGES.iter().find(|p| p.page == page).expect("page info")
}

struct SettingsApp {
    page: Page,
    search: String,
    doc: ConfigDocument,
    cfg: HyprConfig,
    monitors: Vec<MonitorInfo>,
    css: Arc<RwLock<CssStyleSheet>>,
    status_banner: String,
    /// The banner text last shown and when it changed (it's shown briefly as a toast).
    status_seen: String,
    status_at: Option<Instant>,

    new_exec_once: String,

    show_css_editor: bool,
    selected_css_file: String,
    css_editor_buffer: String,

    selected_bezier_idx: usize,
    bezier_preview_t: f32,
    last_tick: Instant,

    bind_search: String,
    recording_keybind: bool,
    recorded_mods: String,
    recorded_key: String,
    new_bind_dispatcher: String,
    new_bind_arg: String,

    inspecting_crosshair: bool,
    inspected_class: String,
    inspected_title: String,
    new_rule_effect: String,

    wallpaper_path_input: String,
    wallpaper_transition: &'static str,

    /// Default file manager registry state (cached; re-read after every change).
    files_handler_active: bool,

    theme_editor: theme_ui::ThemeEditor,
    /// settings.css the current visuals were built from.
    visuals_from: Option<CssStyleSheet>,
}

const TRANSITIONS: [&str; 4] = ["fade", "wipe", "outer", "grow"];

fn bool_str(b: bool) -> &'static str {
    if b { "true" } else { "false" }
}

impl SettingsApp {
    fn new(doc: ConfigDocument, cfg: HyprConfig, css: Arc<RwLock<CssStyleSheet>>) -> Self {
        let wp = cfg.wallpapers.first();
        let wallpaper_transition = wp
            .and_then(|w| TRANSITIONS.iter().copied().find(|t| t.eq_ignore_ascii_case(&w.transition)))
            .unwrap_or("fade");
        // `chickenwm-settings --page appearance` opens a page directly.
        let mut args = std::env::args().skip_while(|a| a != "--page").skip(1);
        let page = args
            .next()
            .and_then(|want| PAGES.iter().find(|p| p.title.replace(' ', "").eq_ignore_ascii_case(&want.replace(['-', '_', ' '], ""))))
            .map_or(Page::General, |p| p.page);
        Self {
            page,
            search: String::new(),
            monitors: enumerate_monitors(),
            css,
            status_banner: String::new(),
            status_seen: String::new(),
            status_at: None,
            new_exec_once: String::new(),
            show_css_editor: false,
            selected_css_file: "launcher.css".to_string(),
            css_editor_buffer: std::fs::read_to_string(config_dir().join("launcher.css")).unwrap_or_default(),
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
            inspected_class: String::new(),
            inspected_title: String::new(),
            new_rule_effect: "float".to_string(),
            wallpaper_path_input: wp.map(|w| w.path.clone()).unwrap_or_default(),
            wallpaper_transition,
            files_handler_active: is_files_handler_registered(),
            theme_editor: theme_ui::ThemeEditor::new(&cfg.theme),
            visuals_from: None,
            doc,
            cfg,
        }
    }

    /// Saves the `ConfigDocument` losslessly to chickenwm.conf and tells the running daemons.
    fn sync_config_and_notify(&mut self) {
        if let Err(e) = save_config_document(&self.doc) {
            self.status_banner = format!("Couldn't save chickenwm.conf: {e}");
            return;
        }
        let _ = send_ipc_command_blocking(PIPE_NAME_DAEMON, &IpcCommand::ReloadConfig);
        let _ = send_ipc_command_blocking(PIPE_NAME_WM, &IpcCommand::ReloadConfig);
        self.status_banner = "Saved".to_string();
    }

    fn send_shell_config(&self) {
        let _ = send_ipc_command_blocking(
            PIPE_NAME_DAEMON,
            &IpcCommand::SetShellConfig {
                replace_winlogon_shell: self.cfg.misc.replace_winlogon_shell,
                suppress_explorer: self.cfg.misc.suppress_explorer,
                register_files_handler: self.cfg.misc.register_files_handler,
            },
        );
    }

    /// Writes (or removes) the default-file-manager registry keys directly, so the toggle works
    /// even when the daemon isn't running, then persists the choice and tells the daemon.
    fn apply_files_handler(&mut self, enable: bool) {
        if let Err(e) = register_files_shell_handler(enable, None) {
            self.status_banner = format!("Could not change the default file manager: {e}");
            self.files_handler_active = is_files_handler_registered();
            return;
        }
        self.files_handler_active = is_files_handler_registered();
        self.cfg.misc.register_files_handler = enable;
        self.doc.set_category_key("misc", "register_files_handler", bool_str(enable));
        self.sync_config_and_notify();
        self.send_shell_config();
        self.status_banner = if enable {
            "ChickenWM Files is now the default file manager".to_string()
        } else {
            "Explorer is the default file manager again".to_string()
        };
    }

    /// Applies the edited theme everywhere and records it in chickenwm.conf.
    fn apply_system_theme(&mut self) {
        let theme = self.theme_editor.draft.clone();
        match chickenwm_core::theme::apply_theme(&theme) {
            Ok(palette) => {
                chickenwm_core::theme::write_theme_keys(&mut self.doc, &theme, &palette);
                let border = chickenwm_core::theme::hypr_color(palette.active_border());
                self.cfg.general.col_active_border = chickenwm_core::config::BorderColorSpec::parse(&border);
                self.cfg.theme = theme;
                self.sync_config_and_notify();
                self.theme_editor.invalidate_icons();
                self.status_banner = "Theme applied".to_string();
            }
            Err(e) => self.status_banner = format!("Couldn't apply the theme: {e}"),
        }
    }

    // ───────────────────────────── General ─────────────────────────────

    fn page_general(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        let mut shell_changed = false;
        kit::group(ui, kit, "Shell", |g| {
            g.row("Use ChickenWM as the Windows shell", "Starts ChickenWM instead of Explorer when you sign in.", |ui| {
                if kit::toggle(ui, kit, &mut self.cfg.misc.replace_winlogon_shell).changed() {
                    self.doc.set_category_key("misc", "replace_winlogon_shell", bool_str(self.cfg.misc.replace_winlogon_shell));
                    shell_changed = true;
                }
            });
            g.row("Hide Explorer's taskbar and desktop", "Closes explorer.exe while ChickenWM runs; roobar takes its place.", |ui| {
                if kit::toggle(ui, kit, &mut self.cfg.misc.suppress_explorer).changed() {
                    self.doc.set_category_key("misc", "suppress_explorer", bool_str(self.cfg.misc.suppress_explorer));
                    shell_changed = true;
                }
            });
        });
        if shell_changed {
            self.sync_config_and_notify();
            self.send_shell_config();
        }

        let mut want = self.cfg.misc.register_files_handler;
        let mut repair = false;
        let mut open_files = false;
        kit::group(ui, kit, "Default file manager", |g| {
            g.row("Open folders in ChickenWM Files", "Folders, drives, Win + E and File Explorer shortcuts. Just for you, no admin needed.", |ui| {
                kit::toggle(ui, kit, &mut want);
            });
            g.row("Status", "", |ui| {
                if kit::button(ui, kit, "Open Files", false).clicked() {
                    open_files = true;
                }
                if !self.files_handler_active && self.cfg.misc.register_files_handler && kit::button(ui, kit, "Repair", true).clicked() {
                    repair = true;
                }
                if self.files_handler_active {
                    kit::chip(ui, kit, "● Active", Color32::from_rgb(52, 199, 89));
                } else if self.cfg.misc.register_files_handler {
                    kit::chip(ui, kit, "Registry keys missing", Color32::from_rgb(255, 149, 0));
                } else {
                    kit::chip(ui, kit, "Explorer opens folders", kit.sub);
                }
            });
        });
        if want != self.cfg.misc.register_files_handler {
            self.apply_files_handler(want);
        }
        if repair {
            self.apply_files_handler(true);
        }
        if open_files {
            if let Ok(mut exe) = std::env::current_exe() {
                exe.set_file_name("chickenwm-files.exe");
                if let Err(e) = std::process::Command::new(&exe).spawn() {
                    self.status_banner = format!("Couldn't start {}: {e}", exe.display());
                }
            }
        }

        let mut remove = None;
        let mut add = false;
        kit::group(ui, kit, "Open at startup", |g| {
            for (idx, cmd) in self.cfg.exec_once.iter().enumerate() {
                g.row(cmd, "", |ui| {
                    if ui.small_button("Remove").clicked() {
                        remove = Some(idx);
                    }
                });
            }
            g.row("Add a program", "A command line, e.g. chickenbrowser.exe", |ui| {
                if kit::button(ui, kit, "Add", true).clicked() {
                    add = true;
                }
                let resp = ui.add(egui::TextEdit::singleline(&mut self.new_exec_once).desired_width(200.0).hint_text("program.exe --args"));
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    add = true;
                }
            });
        });
        if let Some(i) = remove {
            self.cfg.exec_once.remove(i);
            let vals = self.cfg.exec_once.clone();
            self.doc.replace_directives("exec-once", &vals);
            self.sync_config_and_notify();
        }
        if add && !self.new_exec_once.trim().is_empty() {
            let cmd = self.new_exec_once.trim().to_string();
            self.cfg.exec_once.push(cmd.clone());
            self.doc.add_directive("exec-once", &cmd);
            self.new_exec_once.clear();
            self.sync_config_and_notify();
        }

        let mut restore = false;
        kit::group(ui, kit, "Recovery", |g| {
            g.row("Restore Explorer", "Brings back Explorer as the shell right now. Hotkey anytime: Ctrl + Alt + Shift + Esc.", |ui| {
                let btn = egui::Button::new(RichText::new("Restore").color(Color32::WHITE).size(13.0))
                    .fill(kit.danger)
                    .corner_radius(CornerRadius::same(7))
                    .min_size(Vec2::new(0.0, 26.0));
                if ui.add(btn).clicked() {
                    restore = true;
                }
            });
        });
        if restore {
            let _ = send_ipc_command_blocking(PIPE_NAME_DAEMON, &IpcCommand::RestoreExplorer);
            trigger_emergency_restore(false);
            self.status_banner = "Explorer restored".to_string();
        }
    }

    // ───────────────────────────── Appearance ─────────────────────────────

    fn page_appearance(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        let applied = self.cfg.theme.clone();
        if self.theme_editor.ui(ui, kit, &applied) {
            self.apply_system_theme();
        }
        if let Some((app, mode)) = theme_ui::per_app_ui(ui, kit, &self.cfg.theme) {
            match chickenwm_core::theme::set_app_mode(app, mode) {
                Ok(theme) => {
                    // Keep our AST in sync with what the provider just saved.
                    self.doc.set_category_key("theme", "app_modes", &theme.app_modes_string());
                    self.cfg.theme.app_modes = theme.app_modes;
                    self.theme_editor.draft.app_modes = self.cfg.theme.app_modes.clone();
                    self.status_banner = format!("{app}: {}", mode.as_str());
                }
                Err(e) => self.status_banner = format!("Couldn't change {app}: {e}"),
            }
        }

        let mut changed = false;
        let mut border_changed = false;
        kit::group(ui, kit, "Windows", |g| {
            g.row("Active border", "Two colors make a gradient at the given angle.", |ui| {
                let b = &mut self.cfg.general.col_active_border;
                if ui.add(egui::DragValue::new(&mut b.angle_deg).range(0.0..=360.0).suffix("°").speed(1.0)).changed() {
                    border_changed = true;
                }
                let sec = b.secondary.unwrap_or(b.primary);
                let mut c2 = [sec.r, sec.g, sec.b, sec.a];
                if ui.color_edit_button_srgba_unmultiplied(&mut c2).on_hover_text("Second color").changed() {
                    b.secondary = Some(RgbaColor { r: c2[0], g: c2[1], b: c2[2], a: c2[3] });
                    border_changed = true;
                }
                let mut c1 = [b.primary.r, b.primary.g, b.primary.b, b.primary.a];
                if ui.color_edit_button_srgba_unmultiplied(&mut c1).on_hover_text("First color").changed() {
                    b.primary = RgbaColor { r: c1[0], g: c1[1], b: c1[2], a: c1[3] };
                    border_changed = true;
                }
            });
            g.row("Border rotation speed", "0 keeps the gradient still.", |ui| {
                if ui.add(egui::Slider::new(&mut self.cfg.decoration.border_anim_speed, 0.0..=5.0).max_decimals(2)).changed() {
                    self.doc.set_category_key("decoration", "border_anim_speed", &format!("{:.2}", self.cfg.decoration.border_anim_speed));
                    changed = true;
                }
            });
            g.row("Corner rounding", "", |ui| {
                if ui.add(egui::Slider::new(&mut self.cfg.decoration.rounding, 0..=28).suffix(" px")).changed() {
                    self.doc.set_category_key("decoration", "rounding", &self.cfg.decoration.rounding.to_string());
                    changed = true;
                }
            });
            g.row("Focused window opacity", "", |ui| {
                if ui.add(egui::Slider::new(&mut self.cfg.decoration.active_opacity, 0.3..=1.0).max_decimals(2)).changed() {
                    self.doc.set_category_key("decoration", "active_opacity", &format!("{:.2}", self.cfg.decoration.active_opacity));
                    changed = true;
                }
            });
            g.row("Other windows' opacity", "", |ui| {
                if ui.add(egui::Slider::new(&mut self.cfg.decoration.inactive_opacity, 0.3..=1.0).max_decimals(2)).changed() {
                    self.doc.set_category_key("decoration", "inactive_opacity", &format!("{:.2}", self.cfg.decoration.inactive_opacity));
                    changed = true;
                }
            });
            g.row("Backdrop blur", "Acrylic / Mica behind supported windows.", |ui| {
                if kit::toggle(ui, kit, &mut self.cfg.decoration.blur_enabled).changed() {
                    self.doc.set_category_key("decoration", "blur_enabled", bool_str(self.cfg.decoration.blur_enabled));
                    changed = true;
                }
            });
        });
        if border_changed {
            let spec = self.cfg.general.col_active_border.to_hypr_string();
            self.doc.set_category_key("general", "col.active_border", &spec);
            changed = true;
        }
        if changed {
            self.sync_config_and_notify();
        }

        let mut save_css = false;
        let mut load_css: Option<&'static str> = None;
        kit::group(ui, kit, "Advanced", |g| {
            g.row("Edit stylesheets", "Fonts, sizes and colors of each app, in CSS. Changes show up live.", |ui| {
                kit::toggle(ui, kit, &mut self.show_css_editor);
            });
            if self.show_css_editor {
                g.custom(|ui| {
                    ui.horizontal(|ui| {
                        let files: [(&str, &str); 6] = [
                            ("roobar.css", "Bar"),
                            ("launcher.css", "Launcher"),
                            ("files.css", "Files"),
                            ("settings.css", "Settings"),
                            ("browser.css", "Browser"),
                            ("notifications.css", "Notifications"),
                        ];
                        let mut sel = files.iter().position(|f| f.0 == self.selected_css_file).unwrap_or(0);
                        let opts: Vec<(usize, &str)> = files.iter().enumerate().map(|(i, f)| (i, f.1)).collect();
                        if kit::segmented(ui, kit, &mut sel, &opts) {
                            load_css = Some(files[sel].0);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if kit::button(ui, kit, "Save", true).clicked() {
                                save_css = true;
                            }
                        });
                    });
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        ui.add_sized(
                            [ui.available_width(), 280.0],
                            egui::TextEdit::multiline(&mut self.css_editor_buffer).code_editor().desired_rows(14),
                        );
                    });
                });
            }
        });
        if let Some(f) = load_css {
            self.selected_css_file = f.to_string();
            self.css_editor_buffer = std::fs::read_to_string(config_dir().join(f)).unwrap_or_default();
        }
        if save_css {
            let path = config_dir().join(&self.selected_css_file);
            match std::fs::write(&path, &self.css_editor_buffer) {
                Ok(()) => {
                    let _ = send_ipc_command_blocking(PIPE_NAME_DAEMON, &IpcCommand::ReloadConfig);
                    self.status_banner = format!("Saved {}", self.selected_css_file);
                }
                Err(e) => self.status_banner = format!("Couldn't save {}: {e}", self.selected_css_file),
            }
        }
    }

    // ───────────────────────────── Wallpaper ─────────────────────────────

    fn page_wallpaper(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        let mut apply = false;
        kit::group(ui, kit, "Wallpaper", |g| {
            g.row("Image or shader", "A PNG / JPEG / WebP picture, or a .wgsl shader.", |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.wallpaper_path_input).desired_width(260.0).hint_text("~/Pictures/wall.png"));
            });
            g.row("Transition", "", |ui| {
                let opts: Vec<(&'static str, &str)> = TRANSITIONS.iter().map(|t| (*t, *t)).collect();
                kit::segmented(ui, kit, &mut self.wallpaper_transition, &opts);
            });
            g.row("", "", |ui| {
                if kit::button(ui, kit, "Set wallpaper", true).clicked() {
                    apply = true;
                }
            });
        });
        if apply {
            let _ = send_ipc_command_blocking(
                PIPE_NAME_PAPER,
                &IpcCommand::SetWallpaper {
                    monitor: None,
                    path: self.wallpaper_path_input.clone(),
                    transition: Some(self.wallpaper_transition.to_string()),
                },
            );
            self.doc.replace_directives("wallpaper", &[format!(", {}, {}", self.wallpaper_path_input, self.wallpaper_transition)]);
            self.sync_config_and_notify();
        }

        kit::group(ui, kit, "Layers", |g| {
            if self.cfg.paper.layers.is_empty() {
                g.custom(|ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(
                                "Stack images, colors and shaders, with chroma keying, in chickenwm.conf:\n\npaper {\n    layer = image, ~/bg/sky.png\n    layer = image, ~/bg/character.png, key = rgb(00ff00), tolerance = 0.25\n}\n\nLayers replace the single wallpaper above. See docs/STYLING.md.",
                            )
                            .size(12.5)
                            .color(kit.sub),
                        )
                        .wrap(),
                    );
                });
            } else {
                for (i, l) in self.cfg.paper.layers.iter().enumerate() {
                    let kind = match l.kind {
                        PaperLayerKind::Image => "Image",
                        PaperLayerKind::Generated => "Theme wallpaper",
                        PaperLayerKind::Color => "Color",
                        PaperLayerKind::Shader => "Shader",
                    };
                    let mut hint = format!("{}{} · {} · {:.0}%", if l.source.is_empty() { "" } else { &l.source }, if l.source.is_empty() { "" } else { " ·" }, l.blend, l.opacity * 100.0);
                    if let Some(k) = l.key {
                        hint.push_str(&format!(" · key {} ±{:.2}", chickenwm_core::theme::css(k), l.tolerance));
                    }
                    g.row(&format!("{}. {kind}", i + 1), &hint, |ui| {
                        if l.key.is_some() {
                            kit::chip(ui, kit, "Chroma key", kit.accent);
                        }
                    });
                }
            }
        });
    }

    // ───────────────────────────── Displays ─────────────────────────────

    fn page_displays(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        let mut refresh = false;
        kit::group(ui, kit, "Displays", |g| {
            for m in &self.monitors {
                let hint = if m.adapter_name.is_empty() { String::new() } else { m.adapter_name.clone() };
                g.row(&m.name, &hint, |ui| {
                    ui.label(
                        RichText::new(format!("{} × {} · {} Hz · {:.0}%", m.width, m.height, m.refresh_rate_hz, m.dpi_scale * 100.0))
                            .color(kit.sub),
                    );
                    if m.is_primary {
                        kit::chip(ui, kit, "Main", kit.accent);
                    }
                });
            }
            g.row("", "", |ui| {
                if kit::button(ui, kit, "Detect displays", false).clicked() {
                    refresh = true;
                }
            });
        });
        if refresh {
            self.monitors = enumerate_monitors();
        }

        let native_hz = max_native_refresh_rate(&self.monitors);
        let mut follow_native = self.cfg.misc.fps_limit == 0;
        let mut fps_changed = false;
        let mut vfr_changed = false;
        kit::group(ui, kit, "Frame rate", |g| {
            g.row("Match the display", &format!("Animations run at {native_hz} Hz, in step with the screen."), |ui| {
                if kit::toggle(ui, kit, &mut follow_native).changed() {
                    self.cfg.misc.fps_limit = if follow_native { 0 } else { native_hz };
                    fps_changed = true;
                }
            });
            if !follow_native {
                g.row("Frame rate limit", "", |ui| {
                    if ui.add(egui::Slider::new(&mut self.cfg.misc.fps_limit, 30..=500).suffix(" FPS")).changed() {
                        fps_changed = true;
                    }
                });
                g.custom(|ui| {
                    ui.horizontal(|ui| {
                        for preset in [60u32, 120, 144, 165, 240, 360] {
                            if kit::button(ui, kit, &format!("{preset}"), self.cfg.misc.fps_limit == preset).clicked() {
                                self.cfg.misc.fps_limit = preset;
                                fps_changed = true;
                            }
                        }
                    });
                });
            }
            g.row("Save power when idle", "Stops redrawing when nothing moves (variable frame rate).", |ui| {
                if kit::toggle(ui, kit, &mut self.cfg.misc.vfr).changed() {
                    vfr_changed = true;
                }
            });
        });
        if vfr_changed {
            self.doc.set_category_key("misc", "vfr", bool_str(self.cfg.misc.vfr));
            self.sync_config_and_notify();
        }
        if fps_changed {
            self.doc.set_category_key("misc", "fps_limit", &self.cfg.misc.fps_limit.to_string());
            self.sync_config_and_notify();
            let fps = (self.cfg.misc.fps_limit != 0).then_some(self.cfg.misc.fps_limit);
            let _ = send_ipc_command_blocking(PIPE_NAME_DAEMON, &IpcCommand::SetFramerate(fps));
        }

        let mut mode_changed = false;
        kit::group(ui, kit, "Compositor", |g| {
            g.row("Engine", "Standard is safe with anti-cheat games. The DWM hook injects into Windows' compositor (experimental).", |ui| {
                mode_changed = kit::segmented(ui, kit, &mut self.cfg.misc.dwm_hook_enabled, &[(false, "Standard"), (true, "DWM hook")]);
            });
        });
        if mode_changed {
            self.doc.set_category_key("misc", "dwm_hook_enabled", bool_str(self.cfg.misc.dwm_hook_enabled));
            self.sync_config_and_notify();
            let _ = send_ipc_command_blocking(
                PIPE_NAME_DAEMON,
                &IpcCommand::SetCompositorMode { dwm_hook_enabled: self.cfg.misc.dwm_hook_enabled },
            );
        }
    }

    // ───────────────────────────── Tiling ─────────────────────────────

    fn page_tiling(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        let mut changed = false;
        kit::group(ui, kit, "Layout", |g| {
            g.row("Layout", "Dwindle splits space in a spiral; Master keeps one big window and a stack.", |ui| {
                let mut sel: &str = if self.cfg.general.layout == "master" { "master" } else { "dwindle" };
                if kit::segmented(ui, kit, &mut sel, &[("dwindle", "Dwindle"), ("master", "Master")]) {
                    self.cfg.general.layout = sel.to_string();
                    self.doc.set_category_key("general", "layout", sel);
                    changed = true;
                }
            });
            g.row("Gaps between windows", "", |ui| {
                if ui.add(egui::Slider::new(&mut self.cfg.general.gaps_in, 0..=40).suffix(" px")).changed() {
                    self.doc.set_category_key("general", "gaps_in", &self.cfg.general.gaps_in.to_string());
                    changed = true;
                }
            });
            g.row("Gaps at screen edges", "", |ui| {
                if ui.add(egui::Slider::new(&mut self.cfg.general.gaps_out, 0..=60).suffix(" px")).changed() {
                    self.doc.set_category_key("general", "gaps_out", &self.cfg.general.gaps_out.to_string());
                    changed = true;
                }
            });
            g.row("Border width", "", |ui| {
                if ui.add(egui::Slider::new(&mut self.cfg.general.border_size, 0..=12).suffix(" px")).changed() {
                    self.doc.set_category_key("general", "border_size", &self.cfg.general.border_size.to_string());
                    changed = true;
                }
            });
        });
        kit::group(ui, kit, "Dwindle", |g| {
            g.row("Split ratio", "", |ui| {
                if ui.add(egui::Slider::new(&mut self.cfg.dwindle.default_split_ratio, 0.4..=1.6).max_decimals(2)).changed() {
                    self.doc.set_category_key("dwindle", "default_split_ratio", &format!("{:.2}", self.cfg.dwindle.default_split_ratio));
                    changed = true;
                }
            });
            g.row("Keep split direction", "A split keeps its direction when windows are resized.", |ui| {
                if kit::toggle(ui, kit, &mut self.cfg.dwindle.preserve_split).changed() {
                    self.doc.set_category_key("dwindle", "preserve_split", bool_str(self.cfg.dwindle.preserve_split));
                    changed = true;
                }
            });
        });
        kit::group(ui, kit, "Master", |g| {
            g.row("Master area size", "", |ui| {
                if ui.add(egui::Slider::new(&mut self.cfg.master.mfact, 0.25..=0.75).max_decimals(2)).changed() {
                    self.doc.set_category_key("master", "mfact", &format!("{:.2}", self.cfg.master.mfact));
                    changed = true;
                }
            });
        });
        kit::group(ui, kit, "Behavior", |g| {
            g.row("No gaps for a single window", "", |ui| {
                if kit::toggle(ui, kit, &mut self.cfg.dwindle.smart_gaps).changed() {
                    let v = bool_str(self.cfg.dwindle.smart_gaps);
                    self.cfg.master.smart_gaps = self.cfg.dwindle.smart_gaps;
                    self.doc.set_category_key("dwindle", "smart_gaps", v);
                    self.doc.set_category_key("master", "smart_gaps", v);
                    changed = true;
                }
            });
            g.row("Hide title bars", "Removes the Windows title bar from tiled windows.", |ui| {
                if kit::toggle(ui, kit, &mut self.cfg.general.no_titlebars).changed() {
                    self.doc.set_category_key("general", "no_titlebars", bool_str(self.cfg.general.no_titlebars));
                    changed = true;
                }
            });
        });
        if changed {
            self.sync_config_and_notify();
        }
    }

    // ───────────────────────────── Animations ─────────────────────────────

    fn page_animations(&mut self, ui: &mut egui::Ui, kit: &Kit, dt: f32) {
        if self.cfg.beziers.is_empty() {
            self.cfg.beziers.push(BezierCurve { name: "myBezier".to_string(), x0: 0.05, y0: 0.9, x1: 0.1, y1: 1.05 });
        }
        if self.selected_bezier_idx >= self.cfg.beziers.len() {
            self.selected_bezier_idx = 0;
        }
        self.bezier_preview_t = (self.bezier_preview_t + dt * 0.7) % 1.3;
        let mut changed = false;
        kit::group(ui, kit, "Easing curves", |g| {
            g.row("Curve", "", |ui| {
                let names: Vec<(usize, String)> = self.cfg.beziers.iter().map(|b| b.name.clone()).enumerate().collect();
                let opts: Vec<(usize, &str)> = names.iter().map(|(i, n)| (*i, n.as_str())).collect();
                kit::segmented(ui, kit, &mut self.selected_bezier_idx, &opts);
            });
            let curve = &mut self.cfg.beziers[self.selected_bezier_idx];
            for (label, v, range) in [
                ("Point 1 · x", &mut curve.x0, 0.0..=1.0),
                ("Point 1 · y", &mut curve.y0, -0.5..=1.5),
                ("Point 2 · x", &mut curve.x1, 0.0..=1.0),
                ("Point 2 · y", &mut curve.y1, -0.5..=1.5),
            ] {
                g.row(label, "", |ui| {
                    changed |= ui.add(egui::Slider::new(v, range).max_decimals(2)).changed();
                });
            }
            let curve = curve.clone();
            let t = self.bezier_preview_t.min(1.0);
            g.custom(|ui| {
                ui.horizontal(|ui| {
                    // Curve plot.
                    let (rect, _) = ui.allocate_exact_size(Vec2::new(200.0, 150.0), egui::Sense::hover());
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(rect, CornerRadius::same(8), kit.bg);
                    let to_screen = |u: f32, v: f32| egui::pos2(rect.left() + 16.0 + u * (rect.width() - 32.0), rect.bottom() - 22.0 - v.clamp(-0.3, 1.3) * (rect.height() - 44.0));
                    painter.line_segment([to_screen(0.0, 0.0), to_screen(1.0, 1.0)], Stroke::new(1.0_f32, kit.border));
                    let pts: Vec<egui::Pos2> = (0..=48).map(|s| s as f32 / 48.0).map(|u| to_screen(u, curve.sample(u))).collect();
                    painter.add(egui::Shape::line(pts, Stroke::new(2.5_f32, kit.accent)));
                    painter.circle_filled(to_screen(t, curve.sample(t)), 4.5, kit.accent);
                    ui.add_space(12.0);
                    // A window sliding with the curve.
                    let (prev, _) = ui.allocate_exact_size(Vec2::new(ui.available_width().max(160.0), 150.0), egui::Sense::hover());
                    let p = ui.painter_at(prev);
                    p.rect_filled(prev, CornerRadius::same(8), kit.bg);
                    let y = curve.sample(t);
                    let w = 64.0;
                    let x = prev.left() + 14.0 + y.clamp(-0.1, 1.2) * (prev.width() - w - 28.0);
                    let win = egui::Rect::from_min_size(egui::pos2(x, prev.center().y - 26.0), Vec2::new(w, 52.0));
                    p.rect(win, CornerRadius::same(7), kit.card, Stroke::new(2.0_f32, kit.accent), egui::StrokeKind::Inside);
                    p.rect_filled(egui::Rect::from_min_size(win.min + Vec2::new(8.0, 9.0), Vec2::new(w * 0.55, 5.0)), CornerRadius::same(3), kit.sub);
                });
            });
        });
        kit::group(ui, kit, "Animations", |g| {
            for rule in &self.cfg.animation_rules {
                g.row(&rule.name, "", |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} ms · {}{}",
                            (rule.speed * 100.0).round(),
                            rule.curve,
                            rule.style.as_deref().map(|s| format!(" · {s}")).unwrap_or_default()
                        ))
                        .color(kit.sub),
                    );
                    if !rule.enabled {
                        kit::chip(ui, kit, "Off", kit.sub);
                    }
                });
            }
        });
        if changed {
            let lines: Vec<String> = self
                .cfg
                .beziers
                .iter()
                .map(|b| format!("{}, {:.2}, {:.2}, {:.2}, {:.2}", b.name, b.x0, b.y0, b.x1, b.y1))
                .collect();
            self.doc.replace_directives("bezier", &lines);
            self.sync_config_and_notify();
        }
    }

    // ───────────────────────────── Keyboard ─────────────────────────────

    fn page_keyboard(&mut self, ui: &mut egui::Ui, kit: &Kit, ctx: &egui::Context) {
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
                    if let egui::Event::Key { key, pressed: true, .. } = ev {
                        if !mods.is_empty() {
                            self.recorded_mods = mods.join(" ");
                        }
                        self.recorded_key = format!("{key:?}");
                        self.recording_keybind = false;
                    }
                }
            });
        }

        let mut add = false;
        kit::group(ui, kit, "New shortcut", |g| {
            g.row("Keys", if self.recording_keybind { "Press the keys now…" } else { "Type them, or record them." }, |ui| {
                let label = if self.recording_keybind { "Cancel" } else { "Record" };
                if kit::button(ui, kit, label, self.recording_keybind).clicked() {
                    self.recording_keybind = !self.recording_keybind;
                }
                ui.add(egui::TextEdit::singleline(&mut self.recorded_key).desired_width(70.0));
                ui.label(RichText::new("+").color(kit.sub));
                ui.add(egui::TextEdit::singleline(&mut self.recorded_mods).desired_width(100.0));
            });
            g.row("Action", "A dispatcher and its argument, e.g. exec wt.exe.", |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.new_bind_arg).desired_width(130.0));
                ui.add(egui::TextEdit::singleline(&mut self.new_bind_dispatcher).desired_width(90.0));
            });
            g.row("", "", |ui| {
                if kit::button(ui, kit, "Add shortcut", true).clicked() {
                    add = true;
                }
            });
        });
        if add {
            let rule = BindRule {
                kind: BindKind::Press,
                mods: self.recorded_mods.clone(),
                key: self.recorded_key.clone(),
                dispatcher: self.new_bind_dispatcher.clone(),
                arg: self.new_bind_arg.clone(),
            };
            self.doc.add_directive("bind", &format!("{}, {}, {}, {}", rule.mods, rule.key, rule.dispatcher, rule.arg));
            self.cfg.binds.push(rule);
            self.sync_config_and_notify();
        }

        kit::search_field(ui, kit, &mut self.bind_search, "Search shortcuts");
        ui.add_space(10.0);
        let q = self.bind_search.trim().to_ascii_lowercase();
        kit::group(ui, kit, "Shortcuts", |g| {
            for b in &self.cfg.binds {
                let hay = format!("{} {} {} {}", b.mods, b.key, b.dispatcher, b.arg).to_ascii_lowercase();
                if !q.is_empty() && !hay.contains(&q) {
                    continue;
                }
                g.custom(|ui| {
                    ui.horizontal(|ui| {
                        for m in b.mods.split(|c: char| c == ' ' || c == '_' || c == '+').filter(|m| !m.is_empty()) {
                            kit::keycap(ui, kit, m);
                        }
                        kit::keycap(ui, kit, &b.key);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new(format!("{} {}", b.dispatcher, b.arg).trim().to_string()).color(kit.sub));
                        });
                    });
                });
            }
        });
    }

    // ───────────────────────────── Window rules ─────────────────────────────

    fn page_window_rules(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        // Inspector: the next left click anywhere picks the window under the cursor.
        if self.inspecting_crosshair {
            unsafe {
                if (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 {
                    let mut pt = POINT::default();
                    if GetCursorPos(&mut pt).is_ok() {
                        let raw = WindowFromPoint(pt);
                        let root = GetAncestor(raw, GA_ROOT);
                        let target = if !root.0.is_null() { root } else { raw };
                        if !target.0.is_null() {
                            let mut cbuf = [0u16; 128];
                            let clen = GetClassNameW(target, &mut cbuf);
                            if clen > 0 {
                                self.inspected_class = String::from_utf16_lossy(&cbuf[..clen as usize]);
                            }
                            let mut tbuf = [0u16; 256];
                            let tlen = GetWindowTextW(target, &mut tbuf);
                            self.inspected_title = if tlen > 0 { String::from_utf16_lossy(&tbuf[..tlen as usize]) } else { String::new() };
                        }
                    }
                    self.inspecting_crosshair = false;
                }
            }
        }

        let mut add = false;
        kit::group(ui, kit, "New rule", |g| {
            g.row(
                "Pick a window",
                if self.inspecting_crosshair { "Now click any window…" } else { "Click the button, then click a window to read its class." },
                |ui| {
                    let label = if self.inspecting_crosshair { "Cancel" } else { "🎯 Pick" };
                    if kit::button(ui, kit, label, self.inspecting_crosshair).clicked() {
                        self.inspecting_crosshair = !self.inspecting_crosshair;
                    }
                },
            );
            let title_hint = if self.inspected_title.is_empty() { String::new() } else { format!("Title: {}", self.inspected_title) };
            g.row("Window class", &title_hint, |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.inspected_class).desired_width(200.0).hint_text("e.g. Taskmgr"));
            });
            g.row("Rule", "float · workspace 3 · opacity 0.9 · size 800 600 …", |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.new_rule_effect).desired_width(200.0));
            });
            g.row("", "", |ui| {
                if kit::button(ui, kit, "Add rule", true).clicked() {
                    add = true;
                }
            });
        });
        if add && !self.inspected_class.trim().is_empty() {
            let matcher = format!("class:^({})$", self.inspected_class.trim());
            let raw = format!("{}, {}", self.new_rule_effect.trim(), matcher);
            self.doc.add_directive("windowrulev2", &raw);
            self.cfg.window_rules.push(WindowRuleV2 { effect: self.new_rule_effect.trim().to_string(), matcher });
            self.sync_config_and_notify();
        }

        let mut remove = None;
        kit::group(ui, kit, "Rules", |g| {
            if self.cfg.window_rules.is_empty() {
                g.row("No rules yet", "", |_| {});
            }
            for (i, r) in self.cfg.window_rules.iter().enumerate() {
                g.row(&r.effect, &r.matcher, |ui| {
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                });
            }
        });
        if let Some(i) = remove {
            self.cfg.window_rules.remove(i);
            // Rules are rewritten as windowrulev2 lines (old `windowrule` lines use the same syntax).
            let lines: Vec<String> = self.cfg.window_rules.iter().map(|r| format!("{}, {}", r.effect, r.matcher)).collect();
            self.doc.replace_directives("windowrule", &[]);
            self.doc.replace_directives("windowrulev2", &lines);
            self.sync_config_and_notify();
        }
    }

    // ───────────────────────────── frame ─────────────────────────────

    fn sidebar(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        ui.add_space(10.0);
        kit::search_field(ui, kit, &mut self.search, "Search");
        ui.add_space(12.0);
        let q = self.search.trim().to_ascii_lowercase();
        let mut shown = 0;
        for p in &PAGES {
            if !q.is_empty() && !p.title.to_ascii_lowercase().contains(&q) && !p.keywords.contains(&q) {
                continue;
            }
            shown += 1;
            if kit::sidebar_item(ui, kit, p.glyph, p.tile, p.title, self.page == p.page) {
                self.page = p.page;
            }
            ui.add_space(2.0);
        }
        if shown == 0 {
            ui.add_space(8.0);
            ui.label(RichText::new("No results").color(kit.sub));
        } else if shown == 1 && !q.is_empty() {
            // A single match opens it right away.
            if let Some(p) = PAGES.iter().find(|p| p.title.to_ascii_lowercase().contains(&q) || p.keywords.contains(&q)) {
                self.page = p.page;
            }
        }
    }

    fn status_toast(&mut self, ctx: &egui::Context, kit: &Kit, area: egui::Rect) {
        if self.status_banner != self.status_seen {
            self.status_seen = self.status_banner.clone();
            self.status_at = (!self.status_banner.is_empty()).then(Instant::now);
        }
        let Some(at) = self.status_at else { return };
        let age = at.elapsed().as_secs_f32();
        if age > 2.6 {
            self.status_at = None;
            return;
        }
        ctx.request_repaint();
        let a = (age / 0.15).min(1.0) * ((2.6 - age) / 0.4).min(1.0);
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("status-toast")));
        let font = egui::FontId::proportional(13.0);
        let galley = painter.layout_no_wrap(self.status_banner.clone(), font, kit.accent_fg.gamma_multiply(a));
        let size = galley.size() + Vec2::new(28.0, 14.0);
        let rect = egui::Rect::from_center_size(egui::pos2(area.center().x, area.bottom() - 30.0 + (1.0 - a) * 8.0), size);
        painter.rect_filled(rect.translate(Vec2::new(0.0, 2.0)), CornerRadius::same(255), Color32::from_black_alpha((40.0 * a) as u8));
        painter.rect_filled(rect, CornerRadius::same(255), kit.accent.gamma_multiply(a));
        painter.galley(rect.min + Vec2::new(14.0, 7.0), galley, kit.accent_fg.gamma_multiply(a));
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;
        if self.page == Page::Animations || self.inspecting_crosshair {
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        let sheet = self.css.read().clone();
        let kit = Kit::from_css(&sheet);
        if self.visuals_from.as_ref() != Some(&sheet) {
            kit::apply_visuals(ctx, &kit);
            self.visuals_from = Some(sheet);
        }

        egui::SidePanel::left("settings_sidebar")
            .resizable(false)
            .exact_width(236.0)
            .frame(egui::Frame::NONE.fill(kit.sidebar).inner_margin(egui::Margin::symmetric(10, 6)).stroke(Stroke::new(1.0_f32, kit.border)))
            .show(ctx, |ui| self.sidebar(ui, &kit));

        let central = egui::CentralPanel::default().frame(egui::Frame::NONE.fill(kit.bg)).show(ctx, |ui| {
            let page = info(self.page);
            egui::ScrollArea::vertical().id_salt(("page", self.page as u8)).auto_shrink([false, false]).show(ui, |ui| {
                // A centered content column, like macOS.
                let width = ui.available_width();
                let column = width.min(700.0) - 48.0;
                let side = ((width - column) / 2.0).max(24.0);
                ui.horizontal(|ui| {
                    ui.add_space(side);
                    ui.vertical(|ui| {
                        ui.set_width(column);
                        ui.add_space(22.0);
                        kit::page_title(ui, &kit, page.title, page.subtitle);
                        match self.page {
                            Page::General => self.page_general(ui, &kit),
                            Page::Appearance => self.page_appearance(ui, &kit),
                            Page::Wallpaper => self.page_wallpaper(ui, &kit),
                            Page::Displays => self.page_displays(ui, &kit),
                            Page::Tiling => self.page_tiling(ui, &kit),
                            Page::Animations => self.page_animations(ui, &kit, dt),
                            Page::Keyboard => self.page_keyboard(ui, &kit, ctx),
                            Page::WindowRules => self.page_window_rules(ui, &kit),
                        }
                        ui.add_space(30.0);
                    });
                });
            });
        });
        self.status_toast(ctx, &kit, central.response.rect);
    }
}

fn main() -> Result<()> {
    init_tracing("chickenwm_settings");
    let _failsafe = spawn_emergency_failsafe_thread();

    let (doc, cfg) = bootstrap_config_and_themes()?;
    let css = Arc::new(RwLock::new(load_css_sheet("settings.css").unwrap_or_default()));
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
            .with_title("Settings")
            .with_inner_size([960.0, 680.0])
            .with_min_inner_size([720.0, 480.0]),
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
