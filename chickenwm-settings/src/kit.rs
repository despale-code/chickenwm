//! Small widget kit for the macOS-style settings layout: grouped "inset" cards made of rows
//! (label left, control right, hairlines between), toggle switches, segmented controls,
//! sidebar items with colored icon tiles, and page titles. Colors come from `settings.css`.

use chickenwm_core::config::RgbaColor;
use chickenwm_core::css::CssStyleSheet;
use eframe::egui::{self, Align, Color32, CornerRadius, FontId, Layout, Rect, Response, RichText, Sense, Stroke, Vec2};

pub fn c32(c: RgbaColor) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

/// The palette the settings window draws with.
#[derive(Clone, Copy)]
pub struct Kit {
    pub bg: Color32,
    pub sidebar: Color32,
    pub card: Color32,
    pub fg: Color32,
    pub sub: Color32,
    pub accent: Color32,
    pub accent_fg: Color32,
    pub border: Color32,
    pub danger: Color32,
    pub light: bool,
}

impl Kit {
    pub fn from_css(sheet: &CssStyleSheet) -> Self {
        let v = |n: &str, d: RgbaColor| c32(sheet.var_color(n, d));
        let bg = sheet.var_color("--bg-color", RgbaColor { r: 24, g: 24, b: 37, a: 255 });
        let accent = v("--accent-color", RgbaColor { r: 51, g: 204, b: 255, a: 255 });
        Self {
            bg: c32(bg),
            sidebar: v("--sidebar-bg", bg),
            card: v("--card-bg", bg),
            fg: v("--fg-color", RgbaColor { r: 205, g: 214, b: 244, a: 255 }),
            sub: v("--subtext-color", RgbaColor { r: 166, g: 173, b: 200, a: 255 }),
            accent,
            accent_fg: v("--accent-fg", bg),
            border: v("--border-color", RgbaColor { r: 49, g: 50, b: 68, a: 255 }),
            danger: v("--danger-color", RgbaColor { r: 230, g: 80, b: 90, a: 255 }),
            light: chickenwm_core::theme::luminance(bg) > 0.45,
        }
    }

    /// Hairline separators between rows.
    pub fn hairline(&self) -> Color32 {
        self.border.gamma_multiply(0.8)
    }
}

/// Large page title with an optional one-line description.
pub fn page_title(ui: &mut egui::Ui, kit: &Kit, title: &str, subtitle: &str) {
    ui.add_space(4.0);
    ui.label(RichText::new(title).size(24.0).strong().color(kit.fg));
    if !subtitle.is_empty() {
        ui.add_space(2.0);
        ui.label(RichText::new(subtitle).size(13.0).color(kit.sub));
    }
    ui.add_space(14.0);
}

/// A grouped card. `title` is drawn above it in small secondary text, like macOS.
pub fn group<R>(ui: &mut egui::Ui, kit: &Kit, title: &str, add: impl FnOnce(&mut Group) -> R) -> R {
    if !title.is_empty() {
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(RichText::new(title).size(12.5).strong().color(kit.sub));
        });
        ui.add_space(3.0);
    }
    let out = egui::Frame::NONE
        .fill(kit.card)
        .stroke(Stroke::new(1.0_f32, kit.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(14, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let mut g = Group { ui, kit: *kit, rows: 0 };
            add(&mut g)
        })
        .inner;
    ui.add_space(16.0);
    out
}

pub struct Group<'a> {
    pub ui: &'a mut egui::Ui,
    pub kit: Kit,
    rows: usize,
}

impl Group<'_> {
    fn separator(&mut self) {
        if self.rows > 0 {
            let r = self.ui.available_rect_before_wrap();
            let y = r.top();
            self.ui.painter().hline(r.left()..=r.right() + 14.0, y, Stroke::new(1.0_f32, self.kit.hairline()));
        }
        self.rows += 1;
    }

    /// A row: `label` (and an optional hint under it) on the left, `control` right-aligned.
    pub fn row<R>(&mut self, label: &str, hint: &str, control: impl FnOnce(&mut egui::Ui) -> R) -> R {
        self.separator();
        let kit = self.kit;
        let min_h = if hint.is_empty() { 40.0 } else { 52.0 };
        self.ui
            .allocate_ui_with_layout(Vec2::new(self.ui.available_width(), min_h), Layout::left_to_right(Align::Center), |ui| {
                ui.set_min_height(min_h);
                let text_w = (ui.available_width() * 0.55).max(160.0);
                ui.allocate_ui_with_layout(Vec2::new(text_w, min_h), Layout::top_down(Align::Min), |ui| {
                    ui.add_space(if hint.is_empty() { (min_h - 18.0) / 2.0 } else { 9.0 });
                    ui.label(RichText::new(label).size(14.0).color(kit.fg));
                    if !hint.is_empty() {
                        ui.add(egui::Label::new(RichText::new(hint).size(12.0).color(kit.sub)).wrap());
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), control).inner
            })
            .inner
    }

    /// Free-form content in its own row (lists, previews, editors).
    pub fn custom<R>(&mut self, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
        self.separator();
        self.ui.add_space(8.0);
        let r = add(self.ui);
        self.ui.add_space(8.0);
        r
    }
}

