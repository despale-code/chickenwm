//! Popular color schemes as system themes (`theme { preset = dracula }`).
//!
//! Each preset has a light and a dark variant, so per-app light/dark (`app_modes`) keeps
//! working. The scheme's own colors fill the palette; borders, hovers, selection and the like
//! are derived from them the same way the generated ChickenWM theme does.

use crate::config::RgbaColor;
use crate::theme::{Palette, ThemeMode};

/// The colors a preset defines, as `0xRRGGBB`.
#[derive(Debug, Clone, Copy)]
struct Scheme {
    base: u32,
    surface: u32,
    mantle: u32,
    fg: u32,
    subtext: u32,
    accent: u32,
    red: u32,
    green: u32,
    yellow: u32,
    orange: u32,
    purple: u32,
    blue: u32,
    cyan: u32,
}

pub struct Preset {
    /// Id used in `chickenwm.conf`.
    pub id: &'static str,
    pub name: &'static str,
    /// Names of the variants, for the settings UI.
    pub light_name: &'static str,
    pub dark_name: &'static str,
    light: Scheme,
    dark: Scheme,
}

macro_rules! scheme {
    ($base:expr, $surface:expr, $mantle:expr, $fg:expr, $subtext:expr, $accent:expr,
     $red:expr, $green:expr, $yellow:expr, $orange:expr, $purple:expr, $blue:expr, $cyan:expr) => {
        Scheme {
            base: $base, surface: $surface, mantle: $mantle, fg: $fg, subtext: $subtext, accent: $accent,
            red: $red, green: $green, yellow: $yellow, orange: $orange, purple: $purple, blue: $blue, cyan: $cyan,
        }
    };
}

