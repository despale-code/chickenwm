#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! `chickenwm-downloader`: ChickenWM Download Manager & BitTorrent Client.
//!
//! Features:
//! - Redirected browser downloads from `henbrowser` via `\\.\pipe\chickenwm-downloader-ipc` or CLI
//! - Separated tabs: **Downloads** (HTTP/HTTPS) and **Torrents** (BitTorrent `.torrent` & `magnet:`)
//! - Save location selector (default download folder + per-download save location dialog/editor)
//! - Pause & Resume Later (HTTP `Range: bytes=<offset>-` with `.part` files & JSON persistence; torrent piece bitfield persistence)
//! - Download Speed Limit (global & per-download token-bucket rate limiter in KB/s and MB/s)

pub mod engine;
pub mod torrent;

use anyhow::Result;
use chickenwm_core::bootstrap_config_and_themes;
use chickenwm_core::config::{config_dir, RgbaColor};
use chickenwm_core::css::{
    load_css_sheet, spawn_config_watcher, ConfigWatchEvent, CssStyleSheet,
};
use chickenwm_core::failsafe::spawn_emergency_failsafe_thread;
use chickenwm_core::init_tracing;
use chickenwm_core::ipc::{
    send_ipc_command_blocking, serve_ipc, IpcCommand, IpcResponse, PIPE_NAME_DOWNLOADER,
};
use clap::Parser;
use egui::{Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke, Vec2};
use engine::{
    detect_is_torrent, format_bytes, format_eta, format_speed, infer_filename_from_url,
    sanitize_filename, DownloadEngine, DownloadItem, DownloadKind, DownloadStatus,
};
use parking_lot::RwLock;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Parser, Debug, Default)]
#[command(
    name = "chickenwm-downloader",
    version,
    about = "ChickenWM Download Manager & BitTorrent Client"
)]
struct Cli {
    /// Add a download URL, magnet URI, or .torrent file path.
    #[arg(long)]
    add_url: Option<String>,

    /// Suggested filename for `--add-url`.
    #[arg(long)]
    filename: Option<String>,

    /// Suggested save directory for `--add-url`.
    #[arg(long)]
    save_dir: Option<String>,

    /// Treat the added URL as a BitTorrent / Magnet download.
    #[arg(long)]
    torrent: bool,

    /// Prompt for save location before starting the download.
    #[arg(long)]
    prompt_location: bool,

    /// Toggle or open the downloader flyout panel.
    #[arg(long)]
    toggle: bool,

    /// Render a single frame headless to the given PNG path and exit.
    #[arg(long)]
    render_to: Option<PathBuf>,
}

#[derive(Debug, Clone)]
enum IncomingCmd {
    Add(PendingAddRequest),
    TogglePanel,
}

#[derive(Debug, Clone)]
struct PendingAddRequest {
    url: String,
    filename: String,
    save_dir: String,
    is_torrent: bool,
    prompt_location: bool,
    cookies: Option<String>,
    referer: Option<String>,
    user_agent: Option<String>,
}

struct Palette {
    bg: Color32,
    card: Color32,
    sidebar: Color32,
    fg: Color32,
    sub: Color32,
    accent: Color32,
    accent_fg: Color32,
    border: Color32,
    danger: Color32,
    success: Color32,
}

impl Palette {
    fn from_css(sheet: &CssStyleSheet) -> Self {
        let col = |name: &str, r: u8, g: u8, b: u8, a: u8| {
            let c = sheet.var_color(name, RgbaColor { r, g, b, a });
            Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
        };
        let bg_raw = sheet.var_color(
            "--bg-color",
            RgbaColor {
                r: 24,
                g: 24,
                b: 28,
                a: 210,
            },
        );
        let bg_alpha = if bg_raw.a >= 245 { 210 } else { bg_raw.a };
        Self {
            bg: Color32::from_rgba_unmultiplied(bg_raw.r, bg_raw.g, bg_raw.b, bg_alpha),
            card: col("--surface-color", 34, 35, 40, 225),
            sidebar: col("--sidebar-bg", 28, 29, 34, 200),
            fg: col("--fg-color", 238, 238, 242, 255),
            sub: col("--subtext-color", 156, 158, 168, 255),
            accent: col("--accent-color", 94, 156, 246, 255),
            accent_fg: col("--accent-fg", 255, 255, 255, 255),
            border: col("--border-color", 255, 255, 255, 28),
            danger: Color32::from_rgb(232, 78, 78),
            success: Color32::from_rgb(72, 199, 126),
        }
    }
}

pub struct DownloaderApp {
    engine: Arc<DownloadEngine>,
    css: Arc<RwLock<CssStyleSheet>>,
    active_tab: DownloadKind,
    incoming_queue: Arc<RwLock<VecDeque<IncomingCmd>>>,
    /// Whether the slide-out downloader center panel is currently open
    panel_open: bool,
    panel_since: std::time::Instant,
    window_ready: bool,
    last_geometry: Option<(Pos2, Vec2, bool)>,
    /// Modal dialog state for adding a new download / choosing save location
    modal_open: bool,
    modal_url: String,
    modal_filename: String,
    modal_save_dir: String,
    modal_is_torrent: bool,
    modal_cookies: Option<String>,
    modal_referer: Option<String>,
    modal_user_agent: Option<String>,
    /// Inline folder editor for changing an existing item's save location
    editing_location_id: Option<u64>,
    editing_location_path: String,
    /// Custom global speed limit input text (KB/s)
    custom_limit_input: String,
    search_filter: String,
}

