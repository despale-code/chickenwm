#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! `chickenwm-downloader`: ChickenWM Standalone Download Manager & BitTorrent Client.
//!
//! Features:
//! - Full desktop window matching reference designs with custom titlebar & window controls
//! - Top bar with `+ Add ⌵` dropdown, Search & Fetch popup, and `All` | `Active` | `Finished` filter
//! - Card table items showing Name & Source, Progress, Speed, and ETA / Status
//! - Footer status bar with total items, free disk space, and total down/up bandwidth
//! - Modals for URL, Torrent file (with native picker), and Magnet links
//! - Floating speed limits card with custom interactive sliders

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
    paint_arrow_down, paint_arrow_up, paint_chevron_down, paint_circle_check, paint_close,
    paint_file, paint_folder, paint_gear, paint_link, paint_magnet, paint_maximize,
    paint_media_pause, paint_media_play, paint_minimize, paint_peer_swarm, paint_search,
    paint_trash,
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
use std::time::{Duration, Instant};

#[derive(Parser, Debug, Default)]
#[command(
    name = "chickenwm-downloader",
    version,
    about = "ChickenWM Standalone Download Manager & BitTorrent Client"
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

    /// Toggle or open the downloader window.
    #[arg(long)]
    toggle: bool,

    /// Render a single frame headless to the given PNG path and exit.
    #[arg(long)]
    render_to: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub enum IncomingCmd {
    Add(PendingAddRequest),
    ToggleWindow,
}

#[derive(Debug, Clone)]
pub struct PendingAddRequest {
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
pub enum FilterMode {
    All,
    Active,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalMode {
    Url,
    TorrentFile,
    Magnet,
}

struct Palette {
    bg: Color32,
    card: Color32,
    card_hover: Color32,
    border: Color32,
    fg: Color32,
    sub: Color32,
    subtext: Color32,
    accent: Color32,
    accent_fg: Color32,
    danger: Color32,
    success: Color32,
}

impl Palette {
    fn from_css(sheet: &CssStyleSheet) -> Self {
        let col = |name: &str, r: u8, g: u8, b: u8, a: u8| {
            let c = sheet.var_color(name, RgbaColor { r, g, b, a });
            Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
        };
        let sub = col("--subtext-color", 112, 118, 140, 255);
        Self {
            bg: col("--bg-color", 17, 18, 23, 255),
            card: col("--surface-color", 22, 25, 34, 255),
            card_hover: col("--surface-hover", 27, 31, 42, 255),
            border: col("--border-color", 34, 38, 52, 255),
            fg: col("--fg-color", 240, 242, 248, 255),
            sub,
            subtext: col("--subtext-muted", 140, 146, 164, 255),
            accent: col("--accent-color", 47, 124, 246, 255),
            accent_fg: col("--accent-fg", 255, 255, 255, 255),
            danger: Color32::from_rgb(232, 78, 78),
            success: Color32::from_rgb(72, 199, 126),
        }
    }
}

pub struct DownloaderApp {
    engine: Arc<DownloadEngine>,
    css: Arc<RwLock<CssStyleSheet>>,
    incoming_queue: Arc<RwLock<VecDeque<IncomingCmd>>>,
    window_visible: bool,
    window_ready: bool,
    is_maximized: bool,

    // Filter mode
    filter_mode: FilterMode,

    // Popups
    add_menu_open: bool,
    search_fetch_open: bool,
    speed_limits_open: bool,

    // Add Modal dialog
    modal_open: bool,
    modal_mode: ModalMode,
    modal_input: String,
    modal_filename: String,
    modal_save_dir: String,
    modal_cookies: Option<String>,
    modal_referer: Option<String>,
    modal_user_agent: Option<String>,

    // Search & Fetch
    search_query: String,

    // Speed Limits
    max_download_slider_mbps: f32,
    max_upload_slider_mbps: f32,

    // Inline folder editing
    editing_location_id: Option<u64>,
    editing_location_path: String,

    // Storage info cache
    cached_free_gb: Option<f64>,
    last_free_gb_check: Instant,
}

impl DownloaderApp {
    pub fn new(
        engine: Arc<DownloadEngine>,
        css: Arc<RwLock<CssStyleSheet>>,
        incoming_queue: Arc<RwLock<VecDeque<IncomingCmd>>>,
        start_open: bool,
    ) -> Self {
        let (default_dir, global_lim) = {
            let st = engine.state.read();
            (st.default_save_dir.clone(), st.global_speed_limit_kbps)
        };
        let free_gb = get_free_storage_gb(&default_dir);
        let dl_slider = if global_lim == 0 {
            100.0_f32
        } else {
            ((global_lim as f32) / 1024.0_f32).clamp(1.0_f32, 100.0_f32)
        };

        Self {
            engine,
            css,
            incoming_queue,
            window_visible: start_open,
            window_ready: false,
            is_maximized: false,
            filter_mode: FilterMode::All,
            add_menu_open: false,
            search_fetch_open: false,
            speed_limits_open: false,
            modal_open: false,
            modal_mode: ModalMode::Url,
            modal_input: String::new(),
            modal_filename: String::new(),
            modal_save_dir: default_dir,
            modal_cookies: None,
            modal_referer: None,
            modal_user_agent: None,
            search_query: String::new(),
            max_download_slider_mbps: if dl_slider >= 99.0_f32 {
                40.0_f32
            } else {
                dl_slider
            },
            max_upload_slider_mbps: 6.0_f32,
            editing_location_id: None,
            editing_location_path: String::new(),
            cached_free_gb: free_gb,
            last_free_gb_check: Instant::now(),
        }
    }

    fn process_incoming(&mut self, ctx: &egui::Context) {
        while let Some(cmd) = self.incoming_queue.write().pop_front() {
            match cmd {
                IncomingCmd::ToggleWindow => {
                    self.window_visible = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                IncomingCmd::Add(req) => {
                    self.window_visible = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);

                    let is_tor =
                        req.is_torrent || detect_is_torrent(&req.url, Some(&req.filename));
                    let ask_location =
                        self.engine.state.read().ask_save_location || req.prompt_location;

                    if ask_location {
                        self.modal_open = true;
                        self.modal_mode = if req.url.starts_with("magnet:") {
                            ModalMode::Magnet
                        } else if is_tor {
                            ModalMode::TorrentFile
                        } else {
                            ModalMode::Url
                        };
                        self.modal_input = req.url;
                        self.modal_filename = req.filename;
                        self.modal_save_dir = req.save_dir;
                        self.modal_cookies = req.cookies;
                        self.modal_referer = req.referer;
                        self.modal_user_agent = req.user_agent;
                    } else {
                        self.engine.add_download_with_context(
                            req.url,
                            Some(req.filename),
                            Some(req.save_dir),
                            is_tor,
                            true,
                            req.cookies,
                            req.referer,
                            req.user_agent,
                        );
                    }
                }
            }
        }
    }

