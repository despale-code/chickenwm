//! Fonts: resolving the CSS `--font-family` of each module to a font file, listing installed
//! families, and downloading Nerd Fonts into the per-user font folder (no admin rights).
//!
//! Lookups go through DirectWrite's system font collection, which covers both system fonts and
//! fonts installed just for the current user, by their real family names (`Consolas`, not
//! `consola.ttf`).

use anyhow::{bail, Context, Result};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use windows::core::{Interface, HSTRING, PCWSTR};
use windows::Win32::Foundation::{BOOL, LPARAM, WPARAM};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteFontFile,
    IDWriteLocalFontFileLoader, IDWriteLocalizedStrings, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_NORMAL,
};
use windows::Win32::Graphics::Gdi::AddFontResourceW;
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE,
    REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::WindowsAndMessaging::{SendNotifyMessageW, HWND_BROADCAST, WM_FONTCHANGE};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A Nerd Font that can be downloaded from the official release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NerdFont {
    /// Release asset name (`<asset>.zip`).
    pub asset: &'static str,
    /// Family name once installed (what goes into `--font-family`).
    pub family: &'static str,
}

/// Popular Nerd Fonts, by their installed family names.
pub const NERD_FONTS: &[NerdFont] = &[
    NerdFont { asset: "JetBrainsMono", family: "JetBrainsMono Nerd Font" },
    NerdFont { asset: "FiraCode", family: "FiraCode Nerd Font" },
    NerdFont { asset: "CascadiaCode", family: "CaskaydiaCove Nerd Font" },
    NerdFont { asset: "DepartureMono", family: "DepartureMono Nerd Font" },
    NerdFont { asset: "Hack", family: "Hack Nerd Font" },
    NerdFont { asset: "Meslo", family: "MesloLGS Nerd Font" },
    NerdFont { asset: "Iosevka", family: "Iosevka Nerd Font" },
    NerdFont { asset: "SourceCodePro", family: "SauceCodePro Nerd Font" },
    NerdFont { asset: "IBMPlexMono", family: "BlexMono Nerd Font" },
    NerdFont { asset: "GeistMono", family: "GeistMono Nerd Font" },
    NerdFont { asset: "VictorMono", family: "VictorMono Nerd Font" },
    NerdFont { asset: "Mononoki", family: "Mononoki Nerd Font" },
    NerdFont { asset: "UbuntuMono", family: "UbuntuMono Nerd Font" },
    NerdFont { asset: "Ubuntu", family: "Ubuntu Nerd Font" },
    NerdFont { asset: "RobotoMono", family: "RobotoMono Nerd Font" },
    NerdFont { asset: "SpaceMono", family: "SpaceMono Nerd Font" },
    NerdFont { asset: "Inconsolata", family: "Inconsolata Nerd Font" },
    NerdFont { asset: "ZedMono", family: "ZedMono Nerd Font" },
];

/// The CSS files whose `--font-family` sets each module's font, with a display name.
pub const FONT_TARGETS: &[(&str, &str)] = &[
    ("roobar.css", "Bar"),
    ("launcher.css", "Launcher"),
    ("files.css", "Files"),
    ("settings.css", "Settings"),
    ("notifications.css", "Notifications"),
    ("browser.css", "Browser"),
];

/// `%LOCALAPPDATA%\Microsoft\Windows\Fonts`: the per-user font folder.
pub fn user_font_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join("Microsoft").join("Windows").join("Fonts"))
}

fn system_collection(refresh: bool) -> Option<IDWriteFontCollection> {
    unsafe {
        let factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).ok()?;
        let mut collection = None;
        factory.GetSystemFontCollection(&mut collection, BOOL::from(refresh)).ok()?;
        collection
    }
}