/// iOS / macOS style switch.
pub fn toggle(ui: &mut egui::Ui, kit: &Kit, on: &mut bool) -> Response {
    let size = Vec2::new(38.0, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.14);
    let off = if kit.light { kit.fg.gamma_multiply(0.16) } else { kit.fg.gamma_multiply(0.22) };
    let track = off.lerp_to_gamma(kit.accent, t);
    ui.painter().rect_filled(rect, CornerRadius::same(255), track);
    let r = rect.height() / 2.0 - 2.0;
    let x = egui::lerp(rect.left() + r + 2.0..=rect.right() - r - 2.0, t);
    let knob = egui::pos2(x, rect.center().y);
    ui.painter().circle_filled(knob + Vec2::new(0.0, 0.8), r, Color32::from_black_alpha(40));
    ui.painter().circle_filled(knob, r, Color32::WHITE);
    resp
}

/// Segmented control. Returns `true` when the selection changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut egui::Ui, kit: &Kit, value: &mut T, options: &[(T, &str)]) -> bool {
    let font = FontId::proportional(13.0);
    let pad = 12.0;
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, l)| ui.painter().layout_no_wrap(l.to_string(), font.clone(), kit.fg).size().x + pad * 2.0)
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 4.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total, 26.0), Sense::hover());
    let painter = ui.painter();
    let well = if kit.light { kit.fg.gamma_multiply(0.07) } else { kit.fg.gamma_multiply(0.10) };
    painter.rect_filled(rect, CornerRadius::same(7), well);
    let mut changed = false;
    let mut x = rect.left() + 2.0;
    let selected_idx = options.iter().position(|(v, _)| v == value).unwrap_or(0);
    // The selected pill slides between segments.
    let target_x = rect.left() + 2.0 + widths[..selected_idx].iter().sum::<f32>();
    let id = ui.id().with(("segmented", rect.left() as i32, rect.top() as i32));
    let px = ui.ctx().animate_value_with_time(id, target_x, 0.12);
    let pill = Rect::from_min_size(egui::pos2(px, rect.top() + 2.0), Vec2::new(widths[selected_idx], rect.height() - 4.0));
    painter.rect_filled(pill.translate(Vec2::new(0.0, 0.5)), CornerRadius::same(6), Color32::from_black_alpha(if kit.light { 22 } else { 60 }));
    painter.rect_filled(pill, CornerRadius::same(6), if kit.light { Color32::WHITE } else { kit.card.lerp_to_gamma(kit.fg, 0.12) });
    for (i, ((v, label), w)) in options.iter().zip(&widths).enumerate() {
        let seg = Rect::from_min_size(egui::pos2(x, rect.top()), Vec2::new(*w, rect.height()));
        let resp = ui.interact(seg, id.with(i), Sense::click());
        if resp.clicked() && v != value {
            *value = *v;
            changed = true;
        }
        let color = if i == selected_idx { kit.fg } else { kit.sub };
        ui.painter().text(seg.center(), egui::Align2::CENTER_CENTER, *label, font.clone(), color);
        x += w;
    }
    changed
}

/// A rounded button in the accent color (primary action) or neutral.
pub fn button(ui: &mut egui::Ui, kit: &Kit, label: &str, primary: bool) -> Response {
    let (fill, text) = if primary { (kit.accent, kit.accent_fg) } else { (kit.fg.gamma_multiply(if kit.light { 0.08 } else { 0.12 }), kit.fg) };
    ui.add(
        egui::Button::new(RichText::new(label).size(13.0).color(text))
            .fill(fill)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(7))
            .min_size(Vec2::new(0.0, 26.0)),
    )
}