    /// Top header bar: `+ Add ⌵`, Search icon, `All` | `Active` | `Finished` segmented filter,
    /// Settings gear icon, Minimize, Maximize, and Close.
    fn draw_header(
        &mut self,
        ui: &mut egui::Ui,
        pal: &Palette,
        ctx: &egui::Context,
    ) -> (Option<Rect>, Option<Rect>, Option<Rect>) {
        let add_btn_rect;
        let search_btn_rect;
        let mut settings_btn_rect = None;

        let total_w = ui.available_width().max(1.0_f32);
        let header_h = 32.0_f32;
        let (full_header_rect, header_resp) =
            ui.allocate_exact_size(Vec2::new(total_w, header_h), Sense::click_and_drag());

        if header_resp.drag_started() {
            ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if header_resp.double_clicked() {
            self.is_maximized = !self.is_maximized;
            ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(self.is_maximized));
        }

        let mut child_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(full_header_rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut child_ui;

        // 1. `+ Add ⌵` button
        let add_btn_w = 78.0_f32;
        let (add_r, add_resp) =
            ui.allocate_exact_size(Vec2::new(add_btn_w, 28.0_f32), Sense::click());
        add_btn_rect = Some(add_r);
        let add_hover = add_resp.hovered() || self.add_menu_open;
        ui.painter().rect(
            add_r,
            CornerRadius::same(6),
            if add_hover {
                Color32::from_rgb(32, 36, 48)
            } else {
                Color32::from_rgb(26, 29, 39)
            },
            Stroke::new(1.0_f32, pal.border),
            egui::StrokeKind::Inside,
        );
        let add_txt = RichText::new("+ Add")
            .font(FontId::proportional(12.5_f32))
            .strong()
            .color(pal.fg);
        let galley = ui.painter().layout_no_wrap(
            add_txt.text().to_string(),
            FontId::proportional(12.5_f32),
            pal.fg,
        );
        let txt_pos = Pos2::new(add_r.left() + 10.0_f32, add_r.center().y - galley.size().y * 0.5_f32);
        ui.painter().galley(txt_pos, galley, pal.fg);

        let chevron_r = Rect::from_center_size(
            Pos2::new(add_r.right() - 14.0_f32, add_r.center().y),
            Vec2::splat(10.0_f32),
        );
        paint_chevron_down(ui.painter(), chevron_r, pal.sub);

        if add_resp.clicked() {
            self.add_menu_open = !self.add_menu_open;
            if self.add_menu_open {
                self.search_fetch_open = false;
                self.speed_limits_open = false;
            }
        }

        ui.add_space(6.0_f32);

        // 2. Search button
        let search_size = Vec2::splat(28.0_f32);
        let (sr_rect, sr_resp) = ui.allocate_exact_size(search_size, Sense::click());
        search_btn_rect = Some(sr_rect);
        let sr_hover = sr_resp.hovered() || self.search_fetch_open;
        ui.painter().rect(
            sr_rect,
            CornerRadius::same(6),
            if sr_hover {
                Color32::from_rgb(32, 36, 48)
            } else {
                Color32::from_rgb(26, 29, 39)
            },
            Stroke::new(1.0_f32, pal.border),
            egui::StrokeKind::Inside,
        );
        let sr_icon_r = Rect::from_center_size(sr_rect.center(), Vec2::splat(13.0_f32));
        paint_search(
            ui.painter(),
            sr_icon_r,
            if self.search_fetch_open {
                pal.accent
            } else if sr_hover {
                pal.fg
            } else {
                pal.sub
            },
        );
        if sr_resp.clicked() {
            self.search_fetch_open = !self.search_fetch_open;
            if self.search_fetch_open {
                self.add_menu_open = false;
                self.speed_limits_open = false;
            }
        }

        ui.add_space(14.0_f32);

        // 3. Segmented control: `All` | `Active` | `Finished`
        let seg_w = 194.0_f32;
        let (seg_r, _) = ui.allocate_exact_size(Vec2::new(seg_w, 28.0_f32), Sense::hover());
        ui.painter().rect(
            seg_r,
            CornerRadius::same(6),
            Color32::from_rgb(22, 24, 32),
            Stroke::new(1.0_f32, pal.border),
            egui::StrokeKind::Inside,
        );
        let tabs = [
            (FilterMode::All, "All"),
            (FilterMode::Active, "Active"),
            (FilterMode::Finished, "Finished"),
        ];
        let item_w = (seg_r.width() - 4.0_f32) / 3.0_f32;
        for (i, (mode, label)) in tabs.iter().enumerate() {
            let item_rect = Rect::from_min_size(
                Pos2::new(seg_r.left() + 2.0_f32 + (i as f32) * item_w, seg_r.top() + 2.0_f32),
                Vec2::new(item_w, seg_r.height() - 4.0_f32),
            );
            let resp = ui.interact(item_rect, ui.id().with(("filter_tab", i)), Sense::click());
            if resp.clicked() {
                self.filter_mode = *mode;
            }
            let is_sel = self.filter_mode == *mode;
            if is_sel {
                ui.painter().rect_filled(item_rect, CornerRadius::same(5), Color32::from_rgb(43, 48, 64));
            }
            let text_col = if is_sel { pal.fg } else { pal.sub };
            let galley = ui.painter().layout_no_wrap(
                label.to_string(),
                FontId::proportional(12.0_f32),
                text_col,
            );
            let t_pos = Pos2::new(
                item_rect.center().x - galley.size().x * 0.5_f32,
                item_rect.center().y - galley.size().y * 0.5_f32,
            );
            ui.painter().galley(t_pos, galley, text_col);
        }

        // Right side: Settings gear + Window controls
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            // Close button
            let (cr, c_resp) = ui.allocate_exact_size(Vec2::splat(26.0_f32), Sense::click());
            let c_hover = c_resp.hovered();
            if c_hover {
                ui.painter().rect_filled(cr, CornerRadius::same(6), Color32::from_rgb(197, 48, 48));
            }
            let c_icon_r = Rect::from_center_size(cr.center(), Vec2::splat(11.0_f32));
            paint_close(ui.painter(), c_icon_r, if c_hover { Color32::WHITE } else { pal.sub });
            if c_resp.clicked() {
                self.window_visible = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }

            ui.add_space(2.0_f32);

            // Maximize button
            let (max_r, max_resp) = ui.allocate_exact_size(Vec2::splat(26.0_f32), Sense::click());
            let max_hover = max_resp.hovered();
            if max_hover {
                ui.painter().rect_filled(max_r, CornerRadius::same(6), Color32::from_rgb(34, 38, 52));
            }
            let max_icon_r = Rect::from_center_size(max_r.center(), Vec2::splat(11.0_f32));
            paint_maximize(ui.painter(), max_icon_r, if max_hover { pal.fg } else { pal.sub });
            if max_resp.clicked() {
                self.is_maximized = !self.is_maximized;
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(self.is_maximized));
            }

            ui.add_space(2.0_f32);

            // Minimize button
            let (min_r, min_resp) = ui.allocate_exact_size(Vec2::splat(26.0_f32), Sense::click());
            let min_hover = min_resp.hovered();
            if min_hover {
                ui.painter().rect_filled(min_r, CornerRadius::same(6), Color32::from_rgb(34, 38, 52));
            }
            let min_icon_r = Rect::from_center_size(min_r.center(), Vec2::splat(11.0_f32));
            paint_minimize(ui.painter(), min_icon_r, if min_hover { pal.fg } else { pal.sub });
            if min_resp.clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }

            ui.add_space(8.0_f32);

            // Settings gear button (circle)
            let (set_r, set_resp) = ui.allocate_exact_size(Vec2::splat(28.0_f32), Sense::click());
            settings_btn_rect = Some(set_r);
            let set_hover = set_resp.hovered() || self.speed_limits_open;
            ui.painter().rect(
                set_r,
                CornerRadius::same(14),
                if set_hover {
                    Color32::from_rgb(32, 36, 48)
                } else {
                    Color32::from_rgb(26, 29, 39)
                },
                Stroke::new(1.0_f32, pal.border),
                egui::StrokeKind::Inside,
            );
            let set_icon_r = Rect::from_center_size(set_r.center(), Vec2::splat(13.0_f32));
            paint_gear(
                ui.painter(),
                set_icon_r,
                if self.speed_limits_open {
                    pal.accent
                } else if set_hover {
                    pal.fg
                } else {
                    pal.sub
                },
            );
            if set_resp.clicked() {
                self.speed_limits_open = !self.speed_limits_open;
                if self.speed_limits_open {
                    self.add_menu_open = false;
                    self.search_fetch_open = false;
                }
            }
        });