fn localized_name(names: &IDWriteLocalizedStrings) -> Option<String> {
    unsafe {
        let mut index = 0u32;
        let mut exists = BOOL(0);
        let _ = names.FindLocaleName(&HSTRING::from("en-us"), &mut index, &mut exists);
        if !exists.as_bool() {
            index = 0;
        }
        let len = names.GetStringLength(index).ok()?;
        let mut buf = vec![0u16; len as usize + 1];
        names.GetString(index, &mut buf).ok()?;
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Every installed font family (system and per-user), sorted. `refresh` re-scans for fonts
/// installed since the process started.
pub fn installed_families(refresh: bool) -> Vec<String> {
    let mut out = Vec::new();
    let Some(collection) = system_collection(refresh) else {
        return out;
    };
    unsafe {
        for i in 0..collection.GetFontFamilyCount() {
            if let Some(name) = collection
                .GetFontFamily(i)
                .and_then(|f| f.GetFamilyNames())
                .ok()
                .and_then(|n| localized_name(&n))
            {
                out.push(name);
            }
        }
    }
    out.sort_by_key(|s| s.to_lowercase());
    out.dedup();
    out
}

/// Whether `family` is installed.
pub fn is_installed(family: &str) -> bool {
    family_file(family, false).is_some()
}

/// File (and face index inside it) of `family`'s regular face.
fn family_file(family: &str, refresh: bool) -> Option<(PathBuf, u32)> {
    let collection = system_collection(refresh)?;
    unsafe {
        let mut index = 0u32;
        let mut exists = BOOL(0);
        collection.FindFamilyName(&HSTRING::from(family), &mut index, &mut exists).ok()?;
        if !exists.as_bool() {
            return None;
        }
        let font = collection
            .GetFontFamily(index)
            .ok()?
            .GetFirstMatchingFont(DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL)
            .ok()?;
        let face = font.CreateFontFace().ok()?;
        let mut count = 1u32;
        let mut files: [Option<IDWriteFontFile>; 1] = [None];
        face.GetFiles(&mut count, Some(files.as_mut_ptr())).ok()?;
        let file = files[0].take()?;
        let mut key = std::ptr::null_mut();
        let mut key_size = 0u32;
        file.GetReferenceKey(&mut key, &mut key_size).ok()?;
        let loader: IDWriteLocalFontFileLoader = file.GetLoader().ok()?.cast().ok()?;
        let len = loader.GetFilePathLengthFromKey(key, key_size).ok()?;
        let mut buf = vec![0u16; len as usize + 1];
        loader.GetFilePathFromKey(key, key_size, &mut buf).ok()?;
        Some((PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])), face.GetIndex()))
    }
}

/// Names in a CSS `font-family` list, unquoted, generic families (`sans-serif`…) left out.
pub fn css_families(list: &str) -> Vec<String> {
    list.split(',')
        .map(|f| f.trim().trim_matches(['"', '\'']).trim().to_string())
        .filter(|f| {
            !f.is_empty()
                && !matches!(
                    f.to_ascii_lowercase().as_str(),
                    "serif" | "sans-serif" | "monospace" | "cursive" | "fantasy" | "system-ui" | "ui-monospace"
                )
        })
        .collect()
}

/// Resolves a CSS `font-family` list to the first installed family's font file and face index.
/// Re-scans for new fonts (it only runs when a module's font setting changes), and falls back to
/// matching file names for a font installed so recently DirectWrite hasn't picked it up yet.
pub fn resolve_css_font(list: &str) -> Option<(PathBuf, u32)> {
    let families = css_families(list);
    families
        .iter()
        .find_map(|f| family_file(f, true))
        .or_else(|| families.iter().find_map(|f| file_by_name(f).map(|p| (p, 0))))
}

/// `JetBrainsMono Nerd Font` -> `JetBrainsMonoNerdFont-Regular.ttf` in the font folders.
fn file_by_name(family: &str) -> Option<PathBuf> {
    let normalize = |s: &str| s.chars().filter(char::is_ascii_alphanumeric).collect::<String>().to_ascii_lowercase();
    let want = normalize(family);
    if want.is_empty() {
        return None;
    }
    let regular = format!("{want}regular");
    let dirs = [std::env::var_os("WINDIR").map(|w| PathBuf::from(w).join("Fonts")), user_font_dir()];
    let files: Vec<PathBuf> = dirs
        .iter()
        .flatten()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("ttf") || e.eq_ignore_ascii_case("otf"))
        })
        .collect();
    let stem = |p: &PathBuf| p.file_stem().and_then(|s| s.to_str()).map(normalize).unwrap_or_default();
    files
        .iter()
        .find(|p| stem(p) == regular)
        .or_else(|| files.iter().find(|p| stem(p) == want))
        .cloned()
}

/// The `--font-family` value of `css_file` in the config directory.
pub fn css_font_family(css_file: &str) -> Option<String> {
    crate::css::load_css_sheet(css_file)
        .ok()?
        .custom_properties
        .get("--font-family")
        .cloned()
}

/// Sets `css_file`'s font to `family` (keeping a system fallback); modules hot-reload it.
pub fn set_css_font(css_file: &str, family: &str) -> Result<()> {
    let path = crate::config::config_dir().join(css_file);
    let source = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let value = format!("\"{}\", \"Segoe UI\", sans-serif", family.replace('"', ""));
    let updated = crate::theme::set_root_vars(&source, &[("--font-family", value)]);
    std::fs::write(&path, updated).with_context(|| format!("writing {}", path.display()))
}

/// Downloads `font` from the Nerd Fonts release and installs every face for the current user:
/// copied into the per-user font folder, registered under HKCU (so it survives sign-out), loaded
/// into the session and announced with `WM_FONTCHANGE` (so it's usable right away). Returns the
/// number of font files installed. Blocking; run it off the UI thread.
pub fn install_nerd_font(font: &NerdFont) -> Result<usize> {
    let url = format!("https://github.com/ryanoasis/nerd-fonts/releases/latest/download/{}.zip", font.asset);
    let zip_path = std::env::temp_dir().join(format!("chickenwm-font-{}.zip", font.asset));
    let status = std::process::Command::new("curl.exe")
        .args(["-L", "-f", "-s", "-S", "--retry", "2", "-o"])
        .arg(&zip_path)
        .arg(&url)
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .context("couldn't run curl.exe")?;
    if !status.success() {
        let _ = std::fs::remove_file(&zip_path);
        bail!("download failed ({url})");
    }
    let installed = install_font_zip(&zip_path);
    let _ = std::fs::remove_file(&zip_path);
    installed
}