impl DownloaderApp {
    fn new(
        engine: Arc<DownloadEngine>,
        css: Arc<RwLock<CssStyleSheet>>,
        incoming_queue: Arc<RwLock<VecDeque<IncomingCmd>>>,
        start_open: bool,
    ) -> Self {
        let (default_dir, global_lim) = {
            let st = engine.state.read();
            (st.default_save_dir.clone(), st.global_speed_limit_kbps)
        };
        Self {
            engine,
            css,
            active_tab: DownloadKind::Http,
            incoming_queue,
            panel_open: start_open,
            panel_since: std::time::Instant::now(),
            window_ready: false,
            last_geometry: None,
            modal_open: false,
            modal_url: String::new(),
            modal_filename: String::new(),
            modal_save_dir: default_dir,
            modal_is_torrent: false,
            modal_cookies: None,
            modal_referer: None,
            modal_user_agent: None,
            editing_location_id: None,
            editing_location_path: String::new(),
            custom_limit_input: if global_lim > 0 {
                global_lim.to_string()
            } else {
                String::new()
            },
            search_filter: String::new(),
        }
    }

    fn process_incoming(&mut self, ctx: &egui::Context) {
        while let Some(cmd) = self.incoming_queue.write().pop_front() {
            match cmd {
                IncomingCmd::TogglePanel => {
                    self.panel_open = !self.panel_open;
                    self.panel_since = std::time::Instant::now();
                    if self.panel_open {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                    }
                }
                IncomingCmd::Add(req) => {
                    let is_torrent =
                        req.is_torrent || detect_is_torrent(&req.url, Some(&req.filename));
                    self.active_tab = if is_torrent {
                        DownloadKind::Torrent
                    } else {
                        DownloadKind::Http
                    };
                    self.panel_open = true;
                    self.panel_since = std::time::Instant::now();

                    let ask_location =
                        self.engine.state.read().ask_save_location || req.prompt_location;
                    if ask_location {
                        self.modal_open = true;
                        self.modal_url = req.url;
                        self.modal_filename = req.filename;
                        self.modal_save_dir = req.save_dir;
                        self.modal_is_torrent = is_torrent;
                        self.modal_cookies = req.cookies;
                        self.modal_referer = req.referer;
                        self.modal_user_agent = req.user_agent;
                    } else {
                        self.engine.add_download_with_context(
                            req.url,
                            Some(req.filename),
                            Some(req.save_dir),
                            is_torrent,
                            true,
                            req.cookies,
                            req.referer,
                            req.user_agent,
                        );
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
            }
        }
    }

    fn draw_top_header(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, is_max: bool) {
        let avail_w = ui.available_width().max(1.0);
        let (http_total, http_active, torrent_total, torrent_active) = {
            let st = self.engine.state.read();
            let ht = st.items.iter().filter(|i| i.kind == DownloadKind::Http).count();
            let ha = st
                .items
                .iter()
                .filter(|i| i.kind == DownloadKind::Http && i.status.is_active())
                .count();
            let tt = st
                .items
                .iter()
                .filter(|i| i.kind == DownloadKind::Torrent)
                .count();
            let ta = st
                .items
                .iter()
                .filter(|i| i.kind == DownloadKind::Torrent && i.status.is_active())
                .count();
            (ht, ha, tt, ta)
        };

        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(
                RichText::new("ChickenWM Downloader")
                    .font(FontId::proportional(16.0))
                    .strong()
                    .color(pal.fg),
            );
            ui.add_space(18.0);

            // Separated Tabs: 1. Downloads (HTTP/HTTPS) | 2. Torrents (BitTorrent / Magnet)
            let dl_label = if http_active > 0 {
                format!("⬇ Downloads ({http_active}/{http_total})")
            } else {
                format!("⬇ Downloads ({http_total})")
            };
            let tr_label = if torrent_active > 0 {
                format!("🧲 Torrents ({torrent_active}/{torrent_total})")
            } else {
                format!("🧲 Torrents ({torrent_total})")
            };

            if tab_pill(
                ui,
                &dl_label,
                self.active_tab == DownloadKind::Http,
                pal,
            )
            .clicked()
            {
                self.active_tab = DownloadKind::Http;
            }
            ui.add_space(4.0);
            if tab_pill(
                ui,
                &tr_label,
                self.active_tab == DownloadKind::Torrent,
                pal,
            )
            .clicked()
            {
                self.active_tab = DownloadKind::Torrent;
            }

            ui.add_space(12.0);
            let add_btn = egui::Button::new(
                RichText::new(if self.active_tab == DownloadKind::Torrent {
                    "+ Add Torrent / Magnet"
                } else {
                    "+ New Download"
                })
                .size(13.0)
                .color(pal.accent_fg),
            )
            .fill(pal.accent)
            .corner_radius(CornerRadius::same(7));
            if ui.add(add_btn).clicked() {
                self.modal_open = true;
                self.modal_url.clear();
                self.modal_filename.clear();
                self.modal_save_dir = self.engine.state.read().default_save_dir.clone();
                self.modal_is_torrent = self.active_tab == DownloadKind::Torrent;
                self.modal_cookies = None;
                self.modal_referer = None;
                self.modal_user_agent = None;
            }

            // Window controls on the right
            let right_reserve = 92.0;
            let rem = (avail_w - ui.cursor().min.x - right_reserve).max(0.0);
            if rem > 8.0 {
                let (_, drag_resp) = ui.allocate_exact_size(Vec2::new(rem, 32.0), Sense::click_and_drag());
                if drag_resp.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
                if drag_resp.double_clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!is_max));
                }
            }
        });
    }

    fn draw_toolbar_controls(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        egui::Frame::NONE
            .fill(pal.sidebar)
            .stroke(Stroke::new(1.0, pal.border))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(12, 10))
            .show(ui, |ui| {
                // Row 1: Save Location & Ask-every-time toggle
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Save Location:").size(13.0).color(pal.sub));
                    let mut def_dir = self.engine.state.read().default_save_dir.clone();
                    let dir_edit = egui::TextEdit::singleline(&mut def_dir)
                        .desired_width(240.0)
                        .font(FontId::proportional(13.0));
                    if ui.add(dir_edit).changed() {
                        self.engine.state.write().default_save_dir = def_dir.clone();
                        self.engine.save();
                    }
                    if small_action_btn(ui, "Browse…", pal).clicked() {
                        if let Some(chosen) = pick_folder_dialog(&def_dir) {
                            self.engine.state.write().default_save_dir = chosen;
                            self.engine.save();
                        }
                    }
                    if small_action_btn(ui, "Open Folder", pal).clicked() {
                        open_path_in_files(&def_dir);
                    }

                    ui.separator();
                    let mut ask_loc = self.engine.state.read().ask_save_location;
                    if ui
                        .checkbox(
                            &mut ask_loc,
                            RichText::new("Ask save location on redirect")
                                .size(12.5)
                                .color(pal.fg),
                        )
                        .changed()
                    {
                        self.engine.state.write().ask_save_location = ask_loc;
                        self.engine.save();
                    }
                });

                ui.add_space(6.0);

                // Row 2: Speed Limit controls + Batch Pause / Resume / Clear
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Speed Limit:").size(13.0).color(pal.sub));
                    let current_limit = self.engine.state.read().global_speed_limit_kbps;

                    let presets: &[(&str, u32)] = &[
                        ("Unlimited", 0),
                        ("500 KB/s", 500),
                        ("1 MB/s", 1024),
                        ("2 MB/s", 2048),
                        ("5 MB/s", 5120),
                        ("10 MB/s", 10240),
                    ];
                    for &(label, kbps) in presets {
                        let active = current_limit == kbps;
                        let btn = egui::Button::new(
                            RichText::new(label)
                                .size(12.0)
                                .color(if active { pal.accent_fg } else { pal.fg }),
                        )
                        .fill(if active { pal.accent } else { pal.card })
                        .stroke(Stroke::new(1.0, pal.border))
                        .corner_radius(CornerRadius::same(6));
                        if ui.add(btn).clicked() {
                            self.engine.set_global_speed_limit(kbps);
                            self.custom_limit_input = if kbps > 0 {
                                kbps.to_string()
                            } else {
                                String::new()
                            };
                        }
                    }

                    ui.label(RichText::new("Custom (KB/s):").size(12.0).color(pal.sub));
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.custom_limit_input)
                            .desired_width(64.0)
                            .hint_text("0 = max"),
                    );
                    if resp.changed() {
                        let parsed = self
                            .custom_limit_input
                            .trim()
                            .parse::<u32>()
                            .unwrap_or(0);
                        self.engine.set_global_speed_limit(parsed);
                    }

                    ui.separator();
                    if small_action_btn(ui, "⏸ Pause All", pal).clicked() {
                        self.engine.pause_all_in_tab(self.active_tab);
                    }
                    if small_action_btn(ui, "▶ Resume All", pal).clicked() {
                        self.engine.resume_all_in_tab(self.active_tab);
                    }
                    if small_action_btn(ui, "Clear Done", pal).clicked() {
                        self.engine.clear_completed(self.active_tab);
                    }
                });
            });
    }

    fn draw_items_list(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let items: Vec<DownloadItem> = self
            .engine
            .state
            .read()
            .items
            .iter()
            .filter(|i| i.kind == self.active_tab)
            .filter(|i| {
                if self.search_filter.trim().is_empty() {
                    true
                } else {
                    let q = self.search_filter.trim().to_ascii_lowercase();
                    i.filename.to_ascii_lowercase().contains(&q)
                        || i.url.to_ascii_lowercase().contains(&q)
                }
            })
            .cloned()
            .collect();

        if items.is_empty() {
            ui.add_space(48.0);
            ui.vertical_centered(|ui| {
                let empty_title = match self.active_tab {
                    DownloadKind::Http => "No HTTP/HTTPS downloads yet",
                    DownloadKind::Torrent => "No BitTorrent / Magnet downloads yet",
                };
                let empty_sub = match self.active_tab {
                    DownloadKind::Http => {
                        "Downloads started in Hen Browser are automatically redirected here, or click '+ New Download'."
                    }
                    DownloadKind::Torrent => {
                        "Click magnet: links or .torrent files in Hen Browser, or click '+ Add Torrent / Magnet'."
                    }
                };
                ui.label(RichText::new(empty_title).size(16.0).strong().color(pal.fg));
                ui.add_space(6.0);
                ui.label(RichText::new(empty_sub).size(13.0).color(pal.sub));
            });
            return;
        }

        egui::ScrollArea::vertical()
            .id_salt(("dl_scroll", self.active_tab as u8))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for item in items {
                    self.draw_single_item_card(ui, &item, pal);
                    ui.add_space(8.0);
                }
            });
    }

    fn draw_single_item_card(&mut self, ui: &mut egui::Ui, item: &DownloadItem, pal: &Palette) {
        egui::Frame::NONE
            .fill(pal.card)
            .stroke(Stroke::new(1.0, pal.border))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let icon_badge = match item.kind {
                        DownloadKind::Http => "⬇",
                        DownloadKind::Torrent => "🧲",
                    };
                    ui.label(RichText::new(icon_badge).size(16.0).color(pal.accent));
                    ui.label(
                        RichText::new(&item.filename)
                            .size(14.5)
                            .strong()
                            .color(pal.fg),
                    );

                    // Status badge
                    let (status_text, status_col) = match &item.status {
                        DownloadStatus::Queued => ("Queued", pal.sub),
                        DownloadStatus::Downloading => ("Downloading", pal.accent),
                        DownloadStatus::Paused => ("Paused (Resume Later)", Color32::from_rgb(235, 175, 55)),
                        DownloadStatus::Completed => ("Completed", pal.success),
                        DownloadStatus::Failed(err) => (err.as_str(), pal.danger),
                    };
                    ui.add_space(8.0);
                    status_chip(ui, status_text, status_col);
                });

                ui.add_space(6.0);

                // Progress bar
                let frac = item.progress_fraction();
                let bar_w = ui.available_width().max(20.0);
                let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(bar_w, 8.0), Sense::hover());
                ui.painter().rect_filled(
                    bar_rect,
                    CornerRadius::same(4),
                    pal.border.gamma_multiply(0.8),
                );
                if frac > 0.0 {
                    let fill_w = (bar_rect.width() * frac).max(4.0);
                    let fill_rect = Rect::from_min_size(bar_rect.min, Vec2::new(fill_w, bar_rect.height()));
                    let bar_col = if item.status == DownloadStatus::Completed {
                        pal.success
                    } else if matches!(item.status, DownloadStatus::Paused) {
                        Color32::from_rgb(235, 175, 55)
                    } else {
                        pal.accent
                    };
                    ui.painter()
                        .rect_filled(fill_rect, CornerRadius::same(4), bar_col);
                }

                ui.add_space(6.0);

                // Metrics & Save Location info
                ui.horizontal_wrapped(|ui| {
                    let size_str = if item.total_bytes > 0 {
                        format!(
                            "{} / {} ({:.1}%)",
                            format_bytes(item.received_bytes),
                            format_bytes(item.total_bytes),
                            frac * 100.0
                        )
                    } else {
                        format!("{} downloaded", format_bytes(item.received_bytes))
                    };
                    ui.label(RichText::new(size_str).size(12.0).color(pal.sub));

                    if item.status == DownloadStatus::Downloading {
                        ui.label(RichText::new("•").size(12.0).color(pal.sub));
                        ui.label(
                            RichText::new(format_speed(item.speed_bps))
                                .size(12.0)
                                .strong()
                                .color(pal.accent),
                        );
                        if let Some(eta) = item.eta_seconds() {
                            ui.label(
                                RichText::new(format!("ETA {}", format_eta(eta)))
                                    .size(12.0)
                                    .color(pal.sub),
                            );
                        }
                    }

                    if item.kind == DownloadKind::Torrent {
                        ui.label(RichText::new("•").size(12.0).color(pal.sub));
                        ui.label(
                            RichText::new(format!(
                                "Seeders: {}  Leechers: {}  Peers: {}",
                                item.seeders, item.leechers, item.peers_connected
                            ))
                            .size(12.0)
                            .color(pal.sub),
                        );
                    }

                    ui.label(RichText::new("•").size(12.0).color(pal.sub));
                    ui.label(
                        RichText::new(format!("Save to: {}", item.save_dir))
                            .size(12.0)
                            .color(pal.sub),
                    );
                });

                ui.add_space(6.0);

                // Action buttons row
                ui.horizontal_wrapped(|ui| {
                    if item.status.can_pause() {
                        if small_action_btn(ui, "⏸ Pause", pal).clicked() {
                            self.engine.pause(item.id);
                        }
                    }
                    if item.status.can_resume() {
                        let resume_lbl = if item.received_bytes > 0 {
                            "▶ Resume"
                        } else {
                            "▶ Start"
                        };
                        if small_action_btn(ui, resume_lbl, pal).clicked() {
                            self.engine.start_or_resume(item.id);
                        }
                    }
                    if item.status == DownloadStatus::Completed {
                        if small_action_btn(ui, "Open File", pal).clicked() {
                            let target = item.target_path();
                            chickenwm_core::elevation::shell_open_unelevated(&target, None, None);
                        }
                    }
                    if small_action_btn(ui, "Open Folder", pal).clicked() {
                        open_path_in_files(&item.save_dir);
                    }
                    if small_action_btn(ui, "Save Location…", pal).clicked() {
                        if let Some(picked) = pick_folder_dialog(&item.save_dir) {
                            self.engine.change_save_location(item.id, picked);
                        } else {
                            self.editing_location_id = Some(item.id);
                            self.editing_location_path = item.save_dir.clone();
                        }
                    }

                    // Per-item speed limit selector
                    ui.separator();
                    ui.label(RichText::new("Item Limit:").size(11.5).color(pal.sub));
                    for &(lbl, kbps) in &[("Global", 0u32), ("500K", 500), ("1M", 1024), ("5M", 5120)] {
                        let active = item.speed_limit_kbps == kbps;
                        let b = egui::Button::new(
                            RichText::new(lbl)
                                .size(11.0)
                                .color(if active { pal.accent_fg } else { pal.sub }),
                        )
                        .fill(if active { pal.accent } else { pal.sidebar })
                        .corner_radius(CornerRadius::same(5));
                        if ui.add(b).clicked() {
                            self.engine.set_item_speed_limit(item.id, kbps);
                        }
                    }

                    ui.separator();
                    let del_btn = egui::Button::new(
                        RichText::new("✕ Remove")
                            .size(11.5)
                            .color(pal.danger),
                    )
                    .fill(pal.sidebar)
                    .stroke(Stroke::new(1.0, pal.border))
                    .corner_radius(CornerRadius::same(6));
                    if ui.add(del_btn).clicked() {
                        let shift = ui.input(|i| i.modifiers.shift);
                        self.engine.remove_item(item.id, shift);
                    }
                });

                if self.editing_location_id == Some(item.id) {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("New Save Folder:").size(12.0).color(pal.sub));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.editing_location_path)
                                .desired_width(260.0),
                        );
                        if small_action_btn(ui, "Apply", pal).clicked() {
                            let new_dir = self.editing_location_path.trim().to_string();
                            if !new_dir.is_empty() {
                                self.engine.change_save_location(item.id, new_dir);
                            }
                            self.editing_location_id = None;
                        }
                        if small_action_btn(ui, "Cancel", pal).clicked() {
                            self.editing_location_id = None;
                        }
                    });
                }
            });
    }

    fn draw_add_modal(&mut self, ctx: &egui::Context, pal: &Palette) {
        if !self.modal_open {
            return;
        }
        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("dl_modal_backdrop"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click());
                ui.painter()
                    .rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(140));
            });

        let modal_w = (screen.width() - 40.0).clamp(240.0, 540.0);
        egui::Window::new("New Download — Save Location & Options")
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .order(egui::Order::Foreground)
            .fixed_size(Vec2::new(modal_w, 250.0))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(
                egui::Frame::NONE
                    .fill(pal.card)
                    .stroke(Stroke::new(1.5, pal.accent))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(18, 16)),
            )
            .show(ctx, |ui| {
                ui.label(
                    RichText::new(if self.modal_is_torrent {
                        "🧲 Add BitTorrent / Magnet Download"
                    } else {
                        "⬇ Add HTTP / HTTPS Download"
                    })
                    .size(16.0)
                    .strong()
                    .color(pal.fg),
                );
                ui.add_space(10.0);

                ui.label(
                    RichText::new("URL, Magnet Link (`magnet:?xt=urn:btih:...`), or `.torrent` Path:")
                        .size(12.5)
                        .color(pal.sub),
                );
                let url_resp = ui.add(
                    egui::TextEdit::singleline(&mut self.modal_url)
                        .desired_width(f32::INFINITY)
                        .hint_text("https://... or magnet:?xt=urn:btih:..."),
                );
                if url_resp.changed() {
                    self.modal_is_torrent =
                        detect_is_torrent(&self.modal_url, Some(&self.modal_filename));
                    if self.modal_filename.trim().is_empty() {
                        self.modal_filename =
                            infer_filename_from_url(&self.modal_url, self.modal_is_torrent);
                    }
                }

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("File Name:").size(12.5).color(pal.sub));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.modal_filename)
                            .desired_width((modal_w - 110.0).max(80.0))
                            .hint_text("Auto-detected from URL"),
                    );
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Save Location:").size(12.5).color(pal.sub));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.modal_save_dir)
                            .desired_width((modal_w - 200.0).max(80.0)),
                    );
                    if small_action_btn(ui, "Browse…", pal).clicked() {
                        if let Some(folder) = pick_folder_dialog(&self.modal_save_dir) {
                            self.modal_save_dir = folder;
                        }
                    }
                });

                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let start_btn = egui::Button::new(
                        RichText::new("▶ Start Download Now")
                            .size(13.0)
                            .strong()
                            .color(pal.accent_fg),
                    )
                    .fill(pal.accent)
                    .corner_radius(CornerRadius::same(7));
                    if ui.add(start_btn).clicked() && !self.modal_url.trim().is_empty() {
                        let is_tor = self.modal_is_torrent
                            || detect_is_torrent(&self.modal_url, Some(&self.modal_filename));
                        self.active_tab = if is_tor {
                            DownloadKind::Torrent
                        } else {
                            DownloadKind::Http
                        };
                        self.engine.add_download_with_context(
                            self.modal_url.trim().to_string(),
                            Some(sanitize_filename(&self.modal_filename)),
                            Some(self.modal_save_dir.trim().to_string()),
                            is_tor,
                            true,
                            self.modal_cookies.take(),
                            self.modal_referer.take(),
                            self.modal_user_agent.take(),
                        );
                        self.modal_open = false;
                    }

                    if small_action_btn(ui, "⏸ Add Paused (Resume Later)", pal).clicked()
                        && !self.modal_url.trim().is_empty()
                    {
                        let is_tor = self.modal_is_torrent
                            || detect_is_torrent(&self.modal_url, Some(&self.modal_filename));
                        self.active_tab = if is_tor {
                            DownloadKind::Torrent
                        } else {
                            DownloadKind::Http
                        };
                        self.engine.add_download_with_context(
                            self.modal_url.trim().to_string(),
                            Some(sanitize_filename(&self.modal_filename)),
                            Some(self.modal_save_dir.trim().to_string()),
                            is_tor,
                            false,
                            self.modal_cookies.take(),
                            self.modal_referer.take(),
                            self.modal_user_agent.take(),
                        );
                        self.modal_open = false;
                    }

                    if small_action_btn(ui, "Cancel", pal).clicked() {
                        self.modal_open = false;
                    }
                });
            });
    }
}