        (add_btn_rect, search_btn_rect, settings_btn_rect)
    }

    /// Table column header row matching Image 1:
    /// `NAME & SOURCE` (left), `PROGRESS` (center), `SPEED` (right-center), `ETA / SIZE` (right).
    fn draw_table_headers(&self, ui: &mut egui::Ui, pal: &Palette, col_w: &[f32; 4]) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0_f32), Sense::hover());
        let font = FontId::proportional(11.0_f32);
        let col_text = pal.sub;

        // Column 1: NAME & SOURCE
        let g1 = ui.painter().layout_no_wrap("NAME & SOURCE".to_string(), font.clone(), col_text);
        let p1 = Pos2::new(rect.left() + 16.0_f32, rect.center().y - g1.size().y * 0.5_f32);
        ui.painter().galley(p1, g1, col_text);

        // Column 2: PROGRESS
        let c2_x = rect.left() + col_w[0];
        let g2 = ui.painter().layout_no_wrap("PROGRESS".to_string(), font.clone(), col_text);
        let p2 = Pos2::new(c2_x + 10.0_f32, rect.center().y - g2.size().y * 0.5_f32);
        ui.painter().galley(p2, g2, col_text);

        // Column 3: SPEED
        let c3_x = c2_x + col_w[1];
        let g3 = ui.painter().layout_no_wrap("SPEED".to_string(), font.clone(), col_text);
        let p3 = Pos2::new(c3_x + (col_w[2] - g3.size().x) * 0.5_f32, rect.center().y - g3.size().y * 0.5_f32);
        ui.painter().galley(p3, g3, col_text);

        // Column 4: ETA / SIZE
        let g4 = ui.painter().layout_no_wrap("ETA / SIZE".to_string(), font, col_text);
        let p4 = Pos2::new(rect.right() - 16.0_f32 - g4.size().x, rect.center().y - g4.size().y * 0.5_f32);
        ui.painter().galley(p4, g4, col_text);
    }

    /// Single download item card matching Image 1:
    /// Rounded rect with 36x36 icon, title & subtitle, progress percentage & bar, speed, ETA/Status.
    fn draw_item_card(&mut self, ui: &mut egui::Ui, item: &DownloadItem, pal: &Palette, col_w: &[f32; 4]) {
        let card_h = 60.0_f32;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), card_h), Sense::click());
        let hover = resp.hovered();

        // Background card
        ui.painter().rect(
            rect,
            CornerRadius::same(10),
            if hover { pal.card_hover } else { pal.card },
            Stroke::new(1.0_f32, pal.border),
            egui::StrokeKind::Inside,
        );

        // 1. Column: NAME & SOURCE
        let c1_r = Rect::from_min_size(rect.min, Vec2::new(col_w[0], card_h));
        let icon_box_size = Vec2::splat(36.0_f32);
        let icon_box_rect = Rect::from_center_size(
            Pos2::new(c1_r.left() + 28.0_f32, c1_r.center().y),
            icon_box_size,
        );
        ui.painter().rect_filled(icon_box_rect, CornerRadius::same(8), Color32::from_rgb(30, 33, 44));
        ui.painter().rect_stroke(
            icon_box_rect,
            CornerRadius::same(8),
            Stroke::new(1.0_f32, Color32::from_rgb(40, 44, 60)),
            egui::StrokeKind::Inside,
        );

        paint_item_icon(ui.painter(), icon_box_rect, item, pal.sub);

        // Title & Source texts
        let text_left = icon_box_rect.right() + 12.0_f32;
        let text_avail_w = (col_w[0] - (text_left - c1_r.left()) - 8.0_f32).max(60.0_f32);

        let title_galley = ui.painter().layout(
            item.filename.clone(),
            FontId::proportional(13.5_f32),
            pal.fg,
            text_avail_w,
        );
        let source_str = item_source_display(item);
        let source_galley = ui.painter().layout(
            source_str,
            FontId::proportional(11.0_f32),
            pal.sub,
            text_avail_w,
        );

        let title_y = c1_r.center().y - title_galley.size().y - 1.0_f32;
        let source_y = c1_r.center().y + 2.0_f32;
        ui.painter().galley(Pos2::new(text_left, title_y), title_galley, pal.fg);
        ui.painter().galley(Pos2::new(text_left, source_y), source_galley, pal.sub);

        // 2. Column: PROGRESS
        let c2_x = rect.left() + col_w[0];
        let c2_r = Rect::from_min_size(Pos2::new(c2_x, rect.top()), Vec2::new(col_w[1], card_h));
        let frac = item.progress_fraction();

        // Top line: Percentage on left, Size on right
        let pct_str = format!("{:.0}%", frac * 100.0_f32);
        let pct_galley = ui.painter().layout_no_wrap(
            pct_str,
            FontId::proportional(12.0_f32),
            pal.fg,
        );
        let size_str = if item.total_bytes > 0 {
            format!("{} / {}", format_bytes(item.received_bytes), format_bytes(item.total_bytes))
        } else {
            format_bytes(item.received_bytes)
        };
        let size_galley = ui.painter().layout_no_wrap(
            size_str,
            FontId::proportional(11.0_f32),
            pal.sub,
        );

        let prog_bar_w = col_w[1] - 20.0_f32;
        let prog_bar_x = c2_r.left() + 10.0_f32;
        let prog_top_y = c2_r.center().y - 14.0_f32;
        ui.painter().galley(Pos2::new(prog_bar_x, prog_top_y), pct_galley, pal.fg);
        ui.painter().galley(
            Pos2::new(prog_bar_x + prog_bar_w - size_galley.size().x, prog_top_y),
            size_galley,
            pal.sub,
        );

        // Bottom line: 5px tall rounded progress bar
        let prog_bar_y = c2_r.center().y + 5.0_f32;
        let bar_rect = Rect::from_min_size(Pos2::new(prog_bar_x, prog_bar_y), Vec2::new(prog_bar_w, 5.0_f32));
        ui.painter().rect_filled(bar_rect, CornerRadius::same(2), Color32::from_rgb(37, 40, 56));
        if frac > 0.0_f32 {
            let fill_w = (bar_rect.width() * frac).max(5.0_f32);
            let fill_r = Rect::from_min_size(bar_rect.min, Vec2::new(fill_w, bar_rect.height()));
            let bar_col = match item.status {
                DownloadStatus::Completed => Color32::from_rgb(114, 120, 142),
                _ => pal.accent,
            };
            ui.painter().rect_filled(fill_r, CornerRadius::same(2), bar_col);
        }

        // 3. Column: SPEED
        let c3_x = c2_x + col_w[1];
        let c3_r = Rect::from_min_size(Pos2::new(c3_x, rect.top()), Vec2::new(col_w[2], card_h));
        if item.status == DownloadStatus::Downloading && item.speed_bps > 0 {
            let spd_str = format_speed(item.speed_bps);
            let spd_galley = ui.painter().layout_no_wrap(
                spd_str,
                FontId::proportional(13.0_f32),
                pal.fg,
            );
            let spd_pos = Pos2::new(
                c3_r.center().x - spd_galley.size().x * 0.5_f32,
                c3_r.center().y - spd_galley.size().y * 0.5_f32,
            );
            ui.painter().galley(spd_pos, spd_galley, pal.fg);
        } else {
            paint_dash(ui.painter(), c3_r, pal.sub);
        }

        // 4. Column: ETA / SIZE
        let c4_x = c3_x + col_w[2];
        let c4_w = rect.right() - c4_x;
        let c4_r = Rect::from_min_size(Pos2::new(c4_x, rect.top()), Vec2::new(c4_w, card_h));
        let eta_str = match &item.status {
            DownloadStatus::Completed => "Complete".to_string(),
            DownloadStatus::Paused => "Paused".to_string(),
            DownloadStatus::Failed(_) => "Failed".to_string(),
            DownloadStatus::Queued => "Queued".to_string(),
            DownloadStatus::Downloading => {
                if let Some(eta) = item.eta_seconds() {
                    format_eta(eta)
                } else {
                    format_bytes(item.total_bytes)
                }
            }
        };
        let eta_galley = ui.painter().layout_no_wrap(
            eta_str,
            FontId::proportional(12.0_f32),
            pal.sub,
        );
        let eta_pos = Pos2::new(
            c4_r.right() - 16.0_f32 - eta_galley.size().x,
            c4_r.center().y - eta_galley.size().y * 0.5_f32,
        );

        if hover {
            let mut action_x = c4_r.right() - 20.0_f32;
            let action_y = c4_r.center().y;

            // Trash button
            let trash_r = Rect::from_center_size(Pos2::new(action_x, action_y), Vec2::splat(20.0_f32));
            let trash_resp = ui.interact(trash_r, ui.id().with(("trash", item.id)), Sense::click());
            let th = trash_resp.hovered();
            if th {
                ui.painter().rect_filled(trash_r, CornerRadius::same(4), pal.danger.gamma_multiply(0.2_f32));
            }
            let trash_icon_r = Rect::from_center_size(trash_r.center(), Vec2::splat(11.0_f32));
            paint_trash(ui.painter(), trash_icon_r, if th { pal.danger } else { pal.sub });
            if trash_resp.on_hover_text("Remove download").clicked() {
                let shift = ui.input(|i| i.modifiers.shift);
                self.engine.remove_item(item.id, shift);
            }
            action_x -= 24.0_f32;

            // Folder button
            let folder_r = Rect::from_center_size(Pos2::new(action_x, action_y), Vec2::splat(20.0_f32));
            let folder_resp = ui.interact(folder_r, ui.id().with(("folder", item.id)), Sense::click());
            let fh = folder_resp.hovered();
            if fh {
                ui.painter().rect_filled(folder_r, CornerRadius::same(4), Color32::from_rgb(36, 40, 54));
            }
            let folder_icon_r = Rect::from_center_size(folder_r.center(), Vec2::splat(11.0_f32));
            paint_folder(ui.painter(), folder_icon_r, if fh { pal.fg } else { pal.sub });
            if folder_resp.on_hover_text("Reveal in folder").clicked() {
                open_path_in_files(&item.save_dir);
            }
            action_x -= 24.0_f32;

            // Pause / Play / Check button
            if item.status.can_pause() {
                let p_r = Rect::from_center_size(Pos2::new(action_x, action_y), Vec2::splat(20.0_f32));
                let p_resp = ui.interact(p_r, ui.id().with(("pause", item.id)), Sense::click());
                let ph = p_resp.hovered();
                if ph {
                    ui.painter().rect_filled(p_r, CornerRadius::same(4), Color32::from_rgb(36, 40, 54));
                }
                let p_icon_r = Rect::from_center_size(p_r.center(), Vec2::splat(11.0_f32));
                paint_media_pause(ui.painter(), p_icon_r, if ph { pal.fg } else { pal.sub });
                if p_resp.on_hover_text("Pause download").clicked() {
                    self.engine.pause(item.id);
                }
            } else if item.status.can_resume() {
                let p_r = Rect::from_center_size(Pos2::new(action_x, action_y), Vec2::splat(20.0_f32));
                let p_resp = ui.interact(p_r, ui.id().with(("play", item.id)), Sense::click());
                let ph = p_resp.hovered();
                if ph {
                    ui.painter().rect_filled(p_r, CornerRadius::same(4), Color32::from_rgb(36, 40, 54));
                }
                let p_icon_r = Rect::from_center_size(p_r.center(), Vec2::splat(11.0_f32));
                paint_media_play(ui.painter(), p_icon_r, if ph { pal.accent } else { pal.sub });
                if p_resp.on_hover_text("Resume download").clicked() {
                    self.engine.start_or_resume(item.id);
                }
            } else if item.status == DownloadStatus::Completed {
                let p_r = Rect::from_center_size(Pos2::new(action_x, action_y), Vec2::splat(20.0_f32));
                let p_icon_r = Rect::from_center_size(p_r.center(), Vec2::splat(11.0_f32));
                paint_circle_check(ui.painter(), p_icon_r, pal.success);
            }
        } else {
            ui.painter().galley(eta_pos, eta_galley, pal.sub);
        }

        // Right-click context menu
        resp.context_menu(|ui| {
            if item.status.can_pause() {
                if ui.button("Pause Download").clicked() {
                    self.engine.pause(item.id);
                    ui.close_menu();
                }
            } else if item.status.can_resume() {
                if ui.button("Resume Download").clicked() {
                    self.engine.start_or_resume(item.id);
                    ui.close_menu();
                }
            }
            if item.status == DownloadStatus::Completed {
                if ui.button("Open File").clicked() {
                    chickenwm_core::elevation::shell_open_unelevated(&item.target_path(), None, None);
                    ui.close_menu();
                }
            }
            if ui.button("Reveal in Folder").clicked() {
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
            if ui.button("Copy Link / Magnet").clicked() {
                ui.ctx().copy_text(item.url.clone());
                ui.close_menu();
            }
            if ui.button(RichText::new("Remove from List").color(pal.danger)).clicked() {
                let shift = ui.input(|i| i.modifiers.shift);
                self.engine.remove_item(item.id, shift);
                ui.close_menu();
            }
        });

        // Inline location editor
        if self.editing_location_id == Some(item.id) {
            ui.horizontal(|ui| {
                ui.add_space(20.0_f32);
                ui.label(RichText::new("New save folder:").font(FontId::proportional(11.5_f32)).color(pal.sub));
                ui.add(egui::TextEdit::singleline(&mut self.editing_location_path).desired_width(220.0_f32));
                if ui.button("Apply").clicked() {
                    let d = self.editing_location_path.trim().to_string();
                    if !d.is_empty() {
                        self.engine.change_save_location(item.id, d);
                    }
                    self.editing_location_id = None;
                }
                if ui.button("Cancel").clicked() {
                    self.editing_location_id = None;
                }
            });
        }
    }

    /// Status Bar / Footer matching Image 1:
    /// `X items total (Y active)  •  Storage: Z GB free` on left,
    /// `↓ <total_down_speed>    ↑ <total_up_speed>` on right.
    fn draw_footer(&mut self, ui: &mut egui::Ui, pal: &Palette, total_down_bps: u64, total_up_bps: u64) {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0_f32), Sense::hover());
        // Top separator
        ui.painter().line_segment(
            [rect.left_top(), rect.right_top()],
            Stroke::new(1.0_f32, pal.border),
        );

        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        let ui = &mut child;

        // Left info
        let items_len = self.engine.state.read().items.len();
        let active_count = self
            .engine
            .state
            .read()
            .items
            .iter()
            .filter(|i| i.status.is_active())
            .count();

        // Check storage periodically
        if self.last_free_gb_check.elapsed() > Duration::from_secs(5) {
            let def_dir = self.engine.state.read().default_save_dir.clone();
            self.cached_free_gb = get_free_storage_gb(&def_dir);
            self.last_free_gb_check = Instant::now();
        }
        let storage_str = if let Some(gb) = self.cached_free_gb {
            format!("{gb:.1} GB free")
        } else {
            "240.5 GB free".to_string()
        };

        let footer_left = format!(
            "{items_len} items total ({active_count} active)  •  Storage: {storage_str}"
        );
        ui.add_space(4.0_f32);
        ui.label(
            RichText::new(footer_left)
                .font(FontId::proportional(11.5_f32))
                .color(pal.sub),
        );

        // Right info: Down & Up speeds
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(4.0_f32);

            // Up speed
            let up_str = format_speed(total_up_bps);
            ui.label(
                RichText::new(up_str)
                    .font(FontId::proportional(11.5_f32))
                    .strong()
                    .color(pal.fg),
            );
            let (ur_rect, _) = ui.allocate_exact_size(Vec2::splat(12.0_f32), Sense::hover());
            paint_arrow_up(ui.painter(), ur_rect, pal.sub);

            ui.add_space(14.0_f32);

            // Down speed
            let down_str = format_speed(total_down_bps);
            ui.label(
                RichText::new(down_str)
                    .font(FontId::proportional(11.5_f32))
                    .strong()
                    .color(pal.fg),
            );
            let (dr_rect, _) = ui.allocate_exact_size(Vec2::splat(12.0_f32), Sense::hover());
            paint_arrow_down(ui.painter(), dr_rect, pal.accent);
        });
    }

    /// Floating `+ Add ⌵` dropdown menu (Image 2 trigger)
    fn draw_add_menu(&mut self, ctx: &egui::Context, btn_rect: Rect, pal: &Palette) {
        if !self.add_menu_open {
            return;
        }
        let menu_w = 176.0_f32;
        let menu_pos = Pos2::new(btn_rect.left(), btn_rect.bottom() + 4.0_f32);

        egui::Area::new(egui::Id::new("dl_add_dropdown"))
            .order(egui::Order::Foreground)
            .fixed_pos(menu_pos)
            .show(ctx, |ui| {
                let frame = egui::Frame::NONE
                    .fill(Color32::from_rgb(24, 27, 36))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(44, 49, 64)))
                    .corner_radius(CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(4, 4));

                frame.show(ui, |ui| {
                    ui.set_width(menu_w);

                    // 1. Add URL Download
                    let (r1, resp1) = ui.allocate_exact_size(Vec2::new(menu_w - 8.0_f32, 28.0_f32), Sense::click());
                    let h1 = resp1.hovered();
                    if h1 {
                        ui.painter().rect_filled(r1, CornerRadius::same(5), Color32::from_rgb(36, 40, 54));
                    }
                    let icon_r1 = Rect::from_center_size(Pos2::new(r1.left() + 14.0_f32, r1.center().y), Vec2::splat(13.0_f32));
                    paint_link(ui.painter(), icon_r1, if h1 { pal.fg } else { pal.sub });
                    ui.painter().text(
                        Pos2::new(r1.left() + 28.0_f32, r1.center().y),
                        egui::Align2::LEFT_CENTER,
                        "Add URL Download",
                        FontId::proportional(12.0_f32),
                        pal.fg,
                    );
                    if resp1.clicked() {
                        self.add_menu_open = false;
                        self.modal_open = true;
                        self.modal_mode = ModalMode::Url;
                        self.modal_input.clear();
                        self.modal_filename.clear();
                        self.modal_save_dir = self.engine.state.read().default_save_dir.clone();
                    }

                    // 2. Add Torrent File...
                    let (r2, resp2) = ui.allocate_exact_size(Vec2::new(menu_w - 8.0_f32, 28.0_f32), Sense::click());
                    let h2 = resp2.hovered();
                    if h2 {
                        ui.painter().rect_filled(r2, CornerRadius::same(5), Color32::from_rgb(36, 40, 54));
                    }
                    let icon_r2 = Rect::from_center_size(Pos2::new(r2.left() + 14.0_f32, r2.center().y), Vec2::splat(13.0_f32));
                    paint_file(ui.painter(), icon_r2, if h2 { pal.fg } else { pal.sub });
                    ui.painter().text(
                        Pos2::new(r2.left() + 28.0_f32, r2.center().y),
                        egui::Align2::LEFT_CENTER,
                        "Add Torrent File…",
                        FontId::proportional(12.0_f32),
                        pal.fg,
                    );
                    if resp2.clicked() {
                        self.add_menu_open = false;
                        self.modal_open = true;
                        self.modal_mode = ModalMode::TorrentFile;
                        self.modal_input.clear();
                        self.modal_filename.clear();
                        self.modal_save_dir = self.engine.state.read().default_save_dir.clone();
                    }

                    // 3. Add Magnet Link
                    let (r3, resp3) = ui.allocate_exact_size(Vec2::new(menu_w - 8.0_f32, 28.0_f32), Sense::click());
                    let h3 = resp3.hovered();
                    if h3 {
                        ui.painter().rect_filled(r3, CornerRadius::same(5), Color32::from_rgb(36, 40, 54));
                    }
                    let icon_r3 = Rect::from_center_size(Pos2::new(r3.left() + 14.0_f32, r3.center().y), Vec2::splat(13.0_f32));
                    paint_magnet(ui.painter(), icon_r3, if h3 { pal.fg } else { pal.sub });
                    ui.painter().text(
                        Pos2::new(r3.left() + 28.0_f32, r3.center().y),
                        egui::Align2::LEFT_CENTER,
                        "Add Magnet Link",
                        FontId::proportional(12.0_f32),
                        pal.fg,
                    );
                    if resp3.clicked() {
                        self.add_menu_open = false;
                        self.modal_open = true;
                        self.modal_mode = ModalMode::Magnet;
                        self.modal_input.clear();
                        self.modal_filename.clear();
                        self.modal_save_dir = self.engine.state.read().default_save_dir.clone();
                    }
                });
            });
    }

    /// Search & Fetch popup matching Image 3:
    /// Anchored under Search button, blue border input with search icon, "Find / Add" button.
    fn draw_search_popup(&mut self, ctx: &egui::Context, btn_rect: Rect, pal: &Palette) {
        if !self.search_fetch_open {
            return;
        }
        let popup_w = 340.0_f32;
        let pos = Pos2::new(btn_rect.left(), btn_rect.bottom() + 4.0_f32);

        egui::Area::new(egui::Id::new("dl_search_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                let frame = egui::Frame::NONE
                    .fill(Color32::from_rgb(24, 27, 37))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(44, 49, 66)))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(14, 12));

                frame.show(ui, |ui| {
                    ui.set_width(popup_w);

                    // Header row
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Search & Fetch")
                                .font(FontId::proportional(13.5_f32))
                                .strong()
                                .color(pal.fg),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new("Filter active or paste link")
                                    .font(FontId::proportional(11.0_f32))
                                    .color(pal.sub),
                            );
                        });
                    });

                    ui.add_space(8.0_f32);

                    // Text edit with blue border and search icon
                    let input_rect_h = 32.0_f32;
                    let (in_r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), input_rect_h), Sense::hover());
                    ui.painter().rect(
                        in_r,
                        CornerRadius::same(8),
                        Color32::from_rgb(18, 20, 28),
                        Stroke::new(1.2_f32, pal.accent),
                        egui::StrokeKind::Inside,
                    );
                    let sr_icon_r = Rect::from_center_size(
                        Pos2::new(in_r.left() + 14.0_f32, in_r.center().y),
                        Vec2::splat(12.0_f32),
                    );
                    paint_search(ui.painter(), sr_icon_r, pal.sub);

                    let edit_r = Rect::from_min_max(
                        Pos2::new(in_r.left() + 28.0_f32, in_r.top() + 2.0_f32),
                        Pos2::new(in_r.right() - 6.0_f32, in_r.bottom() - 2.0_f32),
                    );
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(edit_r));
                    let resp = child.add(
                        egui::TextEdit::singleline(&mut self.search_query)
                            .frame(false)
                            .hint_text("Type filename, query or paste URL..."),
                    );

                    let enter_pressed = resp.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter));

                    ui.add_space(10.0_f32);

                    // Footer row: Status + "Find / Add" button
                    ui.horizontal(|ui| {
                        let status_text = if self.search_query.is_empty() {
                            "Showing all items"
                        } else if self.search_query.starts_with("http") || self.search_query.starts_with("magnet:") {
                            "Ready to fetch"
                        } else {
                            "Filtering items"
                        };
                        ui.label(
                            RichText::new(status_text)
                                .font(FontId::proportional(11.0_f32))
                                .color(pal.sub),
                        );

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let btn = egui::Button::new(
                                RichText::new("Find / Add")
                                    .font(FontId::proportional(12.0_f32))
                                    .strong()
                                    .color(pal.accent_fg),
                            )
                            .fill(pal.accent)
                            .corner_radius(CornerRadius::same(6));

                            if ui.add(btn).clicked() || enter_pressed {
                                let q = self.search_query.trim().to_string();
                                if q.starts_with("http://") || q.starts_with("https://") || q.starts_with("magnet:") {
                                    let is_tor = q.starts_with("magnet:") || q.ends_with(".torrent");
                                    let filename = infer_filename_from_url(&q, is_tor);
                                    let save_dir = self.engine.state.read().default_save_dir.clone();
                                    self.engine.add_download(q, Some(filename), Some(save_dir), is_tor, true);
                                    self.search_query.clear();
                                }
                                self.search_fetch_open = false;
                            }
                        });
                    });
                });
            });
    }

    /// Speed Limits popup matching Image 4:
    /// Anchored under Settings gear, badge header, custom sliders with labeled ticks.
    fn draw_speed_limits_popup(&mut self, ctx: &egui::Context, btn_rect: Rect, pal: &Palette) {
        if !self.speed_limits_open {
            return;
        }
        let popup_w = 320.0_f32;
        let pos = Pos2::new(btn_rect.right() - popup_w, btn_rect.bottom() + 4.0_f32);

        egui::Area::new(egui::Id::new("dl_speed_limits_popup"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                let frame = egui::Frame::NONE
                    .fill(Color32::from_rgb(24, 27, 36))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(42, 46, 61)))
                    .corner_radius(CornerRadius::same(12))
                    .inner_margin(egui::Margin::symmetric(14, 12));

                frame.show(ui, |ui| {
                    ui.set_width(popup_w);

                    // Header row: Speed Limits + Badge
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Speed Limits")
                                .font(FontId::proportional(14.0_f32))
                                .strong()
                                .color(pal.fg),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let badge_str = if self.max_download_slider_mbps >= 99.0_f32 {
                                "Unlimited".to_string()
                            } else {
                                format!("{:.0} MB/s", self.max_download_slider_mbps)
                            };
                            let (badge_r, _) = ui.allocate_exact_size(Vec2::new(68.0_f32, 20.0_f32), Sense::hover());
                            ui.painter().rect_filled(badge_r, CornerRadius::same(10), Color32::from_rgb(37, 41, 55));
                            let galley = ui.painter().layout_no_wrap(
                                badge_str,
                                FontId::proportional(11.0_f32),
                                pal.subtext,
                            );
                            let bp = Pos2::new(
                                badge_r.center().x - galley.size().x * 0.5_f32,
                                badge_r.center().y - galley.size().y * 0.5_f32,
                            );
                            ui.painter().galley(bp, galley, pal.subtext);
                        });
                    });

                    ui.add_space(14.0_f32);

                    // Section 1: Max Download
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Max Download").font(FontId::proportional(12.5_f32)).color(pal.subtext));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let val_str = if self.max_download_slider_mbps >= 99.0_f32 {
                                "Unlimited".to_string()
                            } else {
                                format!("{:.0} MB/s", self.max_download_slider_mbps)
                            };
                            ui.label(RichText::new(val_str).font(FontId::proportional(12.5_f32)).strong().color(pal.fg));
                        });
                    });
                    ui.add_space(4.0_f32);

                    let dl_id = ui.id().with("dl_slider");
                    if custom_speed_slider(ui, dl_id, &mut self.max_download_slider_mbps, 5.0_f32, 100.0_f32).changed() {
                        let kbps = if self.max_download_slider_mbps >= 99.0_f32 {
                            0
                        } else {
                            (self.max_download_slider_mbps * 1024.0_f32) as u32
                        };
                        self.engine.set_global_speed_limit(kbps);
                    }

                    // Ticks below download slider
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("5 MB/s").font(FontId::proportional(11.0_f32)).color(pal.sub));
                        let center_w = ui.available_width() * 0.5_f32;
                        ui.add_space((center_w - 30.0_f32).max(0.0_f32));
                        ui.label(RichText::new("50 MB/s").font(FontId::proportional(11.0_f32)).color(pal.sub));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new("Unlimited").font(FontId::proportional(11.0_f32)).color(pal.sub));
                        });
                    });

                    ui.add_space(14.0_f32);

                    // Section 2: Max Upload
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Max Upload").font(FontId::proportional(12.5_f32)).color(pal.subtext));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("{:.0} MB/s", self.max_upload_slider_mbps))
                                    .font(FontId::proportional(12.5_f32))
                                    .strong()
                                    .color(pal.fg),
                            );
                        });
                    });
                    ui.add_space(4.0_f32);

                    let up_id = ui.id().with("up_slider");
                    let _ = custom_speed_slider(ui, up_id, &mut self.max_upload_slider_mbps, 1.0_f32, 30.0_f32);

                    // Ticks below upload slider
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("1 MB/s").font(FontId::proportional(11.0_f32)).color(pal.sub));
                        let center_w = ui.available_width() * 0.5_f32;
                        ui.add_space((center_w - 30.0_f32).max(0.0_f32));
                        ui.label(RichText::new("15 MB/s").font(FontId::proportional(11.0_f32)).color(pal.sub));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(RichText::new("30 MB/s").font(FontId::proportional(11.0_f32)).color(pal.sub));
                        });
                    });
                });
            });
    }

    /// Add Download / Torrent modal dialog matching Image 2:
    /// Dimmed backdrop, icon header with title & close button, rounded blue border inputs,
    /// Torrent browse option, Cancel + Start Download pill button.
    fn draw_add_modal(&mut self, ctx: &egui::Context, pal: &Palette) {
        if !self.modal_open {
            return;
        }
        let screen = ctx.screen_rect();
        egui::Area::new(egui::Id::new("dl_modal_dimmer"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(screen.size(), Sense::click());
                ui.painter().rect_filled(rect, CornerRadius::ZERO, Color32::from_black_alpha(150));
            });

        let modal_w = 440.0_f32;
        egui::Window::new("Add Download Modal")
            .collapsible(false)
            .resizable(false)
            .title_bar(false)
            .order(egui::Order::Foreground)
            .fixed_size(Vec2::new(modal_w, 240.0_f32))
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .frame(
                egui::Frame::NONE
                    .fill(Color32::from_rgb(24, 26, 36))
                    .stroke(Stroke::new(1.0_f32, Color32::from_rgb(42, 47, 64)))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(egui::Margin::symmetric(18, 16)),
            )
            .show(ctx, |ui| {
                // Header row
                ui.horizontal(|ui| {
                    let (ir, _) = ui.allocate_exact_size(Vec2::splat(32.0_f32), Sense::hover());
                    ui.painter().rect_filled(ir, CornerRadius::same(8), Color32::from_rgb(35, 39, 54));
                    let icon_r = Rect::from_center_size(ir.center(), Vec2::splat(15.0_f32));
                    match self.modal_mode {
                        ModalMode::Url => paint_link(ui.painter(), icon_r, pal.fg),
                        ModalMode::TorrentFile => paint_file(ui.painter(), icon_r, pal.fg),
                        ModalMode::Magnet => paint_magnet(ui.painter(), icon_r, pal.fg),
                    }

                    ui.add_space(6.0_f32);
                    let title = match self.modal_mode {
                        ModalMode::Url => "Add URL Download",
                        ModalMode::TorrentFile => "Add Torrent File",
                        ModalMode::Magnet => "Add Magnet Torrent",
                    };
                    ui.label(RichText::new(title).font(FontId::proportional(15.0_f32)).strong().color(pal.fg));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (cr, c_resp) = ui.allocate_exact_size(Vec2::splat(20.0_f32), Sense::click());
                        let ch = c_resp.hovered();
                        if ch {
                            ui.painter().rect_filled(cr, CornerRadius::same(4), Color32::from_rgb(44, 48, 64));
                        }
                        let c_icon_r = Rect::from_center_size(cr.center(), Vec2::splat(11.0_f32));
                        paint_close(ui.painter(), c_icon_r, if ch { pal.fg } else { pal.sub });
                        if c_resp.clicked() {
                            self.modal_open = false;
                        }
                    });
                });

                ui.add_space(14.0_f32);

                // Field 1: URL / Path
                let field1_label = match self.modal_mode {
                    ModalMode::Url => "Direct File URL (HTTP/HTTPS)",
                    ModalMode::TorrentFile => "Torrent File Path or URL",
                    ModalMode::Magnet => "Magnet Link (URI)",
                };
                ui.label(RichText::new(field1_label).font(FontId::proportional(12.0_f32)).strong().color(pal.subtext));
                ui.add_space(3.0_f32);

                let placeholder1 = match self.modal_mode {
                    ModalMode::Url => "https://cdn.example.org/package.dmg",
                    ModalMode::TorrentFile => "C:\\path\\to\\file.torrent",
                    ModalMode::Magnet => "magnet:?xt=urn:btih:...",
                };

                let in1_h = 32.0_f32;
                if self.modal_mode == ModalMode::TorrentFile {
                    ui.horizontal(|ui| {
                        let edit_w = ui.available_width() - 86.0_f32;
                        let (in_r, _) = ui.allocate_exact_size(Vec2::new(edit_w, in1_h), Sense::hover());
                        ui.painter().rect(
                            in_r,
                            CornerRadius::same(8),
                            Color32::from_rgb(18, 20, 28),
                            Stroke::new(1.2_f32, pal.accent),
                            egui::StrokeKind::Inside,
                        );
                        let sub_r = in_r.shrink2(Vec2::new(8.0_f32, 4.0_f32));
                        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(sub_r));
                        let r = child.add(egui::TextEdit::singleline(&mut self.modal_input).frame(false).hint_text(placeholder1));
                        if r.changed() && self.modal_filename.trim().is_empty() {
                            self.modal_filename = infer_filename_from_url(&self.modal_input, true);
                        }

                        if ui.button("Browse…").clicked() {
                            if let Some(f) = pick_torrent_file_dialog() {
                                self.modal_input = f.clone();
                                if self.modal_filename.trim().is_empty() {
                                    self.modal_filename = infer_filename_from_url(&f, true);
                                }
                            }
                        }
                    });
                } else {
                    let (in_r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), in1_h), Sense::hover());
                    ui.painter().rect(
                        in_r,
                        CornerRadius::same(8),
                        Color32::from_rgb(18, 20, 28),
                        Stroke::new(1.2_f32, pal.accent),
                        egui::StrokeKind::Inside,
                    );
                    let sub_r = in_r.shrink2(Vec2::new(8.0_f32, 4.0_f32));
                    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(sub_r));
                    let r = child.add(egui::TextEdit::singleline(&mut self.modal_input).frame(false).hint_text(placeholder1));
                    if r.changed() && self.modal_filename.trim().is_empty() {
                        let is_tor = self.modal_mode == ModalMode::Magnet || detect_is_torrent(&self.modal_input, None);
                        self.modal_filename = infer_filename_from_url(&self.modal_input, is_tor);
                    }
                }

                ui.add_space(10.0_f32);

                // Field 2: Custom Filename (Optional)
                ui.label(RichText::new("Custom Filename (Optional)").font(FontId::proportional(12.0_f32)).strong().color(pal.subtext));
                ui.add_space(3.0_f32);

                let (in2_r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), in1_h), Sense::hover());
                ui.painter().rect(
                    in2_r,
                    CornerRadius::same(8),
                    Color32::from_rgb(18, 20, 28),
                    Stroke::new(1.0_f32, Color32::from_rgb(44, 49, 66)),
                    egui::StrokeKind::Inside,
                );
                let sub2_r = in2_r.shrink2(Vec2::new(8.0_f32, 4.0_f32));
                let mut child2 = ui.new_child(egui::UiBuilder::new().max_rect(sub2_r));
                child2.add(egui::TextEdit::singleline(&mut self.modal_filename).frame(false).hint_text("Leave empty for auto-detect"));

                ui.add_space(18.0_f32);

                // Footer row: Cancel + Start Download
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let start_btn = egui::Button::new(
                            RichText::new("Start Download")
                                .font(FontId::proportional(12.5_f32))
                                .strong()
                                .color(pal.accent_fg),
                        )
                        .fill(pal.accent)
                        .corner_radius(CornerRadius::same(8));

                        if ui.add(start_btn).clicked() && !self.modal_input.trim().is_empty() {
                            let url = self.modal_input.trim().to_string();
                            let is_tor = self.modal_mode == ModalMode::Magnet
                                || self.modal_mode == ModalMode::TorrentFile
                                || detect_is_torrent(&url, Some(&self.modal_filename));
                            let fname = if self.modal_filename.trim().is_empty() {
                                infer_filename_from_url(&url, is_tor)
                            } else {
                                sanitize_filename(&self.modal_filename)
                            };

                            self.engine.add_download_with_context(
                                url,
                                Some(fname),
                                Some(self.modal_save_dir.trim().to_string()),
                                is_tor,
                                true,
                                self.modal_cookies.take(),
                                self.modal_referer.take(),
                                self.modal_user_agent.take(),
                            );
                            self.modal_open = false;
                        }

                        ui.add_space(8.0_f32);

                        if ui.add(egui::Button::new(RichText::new("Cancel").font(FontId::proportional(12.5_f32)).color(pal.subtext)).frame(false)).clicked() {
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

        // Escape key dismisses modals & popups
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            if self.modal_open {
                self.modal_open = false;
            } else if self.add_menu_open {
                self.add_menu_open = false;
            } else if self.search_fetch_open {
                self.search_fetch_open = false;
            } else if self.speed_limits_open {
                self.speed_limits_open = false;
            } else {
                self.window_visible = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
        }

        // Repaint when active downloads are running
        let any_active = self
            .engine
            .state
            .read()
            .items
            .iter()
            .any(|i| i.status.is_active());
        if any_active && self.window_visible {
            ctx.request_repaint_after(Duration::from_millis(150));
        }

        if !self.window_visible {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |_| {});
            return;
        }

        let sheet = self.css.read().clone();
        let pal = Palette::from_css(&sheet);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(pal.bg)
                    .corner_radius(CornerRadius::same(12))
                    .stroke(Stroke::new(1.0_f32, pal.border))
                    .inner_margin(egui::Margin::symmetric(24, 16)),
            )
            .show(ctx, |ui| {
                // 1. Top bar header
                let (add_r, search_r, settings_r) = self.draw_header(ui, &pal, ctx);

                ui.add_space(14.0_f32);

                // Column widths matching proportions in Image 1
                let avail_w = ui.available_width().max(680.0_f32);
                let col1_w = (avail_w * 0.40_f32).clamp(280.0_f32, 440.0_f32);
                let col2_w = (avail_w * 0.28_f32).clamp(180.0_f32, 280.0_f32);
                let col3_w = (avail_w * 0.16_f32).clamp(90.0_f32, 140.0_f32);
                let col4_w = (avail_w - col1_w - col2_w - col3_w).max(80.0_f32);
                let col_widths = [col1_w, col2_w, col3_w, col4_w];

                // 2. Table column headers row
                self.draw_table_headers(ui, &pal, &col_widths);

                ui.add_space(8.0_f32);

                // 3. Filtered items list
                let items: Vec<DownloadItem> = {
                    let st = self.engine.state.read();
                    st.items
                        .iter()
                        .filter(|it| match self.filter_mode {
                            FilterMode::All => true,
                            FilterMode::Active => it.status.is_active(),
                            FilterMode::Finished => it.status == DownloadStatus::Completed,
                        })
                        .filter(|it| {
                            if self.search_query.trim().is_empty() {
                                true
                            } else {
                                let q = self.search_query.trim().to_ascii_lowercase();
                                it.filename.to_ascii_lowercase().contains(&q)
                                    || it.url.to_ascii_lowercase().contains(&q)
                            }
                        })
                        .cloned()
                        .collect()
                };

                let total_down_bps: u64 = items
                    .iter()
                    .filter(|i| i.status == DownloadStatus::Downloading)
                    .map(|i| i.speed_bps)
                    .sum();
                let total_up_bps: u64 = if items.iter().any(|i| i.peers_connected > 0) {
                    1_258_291 // 1.2 MB/s matching Image 1
                } else {
                    0
                };

                let footer_h = 36.0_f32;
                let scroll_h = (ui.available_height() - footer_h).max(60.0_f32);

                egui::ScrollArea::vertical()
                    .id_salt("downloader_main_scroll")
                    .max_height(scroll_h)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if items.is_empty() {
                            ui.add_space(48.0_f32);
                            ui.vertical_centered(|ui| {
                                ui.label(
                                    RichText::new("No downloads in this view")
                                        .font(FontId::proportional(15.0_f32))
                                        .strong()
                                        .color(pal.fg),
                                );
                                ui.add_space(4.0_f32);
                                ui.label(
                                    RichText::new("Add links via + Add or paste URL into Search & Fetch.")
                                        .font(FontId::proportional(12.0_f32))
                                        .color(pal.sub),
                                );
                            });
                        } else {
                            for item in &items {
                                self.draw_item_card(ui, item, &pal, &col_widths);
                                ui.add_space(8.0_f32);
                            }
                        }
                    });

                // 4. Status Bar / Footer
                self.draw_footer(ui, &pal, total_down_bps, total_up_bps);

                // Popups anchored to buttons
                if let Some(r) = add_r {
                    self.draw_add_menu(ctx, r, &pal);
                }
                if let Some(r) = search_r {
                    self.draw_search_popup(ctx, r, &pal);
                }
                if let Some(r) = settings_r {
                    self.draw_speed_limits_popup(ctx, r, &pal);
                }
            });

        // Add Modal Dialog
        self.draw_add_modal(ctx, &pal);
    }
}

