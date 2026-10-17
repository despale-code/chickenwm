//! System-wide theme: one accent color generates the palette for every ChickenWM module.
//!
//! The accent (default `#242321`, a warm charcoal) is the seed. Its hue and saturation are kept
//! and only lightness moves, so every surface is a "lighter counterpart" of the same color:
//!
//! - **Light** mode: off-white surfaces tinted with the accent hue, accent-colored text and
//!   highlights, and a light counterpart of the accent for text on accent fills.
//! - **Dark** mode: the accent itself is the window background; highlights use its light
//!   counterpart.
//!
//! [`apply_theme`] writes the palette into the `:root` variables of `files.css`,
//! `launcher.css`, `settings.css`, `roobar.css` and `browser.css`. Only color variables are
//! touched (fonts, radii, motion and custom rules stay as the user left them), and every module
//! hot-reloads its CSS, so the change is live. The theme itself is stored in `chickenwm.conf`:
//!
//! ```text
//! theme {
//!     accent = rgb(242321)
//!     mode = light
//!     icon_theme = chickenwm
//! }
//! ```

use crate::config::{config_dir, RgbaColor};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const DEFAULT_ACCENT: RgbaColor = RgbaColor { r: 0x24, g: 0x23, b: 0x21, a: 255 };

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ThemeMode {
    Light,
    Dark,
}

impl ThemeMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// The `theme { ... }` block of `chickenwm.conf`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeConfig {
    pub accent: RgbaColor,
    pub mode: ThemeMode,
    /// `system` (Windows shell icons), `chickenwm` (built-in, tinted from the palette), or the
    /// name of a folder under `~/.config/chickenwm/icons/`.
    pub icon_theme: String,
    /// Per-app light/dark choice (`app_modes = browser:dark, files:light`); apps not listed
    /// follow `mode`.
    #[serde(default)]
    pub app_modes: Vec<(String, AppMode)>,
    /// A built-in color scheme (`dracula`, `catpuccin`, … see `theme_presets`); empty means
    /// the palette is generated from `accent`.
    #[serde(default)]
    pub preset: String,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            accent: DEFAULT_ACCENT,
            mode: ThemeMode::Light,
            icon_theme: "chickenwm".to_string(),
            app_modes: Vec::new(),
            preset: String::new(),
        }
    }
}

/// An app's light/dark choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppMode {
    /// Use the system `mode`.
    System,
    Light,
    Dark,
}

impl AppMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "system" | "auto" | "follow" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// Every themed app: (id used in `app_modes`, its stylesheet, display name).
pub const APPS: [(&str, &str, &str); 6] = [
    ("roobar", "roobar.css", "Status bar"),
    ("launcher", "launcher.css", "Launcher"),
    ("files", "files.css", "File explorer"),
    ("settings", "settings.css", "Settings"),
    ("browser", "browser.css", "Browser"),
    ("notifications", "notifications.css", "Notifications"),
];

impl ThemeConfig {
    /// The app's own choice (`System` if it has none).
    pub fn app_mode(&self, app: &str) -> AppMode {
        self.app_modes
            .iter()
            .find(|(a, _)| a.eq_ignore_ascii_case(app))
            .map_or(AppMode::System, |(_, m)| *m)
    }

    /// The light/dark mode an app actually uses.
    pub fn mode_for(&self, app: &str) -> ThemeMode {
        match self.app_mode(app) {
            AppMode::System => self.mode,
            AppMode::Light => ThemeMode::Light,
            AppMode::Dark => ThemeMode::Dark,
        }
    }

    /// The master color provider: the palette an app should be drawn with.
    pub fn palette_for(&self, app: &str) -> Palette {
        self.palette(self.mode_for(app))
    }

    /// The palette for `mode`: the preset's, or the one generated from the accent.
    pub fn palette(&self, mode: ThemeMode) -> Palette {
        match crate::theme_presets::find(&self.preset) {
            Some(p) if !self.preset.is_empty() => p.palette(mode),
            _ => Palette::new(self.accent, mode),
        }
    }

