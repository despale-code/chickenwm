use crate::kit::{self, Kit};
use crate::{bool_str, SettingsApp};
use chickenwm_core::config::config_dir;
use chickenwm_core::failsafe::trigger_emergency_restore;
use chickenwm_core::ipc::{send_ipc_command_blocking, IpcCommand, PIPE_NAME_DAEMON};
use eframe::egui::{self, Color32, CornerRadius, RichText, Vec2};

impl SettingsApp {
    pub(crate) fn page_general(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        if !self.config_issues.is_empty() {
            let issues = self.config_issues.clone();
            kit::group(ui, kit, "Problems in chickenwm.conf", |g| {
                g.custom(|ui| {
                    for i in issues.iter().take(10) {
                        let color = if i.severity == chickenwm_core::config_check::Severity::Error {
                            Color32::from_rgb(220, 80, 70)
                        } else {
                            kit.sub
                        };
                        ui.label(
                            RichText::new(format!("Line {}: {}", i.line, i.message))
                                .size(12.5)
                                .color(color),
                        );
                    }
                    if issues.len() > 10 {
                        ui.label(
                            RichText::new(format!(
                                "…and {} more (chickenctl check-config)",
                                issues.len() - 10
                            ))
                            .size(12.0)
                            .color(kit.sub),
                        );
                    }
                });
                g.row("Fix them by hand", "These lines are skipped until they're fixed.", |ui| {
                    if kit::button(ui, kit, "Open chickenwm.conf", false).clicked() {
                        let _ = std::process::Command::new("notepad.exe")
                            .arg(config_dir().join("chickenwm.conf"))
                            .spawn();
                    }
                });
            });
        }
        let mut shell_changed = false;
        kit::group(ui, kit, "Shell", |g| {
            g.row(
                "Use ChickenWM as the Windows shell",
                "Starts ChickenWM instead of Explorer when you sign in.",
                |ui| {
                    if kit::toggle(ui, kit, &mut self.cfg.misc.replace_winlogon_shell).changed() {
                        self.doc.set_category_key(
                            "misc",
                            "replace_winlogon_shell",
                            bool_str(self.cfg.misc.replace_winlogon_shell),
                        );
                        shell_changed = true;
                    }
                },
            );
            g.row(
                "Hide Explorer's taskbar and desktop",
                "Closes explorer.exe while ChickenWM runs; roobar takes its place.",
                |ui| {
                    if kit::toggle(ui, kit, &mut self.cfg.misc.suppress_explorer).changed() {
                        self.doc.set_category_key(
                            "misc",
                            "suppress_explorer",
                            bool_str(self.cfg.misc.suppress_explorer),
                        );
                        shell_changed = true;
                    }
                },
            );
        });
        if shell_changed {
            self.sync_config_and_notify_labeled("Shell settings");
            self.send_shell_config();
        }

        let mut want_admin = self.cfg.misc.run_as_admin;
        let mut restart_admin = false;
        let elevated = chickenwm_core::elevation::is_elevated();
        kit::group(ui, kit, "Administrator", |g| {
            g.row(
                "Run ChickenWM as administrator",
                "Starts elevated at every sign-in, so windows of administrator programs can be tiled. Asks for approval once. Apps you launch from ChickenWM also run as administrator.",
                |ui| {
                    kit::toggle(ui, kit, &mut want_admin);
                },
            );
            g.row("Status", "", |ui| {
                if !elevated && kit::button(ui, kit, "Restart as administrator", true).clicked() {
                    restart_admin = true;
                }
                if elevated {
                    kit::chip(
                        ui,
                        kit,
                        "● Running as administrator",
                        Color32::from_rgb(52, 199, 89),
                    );
                } else {
                    kit::chip(ui, kit, "Normal privileges", kit.sub);
                }
            });
        });
        if want_admin != self.cfg.misc.run_as_admin {
            self.apply_run_as_admin(want_admin);
        }
        if restart_admin {
            self.status_banner =
                match send_ipc_command_blocking(PIPE_NAME_DAEMON, &IpcCommand::RestartElevated) {
                    Ok(chickenwm_core::ipc::IpcResponse::Error { message }) => {
                        format!("Not restarted: {message}")
                    }
                    Ok(_) => "Restarting ChickenWM as administrator".to_string(),
                    Err(_) => "ChickenWM isn't running".to_string(),
                };
        }

        let mut want = self.cfg.misc.register_files_handler;
        let mut repair = false;
        let mut open_files = false;
        kit::group(ui, kit, "Default file manager", |g| {
            g.row(
                "Open folders in ChickenWM Files",
                "Folders, drives, Win + E and File Explorer shortcuts. Just for you, no admin needed.",
                |ui| {
                    kit::toggle(ui, kit, &mut want);
                },
            );
            g.row("Status", "", |ui| {
                if kit::button(ui, kit, "Open Files", false).clicked() {
                    open_files = true;
                }
                if !self.files_handler_active
                    && self.cfg.misc.register_files_handler
                    && kit::button(ui, kit, "Repair", true).clicked()
                {
                    repair = true;
                }
                if self.files_handler_active {
                    kit::chip(ui, kit, "● Active", Color32::from_rgb(52, 199, 89));
                } else if self.cfg.misc.register_files_handler {
                    kit::chip(ui, kit, "Registry keys missing", Color32::from_rgb(255, 149, 0));
                } else {
                    kit::chip(ui, kit, "Explorer opens folders", kit.sub);
                }
            });
        });
        if want != self.cfg.misc.register_files_handler {
            self.apply_files_handler(want);
        }
        if repair {
            self.apply_files_handler(true);
        }
        if open_files {
            if let Ok(mut exe) = std::env::current_exe() {
                exe.set_file_name("chickenwm-files.exe");
                if let Err(e) = std::process::Command::new(&exe).spawn() {
                    self.status_banner = format!("Couldn't start {}: {e}", exe.display());
                }
            }
        }

        let mut remove = None;
        let mut add = false;
        kit::group(ui, kit, "Open at startup", |g| {
            for (idx, cmd) in self.cfg.exec_once.iter().enumerate() {
                g.row(cmd, "", |ui| {
                    if ui.small_button("Remove").clicked() {
                        remove = Some(idx);
                    }
                });
            }
            g.row(
                "Add a program",
                "A command line, e.g. henbrowser.exe",
                |ui| {
                    if kit::button(ui, kit, "Add", true).clicked() {
                        add = true;
                    }
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.new_exec_once)
                            .desired_width(200.0)
                            .hint_text("program.exe --args"),
                    );
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        add = true;
                    }
                },
            );
        });
        if let Some(i) = remove {
            self.cfg.exec_once.remove(i);
            let vals = self.cfg.exec_once.clone();
            self.doc.replace_directives("exec-once", &vals);
            self.sync_config_and_notify_labeled("Remove startup app");
        }
        if add && !self.new_exec_once.trim().is_empty() {
            let cmd = self.new_exec_once.trim().to_string();
            self.cfg.exec_once.push(cmd.clone());
            self.doc.add_directive("exec-once", &cmd);
            self.new_exec_once.clear();
            self.sync_config_and_notify_labeled("Add startup app");
        }

        let mut restore = false;
        kit::group(ui, kit, "Recovery", |g| {
            g.row(
                "Restore Explorer",
                "Brings back Explorer as the shell right now. Hotkey anytime: Ctrl + Alt + Shift + Esc.",
                |ui| {
                    let btn = egui::Button::new(
                        RichText::new("Restore").color(Color32::WHITE).size(13.0),
                    )
                    .fill(kit.danger)
                    .corner_radius(CornerRadius::same(7))
                    .min_size(Vec2::new(0.0, 26.0));
                    if ui.add(btn).clicked() {
                        restore = true;
                    }
                },
            );
        });
        if restore {
            let _ = send_ipc_command_blocking(PIPE_NAME_DAEMON, &IpcCommand::RestoreExplorer);
            trigger_emergency_restore(false);
            self.status_banner = "Explorer restored".to_string();
        }
    }
}
