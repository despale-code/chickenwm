//! Icon themes.
//!
//! An icon theme is a folder of PNGs under `~/.config/chickenwm/icons/<theme>/`. Modules ask for
//! an icon by [`IconRequest`]; the theme answers with a PNG path, or `None` to fall back to the
//! Windows shell icon. Lookups go from most to least specific:
//!
//! | request                    | files tried                                        |
//! |----------------------------|----------------------------------------------------|
//! | folder                     | `folder.png`                                       |
//! | special folder (Downloads) | `folder-downloads.png`, `folder.png`               |
//! | drive                      | `drive.png`                                        |
//! | file `report.pdf`          | `ext-pdf.png`, `document.png` (category), `file.png` |
//! | app `code.exe` (launcher)  | `app-code.png` (apps keep their own icon otherwise) |
//!
//! Themes:
//! - `system`: no theme; Windows shell icons everywhere.
//! - `chickenwm`: built in. [`generate_builtin`] draws the whole set from the current palette, so
//!   icons follow the accent color and light/dark mode.
//! - anything else: a user-provided folder (any subset of the names above; missing ones fall
//!   back).

use crate::config::{config_dir, RgbaColor};
use crate::theme::{Palette, ThemeMode};
use anyhow::Result;
use std::path::{Path, PathBuf};

pub const SPECIAL_FOLDERS: [&str; 7] = ["home", "desktop", "downloads", "documents", "pictures", "music", "videos"];

/// File categories with their own built-in icon.
pub const CATEGORIES: [&str; 12] = [
    "image", "video", "audio", "archive", "code", "text", "document", "pdf", "executable", "font",
    "disk", "file",
];

pub fn icons_root() -> PathBuf {
    config_dir().join("icons")
}

/// Category for an extension (lower-case, no dot).
pub fn category_for_ext(ext: &str) -> &'static str {
    match ext {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "ico" | "tif" | "tiff" | "heic" | "avif" | "raw" | "psd" => "image",
        "mp4" | "mkv" | "webm" | "mov" | "avi" | "wmv" | "flv" | "m4v" => "video",
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "aac" | "opus" | "wma" => "audio",
        "zip" | "7z" | "rar" | "gz" | "tgz" | "tar" | "xz" | "bz2" | "zst" | "cab" => "archive",
        "rs" | "py" | "js" | "ts" | "tsx" | "jsx" | "c" | "h" | "cpp" | "hpp" | "cs" | "go" | "java" | "kt" | "lua" | "rb"
        | "php" | "sh" | "ps1" | "bat" | "cmd" | "html" | "css" | "scss" | "json" | "toml" | "yaml" | "yml" | "xml"
        | "sql" | "wgsl" | "glsl" | "hlsl" | "conf" | "ini" => "code",
        "txt" | "md" | "log" | "csv" | "rtf" | "nfo" => "text",
        "doc" | "docx" | "odt" | "xls" | "xlsx" | "ods" | "ppt" | "pptx" | "odp" | "epub" => "document",
        "pdf" => "pdf",
        "exe" | "msi" | "com" | "appx" | "msix" | "dll" | "sys" => "executable",
        "ttf" | "otf" | "woff" | "woff2" => "font",
        "iso" | "img" | "vhd" | "vhdx" => "disk",
        _ => "file",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum IconRequest {
    Folder,
    /// One of [`SPECIAL_FOLDERS`].
    SpecialFolder(String),
    Drive,
    /// Lower-case extension without the dot ("" for none).
    File(String),
    /// Executable stem, lower-case ("code" for `Code.exe`).
    App(String),
}

/// A loaded icon theme. Cheap to clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconTheme {
    pub name: String,
    dir: Option<PathBuf>,
}

impl IconTheme {
    pub fn load(name: &str) -> Self {
        let name = name.trim();
        let dir = if name.is_empty() || name.eq_ignore_ascii_case("system") {
            None
        } else {
            let d = icons_root().join(name);
            d.is_dir().then_some(d)
        };
        Self { name: name.to_string(), dir }
    }

    /// The theme selected in `chickenwm.conf`.
    pub fn current() -> Self {
        let name = crate::config::load_or_init_config()
            .map(|(_, cfg)| cfg.theme.icon_theme)
            .unwrap_or_else(|_| "chickenwm".into());
        Self::load(&name)
    }

    pub fn is_system(&self) -> bool {
        self.dir.is_none()
    }

