//! Phase 2.3 & Phase 10.3: Hot-Reloading CSS Theme Engine & File System Watcher (`notify`).
//! Supports CSS selectors, custom properties (`--bg-color`, `--border-radius`, `var(--...)`),
//! RGBA/Hex colors, padding, margins, borders, font families, and transitions.

use crate::config::{config_dir, RgbaColor};
use anyhow::{Context, Result};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::thread;
use tracing::{info, warn};

/// Computed style properties for a single CSS selector (`.container`, `.search-input`, `.row:selected`, etc.).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ComputedStyle {
    pub background_color: Option<RgbaColor>,
    pub text_color: Option<RgbaColor>,
    pub border_color: Option<RgbaColor>,
    pub accent_color: Option<RgbaColor>,
    pub border_width: Option<f32>,
    pub border_radius: Option<f32>,
    pub padding: Option<[f32; 4]>, // [top, right, bottom, left]
    pub margin: Option<[f32; 4]>,
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub row_height: Option<f32>,
    pub icon_size: Option<f32>,
    pub opacity: Option<f32>,
    pub backdrop_blur: Option<f32>,
    pub transition_ms: Option<f32>,
    pub raw_properties: HashMap<String, String>,
}

/// Parsed CSS Theme Sheet with `:root` custom variables and selector rules.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CssStyleSheet {
    pub custom_properties: HashMap<String, String>,
    pub rules: HashMap<String, ComputedStyle>,
}

