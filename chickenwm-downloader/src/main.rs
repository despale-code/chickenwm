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
use chickenwm_core::ui_icons::{
    paint_circle_check, paint_close, paint_download, paint_file, paint_folder, paint_gear,
    paint_magnet, paint_media_pause, paint_media_play, paint_status_dot, paint_trash,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloaderTab {
    Downloads,
    Torrents,
    Settings,
}

#[allow(dead_code)]
struct Palette {
    bg: Color32,
    card: Color32,
    sidebar: Color32,
    fg: Color32,
    sub: Color32,
    subtext: Color32,
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
                a: 225,
            },
        );
        let bg_alpha = if bg_raw.a >= 245 { 225 } else { bg_raw.a };
        let sub = col("--subtext-color", 156, 158, 168, 255);
        Self {
            bg: Color32::from_rgba_unmultiplied(bg_raw.r, bg_raw.g, bg_raw.b, bg_alpha),
            card: col("--surface-color", 34, 35, 40, 225),
            sidebar: col("--sidebar-bg", 28, 29, 34, 200),
            fg: col("--fg-color", 238, 238, 242, 255),
            sub,
            subtext: sub,
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
    active_tab: DownloaderTab,
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
    custom_upload_limit_input: String,
    upload_speed_limit_kbps: u32,
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
            active_tab: DownloaderTab::Downloads,
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
            custom_upload_limit_input: String::new(),
            upload_speed_limit_kbps: 0,
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
                        DownloaderTab::Torrents
                    } else {
                        DownloaderTab::Downloads
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

    fn draw_top_header(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            ui.add_space(2.0_f32);
            let _title_lbl = ui.label(
                RichText::new("Downloads")
                    .font(FontId::proportional(17.0_f32))
                    .strong()
                    .color(pal.fg),
            );

            // Drag support on title area
            let avail_w = ui.available_width().max(1.0_f32);
            let right_reserve = 140.0_f32;
            let drag_w = (avail_w - right_reserve).max(0.0_f32);
            if drag_w > 8.0_f32 {
                let (_, drag_resp) = ui.allocate_exact_size(Vec2::new(drag_w, 24.0_f32), Sense::click_and_drag());
                if drag_resp.drag_started() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Vector close button on top right
                let close_size = Vec2::splat(22.0_f32);
                let (close_rect, close_resp) = ui.allocate_exact_size(close_size, Sense::click());
                let hover = close_resp.hovered();
                if hover {
                    ui.painter().rect_filled(
                        close_rect,
                        CornerRadius::same(6),
                        Color32::from_white_alpha(24),
                    );
                }
                let close_icon_rect =
                    Rect::from_center_size(close_rect.center(), Vec2::splat(11.0_f32));
                paint_close(
                    ui.painter(),
                    close_icon_rect,
                    if hover { pal.fg } else { pal.sub },
                );
                if close_resp.on_hover_text("Close (Esc)").clicked() {
                    self.panel_open = false;
                }

                ui.add_space(4.0_f32);

                // + Add button
                let add_btn = egui::Button::new(
                    RichText::new(if self.active_tab == DownloaderTab::Torrents {
                        "+ Add Torrent"
                    } else {
                        "+ Add Download"
                    })
                    .size(11.5_f32)
                    .color(pal.accent_fg),
                )
                .fill(pal.accent)
                .corner_radius(CornerRadius::same(11));
                if ui.add(add_btn).clicked() {
                    self.modal_open = true;
                    self.modal_url.clear();
                    self.modal_filename.clear();
                    self.modal_save_dir = self.engine.state.read().default_save_dir.clone();
                    self.modal_is_torrent = self.active_tab == DownloaderTab::Torrents;
                    self.modal_cookies = None;
                    self.modal_referer = None;
                    self.modal_user_agent = None;
                }
            });
        });
    }

    fn draw_segmented_tabs(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let tabs = [
            (DownloaderTab::Downloads, "Downloads"),
            (DownloaderTab::Torrents, "Torrents"),
            (DownloaderTab::Settings, "Settings"),
        ];
        let avail_w = ui.available_width().max(100.0_f32);
        let height = 28.0_f32;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(avail_w, height), Sense::hover());
        let painter = ui.painter();

        // Capsule well background
        let well_bg = Color32::from_white_alpha(10);
        painter.rect(
            rect,
            CornerRadius::same(8),
            well_bg,
            Stroke::new(1.0_f32, pal.border),
            egui::StrokeKind::Inside,
        );

        let seg_w = (rect.width() - 4.0_f32) / 3.0_f32;
        let selected_idx = match self.active_tab {
            DownloaderTab::Downloads => 0,
            DownloaderTab::Torrents => 1,
            DownloaderTab::Settings => 2,
        };

        let target_x = rect.left() + 2.0_f32 + (selected_idx as f32) * seg_w;
        let id = ui.id().with("downloader_tab_slide");
        let px = ui.ctx().animate_value_with_time(id, target_x, 0.12_f32);
        let pill_rect = Rect::from_min_size(
            Pos2::new(px, rect.top() + 2.0_f32),
            Vec2::new(seg_w, height - 4.0_f32),
        );
        // Smooth capsule pill fill
        painter.rect_filled(
            pill_rect,
            CornerRadius::same(6),
            pal.card.lerp_to_gamma(pal.fg, 0.12_f32),
        );
        painter.rect_stroke(
            pill_rect,
            CornerRadius::same(6),
            Stroke::new(1.0_f32, Color32::from_white_alpha(25)),
            egui::StrokeKind::Inside,
        );

        for (i, (tab, label)) in tabs.iter().enumerate() {
            let seg_rect = Rect::from_min_size(
                Pos2::new(rect.left() + 2.0_f32 + (i as f32) * seg_w, rect.top()),
                Vec2::new(seg_w, height),
            );
            let resp = ui.interact(seg_rect, ui.id().with(("tab_click", i)), Sense::click());
            if resp.clicked() {
                self.active_tab = *tab;
            }

            let is_selected = self.active_tab == *tab;
            let text_col = if is_selected { pal.fg } else { pal.sub };
            let icon_col = if is_selected { pal.accent } else { pal.sub };

            let display_label = match tab {
                DownloaderTab::Downloads => {
                    let st = self.engine.state.read();
                    let active = st
                        .items
                        .iter()
                        .filter(|it| it.kind == DownloadKind::Http && it.status.is_active())
                        .count();
                    if active > 0 {
                        format!("{label} ({active})")
                    } else {
                        label.to_string()
                    }
                }
                DownloaderTab::Torrents => {
                    let st = self.engine.state.read();
                    let active = st
                        .items
                        .iter()
                        .filter(|it| it.kind == DownloadKind::Torrent && it.status.is_active())
                        .count();
                    if active > 0 {
                        format!("{label} ({active})")
                    } else {
                        label.to_string()
                    }
                }
                DownloaderTab::Settings => label.to_string(),
            };

            let font = FontId::proportional(12.5_f32);
            let galley = ui.painter().layout_no_wrap(display_label, font, text_col);
            let icon_size = 14.0_f32;
            let spacing = 5.0_f32;
            let total_content_w = icon_size + spacing + galley.size().x;
            let content_start_x = seg_rect.center().x - total_content_w * 0.5_f32;

            let icon_rect = Rect::from_center_size(
                Pos2::new(content_start_x + icon_size * 0.5_f32, seg_rect.center().y),
                Vec2::splat(icon_size),
            );

            match tab {
                DownloaderTab::Downloads => {
                    paint_download(painter, icon_rect, icon_col, false);
                }
                DownloaderTab::Torrents => {
                    paint_magnet(painter, icon_rect, icon_col);
                }
                DownloaderTab::Settings => {
                    paint_gear(painter, icon_rect, icon_col);
                }
            }

            let text_pos = Pos2::new(
                content_start_x + icon_size + spacing,
                seg_rect.center().y - galley.size().y * 0.5_f32,
            );
            painter.galley(text_pos, galley, text_col);
        }
    }

    fn draw_action_bar(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let kind = match self.active_tab {
            DownloaderTab::Torrents => DownloadKind::Torrent,
            _ => DownloadKind::Http,
        };
        ui.horizontal(|ui| {
            if action_pill_btn(ui, "Pause All", pal, |painter, r, c| {
                paint_media_pause(painter, r, c);
            })
            .clicked()
            {
                self.engine.pause_all_in_tab(kind);
            }

            ui.add_space(2.0_f32);

            if action_pill_btn(ui, "Resume All", pal, |painter, r, c| {
                paint_media_play(painter, r, c);
            })
            .clicked()
            {
                self.engine.resume_all_in_tab(kind);
            }

            ui.add_space(2.0_f32);

            if action_pill_btn(ui, "Clear Finished", pal, |painter, r, c| {
                paint_circle_check(painter, r, c);
            })
            .clicked()
            {
                self.engine.clear_completed(kind);
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.search_filter)
                        .desired_width(72.0_f32)
                        .hint_text("Filter..."),
                );
            });
        });
    }

    fn draw_items_list(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let expected_kind = match self.active_tab {
            DownloaderTab::Torrents => DownloadKind::Torrent,
            _ => DownloadKind::Http,
        };
        let items: Vec<DownloadItem> = self
            .engine
            .state
            .read()
            .items
            .iter()
            .filter(|i| i.kind == expected_kind)
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
            ui.add_space(52.0_f32);
            ui.vertical_centered(|ui| {
                let center_x = ui.cursor().min.x + ui.available_width() * 0.5_f32;
                let icon_tile = Rect::from_center_size(
                    Pos2::new(center_x, ui.cursor().min.y + 24.0_f32),
                    Vec2::splat(48.0_f32),
                );
                ui.painter().rect_filled(
                    icon_tile,
                    CornerRadius::same(12),
                    Color32::from_white_alpha(10),
                );
                let inner_icon_r =
                    Rect::from_center_size(icon_tile.center(), Vec2::splat(24.0_f32));
                if expected_kind == DownloadKind::Torrent {
                    paint_magnet(ui.painter(), inner_icon_r, pal.sub);
                } else {
                    paint_download(ui.painter(), inner_icon_r, pal.sub, false);
                }

                ui.add_space(60.0_f32);
                let empty_title = match self.active_tab {
                    DownloaderTab::Torrents => "No Torrents",
                    _ => "No Downloads",
                };
                let empty_sub = match self.active_tab {
                    DownloaderTab::Torrents => {
                        "Magnet links and .torrent files added or opened will appear here."
                    }
                    _ => {
                        "Downloads from HenBrowser or added via + will appear here."
                    }
                };
                ui.label(
                    RichText::new(empty_title)
                        .font(FontId::proportional(15.0_f32))
                        .strong()
                        .color(pal.fg),
                );
                ui.add_space(4.0_f32);
                ui.label(
                    RichText::new(empty_sub)
                        .font(FontId::proportional(12.0_f32))
                        .color(pal.subtext),
                );
            });
            return;
        }

        egui::ScrollArea::vertical()
            .id_salt(("dl_items_scroll", expected_kind as u8))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for item in items {
                    self.draw_single_item_card(ui, &item, pal);
                    ui.add_space(7.0_f32);
                }
            });
    }

    fn draw_single_item_card(&mut self, ui: &mut egui::Ui, item: &DownloadItem, pal: &Palette) {
        let frac = item.progress_fraction();
        let card_frame = egui::Frame::NONE
            .fill(Color32::from_white_alpha(10))
            .stroke(Stroke::new(1.0_f32, pal.border))
            .corner_radius(CornerRadius::same(12))
            .inner_margin(egui::Margin::symmetric(12, 10));

        let resp = card_frame.show(ui, |ui| {
            // First Row: File type icon, File Name, Percentage / Status Badge
            ui.horizontal(|ui| {
                let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(28.0_f32), Sense::hover());
                let (pill_bg, icon_col) = match item.status {
                    DownloadStatus::Completed => (pal.success.gamma_multiply(0.18_f32), pal.success),
                    DownloadStatus::Downloading => (pal.accent.gamma_multiply(0.18_f32), pal.accent),
                    DownloadStatus::Paused => (
                        Color32::from_rgb(235, 175, 55).gamma_multiply(0.18_f32),
                        Color32::from_rgb(235, 175, 55),
                    ),
                    DownloadStatus::Failed(_) => (pal.danger.gamma_multiply(0.18_f32), pal.danger),
                    DownloadStatus::Queued => (pal.sub.gamma_multiply(0.18_f32), pal.sub),
                };
                ui.painter().rect_filled(icon_rect, CornerRadius::same(7), pill_bg);
                let inner_icon_rect =
                    Rect::from_center_size(icon_rect.center(), Vec2::splat(15.0_f32));
                if item.kind == DownloadKind::Torrent {
                    paint_magnet(ui.painter(), inner_icon_rect, icon_col);
                } else {
                    match item.status {
                        DownloadStatus::Completed => {
                            paint_circle_check(ui.painter(), inner_icon_rect, pal.success);
                        }
                        DownloadStatus::Downloading => {
                            paint_download(ui.painter(), inner_icon_rect, pal.accent, true);
                        }
                        _ => {
                            paint_file(ui.painter(), inner_icon_rect, icon_col);
                        }
                    }
                }

                ui.add_space(6.0_f32);

                let badge_text = match &item.status {
                    DownloadStatus::Queued => "Queued".to_string(),
                    DownloadStatus::Downloading => format!("{:.0}%", frac * 100.0_f32),
                    DownloadStatus::Paused => "Paused".to_string(),
                    DownloadStatus::Completed => "Completed".to_string(),
                    DownloadStatus::Failed(_) => "Failed".to_string(),
                };
                let badge_font = FontId::proportional(11.0_f32);
                let badge_galley =
                    ui.painter().layout_no_wrap(badge_text, badge_font, icon_col);
                let badge_w = badge_galley.size().x + 14.0_f32;
                let name_max_w = (ui.available_width() - badge_w - 6.0_f32).max(60.0_f32);

                let name_label = egui::Label::new(
                    RichText::new(&item.filename)
                        .font(FontId::proportional(13.5_f32))
                        .strong()
                        .color(pal.fg),
                )
                .truncate();
                ui.add_sized(Vec2::new(name_max_w, 24.0_f32), name_label);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let (b_rect, _) =
                        ui.allocate_exact_size(Vec2::new(badge_w, 18.0_f32), Sense::hover());
                    ui.painter().rect_filled(b_rect, CornerRadius::same(9), pill_bg);
                    ui.painter().galley(
                        Pos2::new(
                            b_rect.center().x - badge_galley.size().x * 0.5_f32,
                            b_rect.center().y - badge_galley.size().y * 0.5_f32,
                        ),
                        badge_galley,
                        icon_col,
                    );
                });
            });

            ui.add_space(5.0_f32);

            // Second Row: Sleek macOS-style 4px/5px tall progress bar with smooth rounded caps
            let bar_w = ui.available_width().max(20.0_f32);
            let (bar_rect, _) = ui.allocate_exact_size(Vec2::new(bar_w, 5.0_f32), Sense::hover());
            ui.painter().rect_filled(
                bar_rect,
                CornerRadius::same(2),
                pal.border.gamma_multiply(0.6_f32),
            );
            if frac > 0.0_f32 {
                let fill_w = (bar_rect.width() * frac).max(5.0_f32);
                let fill_rect = Rect::from_min_size(bar_rect.min, Vec2::new(fill_w, bar_rect.height()));
                let bar_col = match item.status {
                    DownloadStatus::Completed => pal.success,
                    DownloadStatus::Paused => Color32::from_rgb(235, 175, 55),
                    DownloadStatus::Failed(_) => pal.danger,
                    _ => pal.accent,
                };
                ui.painter().rect_filled(fill_rect, CornerRadius::same(2), bar_col);
            }

            ui.add_space(5.0_f32);

            // Third Row: Transfer rate & size in secondary text color, right-aligned vector actions
            ui.horizontal(|ui| {
                let metrics_text = match &item.status {
                    DownloadStatus::Downloading => {
                        let mut s = format!(
                            "{} • {}",
                            format_speed(item.speed_bps),
                            if item.total_bytes > 0 {
                                format!(
                                    "{} of {}",
                                    format_bytes(item.received_bytes),
                                    format_bytes(item.total_bytes)
                                )
                            } else {
                                format_bytes(item.received_bytes)
                            }
                        );
                        if let Some(eta) = item.eta_seconds() {
                            s.push_str(&format!(" • {}", format_eta(eta)));
                        }
                        if item.kind == DownloadKind::Torrent && item.peers_connected > 0 {
                            s.push_str(&format!(" • {}p", item.peers_connected));
                        }
                        s
                    }
                    DownloadStatus::Completed => {
                        format!(
                            "{} • Completed",
                            format_bytes(if item.total_bytes > 0 {
                                item.total_bytes
                            } else {
                                item.received_bytes
                            })
                        )
                    }
                    DownloadStatus::Paused => {
                        if item.total_bytes > 0 {
                            format!(
                                "{} of {} • Paused",
                                format_bytes(item.received_bytes),
                                format_bytes(item.total_bytes)
                            )
                        } else {
                            format!("{} • Paused", format_bytes(item.received_bytes))
                        }
                    }
                    DownloadStatus::Failed(err) => {
                        format!("Error: {err}")
                    }
                    DownloadStatus::Queued => "Queued • Waiting...".to_string(),
                };

                let metrics_galley = ui.painter().layout_no_wrap(
                    metrics_text,
                    FontId::proportional(11.5_f32),
                    pal.subtext,
                );
                let metrics_pos = ui.cursor().min + Vec2::new(0.0_f32, 3.0_f32);
                ui.painter().galley(metrics_pos, metrics_galley, pal.subtext);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Trash / Cancel button
                    let trash_id = ui.id().with(("trash", item.id));
                    if card_icon_btn(
                        ui,
                        trash_id,
                        "Remove download (Shift: trash file)",
                        pal,
                        true,
                        |p, r, c| {
                            paint_trash(p, r, c);
                        },
                    )
                    .clicked()
                    {
                        let shift = ui.input(|i| i.modifiers.shift);
                        self.engine.remove_item(item.id, shift);
                    }

                    ui.add_space(2.0_f32);

                    // Reveal in Files button
                    let folder_id = ui.id().with(("folder", item.id));
                    if card_icon_btn(ui, folder_id, "Reveal in Files", pal, false, |p, r, c| {
                        paint_folder(p, r, c);
                    })
                    .clicked()
                    {
                        open_path_in_files(&item.save_dir);
                    }

                    ui.add_space(2.0_f32);

                    // Pause / Resume button
                    if item.status.can_pause() {
                        let pause_id = ui.id().with(("pause", item.id));
                        if card_icon_btn(ui, pause_id, "Pause download", pal, false, |p, r, c| {
                            paint_media_pause(p, r, c);
                        })
                        .clicked()
                        {
                            self.engine.pause(item.id);
                        }
                    } else if item.status.can_resume() {
                        let play_id = ui.id().with(("resume", item.id));
                        if card_icon_btn(ui, play_id, "Resume download", pal, false, |p, r, c| {
                            paint_media_play(p, r, c);
                        })
                        .clicked()
                        {
                            self.engine.start_or_resume(item.id);
                        }
                    } else if item.status == DownloadStatus::Completed {
                        let open_id = ui.id().with(("open_file", item.id));
                        if card_icon_btn(ui, open_id, "Open downloaded file", pal, false, |p, r, c| {
                            paint_file(p, r, c);
                        })
                        .clicked()
                        {
                            let target = item.target_path();
                            chickenwm_core::elevation::shell_open_unelevated(&target, None, None);
                        }
                    }
                });
            });

            if self.editing_location_id == Some(item.id) {
                ui.add_space(6.0_f32);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("New Save Folder:").size(12.0_f32).color(pal.sub));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.editing_location_path)
                            .desired_width(180.0_f32),
                    );
                    if action_pill_btn(ui, "Apply", pal, |p, r, c| paint_circle_check(p, r, c)).clicked() {
                        let new_dir = self.editing_location_path.trim().to_string();
                        if !new_dir.is_empty() {
                            self.engine.change_save_location(item.id, new_dir);
                        }
                        self.editing_location_id = None;
                    }
                    if action_pill_btn(ui, "Cancel", pal, |p, r, c| paint_close(p, r, c)).clicked() {
                        self.editing_location_id = None;
                    }
                });
            }
        });

        // Context menu on the card
        resp.response.context_menu(|ui| {
            if item.status == DownloadStatus::Completed {
                if ui.button("Open File").clicked() {
                    let target = item.target_path();
                    chickenwm_core::elevation::shell_open_unelevated(&target, None, None);
                    ui.close_menu();
                }
            }
            if ui.button("Reveal in Files").clicked() {
                open_path_in_files(&item.save_dir);
                ui.close_menu();
            }
            if ui.button("Change Save Location…").clicked() {
                if let Some(picked) = pick_folder_dialog(&item.save_dir) {
                    self.engine.change_save_location(item.id, picked);
                } else {
                    self.editing_location_id = Some(item.id);
                    self.editing_location_path = item.save_dir.clone();
                }
                ui.close_menu();
            }
            ui.separator();
            if ui.button("Copy URL").clicked() {
                ui.ctx().copy_text(item.url.clone());
                ui.close_menu();
            }
            if ui.button(RichText::new("Remove from List").color(pal.danger)).clicked() {
                self.engine.remove_item(item.id, false);
                ui.close_menu();
            }
        });
    }

    fn draw_settings_tab(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        egui::ScrollArea::vertical()
            .id_salt("dl_settings_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // SECTION 1: DOWNLOAD DESTINATION
                section_header(ui, "DOWNLOAD DESTINATION", pal);
                egui::Frame::NONE
                    .fill(Color32::from_white_alpha(10))
                    .stroke(Stroke::new(1.0_f32, pal.border))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        // Row 1: Default Folder
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Default Folder").size(13.0_f32).color(pal.fg));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let def_dir = self.engine.state.read().default_save_dir.clone();
                                if action_pill_btn(ui, "Browse…", pal, |p, r, c| {
                                    paint_folder(p, r, c);
                                })
                                .clicked()
                                {
                                    if let Some(chosen) = pick_folder_dialog(&def_dir) {
                                        self.engine.state.write().default_save_dir = chosen;
                                        self.engine.save();
                                    }
                                }
                            });
                        });

                        ui.add_space(4.0_f32);
                        let def_dir = self.engine.state.read().default_save_dir.clone();
                        let (path_r, path_resp) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 24.0_f32),
                            Sense::click(),
                        );
                        ui.painter().rect_filled(
                            path_r,
                            CornerRadius::same(6),
                            Color32::from_white_alpha(12),
                        );
                        let path_galley = ui.painter().layout_no_wrap(
                            def_dir.clone(),
                            FontId::proportional(11.5_f32),
                            pal.sub,
                        );
                        ui.painter().galley(
                            Pos2::new(path_r.left() + 8.0_f32, path_r.center().y - path_galley.size().y * 0.5_f32),
                            path_galley,
                            pal.sub,
                        );
                        if path_resp.on_hover_text("Click to open folder in Files").clicked() {
                            open_path_in_files(&def_dir);
                        }

                        ui.add_space(8.0_f32);
                        ui.separator();
                        ui.add_space(8.0_f32);

                        // Row 2: Always ask where to save
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Always ask where to save each file")
                                    .size(13.0_f32)
                                    .color(pal.fg),
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let mut ask_loc = self.engine.state.read().ask_save_location;
                                let id = ui.id().with("ask_location_toggle");
                                if apple_toggle(ui, id, &mut ask_loc, pal).changed() {
                                    self.engine.state.write().ask_save_location = ask_loc;
                                    self.engine.save();
                                }
                            });
                        });
                    });

                ui.add_space(14.0_f32);

                // SECTION 2: SPEED LIMITS
                section_header(ui, "SPEED LIMITS", pal);
                egui::Frame::NONE
                    .fill(Color32::from_white_alpha(10))
                    .stroke(Stroke::new(1.0_f32, pal.border))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Max Download Speed").size(13.0_f32).color(pal.fg));
                        });
                        ui.add_space(6.0_f32);

                        let current_dl = self.engine.state.read().global_speed_limit_kbps;
                        let presets: &[(&str, u32)] = &[
                            ("Unlimited", 0),
                            ("500 KB/s", 500),
                            ("1 MB/s", 1024),
                            ("2 MB/s", 2048),
                            ("5 MB/s", 5120),
                            ("10 MB/s", 10240),
                        ];
                        ui.horizontal_wrapped(|ui| {
                            for &(label, kbps) in presets {
                                let active = current_dl == kbps;
                                let btn = egui::Button::new(
                                    RichText::new(label)
                                        .size(11.5_f32)
                                        .color(if active { pal.accent_fg } else { pal.fg }),
                                )
                                .fill(if active { pal.accent } else { Color32::from_white_alpha(15) })
                                .stroke(Stroke::new(1.0_f32, pal.border))
                                .corner_radius(CornerRadius::same(12));
                                if ui.add(btn).clicked() {
                                    self.engine.set_global_speed_limit(kbps);
                                    self.custom_limit_input = if kbps > 0 {
                                        kbps.to_string()
                                    } else {
                                        String::new()
                                    };
                                }
                            }
                        });

                        ui.add_space(6.0_f32);
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Custom (KB/s):").size(12.0_f32).color(pal.sub));
                            let resp = ui.add(
                                egui::TextEdit::singleline(&mut self.custom_limit_input)
                                    .desired_width(70.0_f32)
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
                        });

                        ui.add_space(8.0_f32);
                        ui.separator();
                        ui.add_space(8.0_f32);

                        // Row 2: Max Upload Speed
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Max Upload Speed").size(13.0_f32).color(pal.fg));
                        });
                        ui.add_space(6.0_f32);
                        let up_presets: &[(&str, u32)] = &[
                            ("Unlimited", 0),
                            ("250 KB/s", 250),
                            ("500 KB/s", 500),
                            ("1 MB/s", 1024),
                            ("2 MB/s", 2048),
                        ];
                        ui.horizontal_wrapped(|ui| {
                            for &(label, kbps) in up_presets {
                                let active = self.upload_speed_limit_kbps == kbps;
                                let btn = egui::Button::new(
                                    RichText::new(label)
                                        .size(11.5_f32)
                                        .color(if active { pal.accent_fg } else { pal.fg }),
                                )
                                .fill(if active { pal.accent } else { Color32::from_white_alpha(15) })
                                .stroke(Stroke::new(1.0_f32, pal.border))
                                .corner_radius(CornerRadius::same(12));
                                if ui.add(btn).clicked() {
                                    self.upload_speed_limit_kbps = kbps;
                                    self.custom_upload_limit_input = if kbps > 0 {
                                        kbps.to_string()
                                    } else {
                                        String::new()
                                    };
                                }
                            }
                        });
                    });

                ui.add_space(14.0_f32);

                // SECTION 3: INTEGRATION
                section_header(ui, "INTEGRATION", pal);
                egui::Frame::NONE
                    .fill(Color32::from_white_alpha(10))
                    .stroke(Stroke::new(1.0_f32, pal.border))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(14, 12))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(RichText::new("HenBrowser & HenDownloader IPC").size(13.0_f32).strong().color(pal.fg));
                                ui.label(RichText::new("Integrated with HenBrowser & HenDownloader IPC").size(11.5_f32).color(pal.sub));
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                // Active status badge with green status dot
                                let (badge_r, _) = ui.allocate_exact_size(Vec2::new(76.0_f32, 22.0_f32), Sense::hover());
                                ui.painter().rect_filled(
                                    badge_r,
                                    CornerRadius::same(11),
                                    pal.success.gamma_multiply(0.18_f32),
                                );
                                let dot_r = Rect::from_center_size(
                                    Pos2::new(badge_r.left() + 12.0_f32, badge_r.center().y),
                                    Vec2::splat(8.0_f32),
                                );
                                paint_status_dot(ui.painter(), dot_r, pal.success);
                                let txt_pos = Pos2::new(badge_r.left() + 22.0_f32, badge_r.center().y - 6.0_f32);
                                ui.painter().text(
                                    txt_pos,
                                    egui::Align2::LEFT_TOP,
                                    "Active",
                                    FontId::proportional(11.5_f32),
                                    pal.success,
                                );
                            });
                        });

                        ui.add_space(8.0_f32);
                        ui.separator();
                        ui.add_space(8.0_f32);

                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Named Pipe").size(12.0_f32).color(pal.sub));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(
                                    RichText::new(r"\\.\pipe\chickenwm-downloader-ipc")
                                        .monospace()
                                        .size(11.0_f32)
                                        .color(pal.fg),
                                );
                            });
                        });
                    });
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
                    .rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(150));
            });

        let modal_w = (screen.width() - 40.0_f32).clamp(240.0_f32, 360.0_f32);
        egui::Window::new("New Download")
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .order(egui::Order::Foreground)
            .fixed_size(Vec2::new(modal_w, 260.0_f32))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(
                egui::Frame::NONE
                    .fill(pal.card)
                    .stroke(Stroke::new(1.0_f32, pal.accent.gamma_multiply(0.7_f32)))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(egui::Margin::symmetric(16, 14)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let icon_size = 18.0_f32;
                    let (ir, _) = ui.allocate_exact_size(Vec2::splat(icon_size), Sense::hover());
                    if self.modal_is_torrent {
                        paint_magnet(ui.painter(), ir, pal.accent);
                    } else {
                        paint_download(ui.painter(), ir, pal.accent, false);
                    }
                    ui.add_space(4.0_f32);
                    ui.label(
                        RichText::new(if self.modal_is_torrent {
                            "Add BitTorrent / Magnet"
                        } else {
                            "Add Download"
                        })
                        .size(15.0_f32)
                        .strong()
                        .color(pal.fg),
                    );

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (cr, c_resp) = ui.allocate_exact_size(Vec2::splat(18.0_f32), Sense::click());
                        if c_resp.hovered() {
                            ui.painter().rect_filled(cr, CornerRadius::same(5), Color32::from_white_alpha(20));
                        }
                        let c_icon_r = Rect::from_center_size(cr.center(), Vec2::splat(10.0_f32));
                        paint_close(ui.painter(), c_icon_r, if c_resp.hovered() { pal.fg } else { pal.sub });
                        if c_resp.clicked() {
                            self.modal_open = false;
                        }
                    });
                });

                ui.add_space(10.0_f32);

                ui.label(RichText::new("URL or Magnet Link:").size(12.0_f32).color(pal.sub));
                let url_resp = ui.add(
                    egui::TextEdit::singleline(&mut self.modal_url)
                        .desired_width(f32::INFINITY)
                        .hint_text("https://... or magnet:?xt=..."),
                );
                if url_resp.changed() {
                    self.modal_is_torrent =
                        detect_is_torrent(&self.modal_url, Some(&self.modal_filename));
                    if self.modal_filename.trim().is_empty() {
                        self.modal_filename =
                            infer_filename_from_url(&self.modal_url, self.modal_is_torrent);
                    }
                }

                ui.add_space(6.0_f32);
                ui.label(RichText::new("File Name:").size(12.0_f32).color(pal.sub));
                ui.add(
                    egui::TextEdit::singleline(&mut self.modal_filename)
                        .desired_width(f32::INFINITY)
                        .hint_text("Auto-detected"),
                );

                ui.add_space(6.0_f32);
                ui.label(RichText::new("Save Folder:").size(12.0_f32).color(pal.sub));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.modal_save_dir)
                            .desired_width((modal_w - 110.0_f32).max(80.0_f32)),
                    );
                    if action_pill_btn(ui, "Browse…", pal, |p, r, c| {
                        paint_folder(p, r, c);
                    })
                    .clicked()
                    {
                        if let Some(folder) = pick_folder_dialog(&self.modal_save_dir) {
                            self.modal_save_dir = folder;
                        }
                    }
                });

                ui.add_space(12.0_f32);
                ui.horizontal(|ui| {
                    let start_btn = egui::Button::new(
                        RichText::new("Start")
                            .size(12.5_f32)
                            .strong()
                            .color(pal.accent_fg),
                    )
                    .fill(pal.accent)
                    .corner_radius(CornerRadius::same(12));
                    if ui.add(start_btn).clicked() && !self.modal_url.trim().is_empty() {
                        let is_tor = self.modal_is_torrent
                            || detect_is_torrent(&self.modal_url, Some(&self.modal_filename));
                        self.active_tab = if is_tor {
                            DownloaderTab::Torrents
                        } else {
                            DownloaderTab::Downloads
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

                    ui.add_space(4.0_f32);

                    if action_pill_btn(ui, "Add Paused", pal, |p, r, c| {
                        paint_media_pause(p, r, c);
                    })
                    .clicked()
                        && !self.modal_url.trim().is_empty()
                    {
                        let is_tor = self.modal_is_torrent
                            || detect_is_torrent(&self.modal_url, Some(&self.modal_filename));
                        self.active_tab = if is_tor {
                            DownloaderTab::Torrents
                        } else {
                            DownloaderTab::Downloads
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

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add(
                                egui::Button::new(RichText::new("Cancel").size(12.0_f32).color(pal.sub))
                                    .fill(Color32::TRANSPARENT)
                                    .stroke(Stroke::NONE),
                            )
                            .clicked()
                        {
                            self.modal_open = false;
                        }
                    });
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

        // Compute slide-out panel geometry: EXACT width 400.0 matching notification center
        let ppp = ctx.pixels_per_point().max(1.0_f32);
        let work_px = primary_work_area();
        let work = [
            work_px[0] / ppp,
            work_px[1] / ppp,
            work_px[2] / ppp,
            work_px[3] / ppp,
        ];
        let panel_w = 400.0_f32;
        let panel_h = 580.0_f32.min((work[3] - 32.0_f32).max(360.0_f32));
        let (pos, size, passthrough) = if self.panel_open {
            let x = (work[0] + work[2] - panel_w - 12.0_f32).round();
            let y = (work[1] + 10.0_f32).round();
            (Pos2::new(x, y), Vec2::new(panel_w, panel_h), false)
        } else {
            (
                Pos2::new((work[0] + work[2] - 2.0_f32).round(), work[1].round()),
                Vec2::new(1.0_f32, 1.0_f32),
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

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(pal.bg)
                    .corner_radius(CornerRadius::same(18))
                    .inner_margin(egui::Margin::symmetric(14, 12)),
            )
            .show(ctx, |ui| {
                self.draw_top_header(ui, &pal, ctx);
                ui.add_space(8.0_f32);
                self.draw_segmented_tabs(ui, &pal);
                ui.add_space(8.0_f32);

                match self.active_tab {
                    DownloaderTab::Downloads | DownloaderTab::Torrents => {
                        self.draw_action_bar(ui, &pal);
                        ui.add_space(8.0_f32);
                        self.draw_items_list(ui, &pal);
                    }
                    DownloaderTab::Settings => {
                        self.draw_settings_tab(ui, &pal);
                    }
                }
            });

        self.draw_add_modal(ctx, &pal);

        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("downloader_border"),
        ));
        chickenwm_core::window_frame::paint_window_gradient_border(&painter, screen, 18.0_f32);
    }
}

/// Apple-style pill button with vector icon on left and plain text label on right.
fn action_pill_btn<F>(
    ui: &mut egui::Ui,
    label: &str,
    pal: &Palette,
    paint_icon: F,
) -> egui::Response
where
    F: FnOnce(&egui::Painter, Rect, Color32),
{
    let font = FontId::proportional(11.5_f32);
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, pal.fg);
    let icon_size = 12.0_f32;
    let pad_x = 7.0_f32;
    let spacing = 5.0_f32;
    let total_w = pad_x * 2.0_f32 + icon_size + spacing + galley.size().x;
    let total_h = 24.0_f32;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(total_w, total_h), Sense::click());

    let hover = resp.hovered();
    let bg = if hover {
        Color32::from_white_alpha(22)
    } else {
        Color32::from_white_alpha(10)
    };
    ui.painter().rect(
        rect,
        CornerRadius::same(12),
        bg,
        Stroke::new(1.0_f32, pal.border),
        egui::StrokeKind::Inside,
    );

    let icon_rect = Rect::from_center_size(
        Pos2::new(rect.left() + pad_x + icon_size * 0.5_f32, rect.center().y),
        Vec2::splat(icon_size),
    );
    let icon_col = if hover { pal.fg } else { pal.sub };
    paint_icon(ui.painter(), icon_rect, icon_col);

    let text_pos = Pos2::new(icon_rect.right() + spacing, rect.center().y - galley.size().y * 0.5_f32);
    ui.painter().galley(text_pos, galley, if hover { pal.fg } else { pal.sub });

    resp
}