/// Sidebar entry: colored icon tile + label; the selected one is filled with the accent.
pub fn sidebar_item(ui: &mut egui::Ui, kit: &Kit, glyph: &str, tile: Color32, label: &str, selected: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::same(7), kit.accent);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(7), kit.fg.gamma_multiply(0.07));
    }
    let icon = Rect::from_center_size(egui::pos2(rect.left() + 20.0, rect.center().y), Vec2::splat(22.0));
    painter.rect_filled(icon, CornerRadius::same(6), tile);
    painter.text(icon.center(), egui::Align2::CENTER_CENTER, glyph, FontId::proportional(13.0), Color32::WHITE);
    painter.text(
        egui::pos2(icon.right() + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        if selected { kit.accent_fg } else { kit.fg },
    );
    resp.clicked()
}

/// Rounded search field.
pub fn search_field(ui: &mut egui::Ui, kit: &Kit, text: &mut String, hint: &str) -> Response {
    egui::Frame::NONE
        .fill(kit.fg.gamma_multiply(if kit.light { 0.06 } else { 0.09 }))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(8, 5))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("🔍").size(12.0).color(kit.sub));
                ui.add(
                    egui::TextEdit::singleline(text)
                        .hint_text(RichText::new(hint).color(kit.sub))
                        .frame(false)
                        .desired_width(f32::INFINITY),
                )
            })
            .inner
        })
        .inner
}

/// Small rounded label ("chip").
pub fn chip(ui: &mut egui::Ui, kit: &Kit, text: &str, color: Color32) {
    egui::Frame::NONE
        .fill(color.gamma_multiply(0.16))
        .corner_radius(CornerRadius::same(255))
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| ui.label(RichText::new(text).size(11.5).color(if kit.light { color } else { color.lerp_to_gamma(Color32::WHITE, 0.3) })));
}

/// Keyboard key cap, for keybind lists.
pub fn keycap(ui: &mut egui::Ui, kit: &Kit, text: &str) {
    egui::Frame::NONE
        .fill(kit.bg)
        .stroke(Stroke::new(1.0_f32, kit.border))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| ui.label(RichText::new(text).size(12.0).monospace().color(kit.fg)));
}

/// egui visuals for the whole window (fields, sliders, combo boxes, scroll bars).
pub fn apply_visuals(ctx: &egui::Context, kit: &Kit) {
    let mut vis = if kit.light { egui::Visuals::light() } else { egui::Visuals::dark() };
    vis.panel_fill = kit.bg;
    vis.window_fill = kit.card;
    vis.extreme_bg_color = if kit.light { Color32::WHITE } else { kit.bg };
    vis.faint_bg_color = kit.sidebar;
    vis.override_text_color = Some(kit.fg);
    vis.hyperlink_color = kit.accent;
    vis.selection.bg_fill = kit.accent.gamma_multiply(0.35);
    vis.selection.stroke = Stroke::new(1.0_f32, kit.accent);
    vis.window_stroke = Stroke::new(1.0_f32, kit.border);
    vis.window_corner_radius = CornerRadius::same(10);
    vis.menu_corner_radius = CornerRadius::same(8);
    vis.slider_trailing_fill = true;
    let field = kit.fg.gamma_multiply(if kit.light { 0.06 } else { 0.10 });
    for (w, fill) in [
        (&mut vis.widgets.noninteractive, kit.card),
        (&mut vis.widgets.inactive, field),
        (&mut vis.widgets.hovered, kit.fg.gamma_multiply(if kit.light { 0.10 } else { 0.15 })),
        (&mut vis.widgets.active, kit.accent),
        (&mut vis.widgets.open, field),
    ] {
        w.bg_fill = fill;
        w.weak_bg_fill = fill;
        w.corner_radius = CornerRadius::same(7);
        w.bg_stroke = Stroke::NONE;
        w.expansion = 0.0;
    }
    vis.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, kit.border);
    vis.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, kit.fg);
    vis.widgets.inactive.fg_stroke = Stroke::new(1.0_f32, kit.fg);
    vis.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, kit.fg);
    vis.widgets.active.fg_stroke = Stroke::new(1.0_f32, kit.accent_fg);
    ctx.set_visuals(vis);
    ctx.style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 6.0);
        s.spacing.button_padding = Vec2::new(10.0, 4.0);
        s.spacing.slider_width = 180.0;
        s.spacing.interact_size.y = 24.0;
        s.spacing.scroll.bar_width = 6.0;
        s.spacing.scroll.floating = true;
    });
}