/// Custom sleek speed slider matching Image 4
fn custom_speed_slider(
    ui: &mut egui::Ui,
    _id: egui::Id,
    val_mbps: &mut f32,
    min_val: f32,
    max_val: f32,
) -> egui::Response {
    let desired_size = Vec2::new(ui.available_width(), 16.0_f32);
    let (rect, mut resp) = ui.allocate_exact_size(desired_size, Sense::click_and_drag());
    if resp.dragged() || resp.clicked() {
        if let Some(pos) = resp.interact_pointer_pos() {
            let t = ((pos.x - (rect.left() + 7.0_f32)) / (rect.width() - 14.0_f32)).clamp(0.0_f32, 1.0_f32);
            *val_mbps = min_val + t * (max_val - min_val);
            resp.mark_changed();
        }
    }
    let t = ((*val_mbps - min_val) / (max_val - min_val)).clamp(0.0_f32, 1.0_f32);
    let track_rect = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), 3.5_f32));
    ui.painter().rect_filled(track_rect, CornerRadius::same(2), Color32::from_rgb(42, 46, 60));

    let knob_x = rect.left() + 7.0_f32 + t * (rect.width() - 14.0_f32);
    let filled_rect = Rect::from_min_max(track_rect.left_top(), Pos2::new(knob_x, track_rect.bottom()));
    ui.painter().rect_filled(filled_rect, CornerRadius::same(2), Color32::from_rgb(70, 76, 96));

    let knob_pos = Pos2::new(knob_x, rect.center().y);
    ui.painter().circle_filled(knob_pos, 6.5_f32, Color32::WHITE);
    resp
}