    pub fn set_app_mode(&mut self, app: &str, mode: AppMode) {
        self.app_modes.retain(|(a, _)| !a.eq_ignore_ascii_case(app));
        if mode != AppMode::System {
            self.app_modes.push((app.to_ascii_lowercase(), mode));
        }
    }

    /// `browser:dark, files:light` for chickenwm.conf.
    pub fn app_modes_string(&self) -> String {
        self.app_modes
            .iter()
            .map(|(a, m)| format!("{a}:{}", m.as_str()))
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn parse_app_modes(value: &str) -> Vec<(String, AppMode)> {
        value
            .split(',')
            .filter_map(|pair| {
                let (app, mode) = pair.split_once(':')?;
                Some((app.trim().to_ascii_lowercase(), AppMode::parse(mode)?))
            })
            .filter(|(app, m)| !app.is_empty() && *m != AppMode::System)
            .collect()
    }
}

/// Stylesheet of an app id, or the app id of a stylesheet.
pub fn app_file(app: &str) -> Option<&'static str> {
    APPS.iter().find(|(id, _, _)| id.eq_ignore_ascii_case(app)).map(|(_, f, _)| *f)
}

fn app_of_file(file: &str) -> Option<&'static str> {
    APPS.iter().find(|(_, f, _)| f.eq_ignore_ascii_case(file)).map(|(id, _, _)| *id)
}

// ───────────────────────────── color math ─────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsl {
    pub h: f32, // degrees
    pub s: f32, // 0..1
    pub l: f32, // 0..1
}

pub fn to_hsl(c: RgbaColor) -> Hsl {
    let (r, g, b) = (c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d < 1e-6 {
        return Hsl { h: 0.0, s: 0.0, l };
    }
    let s = d / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * (((g - b) / d).rem_euclid(6.0))
    } else if max == g {
        60.0 * ((b - r) / d + 2.0)
    } else {
        60.0 * ((r - g) / d + 4.0)
    };
    Hsl { h, s: s.clamp(0.0, 1.0), l }
}

pub fn from_hsl(hsl: Hsl, a: u8) -> RgbaColor {
    let c = (1.0 - (2.0 * hsl.l - 1.0).abs()) * hsl.s;
    let hp = hsl.h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r, g, b) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = hsl.l - c / 2.0;
    let to = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    RgbaColor { r: to(r), g: to(g), b: to(b), a }
}