    pub fn resolve(&self, req: &IconRequest) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        let candidates: Vec<String> = match req {
            IconRequest::Folder => vec!["folder".into()],
            IconRequest::SpecialFolder(s) => vec![format!("folder-{s}"), "folder".into()],
            IconRequest::Drive => vec!["drive".into()],
            IconRequest::File(ext) => {
                let mut v = Vec::new();
                if !ext.is_empty() {
                    v.push(format!("ext-{ext}"));
                }
                v.push(category_for_ext(ext).to_string());
                v.push("file".into());
                v
            }
            IconRequest::App(stem) => vec![format!("app-{stem}")],
        };
        candidates
            .into_iter()
            .map(|n| dir.join(format!("{n}.png")))
            .find(|p| p.is_file())
    }
}

/// Folder names under `icons/` (plus `system`), for theme pickers.
pub fn available_themes() -> Vec<String> {
    let mut v = vec!["system".to_string(), "chickenwm".to_string()];
    if let Ok(rd) = std::fs::read_dir(icons_root()) {
        for e in rd.flatten() {
            if e.path().is_dir() {
                let n = e.file_name().to_string_lossy().into_owned();
                if !v.iter().any(|x| x.eq_ignore_ascii_case(&n)) {
                    v.push(n);
                }
            }
        }
    }
    v
}

// ───────────────────────────── built-in icon set ─────────────────────────────

const SIZE: u32 = 64;
const SS: u32 = 4; // supersampling per axis

struct Canvas {
    px: Vec<[f32; 4]>, // premultiplied RGBA 0..1
}

type Shape<'a> = &'a dyn Fn(f32, f32) -> bool;

impl Canvas {
    fn new() -> Self {
        Self { px: vec![[0.0; 4]; (SIZE * SIZE) as usize] }
    }

    /// Fills `shape` (a predicate in 0..64 icon units) with anti-aliased coverage.
    fn fill(&mut self, shape: Shape, color: RgbaColor) {
        let a = color.a as f32 / 255.0;
        let rgb = [color.r as f32 / 255.0, color.g as f32 / 255.0, color.b as f32 / 255.0];
        for y in 0..SIZE {
            for x in 0..SIZE {
                let mut hits = 0;
                for sy in 0..SS {
                    for sx in 0..SS {
                        let fx = x as f32 + (sx as f32 + 0.5) / SS as f32;
                        let fy = y as f32 + (sy as f32 + 0.5) / SS as f32;
                        if shape(fx, fy) {
                            hits += 1;
                        }
                    }
                }
                if hits == 0 {
                    continue;
                }
                let cov = hits as f32 / (SS * SS) as f32 * a;
                let p = &mut self.px[(y * SIZE + x) as usize];
                for i in 0..3 {
                    p[i] = rgb[i] * cov + p[i] * (1.0 - cov);
                }
                p[3] = cov + p[3] * (1.0 - cov);
            }
        }
    }

    fn to_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for p in &self.px {
            let a = p[3];
            let un = |v: f32| if a > 0.0 { (v / a).clamp(0.0, 1.0) } else { 0.0 };
            out.extend_from_slice(&[
                (un(p[0]) * 255.0).round() as u8,
                (un(p[1]) * 255.0).round() as u8,
                (un(p[2]) * 255.0).round() as u8,
                (a * 255.0).round() as u8,
            ]);
        }
        out
    }
}