/// The built-in presets, in the order the settings app shows them.
pub const PRESETS: [Preset; 10] = [
    Preset {
        id: "dracula",
        name: "Dracula",
        light_name: "Alucard",
        dark_name: "Dracula",
        light: scheme!(0xfffbeb, 0xfffdf5, 0xf2ecd4, 0x1f1f1f, 0x6c664b, 0x644ac9,
            0xcb3a2a, 0x14710a, 0x846e15, 0xa34d14, 0x644ac9, 0x036a96, 0x036a96),
        dark: scheme!(0x282a36, 0x313342, 0x21222c, 0xf8f8f2, 0xa9adc8, 0xbd93f9,
            0xff5555, 0x50fa7b, 0xf1fa8c, 0xffb86c, 0xff79c6, 0xbd93f9, 0x8be9fd),
    },
    Preset {
        id: "catppuccin",
        name: "Catppuccin",
        light_name: "Latte",
        dark_name: "Mocha",
        light: scheme!(0xeff1f5, 0xfafbfd, 0xe6e9ef, 0x4c4f69, 0x6c6f85, 0x8839ef,
            0xd20f39, 0x40a02b, 0xdf8e1d, 0xfe640b, 0xea76cb, 0x1e66f5, 0x179299),
        dark: scheme!(0x1e1e2e, 0x262637, 0x181825, 0xcdd6f4, 0xa6adc8, 0xcba6f7,
            0xf38ba8, 0xa6e3a1, 0xf9e2af, 0xfab387, 0xf5c2e7, 0x89b4fa, 0x94e2d5),
    },
    Preset {
        id: "nord",
        name: "Nord",
        light_name: "Snow Storm",
        dark_name: "Polar Night",
        light: scheme!(0xeceff4, 0xf8f9fb, 0xe5e9f0, 0x2e3440, 0x4c566a, 0x5e81ac,
            0xbf616a, 0x6f8f57, 0xb08a3e, 0xc0704f, 0x9a6f94, 0x5e81ac, 0x4c8c8a),
        dark: scheme!(0x2e3440, 0x3b4252, 0x272c36, 0xeceff4, 0xa9b1c1, 0x88c0d0,
            0xbf616a, 0xa3be8c, 0xebcb8b, 0xd08770, 0xb48ead, 0x81a1c1, 0x8fbcbb),
    },
    Preset {
        id: "gruvbox",
        name: "Gruvbox",
        light_name: "Gruvbox Light",
        dark_name: "Gruvbox Dark",
        light: scheme!(0xfbf1c7, 0xf9f5d7, 0xf2e5bc, 0x3c3836, 0x7c6f64, 0xaf3a03,
            0x9d0006, 0x79740e, 0xb57614, 0xaf3a03, 0x8f3f71, 0x076678, 0x427b58),
        dark: scheme!(0x282828, 0x32302f, 0x1d2021, 0xebdbb2, 0xa89984, 0xfabd2f,
            0xfb4934, 0xb8bb26, 0xfabd2f, 0xfe8019, 0xd3869b, 0x83a598, 0x8ec07c),
    },
    Preset {
        id: "tokyonight",
        name: "Tokyo Night",
        light_name: "Day",
        dark_name: "Night",
        light: scheme!(0xe1e2e7, 0xeceef3, 0xd5d6db, 0x3760bf, 0x6172b0, 0x2e7de9,
            0xf52a65, 0x587539, 0x8c6c3e, 0xb15c00, 0x9854f1, 0x2e7de9, 0x007197),
        dark: scheme!(0x1a1b26, 0x24283b, 0x16161e, 0xc0caf5, 0xa9b1d6, 0x7aa2f7,
            0xf7768e, 0x9ece6a, 0xe0af68, 0xff9e64, 0xbb9af7, 0x7aa2f7, 0x7dcfff),
    },
    Preset {
        id: "rosepine",
        name: "Rosé Pine",
        light_name: "Dawn",
        dark_name: "Rosé Pine",
        light: scheme!(0xfaf4ed, 0xfffaf3, 0xf2e9e1, 0x575279, 0x797593, 0x907aa9,
            0xb4637a, 0x286983, 0xea9d34, 0xd7827e, 0x907aa9, 0x286983, 0x56949f),
        dark: scheme!(0x191724, 0x1f1d2e, 0x16141f, 0xe0def4, 0x908caa, 0xc4a7e7,
            0xeb6f92, 0x9ccfd8, 0xf6c177, 0xebbcba, 0xc4a7e7, 0x31748f, 0x9ccfd8),
    },
    Preset {
        id: "solarized",
        name: "Solarized",
        light_name: "Solarized Light",
        dark_name: "Solarized Dark",
        light: scheme!(0xfdf6e3, 0xfffbf0, 0xeee8d5, 0x586e75, 0x657b83, 0x268bd2,
            0xdc322f, 0x859900, 0xb58900, 0xcb4b16, 0xd33682, 0x268bd2, 0x2aa198),
        dark: scheme!(0x002b36, 0x073642, 0x00212b, 0x93a1a1, 0x839496, 0x268bd2,
            0xdc322f, 0x859900, 0xb58900, 0xcb4b16, 0xd33682, 0x268bd2, 0x2aa198),
    },
    Preset {
        id: "one",
        name: "One",
        light_name: "One Light",
        dark_name: "One Dark",
        light: scheme!(0xfafafa, 0xffffff, 0xf0f0f0, 0x383a42, 0x696c77, 0x4078f2,
            0xe45649, 0x50a14f, 0xc18401, 0x986801, 0xa626a4, 0x4078f2, 0x0184bc),
        dark: scheme!(0x282c34, 0x2f343e, 0x21252b, 0xabb2bf, 0x7f848e, 0x61afef,
            0xe06c75, 0x98c379, 0xe5c07b, 0xd19a66, 0xc678dd, 0x61afef, 0x56b6c2),
    },
    Preset {
        id: "everforest",
        name: "Everforest",
        light_name: "Everforest Light",
        dark_name: "Everforest Dark",
        light: scheme!(0xfdf6e3, 0xfffbef, 0xf4f0d9, 0x5c6a72, 0x829181, 0x8da101,
            0xf85552, 0x8da101, 0xdfa000, 0xf57d26, 0xdf69ba, 0x3a94c5, 0x35a77c),
        dark: scheme!(0x2d353b, 0x343f44, 0x232a2e, 0xd3c6aa, 0x9da9a0, 0xa7c080,
            0xe67e80, 0xa7c080, 0xdbbc7f, 0xe69875, 0xd699b6, 0x7fbbb3, 0x83c092),
    },
    Preset {
        id: "kanagawa",
        name: "Kanagawa",
        light_name: "Lotus",
        dark_name: "Wave",
        light: scheme!(0xf2ecbc, 0xf6f1cf, 0xe7dba0, 0x545464, 0x7a7974, 0x4d699b,
            0xc84053, 0x6f894e, 0x77713f, 0xcc6d00, 0x624c83, 0x4d699b, 0x597b75),
        dark: scheme!(0x1f1f28, 0x2a2a37, 0x16161d, 0xdcd7ba, 0x9e9b93, 0x7e9cd8,
            0xe46876, 0x98bb6c, 0xe6c384, 0xffa066, 0x957fb8, 0x7e9cd8, 0x7fb4ca),
    },
];

