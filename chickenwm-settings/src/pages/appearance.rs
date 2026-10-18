use crate::kit::{self, Kit};
use crate::{bool_str, theme_ui, SettingsApp};
use chickenwm_core::config::{config_dir, RgbaColor};
use chickenwm_core::ipc::{send_ipc_command_blocking, IpcCommand, PIPE_NAME_DAEMON};
use eframe::egui::{self, Color32};

impl SettingsApp {
    pub(crate) fn theme_tools_ui(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        use chickenwm_core::theme_tools;
        let mut auto_changed = false;
        kit::group(ui, kit, "Light and dark by the clock", |g| {
            g.row(
                "Switch automatically",
                "Dark from evening to morning: at set times, or from sunset to sunrise.",
                |ui| {
                    let mut mode = self.auto_dark.mode.clone();
                    let options = [
                        ("off".to_string(), "Off"),
                        ("times".to_string(), "Set times"),
                        ("sun".to_string(), "Sunset"),
                    ];
                    let opts: Vec<(&str, &str)> =
                        options.iter().map(|(v, l)| (v.as_str(), *l)).collect();
                    let mut cur = opts.iter().position(|(v, _)| *v == mode).unwrap_or(0);
                    let indices: Vec<(usize, &str)> =
                        opts.iter().enumerate().map(|(i, (_, l))| (i, *l)).collect();
                    if kit::segmented(ui, kit, &mut cur, &indices) {
                        mode = opts[cur].0.to_string();
                        self.auto_dark.mode = mode;
                        auto_changed = true;
                    }
                },
            );
            if self.auto_dark.mode == "times" {
                for (label, value) in [
                    ("Light from", &mut self.auto_dark.light_at),
                    ("Dark from", &mut self.auto_dark.dark_at),
                ] {
                    g.row(label, "24-hour time, like 07:00.", |ui| {
                        let ok = theme_tools::parse_hm(value).is_some();
                        let r = ui.add(
                            egui::TextEdit::singleline(value)
                                .desired_width(70.0)
                                .text_color_opt((!ok).then_some(Color32::from_rgb(220, 80, 70))),
                        );
                        if r.lost_focus() && theme_tools::parse_hm(value).is_some() {
                            auto_changed = true;
                        }
                    });
                }
            }
            if self.auto_dark.mode == "sun" {
                let mut loc = match (self.auto_dark.latitude, self.auto_dark.longitude) {
                    (Some(a), Some(b)) => format!("{a:.4}, {b:.4}"),
                    _ => String::new(),
                };
                let (y, m, d, _, off) = theme_tools::local_clock();
                let hint = match (self.auto_dark.latitude, self.auto_dark.longitude) {
                    (Some(lat), Some(lon)) => match theme_tools::sun_times(lat, lon, y, m, d, off) {
                        Some((rise, set)) => format!(
                            "Today: light at {}, dark at {}.",
                            theme_tools::fmt_hm(rise),
                            theme_tools::fmt_hm(set)
                        ),
                        None => "No sunrise or sunset there today.".to_string(),
                    },
                    _ => "Latitude, longitude — e.g. 41.01, 28.97 for Istanbul.".to_string(),
                };
                g.row("Location", &hint, |ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut loc)
                            .desired_width(150.0)
                            .hint_text("41.01, 28.97"),
                    );
                    if r.lost_focus() {
                        let mut it = loc.split(',').map(|s| s.trim().parse::<f64>());
                        if let (Some(Ok(lat)), Some(Ok(lon))) = (it.next(), it.next()) {
                            self.auto_dark.latitude = Some(lat);
                            self.auto_dark.longitude = Some(lon);
                            auto_changed = true;
                        }
                    }
                });
            }
        });
        if auto_changed {
            theme_tools::write_auto_dark_keys(&mut self.doc, &self.auto_dark);
            self.cfg.theme.auto_dark = self.auto_dark.clone();
            self.theme_editor.draft.auto_dark = self.auto_dark.clone();
            let (y, m, d, minute, off) = theme_tools::local_clock();
            match theme_tools::desired_mode(&self.auto_dark, y, m, d, minute, off) {
                Some(want) if want != self.cfg.theme.mode => {
                    self.theme_editor.draft.mode = want;
                    self.apply_system_theme();
                }
                _ => self.sync_config_and_notify_labeled("Auto-dark schedule"),
            }
        }

        let mut wallpaper_accent = false;
        let mut export = false;
        let mut import = false;
        kit::group(ui, kit, "More", |g| {
            g.row(
                "Accent from wallpaper",
                "Picks the most prominent colour of the current wallpaper image.",
                |ui| {
                    wallpaper_accent = kit::button(ui, kit, "Use wallpaper colour", false).clicked();
                },
            );
            g.row(
                "Theme file",
                "Share a theme as one .chickentheme file, or load someone else's.",
                |ui| {
                    import = kit::button(ui, kit, "Import", false).clicked();
                    export = kit::button(ui, kit, "Export", false).clicked();
                    ui.add(egui::TextEdit::singleline(&mut self.theme_file_path).desired_width(220.0));
                },
            );
        });
        if wallpaper_accent {
            let path = self
                .cfg
                .wallpapers
                .first()
                .map(|w| chickenwm_core::config::expand_home(&w.path));
            match path.as_deref().and_then(theme_tools::accent_from_image) {
                Some(c) => {
                    self.theme_editor.draft.accent = c;
                    self.theme_editor.draft.preset.clear();
                    self.apply_system_theme();
                }
                None => {
                    self.status_banner =
                        "No wallpaper image, or it has no clear colour".to_string()
                }
            }
        }
        let path = std::path::PathBuf::from(self.theme_file_path.trim().trim_matches('"'));
        if export {
            let name = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "My theme".into());
            self.status_banner =
                match std::fs::write(&path, theme_tools::export_theme(&self.cfg.theme, &name)) {
                    Ok(()) => format!("Saved {}", path.display()),
                    Err(e) => format!("Couldn't save: {e}"),
                };
        }
        if import {
            match std::fs::read_to_string(&path)
                .map_err(anyhow::Error::from)
                .and_then(|t| theme_tools::import_theme(&t, &self.cfg.theme))
            {
                Ok((name, theme)) => {
                    self.theme_editor.draft = theme.clone();
                    self.auto_dark = theme.auto_dark.clone();
                    theme_tools::write_auto_dark_keys(&mut self.doc, &self.auto_dark);
                    self.apply_system_theme();
                    self.status_banner = format!("Theme \"{name}\" applied");
                }
                Err(e) => self.status_banner = format!("Couldn't import: {e}"),
            }
        }
    }

    pub(crate) fn page_appearance(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        let applied = self.cfg.theme.clone();
        if self.theme_editor.ui(ui, kit, &applied) {
            self.apply_system_theme();
        }
        if let Some((app, mode)) = theme_ui::per_app_ui(ui, kit, &self.cfg.theme) {
            match chickenwm_core::theme::set_app_mode(app, mode) {
                Ok(theme) => {
                    self.doc
                        .set_category_key("theme", "app_modes", &theme.app_modes_string());
                    self.cfg.theme.app_modes = theme.app_modes;
                    self.theme_editor.draft.app_modes = self.cfg.theme.app_modes.clone();
                    self.status_banner = format!("{app}: {}", mode.as_str());
                }
                Err(e) => self.status_banner = format!("Couldn't change {app}: {e}"),
            }
        }
        self.theme_tools_ui(ui, kit);

        let mut changed = false;
        let mut border_changed = false;
        kit::group(ui, kit, "Windows", |g| {
            g.row(
                "Active border",
                "Two colors make a gradient at the given angle.",
                |ui| {
                    let b = &mut self.cfg.general.col_active_border;
                    if ui
                        .add(
                            egui::DragValue::new(&mut b.angle_deg)
                                .range(0.0..=360.0)
                                .suffix("°")
                                .speed(1.0),
                        )
                        .changed()
                    {
                        border_changed = true;
                    }
                    let sec = b.secondary.unwrap_or(b.primary);
                    let mut c2 = [sec.r, sec.g, sec.b, sec.a];
                    if ui
                        .color_edit_button_srgba_unmultiplied(&mut c2)
                        .on_hover_text("Second color")
                        .changed()
                    {
                        b.secondary = Some(RgbaColor {
                            r: c2[0],
                            g: c2[1],
                            b: c2[2],
                            a: c2[3],
                        });
                        border_changed = true;
                    }
                    let mut c1 = [b.primary.r, b.primary.g, b.primary.b, b.primary.a];
                    if ui
                        .color_edit_button_srgba_unmultiplied(&mut c1)
                        .on_hover_text("First color")
                        .changed()
                    {
                        b.primary = RgbaColor {
                            r: c1[0],
                            g: c1[1],
                            b: c1[2],
                            a: c1[3],
                        };
                        border_changed = true;
                    }
                },
            );
            g.row(
                "Border rotation speed",
                "0 keeps the gradient still.",
                |ui| {
                    if ui
                        .add(
                            egui::Slider::new(&mut self.cfg.decoration.border_anim_speed, 0.0..=5.0)
                                .max_decimals(2),
                        )
                        .changed()
                    {
                        self.doc.set_category_key(
                            "decoration",
                            "border_anim_speed",
                            &format!("{:.2}", self.cfg.decoration.border_anim_speed),
                        );
                        changed = true;
                    }
                },
            );
            g.row("Corner rounding", "", |ui| {
                if ui
                    .add(egui::Slider::new(&mut self.cfg.decoration.rounding, 0..=28).suffix(" px"))
                    .changed()
                {
                    self.doc.set_category_key(
                        "decoration",
                        "rounding",
                        &self.cfg.decoration.rounding.to_string(),
                    );
                    changed = true;
                }
            });
            g.row("Focused window opacity", "", |ui| {
                if ui
                    .add(
                        egui::Slider::new(&mut self.cfg.decoration.active_opacity, 0.3..=1.0)
                            .max_decimals(2),
                    )
                    .changed()
                {
                    self.doc.set_category_key(
                        "decoration",
                        "active_opacity",
                        &format!("{:.2}", self.cfg.decoration.active_opacity),
                    );
                    changed = true;
                }
            });
            g.row("Other windows' opacity", "", |ui| {
                if ui
                    .add(
                        egui::Slider::new(&mut self.cfg.decoration.inactive_opacity, 0.3..=1.0)
                            .max_decimals(2),
                    )
                    .changed()
                {
                    self.doc.set_category_key(
                        "decoration",
                        "inactive_opacity",
                        &format!("{:.2}", self.cfg.decoration.inactive_opacity),
                    );
                    changed = true;
                }
            });
            g.row(
                "Backdrop blur",
                "Acrylic / Mica behind supported windows.",
                |ui| {
                    if kit::toggle(ui, kit, &mut self.cfg.decoration.blur_enabled).changed() {
                        self.doc.set_category_key(
                            "decoration",
                            "blur_enabled",
                            bool_str(self.cfg.decoration.blur_enabled),
                        );
                        changed = true;
                    }
                },
            );
        });
        if border_changed {
            let spec = self.cfg.general.col_active_border.to_hypr_string();
            self.doc
                .set_category_key("general", "col.active_border", &spec);
            changed = true;
        }
        if changed {
            self.sync_config_and_notify_labeled("Window appearance");
        }

        let mut islands_changed = false;
        kit::group(ui, kit, "Bar", |g| {
            g.row(
                "Island separation",
                "Each group of widgets in its own pill. Off: one background and border for the whole bar, same layout.",
                |ui| {
                    islands_changed = kit::toggle(ui, kit, &mut self.bar_islands).clicked();
                },
            );
        });
        if islands_changed {
            let path = config_dir().join("roobar.css");
            let source = std::fs::read_to_string(&path).unwrap_or_default();
            let value = if self.bar_islands { "true" } else { "false" };
            let updated = chickenwm_core::theme::set_root_vars(&source, &[("--islands", value.to_owned())]);
            if let Err(e) = std::fs::write(&path, updated) {
                tracing::warn!("settings: couldn't write roobar.css: {e}");
            }
        }

        let mut save_css = false;
        let mut load_css: Option<&'static str> = None;
        kit::group(ui, kit, "Advanced", |g| {
            g.row(
                "Edit stylesheets",
                "Fonts, sizes and colors of each app, in CSS. Changes show up live.",
                |ui| {
                    kit::toggle(ui, kit, &mut self.show_css_editor);
                },
            );
            if self.show_css_editor {
                g.custom(|ui| {
                    ui.horizontal(|ui| {
                        let files: [(&str, &str); 6] = [
                            ("roobar.css", "Bar"),
                            ("launcher.css", "Launcher"),
                            ("files.css", "Files"),
                            ("settings.css", "Settings"),
                            ("browser.css", "Browser"),
                            ("notifications.css", "Notifications"),
                        ];
                        let mut sel = files
                            .iter()
                            .position(|f| f.0 == self.selected_css_file)
                            .unwrap_or(0);
                        let opts: Vec<(usize, &str)> =
                            files.iter().enumerate().map(|(i, f)| (i, f.1)).collect();
                        if kit::segmented(ui, kit, &mut sel, &opts) {
                            load_css = Some(files[sel].0);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if kit::button(ui, kit, "Save", true).clicked() {
                                save_css = true;
                            }
                        });
                    });
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                        ui.add_sized(
                            [ui.available_width(), 280.0],
                            egui::TextEdit::multiline(&mut self.css_editor_buffer)
                                .code_editor()
                                .desired_rows(14),
                        );
                    });
                });
            }
        });
        if let Some(f) = load_css {
            self.selected_css_file = f.to_string();
            self.css_editor_buffer =
                std::fs::read_to_string(config_dir().join(f)).unwrap_or_default();
        }
        if save_css {
            let path = config_dir().join(&self.selected_css_file);
            match std::fs::write(&path, &self.css_editor_buffer) {
                Ok(()) => {
                    let _ = send_ipc_command_blocking(PIPE_NAME_DAEMON, &IpcCommand::ReloadConfig);
                    self.status_banner = format!("Saved {}", self.selected_css_file);
                }
                Err(e) => {
                    self.status_banner = format!("Couldn't save {}: {e}", self.selected_css_file)
                }
            }
        }
    }
}