fn strip_css_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '/' && chars.peek() == Some(&'*') {
            let _ = chars.next();
            while let Some(nc) = chars.next() {
                if nc == '*' && chars.peek() == Some(&'/') {
                    let _ = chars.next();
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn parse_px(val: &str) -> Option<f32> {
    let v = val
        .trim()
        .trim_end_matches("px")
        .trim_end_matches("rem")
        .trim_end_matches("pt")
        .trim();
    v.parse::<f32>().ok()
}

fn parse_box_4(val: &str) -> Option<[f32; 4]> {
    let nums: Vec<f32> = val
        .split_whitespace()
        .filter_map(parse_px)
        .collect();
    match nums.len() {
        1 => Some([nums[0], nums[0], nums[0], nums[0]]),
        2 => Some([nums[0], nums[1], nums[0], nums[1]]),
        4 => Some([nums[0], nums[1], nums[2], nums[3]]),
        _ => None,
    }
}

fn parse_duration_ms(val: &str) -> Option<f32> {
    for tok in val.split_whitespace() {
        if let Some(ms) = tok.strip_suffix("ms") {
            if let Ok(n) = ms.parse::<f32>() {
                return Some(n);
            }
        } else if let Some(s) = tok.strip_suffix('s') {
            if let Ok(n) = s.parse::<f32>() {
                return Some(n * 1000.0);
            }
        }
    }
    None
}

impl CssStyleSheet {
    /// Parses a CSS stylesheet string into a `CssStyleSheet`.
    pub fn parse(source: &str) -> Self {
        let cleaned = strip_css_comments(source);
        let mut sheet = Self::default();

        // First pass: collect blocks `selector { prop: val; ... }`
        let mut raw_blocks: Vec<(Vec<String>, Vec<(String, String)>)> = Vec::new();
        for chunk in cleaned.split('}') {
            let Some((sel_part, body_part)) = chunk.split_once('{') else {
                continue;
            };
            let selectors: Vec<String> = sel_part
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if selectors.is_empty() {
                continue;
            }
            let mut decls = Vec::new();
            for decl in body_part.split(';') {
                let Some((prop, val)) = decl.split_once(':') else {
                    continue;
                };
                let prop = prop.trim().to_string();
                let val = val.trim().to_string();
                if prop.starts_with("--") {
                    sheet.custom_properties.insert(prop.clone(), val.clone());
                }
                decls.push((prop, val));
            }
            raw_blocks.push((selectors, decls));
        }

        // Second pass: resolve `var(--name)` and populate `ComputedStyle`
        for (selectors, decls) in raw_blocks {
            for sel in selectors {
                let style = sheet.rules.entry(sel).or_default();
                for (prop, raw_val) in &decls {
                    let val = Self::resolve_var(raw_val, &sheet.custom_properties);
                    style.raw_properties.insert(prop.clone(), val.clone());
                    Self::apply_Declaration(style, prop, &val);
                }
            }
        }

        sheet
    }

    fn resolve_var(input: &str, vars: &HashMap<String, String>) -> String {
        let mut s = input.to_string();
        for _ in 0..4 {
            let Some(start) = s.find("var(") else {
                break;
            };
            let Some(end_rel) = s[start..].find(')') else {
                break;
            };
            let end = start + end_rel;
            let inner = &s[start + 4..end];
            let (var_name, fallback) = match inner.split_once(',') {
                Some((vn, fb)) => (vn.trim(), Some(fb.trim())),
                None => (inner.trim(), None),
            };
            let replacement = vars
                .get(var_name)
                .map(|v| v.as_str())
                .or(fallback)
                .unwrap_or("")
                .to_string();
            s.replace_range(start..=end, &replacement);
        }
        s
    }

    #[allow(non_snake_case)]
    fn apply_Declaration(style: &mut ComputedStyle, prop: &str, val: &str) {
        match prop.to_ascii_lowercase().as_str() {
            "background" | "background-color" => {
                if let Some(c) = RgbaColor::parse(val) {
                    style.background_color = Some(c);
                }
            }
            "color" => {
                if let Some(c) = RgbaColor::parse(val) {
                    style.text_color = Some(c);
                }
            }
            "border-color" => {
                if let Some(c) = RgbaColor::parse(val) {
                    style.border_color = Some(c);
                }
            }
            "accent-color" | "--accent-color" => {
                if let Some(c) = RgbaColor::parse(val) {
                    style.accent_color = Some(c);
                }
            }
            "border" => {
                for tok in val.split_whitespace() {
                    if let Some(px) = parse_px(tok) {
                        style.border_width = Some(px);
                    } else if let Some(c) = RgbaColor::parse(tok) {
                        style.border_color = Some(c);
                    }
                }
            }
            "border-width" => {
                if let Some(px) = parse_px(val) {
                    style.border_width = Some(px);
                }
            }
            "border-radius" => {
                if let Some(px) = parse_px(val) {
                    style.border_radius = Some(px);
                }
            }
            "padding" => {
                if let Some(b) = parse_box_4(val) {
                    style.padding = Some(b);
                }
            }
            "margin" => {
                if let Some(b) = parse_box_4(val) {
                    style.margin = Some(b);
                }
            }
            "font-family" => {
                style.font_family = Some(val.trim_matches('"').trim_matches('\'').to_string());
            }
            "font-size" => {
                if let Some(px) = parse_px(val) {
                    style.font_size = Some(px);
                }
            }
            "height" | "row-height" => {
                if let Some(px) = parse_px(val) {
                    style.row_height = Some(px);
                }
            }
            "icon-size" => {
                if let Some(px) = parse_px(val) {
                    style.icon_size = Some(px);
                }
            }
            "opacity" => {
                if let Ok(op) = val.trim().parse::<f32>() {
                    style.opacity = Some(op.clamp(0.0, 1.0));
                }
            }
            "backdrop-filter" => {
                if let Some(idx) = val.find("blur(") {
                    if let Some(end) = val[idx..].find(')') {
                        if let Some(px) = parse_px(&val[idx + 5..idx + end]) {
                            style.backdrop_blur = Some(px);
                        }
                    }
                }
            }
            "transition" => {
                if let Some(ms) = parse_duration_ms(val) {
                    style.transition_ms = Some(ms);
                }
            }
            _ => {}
        }
    }

    /// Retrieve a custom variable (e.g. `--bg-color`) parsed as `RgbaColor`.
    pub fn var_color(&self, name: &str, fallback: RgbaColor) -> RgbaColor {
        self.custom_properties
            .get(name)
            .and_then(|v| RgbaColor::parse(v))
            .unwrap_or(fallback)
    }

    /// Retrieve a custom variable (e.g. `--border-radius`) parsed as `f32`.
    pub fn var_px(&self, name: &str, fallback: f32) -> f32 {
        self.custom_properties
            .get(name)
            .and_then(|v| parse_px(v))
            .unwrap_or(fallback)
    }
}

/// Phase 10.3: Sleek default dark-mode CSS for `chickenwm-launcher` (`~/.config/chickenwm/launcher.css`).
pub const DEFAULT_LAUNCHER_CSS: &str = r#"/* ChickenWM Application Launcher Theme (~/.config/chickenwm/launcher.css) */
:root {
    --bg-color: rgba(24, 24, 37, 0.94);
    --surface-color: rgba(30, 30, 46, 0.95);
    --fg-color: #cdd6f4;
    --subtext-color: #a6adc8;
    --accent-color: #33ccff;
    --accent-secondary: #00ff99;
    --selected-bg: rgba(51, 204, 255, 0.18);
    --border-color: rgba(51, 204, 255, 0.85);
    --border-radius: 12px;
    --border-width: 2px;
    --font-family: "Segoe UI", "Inter", sans-serif;
    --font-size: 15px;
    --row-height: 44px;
    --icon-size: 28px;
}

.launcher-window {
    background-color: var(--bg-color);
    color: var(--fg-color);
    border: 2px solid var(--border-color);
    border-radius: var(--border-radius);
    padding: 14px;
    font-family: var(--font-family);
    font-size: var(--font-size);
    backdrop-filter: blur(16px);
    transition: all 120ms;
}

.search-input {
    background-color: var(--surface-color);
    color: var(--fg-color);
    border: 1px solid rgba(88, 91, 112, 0.6);
    border-radius: 8px;
    padding: 10px 14px;
    font-size: 18px;
}

.result-row {
    background-color: rgba(0, 0, 0, 0);
    color: var(--fg-color);
    border-radius: 8px;
    padding: 8px 12px;
    height: var(--row-height);
    icon-size: var(--icon-size);
}

.result-row:selected {
    background-color: var(--selected-bg);
    border-color: var(--accent-color);
    color: #ffffff;
}
"#;

/// Phase 10.3: Sleek default dark-mode CSS for `chickenwm-files` (`~/.config/chickenwm/files.css`).
pub const DEFAULT_FILES_CSS: &str = r#"/* ChickenWM Keyboard-First File Explorer Theme (~/.config/chickenwm/files.css) */
:root {
    --bg-color: #181825;
    --pane-bg: #1e1e2e;
    --sidebar-bg: #11111b;
    --fg-color: #cdd6f4;
    --dir-color: #89b4fa;
    --exec-color: #a6e3a1;
    --archive-color: #f9e2af;
    --media-color: #cba6f7;
    --accent-color: #33ccff;
    --selected-bg: rgba(51, 204, 255, 0.22);
    --border-color: #313244;
    --border-radius: 8px;
    --font-family: "Cascadia Code", "Consolas", "Segoe UI", monospace;
    --font-size: 14px;
    --row-height: 28px;
}

.files-window {
    background-color: var(--bg-color);
    color: var(--fg-color);
    font-family: var(--font-family);
    font-size: var(--font-size);
}

.pane {
    background-color: var(--pane-bg);
    border: 1px solid var(--border-color);
    border-radius: var(--border-radius);
    padding: 8px;
}

.row:selected {
    background-color: var(--selected-bg);
    color: #ffffff;
    border-radius: 6px;
}

.status-bar {
    background-color: var(--sidebar-bg);
    color: var(--fg-color);
    padding: 4px 10px;
}
"#;

/// Phase 10.3: Sleek default dark-mode CSS for `chickenwm-settings` (`~/.config/chickenwm/settings.css`).
pub const DEFAULT_SETTINGS_CSS: &str = r#"/* ChickenWM Settings Control Center Theme (~/.config/chickenwm/settings.css) */
:root {
    --bg-color: #181825;
    --card-bg: #1e1e2e;
    --sidebar-bg: #11111b;
    --fg-color: #cdd6f4;
    --subtext-color: #a6adc8;
    --accent-color: #33ccff;
    --accent-secondary: #00ff99;
    --danger-color: #f38ba8;
    --border-color: #313244;
    --border-radius: 10px;
    --font-family: "Segoe UI", sans-serif;
    --font-size: 14px;
}

.settings-window {
    background-color: var(--bg-color);
    color: var(--fg-color);
    font-family: var(--font-family);
    font-size: var(--font-size);
}

.sidebar-tab {
    background-color: rgba(0, 0, 0, 0);
    color: var(--subtext-color);
    border-radius: 8px;
    padding: 10px 14px;
}

.sidebar-tab:active {
    background-color: rgba(51, 204, 255, 0.20);
    color: var(--accent-color);
    border: 1px solid var(--accent-color);
}

.card {
    background-color: var(--card-bg);
    border: 1px solid var(--border-color);
    border-radius: var(--border-radius);
    padding: 16px;
}
"#;

/// Sleek default Waybar-style CSS for `roobar` (`~/.config/chickenwm/roobar.css`).
pub const DEFAULT_ROOBAR_CSS: &str = r#"/* ChickenWM Status Bar (roobar) Theme (~/.config/chickenwm/roobar.css) */
:root {
    --bar-bg: rgba(17, 17, 27, 0.88);
    --pill-bg: rgba(30, 30, 46, 0.94);
    --fg-color: #cdd6f4;
    --subtext-color: #a6adc8;
    --accent-color: #33ccff;
    --accent-secondary: #00ff99;
    --ws-active-bg: #33ccff;
    --ws-active-fg: #11111b;
    --ws-occupied-bg: rgba(51, 204, 255, 0.20);
    --ws-occupied-fg: #cdd6f4;
    --ws-empty-bg: rgba(49, 50, 68, 0.55);
    --ws-empty-fg: #6c7086;
    --border-color: rgba(51, 204, 255, 0.50);
    --border-radius: 10px;
    --pill-radius: 8px;
    --bar-height: 36px;
    --bar-margin-top: 6px;
    --bar-margin-x: 10px;
    --font-family: "Cascadia Code", "Segoe UI", sans-serif;
    --font-size: 13px;
}

window#roobar {
    background-color: var(--bar-bg);
    color: var(--fg-color);
    border: 1px solid var(--border-color);
    border-radius: var(--border-radius);
    font-family: var(--font-family);
    font-size: var(--font-size);
    height: var(--bar-height);
}

#workspaces {
    background-color: var(--pill-bg);
    border-radius: var(--pill-radius);
    padding: 2px 6px;
}

#workspaces button {
    background-color: var(--ws-empty-bg);
    color: var(--ws-empty-fg);
    border-radius: 6px;
    padding: 2px 8px;
    transition: all 140ms;
}