fn install_font_zip(zip_path: &Path) -> Result<usize> {
    let dir = user_font_dir().context("LOCALAPPDATA is not set")?;
    std::fs::create_dir_all(&dir)?;
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip_path)?).context("not a valid zip")?;
    let mut count = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        if !entry.is_file() {
            continue;
        }
        let Ok(entry_name) = entry.name().map(|n| n.into_owned()) else {
            continue;
        };
        let Some(name) = Path::new(&entry_name).file_name().map(|n| n.to_owned()) else {
            continue;
        };
        let lower = name.to_string_lossy().to_ascii_lowercase();
        let kind = if lower.ends_with(".ttf") {
            "TrueType"
        } else if lower.ends_with(".otf") {
            "OpenType"
        } else {
            continue;
        };
        let target = dir.join(&name);
        if !target.exists() {
            let mut out = std::fs::File::create(&target)?;
            std::io::copy(&mut entry, &mut out)?;
        }
        let stem = Path::new(&name).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        register_user_font(&format!("{stem} ({kind})"), &target)?;
        unsafe {
            AddFontResourceW(&HSTRING::from(target.as_os_str()));
        }
        count += 1;
    }
    if count == 0 {
        bail!("the download contained no fonts");
    }
    unsafe {
        let _ = SendNotifyMessageW(HWND_BROADCAST, WM_FONTCHANGE, WPARAM(0), LPARAM(0));
    }
    Ok(count)
}

/// `HKCU\Software\Microsoft\Windows NT\CurrentVersion\Fonts`: per-user fonts, by full path.
fn register_user_font(value_name: &str, path: &Path) -> Result<()> {
    unsafe {
        let mut key = HKEY::default();
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(r"Software\Microsoft\Windows NT\CurrentVersion\Fonts"),
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
        .ok()
        .context("couldn't open the per-user font registry key")?;
        let data: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = std::slice::from_raw_parts(data.as_ptr() as *const u8, data.len() * 2);
        let res = RegSetValueExW(key, &HSTRING::from(value_name), 0, REG_SZ, Some(bytes));
        let _ = RegCloseKey(key);
        res.ok().context("couldn't register the font")
    }
}

/// Makes the font named by a CSS `font-family` list the UI font of an egui app (first in line,
/// with egui's defaults as fallback for glyphs it lacks). `monospace` also uses it for code.
/// Returns the font file used, if any.
#[cfg(feature = "egui")]
pub fn apply_egui_font(ctx: &egui::Context, css_list: &str, monospace: bool) -> Option<PathBuf> {
    let mut fonts = egui::FontDefinitions::default();
    let found = resolve_css_font(css_list).and_then(|(path, index)| {
        let bytes = std::fs::read(&path).ok()?;
        let mut data = egui::FontData::from_owned(bytes);
        data.index = index;
        fonts.font_data.insert("css".to_owned(), std::sync::Arc::new(data));
        let mut families = vec![egui::FontFamily::Proportional];
        if monospace {
            families.push(egui::FontFamily::Monospace);
        }
        for family in families {
            fonts.families.entry(family).or_default().insert(0, "css".to_owned());
        }
        Some(path)
    });
    ctx.set_fonts(fonts);
    found
}

/// Keeps an egui app's font in step with its stylesheet's `--font-family` (call every frame;
/// fonts are only rebuilt when the value changes).
#[cfg(feature = "egui")]
#[derive(Default)]
pub struct EguiFont {
    applied: Option<String>,
}

#[cfg(feature = "egui")]
impl EguiFont {
    pub fn sync(&mut self, ctx: &egui::Context, sheet: &crate::css::CssStyleSheet, monospace: bool) {
        let list = sheet.custom_properties.get("--font-family").map_or("", String::as_str);
        if self.applied.as_deref() == Some(list) {
            return;
        }
        apply_egui_font(ctx, list, monospace);
        self.applied = Some(list.to_owned());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_family_lists_are_parsed() {
        assert_eq!(
            css_families(r#""DepartureMono Nerd Font Mono", 'Cascadia Code', Consolas, monospace"#),
            vec!["DepartureMono Nerd Font Mono", "Cascadia Code", "Consolas"]
        );
    }

    #[test]
    fn system_fonts_resolve_by_family_name() {
        // Segoe UI ships with every Windows install; its file is segoeui.ttf.
        let (path, _) = resolve_css_font(r#""No Such Font", "Segoe UI", sans-serif"#).expect("Segoe UI resolves");
        assert!(path.to_string_lossy().to_ascii_lowercase().ends_with("segoeui.ttf"));
        assert!(installed_families(false).iter().any(|f| f == "Segoe UI"));
    }
}
