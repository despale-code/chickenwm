use crate::kit::{self, Kit};
use crate::{bool_str, SettingsApp};
use eframe::egui;

impl SettingsApp {
    pub(crate) fn page_tiling(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        let mut changed = false;
        kit::group(ui, kit, "Layout", |g| {
            g.row(
                "Layout",
                "Dwindle splits space in a spiral; Master keeps one big window and a stack.",
                |ui| {
                    let mut sel: &str = if self.cfg.general.layout == "master" {
                        "master"
                    } else {
                        "dwindle"
                    };
                    if kit::segmented(
                        ui,
                        kit,
                        &mut sel,
                        &[("dwindle", "Dwindle"), ("master", "Master")],
                    ) {
                        self.cfg.general.layout = sel.to_string();
                        self.doc.set_category_key("general", "layout", sel);
                        changed = true;
                    }
                },
            );
            g.row("Gaps between windows", "", |ui| {
                if ui
                    .add(egui::Slider::new(&mut self.cfg.general.gaps_in, 0..=40).suffix(" px"))
                    .changed()
                {
                    self.doc.set_category_key(
                        "general",
                        "gaps_in",
                        &self.cfg.general.gaps_in.to_string(),
                    );
                    changed = true;
                }
            });
            g.row("Gaps at screen edges", "", |ui| {
                if ui
                    .add(egui::Slider::new(&mut self.cfg.general.gaps_out, 0..=60).suffix(" px"))
                    .changed()
                {
                    self.doc.set_category_key(
                        "general",
                        "gaps_out",
                        &self.cfg.general.gaps_out.to_string(),
                    );
                    changed = true;
                }
            });
            g.row("Border width", "", |ui| {
                if ui
                    .add(egui::Slider::new(&mut self.cfg.general.border_size, 0..=12).suffix(" px"))
                    .changed()
                {
                    self.doc.set_category_key(
                        "general",
                        "border_size",
                        &self.cfg.general.border_size.to_string(),
                    );
                    changed = true;
                }
            });
        });
        kit::group(ui, kit, "Dwindle", |g| {
            g.row("Split ratio", "", |ui| {
                if ui
                    .add(
                        egui::Slider::new(&mut self.cfg.dwindle.default_split_ratio, 0.4..=1.6)
                            .max_decimals(2),
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
            });
            g.row(
                "Keep split direction",
                "A split keeps its direction when windows are resized.",
                |ui| {
                    if kit::toggle(ui, kit, &mut self.cfg.dwindle.preserve_split).changed() {
                        self.doc.set_category_key(
                            "dwindle",
                            "preserve_split",
                            bool_str(self.cfg.dwindle.preserve_split),
                        );
                        changed = true;
                    }
                },
            );
        });
        kit::group(ui, kit, "Master", |g| {
            g.row("Master area size", "", |ui| {
                if ui
                    .add(egui::Slider::new(&mut self.cfg.master.mfact, 0.25..=0.75).max_decimals(2))
                    .changed()
                {
                    self.doc.set_category_key(
                        "master",
                        "mfact",
                        &format!("{:.2}", self.cfg.master.mfact),
                    );
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
            g.row(
                "Hide title bars",
                "Removes the Windows title bar from tiled windows.",
                |ui| {
                    if kit::toggle(ui, kit, &mut self.cfg.general.no_titlebars).changed() {
                        self.doc.set_category_key(
                            "general",
                            "no_titlebars",
                            bool_str(self.cfg.general.no_titlebars),
                        );
                        changed = true;
                    }
                },
            );
        });
        kit::group(ui, kit, "Focus", |g| {
            g.row(
                "Focus follows the mouse",
                "Hovering a window focuses it.",
                |ui| {
                    let mut on = self.cfg.input.follow_mouse != 0;
                    if kit::toggle(ui, kit, &mut on).changed() {
                        self.cfg.input.follow_mouse = on as u8;
                        self.doc
                            .set_category_key("input", "follow_mouse", if on { "1" } else { "0" });
                        changed = true;
                    }
                },
            );
            g.row(
                "Hover delay",
                "How long the cursor rests on a window before it takes focus.",
                |ui| {
                    if ui
                        .add(
                            egui::Slider::new(&mut self.cfg.input.hover_delay_ms, 0..=600)
                                .suffix(" ms"),
                        )
                        .changed()
                    {
                        self.doc.set_category_key(
                            "input",
                            "hover_delay_ms",
                            &self.cfg.input.hover_delay_ms.to_string(),
                        );
                        changed = true;
                    }
                },
            );
            g.row(
                "Ignore the mouse after typing",
                "Hover focus is paused this long after a key press.",
                |ui| {
                    if ui
                        .add(
                            egui::Slider::new(&mut self.cfg.input.keyboard_grace_ms, 0..=2000)
                                .suffix(" ms"),
                        )
                        .changed()
                    {
                        self.doc.set_category_key(
                            "input",
                            "keyboard_grace_ms",
                            &self.cfg.input.keyboard_grace_ms.to_string(),
                        );
                        changed = true;
                    }
                },
            );
        });
        if changed {
            self.sync_config_and_notify_labeled("Tiling & focus");
        }
    }
}
