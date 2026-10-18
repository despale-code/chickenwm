use crate::kit::{self, Kit};
use crate::SettingsApp;
use chickenwm_core::config::config_dir;
use eframe::egui::{self, Color32, RichText};
use std::time::{Duration, Instant};

impl SettingsApp {
    pub(crate) fn page_logs(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        let dir = config_dir().join("logs");
        if self
            .log_cache
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(2))
        {
            let mut files: Vec<(std::path::PathBuf, u64, Option<std::time::SystemTime>, usize)> =
                std::fs::read_dir(&dir)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "log"))
                    .map(|e| {
                        let m = e.metadata().ok();
                        let errors = std::fs::read_to_string(e.path())
                            .map_or(0, |t| t.lines().filter(|l| l.contains(" ERROR ")).count());
                        (
                            e.path(),
                            m.as_ref().map_or(0, |m| m.len()),
                            m.and_then(|m| m.modified().ok()),
                            errors,
                        )
                    })
                    .collect();
            files.sort_by(|a, b| b.2.cmp(&a.2));
            self.log_cache = Some((Instant::now(), files));
        }
        let files = self
            .log_cache
            .as_ref()
            .map(|c| c.1.clone())
            .unwrap_or_default();
        let day_ago = std::time::SystemTime::now() - Duration::from_secs(86_400);
        let crashes = dir.join("crashes.log");
        let crash_text = std::fs::read_to_string(&crashes).unwrap_or_default();
        let recent_crashes = crash_text
            .lines()
            .filter(|l| l.contains("PANIC") || l.contains("panicked"))
            .count();
        kit::group(ui, kit, "Crashes", |g| {
            g.row(
                if recent_crashes == 0 {
                    "No crashes recorded"
                } else {
                    "Crashes recorded"
                },
                "Every module that panics is logged in crashes.log, with where it happened.",
                |ui| {
                    if recent_crashes > 0
                        && kit::button(ui, kit, &format!("Show {recent_crashes}"), true).clicked()
                    {
                        self.log_selected = Some(crashes.clone());
                        self.log_errors_only = false;
                    }
                },
            );
        });
        let selected = self.log_selected.clone();
        kit::group(ui, kit, "Module logs", |g| {
            for (path, size, modified, errors) in &files {
                let (errors, size) = (*errors, *size);
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let fresh = modified.is_some_and(|m| m >= day_ago);
                let hint = format!(
                    "{:.0} KB{}{}",
                    size as f64 / 1024.0,
                    if errors > 0 {
                        format!(" · {errors} error line(s)")
                    } else {
                        String::new()
                    },
                    if fresh { "" } else { " · not written today" }
                );
                g.row(&name, &hint, |ui| {
                    let open = selected.as_deref() == Some(path.as_path());
                    if kit::button(ui, kit, if open { "Showing" } else { "Show" }, open).clicked() {
                        self.log_selected = Some(path.clone());
                    }
                });
            }
            g.row("Folder", &dir.display().to_string(), |ui| {
                if kit::button(ui, kit, "Open folder", false).clicked() {
                    let _ = std::process::Command::new("explorer.exe").arg(&dir).spawn();
                }
            });
        });
        let Some(path) = self.log_selected.clone() else {
            return;
        };
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        kit::group(ui, kit, &name, |g| {
            g.row("Filter", "Only lines containing this text.", |ui| {
                ui.checkbox(&mut self.log_errors_only, "Errors only");
                ui.add(egui::TextEdit::singleline(&mut self.log_filter).desired_width(160.0));
            });
            g.custom(|ui| {
                let q = self.log_filter.to_lowercase();
                let lines: Vec<&str> = text
                    .lines()
                    .filter(|l| {
                        !self.log_errors_only
                            || l.contains(" ERROR ")
                            || l.contains(" WARN ")
                            || l.contains("PANIC")
                    })
                    .filter(|l| q.is_empty() || l.to_lowercase().contains(&q))
                    .collect();
                let start = lines.len().saturating_sub(400);
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for l in &lines[start..] {
                            let color = if l.contains(" ERROR ") || l.contains("PANIC") {
                                Color32::from_rgb(220, 80, 70)
                            } else if l.contains(" WARN ") {
                                Color32::from_rgb(220, 160, 60)
                            } else {
                                kit.sub
                            };
                            ui.label(RichText::new(*l).monospace().size(11.0).color(color));
                        }
                        if lines.is_empty() {
                            ui.label(RichText::new("Nothing matches.").color(kit.sub));
                        }
                    });
                if kit::button(ui, kit, "Open in Notepad", false).clicked() {
                    let _ = std::process::Command::new("notepad.exe").arg(&path).spawn();
                }
            });
        });
    }
}
