//! `chickenwm-settings`: the ChickenWM settings app.
//!
//! Laid out like macOS System Settings: a searchable sidebar of sections with colored icon
//! tiles, and pages made of grouped cards whose rows hold a label on the left and a control on
//! the right. Every change is written straight into `chickenwm.conf` through the lossless
//! `ConfigDocument` AST and applied live over IPC. The window follows the system theme
//! (`settings.css`).
//!
//! Pages live under `src/pages/`: General, Appearance, Wallpaper, Displays, Tiling,
//! Animations, Keyboard, Window Rules, Fonts, and Logs.

// Release builds are windowed apps: no console window when launched from the shell, a
// hotkey or the launcher. Debug builds keep the console for logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod kit;
mod pages;
mod theme_ui;

use anyhow::Result;
use chickenwm_core::bootstrap_config_and_themes;
use chickenwm_core::config::{config_dir, save_config_document, ConfigDocument, HyprConfig};
use chickenwm_core::css::{load_css_sheet, spawn_config_watcher, ConfigWatchEvent, CssStyleSheet};
use chickenwm_core::failsafe::{
    is_files_handler_registered, register_files_shell_handler, spawn_emergency_failsafe_thread,
};
use chickenwm_core::init_tracing;
use chickenwm_core::ipc::{
    send_ipc_command_blocking, IpcCommand, MonitorInfo, PIPE_NAME_DAEMON, PIPE_NAME_WM,
};
use chickenwm_core::monitor::enumerate_monitors;
use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke, Vec2};
use kit::Kit;
use parking_lot::RwLock;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    General,
    Appearance,
    Wallpaper,
    Displays,
    Tiling,
    Animations,
    Keyboard,
    WindowRules,
    Fonts,
    Logs,
}

pub(crate) struct PageInfo {
    pub page: Page,
    pub glyph: &'static str,
    pub tile: Color32,
    pub title: &'static str,
    pub subtitle: &'static str,
    /// Extra words the sidebar search matches.
    pub keywords: &'static str,
}

pub(crate) const PAGES: [PageInfo; 10] = [
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
        page: Page::Fonts,
        glyph: "A",
        tile: Color32::from_rgb(175, 82, 222),
        title: "Fonts",
        subtitle: "Typefaces for the bar, launcher, apps and browser, and Nerd Font downloads.",
        keywords: "font typeface nerd fonts icons glyphs download install jetbrains fira cascadia departure",
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
        keywords: "keybinds binds shortcuts hotkeys keys submap chain",
    },
    PageInfo {
        page: Page::WindowRules,
        glyph: "▣",
        tile: Color32::from_rgb(255, 59, 48),
        title: "Window Rules",
        subtitle: "Per-app rules: float, workspace, opacity, process, initialTitle and more.",
        keywords: "windowrule windowrulev2 float class process initialtitle inspector crosshair",
    },
    PageInfo {
        page: Page::Logs,
        glyph: "☰",
        tile: Color32::from_rgb(120, 120, 128),
        title: "Logs",
        subtitle: "What each module wrote, and crashes.",
        keywords: "log logs crash crashes error errors panic debug trace module",
    },
];

pub(crate) fn info(page: Page) -> &'static PageInfo {
    PAGES.iter().find(|p| p.page == page).unwrap_or(&PAGES[0])
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoEntry {
    pub label: String,
    pub before: String,
    pub diff: Vec<String>,
}

/// Computes a concise `- old` / `+ new` line diff between two texts.
pub fn compute_line_diff(before: &str, after: &str) -> Vec<String> {
    let old_lines: Vec<&str> = before.lines().collect();
    let new_lines: Vec<&str> = after.lines().collect();
    let mut out = Vec::new();
    let max_len = old_lines.len().max(new_lines.len());
    for i in 0..max_len {
        match (old_lines.get(i).copied(), new_lines.get(i).copied()) {
            (Some(a), Some(b)) if a != b => {
                out.push(format!("- L{}: {a}", i + 1));
                out.push(format!("+ L{}: {b}", i + 1));
            }
            (Some(a), None) => out.push(format!("- L{}: {a}", i + 1)),
            (None, Some(b)) => out.push(format!("+ L{}: {b}", i + 1)),
            _ => {}
        }
    }
    out
}