impl eframe::App for DownloaderApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        if !self.window_ready {
            self.window_ready = prepare_service_window(frame);
        }
        self.process_incoming(ctx);

        if self.panel_open && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.modal_open {
                self.modal_open = false;
            } else {
                self.panel_open = false;
            }
        }

        let any_active = self
            .engine
            .state
            .read()
            .items
            .iter()
            .any(|i| i.status.is_active());
        if any_active && self.panel_open {
            ctx.request_repaint_after(Duration::from_millis(120));
        }

        // Compute slide-out panel geometry (anchored below roobar at top-right of primary work area, like notifications)
        let ppp = ctx.pixels_per_point().max(1.0);
        let work_px = primary_work_area();
        let work = [
            work_px[0] / ppp,
            work_px[1] / ppp,
            work_px[2] / ppp,
            work_px[3] / ppp,
        ];
        let panel_w = 780.0_f32.min((work[2] - 24.0).max(480.0));
        let panel_h = 560.0_f32.min((work[3] - 24.0).max(360.0));
        let (pos, size, passthrough) = if self.panel_open {
            let x = (work[0] + work[2] - panel_w - 12.0).round();
            let y = (work[1] + 10.0).round();
            (Pos2::new(x, y), Vec2::new(panel_w, panel_h), false)
        } else {
            (
                Pos2::new((work[0] + work[2] - 2.0).round(), work[1].round()),
                Vec2::new(1.0, 1.0),
                true,
            )
        };
        let geometry = (pos, size, passthrough);
        if self.last_geometry != Some(geometry) {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(geometry.0));
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(geometry.1));
            ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(geometry.2));
            self.last_geometry = Some(geometry);
        }

        if !self.panel_open {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |_ui| {});
            return;
        }

        let sheet = self.css.read().clone();
        let pal = Palette::from_css(&sheet);
        let screen = ctx.screen_rect();
        let is_max = false;

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(pal.bg)
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(14, 10)),
            )
            .show(ctx, |ui| {
                self.draw_top_header(ui, &pal, ctx, is_max);
                ui.add_space(8.0);
                self.draw_toolbar_controls(ui, &pal);
                ui.add_space(10.0);
                self.draw_items_list(ui, &pal);
            });

        self.draw_add_modal(ctx, &pal);

        if draw_panel_close_button(ctx, screen, &pal) {
            self.panel_open = false;
        }

        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("downloader_border"),
        ));
        chickenwm_core::window_frame::paint_window_gradient_border(&painter, screen, 12.0);
    }
}