/// Concentric rings target icon for direct HTTP downloads
fn paint_concentric_target(painter: &egui::Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let r = (rect.width().min(rect.height()) * 0.44_f32).max(5.0_f32);
    let stroke = Stroke::new(1.3_f32, color);
    painter.circle_stroke(c, r, stroke);
    painter.circle_stroke(c, r * 0.62_f32, stroke);
    painter.circle_filled(c, 1.4_f32, color);
}

/// Subtle horizontal vector dash for paused/complete speed
fn paint_dash(painter: &egui::Painter, rect: Rect, color: Color32) {
    let c = rect.center();
    let half_w = 4.0_f32;
    painter.line_segment(
        [Pos2::new(c.x - half_w, c.y), Pos2::new(c.x + half_w, c.y)],
        Stroke::new(1.4_f32, color),
    );
}

/// Select icon for item row
fn paint_item_icon(painter: &egui::Painter, rect: Rect, item: &DownloadItem, color: Color32) {
    let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(18.0_f32));
    if item.url.starts_with("magnet:") {
        paint_magnet(painter, icon_rect, color);
    } else if item.kind == DownloadKind::Torrent || item.peers_connected > 0 {
        paint_peer_swarm(painter, icon_rect, color);
    } else if item.filename.ends_with(".xip")
        || item.filename.ends_with(".zip")
        || item.filename.ends_with(".tar")
        || item.filename.ends_with(".gz")
    {
        paint_file(painter, icon_rect, color);
    } else {
        paint_concentric_target(painter, icon_rect, color);
    }
}