pub(crate) struct SettingsApp {
    pub page: Page,
    pub search: String,
    pub last_search_jumped: String,
    pub doc: ConfigDocument,
    pub cfg: HyprConfig,
    pub monitors: Vec<MonitorInfo>,
    pub css: Arc<RwLock<CssStyleSheet>>,
    pub font: chickenwm_core::fonts::EguiFont,
    pub installed_fonts: Option<Vec<String>>,
    pub current_fonts: Option<(Instant, Vec<String>)>,
    pub font_filter: String,
    pub font_confirm: Option<FontRequest>,
    pub font_job: Option<Arc<std::sync::Mutex<FontJob>>>,
    pub status_banner: String,
    pub status_seen: String,
    pub status_at: Option<Instant>,

    pub new_exec_once: String,

    pub show_css_editor: bool,
    /// roobar.css `--islands` (each widget its own pill, or one background for the whole bar).
    pub bar_islands: bool,
    pub selected_css_file: String,
    pub css_editor_buffer: String,

    pub selected_bezier_idx: usize,
    pub bezier_preview_t: f32,
    pub last_tick: Instant,

    pub bind_search: String,
    pub recording_keybind: bool,
    pub recorded_mods: String,
    pub recorded_key: String,
    pub new_bind_dispatcher: String,
    pub new_bind_arg: String,

    pub inspecting_crosshair: bool,
    pub inspected_class: String,
    pub inspected_title: String,
    pub inspected_initial_title: String,
    pub inspected_process: String,
    pub new_rule_effect: String,

    pub wallpaper_path_input: String,
    pub wallpaper_transition: &'static str,

    pub files_handler_active: bool,

    pub theme_editor: theme_ui::ThemeEditor,
    pub visuals_from: Option<CssStyleSheet>,
    pub auto_dark: chickenwm_core::theme_tools::AutoDark,
    pub log_selected: Option<std::path::PathBuf>,
    pub log_filter: String,
    pub log_errors_only: bool,
    pub log_cache:
        Option<(Instant, Vec<(std::path::PathBuf, u64, Option<std::time::SystemTime>, usize)>)>,
    pub theme_file_path: String,
    /// Multi-step undo history (oldest to newest).
    pub undo: Vec<UndoEntry>,
    pub show_undo_history: bool,
    pub show_unsaved_diff: bool,
    /// Every setting row: (page, label, hint), for search. Built on the first frame.
    pub search_index: Option<Vec<(Page, String, String)>>,
    /// Problems `config_check` found in chickenwm.conf.
    pub config_issues: Vec<chickenwm_core::config_check::Issue>,
}

