//! Theme section of the Appearance tab: accent color, light/dark mode, icon theme, a live
//! palette + icon preview, and "Apply to whole system". Also turns `settings.css` into egui
//! visuals so the settings window itself follows the theme.

use eframe::egui::{self, Color32, CornerRadius, RichText, Stroke};
use chickenwm_core::config::{RgbaColor};
use chickenwm_core::css::CssStyleSheet;
use chickenwm_core::icon_theme::{available_themes, icons_root, IconTheme, CATEGORIES, SPECIAL_FOLDERS};
use chickenwm_core::theme::{css, luminance, Palette, ThemeConfig, ThemeMode, DEFAULT_ACCENT};

pub fn c32(c: RgbaColor) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r, c.g, c.b, c.a)
}

pub struct ThemeEditor {
    pub draft: ThemeConfig,
    hex: String,
    icon_themes: Vec<String>,
    /// (icon theme name, textures) — reloaded when the theme or palette changes.
    icons: Option<(String, Vec<(String, egui::TextureHandle)>)>,
}

impl ThemeEditor {
    pub fn new(current: &ThemeConfig) -> Self {
        Self {
            hex: css(current.accent),
            draft: current.clone(),
            icon_themes: available_themes(),
            icons: None,
        }
    }

    pub fn invalidate_icons(&mut self) {
        self.icons = None;
        self.icon_themes = available_themes();
    }

    /// Draws the section. Returns `true` when the user pressed Apply.
    pub fn ui(&mut self, ui: &mut egui::Ui, applied: &ThemeConfig) -> bool {
        let mut apply = false;
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.label(RichText::new("System Theme").strong().size(15.0));
            ui.label(
                RichText::new(
                    "One accent color themes the bar, launcher, file explorer, browser, settings, window borders and icons.",
                )
                .small(),
            );
            ui.add_space(6.0);

            ui.horizontal(|ui| {
                ui.label("Accent:");
                let mut rgb = [self.draft.accent.r, self.draft.accent.g, self.draft.accent.b];
                if ui.color_edit_button_srgb(&mut rgb).changed() {
                    self.draft.accent = RgbaColor { r: rgb[0], g: rgb[1], b: rgb[2], a: 255 };
                    self.hex = css(self.draft.accent);
                }
                let resp = ui.add(egui::TextEdit::singleline(&mut self.hex).desired_width(80.0).font(egui::TextStyle::Monospace));
                if resp.changed() {
                    if let Some(c) = RgbaColor::parse(self.hex.trim()) {
                        self.draft.accent = RgbaColor { a: 255, ..c };
                    }
                }
                if ui.small_button("Reset").on_hover_text("#242321").clicked() {
                    self.draft.accent = DEFAULT_ACCENT;
                    self.hex = css(DEFAULT_ACCENT);
                }
                ui.separator();
                ui.label("Mode:");
                ui.selectable_value(&mut self.draft.mode, ThemeMode::Light, "☀ Light");
                ui.selectable_value(&mut self.draft.mode, ThemeMode::Dark, "🌙 Dark");
                ui.separator();
                ui.label("Icons:");
                egui::ComboBox::from_id_salt("icon_theme")
                    .selected_text(&self.draft.icon_theme)
                    .show_ui(ui, |ui| {
                        for t in self.icon_themes.clone() {
                            let label = match t.as_str() {
                                "system" => "system (Windows icons)".to_string(),
                                "chickenwm" => "chickenwm (built-in, follows accent)".to_string(),
                                _ => t.clone(),
                            };
                            ui.selectable_value(&mut self.draft.icon_theme, t, label);
                        }
                    });
            });

            ui.add_space(8.0);
            let palette = Palette::new(self.draft.accent, self.draft.mode);
            self.preview(ui, &palette);

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                let dirty = &self.draft != applied;
                let btn = egui::Button::new(RichText::new("Apply to whole system").strong());
                if ui.add_enabled(dirty, btn).clicked() {
                    apply = true;
                }
                if dirty {
                    ui.label(RichText::new("Preview — not applied yet").small().italics());
                    if ui.small_button("Revert").clicked() {
                        self.draft = applied.clone();
                        self.hex = css(applied.accent);
                    }
                } else {
                    ui.label(RichText::new("✔ Active").small());
                }
            });
            ui.label(
                RichText::new(format!(
                    "Writes the colors into launcher.css, files.css, settings.css, roobar.css and browser.css (other customizations are kept), sets the window border, and redraws the built-in icons in {}.",
                    icons_root().join("chickenwm").display()
                ))
                .small(),
            );
        });
        apply
    }

    fn preview(&mut self, ui: &mut egui::Ui, p: &Palette) {
        // Mini window mock-up
        egui::Frame::NONE
            .fill(c32(p.base))
            .stroke(Stroke::new(2.0_f32, c32(p.accent)))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    egui::Frame::NONE.fill(c32(p.mantle)).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Sidebar").color(c32(p.subtext)).small());
                            ui.label(RichText::new("Downloads").color(c32(p.fg)));
                            egui::Frame::NONE.fill(c32(p.selected)).corner_radius(CornerRadius::same(4)).inner_margin(egui::Margin::symmetric(4, 1)).show(ui, |ui| {
                                ui.label(RichText::new("Documents").color(c32(p.fg)));
                            });
                        });
                    });
                    egui::Frame::NONE.fill(c32(p.surface)).corner_radius(CornerRadius::same(6)).stroke(Stroke::new(1.0_f32, c32(p.border))).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Projects").color(c32(p.dir)));
                            ui.label(RichText::new("setup.exe").color(c32(p.exec)));
                            ui.label(RichText::new("backup.zip").color(c32(p.archive)));
                            ui.label(RichText::new("song.flac").color(c32(p.media)));
                        });
                    });
                    ui.vertical(|ui| {
                        egui::Frame::NONE.fill(c32(p.accent)).corner_radius(CornerRadius::same(6)).inner_margin(egui::Margin::symmetric(10, 4)).show(ui, |ui| {
                            ui.label(RichText::new("Active workspace").color(c32(p.accent_fg)));
                        });
                        ui.label(RichText::new("Body text").color(c32(p.fg)));
                        ui.label(RichText::new("Secondary text").color(c32(p.subtext)));
                        ui.label(RichText::new("Danger").color(c32(p.danger)));
                    });
                });
            });

        // Swatches
        ui.horizontal_wrapped(|ui| {
            for (name, col) in [
                ("base", p.base),
                ("surface", p.surface),
                ("mantle", p.mantle),
                ("text", p.fg),
                ("subtext", p.subtext),
                ("accent", p.accent),
                ("on accent", p.accent_fg),
                ("accent soft", p.accent_soft),
                ("folders", p.dir),
                ("apps", p.exec),
                ("archives", p.archive),
                ("media", p.media),
                ("code", p.code),
            ] {
                ui.vertical(|ui| {
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(44.0, 26.0), egui::Sense::click());
                    ui.painter().rect(rect, CornerRadius::same(5), c32(col), Stroke::new(1.0_f32, c32(p.border)), egui::StrokeKind::Inside);
                    resp.on_hover_text(format!("{name}: {}", css(col)));
                    ui.label(RichText::new(name).small());
                });
            }
        });

        // Icons (built-in set reflects the applied palette; it's redrawn on Apply)
        let theme_name = self.draft.icon_theme.clone();
        if self.icons.as_ref().map(|(n, _)| n != &theme_name).unwrap_or(true) {
            self.icons = Some((theme_name.clone(), load_icon_previews(ui.ctx(), &theme_name)));
        }
        if let Some((_, icons)) = &self.icons {
            if icons.is_empty() {
                ui.label(RichText::new("Windows shell icons (no icon theme)").small());
            } else {
                ui.horizontal_wrapped(|ui| {
                    for (name, tex) in icons {
                        ui.add(egui::Image::new(tex).fit_to_exact_size(egui::vec2(32.0, 32.0))).on_hover_text(name);
                    }
                });
            }
        }
    }
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

