use crate::kit::{self, Kit};
use crate::{bool_str, SettingsApp};
use chickenwm_core::ipc::{send_ipc_command_blocking, IpcCommand, PIPE_NAME_DAEMON};
use chickenwm_core::monitor::{enumerate_monitors, max_native_refresh_rate};
use eframe::egui::{self, RichText};

impl SettingsApp {
    pub(crate) fn page_displays(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        let mut refresh = false;
        kit::group(ui, kit, "Displays", |g| {
            for m in &self.monitors {
                let hint = if m.adapter_name.is_empty() {
                    String::new()
                } else {
                    m.adapter_name.clone()
                };
                g.row(&m.name, &hint, |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} × {} · {} Hz · {:.0}%",
                            m.width,
                            m.height,
                            m.refresh_rate_hz,
                            m.dpi_scale * 100.0
                        ))
                        .color(kit.sub),
                    );
                    if m.is_primary {
                        kit::chip(ui, kit, "Main", kit.accent);
                    }
                });
            }
            g.row("", "", |ui| {
                if kit::button(ui, kit, "Detect displays", false).clicked() {
                    refresh = true;
                }
            });
        });
        if refresh {
            self.monitors = enumerate_monitors();
        }

        let native_hz = max_native_refresh_rate(&self.monitors);
        let mut follow_native = self.cfg.misc.fps_limit == 0;
        let mut fps_changed = false;
        let mut vfr_changed = false;
        kit::group(ui, kit, "Frame rate", |g| {
            g.row(
                "Match the display",
                &format!("Animations run at {native_hz} Hz, in step with the screen."),
                |ui| {
                    if kit::toggle(ui, kit, &mut follow_native).changed() {
                        self.cfg.misc.fps_limit = if follow_native { 0 } else { native_hz };
                        fps_changed = true;
                    }
                },
            );
            if !follow_native {
                g.row("Frame rate limit", "", |ui| {
                    if ui
                        .add(egui::Slider::new(&mut self.cfg.misc.fps_limit, 30..=500).suffix(" FPS"))
                        .changed()
                    {
                        fps_changed = true;
                    }
                });
                g.custom(|ui| {
                    ui.horizontal(|ui| {
                        for preset in [60u32, 120, 144, 165, 240, 360] {
                            if kit::button(
                                ui,
                                kit,
                                &format!("{preset}"),
                                self.cfg.misc.fps_limit == preset,
                            )
                            .clicked()
                            {
                                self.cfg.misc.fps_limit = preset;
                                fps_changed = true;
                            }
                        }
                    });
                });
            }
            g.row(
                "Save power when idle",
                "Stops redrawing when nothing moves (variable frame rate).",
                |ui| {
                    if kit::toggle(ui, kit, &mut self.cfg.misc.vfr).changed() {
                        vfr_changed = true;
                    }
                },
            );
        });
        if vfr_changed {
            self.doc
                .set_category_key("misc", "vfr", bool_str(self.cfg.misc.vfr));
            self.sync_config_and_notify_labeled("Variable frame rate");
        }
        if fps_changed {
            self.doc.set_category_key(
                "misc",
                "fps_limit",
                &self.cfg.misc.fps_limit.to_string(),
            );
            self.sync_config_and_notify_labeled("Frame rate limit");
            let fps = (self.cfg.misc.fps_limit != 0).then_some(self.cfg.misc.fps_limit);
            let _ = send_ipc_command_blocking(PIPE_NAME_DAEMON, &IpcCommand::SetFramerate(fps));
        }

        let mut mode_changed = false;
        kit::group(ui, kit, "Compositor", |g| {
            g.row(
                "Engine",
                "Standard is safe with anti-cheat games. The DWM hook injects into Windows' compositor (experimental).",
                |ui| {
                    mode_changed = kit::segmented(
                        ui,
                        kit,
                        &mut self.cfg.misc.dwm_hook_enabled,
                        &[(false, "Standard"), (true, "DWM hook")],
                    );
                },
            );
        });
        if mode_changed {
            self.doc.set_category_key(
                "misc",
                "dwm_hook_enabled",
                bool_str(self.cfg.misc.dwm_hook_enabled),
            );
            self.sync_config_and_notify_labeled("Compositor engine");
            let _ = send_ipc_command_blocking(
                PIPE_NAME_DAEMON,
                &IpcCommand::SetCompositorMode {
                    dwm_hook_enabled: self.cfg.misc.dwm_hook_enabled,
                },
            );
            self.status_banner =
                "Compositor engine changed: ChickenWM modules restarted".to_string();
        }
    }
}
