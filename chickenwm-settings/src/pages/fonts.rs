use crate::kit::{self, Kit};
use crate::{primary_family, FontJob, FontRequest, FontTarget, SettingsApp};
use eframe::egui::{self, Color32, RichText};
use std::sync::Arc;
use std::time::{Duration, Instant};

impl SettingsApp {
    pub(crate) fn page_fonts(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        use chickenwm_core::fonts::{self, FONT_TARGETS, NERD_FONTS};
        let ctx = ui.ctx().clone();
        let installed = self
            .installed_fonts
            .get_or_insert_with(|| fonts::installed_families(false))
            .clone();
        let is_installed = |family: &str| installed.iter().any(|f| f.eq_ignore_ascii_case(family));

        self.poll_font_job();
        if self
            .current_fonts
            .as_ref()
            .is_none_or(|(at, _)| at.elapsed() > Duration::from_secs(1))
        {
            let current = FONT_TARGETS
                .iter()
                .map(|(file, _)| primary_family(&fonts::css_font_family(file).unwrap_or_default()))
                .collect();
            self.current_fonts = Some((Instant::now(), current));
        }
        let current_fonts = self
            .current_fonts
            .as_ref()
            .map(|(_, c)| c.clone())
            .unwrap_or_default();
        let busy = self
            .font_job
            .as_ref()
            .map(|j| j.lock().map(|j| j.request.font.family).unwrap_or(""));

        let mut picked: Option<(FontTarget, String)> = None;
        let mut download: Option<FontRequest> = None;
        let filter = self.font_filter.to_lowercase();

        let mut font_menu = |ui: &mut egui::Ui, target: FontTarget, current: &str| {
            egui::ComboBox::from_id_salt(("font", format!("{target:?}")))
                .selected_text(current)
                .width(230.0)
                .height(380.0)
                .show_ui(ui, |ui| {
                    let missing: Vec<_> =
                        NERD_FONTS.iter().filter(|n| !is_installed(n.family)).collect();
                    if !missing.is_empty() {
                        ui.label(
                            RichText::new("Nerd Fonts (click to download)")
                                .size(11.0)
                                .color(kit.sub),
                        );
                        for nf in missing {
                            if filter.is_empty() || nf.family.to_lowercase().contains(&filter) {
                                if ui.selectable_label(false, format!("⬇  {}", nf.family)).clicked() {
                                    download = Some(FontRequest { font: *nf, target });
                                }
                            }
                        }
                        ui.separator();
                    }
                    ui.label(RichText::new("Installed").size(11.0).color(kit.sub));
                    for family in installed
                        .iter()
                        .filter(|f| filter.is_empty() || f.to_lowercase().contains(&filter))
                    {
                        if ui.selectable_label(family == current, family).clicked() {
                            picked = Some((target, family.clone()));
                        }
                    }
                });
        };

        kit::group(ui, kit, "Fonts", |g| {
            g.row("Search fonts", "Filters the lists below.", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.font_filter)
                        .hint_text("e.g. mono, nerd")
                        .desired_width(230.0),
                );
            });
            g.row(
                "Everything",
                "Use one font for every module below.",
                |ui| font_menu(ui, FontTarget::All, "Choose…"),
            );
            for ((file, label), current) in FONT_TARGETS.iter().zip(&current_fonts) {
                let hint = if *file == "browser.css" {
                    "Applies to new browser windows."
                } else {
                    ""
                };
                g.row(label, hint, |ui| font_menu(ui, FontTarget::One(file), current));
            }
        });

        kit::group(ui, kit, "Nerd Fonts", |g| {
            g.custom(|ui| {
                ui.add(
                    egui::Label::new(
                        RichText::new("Fonts patched with thousands of icons, which roobar uses for its glyphs. Click one to download it from github.com/ryanoasis/nerd-fonts and install it just for you (no admin).")
                            .size(12.0)
                            .color(kit.sub),
                    )
                    .wrap(),
                );
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    for nf in NERD_FONTS {
                        if busy == Some(nf.family) {
                            ui.add(egui::Spinner::new().size(14.0));
                            kit::chip(ui, kit, &format!("Installing {}…", nf.family), kit.sub);
                        } else if is_installed(nf.family) {
                            kit::chip(
                                ui,
                                kit,
                                &format!("✓ {}", nf.family),
                                Color32::from_rgb(52, 199, 89),
                            );
                        } else if kit::button(ui, kit, &format!("⬇ {}", nf.family), false).clicked()
                        {
                            download = Some(FontRequest {
                                font: *nf,
                                target: FontTarget::InstallOnly,
                            });
                        }
                    }
                });
            });
        });

        if let Some((target, family)) = picked {
            self.apply_font_choice(target, &family);
        }
        if download.is_some() {
            self.font_confirm = download;
        }

        if let Some(req) = self.font_confirm {
            let mut answer = None;
            egui::Modal::new(egui::Id::new("font-download-confirm")).show(&ctx, |ui| {
                ui.set_width(380.0);
                ui.label(
                    RichText::new(format!("Download {}?", req.font.family))
                        .size(16.0)
                        .color(kit.fg),
                );
                ui.add_space(6.0);
                let then = match req.target {
                    FontTarget::One(file) => format!(
                        " Then it's used for {}.",
                        FONT_TARGETS
                            .iter()
                            .find(|(f, _)| *f == file)
                            .map_or(file, |(_, l)| l)
                    ),
                    FontTarget::All => " Then it's used everywhere.".to_string(),
                    FontTarget::InstallOnly => String::new(),
                };
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "It's downloaded from the official Nerd Fonts release on GitHub and installed for your user only, in your local font folder. No administrator rights needed.{then}"
                        ))
                        .size(12.0)
                        .color(kit.sub),
                    )
                    .wrap(),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if kit::button(ui, kit, "Download & install", true).clicked() {
                        answer = Some(true);
                    }
                    if kit::button(ui, kit, "Cancel", false).clicked() {
                        answer = Some(false);
                    }
                });
            });
            match answer {
                Some(true) => {
                    self.font_confirm = None;
                    self.start_font_job(req, &ctx);
                }
                Some(false) => self.font_confirm = None,
                None => {}
            }
        }
    }

    pub(crate) fn apply_font_choice(&mut self, target: FontTarget, family: &str) {
        use chickenwm_core::fonts::{set_css_font, FONT_TARGETS};
        let files: Vec<&str> = match target {
            FontTarget::One(file) => vec![file],
            FontTarget::All => FONT_TARGETS
                .iter()
                .map(|(f, _)| *f)
                .filter(|f| chickenwm_core::config::config_dir().join(f).exists())
                .collect(),
            FontTarget::InstallOnly => return,
        };
        self.current_fonts = None;
        let failed: Vec<String> = files
            .iter()
            .filter_map(|f| set_css_font(f, family).err().map(|e| format!("{f}: {e}")))
            .collect();
        self.status_banner = if failed.is_empty() {
            format!("Font set to {family}")
        } else {
            format!("Couldn't set the font for {}", failed.join(", "))
        };
    }

    pub(crate) fn start_font_job(&mut self, request: FontRequest, ctx: &egui::Context) {
        if self.font_job.is_some() {
            self.status_banner = "Another font is still installing".to_string();
            return;
        }
        let job = Arc::new(std::sync::Mutex::new(FontJob {
            request,
            result: None,
        }));
        self.font_job = Some(Arc::clone(&job));
        self.status_banner = format!("Downloading {}…", request.font.family);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = chickenwm_core::fonts::install_nerd_font(&request.font)
                .map_err(|e| format!("{e:#}"));
            if let Ok(mut j) = job.lock() {
                j.result = Some(result);
            }
            ctx.request_repaint();
        });
    }

    pub(crate) fn poll_font_job(&mut self) {
        let Some(job) = &self.font_job else { return };
        let finished = job
            .lock()
            .ok()
            .and_then(|mut j| j.result.take().map(|r| (j.request, r)));
        let Some((request, result)) = finished else {
            return;
        };
        self.font_job = None;
        match result {
            Ok(n) => {
                self.installed_fonts = Some(chickenwm_core::fonts::installed_families(true));
                self.status_banner = format!("Installed {} ({n} files)", request.font.family);
                if request.target != FontTarget::InstallOnly {
                    self.apply_font_choice(request.target, request.font.family);
                }
            }
            Err(e) => {
                self.status_banner = format!("Couldn't install {}: {e}", request.font.family)
            }
        }
    }
}