fn rrect(x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> impl Fn(f32, f32) -> bool {
    // A radius larger than half the shorter side would make the clamp ranges invalid.
    let r = r.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
    move |x, y| {
        if x < x0 || x > x1 || y < y0 || y > y1 {
            return false;
        }
        let cx = x.clamp(x0 + r, x1 - r);
        let cy = y.clamp(y0 + r, y1 - r);
        (x - cx).powi(2) + (y - cy).powi(2) <= r * r
    }
}

fn circle(cx: f32, cy: f32, r: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

fn ring(cx: f32, cy: f32, r: f32, w: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| {
        let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
        (d - r).abs() <= w / 2.0
    }
}

fn poly(points: Vec<(f32, f32)>) -> impl Fn(f32, f32) -> bool {
    move |x, y| {
        let mut inside = false;
        let n = points.len();
        for i in 0..n {
            let (xi, yi) = points[i];
            let (xj, yj) = points[(i + n - 1) % n];
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                inside = !inside;
            }
        }
        inside
    }
}

/// A stroked polyline with round caps.
fn stroke(points: Vec<(f32, f32)>, w: f32) -> impl Fn(f32, f32) -> bool {
    move |x, y| {
        points.windows(2).any(|s| {
            let ((ax, ay), (bx, by)) = (s[0], s[1]);
            let (dx, dy) = (bx - ax, by - ay);
            let len2 = dx * dx + dy * dy;
            let t = if len2 > 0.0 { (((x - ax) * dx + (y - ay) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
            let (px, py) = (ax + t * dx, ay + t * dy);
            (x - px).powi(2) + (y - py).powi(2) <= (w / 2.0).powi(2)
        })
    }
}

fn mix(a: RgbaColor, b: RgbaColor, t: f32) -> RgbaColor {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    RgbaColor { r: l(a.r, b.r), g: l(a.g, b.g), b: l(a.b, b.b), a: l(a.a, b.a) }
}

/// Draws a glyph centered at (cx, cy) with roughly `s` units of extent.
fn glyph(c: &mut Canvas, name: &str, cx: f32, cy: f32, s: f32, col: RgbaColor) {
    let w = (s * 0.16).max(2.2);
    match name {
        "home" => {
            c.fill(&poly(vec![(cx - s * 0.5, cy), (cx, cy - s * 0.45), (cx + s * 0.5, cy)]), col);
            c.fill(&rrect(cx - s * 0.34, cy - 1.0, cx + s * 0.34, cy + s * 0.42, 1.5), col);
        }
        "desktop" => {
            c.fill(&rrect(cx - s * 0.5, cy - s * 0.38, cx + s * 0.5, cy + s * 0.2, 2.0), col);
            c.fill(&rrect(cx - s * 0.08, cy + s * 0.2, cx + s * 0.08, cy + s * 0.36, 0.5), col);
            c.fill(&rrect(cx - s * 0.26, cy + s * 0.34, cx + s * 0.26, cy + s * 0.44, 1.0), col);
        }
        "downloads" => {
            c.fill(&stroke(vec![(cx, cy - s * 0.42), (cx, cy + s * 0.14)], w), col);
            c.fill(&stroke(vec![(cx - s * 0.28, cy - s * 0.06), (cx, cy + s * 0.2), (cx + s * 0.28, cy - s * 0.06)], w), col);
            c.fill(&stroke(vec![(cx - s * 0.4, cy + s * 0.42), (cx + s * 0.4, cy + s * 0.42)], w), col);
        }
        "documents" | "text" | "document" => {
            for (i, frac) in [1.0, 0.8, 1.0, 0.6].iter().enumerate() {
                let y = cy - s * 0.36 + i as f32 * s * 0.24;
                c.fill(&stroke(vec![(cx - s * 0.4, y), (cx - s * 0.4 + s * 0.8 * frac, y)], w * 0.85), col);
            }
        }
        "pictures" | "image" => {
            c.fill(&poly(vec![(cx - s * 0.5, cy + s * 0.38), (cx - s * 0.12, cy - s * 0.14), (cx + s * 0.14, cy + s * 0.14), (cx + s * 0.28, cy), (cx + s * 0.5, cy + s * 0.38)]), col);
            c.fill(&circle(cx + s * 0.24, cy - s * 0.28, s * 0.13), col);
        }
        "music" | "audio" => {
            c.fill(&stroke(vec![(cx + s * 0.18, cy + s * 0.28), (cx + s * 0.18, cy - s * 0.42), (cx + s * 0.44, cy - s * 0.3)], w), col);
            c.fill(&circle(cx - s * 0.02, cy + s * 0.3, s * 0.2), col);
        }
        "videos" | "video" => {
            c.fill(&poly(vec![(cx - s * 0.28, cy - s * 0.4), (cx + s * 0.4, cy), (cx - s * 0.28, cy + s * 0.4)]), col);
        }
        "archive" => {
            c.fill(&rrect(cx - s * 0.1, cy - s * 0.5, cx + s * 0.1, cy + s * 0.5, 1.0), col);
            for i in 0..4 {
                let y = cy - s * 0.42 + i as f32 * s * 0.22;
                let dx = if i % 2 == 0 { -s * 0.2 } else { s * 0.2 };
                c.fill(&rrect(cx + dx - s * 0.1, y, cx + dx + s * 0.1, y + s * 0.12, 0.8), col);
            }
        }
        "code" => {
            c.fill(&stroke(vec![(cx - s * 0.14, cy - s * 0.32), (cx - s * 0.46, cy), (cx - s * 0.14, cy + s * 0.32)], w), col);
            c.fill(&stroke(vec![(cx + s * 0.14, cy - s * 0.32), (cx + s * 0.46, cy), (cx + s * 0.14, cy + s * 0.32)], w), col);
        }
        "pdf" => {
            c.fill(&rrect(cx - s * 0.5, cy - s * 0.2, cx + s * 0.5, cy + s * 0.22, 2.0), col);
        }
        "executable" => {
            c.fill(&rrect(cx - s * 0.46, cy - s * 0.38, cx + s * 0.46, cy + s * 0.38, 2.5), col);
            let bg = RgbaColor { a: 255, ..mix(col, RgbaColor { r: 255, g: 255, b: 255, a: 255 }, 0.85) };
            c.fill(&rrect(cx - s * 0.38, cy - s * 0.18, cx + s * 0.38, cy + s * 0.3, 1.5), bg);
        }
        "font" => {
            c.fill(&stroke(vec![(cx - s * 0.4, cy + s * 0.42), (cx, cy - s * 0.42), (cx + s * 0.4, cy + s * 0.42)], w), col);
            c.fill(&stroke(vec![(cx - s * 0.22, cy + s * 0.12), (cx + s * 0.22, cy + s * 0.12)], w), col);
        }
        "disk" => {
            c.fill(&ring(cx, cy, s * 0.36, w), col);
            c.fill(&circle(cx, cy, s * 0.1), col);
        }
        _ => {}
    }
}

fn folder_icon(p: &Palette, special: Option<&str>) -> Canvas {
    let mut c = Canvas::new();
    let body = p.dir;
    let back = mix(p.dir, if p.mode == ThemeMode::Light { RgbaColor { r: 0, g: 0, b: 0, a: 255 } } else { p.base }, 0.25);
    let front = mix(p.dir, RgbaColor { r: 255, g: 255, b: 255, a: 255 }, if p.mode == ThemeMode::Light { 0.12 } else { 0.18 });
    // Back with tab
    c.fill(&rrect(6.0, 12.0, 28.0, 24.0, 3.5), back);
    c.fill(&rrect(6.0, 17.0, 58.0, 52.0, 4.0), back);
    // Front
    c.fill(&rrect(6.0, 22.0, 58.0, 54.0, 4.0), body);
    c.fill(&rrect(6.0, 22.0, 58.0, 26.0, 2.0), front);
    if let Some(s) = special {
        let ink = if p.mode == ThemeMode::Light { p.accent_fg } else { p.base };
        glyph(&mut c, s, 32.0, 39.0, 18.0, RgbaColor { a: 235, ..ink });
    }
    c
}

fn file_icon(p: &Palette, category: &str) -> Canvas {
    let mut c = Canvas::new();
    let paper = if p.mode == ThemeMode::Light { p.surface } else { mix(p.surface, p.fg, 0.10) };
    let edge = mix(paper, p.fg, 0.28);
    let fold = mix(paper, p.fg, 0.16);
    let page = |x: f32, y: f32| {
        // page with the top-right corner cut off
        x >= 12.0 && x <= 52.0 && y >= 5.0 && y <= 59.0 && (x - 40.0) <= (y - 5.0)
    };
    let inner = |x: f32, y: f32| x >= 14.0 && x <= 50.0 && y >= 7.0 && y <= 57.0 && (x - 39.0) <= (y - 7.0);
    c.fill(&page, edge);
    c.fill(&inner, paper);
    c.fill(&poly(vec![(40.0, 5.0), (52.0, 17.0), (40.0, 17.0)]), fold);
    let (tint, glyph_name) = match category {
        "image" => (p.media, "image"),
        "video" => (p.media, "video"),
        "audio" => (p.media, "audio"),
        "archive" => (p.archive, "archive"),
        "code" => (p.code, "code"),
        "text" => (p.document, "text"),
        "document" => (p.code, "document"),
        "pdf" => (p.danger, "pdf"),
        "font" => (p.document, "font"),
        "disk" => (p.subtext, "disk"),
        _ => (p.document, ""),
    };
    if category == "pdf" {
        glyph(&mut c, "pdf", 32.0, 44.0, 26.0, tint);
        let txt = RgbaColor { a: 255, ..paper };
        // three small bars on the band, a stand-in for the "PDF" label
        for i in 0..3 {
            let x = 22.0 + i as f32 * 8.0;
            c.fill(&rrect(x, 42.0, x + 5.0, 46.0, 1.0), txt);
        }
    } else if !glyph_name.is_empty() {
        glyph(&mut c, glyph_name, 32.0, 38.0, 20.0, tint);
    }
    c
}

fn executable_icon(p: &Palette) -> Canvas {
    let mut c = Canvas::new();
    let frame = p.exec;
    c.fill(&rrect(6.0, 10.0, 58.0, 54.0, 6.0), frame);
    let screen = if p.mode == ThemeMode::Light { p.surface } else { p.base };
    c.fill(&rrect(10.0, 20.0, 54.0, 50.0, 3.0), screen);
    for (i, x) in [12.0f32, 18.0, 24.0].iter().enumerate() {
        let dot = mix(screen, frame, 0.4 + i as f32 * 0.2);
        c.fill(&circle(*x, 15.0, 2.0), dot);
    }
    c.fill(&stroke(vec![(18.0, 28.0), (26.0, 35.0), (18.0, 42.0)], 3.5), frame);
    c.fill(&stroke(vec![(30.0, 43.0), (42.0, 43.0)], 3.5), frame);
    c
}

fn drive_icon(p: &Palette) -> Canvas {
    let mut c = Canvas::new();
    let body = mix(p.surface, p.fg, if p.mode == ThemeMode::Light { 0.22 } else { 0.18 });
    let face = mix(body, p.fg, 0.12);
    c.fill(&rrect(6.0, 18.0, 58.0, 46.0, 6.0), body);
    c.fill(&rrect(6.0, 34.0, 58.0, 46.0, 6.0), face);
    c.fill(&circle(48.0, 40.0, 3.0), p.accent);
    for i in 0..4 {
        let x = 14.0 + i as f32 * 6.0;
        c.fill(&rrect(x, 38.0, x + 3.0, 42.0, 1.0), mix(face, p.fg, 0.35));
    }
    c
}

fn save_png(canvas: &Canvas, path: &Path) -> Result<()> {
    let img = image::RgbaImage::from_raw(SIZE, SIZE, canvas.to_rgba8())
        .ok_or_else(|| anyhow::anyhow!("icon buffer size"))?;
    img.save(path)?;
    Ok(())
}

/// (Re)draws the built-in `chickenwm` icon theme from the palette.
pub fn generate_builtin(p: &Palette) -> Result<PathBuf> {
    let dir = icons_root().join("chickenwm");
    std::fs::create_dir_all(&dir)?;
    save_png(&folder_icon(p, None), &dir.join("folder.png"))?;
    for s in SPECIAL_FOLDERS {
        save_png(&folder_icon(p, Some(s)), &dir.join(format!("folder-{s}.png")))?;
    }
    save_png(&drive_icon(p), &dir.join("drive.png"))?;
    for cat in CATEGORIES {
        let canvas = if cat == "executable" { executable_icon(p) } else { file_icon(p, cat) };
        save_png(&canvas, &dir.join(format!("{cat}.png")))?;
    }
    std::fs::write(
        dir.join("README.txt"),
        "Built-in ChickenWM icon theme, regenerated from the system palette whenever the theme \
         changes. Don't edit these files; copy the folder under a new name to make your own theme.\n",
    )?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::DEFAULT_ACCENT;

    #[test]
    fn categories_and_resolution() {
        assert_eq!(category_for_ext("rs"), "code");
        assert_eq!(category_for_ext("PDF".to_ascii_lowercase().as_str()), "pdf");
        assert_eq!(category_for_ext("weird"), "file");

        let dir = std::env::temp_dir().join("chickenwm-icon-theme-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["folder.png", "code.png", "file.png", "ext-rs.png"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let theme = IconTheme { name: "t".into(), dir: Some(dir.clone()) };
        assert_eq!(theme.resolve(&IconRequest::File("rs".into())), Some(dir.join("ext-rs.png")));
        assert_eq!(theme.resolve(&IconRequest::File("py".into())), Some(dir.join("code.png")));
        assert_eq!(theme.resolve(&IconRequest::File("zip".into())), Some(dir.join("file.png")));
        assert_eq!(theme.resolve(&IconRequest::SpecialFolder("music".into())), Some(dir.join("folder.png")));
        assert_eq!(theme.resolve(&IconRequest::App("code".into())), None);
        assert!(IconTheme::load("system").is_system());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn builtin_icons_render() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let p = Palette::new(DEFAULT_ACCENT, mode);
            let mut all = vec![folder_icon(&p, None), executable_icon(&p), drive_icon(&p)];
            all.extend(SPECIAL_FOLDERS.iter().map(|s| folder_icon(&p, Some(s))));
            all.extend(CATEGORIES.iter().filter(|c| **c != "executable").map(|c| file_icon(&p, c)));
            for canvas in all {
                let px = canvas.to_rgba8();
                let opaque = px.chunks(4).filter(|c| c[3] > 200).count();
                let clear = px.chunks(4).filter(|c| c[3] == 0).count();
                assert!(opaque > 600 && clear > 300, "icon should have a solid shape and transparent margins");
            }
        }
    }
}