/// egui visuals from `settings.css` (light or dark depending on the background).
pub fn apply_visuals(ctx: &egui::Context, sheet: &CssStyleSheet) {
    let v = |n: &str, d: RgbaColor| sheet.var_color(n, d);
    let bg = v("--bg-color", RgbaColor { r: 24, g: 24, b: 37, a: 255 });
    let card = v("--card-bg", bg);
    let side = v("--sidebar-bg", bg);
    let fg = v("--fg-color", RgbaColor { r: 205, g: 214, b: 244, a: 255 });
    let accent = v("--accent-color", RgbaColor { r: 51, g: 204, b: 255, a: 255 });
    let accent_fg = v("--accent-fg", bg);
    let selected = v("--selected-bg", RgbaColor { a: 50, ..accent });
    let border = v("--border-color", RgbaColor { r: 49, g: 50, b: 68, a: 255 });

    let light = luminance(bg) > 0.45;
    let mut vis = if light { egui::Visuals::light() } else { egui::Visuals::dark() };
    vis.panel_fill = c32(side);
    vis.window_fill = c32(card);
    vis.extreme_bg_color = c32(card);
    vis.faint_bg_color = c32(side);
    vis.override_text_color = Some(c32(fg));
    vis.hyperlink_color = c32(accent);
    vis.selection.bg_fill = c32(selected).gamma_multiply(if light { 1.6 } else { 1.4 });
    vis.selection.stroke = Stroke::new(1.0_f32, c32(accent));
    vis.window_stroke = Stroke::new(1.0_f32, c32(border));
    vis.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, c32(border));
    vis.widgets.noninteractive.bg_fill = c32(card);
    vis.widgets.inactive.bg_fill = c32(card);
    vis.widgets.inactive.weak_bg_fill = c32(card);
    vis.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, c32(border));
    vis.widgets.hovered.weak_bg_fill = c32(selected);
    vis.widgets.hovered.bg_fill = c32(selected);
    vis.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, c32(accent));
    vis.widgets.active.weak_bg_fill = c32(accent);
    vis.widgets.active.bg_fill = c32(accent);
    vis.widgets.active.fg_stroke = Stroke::new(1.0_f32, c32(accent_fg));
    vis.widgets.open.weak_bg_fill = c32(selected);
    ctx.set_visuals(vis);
}