fn tab_pill(ui: &mut egui::Ui, label: &str, active: bool, pal: &Palette) -> egui::Response {
    let btn = egui::Button::new(
        RichText::new(label)
            .size(13.5)
            .strong()
            .color(if active { pal.accent_fg } else { pal.fg }),
    )
    .fill(if active { pal.accent } else { pal.card })
    .stroke(Stroke::new(1.0, pal.border))
    .corner_radius(CornerRadius::same(8));
    ui.add(btn)
}

fn small_action_btn(ui: &mut egui::Ui, label: &str, pal: &Palette) -> egui::Response {
    let btn = egui::Button::new(RichText::new(label).size(12.0).color(pal.fg))
        .fill(pal.card)
        .stroke(Stroke::new(1.0, pal.border))
        .corner_radius(CornerRadius::same(6));
    ui.add(btn)
}

fn status_chip(ui: &mut egui::Ui, text: &str, color: Color32) {
    egui::Frame::NONE
        .fill(color.gamma_multiply(0.18))
        .stroke(Stroke::new(1.0, color.gamma_multiply(0.6)))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(text).size(11.5).strong().color(color));
        });
}

fn draw_panel_close_button(ctx: &egui::Context, screen: Rect, pal: &Palette) -> bool {
    let mut close_clicked = false;
    let x = (screen.right() - 38.0).max(screen.left() + 4.0);
    egui::Area::new(egui::Id::new("downloader_win_controls"))
        .order(egui::Order::Foreground)
        .fixed_pos(Pos2::new(x, screen.top() + 10.0))
        .show(ctx, |ui| {
            let (close_r, close_resp) =
                ui.allocate_exact_size(Vec2::new(24.0, 22.0), Sense::click());
            if close_resp.hovered() {
                ui.painter()
                    .rect_filled(close_r, CornerRadius::same(4), pal.danger);
            }
            let cc = close_r.center();
            let col = if close_resp.hovered() {
                Color32::WHITE
            } else {
                pal.fg
            };
            ui.painter().line_segment(
                [cc + Vec2::new(-4.5, -4.5), cc + Vec2::new(4.5, 4.5)],
                Stroke::new(1.4, col),
            );
            ui.painter().line_segment(
                [cc + Vec2::new(4.5, -4.5), cc + Vec2::new(-4.5, 4.5)],
                Stroke::new(1.4, col),
            );
            if close_resp.on_hover_text("Hide download manager (Esc)").clicked() {
                close_clicked = true;
            }
        });
    close_clicked
}

