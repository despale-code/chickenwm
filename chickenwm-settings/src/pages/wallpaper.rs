use crate::kit::{self, Kit};
use crate::{SettingsApp, TRANSITIONS};
use chickenwm_core::config::PaperLayerKind;
use chickenwm_core::ipc::{send_ipc_command_blocking, IpcCommand, PIPE_NAME_PAPER};
use eframe::egui::{self, RichText};

impl SettingsApp {
    pub(crate) fn page_wallpaper(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        let mut apply = false;
        kit::group(ui, kit, "Wallpaper", |g| {
            g.row(
                "Image or shader",
                "A PNG / JPEG / WebP picture, or a .wgsl shader.",
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.wallpaper_path_input)
                            .desired_width(260.0)
                            .hint_text("~/Pictures/wall.png"),
                    );
                },
            );
            g.row("Transition", "", |ui| {
                let opts: Vec<(&'static str, &str)> =
                    TRANSITIONS.iter().map(|t| (*t, *t)).collect();
                kit::segmented(ui, kit, &mut self.wallpaper_transition, &opts);
            });
            g.row("", "", |ui| {
                if kit::button(ui, kit, "Set wallpaper", true).clicked() {
                    apply = true;
                }
            });
        });
        if apply {
            let _ = send_ipc_command_blocking(
                PIPE_NAME_PAPER,
                &IpcCommand::SetWallpaper {
                    monitor: None,
                    path: self.wallpaper_path_input.clone(),
                    transition: Some(self.wallpaper_transition.to_string()),
                },
            );
            self.doc.replace_directives(
                "wallpaper",
                &[format!(
                    ", {}, {}",
                    self.wallpaper_path_input, self.wallpaper_transition
                )],
            );
            self.sync_config_and_notify_labeled("Wallpaper");
        }

        kit::group(ui, kit, "Layers", |g| {
            if self.cfg.paper.layers.is_empty() {
                g.custom(|ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(
                                "Stack images, colors and shaders, with chroma keying, in chickenwm.conf:\n\npaper {\n    layer = image, ~/bg/sky.png\n    layer = image, ~/bg/character.png, key = rgb(00ff00), tolerance = 0.25\n}\n\nLayers replace the single wallpaper above. See docs/STYLING.md.",
                            )
                            .size(12.5)
                            .color(kit.sub),
                        )
                        .wrap(),
                    );
                });
            } else {
                for (i, l) in self.cfg.paper.layers.iter().enumerate() {
                    let kind = match l.kind {
                        PaperLayerKind::Image => "Image",
                        PaperLayerKind::Generated => "Theme wallpaper",
                        PaperLayerKind::Color => "Color",
                        PaperLayerKind::Shader => "Shader",
                    };
                    let mut hint = format!(
                        "{}{} · {} · {:.0}%",
                        if l.source.is_empty() { "" } else { &l.source },
                        if l.source.is_empty() { "" } else { " ·" },
                        l.blend,
                        l.opacity * 100.0
                    );
                    if let Some(k) = l.key {
                        hint.push_str(&format!(
                            " · key {} ±{:.2}",
                            chickenwm_core::theme::css(k),
                            l.tolerance
                        ));
                    }
                    g.row(&format!("{}. {kind}", i + 1), &hint, |ui| {
                        if l.key.is_some() {
                            kit::chip(ui, kit, "Chroma key", kit.accent);
                        }
                    });
                }
            }
        });
    }
}