#workspaces button.occupied {
    background-color: var(--ws-occupied-bg);
    color: var(--ws-occupied-fg);
}

#workspaces button.active {
    background-color: var(--ws-active-bg);
    color: var(--ws-active-fg);
    border-color: var(--accent-secondary);
}

#window {
    background-color: var(--pill-bg);
    color: var(--fg-color);
    border-radius: var(--pill-radius);
    padding: 4px 12px;
}

#clock, #cpu, #memory, #layout, #custom-launcher {
    background-color: var(--pill-bg);
    color: var(--fg-color);
    border-radius: var(--pill-radius);
    padding: 4px 10px;
}
"#;

/// Ensures all default CSS files (`launcher.css`, `files.css`, `settings.css`, `roobar.css`) exist in `~/.config/chickenwm/`.
pub fn ensure_default_css_files() -> Result<()> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)?;

    let files = [
        ("launcher.css", DEFAULT_LAUNCHER_CSS),
        ("files.css", DEFAULT_FILES_CSS),
        ("settings.css", DEFAULT_SETTINGS_CSS),
        ("roobar.css", DEFAULT_ROOBAR_CSS),
    ];

    for (name, content) in files {
        let path = dir.join(name);
        if !path.exists() {
            std::fs::write(&path, content)
                .with_context(|| format!("Failed to write default {}", path.display()))?;
        }
    }
    Ok(())
}

