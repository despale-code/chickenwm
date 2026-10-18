use crate::kit::{self, Kit};
use crate::SettingsApp;
use chickenwm_core::config::BezierCurve;
use eframe::egui::{self, CornerRadius, RichText, Stroke, Vec2};

impl SettingsApp {
    pub(crate) fn page_animations(&mut self, ui: &mut egui::Ui, kit: &Kit, dt: f32) {
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
        self.bezier_preview_t = (self.bezier_preview_t + dt * 0.7) % 1.3;
        let mut changed = false;
        kit::group(ui, kit, "Easing curves", |g| {
            g.row("Curve", "", |ui| {
                let names: Vec<(usize, String)> = self
                    .cfg
                    .beziers
                    .iter()
                    .map(|b| b.name.clone())
                    .enumerate()
                    .collect();
                let opts: Vec<(usize, &str)> =
                    names.iter().map(|(i, n)| (*i, n.as_str())).collect();
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
                    let (rect, _) =
                        ui.allocate_exact_size(Vec2::new(200.0, 150.0), egui::Sense::hover());
                    let painter = ui.painter_at(rect);
                    painter.rect_filled(rect, CornerRadius::same(8), kit.bg);
                    let to_screen = |u: f32, v: f32| {
                        egui::pos2(
                            rect.left() + 16.0 + u * (rect.width() - 32.0),
                            rect.bottom() - 22.0 - v.clamp(-0.3, 1.3) * (rect.height() - 44.0),
                        )
                    };
                    painter.line_segment(
                        [to_screen(0.0, 0.0), to_screen(1.0, 1.0)],
                        Stroke::new(1.0_f32, kit.border),
                    );
                    let pts: Vec<egui::Pos2> = (0..=48)
                        .map(|s| s as f32 / 48.0)
                        .map(|u| to_screen(u, curve.sample(u)))
                        .collect();
                    painter.add(egui::Shape::line(pts, Stroke::new(2.5_f32, kit.accent)));
                    painter.circle_filled(to_screen(t, curve.sample(t)), 4.5, kit.accent);
                    ui.add_space(12.0);
                    let (prev, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width().max(160.0), 150.0),
                        egui::Sense::hover(),
                    );
                    let p = ui.painter_at(prev);
                    p.rect_filled(prev, CornerRadius::same(8), kit.bg);
                    let y = curve.sample(t);
                    let w = 64.0;
                    let x =
                        prev.left() + 14.0 + y.clamp(-0.1, 1.2) * (prev.width() - w - 28.0);
                    let win = egui::Rect::from_min_size(
                        egui::pos2(x, prev.center().y - 26.0),
                        Vec2::new(w, 52.0),
                    );
                    p.rect(
                        win,
                        CornerRadius::same(7),
                        kit.card,
                        Stroke::new(2.0_f32, kit.accent),
                        egui::StrokeKind::Inside,
                    );
                    p.rect_filled(
                        egui::Rect::from_min_size(
                            win.min + Vec2::new(8.0, 9.0),
                            Vec2::new(w * 0.55, 5.0),
                        ),
                        CornerRadius::same(3),
                        kit.sub,
                    );
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
                            rule.style
                                .as_deref()
                                .map(|s| format!(" · {s}"))
                                .unwrap_or_default()
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
            self.sync_config_and_notify_labeled("Bezier curve");
        }
    }
}