/// Relative luminance (WCAG).
pub fn luminance(c: RgbaColor) -> f32 {
    let lin = |v: u8| {
        let v = v as f32 / 255.0;
        if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

pub fn contrast(a: RgbaColor, b: RgbaColor) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

pub fn with_alpha(c: RgbaColor, alpha: f32) -> RgbaColor {
    RgbaColor { a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8, ..c }
}

/// `#rrggbb` when opaque, else `rgba(r, g, b, 0.xx)` — both understood by the CSS engine and
/// by the browser.
pub fn css(c: RgbaColor) -> String {
    if c.a == 255 {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        format!("rgba({}, {}, {}, {:.2})", c.r, c.g, c.b, c.a as f32 / 255.0)
    }
}

// ───────────────────────────── palette ─────────────────────────────

/// Every color the modules use, derived from the accent.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub mode: ThemeMode,
    /// Window / page background.
    pub base: RgbaColor,
    /// Cards, panes, fields: one step "up" from `base`.
    pub surface: RgbaColor,
    /// Sidebars, tab strips, status bars: one step "down" from `base`.
    pub mantle: RgbaColor,
    pub fg: RgbaColor,
    pub subtext: RgbaColor,
    pub faint: RgbaColor,
    pub border: RgbaColor,
    pub shadow: RgbaColor,
    /// Highlight color (selection fill, active tab/workspace, focus rings, window border).
    pub accent: RgbaColor,
    /// Text drawn on an `accent` fill.
    pub accent_fg: RgbaColor,
    /// Softer, lighter counterpart of the accent (secondary highlights).
    pub accent_soft: RgbaColor,
    pub hover: RgbaColor,
    pub press: RgbaColor,
    pub selected: RgbaColor,
    pub danger: RgbaColor,
    pub success: RgbaColor,
    /// File-type tints (folders, executables, archives, media) — muted, accent-friendly.
    pub dir: RgbaColor,
    pub exec: RgbaColor,
    pub archive: RgbaColor,
    pub media: RgbaColor,
    pub code: RgbaColor,
    pub document: RgbaColor,
}

impl Palette {
    pub fn new(seed: RgbaColor, mode: ThemeMode) -> Self {
        let hsl = to_hsl(RgbaColor { a: 255, ..seed });
        let h = hsl.h;
        // Near-neutral seeds (like #242321) still get a hint of their hue in tints.
        let tint_s = (hsl.s * 0.9).clamp(0.06, 0.35);
        let tone = |l: f32| from_hsl(Hsl { h, s: tint_s, l }, 255);
        let seed = RgbaColor { a: 255, ..seed };
        // A colorful seed works as-is; a very dark/neutral one becomes the "ink" of light mode
        // and its lighter counterpart is the dark-mode accent.
        let light_counterpart = from_hsl(Hsl { h, s: tint_s.max(hsl.s * 0.6), l: 0.84 }, 255);
        let hue_tint = |hue: f32, s: f32, l: f32| from_hsl(Hsl { h: hue, s, l }, 255);

        match mode {
            ThemeMode::Light => {
                // Keep the accent readable on light surfaces: darken it if it's too light.
                let accent = if contrast(seed, tone(0.95)) >= 3.0 {
                    seed
                } else {
                    from_hsl(Hsl { l: hsl.l.min(0.35), ..hsl }, 255)
                };
                let ink = if hsl.l <= 0.25 { seed } else { tone(0.15) };
                Self {
                    mode,
                    base: tone(0.945),
                    surface: tone(0.985),
                    mantle: tone(0.905),
                    fg: ink,
                    subtext: tone(0.38),
                    faint: with_alpha(ink, 0.45),
                    border: with_alpha(ink, 0.16),
                    shadow: with_alpha(tone(0.08), 0.18),
                    accent,
                    accent_fg: tone(0.965),
                    accent_soft: tone(0.82),
                    hover: with_alpha(ink, 0.07),
                    press: with_alpha(ink, 0.14),
                    selected: with_alpha(accent, 0.13),
                    danger: hue_tint(4.0, 0.62, 0.48),
                    success: hue_tint(130.0, 0.32, 0.36),
                    dir: hue_tint(h + 4.0, 0.30, 0.36),
                    exec: hue_tint(115.0, 0.24, 0.32),
                    archive: hue_tint(28.0, 0.42, 0.40),
                    media: hue_tint(300.0, 0.18, 0.40),
                    code: hue_tint(205.0, 0.26, 0.36),
                    document: hue_tint(h, tint_s, 0.30),
                }
            }
            ThemeMode::Dark => {
                // The seed is the background; if it's light, use its dark counterpart instead.
                let base = if hsl.l <= 0.22 { seed } else { tone(0.14) };
                let base_l = to_hsl(base).l;
                let accent = if hsl.l <= 0.22 || contrast(seed, base) < 3.0 { light_counterpart } else { seed };
                let fg = tone(0.92);
                Self {
                    mode,
                    base,
                    surface: tone((base_l + 0.045).min(0.30)),
                    mantle: tone((base_l - 0.03).max(0.04)),
                    fg,
                    subtext: tone(0.66),
                    faint: with_alpha(fg, 0.45),
                    border: with_alpha(fg, 0.11),
                    shadow: with_alpha(tone(0.03), 0.45),
                    accent,
                    accent_fg: base,
                    accent_soft: with_alpha(accent, 0.22),
                    hover: with_alpha(fg, 0.06),
                    press: with_alpha(fg, 0.12),
                    selected: with_alpha(accent, 0.16),
                    danger: hue_tint(355.0, 0.70, 0.70),
                    success: hue_tint(120.0, 0.30, 0.68),
                    dir: hue_tint(h + 6.0, 0.42, 0.74),
                    exec: hue_tint(100.0, 0.28, 0.70),
                    archive: hue_tint(36.0, 0.55, 0.72),
                    media: hue_tint(300.0, 0.25, 0.76),
                    code: hue_tint(200.0, 0.32, 0.72),
                    document: tone(0.80),
                }
            }
        }
    }

    /// The window border color for `col.active_border` (the accent; on light mode the inked
    /// accent reads as a crisp outline, on dark mode its light counterpart does).
    pub fn active_border(&self) -> RgbaColor {
        self.accent
    }

    /// `:root` variables for one module's CSS file.
    pub fn css_vars(&self, file: &str) -> Vec<(&'static str, String)> {
        let p = self;
        let c = |x: RgbaColor| css(x);
        match file {
            "files.css" => vec![
                ("--bg-color", c(p.base)),
                ("--pane-bg", c(p.surface)),
                ("--sidebar-bg", c(p.mantle)),
                ("--fg-color", c(p.fg)),
                ("--subtext-color", c(p.subtext)),
                ("--dir-color", c(p.dir)),
                ("--exec-color", c(p.exec)),
                ("--archive-color", c(p.archive)),
                ("--media-color", c(p.media)),
                ("--accent-color", c(p.accent)),
                ("--accent-fg", c(p.accent_fg)),
                ("--selected-bg", c(p.selected)),
                ("--border-color", c(p.border)),
                ("--danger-color", c(p.danger)),
            ],
            "launcher.css" => vec![
                ("--bg-color", c(p.base)),
                ("--surface-color", c(p.surface)),
                ("--fg-color", c(p.fg)),
                ("--subtext-color", c(p.subtext)),
                ("--accent-color", c(p.accent)),
                ("--accent-secondary", c(p.accent_soft)),
                ("--selected-bg", c(p.selected)),
                ("--border-color", c(p.border)),
            ],
            "settings.css" => vec![
                ("--bg-color", c(p.base)),
                ("--card-bg", c(p.surface)),
                ("--sidebar-bg", c(p.mantle)),
                ("--fg-color", c(p.fg)),
                ("--subtext-color", c(p.subtext)),
                ("--accent-color", c(p.accent)),
                ("--accent-fg", c(p.accent_fg)),
                ("--accent-secondary", c(p.accent_soft)),
                ("--selected-bg", c(p.selected)),
                ("--danger-color", c(p.danger)),
                ("--border-color", c(p.border)),
            ],
            "roobar.css" => vec![
                ("--pill-bg", c(with_alpha(p.surface, 0.95))),
                ("--fg-color", c(p.fg)),
                ("--subtext-color", c(p.subtext)),
                // roobar's accent is the bright text/glow used on the active highlight.
                ("--accent-color", c(p.accent_fg)),
                ("--accent-secondary", c(p.subtext)),
                ("--border-color", c(p.border)),
                ("--shadow-color", c(p.shadow)),
                ("--ws-active-bg", c(p.accent)),
                ("--ws-active-fg", c(p.accent_fg)),
                ("--ws-occupied-bg", c(with_alpha(p.accent, if p.mode == ThemeMode::Light { 0.12 } else { 0.18 }))),
                ("--ws-occupied-fg", c(p.fg)),
                ("--ws-empty-fg", c(p.faint)),
            ],
            "notifications.css" => vec![
                ("--card-bg", c(with_alpha(p.surface, 0.97))),
                ("--fg-color", c(p.fg)),
                ("--subtext-color", c(p.subtext)),
                ("--accent-color", c(p.accent)),
                ("--accent-fg", c(p.accent_fg)),
                ("--border-color", c(p.border)),
                ("--shadow-color", c(p.shadow)),
                ("--critical-color", c(p.danger)),
            ],
            "browser.css" => vec![
                ("--bg-color", c(p.base)),
                ("--tab-strip-bg", c(p.mantle)),
                ("--sidebar-bg", c(p.mantle)),
                ("--surface-color", c(p.surface)),
                ("--card-bg", c(p.surface)),
                ("--field-bg", c(with_alpha(p.surface, 0.85))),
                ("--field-hover-bg", c(p.surface)),
                ("--field-focus-bg", c(p.surface)),
                ("--fg-color", c(p.fg)),
                ("--subtext-color", c(p.subtext)),
                ("--faint-color", c(p.faint)),
                ("--accent-color", c(p.accent)),
                ("--accent-fg", c(p.accent_fg)),
                ("--accent-secondary", c(p.subtext)),
                ("--accent-ring", c(p.accent)),
                ("--hover-bg", c(p.hover)),
                ("--press-bg", c(p.press)),
                ("--active-bg", c(p.press)),
                ("--border-color", c(p.border)),
                ("--shadow-color", c(p.shadow)),
            ],
            _ => Vec::new(),
        }
    }
}

pub const THEMED_CSS_FILES: [&str; 6] =
    ["files.css", "launcher.css", "settings.css", "roobar.css", "browser.css", "notifications.css"];

/// Sets `--name: value;` declarations inside the first `:root { }` block, keeping everything
/// else (other variables, comments, rules) untouched. Missing variables are appended to the
/// block; a missing block is created at the top.
pub fn set_root_vars(source: &str, vars: &[(&str, String)]) -> String {
    let Some(root_start) = source.find(":root") else {
        let mut block = String::from(":root {\n");
        for (k, v) in vars {
            block.push_str(&format!("    {k}: {v};\n"));
        }
        block.push_str("}\n\n");
        return block + source;
    };
    let Some(open_rel) = source[root_start..].find('{') else {
        return source.to_string();
    };
    let open = root_start + open_rel + 1;
    let Some(close_rel) = source[open..].find('}') else {
        return source.to_string();
    };
    let close = open + close_rel;
    let mut body = source[open..close].to_string();
    let mut missing = Vec::new();
    for (name, value) in vars {
        // Match `name` only as a whole property (not `--bg-color-2`).
        let mut found = false;
        let mut search = 0;
        while let Some(rel) = body[search..].find(name) {
            let at = search + rel;
            let after = &body[at + name.len()..];
            let before_ok = at == 0 || body[..at].ends_with(|c: char| c.is_whitespace() || c == ';' || c == '{');
            let after_trim = after.trim_start();
            if before_ok && after_trim.starts_with(':') {
                let colon = at + name.len() + (after.len() - after_trim.len());
                let value_start = colon + 1;
                let value_end = body[value_start..].find(';').map(|i| value_start + i).unwrap_or(body.len());
                body.replace_range(value_start..value_end, &format!(" {value}"));
                found = true;
                break;
            }
            search = at + name.len();
        }
        if !found {
            missing.push(format!("    {name}: {value};\n"));
        }
    }
    if !missing.is_empty() {
        if !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str("    /* ChickenWM theme */\n");
        for m in missing {
            body.push_str(&m);
        }
    }
    format!("{}{}{}", &source[..open], body, &source[close..])
}

/// Writes the palette into one CSS file (if it exists). Returns whether it changed.
pub fn apply_palette_to_file(palette: &Palette, path: &Path) -> Result<bool> {
    let file = path.file_name().and_then(|f| f.to_str()).unwrap_or_default();
    let vars = palette.css_vars(file);
    if vars.is_empty() || !path.exists() {
        return Ok(false);
    }
    let old = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let new = set_root_vars(&old, &vars);
    if new != old {
        std::fs::write(path, &new).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(new != old)
}

/// Applies a theme to every module's CSS (and regenerates the built-in icon theme). The caller
/// persists `theme { }` and `col.active_border` in `chickenwm.conf` through its `ConfigDocument`.
///
/// Each app gets the palette for *its* mode (`app_modes`); the returned palette is the system
/// one (window borders, icons, wallpaper).
pub fn apply_theme(theme: &ThemeConfig) -> Result<Palette> {
    let palette = theme.palette(theme.mode);
    let dir = config_dir();
    crate::css::ensure_default_css_files()?;
    for file in THEMED_CSS_FILES {
        let app_palette = app_of_file(file).map_or(palette, |app| theme.palette_for(app));
        apply_palette_to_file(&app_palette, &dir.join(file))?;
    }
    crate::icon_theme::generate_builtin(&palette)?;
    Ok(palette)
}

/// Sets one app's light/dark mode, saves it in chickenwm.conf and repaints that app (its
/// stylesheet hot-reloads). This is what every app's "Appearance" option calls.
pub fn set_app_mode(app: &str, mode: AppMode) -> Result<ThemeConfig> {
    let (mut doc, cfg) = crate::config::load_or_init_config()?;
    let mut theme = cfg.theme;
    theme.set_app_mode(app, mode);
    doc.set_category_key("theme", "app_modes", &theme.app_modes_string());
    crate::config::save_config_document(&doc)?;
    if let Some(file) = app_file(app) {
        apply_palette_to_file(&theme.palette_for(app), &config_dir().join(file))?;
    }
    Ok(theme)
}

/// The canonical id of a preset name (`"Tokyo Night"` → `tokyonight`).
pub fn theme_preset_id(name: &str) -> Option<String> {
    crate::theme_presets::find(name).map(|p| p.id.to_string())
}

/// Records `theme { }` (and the window border that follows it) in chickenwm.conf.
pub fn write_theme_keys(doc: &mut crate::config::ConfigDocument, theme: &ThemeConfig, palette: &Palette) {
    doc.set_category_key("theme", "accent", &hypr_rgb(theme.accent));
    doc.set_category_key("theme", "mode", theme.mode.as_str());
    doc.set_category_key("theme", "preset", if theme.preset.is_empty() { "none" } else { &theme.preset });
    doc.set_category_key("theme", "icon_theme", &theme.icon_theme);
    doc.set_category_key("theme", "app_modes", &theme.app_modes_string());
    doc.set_category_key("general", "col.active_border", &hypr_color(palette.active_border()));
}

/// Every app's current setting, for settings UIs: (id, display name, choice, effective mode).
pub fn app_mode_table(theme: &ThemeConfig) -> Vec<(&'static str, &'static str, AppMode, ThemeMode)> {
    APPS.iter()
        .map(|(id, _, name)| (*id, *name, theme.app_mode(id), theme.mode_for(id)))
        .collect()
}

/// `rgba(rrggbbaa)` for `chickenwm.conf`.
pub fn hypr_color(c: RgbaColor) -> String {
    format!("rgba({:02x}{:02x}{:02x}{:02x})", c.r, c.g, c.b, c.a)
}

/// `rgb(rrggbb)` for `chickenwm.conf` (a bare `#rrggbb` would start a comment there).
pub fn hypr_rgb(c: RgbaColor) -> String {
    format!("rgb({:02x}{:02x}{:02x})", c.r, c.g, c.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsl_roundtrip() {
        for c in [DEFAULT_ACCENT, RgbaColor { r: 51, g: 204, b: 255, a: 255 }, RgbaColor { r: 250, g: 10, b: 120, a: 255 }] {
            let back = from_hsl(to_hsl(c), 255);
            assert!((back.r as i32 - c.r as i32).abs() <= 1 && (back.g as i32 - c.g as i32).abs() <= 1 && (back.b as i32 - c.b as i32).abs() <= 1, "{c:?} -> {back:?}");
        }
    }

    #[test]
    fn default_palettes_are_readable_and_warm() {
        let light = Palette::new(DEFAULT_ACCENT, ThemeMode::Light);
        assert_eq!(light.accent, DEFAULT_ACCENT);
        assert_eq!(light.fg, DEFAULT_ACCENT);
        assert!(contrast(light.fg, light.base) > 12.0);
        assert!(contrast(light.accent_fg, light.accent) > 12.0);
        // Surfaces keep the accent's warm hue (red >= blue).
        assert!(light.base.r >= light.base.b && light.base.r > 225);

        let dark = Palette::new(DEFAULT_ACCENT, ThemeMode::Dark);
        assert_eq!(dark.base, DEFAULT_ACCENT);
        assert!(contrast(dark.fg, dark.base) > 10.0);
        assert!(contrast(dark.accent, dark.base) > 8.0, "lighter counterpart must stand out");
        assert!(to_hsl(dark.surface).l > to_hsl(dark.base).l && to_hsl(dark.mantle).l < to_hsl(dark.base).l);

        // A bright seed still yields a readable light theme.
        let cyan = Palette::new(RgbaColor { r: 51, g: 204, b: 255, a: 255 }, ThemeMode::Light);
        assert!(contrast(cyan.accent, cyan.base) >= 3.0);
    }

    #[test]
    fn per_app_modes() {
        let mut t = ThemeConfig::default();
        assert_eq!(t.mode_for("browser"), ThemeMode::Light);
        t.set_app_mode("browser", AppMode::Dark);
        t.set_app_mode("files", AppMode::Light);
        t.set_app_mode("files", AppMode::System); // back to following
        assert_eq!(t.mode_for("browser"), ThemeMode::Dark);
        assert_eq!(t.mode_for("files"), ThemeMode::Light);
        assert_eq!(t.app_modes_string(), "browser:dark");
        assert_eq!(ThemeConfig::parse_app_modes("Browser:dark, files : light, bogus, x:weird"),
            vec![("browser".to_string(), AppMode::Dark), ("files".to_string(), AppMode::Light)]);
        t.mode = ThemeMode::Dark;
        assert_eq!(t.palette_for("launcher").mode, ThemeMode::Dark);
    }

    #[test]
    fn root_vars_are_replaced_in_place() {
        let src = ":root {\n    --bg-color: #000; /* keep */\n    --bg-color-2: #111;\n    --font-size: 14px;\n}\n.a { color: var(--bg-color); }\n";
        let out = set_root_vars(src, &[("--bg-color", "#fff".into()), ("--new-var", "red".into())]);
        assert!(out.contains("--bg-color: #fff; /* keep */"));
        assert!(out.contains("--bg-color-2: #111;"));
        assert!(out.contains("--font-size: 14px;"));
        assert!(out.contains("--new-var: red;"));
        assert!(out.ends_with(".a { color: var(--bg-color); }\n"));
        // Idempotent.
        assert_eq!(set_root_vars(&out, &[("--bg-color", "#fff".into()), ("--new-var", "red".into())]), out);
        // No :root block.
        assert!(set_root_vars(".x{}", &[("--a", "1".into())]).starts_with(":root {\n    --a: 1;"));
    }

    #[test]
    fn css_format_is_parsed_back() {
        let sheet = crate::css::CssStyleSheet::parse(&format!(
            ":root {{ --a: {}; --b: {}; }}",
            css(DEFAULT_ACCENT),
            css(with_alpha(DEFAULT_ACCENT, 0.5))
        ));
        let fallback = RgbaColor { r: 1, g: 2, b: 3, a: 4 };
        assert_eq!(sheet.var_color("--a", fallback), DEFAULT_ACCENT);
        let b = sheet.var_color("--b", fallback);
        assert_eq!((b.r, b.g, b.b), (0x24, 0x23, 0x21));
        assert!((b.a as i32 - 128).abs() <= 1);
    }
}