fn primary_work_area() -> [f32; 4] {
    for m in chickenwm_core::monitor::enumerate_monitors() {
        if m.is_primary {
            let [x, y, w, h] = m.work_rect;
            return [x as f32, y as f32, w as f32, h as f32];
        }
    }
    [0.0, 0.0, 1920.0, 1080.0]
}

fn prepare_service_window(frame: &eframe::Frame) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::{BOOL, HWND};
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_TOOLWINDOW,
    };
    let Ok(handle) = frame.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return false;
    };
    let hwnd = HWND(h.hwnd.get() as _);
    chickenwm_core::sys::suppress_nc_paint(hwnd);
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex | (WS_EX_TOOLWINDOW.0 as isize));
        let off = BOOL(1);
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED,
            &off as *const _ as *const core::ffi::c_void,
            4,
        );
    }
    true
}

fn open_path_in_files(dir: &str) {
    let mut files_exe = std::env::current_exe().unwrap_or_default();
    files_exe.set_file_name("chickenwm-files.exe");
    if files_exe.exists() {
        chickenwm_core::elevation::shell_open_unelevated(&files_exe, Some(dir), None);
    } else {
        chickenwm_core::elevation::shell_open_unelevated(Path::new(dir), None, None);
    }
}

/// Opens a native Windows folder selection dialog and returns the chosen directory path.
fn pick_folder_dialog(initial_dir: &str) -> Option<String> {
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let script = format!(
        "Add-Type -AssemblyName System.Windows.Forms; $f = New-Object System.Windows.Forms.FolderBrowserDialog; $f.SelectedPath = '{}'; if ($f.ShowDialog() -eq 'OK') {{ Write-Output $f.SelectedPath }}",
        initial_dir.replace('\'', "''")
    );
    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args(["-NoProfile", "-STA", "-Command", &script]);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd.output().ok()?;
    let chosen = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!chosen.is_empty()).then_some(chosen)
}

