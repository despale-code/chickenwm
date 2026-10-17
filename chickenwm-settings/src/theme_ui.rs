//! Appearance page: the theme gallery (ChickenWM + popular presets), light/dark, accent, icon
//! theme and per-app modes. Changes apply to the whole system right away, except the accent,
//! which applies when you press Apply (so dragging the color picker doesn't rewrite every
//! stylesheet on each frame).

use crate::kit::{self, c32, Kit};
use chickenwm_core::config::RgbaColor;
use chickenwm_core::icon_theme::{available_themes, icons_root, IconTheme, CATEGORIES, SPECIAL_FOLDERS};
use chickenwm_core::theme::{css, AppMode, Palette, ThemeConfig, ThemeMode, DEFAULT_ACCENT};
use chickenwm_core::theme_presets::PRESETS;
use eframe::egui::{self, CornerRadius, FontId, Rect, Sense, Stroke, Vec2};

pub struct ThemeEditor {
    pub draft: ThemeConfig,
    hex: String,
    icon_themes: Vec<String>,
    /// (icon theme name, textures) — reloaded when the theme or palette changes.
    icons: Option<(String, Vec<(String, egui::TextureHandle)>)>,
}

impl ThemeEditor {
    pub fn new(current: &ThemeConfig) -> Self {
        Self { hex: css(current.accent), draft: current.clone(), icon_themes: available_themes(), icons: None }
    }

    pub fn invalidate_icons(&mut self) {
        self.icons = None;
        self.icon_themes = available_themes();
    }

    /// Draws the theme groups. Returns `true` when the draft should be applied now.
    pub fn ui(&mut self, ui: &mut egui::Ui, kit: &Kit, applied: &ThemeConfig) -> bool {
        let mut apply = false;
        // Keep the draft in step with changes made elsewhere (per-app modes, other apps).
        if self.draft.app_modes != applied.app_modes {
            self.draft.app_modes = applied.app_modes.clone();
        }

        kit::group(ui, kit, "Theme", |g| {
            g.custom(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(12.0, 12.0);
                    let mode = self.draft.mode;
                    let generated = Palette::new(self.draft.accent, mode);
                    if theme_tile(ui, kit, "ChickenWM", &generated, self.draft.preset.is_empty()) {
                        self.draft.preset.clear();
                        apply = true;
                    }
                    for p in &PRESETS {
                        let name = match mode {
                            ThemeMode::Light => p.light_name,
                            ThemeMode::Dark => p.dark_name,
                        };
                        let label = if name == p.name { p.name.to_string() } else { format!("{} · {name}", p.name) };
                        if theme_tile(ui, kit, &label, &p.palette(mode), self.draft.preset == p.id) {
                            self.draft.preset = p.id.to_string();
                            apply = true;
                        }
                    }
                });
            });
            g.row("Appearance", "Light or dark for every app that follows the system.", |ui| {
                if kit::segmented(ui, kit, &mut self.draft.mode, &[(ThemeMode::Light, "☀ Light"), (ThemeMode::Dark, "🌙 Dark")]) {
                    apply = true;
                }
            });
            if self.draft.preset.is_empty() {
                g.row("Accent color", "The ChickenWM theme is generated from this one color.", |ui| {
                    let dirty = self.draft.accent != applied.accent;
                    if dirty && kit::button(ui, kit, "Apply", true).clicked() {
                        apply = true;
                    }
                    if ui.small_button("Reset").on_hover_text("#242321").clicked() {
                        self.draft.accent = DEFAULT_ACCENT;
                        self.hex = css(DEFAULT_ACCENT);
                    }
                    let resp = ui.add(egui::TextEdit::singleline(&mut self.hex).desired_width(72.0).font(egui::TextStyle::Monospace));
                    if resp.changed() {
                        if let Some(c) = RgbaColor::parse(self.hex.trim()) {
                            self.draft.accent = RgbaColor { a: 255, ..c };
                        }
                    }
                    let mut rgb = [self.draft.accent.r, self.draft.accent.g, self.draft.accent.b];
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        self.draft.accent = RgbaColor { r: rgb[0], g: rgb[1], b: rgb[2], a: 255 };
                        self.hex = css(self.draft.accent);
                    }
                });
            }
            g.row("Icons", "Icons in the file explorer and launcher.", |ui| {
                let before = self.draft.icon_theme.clone();
                egui::ComboBox::from_id_salt("icon_theme")
                    .selected_text(icon_theme_label(&self.draft.icon_theme))
                    .width(190.0)
                    .show_ui(ui, |ui| {
                        for t in self.icon_themes.clone() {
                            let label = icon_theme_label(&t);
                            ui.selectable_value(&mut self.draft.icon_theme, t, label);
                        }
                    });
                if self.draft.icon_theme != before {
                    apply = true;
                }
            });
            // Icon preview (the built-in set is redrawn from the palette on apply).
            let theme_name = self.draft.icon_theme.clone();
            if self.icons.as_ref().map(|(n, _)| n != &theme_name).unwrap_or(true) {
                self.icons = Some((theme_name.clone(), load_icon_previews(g.ui.ctx(), &theme_name)));
            }
            if let Some((_, icons)) = &self.icons {
                if !icons.is_empty() {
                    g.custom(|ui| {
                        ui.horizontal_wrapped(|ui| {
                            for (name, tex) in icons {
                                ui.add(egui::Image::new(tex).fit_to_exact_size(Vec2::splat(28.0))).on_hover_text(name);
                            }
                        });
                    });
                }
            }
        });
        apply
    }
}