/// Looks a preset up by id (case-insensitive, spaces/dashes ignored: "Tokyo Night" works).
pub fn find(id: &str) -> Option<&'static Preset> {
    let norm: String = id.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_ascii_lowercase();
    PRESETS.iter().find(|p| p.id == norm || p.name.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_ascii_lowercase() == norm)
}

fn rgb(hex: u32) -> RgbaColor {
    RgbaColor { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8, a: 255 }
}

fn alpha(c: RgbaColor, a: f32) -> RgbaColor {
    RgbaColor { a: (a.clamp(0.0, 1.0) * 255.0).round() as u8, ..c }
}

fn mix(a: RgbaColor, b: RgbaColor, t: f32) -> RgbaColor {
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    RgbaColor { r: m(a.r, b.r), g: m(a.g, b.g), b: m(a.b, b.b), a: 255 }
}

impl Preset {
    /// The palette for one mode.
    pub fn palette(&self, mode: ThemeMode) -> Palette {
        let s = match mode {
            ThemeMode::Light => self.light,
            ThemeMode::Dark => self.dark,
        };
        let (base, fg, accent) = (rgb(s.base), rgb(s.fg), rgb(s.accent));
        let dark = mode == ThemeMode::Dark;
        // Text on an accent fill: whichever of the background / text color reads better.
        let accent_fg = if crate::theme::contrast(base, accent) >= crate::theme::contrast(fg, accent) { base } else { fg };
        Palette {
            mode,
            base,
            surface: rgb(s.surface),
            mantle: rgb(s.mantle),
            fg,
            subtext: rgb(s.subtext),
            faint: alpha(fg, 0.45),
            border: alpha(fg, if dark { 0.11 } else { 0.16 }),
            shadow: alpha(mix(base, RgbaColor { r: 0, g: 0, b: 0, a: 255 }, 0.85), if dark { 0.45 } else { 0.18 }),
            accent,
            accent_fg,
            accent_soft: if dark { alpha(accent, 0.22) } else { mix(accent, base, 0.72) },
            hover: alpha(fg, if dark { 0.06 } else { 0.07 }),
            press: alpha(fg, if dark { 0.12 } else { 0.14 }),
            selected: alpha(accent, if dark { 0.2 } else { 0.15 }),
            danger: rgb(s.red),
            success: rgb(s.green),
            dir: rgb(s.blue),
            exec: rgb(s.green),
            archive: rgb(s.orange),
            media: rgb(s.purple),
            code: rgb(s.cyan),
            document: rgb(s.subtext),
        }
    }

    /// A few representative colors for a swatch: base, surface, accent and four highlights.
    pub fn swatch(&self, mode: ThemeMode) -> [RgbaColor; 7] {
        let s = match mode {
            ThemeMode::Light => self.light,
            ThemeMode::Dark => self.dark,
        };
        [s.base, s.surface, s.accent, s.red, s.green, s.yellow, s.blue].map(rgb)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::contrast;

    #[test]
    fn presets_are_readable() {
        for p in &PRESETS {
            for mode in [ThemeMode::Light, ThemeMode::Dark] {
                let pal = p.palette(mode);
                assert!(contrast(pal.fg, pal.base) >= 4.5, "{} {mode:?}: text contrast {}", p.id, contrast(pal.fg, pal.base));
                assert!(contrast(pal.accent_fg, pal.accent) >= 2.0, "{} {mode:?}: accent text", p.id);
            }
        }
        assert_eq!(find("Tokyo Night").map(|p| p.id), Some("tokyonight"));
        assert_eq!(find("rose-pine").map(|p| p.id), Some("rosepine"));
        assert!(find("nope").is_none());
    }
}