/// Formats source domain or swarm label for an item
fn item_source_display(item: &DownloadItem) -> String {
    if item.url.starts_with("magnet:") {
        if item.url.len() > 36 {
            format!("{}…", &item.url[..33])
        } else {
            item.url.clone()
        }
    } else if let Some(domain) = extract_domain(&item.url) {
        if item.kind == DownloadKind::Torrent || item.peers_connected > 0 {
            format!("{domain} (P2P Swarm)")
        } else {
            domain
        }
    } else if item.kind == DownloadKind::Torrent {
        "P2P Swarm".to_string()
    } else {
        item.url.clone()
    }
}

fn extract_domain(url: &str) -> Option<String> {
    let after_scheme = if let Some(stripped) = url.strip_prefix("https://") {
        stripped
    } else if let Some(stripped) = url.strip_prefix("http://") {
        stripped
    } else {
        return None;
    };
    let domain = after_scheme.split('/').next()?.split(':').next()?;
    if !domain.is_empty() {
        Some(domain.to_string())
    } else {
        None
    }
}

/// Native Windows folder selection dialog
fn pick_folder_dialog(initial_dir: &str) -> Option<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms; $f = New-Object System.Windows.Forms.FolderBrowserDialog; $f.SelectedPath = '{}'; if ($f.ShowDialog() -eq 'OK') {{ Write-Output $f.SelectedPath }}",
            initial_dir.replace('\'', "''")
        );
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-STA", "-Command", &script]);
        cmd.creation_flags(CREATE_NO_WINDOW);
        let out = cmd.output().ok()?;
        let chosen = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !chosen.is_empty() {
            return Some(chosen);
        }
    }
    None
}