fn generate_downloader_icon() -> egui::IconData {
    let mut rgba = vec![0u8; 64 * 64 * 4];
    for y in 0..64 {
        for x in 0..64 {
            let idx = (y * 64 + x) * 4;
            let cx = x as f32 - 31.5;
            let cy = y as f32 - 31.5;
            let dist = (cx * cx + cy * cy).sqrt();
            if dist <= 28.0 {
                rgba[idx] = 36;
                rgba[idx + 1] = 38;
                rgba[idx + 2] = 46;
                rgba[idx + 3] = 255;

                // Download arrow shaft & head
                let in_shaft = (x >= 28 && x <= 35) && (y >= 14 && y <= 36);
                let in_head = (y >= 32 && y <= 44) && ((x as i32 - 32).abs() <= (44 - y as i32));
                let in_tray = (y >= 47 && y <= 51) && (x >= 18 && x <= 45);
                if in_shaft || in_head || in_tray {
                    rgba[idx] = 94;
                    rgba[idx + 1] = 176;
                    rgba[idx + 2] = 252;
                }
            }
        }
    }
    egui::IconData {
        rgba,
        width: 64,
        height: 64,
    }
}

fn main() -> Result<()> {
    init_tracing("chickenwm_downloader");
    let _failsafe = spawn_emergency_failsafe_thread();
    let cli = Cli::parse();

    let state_path = config_dir().join("downloader.json");
    let engine = DownloadEngine::load_or_new(state_path);
    let incoming_queue: Arc<RwLock<VecDeque<IncomingCmd>>> =
        Arc::new(RwLock::new(VecDeque::new()));

    // If `--add-url` was passed on CLI and another instance is already running, forward via IPC
    if let Some(ref url) = cli.add_url {
        let is_tor = cli.torrent || detect_is_torrent(url, cli.filename.as_deref());
        let cmd = IpcCommand::AddDownload {
            url: url.clone(),
            filename: cli.filename.clone(),
            save_dir: cli.save_dir.clone(),
            is_torrent: is_tor,
            prompt_location: cli.prompt_location,
            cookies: None,
            referer: None,
            user_agent: None,
        };
        if send_ipc_command_blocking(PIPE_NAME_DOWNLOADER, &cmd).is_ok() {
            return Ok(());
        }
        // Otherwise queue it for this newly launched instance
        let def_dir = engine.state.read().default_save_dir.clone();
        incoming_queue.write().push_back(IncomingCmd::Add(PendingAddRequest {
            url: url.clone(),
            filename: cli
                .filename
                .clone()
                .unwrap_or_else(|| infer_filename_from_url(url, is_tor)),
            save_dir: cli.save_dir.clone().unwrap_or(def_dir),
            is_torrent: is_tor,
            prompt_location: cli.prompt_location,
            cookies: None,
            referer: None,
            user_agent: None,
        }));
    } else if cli.render_to.is_none() {
        // Check if the background service is already running; if so, toggle the panel and exit
        if cli.toggle {
            if send_ipc_command_blocking(PIPE_NAME_DOWNLOADER, &IpcCommand::ToggleDownloaderCenter)
                .is_ok()
            {
                return Ok(());
            }
        } else if send_ipc_command_blocking(
            PIPE_NAME_DOWNLOADER,
            &IpcCommand::QueryDownloaderStatus,
        )
        .is_ok()
        {
            // Another instance is already running as the background service; toggle its panel
            let _ = send_ipc_command_blocking(
                PIPE_NAME_DOWNLOADER,
                &IpcCommand::ToggleDownloaderCenter,
            );
            return Ok(());
        }
    }

    let (_doc, _cfg) = bootstrap_config_and_themes()?;
    let css = Arc::new(RwLock::new(
        load_css_sheet("downloader.css")
            .or_else(|_| load_css_sheet("files.css"))
            .unwrap_or_default(),
    ));
    let css_watcher_ref = Arc::clone(&css);
    let _watcher = spawn_config_watcher(move |ev| {
        if let ConfigWatchEvent::CssChanged { file_name, .. } = ev {
            if file_name.eq_ignore_ascii_case("downloader.css")
                || file_name.eq_ignore_ascii_case("files.css")
            {
                if let Ok(new_sheet) =
                    load_css_sheet("downloader.css").or_else(|_| load_css_sheet("files.css"))
                {
                    *css_watcher_ref.write() = new_sheet;
                }
            }
        }
    });

    if let Some(req) = chickenwm_core::headless::RenderRequest::from_args([920.0, 620.0]) {
        return chickenwm_core::headless::render_app(&req, |_cc| {
            DownloaderApp::new(engine, css, incoming_queue, true)
        });
    }

    // Spawn IPC server on `\\.\pipe\chickenwm-downloader-ipc`
    let q_ipc = Arc::clone(&incoming_queue);
    let eng_ipc = Arc::clone(&engine);
    let egui_ctx_slot: Arc<std::sync::OnceLock<egui::Context>> =
        Arc::new(std::sync::OnceLock::new());
    let ctx_ipc = Arc::clone(&egui_ctx_slot);
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let _ = serve_ipc(PIPE_NAME_DOWNLOADER, move |cmd| {
                let q = Arc::clone(&q_ipc);
                let eng = Arc::clone(&eng_ipc);
                let ctx_slot = Arc::clone(&ctx_ipc);
                async move {
                    match cmd {
                        IpcCommand::AddDownload {
                            url,
                            filename,
                            save_dir,
                            is_torrent,
                            prompt_location,
                            cookies,
                            referer,
                            user_agent,
                        } => {
                            let is_tor =
                                is_torrent || detect_is_torrent(&url, filename.as_deref());
                            let def_dir = eng.state.read().default_save_dir.clone();
                            q.write().push_back(IncomingCmd::Add(PendingAddRequest {
                                filename: filename
                                    .unwrap_or_else(|| infer_filename_from_url(&url, is_tor)),
                                save_dir: save_dir.unwrap_or(def_dir),
                                url,
                                is_torrent: is_tor,
                                prompt_location,
                                cookies,
                                referer,
                                user_agent,
                            }));
                            if let Some(ctx) = ctx_slot.get() {
                                ctx.request_repaint();
                            }
                            IpcResponse::Ok {
                                message: "Download queued in ChickenWM Downloader".to_string(),
                            }
                        }
                        IpcCommand::ToggleDownloaderCenter => {
                            q.write().push_back(IncomingCmd::TogglePanel);
                            if let Some(ctx) = ctx_slot.get() {
                                ctx.request_repaint();
                            }
                            IpcResponse::Ok {
                                message: "ok".to_string(),
                            }
                        }
                        IpcCommand::QueryDownloaderStatus => {
                            let st = eng.state.read();
                            let total_count = st.items.len() as u32;
                            let active_items: Vec<&DownloadItem> =
                                st.items.iter().filter(|i| i.status.is_active()).collect();
                            let active_count = active_items.len() as u32;
                            let speed_bps: u64 = active_items.iter().map(|i| i.speed_bps).sum();
                            let progress_pct = if active_items.is_empty() {
                                0
                            } else {
                                let sum_frac: f32 =
                                    active_items.iter().map(|i| i.progress_fraction()).sum();
                                ((sum_frac / active_items.len() as f32) * 100.0)
                                    .round()
                                    .clamp(0.0, 100.0) as u8
                            };
                            IpcResponse::DownloaderStatus {
                                active_count,
                                total_count,
                                speed_bps,
                                progress_pct,
                            }
                        }
                        _ => IpcResponse::Ok {
                            message: "OK".to_string(),
                        },
                    }
                }
            })
            .await;
        });
    });

    let start_open = cli.toggle || cli.add_url.is_some();
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_icon(generate_downloader_icon())
            .with_title("ChickenWM Downloader")
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_taskbar(false)
            .with_inner_size([1.0, 1.0])
            .with_position([0.0, 0.0])
            .with_mouse_passthrough(true),
        ..Default::default()
    };

    eframe::run_native(
        "ChickenWM Downloader",
        native_options,
        Box::new(move |cc| {
            let _ = egui_ctx_slot.set(cc.egui_ctx.clone());
            Ok(Box::new(DownloaderApp::new(
                engine,
                css,
                incoming_queue,
                start_open,
            )))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_render_Verify_downloads_and_torrents_tabs() {
        let tmp_state = std::env::temp_dir().join("chickenwm_downloader_ui_test.json");
        let _ = std::fs::remove_file(&tmp_state);
        let engine = DownloadEngine::load_or_new(tmp_state.clone());
        engine.add_download(
            "https://releases.ubuntu.com/24.04/ubuntu-24.04-desktop-amd64.iso".to_string(),
            Some("ubuntu-24.04-desktop-amd64.iso".to_string()),
            Some("C:\\Users\\Public\\Downloads".to_string()),
            false,
            false,
        );
        engine.add_download(
            "magnet:?xt=urn:btih:a9993e364706816aba3e25717850c26c9cd0d89d&dn=ArchLinux-x86_64.iso"
                .to_string(),
            Some("ArchLinux-x86_64.iso".to_string()),
            Some("C:\\Users\\Public\\Downloads".to_string()),
            true,
            false,
        );

        let out_path = std::env::temp_dir().join("chickenwm_downloader_headless_test.png");
        let req = chickenwm_core::headless::RenderRequest {
            out: out_path.clone(),
            size: egui::vec2(920.0, 620.0),
            pixels_per_point: 1.0,
            frames: 2,
        };
        let css = Arc::new(RwLock::new(CssStyleSheet::default()));
        let q = Arc::new(RwLock::new(VecDeque::new()));
        chickenwm_core::headless::render_app(&req, |_cc| DownloaderApp::new(engine, css, q, true))
            .expect("headless render of chickenwm-downloader should succeed");
        let meta = std::fs::metadata(&out_path).expect("screenshot written");
        assert!(meta.len() > 1000);
        let _ = std::fs::remove_file(&out_path);
        let _ = std::fs::remove_file(&tmp_state);
    }
}