pub(crate) const TRANSITIONS: [&str; 4] = ["fade", "wipe", "outer", "grow"];

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FontTarget {
    One(&'static str),
    All,
    InstallOnly,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FontRequest {
    pub font: chickenwm_core::fonts::NerdFont,
    pub target: FontTarget,
}

pub(crate) struct FontJob {
    pub request: FontRequest,
    pub result: Option<Result<usize, String>>,
}

pub(crate) fn primary_family(list: &str) -> String {
    chickenwm_core::fonts::css_families(list)
        .into_iter()
        .next()
        .unwrap_or_else(|| "Default".to_string())
}

pub(crate) fn bool_str(b: bool) -> &'static str {
    if b {
        "true"
    } else {
        "false"
    }
}

impl SettingsApp {
    pub(crate) fn new(
        doc: ConfigDocument,
        cfg: HyprConfig,
        css: Arc<RwLock<CssStyleSheet>>,
    ) -> Self {
        let wp = cfg.wallpapers.first();
        let wallpaper_transition = wp
            .and_then(|w| {
                TRANSITIONS
                    .iter()
                    .copied()
                    .find(|t| t.eq_ignore_ascii_case(&w.transition))
            })
            .unwrap_or("fade");
        let mut args = std::env::args().skip_while(|a| a != "--page").skip(1);
        let page = args
            .next()
            .and_then(|want| {
                PAGES.iter().find(|p| {
                    p.title
                        .replace(' ', "")
                        .eq_ignore_ascii_case(&want.replace(['-', '_', ' '], ""))
                })
            })
            .map_or(Page::General, |p| p.page);
        Self {
            page,
            search: std::env::var("CHICKENWM_SETTINGS_SEARCH").unwrap_or_default(),
            last_search_jumped: String::new(),
            monitors: enumerate_monitors(),
            css,
            font: Default::default(),
            installed_fonts: None,
            current_fonts: None,
            font_filter: String::new(),
            font_confirm: None,
            font_job: None,
            status_banner: String::new(),
            status_seen: String::new(),
            status_at: None,
            new_exec_once: String::new(),
            show_css_editor: false,
            bar_islands: load_css_sheet("roobar.css")
                .ok()
                .and_then(|s| s.custom_properties.get("--islands").cloned())
                .is_none_or(|v| !matches!(v.trim().to_ascii_lowercase().as_str(), "false" | "no" | "off" | "0" | "none")),
            selected_css_file: "launcher.css".to_string(),
            css_editor_buffer: std::fs::read_to_string(config_dir().join("launcher.css"))
                .unwrap_or_default(),
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
            inspected_initial_title: String::new(),
            inspected_process: String::new(),
            new_rule_effect: "float".to_string(),
            wallpaper_path_input: wp.map(|w| w.path.clone()).unwrap_or_default(),
            wallpaper_transition,
            files_handler_active: is_files_handler_registered(),
            theme_editor: theme_ui::ThemeEditor::new(&cfg.theme),
            visuals_from: None,
            auto_dark: cfg.theme.auto_dark.clone(),
            log_selected: None,
            log_filter: String::new(),
            log_errors_only: false,
            log_cache: None,
            theme_file_path: config_dir()
                .join("my-theme.chickentheme")
                .display()
                .to_string(),
            undo: Vec::new(),
            show_undo_history: false,
            show_unsaved_diff: false,
            search_index: None,
            config_issues: chickenwm_core::config_check::validate_user_config().unwrap_or_default(),
            doc,
            cfg,
        }
    }

    /// Computes a diff of any unsaved changes (theme draft, CSS editor buffer, or AST vs disk).
    pub(crate) fn unsaved_diff(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.theme_editor.draft != self.cfg.theme {
            if self.theme_editor.draft.preset != self.cfg.theme.preset {
                out.push(format!("- theme.preset = {}", self.cfg.theme.preset));
                out.push(format!("+ theme.preset = {}", self.theme_editor.draft.preset));
            }
            if self.theme_editor.draft.mode != self.cfg.theme.mode {
                out.push(format!("- theme.mode = {}", self.cfg.theme.mode.as_str()));
                out.push(format!(
                    "+ theme.mode = {}",
                    self.theme_editor.draft.mode.as_str()
                ));
            }
            if self.theme_editor.draft.accent != self.cfg.theme.accent {
                out.push(format!(
                    "- theme.accent = {}",
                    chickenwm_core::theme::css(self.cfg.theme.accent)
                ));
                out.push(format!(
                    "+ theme.accent = {}",
                    chickenwm_core::theme::css(self.theme_editor.draft.accent)
                ));
            }
            if self.theme_editor.draft.icon_theme != self.cfg.theme.icon_theme {
                out.push(format!(
                    "- theme.icon_theme = {}",
                    self.cfg.theme.icon_theme
                ));
                out.push(format!(
                    "+ theme.icon_theme = {}",
                    self.theme_editor.draft.icon_theme
                ));
            }
        }
        if self.show_css_editor {
            let disk_css = std::fs::read_to_string(config_dir().join(&self.selected_css_file))
                .unwrap_or_default();
            if disk_css != self.css_editor_buffer {
                for l in compute_line_diff(&disk_css, &self.css_editor_buffer)
                    .into_iter()
                    .take(16)
                {
                    out.push(format!("[{}] {l}", self.selected_css_file));
                }
            }
        }
        let disk_conf =
            std::fs::read_to_string(config_dir().join("chickenwm.conf")).unwrap_or_default();
        let mem_conf = self.doc.to_lossless_string();
        if !disk_conf.is_empty() && disk_conf.trim() != mem_conf.trim() {
            out.extend(compute_line_diff(&disk_conf, &mem_conf).into_iter().take(20));
        }
        out
    }

    pub(crate) fn sync_config_and_notify_labeled(&mut self, label: &str) {
        let path = config_dir().join("chickenwm.conf");
        let before = std::fs::read_to_string(&path).unwrap_or_default();
        if let Err(e) = save_config_document(&self.doc) {
            self.status_banner = format!("Couldn't save chickenwm.conf: {e}");
            return;
        }
        if let Ok(now) = std::fs::read_to_string(&path) {
            if now != before {
                let diff = compute_line_diff(&before, &now);
                self.undo.push(UndoEntry {
                    label: label.to_string(),
                    before,
                    diff,
                });
                if self.undo.len() > 30 {
                    self.undo.remove(0);
                }
            }
        }
        self.config_issues =
            chickenwm_core::config_check::validate_user_config().unwrap_or_default();
        let _ = send_ipc_command_blocking(PIPE_NAME_DAEMON, &IpcCommand::ReloadConfig);
        let _ = send_ipc_command_blocking(PIPE_NAME_WM, &IpcCommand::ReloadConfig);
        self.status_banner = "Saved".to_string();
    }

    /// Reverts `chickenwm.conf` back to the state before `self.undo[step_idx]`, truncating newer steps.
    pub(crate) fn undo_to_step(&mut self, step_idx: usize) {
        if step_idx >= self.undo.len() {
            self.status_banner = "Nothing to undo".to_string();
            return;
        }
        let target = self.undo[step_idx].clone();
        self.undo.truncate(step_idx);
        let path = config_dir().join("chickenwm.conf");
        if let Err(e) = std::fs::write(&path, &target.before) {
            self.status_banner = format!("Couldn't undo: {e}");
            return;
        }
        match chickenwm_core::config::load_or_init_config() {
            Ok((doc, cfg)) => {
                let theme_changed = cfg.theme != self.cfg.theme;
                self.theme_editor = theme_ui::ThemeEditor::new(&cfg.theme);
                self.auto_dark = cfg.theme.auto_dark.clone();
                if theme_changed {
                    let _ = chickenwm_core::theme::apply_theme(&cfg.theme);
                }
                self.doc = doc;
                self.cfg = cfg;
                let _ = send_ipc_command_blocking(PIPE_NAME_DAEMON, &IpcCommand::ReloadConfig);
                let _ = send_ipc_command_blocking(PIPE_NAME_WM, &IpcCommand::ReloadConfig);
                self.config_issues =
                    chickenwm_core::config_check::validate_user_config().unwrap_or_default();
                self.status_banner = format!("Reverted: {}", target.label);
            }
            Err(e) => self.status_banner = format!("Undo wrote the file but it doesn't load: {e}"),
        }
    }

    pub(crate) fn undo_last_change(&mut self) {
        if self.undo.is_empty() {
            self.status_banner = "Nothing to undo".to_string();
            return;
        }
        let last = self.undo.len() - 1;
        self.undo_to_step(last);
    }

    pub(crate) fn send_shell_config(&self) {
        let _ = send_ipc_command_blocking(
            PIPE_NAME_DAEMON,
            &IpcCommand::SetShellConfig {
                replace_winlogon_shell: self.cfg.misc.replace_winlogon_shell,
                suppress_explorer: self.cfg.misc.suppress_explorer,
                register_files_handler: self.cfg.misc.register_files_handler,
            },
        );
    }

    pub(crate) fn apply_files_handler(&mut self, enable: bool) {
        if let Err(e) = register_files_shell_handler(enable, None) {
            self.status_banner = format!("Could not change the default file manager: {e}");
            self.files_handler_active = is_files_handler_registered();
            return;
        }
        self.files_handler_active = is_files_handler_registered();
        self.cfg.misc.register_files_handler = enable;
        self.doc
            .set_category_key("misc", "register_files_handler", bool_str(enable));
        self.sync_config_and_notify_labeled("Default file manager");
        self.send_shell_config();
        self.status_banner = if enable {
            "ChickenWM Files is now the default file manager".to_string()
        } else {
            "Explorer is the default file manager again".to_string()
        };
    }

    pub(crate) fn apply_run_as_admin(&mut self, enable: bool) {
        let exe = match std::env::current_exe() {
            Ok(mut p) => {
                p.set_file_name("chickenwm-rooster.exe");
                p
            }
            Err(e) => {
                self.status_banner = format!("Couldn't find chickenwm-rooster.exe: {e}");
                return;
            }
        };
        let flag = if enable {
            "--install-admin-task"
        } else {
            "--remove-admin-task"
        };
        match chickenwm_core::elevation::run_elevated(&exe, &[flag], true) {
            Ok(0) => {
                self.cfg.misc.run_as_admin = enable;
                self.doc
                    .set_category_key("misc", "run_as_admin", bool_str(enable));
                self.sync_config_and_notify_labeled("Run as administrator");
                self.status_banner = if enable {
                    "ChickenWM will start as administrator at your next sign-in".to_string()
                } else {
                    "ChickenWM will start normally again".to_string()
                };
            }
            Ok(code) => {
                self.status_banner = format!("Couldn't change the administrator task (code {code})")
            }
            Err(_) => self.status_banner = "Administrator approval was cancelled".to_string(),
        }
    }

    pub(crate) fn apply_system_theme(&mut self) {
        let theme = self.theme_editor.draft.clone();
        match chickenwm_core::theme::apply_theme(&theme) {
            Ok(palette) => {
                chickenwm_core::theme::write_theme_keys(&mut self.doc, &theme, &palette);
                let border = chickenwm_core::theme::hypr_color(palette.active_border());
                self.cfg.general.col_active_border =
                    chickenwm_core::config::BorderColorSpec::parse(&border);
                self.cfg.theme = theme;
                self.sync_config_and_notify_labeled("Apply system theme");
                self.theme_editor.invalidate_icons();
                self.status_banner = "Theme applied".to_string();
            }
            Err(e) => self.status_banner = format!("Couldn't apply the theme: {e}"),
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        ui.add_space(10.0);
        let search_resp = kit::search_field(ui, kit, &mut self.search, "Search");
        ui.add_space(12.0);
        let q = self.search.trim().to_ascii_lowercase();
        let mut shown = 0;
        for p in &PAGES {
            if !q.is_empty()
                && !p.title.to_ascii_lowercase().contains(&q)
                && !p.keywords.contains(&q)
            {
                continue;
            }
            shown += 1;
            if kit::sidebar_item(ui, kit, p.glyph, p.tile, p.title, self.page == p.page) {
                self.page = p.page;
            }
            ui.add_space(2.0);
        }

        // Individual settings that match, across every page.
        let mut settings_hits = 0;
        if !q.is_empty() {
            let hits: Vec<(Page, String)> = self
                .search_index
                .as_deref()
                .unwrap_or_default()
                .iter()
                .filter(|(_, label, hint)| {
                    label.to_lowercase().contains(&q) || hint.to_lowercase().contains(&q)
                })
                .take(14)
                .map(|(p, l, _)| (*p, l.clone()))
                .collect();

            // Automatically jump to and highlight the top matching control as the user searches or presses Enter.
            if let Some((first_page, first_label)) = hits.first() {
                let enter_pressed =
                    search_resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if self.last_search_jumped != q || enter_pressed {
                    self.last_search_jumped = q.clone();
                    self.page = *first_page;
                    kit::highlight_row(first_label);
                }
            }

            if !hits.is_empty() {
                ui.add_space(10.0);
                ui.label(RichText::new("SETTINGS").size(11.0).color(kit.sub));
                ui.add_space(4.0);
            }
            for (page, label) in hits {
                settings_hits += 1;
                let text = format!("{label}\n{}", info(page).title);
                let resp = ui.add(
                    egui::Button::new(RichText::new(text).size(12.5))
                        .frame(false)
                        .wrap(),
                );
                if resp.clicked() {
                    self.page = page;
                    kit::highlight_row(&label);
                }
            }
        } else {
            self.last_search_jumped.clear();
        }

        // Unsaved changes diff preview.
        let unsaved = self.unsaved_diff();
        if !unsaved.is_empty() {
            ui.add_space(12.0);
            let label = format!("Unsaved diff ({})", unsaved.len());
            if kit::button(ui, kit, &label, self.show_unsaved_diff).clicked() {
                self.show_unsaved_diff = !self.show_unsaved_diff;
            }
            if self.show_unsaved_diff {
                ui.add_space(4.0);
                egui::Frame::NONE
                    .fill(kit.card)
                    .stroke(Stroke::new(1.0_f32, kit.border))
                    .corner_radius(CornerRadius::same(7))
                    .inner_margin(egui::Margin::symmetric(8, 6))
                    .show(ui, |ui| {
                        for line in unsaved.iter().take(12) {
                            let color = if line.starts_with('-') || line.contains(" - ") {
                                kit.danger
                            } else {
                                Color32::from_rgb(52, 199, 89)
                            };
                            ui.label(RichText::new(line).monospace().size(10.5).color(color));
                        }
                    });
            }
        }

        // Multi-step Undo history.
        if !self.undo.is_empty() {
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if kit::button(ui, kit, "Undo", false)
                    .on_hover_text("Ctrl+Z")
                    .clicked()
                {
                    self.undo_last_change();
                }
                let hist_label = format!("History ({})", self.undo.len());
                if kit::button(ui, kit, &hist_label, self.show_undo_history).clicked() {
                    self.show_undo_history = !self.show_undo_history;
                }
            });
            if self.show_undo_history {
                ui.add_space(4.0);
                let mut revert_to = None;
                let entries: Vec<(usize, String, usize)> = self
                    .undo
                    .iter()
                    .enumerate()
                    .rev()
                    .map(|(i, e)| (i, e.label.clone(), e.diff.len()))
                    .collect();
                for (idx, label, diff_len) in entries {
                    let row_text = format!("#{} {label} ({diff_len}Δ)", idx + 1);
                    if ui
                        .small_button(RichText::new(row_text).size(11.5).color(kit.fg))
                        .on_hover_text("Click to revert back to before this step")
                        .clicked()
                    {
                        revert_to = Some(idx);
                    }
                }
                if let Some(idx) = revert_to {
                    self.undo_to_step(idx);
                }
            }
        }

        if shown == 0 && settings_hits == 0 {
            ui.add_space(8.0);
            ui.label(RichText::new("No results").color(kit.sub));
        } else if shown == 1 && settings_hits == 0 && !q.is_empty() {
            if let Some(p) = PAGES
                .iter()
                .find(|p| p.title.to_ascii_lowercase().contains(&q) || p.keywords.contains(&q))
            {
                self.page = p.page;
            }
        }
    }

    fn draw_page(
        &mut self,
        page: Page,
        ui: &mut egui::Ui,
        kit: &Kit,
        dt: f32,
        ctx: &egui::Context,
    ) {
        match page {
            Page::General => self.page_general(ui, kit),
            Page::Appearance => self.page_appearance(ui, kit),
            Page::Wallpaper => self.page_wallpaper(ui, kit),
            Page::Displays => self.page_displays(ui, kit),
            Page::Tiling => self.page_tiling(ui, kit),
            Page::Animations => self.page_animations(ui, kit, dt),
            Page::Keyboard => self.page_keyboard(ui, kit, ctx),
            Page::WindowRules => self.page_window_rules(ui, kit),
            Page::Fonts => self.page_fonts(ui, kit),
            Page::Logs => self.page_logs(ui, kit),
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
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("status-toast"),
        ));
        let font = egui::FontId::proportional(13.0);
        let galley =
            painter.layout_no_wrap(self.status_banner.clone(), font, kit.accent_fg.gamma_multiply(a));
        let size = galley.size() + Vec2::new(28.0, 14.0);
        let rect = egui::Rect::from_center_size(
            egui::pos2(area.center().x, area.bottom() - 30.0 + (1.0 - a) * 8.0),
            size,
        );
        painter.rect_filled(
            rect.translate(Vec2::new(0.0, 2.0)),
            CornerRadius::same(255),
            Color32::from_black_alpha((40.0 * a) as u8),
        );
        painter.rect_filled(rect, CornerRadius::same(255), kit.accent.gamma_multiply(a));
        painter.galley(
            rect.min + Vec2::new(14.0, 7.0),
            galley,
            kit.accent_fg.gamma_multiply(a),
        );
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
            self.font.sync(ctx, &sheet, false);
            self.visuals_from = Some(sheet);
        }

        kit::set_config_issues(&self.config_issues);

        egui::SidePanel::left("settings_sidebar")
            .resizable(false)
            .exact_width(236.0)
            .frame(
                egui::Frame::NONE
                    .fill(kit.sidebar)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .stroke(Stroke::new(1.0_f32, kit.border)),
            )
            .show(ctx, |ui| self.sidebar(ui, &kit));

        // Ctrl+Z (when no text field has the keyboard): undo the last saved change.
        if !ctx.wants_keyboard_input()
            && ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Z))
        {
            self.undo_last_change();
        }

        let central = egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(kit.bg))
            .show(ctx, |ui| {
                // First frame: draw every page once, invisibly, to learn its settings for search.
                if self.search_index.is_none() {
                    let current = self.page;
                    let mut index = Vec::new();
                    for p in PAGES.iter().map(|p| p.page) {
                        let mut hidden = ui.new_child(
                            egui::UiBuilder::new()
                                .id_salt(("search-index", p as u8))
                                .invisible(),
                        );
                        let rows =
                            kit::collect_rows(|| self.draw_page(p, &mut hidden, &kit, 0.0, ctx));
                        index.extend(rows.into_iter().map(|(label, hint)| (p, label, hint)));
                    }
                    self.page = current;
                    self.search_index = Some(index);
                }
                let page = info(self.page);
                egui::ScrollArea::vertical()
                    .id_salt(("page", self.page as u8))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let width = ui.available_width();
                        let column = width.min(700.0) - 48.0;
                        let side = ((width - column) / 2.0).max(24.0);
                        ui.horizontal(|ui| {
                            ui.add_space(side);
                            ui.vertical(|ui| {
                                ui.set_width(column);
                                ui.add_space(22.0);
                                kit::page_title(ui, &kit, page.title, page.subtitle);
                                self.draw_page(self.page, ui, &kit, dt, ctx);
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

    // `--render-to out.png [--page <name>]`: one frame, headless.
    if let Some(req) = chickenwm_core::headless::RenderRequest::from_args([960.0, 680.0]) {
        return chickenwm_core::headless::render_app(&req, |_cc| SettingsApp::new(doc, cfg, css));
    }

    let native_options = eframe::NativeOptions {
        // glow, not wgpu: egui_kittest (headless feature) also enables eframe's wgpu backend,
        // which eframe would then prefer, and wgpu draws transparent windows black on Windows.
        renderer: eframe::Renderer::Glow,
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

#[cfg(test)]
mod tests {
    use super::*;
    use chickenwm_core::config::DEFAULT_CHICKENWM_CONF;

    #[test]
    fn compute_line_diff_detects_modified_lines() {
        let a = "general {\n    gaps_in = 5\n    gaps_out = 10\n}\n";
        let b = "general {\n    gaps_in = 12\n    gaps_out = 10\n}\n";
        let diff = compute_line_diff(a, b);
        assert_eq!(
            diff,
            vec!["- L2:     gaps_in = 5", "+ L2:     gaps_in = 12"]
        );
    }

    #[test]
    fn inline_config_issues_map_to_control_labels_with_line_numbers() {
        let src = "general {\n    gaps_in = bad_num\n    border_size = thick\n}\n";
        let issues = chickenwm_core::config_check::validate(src);
        kit::set_config_issues(&issues);

        let gap_issues = kit::inline_issues_for_row("Gaps between windows");
        assert_eq!(gap_issues.len(), 1);
        assert_eq!(gap_issues[0].line, 2);

        let border_issues = kit::inline_issues_for_row("Border width");
        assert_eq!(border_issues.len(), 1);
        assert_eq!(border_issues[0].line, 3);
    }

    #[test]
    fn window_rules_builder_combines_multiple_matchers() {
        let m = pages::window_rules::build_combined_matcher(
            "Spotify",
            "",
            "Spotify.*",
            "spotify.exe",
        );
        assert_eq!(
            m,
            "class:^(Spotify)$ initialTitle:Spotify.* process:^(spotify.exe)$"
        );
    }

    #[test]
    fn headless_render_captures_settings_screenshot_from_parsed_config() {
        let doc = ConfigDocument::parse(DEFAULT_CHICKENWM_CONF).unwrap();
        let cfg = doc.compile();
        let css = Arc::new(RwLock::new(CssStyleSheet::default()));
        let out_path = std::env::temp_dir().join("chickenwm_settings_headless_test.png");
        let _ = std::fs::remove_file(&out_path);
        let req = chickenwm_core::headless::RenderRequest {
            out: out_path.clone(),
            size: egui::vec2(960.0, 680.0),
            pixels_per_point: 1.0,
            frames: 2,
        };
        chickenwm_core::headless::render_app(&req, |_cc| SettingsApp::new(doc, cfg, css))
            .expect("headless screenshot render should succeed");
        let meta = std::fs::metadata(&out_path).expect("screenshot file written");
        assert!(meta.len() > 1000);
        let _ = std::fs::remove_file(&out_path);
    }
}
