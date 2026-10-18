use crate::kit::{self, Kit};
use crate::SettingsApp;
use chickenwm_core::config::WindowRuleV2;
use chickenwm_core::window_rules::CompiledRule;
use eframe::egui::{self, Color32, RichText};
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    GetAncestor, GetClassNameW, GetCursorPos, GetWindowTextW, GetWindowThreadProcessId,
    WindowFromPoint, GA_ROOT,
};

fn process_name_for_hwnd(hwnd: windows::Win32::Foundation::HWND) -> String {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return String::new();
        }
        let Ok(hproc) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buf = [0u16; 260];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(hproc, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(hproc);
        if !ok || len == 0 {
            return String::new();
        }
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        std::path::Path::new(&full)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(&full)
            .to_string()
    }
}

/// Builds a combined `windowrulev2` matcher string from class, title, initialTitle, and process regexes.
pub fn build_combined_matcher(
    class_re: &str,
    title_re: &str,
    initial_title_re: &str,
    process_re: &str,
) -> String {
    let wrap = |prefix: &str, val: &str| -> Option<String> {
        let v = val.trim();
        if v.is_empty() {
            return None;
        }
        if v.starts_with("class:")
            || v.starts_with("title:")
            || v.starts_with("initialTitle:")
            || v.starts_with("process:")
        {
            return Some(v.to_string());
        }
        if v.starts_with('^') || v.ends_with('$') || v.contains(".*") {
            Some(format!("{prefix}:{v}"))
        } else {
            Some(format!("{prefix}:^({v})$"))
        }
    };
    [
        wrap("class", class_re),
        wrap("title", title_re),
        wrap("initialTitle", initial_title_re),
        wrap("process", process_re),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(" ")
}

impl SettingsApp {
    pub(crate) fn page_window_rules(&mut self, ui: &mut egui::Ui, kit: &Kit) {
        // Inspector: the next left click anywhere picks the window under the cursor.
        if self.inspecting_crosshair {
            unsafe {
                if (GetAsyncKeyState(VK_LBUTTON.0 as i32) as u16 & 0x8000) != 0 {
                    let mut pt = POINT::default();
                    if GetCursorPos(&mut pt).is_ok() {
                        let raw = WindowFromPoint(pt);
                        let root = GetAncestor(raw, GA_ROOT);
                        let target = if !root.0.is_null() { root } else { raw };
                        if !target.0.is_null() {
                            let mut cbuf = [0u16; 128];
                            let clen = GetClassNameW(target, &mut cbuf);
                            if clen > 0 {
                                self.inspected_class =
                                    String::from_utf16_lossy(&cbuf[..clen as usize]);
                            }
                            let mut tbuf = [0u16; 256];
                            let tlen = GetWindowTextW(target, &mut tbuf);
                            self.inspected_title = if tlen > 0 {
                                String::from_utf16_lossy(&tbuf[..tlen as usize])
                            } else {
                                String::new()
                            };
                            self.inspected_process = process_name_for_hwnd(target);
                        }
                    }
                    self.inspecting_crosshair = false;
                }
            }
        }

        let preview_matcher = build_combined_matcher(
            &self.inspected_class,
            &self.inspected_title,
            &self.inspected_initial_title,
            &self.inspected_process,
        );
        let draft_problems = if preview_matcher.is_empty() {
            Vec::new()
        } else {
            CompiledRule::new(&WindowRuleV2 {
                effect: self.new_rule_effect.trim().to_string(),
                matcher: preview_matcher.clone(),
            })
            .problems()
        };

        let mut add = false;
        kit::group(ui, kit, "New rule", |g| {
            g.row(
                "Pick a window",
                if self.inspecting_crosshair {
                    "Now click any window…"
                } else {
                    "Click the button, then click a window to read its class, title and process."
                },
                |ui| {
                    let label = if self.inspecting_crosshair {
                        "Cancel"
                    } else {
                        "🎯 Pick"
                    };
                    if kit::button(ui, kit, label, self.inspecting_crosshair).clicked() {
                        self.inspecting_crosshair = !self.inspecting_crosshair;
                    }
                },
            );
            g.row(
                "Window class",
                "Case-insensitive regex matched against Win32 class (class:).",
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.inspected_class)
                            .desired_width(210.0)
                            .hint_text("e.g. Taskmgr"),
                    );
                },
            );
            g.row(
                "Window title",
                "Optional regex matched against current window title (title:).",
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.inspected_title)
                            .desired_width(210.0)
                            .hint_text("e.g. Picture-in-Picture"),
                    );
                },
            );
            g.row(
                "Initial title",
                "Optional regex matched against the window's creation title (initialTitle:).",
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.inspected_initial_title)
                            .desired_width(210.0)
                            .hint_text("e.g. Untitled.*"),
                    );
                },
            );
            g.row(
                "Process name",
                "Optional regex matched against executable name (process:).",
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.inspected_process)
                            .desired_width(210.0)
                            .hint_text("e.g. spotify.exe"),
                    );
                },
            );
            g.row(
                "Rule effect",
                "float · tile · workspace 3 · monitor 0 · opacity 0.9 0.8 · size 800 600 · move 100 100 · center · fullscreen · maximize · nofocus",
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_rule_effect).desired_width(210.0),
                    );
                },
            );
            g.custom(|ui| {
                ui.horizontal_wrapped(|ui| {
                    for preset in [
                        "float",
                        "tile",
                        "center",
                        "workspace 2",
                        "monitor 0",
                        "opacity 0.92 0.85",
                        "size 900 650",
                        "move 120 80",
                        "fullscreen",
                        "maximize",
                        "nofocus",
                    ] {
                        if kit::button(ui, kit, preset, self.new_rule_effect == preset).clicked() {
                            self.new_rule_effect = preset.to_string();
                        }
                    }
                });
                if !draft_problems.is_empty() {
                    ui.add_space(4.0);
                    for p in &draft_problems {
                        ui.label(
                            RichText::new(format!("⚠ {p}"))
                                .size(12.0)
                                .color(Color32::from_rgb(235, 160, 40)),
                        );
                    }
                }
            });
            g.row(
                "Matcher preview",
                if preview_matcher.is_empty() {
                    "Fill at least one matcher above (all non-empty matchers must hit)."
                } else {
                    &preview_matcher
                },
                |ui| {
                    let can_add = !preview_matcher.is_empty() && draft_problems.is_empty();
                    if kit::button(ui, kit, "Add rule", can_add).clicked() && can_add {
                        add = true;
                    }
                },
            );
        });
        if add && !preview_matcher.is_empty() {
            let raw = format!("{}, {}", self.new_rule_effect.trim(), preview_matcher);
            self.doc.add_directive("windowrulev2", &raw);
            self.cfg.window_rules.push(WindowRuleV2 {
                effect: self.new_rule_effect.trim().to_string(),
                matcher: preview_matcher,
            });
            self.sync_config_and_notify_labeled("Add window rule");
        }

        let rule_issues: Vec<_> = self
            .config_issues
            .iter()
            .filter(|i| i.key.as_deref() == Some("windowrulev2"))
            .cloned()
            .collect();

        let mut remove = None;
        kit::group(ui, kit, "Rules", |g| {
            if self.cfg.window_rules.is_empty() {
                g.row("No rules yet", "", |_| {});
            }
            for (i, r) in self.cfg.window_rules.iter().enumerate() {
                let problems = CompiledRule::new(r).problems();
                g.row(&r.effect, &r.matcher, |ui| {
                    if ui.small_button("Remove").clicked() {
                        remove = Some(i);
                    }
                    for p in &problems {
                        let line_tag = rule_issues
                            .iter()
                            .find(|iss| iss.message.contains(p))
                            .map(|iss| format!("L{}: ", iss.line))
                            .unwrap_or_default();
                        kit::chip(
                            ui,
                            kit,
                            &format!("⚠ {line_tag}{p}"),
                            Color32::from_rgb(235, 160, 40),
                        );
                    }
                });
            }
        });
        if let Some(i) = remove {
            self.cfg.window_rules.remove(i);
            let lines: Vec<String> = self
                .cfg
                .window_rules
                .iter()
                .map(|r| format!("{}, {}", r.effect, r.matcher))
                .collect();
            self.doc.replace_directives("windowrule", &[]);
            self.doc.replace_directives("windowrulev2", &lines);
            self.sync_config_and_notify_labeled("Remove window rule");
        }
    }
}