/// Loads and parses one of the CSS files (`launcher.css`, `files.css`, `settings.css`) from `~/.config/chickenwm/`.
pub fn load_css_sheet(file_name: &str) -> Result<CssStyleSheet> {
    let _ = ensure_default_css_files();
    let path = config_dir().join(file_name);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("Failed to read CSS file {}", path.display()))?;
    Ok(CssStyleSheet::parse(&content))
}

/// Event emitted when `chickenwm.conf` or any `.css` file inside `~/.config/chickenwm/` is modified.
#[derive(Debug, Clone)]
pub enum ConfigWatchEvent {
    HyprConfChanged(PathBuf),
    CssChanged { file_name: String, path: PathBuf },
}

/// Phase 2.3: Spawns a background file watcher (`notify` crate) on `~/.config/chickenwm/`
/// to hot-reload `chickenwm.conf`, `launcher.css`, `files.css`, and `settings.css` in `<5ms`.
pub fn spawn_config_watcher<F>(on_change: F) -> Result<RecommendedWatcher>
where
    F: Fn(ConfigWatchEvent) + Send + 'static,
{
    let dir = config_dir();
    std::fs::create_dir_all(&dir)?;

    let (tx, rx) = channel::<notify::Result<Event>>();
    let mut watcher = notify::recommended_watcher(tx)
        .context("Failed to initialize notify file system watcher")?;
    watcher
        .watch(&dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("Failed to watch directory {}", dir.display()))?;

    thread::Builder::new()
        .name("chickenwm-config-watcher".to_string())
        .spawn(move || {
            while let Ok(res) = rx.recv() {
                match res {
                    Ok(event) => {
                        if event.kind.is_modify() || event.kind.is_create() {
                            for path in event.paths {
                                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                                    if name.eq_ignore_ascii_case("chickenwm.conf") {
                                        info!("Hot-reloading chickenwm.conf (<5ms notify trigger)");
                                        on_change(ConfigWatchEvent::HyprConfChanged(path));
                                    } else if name.ends_with(".css") {
                                        info!("Hot-reloading CSS stylesheet {}", name);
                                        on_change(ConfigWatchEvent::CssChanged {
                                            file_name: name.to_string(),
                                            path,
                                        });
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => warn!("Config watcher error: {e}"),
                }
            }
        })?;

    Ok(watcher)
}

/// Helper to check whether a given path is inside `~/.config/chickenwm/`.
pub fn is_in_config_dir(path: &Path) -> bool {
    path.starts_with(config_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_css_parser_and_var_resolution() {
        let sheet = CssStyleSheet::parse(DEFAULT_LAUNCHER_CSS);
        assert_eq!(sheet.var_px("--border-radius", 0.0), 12.0);
        assert_eq!(sheet.var_px("--row-height", 0.0), 44.0);
        let win_style = sheet.rules.get(".launcher-window").expect("launcher-window rule");
        assert_eq!(win_style.border_radius, Some(12.0));
        assert_eq!(win_style.backdrop_blur, Some(16.0));
        assert_eq!(win_style.transition_ms, Some(120.0));
    }
}