fn icon_theme_label(t: &str) -> String {
    match t {
        "system" => "Windows icons".to_string(),
        "chickenwm" => "ChickenWM (follows theme)".to_string(),
        _ => t.to_string(),
    }
}

/// One gallery tile: a tiny window mock-up in the theme's colors. Returns `true` when clicked.
fn theme_tile(ui: &mut egui::Ui, kit: &Kit, name: &str, p: &Palette, selected: bool) -> bool {
    let size = Vec2::new(132.0, 106.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    let preview = Rect::from_min_size(rect.min, Vec2::new(size.x, 78.0));
    let t = ui.ctx().animate_bool_with_time(resp.id, selected || resp.hovered(), 0.12);
    // Selection ring.
    if selected {
        painter.rect_stroke(preview.expand(3.0), CornerRadius::same(11), Stroke::new(2.5_f32, kit.accent), egui::StrokeKind::Inside);
    } else if t > 0.0 {
        painter.rect_stroke(preview.expand(3.0), CornerRadius::same(11), Stroke::new(2.0_f32, kit.border.gamma_multiply(t * 2.0)), egui::StrokeKind::Inside);
    }
    let win = preview.shrink(if selected { 4.0 } else { 1.0 });
    painter.rect(win, CornerRadius::same(8), c32(p.base), Stroke::new(1.0_f32, kit.border), egui::StrokeKind::Inside);
    // Sidebar strip with a selected item.
    let side = Rect::from_min_max(win.min, egui::pos2(win.left() + win.width() * 0.3, win.bottom()));
    painter.rect_filled(side, CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }, c32(p.mantle));
    for i in 0..3 {
        let y = side.top() + 12.0 + i as f32 * 12.0;
        let line = Rect::from_min_size(egui::pos2(side.left() + 6.0, y), Vec2::new(side.width() - 12.0, 6.0));
        let fill = if i == 1 { c32(p.accent) } else { c32(p.fg).gamma_multiply(0.25) };
        painter.rect_filled(line, CornerRadius::same(3), fill);
    }
    // Content card.
    let card = Rect::from_min_max(egui::pos2(side.right() + 7.0, win.top() + 8.0), egui::pos2(win.right() - 7.0, win.bottom() - 8.0));
    painter.rect(card, CornerRadius::same(5), c32(p.surface), Stroke::new(1.0_f32, c32(p.border)), egui::StrokeKind::Inside);
    let text = Rect::from_min_size(card.min + Vec2::new(6.0, 7.0), Vec2::new(card.width() * 0.6, 5.0));
    painter.rect_filled(text, CornerRadius::same(3), c32(p.fg).gamma_multiply(0.85));
    painter.rect_filled(text.translate(Vec2::new(0.0, 9.0)).scale_from_center2(Vec2::new(0.75, 1.0)), CornerRadius::same(3), c32(p.subtext).gamma_multiply(0.7));
    // Highlight dots.
    for (i, c) in [p.dir, p.exec, p.archive, p.media, p.danger].into_iter().enumerate() {
        painter.circle_filled(egui::pos2(card.left() + 9.0 + i as f32 * 11.0, card.bottom() - 9.0), 3.5, c32(c));
    }
    // Accent button.
    let btn = Rect::from_min_size(egui::pos2(card.right() - 26.0, card.bottom() - 15.0), Vec2::new(20.0, 10.0));
    painter.rect_filled(btn, CornerRadius::same(5), c32(p.accent));
    painter.text(
        egui::pos2(rect.center().x, preview.bottom() + 15.0),
        egui::Align2::CENTER_CENTER,
        name,
        FontId::proportional(12.0),
        if selected { kit.fg } else { kit.sub },
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// Per-app light/dark rows. Returns the app whose choice the user changed.
pub fn per_app_ui(ui: &mut egui::Ui, kit: &Kit, theme: &ThemeConfig) -> Option<(&'static str, AppMode)> {
    use chickenwm_core::theme::app_mode_table;
    let mut changed = None;
    kit::group(ui, kit, "Per-app appearance", |g| {
        for (id, name, choice, effective) in app_mode_table(theme) {
            let hint = if choice == AppMode::System { format!("Follows the system ({})", effective.as_str()) } else { String::new() };
            g.row(name, &hint, |ui| {
                let mut sel = choice;
                if kit::segmented(ui, kit, &mut sel, &[(AppMode::System, "Auto"), (AppMode::Light, "Light"), (AppMode::Dark, "Dark")]) {
                    changed = Some((id, sel));
                }
            });
        }
    });
    changed
}

fn load_icon_previews(ctx: &egui::Context, theme: &str) -> Vec<(String, egui::TextureHandle)> {
    let t = IconTheme::load(theme);
    if t.is_system() {
        return Vec::new();
    }
    let dir = icons_root().join(theme);
    let mut names: Vec<String> = vec!["folder".into()];
    names.extend(SPECIAL_FOLDERS.iter().map(|s| format!("folder-{s}")));
    names.push("drive".into());
    names.extend(CATEGORIES.iter().map(|s| s.to_string()));
    names
        .into_iter()
        .filter_map(|n| {
            let img = image::open(dir.join(format!("{n}.png"))).ok()?.to_rgba8();
            let size = [img.width() as usize, img.height() as usize];
            let tex = ctx.load_texture(
                format!("theme-icon-{theme}-{n}"),
                egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw()),
                egui::TextureOptions::LINEAR,
            );
            Some((n, tex))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drawing the page without any input must never apply a theme.
    #[test]
    fn idle_frames_do_not_apply() {
        let ctx = egui::Context::default();
        let applied = ThemeConfig { app_modes: vec![("files".into(), AppMode::Dark)], ..ThemeConfig::default() };
        let mut editor = ThemeEditor::new(&applied);
        let kit = Kit::from_css(&chickenwm_core::css::CssStyleSheet::default());
        for frame in 0..8 {
            let mut fired = false;
            let mut per_app = None;
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    fired = editor.ui(ui, &kit, &applied);
                    per_app = per_app_ui(ui, &kit, &applied);
                });
            });
            assert!(!fired, "theme applied on idle frame {frame}");
            assert!(per_app.is_none(), "per-app mode changed on idle frame {frame}");
        }
        assert_eq!(editor.draft, applied);
    }
}
