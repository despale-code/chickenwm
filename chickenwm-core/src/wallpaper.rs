//! Generated default wallpaper.
//!
//! A quiet background drawn entirely from the theme palette: a soft diagonal gradient between
//! lighter and darker counterparts of the accent, a few large blurred glows, faint topographic
//! contour lines, and a light vignette. Fine dithering keeps 4K gradients from banding.
//!
//! `chickenwm-paper` uses it when no `wallpaper = …` is configured and `~/.config/chickenwm/bg/`
//! is empty, so the desktop always matches the accent color and light/dark mode. The repository's
//! `assets/wallpapers/chickenwm-default-4k.png` is this image at 3840×2160 for `#242321` in dark
//! mode (`chickenctl render-wallpaper` re-creates it for any theme).

use crate::config::RgbaColor;
use crate::theme::{Palette, ThemeMode};

#[derive(Clone, Copy)]
struct Glow {
    x: f32,
    y: f32,
    r: f32,
    color: [f32; 3],
    strength: f32,
}

fn rgb(c: RgbaColor) -> [f32; 3] {
    [c.r as f32, c.g as f32, c.b as f32]
}

fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Cheap per-pixel hash in [-0.5, 0.5) for dithering.
fn hash(x: u32, y: u32) -> f32 {
    let mut h = x.wrapping_mul(374_761_393) ^ y.wrapping_mul(668_265_263);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) & 0xFFFF) as f32 / 65536.0 - 0.5
}

/// Height field whose contour lines form the topographic pattern (resolution independent:
/// coordinates are in units of the image height).
fn field(u: f32, v: f32) -> f32 {
    let bump = |cx: f32, cy: f32, s: f32, a: f32| a * (-((u - cx).powi(2) + (v - cy).powi(2)) / (s * s)).exp();
    bump(1.30, 0.70, 0.55, 1.00) + bump(0.35, 0.25, 0.40, 0.70) + bump(1.55, 0.05, 0.30, 0.45)
        - bump(0.85, 1.05, 0.45, 0.55)
        + 0.12 * (u * 2.1 + v * 1.3).sin() * (v * 2.7 - u * 0.8).cos()
}

/// Renders the wallpaper as straight RGBA8, row-major.
pub fn render(p: &Palette, width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width.max(1), height.max(1));
    let light = p.mode == ThemeMode::Light;
    // Gradient ends: a touch lighter / darker than the base, along the accent hue.
    // A clearly visible (but still soft) diagonal gradient.
    let (top, bottom) = if light {
        (rgb(p.surface), lerp3(rgb(p.mantle), rgb(p.fg), 0.07))
    } else {
        (lerp3(rgb(p.surface), rgb(p.fg), 0.07), lerp3(rgb(p.mantle), [0.0, 0.0, 0.0], 0.3))
    };
    let line = rgb(p.fg);
    let line_alpha = if light { 0.055 } else { 0.07 };
    let aspect = w as f32 / h as f32;
    let glows = [
        Glow { x: 0.18 * aspect, y: 0.12, r: 0.75, color: rgb(p.surface), strength: if light { 0.55 } else { 0.35 } },
        Glow { x: 0.86 * aspect, y: 0.80, r: 0.65, color: rgb(p.dir), strength: if light { 0.10 } else { 0.10 } },
        Glow { x: 0.62 * aspect, y: 0.30, r: 0.50, color: rgb(p.accent_soft), strength: if light { 0.16 } else { 0.06 } },
    ];
    let contour_spacing = 0.075; // in field units
    let px_per_unit = h as f32; // field coordinates are scaled by height

    let mut out = vec![0u8; (w * h * 4) as usize];
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(16) as u32;
    let rows_per = h.div_ceil(threads);
    std::thread::scope(|scope| {
        for (i, chunk) in out.chunks_mut((rows_per * w * 4) as usize).enumerate() {
            let y0 = i as u32 * rows_per;
            let glows = &glows;
            scope.spawn(move || {
                for (row_i, row) in chunk.chunks_mut((w * 4) as usize).enumerate() {
                    let y = y0 + row_i as u32;
                    let v = y as f32 / h as f32;
                    for x in 0..w {
                        let u = x as f32 / h as f32;
                        // Diagonal gradient
                        let t = smoothstep(0.0, 1.0, (u / aspect) * 0.45 + v * 0.55);
                        let mut c = lerp3(top, bottom, t);
                        // Glows
                        for g in glows.iter() {
                            let d2 = ((u - g.x).powi(2) + (v - g.y).powi(2)) / (g.r * g.r);
                            let k = (-d2 * 2.2).exp() * g.strength;
                            c = lerp3(c, g.color, k);
                        }
                        // Topographic lines: distance to the nearest contour, in pixels.
                        let f = field(u, v);
                        let eps = 1.0 / px_per_unit;
                        let gx = (field(u + eps, v) - f) / eps;
                        let gy = (field(u, v + eps) - f) / eps;
                        let grad = (gx * gx + gy * gy).sqrt().max(1e-4);
                        // rem_euclid, not fract: fract keeps the sign and would fill f < 0.
                        let phase = (f / contour_spacing).rem_euclid(1.0);
                        let dist_units = phase.min(1.0 - phase) * contour_spacing / grad;
                        let dist_px = dist_units * px_per_unit;
                        // ~4 px wide lines with a 1 px anti-aliased edge.
                        let coverage = 1.0 - smoothstep(1.4, 2.6, dist_px);
                        // Lines fade toward the top-left so the corner stays calm.
                        let fade = smoothstep(0.15, 0.9, (u / aspect) * 0.6 + v * 0.4);
                        c = lerp3(c, line, coverage * line_alpha * fade);
                        // Vignette
                        let du = u / aspect - 0.5;
                        let dv = v - 0.5;
                        let vig = smoothstep(0.35, 0.95, (du * du + dv * dv).sqrt() * 1.25);
                        c = lerp3(c, rgb(p.mantle), vig * if light { 0.35 } else { 0.5 });
                        // Dither
                        let n = hash(x, y);
                        let o = (x * 4) as usize;
                        for ch in 0..3 {
                            row[o + ch] = (c[ch] + n).round().clamp(0.0, 255.0) as u8;
                        }
                        row[o + 3] = 255;
                    }
                }
            });
        }
    });
    out
}

/// Renders straight to a PNG file.
pub fn save_png(p: &Palette, width: u32, height: u32, path: &std::path::Path) -> anyhow::Result<()> {
    let img = image::RgbaImage::from_raw(width, height, render(p, width, height))
        .ok_or_else(|| anyhow::anyhow!("wallpaper buffer size"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    img.save(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::DEFAULT_ACCENT;

    #[test]
    fn renders_opaque_and_on_palette() {
        for mode in [ThemeMode::Light, ThemeMode::Dark] {
            let p = Palette::new(DEFAULT_ACCENT, mode);
            let px = render(&p, 320, 180);
            assert_eq!(px.len(), 320 * 180 * 4);
            assert!(px.chunks(4).all(|c| c[3] == 255));
            // Average brightness follows the mode.
            let avg = px.chunks(4).map(|c| c[0] as u32 + c[1] as u32 + c[2] as u32).sum::<u32>() as f32
                / (320.0 * 180.0 * 3.0);
            match mode {
                ThemeMode::Light => assert!(avg > 200.0, "light wallpaper too dark: {avg}"),
                ThemeMode::Dark => assert!(avg < 70.0, "dark wallpaper too light: {avg}"),
            }
        }
    }
}