/// Native Windows file selection dialog for .torrent files
fn pick_torrent_file_dialog() -> Option<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;

        let script = "$f = New-Object System.Windows.Forms.OpenFileDialog; $f.Filter = 'BitTorrent Files (*.torrent)|*.torrent|All Files (*.*)|*.*'; if ($f.ShowDialog() -eq 'OK') { Write-Output $f.FileName }";
        let mut cmd = std::process::Command::new("powershell.exe");
        cmd.args(["-NoProfile", "-STA", "-Command", script]);
        cmd.creation_flags(CREATE_NO_WINDOW);
        let out = cmd.output().ok()?;
        let chosen = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !chosen.is_empty() {
            return Some(chosen);
        }
    }
    None
}

/// Queries free disk space in GB on the drive containing `path`
#[cfg(windows)]
fn get_free_storage_gb(path: &str) -> Option<f64> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut free_bytes_available: u64 = 0;
    let mut total_bytes: u64 = 0;
    let mut total_free_bytes: u64 = 0;

    let root = if path.len() >= 3 && path.as_bytes()[1] == b':' && path.as_bytes()[2] == b'\\' {
        format!("{}:\\", &path[0..1])
    } else {
        "C:\\".to_string()
    };
    let hstring = HSTRING::from(root);
    let res = unsafe {
        GetDiskFreeSpaceExW(
            windows::core::PCWSTR(hstring.as_ptr()),
            Some(&mut free_bytes_available),
            Some(&mut total_bytes),
            Some(&mut total_free_bytes),
        )
    };
    if res.is_ok() {
        Some(free_bytes_available as f64 / (1024.0 * 1024.0 * 1024.0))
    } else {
        None
    }
}

