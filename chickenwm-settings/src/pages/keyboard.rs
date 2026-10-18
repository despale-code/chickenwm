use crate::kit::{self, Kit};
use crate::SettingsApp;
use chickenwm_core::config::{BindKind, BindRule};
use eframe::egui::{self, RichText};

impl SettingsApp {
    pub(crate) fn page_keyboard(&mut self, ui: &mut egui::Ui, kit: &Kit, ctx: &egui::Context) {
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
                    if let egui::Event::Key {
                        key, pressed: true, ..
                    } = ev
                    {
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
            g.row(
                "Keys",
                if self.recording_keybind {
                    "Press the keys now…"
                } else {
                    "Type them (or a chain like K > R), or record them."
                },
                |ui| {
                    let label = if self.recording_keybind {
                        "Cancel"
                    } else {
                        "Record"
                    };
                    if kit::button(ui, kit, label, self.recording_keybind).clicked() {
                        self.recording_keybind = !self.recording_keybind;
                    }
                    ui.add(egui::TextEdit::singleline(&mut self.recorded_key).desired_width(90.0));
                    ui.label(RichText::new("+").color(kit.sub));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.recorded_mods).desired_width(100.0),
                    );
                },
            );
            g.row(
                "Action",
                "A dispatcher and its argument, e.g. exec wt.exe or submap resize.",
                |ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.new_bind_arg).desired_width(130.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_bind_dispatcher)
                            .desired_width(90.0),
                    );
                },
            );
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
                submap: String::new(),
            };
            self.doc.add_directive(
                "bind",
                &format!("{}, {}, {}, {}", rule.mods, rule.key, rule.dispatcher, rule.arg),
            );
            self.cfg.binds.push(rule);
            self.sync_config_and_notify_labeled("Add keyboard shortcut");
        }

        kit::search_field(ui, kit, &mut self.bind_search, "Search shortcuts");
        ui.add_space(10.0);
        let q = self.bind_search.trim().to_ascii_lowercase();
        kit::group(ui, kit, "Shortcuts", |g| {
            for b in &self.cfg.binds {
                let hay = format!("{} {} {} {} {}", b.submap, b.mods, b.key, b.dispatcher, b.arg)
                    .to_ascii_lowercase();
                if !q.is_empty() && !hay.contains(&q) {
                    continue;
                }
                g.custom(|ui| {
                    ui.horizontal(|ui| {
                        if !b.submap.is_empty() {
                            kit::chip(ui, kit, &format!("mode: {}", b.submap), kit.accent);
                        }
                        for m in b
                            .mods
                            .split(|c: char| c == ' ' || c == '_' || c == '+')
                            .filter(|m| !m.is_empty())
                        {
                            kit::keycap(ui, kit, m);
                        }
                        kit::keycap(ui, kit, &b.key);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(
                                RichText::new(
                                    format!("{} {}", b.dispatcher, b.arg).trim().to_string(),
                                )
                                .color(kit.sub),
                            );
                        });
                    });
                });
            }
        });
    }
}