/// Apple-style card icon button with hover highlight and tooltip.
fn card_icon_btn<F>(
    ui: &mut egui::Ui,
    id: egui::Id,
    tooltip: &str,
    pal: &Palette,
    danger: bool,
    paint_icon: F,
) -> egui::Response
where
    F: FnOnce(&egui::Painter, Rect, Color32),
{
    let size = Vec2::splat(22.0_f32);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let resp = ui.interact(rect, id, Sense::click());
    let hover = resp.hovered();
    if hover {
        let hover_bg = if danger {
            pal.danger.gamma_multiply(0.2_f32)
        } else {
            Color32::from_white_alpha(22)
        };
        ui.painter().rect_filled(rect, CornerRadius::same(5), hover_bg);
    }
    let icon_col = if danger {
        if hover { pal.danger } else { pal.danger.gamma_multiply(0.85_f32) }
    } else {
        if hover { pal.fg } else { pal.sub }
    };
    let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(13.0_f32));
    paint_icon(ui.painter(), icon_rect, icon_col);
    resp.on_hover_text(tooltip)
}

/// macOS-style pill toggle switch.
fn apple_toggle(ui: &mut egui::Ui, id: egui::Id, on: &mut bool, pal: &Palette) -> egui::Response {
    let size = Vec2::new(36.0_f32, 20.0_f32);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(id, *on, 0.14_f32);
    let off_bg = Color32::from_white_alpha(30);
    let track = off_bg.lerp_to_gamma(pal.accent, t);
    ui.painter().rect_filled(rect, CornerRadius::same(10), track);
    let r = rect.height() * 0.5_f32 - 2.0_f32;
    let x = egui::lerp(rect.left() + r + 2.0_f32..=rect.right() - r - 2.0_f32, t);
    let knob = Pos2::new(x, rect.center().y);
    ui.painter().circle_filled(knob + Vec2::new(0.0_f32, 0.6_f32), r, Color32::from_black_alpha(45));
    ui.painter().circle_filled(knob, r, Color32::WHITE);
    resp
}

/// Section header in macOS Settings grouped style (small uppercase bold subtext).
fn section_header(ui: &mut egui::Ui, title: &str, pal: &Palette) {
    ui.horizontal(|ui| {
        ui.add_space(4.0_f32);
        ui.label(
            RichText::new(title)
                .font(FontId::proportional(11.0_f32))
                .strong()
                .color(pal.sub),
        );
    });
    ui.add_space(4.0_f32);
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

    if let Some(req) = chickenwm_core::headless::RenderRequest::from_args([400.0, 580.0]) {
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
    fn headless_render_verify_downloads_and_torrents_tabs() {
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
            size: egui::vec2(400.0, 580.0),
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
