//! Theme helpers:
//! - automatic light/dark by the clock: fixed times or the real sunset/sunrise for a location
//!   (`theme { auto_dark = sun; location = 41.0, 29.0 }` or `auto_dark = times; dark_at = 19:30;
//!   light_at = 07:00 }`), applied by a background thread in the daemon,
//! - an accent colour picked from the wallpaper image,
//! - theme export / import as one shareable `.chickentheme` file.

use crate::config::RgbaColor;
use crate::theme::{ThemeConfig, ThemeMode};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoDark {
    /// `off`, `times` (use `light_at` / `dark_at`) or `sun` (sunrise / sunset at `location`).
    pub mode: String,
    pub light_at: String,
    pub dark_at: String,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

impl Default for AutoDark {
    fn default() -> Self {
        Self { mode: "off".into(), light_at: "07:00".into(), dark_at: "19:00".into(), latitude: None, longitude: None }
    }
}

/// `"19:30"` → minutes after midnight.
pub fn parse_hm(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.trim().parse().ok()?, m.trim().parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

pub fn fmt_hm(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60 % 24, minutes % 60)
}

fn day_of_year(year: i32, month: u32, day: u32) -> u32 {
    const CUM: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    CUM[(month.clamp(1, 12) - 1) as usize] + day + u32::from(leap && month > 2)
}

/// Sunrise and sunset in local minutes after midnight (NOAA's approximation, good to a few
/// minutes). `None` during polar day / night.
pub fn sun_times(lat: f64, lon: f64, year: i32, month: u32, day: u32, utc_offset_min: i32) -> Option<(u32, u32)> {
    use std::f64::consts::PI;
    let n = f64::from(day_of_year(year, month, day));
    let gamma = 2.0 * PI / 365.0 * (n - 1.0);
    let eqtime = 229.18
        * (0.000075 + 0.001868 * gamma.cos() - 0.032077 * gamma.sin() - 0.014615 * (2.0 * gamma).cos()
            - 0.040849 * (2.0 * gamma).sin());
    let decl = 0.006918 - 0.399912 * gamma.cos() + 0.070257 * gamma.sin() - 0.006758 * (2.0 * gamma).cos()
        + 0.000907 * (2.0 * gamma).sin()
        - 0.002697 * (3.0 * gamma).cos()
        + 0.00148 * (3.0 * gamma).sin();
    let lat_r = lat.to_radians();
    let cos_ha = (90.833f64.to_radians().cos() / (lat_r.cos() * decl.cos())) - lat_r.tan() * decl.tan();
    if !(-1.0..=1.0).contains(&cos_ha) {
        return None;
    }
    let ha = cos_ha.acos().to_degrees();
    let rise_utc = 720.0 - 4.0 * (lon + ha) - eqtime;
    let set_utc = 720.0 - 4.0 * (lon - ha) - eqtime;
    let local = |m: f64| ((m + f64::from(utc_offset_min)).rem_euclid(1440.0)).round() as u32;
    Some((local(rise_utc), local(set_utc)))
}

/// The mode the clock asks for (`None` when automatic switching is off or not configured).
/// `minute` is local minutes after midnight.
pub fn desired_mode(auto: &AutoDark, year: i32, month: u32, day: u32, minute: u32, utc_offset_min: i32) -> Option<ThemeMode> {
    let (light, dark) = match auto.mode.as_str() {
        "times" | "time" | "schedule" => (parse_hm(&auto.light_at)?, parse_hm(&auto.dark_at)?),
        "sun" | "sunset" => {
            let (lat, lon) = (auto.latitude?, auto.longitude?);
            match sun_times(lat, lon, year, month, day, utc_offset_min) {
                Some(t) => t,
                // Polar summer / winter: decide by the sun's height at noon.
                None => return Some(if (lat > 0.0) == (3..=9).contains(&month) { ThemeMode::Light } else { ThemeMode::Dark }),
            }
        }
        _ => return None,
    };
    let is_light = if light <= dark { minute >= light && minute < dark } else { minute >= light || minute < dark };
    Some(if is_light { ThemeMode::Light } else { ThemeMode::Dark })
}

/// Local date, minutes after midnight, and the UTC offset in minutes.
pub fn local_clock() -> (i32, u32, u32, u32, i32) {
    use windows::Win32::System::SystemInformation::{GetLocalTime, GetSystemTime};
    let (l, u) = unsafe { (GetLocalTime(), GetSystemTime()) };
    let lm = i32::from(l.wHour) * 60 + i32::from(l.wMinute);
    let um = i32::from(u.wHour) * 60 + i32::from(u.wMinute);
    let mut offset = lm - um;
    // Across midnight UTC: the local date differs from the UTC date.
    if l.wDay != u.wDay {
        offset += if (l.wYear, l.wMonth, l.wDay) > (u.wYear, u.wMonth, u.wDay) { 1440 } else { -1440 };
    }
    (i32::from(l.wYear), u32::from(l.wMonth), u32::from(l.wDay), lm as u32, offset)
}

/// Checks the clock every minute and flips `theme.mode` (re-theming every app) when the
/// schedule says so. Run once, from the daemon.
pub fn spawn_auto_dark_thread() {
    std::thread::Builder::new()
        .name("chickenwm-auto-dark".into())
        .spawn(|| loop {
            if let Err(e) = auto_dark_tick() {
                tracing::warn!("auto dark: {e:#}");
            }
            std::thread::sleep(std::time::Duration::from_secs(60));
        })
        .ok();
}

/// One check; returns the mode it switched to, if it switched.
pub fn auto_dark_tick() -> anyhow::Result<Option<ThemeMode>> {
    let (mut doc, cfg) = crate::config::load_or_init_config()?;
    let (y, m, d, minute, offset) = local_clock();
    let Some(want) = desired_mode(&cfg.theme.auto_dark, y, m, d, minute, offset) else { return Ok(None) };
    if want == cfg.theme.mode {
        return Ok(None);
    }
    let mut theme = cfg.theme;
    theme.mode = want;
    let palette = crate::theme::apply_theme(&theme)?;
    crate::theme::write_theme_keys(&mut doc, &theme, &palette);
    crate::config::save_config_document(&doc)?;
    let _ = crate::ipc::send_ipc_command_blocking(crate::ipc::PIPE_NAME_WM, &crate::ipc::IpcCommand::ReloadConfig);
    tracing::info!("auto dark: switched to {}", want.as_str());
    Ok(Some(want))
}

/// Writes the auto-dark keys into `theme { }`.
pub fn write_auto_dark_keys(doc: &mut crate::config::ConfigDocument, auto: &AutoDark) {
    doc.set_category_key("theme", "auto_dark", &auto.mode);
    doc.set_category_key("theme", "light_at", &auto.light_at);
    doc.set_category_key("theme", "dark_at", &auto.dark_at);
    if let (Some(lat), Some(lon)) = (auto.latitude, auto.longitude) {
        doc.set_category_key("theme", "location", &format!("{lat:.4}, {lon:.4}"));
    }
}

// ---------------------------------------------------------------------------------------------
// Accent from wallpaper
// ---------------------------------------------------------------------------------------------

/// The wallpaper's most prominent colourful hue, as an accent (lightness / saturation evened
/// out so it works as a UI colour). `None` if the image can't be read or is all greys.
pub fn accent_from_image(path: &Path) -> Option<RgbaColor> {
    let img = image::open(path).ok()?.thumbnail(96, 96).to_rgb8();
    accent_from_pixels(img.pixels().map(|p| [p[0], p[1], p[2]]))
}

pub fn accent_from_pixels(pixels: impl Iterator<Item = [u8; 3]>) -> Option<RgbaColor> {
    // 36 hue buckets, weighted by how colourful (and not too dark / bright) each pixel is.
    let mut weight = [0f64; 36];
    let mut sums = [[0f64; 3]; 36];
    for [r, g, b] in pixels {
        let hsl = crate::theme::to_hsl(RgbaColor { r, g, b, a: 255 });
        let w = f64::from(hsl.s) * (1.0 - (f64::from(hsl.l) - 0.5).abs() * 2.0).max(0.0);
        if w < 0.08 {
            continue;
        }
        let i = ((hsl.h / 10.0) as usize).min(35);
        weight[i] += w;
        sums[i][0] += f64::from(r) * w;
        sums[i][1] += f64::from(g) * w;
        sums[i][2] += f64::from(b) * w;
    }
    // Smooth over neighbouring buckets so a hue split across two still wins.
    let score = |i: usize| weight[(i + 35) % 36] * 0.5 + weight[i] + weight[(i + 1) % 36] * 0.5;
    let best = (0..36).max_by(|&a, &b| score(a).total_cmp(&score(b)))?;
    if weight[best] <= 0.0 {
        return None;
    }
    let avg = RgbaColor {
        r: (sums[best][0] / weight[best]) as u8,
        g: (sums[best][1] / weight[best]) as u8,
        b: (sums[best][2] / weight[best]) as u8,
        a: 255,
    };
    let mut hsl = crate::theme::to_hsl(avg);
    hsl.s = hsl.s.clamp(0.35, 0.75);
    hsl.l = hsl.l.clamp(0.42, 0.58);
    Some(crate::theme::from_hsl(hsl, 255))
}

// ---------------------------------------------------------------------------------------------
// Export / import
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThemeFile {
    /// Format version.
    pub chickenwm_theme: u32,
    pub name: String,
    /// `#rrggbb`
    pub accent: String,
    pub mode: String,
    #[serde(default)]
    pub preset: String,
    #[serde(default)]
    pub icon_theme: String,
    /// `browser:dark, files:light`
    #[serde(default)]
    pub app_modes: String,
    #[serde(default)]
    pub auto_dark: Option<AutoDark>,
}

pub fn export_theme(theme: &ThemeConfig, name: &str) -> String {
    let c = theme.accent;
    let file = ThemeFile {
        chickenwm_theme: 1,
        name: name.to_owned(),
        accent: format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b),
        mode: theme.mode.as_str().to_owned(),
        preset: theme.preset.clone(),
        icon_theme: theme.icon_theme.clone(),
        app_modes: theme.app_modes_string(),
        auto_dark: (theme.auto_dark.mode != "off").then(|| theme.auto_dark.clone()),
    };
    serde_json::to_string_pretty(&file).unwrap_or_default()
}

