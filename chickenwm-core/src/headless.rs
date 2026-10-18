//! Headless rendering for the egui apps: `<app> --render-to out.png [--render-size 1280x800]
//! [--render-scale 1.5] [--render-frames 30]` runs the app's UI without creating a window and
//! saves one frame as a PNG. UI changes can then be checked without a test window stealing
//! keyboard focus. Needs a GPU adapter (wgpu); nothing is shown on screen.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct RenderRequest {
    pub out: PathBuf,
    /// Logical size of the "window".
    pub size: egui::Vec2,
    pub pixels_per_point: f32,
    /// Frames to run before the capture, so animations and async loads can settle.
    pub frames: usize,
}

impl RenderRequest {
    /// Reads `--render-to` (and the optional size/scale/frames flags) from the command line.
    /// `default_size` is used without `--render-size`.
    pub fn from_args(default_size: [f32; 2]) -> Option<Self> {
        Self::parse(std::env::args().skip(1), default_size)
    }

    pub fn parse(args: impl IntoIterator<Item = String>, default_size: [f32; 2]) -> Option<Self> {
        let args: Vec<String> = args.into_iter().collect();
        let value = |flag: &str| {
            args.iter().enumerate().find_map(|(i, a)| {
                a.strip_prefix(&format!("{flag}=")).map(str::to_owned).or_else(|| (a == flag).then(|| args.get(i + 1).cloned()).flatten())
            })
        };
        let out = PathBuf::from(value("--render-to")?);
        let size = value("--render-size")
            .and_then(|s| {
                let (w, h) = s.split_once(['x', 'X'])?;
                Some(egui::vec2(w.trim().parse().ok()?, h.trim().parse().ok()?))
            })
            .unwrap_or(egui::vec2(default_size[0], default_size[1]));
        let pixels_per_point = value("--render-scale").and_then(|s| s.parse().ok()).unwrap_or(1.0f32).clamp(0.25, 4.0);
        let frames = value("--render-frames").and_then(|s| s.parse().ok()).unwrap_or(60usize).clamp(1, 10_000);
        Some(Self { out, size, pixels_per_point, frames })
    }
}

/// The flags `from_args` understands, for apps that pass the rest of the command line on.
pub const FLAGS: [&str; 4] = ["--render-to", "--render-size", "--render-scale", "--render-frames"];

/// `args` without the render flags (and their values).
pub fn strip_render_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut skip_value = false;
    for a in args {
        if std::mem::take(&mut skip_value) {
            continue;
        }
        if FLAGS.contains(&a.as_str()) {
            skip_value = true;
        } else if !FLAGS.iter().any(|f| a.starts_with(&format!("{f}="))) {
            out.push(a.clone());
        }
    }
    out
}

/// Builds the app the way `eframe::run_native` would, runs `req.frames` frames and writes the
/// last one to `req.out`.
pub fn render_app<A: eframe::App>(
    req: &RenderRequest,
    build: impl FnOnce(&mut eframe::CreationContext<'_>) -> A,
) -> anyhow::Result<()> {
    let mut harness = egui_kittest::Harness::<A>::builder()
        .with_size(req.size)
        .with_pixels_per_point(req.pixels_per_point)
        .wgpu()
        .build_eframe(build);
    // Fixed steps rather than `run()`: the apps keep requesting repaints (clocks, animations),
    // so "run until idle" would never finish. Steps are paced at ~60 fps so background work
    // (status polling, file listing) has had real time to land in the captured frame.
    for _ in 0..req.frames {
        harness.step();
        std::thread::sleep(std::time::Duration::from_millis(16));
    }
    let image = harness.render().map_err(|e| anyhow::anyhow!("headless render failed: {e}"))?;
    save(&image, &req.out)
}

fn save(image: &image::RgbaImage, out: &Path) -> anyhow::Result<()> {
    if let Some(dir) = out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    image.save(out)?;
    tracing::info!("headless frame written to {}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn parses_flags() {
        assert_eq!(RenderRequest::parse(args("--foo"), [10.0, 20.0]), None);
        let r = RenderRequest::parse(args("--render-to a.png --render-size 800x40 --render-scale=2"), [1.0, 1.0]).unwrap();
        assert_eq!(r.out, PathBuf::from("a.png"));
        assert_eq!(r.size, egui::vec2(800.0, 40.0));
        assert_eq!(r.pixels_per_point, 2.0);
        assert_eq!(r.frames, 60);
        let r = RenderRequest::parse(args("--render-to=b.png --render-frames 5"), [300.0, 200.0]).unwrap();
        assert_eq!((r.size, r.frames), (egui::vec2(300.0, 200.0), 5));
        assert_eq!(strip_render_args(&args("C:\\dir --render-to a.png --render-scale=2 x")), args("C:\\dir x"));
    }
}