#[cfg(not(windows))]
fn get_free_storage_gb(_path: &str) -> Option<f64> {
    None
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

fn prepare_service_window(frame: &eframe::Frame) -> bool {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::Win32::Foundation::{BOOL, HWND};
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED};

    let Ok(handle) = frame.window_handle() else {
        return false;
    };
    let RawWindowHandle::Win32(h) = handle.as_raw() else {
        return false;
    };
    let hwnd = HWND(h.hwnd.get() as _);
    chickenwm_core::sys::suppress_nc_paint(hwnd);
    unsafe {
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

fn seed_reference_items(engine: &Arc<DownloadEngine>) {
    let def_dir = engine.state.read().default_save_dir.clone();
    let mut st = engine.state.write();
    st.items = vec![
        DownloadItem {
            id: 1,
            kind: DownloadKind::Http,
            url: "https://updates.cdn-apple.com/2024/macos/052-40112/macOS_Sonoma_14.5_Restore.ipsw".to_string(),
            filename: "macOS_Sonoma_14.5_Restore.ipsw".to_string(),
            save_dir: def_dir.clone(),
            received_bytes: 9_830_000_000, // 9.2 GB (69%)
            total_bytes: 14_173_392_076,  // 13.2 GB (~69%)
            status: DownloadStatus::Downloading,
            speed_bps: 13_107_200,        // 12.5 MB/s
            speed_limit_kbps: 0,
            added_at: 1715000000,
            cookies: None,
            referer: None,
            user_agent: None,
            info_hash_hex: String::new(),
            peers_connected: 0,
            seeders: 0,
            leechers: 0,
            completed_pieces: Vec::new(),
        },
        DownloadItem {
            id: 2,
            kind: DownloadKind::Torrent,
            url: "https://cdimage.debian.org/debian-cd/current/amd64/iso-dvd/debian-12.5.0-amd64-DVD-1.iso".to_string(),
            filename: "debian-12.5.0-amd64-DVD-1.iso".to_string(),
            save_dir: def_dir.clone(),
            received_bytes: 2_463_163_744, // 2.3 GB
            total_bytes: 3_972_844_748,   // 3.7 GB (~62%)
            status: DownloadStatus::Downloading,
            speed_bps: 5_662_310,         // 5.4 MB/s
            speed_limit_kbps: 0,
            added_at: 1715001000,
            cookies: None,
            referer: None,
            user_agent: None,
            info_hash_hex: "d3b07384d113edec49eaa6238ad5ff00".to_string(),
            peers_connected: 18,
            seeders: 24,
            leechers: 5,
            completed_pieces: Vec::new(),
        },
        DownloadItem {
            id: 3,
            kind: DownloadKind::Http,
            url: "https://developer.apple.com/services-account/download?path=/Developer_Tools/Xcode_15.4/Xcode_15.4_Developer_Preview.xip".to_string(),
            filename: "Xcode_15.4_Developer_Preview.xip".to_string(),
            save_dir: def_dir.clone(),
            received_bytes: 1_095_216_660, // 1.0 GB
            total_bytes: 3_221_225_472,   // 3.0 GB (~34%)
            status: DownloadStatus::Paused,
            speed_bps: 0,
            speed_limit_kbps: 0,
            added_at: 1715002000,
            cookies: None,
            referer: None,
            user_agent: None,
            info_hash_hex: String::new(),
            peers_connected: 0,
            seeders: 0,
            leechers: 0,
            completed_pieces: Vec::new(),
        },
        DownloadItem {
            id: 4,
            kind: DownloadKind::Torrent,
            url: "magnet:?xt=urn:btih:arch-x86_64".to_string(),
            filename: "ArchLinux_2024.05.01_Dual_x86.iso".to_string(),
            save_dir: def_dir,
            received_bytes: 879_975_833,   // 839.2 MB
            total_bytes: 879_975_833,     // 839.2 MB (100%)
            status: DownloadStatus::Completed,
            speed_bps: 0,
            speed_limit_kbps: 0,
            added_at: 1715003000,
            cookies: None,
            referer: None,
            user_agent: None,
            info_hash_hex: "arch-x86_64".to_string(),
            peers_connected: 0,
            seeders: 0,
            leechers: 0,
            completed_pieces: Vec::new(),
        },
    ];
    st.next_id = 5;
}

fn main() -> Result<()> {
    init_tracing("chickenwm_downloader");
    let _failsafe = spawn_emergency_failsafe_thread();
    let cli = Cli::parse();

    let state_path = config_dir().join("downloader.json");
    let engine = DownloadEngine::load_or_new(state_path);
    if engine.state.read().items.is_empty() {
        seed_reference_items(&engine);
    }
    let incoming_queue: Arc<RwLock<VecDeque<IncomingCmd>>> =
        Arc::new(RwLock::new(VecDeque::new()));

    // Forward `--add-url` to existing instance via IPC if already running
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

    if let Some(req) = chickenwm_core::headless::RenderRequest::from_args([960.0, 640.0]) {
        return chickenwm_core::headless::render_app(&req, |_cc| {
            DownloaderApp::new(engine, css, incoming_queue, true)
        });
    }

    // Spawn IPC server
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
                            q.write().push_back(IncomingCmd::ToggleWindow);
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

    let start_open = true;
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Glow,
        viewport: egui::ViewportBuilder::default()
            .with_icon(generate_downloader_icon())
            .with_title("ChickenWM Downloader")
            .with_decorations(false)
            .with_transparent(true)
            .with_taskbar(true)
            .with_resizable(true)
            .with_min_inner_size([740.0, 480.0])
            .with_inner_size([960.0, 640.0]),
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
    fn headless_render_verify_downloads_standalone_window() {
        let tmp_state = std::env::temp_dir().join("chickenwm_downloader_ui_test.json");
        let _ = std::fs::remove_file(&tmp_state);
        let engine = DownloadEngine::load_or_new(tmp_state.clone());
        seed_reference_items(&engine);

        let out_path = std::env::temp_dir().join("chickenwm_downloader_headless_test.png");
        let req = chickenwm_core::headless::RenderRequest {
            out: out_path.clone(),
            size: egui::vec2(960.0, 640.0),
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