/// Reads a `.chickentheme` into a theme (starting from `base`, so unset fields keep their values).
pub fn import_theme(text: &str, base: &ThemeConfig) -> anyhow::Result<(String, ThemeConfig)> {
    let file: ThemeFile = serde_json::from_str(text).map_err(|e| anyhow::anyhow!("not a ChickenWM theme file: {e}"))?;
    anyhow::ensure!(file.chickenwm_theme >= 1, "not a ChickenWM theme file");
    let mut t = base.clone();
    t.accent = RgbaColor::parse(&file.accent).map(|c| RgbaColor { a: 255, ..c }).ok_or_else(|| anyhow::anyhow!("bad accent colour {}", file.accent))?;
    t.mode = ThemeMode::parse(&file.mode).unwrap_or(t.mode);
    t.preset = file.preset;
    if !file.icon_theme.is_empty() {
        t.icon_theme = file.icon_theme;
    }
    t.app_modes = ThemeConfig::parse_app_modes(&file.app_modes);
    if let Some(a) = file.auto_dark {
        t.auto_dark = a;
    }
    Ok((file.name, t))
}

/// Imports a theme file, applies it to every app and saves it in chickenwm.conf.
pub fn import_and_apply(path: &Path) -> anyhow::Result<String> {
    let text = std::fs::read_to_string(path)?;
    let (mut doc, cfg) = crate::config::load_or_init_config()?;
    let (name, theme) = import_theme(&text, &cfg.theme)?;
    let palette = crate::theme::apply_theme(&theme)?;
    crate::theme::write_theme_keys(&mut doc, &theme, &palette);
    write_auto_dark_keys(&mut doc, &theme.auto_dark);
    crate::config::save_config_document(&doc)?;
    let _ = crate::ipc::send_ipc_command_blocking(crate::ipc::PIPE_NAME_WM, &crate::ipc::IpcCommand::ReloadConfig);
    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn istanbul_sun() {
        // Istanbul, 2026-06-21, UTC+3: sunrise ≈ 05:32, sunset ≈ 20:39.
        let (rise, set) = sun_times(41.01, 28.97, 2026, 6, 21, 180).unwrap();
        assert!((rise as i32 - (5 * 60 + 32)).abs() <= 6, "rise {}", fmt_hm(rise));
        assert!((set as i32 - (20 * 60 + 39)).abs() <= 6, "set {}", fmt_hm(set));
        // Midwinter (Istanbul is on UTC+3 all year: sunset ≈ 17:39).
        let (rise, set) = sun_times(41.01, 28.97, 2026, 12, 21, 180).unwrap();
        assert!((rise as i32 - (8 * 60 + 25)).abs() <= 8, "rise {}", fmt_hm(rise));
        assert!((set as i32 - (17 * 60 + 39)).abs() <= 8, "set {}", fmt_hm(set));
        // Polar night.
        assert!(sun_times(78.2, 15.6, 2026, 12, 21, 60).is_none());
    }

    #[test]
    fn schedules() {
        let times = AutoDark { mode: "times".into(), light_at: "07:00".into(), dark_at: "19:30".into(), ..Default::default() };
        assert_eq!(desired_mode(&times, 2026, 10, 18, 12 * 60, 180), Some(ThemeMode::Light));
        assert_eq!(desired_mode(&times, 2026, 10, 18, 20 * 60, 180), Some(ThemeMode::Dark));
        assert_eq!(desired_mode(&times, 2026, 10, 18, 3 * 60, 180), Some(ThemeMode::Dark));
        // Inverted schedule (night-shift people): light from 20:00 to 06:00.
        let inv = AutoDark { light_at: "20:00".into(), dark_at: "06:00".into(), ..times.clone() };
        assert_eq!(desired_mode(&inv, 2026, 10, 18, 23 * 60, 180), Some(ThemeMode::Light));
        assert_eq!(desired_mode(&AutoDark::default(), 2026, 10, 18, 600, 0), None);
        let sun_no_loc = AutoDark { mode: "sun".into(), ..Default::default() };
        assert_eq!(desired_mode(&sun_no_loc, 2026, 10, 18, 600, 0), None);
        assert_eq!(parse_hm("7:05"), Some(425));
        assert_eq!(parse_hm("24:00"), None);
    }

    #[test]
    fn wallpaper_accent() {
        // Mostly a teal picture with some grey and a little red.
        let mut px = vec![[20u8, 140, 150]; 600];
        px.extend(vec![[120, 120, 120]; 900]);
        px.extend(vec![[200, 30, 30]; 100]);
        let c = accent_from_pixels(px.into_iter()).unwrap();
        let h = crate::theme::to_hsl(c).h;
        assert!((170.0..200.0).contains(&h), "hue {h}");
        assert!(accent_from_pixels(vec![[128u8, 128, 128]; 50].into_iter()).is_none());
    }

    #[test]
    fn export_import_round_trip() {
        let mut t = ThemeConfig::default();
        t.accent = RgbaColor { r: 0x24, g: 0x23, b: 0x21, a: 255 };
        t.mode = ThemeMode::Dark;
        t.app_modes = ThemeConfig::parse_app_modes("browser:light");
        t.auto_dark = AutoDark { mode: "sun".into(), latitude: Some(41.0), longitude: Some(29.0), ..Default::default() };
        let text = export_theme(&t, "Night Coffee");
        let (name, back) = import_theme(&text, &ThemeConfig::default()).unwrap();
        assert_eq!(name, "Night Coffee");
        assert_eq!(back, t);
        assert!(import_theme("{}", &t).is_err());
    }
}
