# Styling ChickenWM

> ChickenWM is vibecoded (built with an AI coding assistant). If something in this guide doesn't
> match what you see, the code wins; please open an issue.

ChickenWM is styled at two levels:

1. **The system theme**: one accent color plus light/dark mode. ChickenWM derives a full palette
   from it and writes it into every module's stylesheet. This is what most people want.
2. **Per-module CSS**: each program has a stylesheet in `%USERPROFILE%\.config\chickenwm\` with
   variables for colors, sizes, fonts and motion. Edit them for fine control; they hot-reload when
   you save.

Icons are themed separately, with **icon themes** (see [Icon themes](#icon-themes)).

## Contents

- [The system theme](#the-system-theme)
- [How the palette is derived](#how-the-palette-is-derived)
- [Palette reference](#palette-reference)
- [Per-module stylesheets](#per-module-stylesheets)
  - [roobar.css](#roobarcss): status bar
  - [launcher.css](#launchercss): launcher
  - [files.css](#filescss): file explorer
  - [settings.css](#settingscss): settings app
  - [browser.css](#browsercss): browser
- [Window borders](#window-borders)
- [Icon themes](#icon-themes)
- [Wallpaper](#wallpaper)
- [CSS support](#css-support)
- [Recipes](#recipes)

## The system theme

The theme lives in `chickenwm.conf`:

```ini
theme {
    accent = rgb(242321)    # the seed color
    mode = light            # light | dark
    icon_theme = chickenwm  # chickenwm | system | <your pack>
}
```

The default accent is **`#242321`**, a warm charcoal. In light mode it's the color of text and
highlights on near-white surfaces that carry a hint of the same warm hue. In dark mode it becomes
the background, and its lighter counterpart takes over the highlights.

There are three ways to apply a theme. All three do the same thing:

| Where | How |
|---|---|
| Settings app | **Appearance → System Theme**: pick the accent (color picker or hex), Light/Dark and the icon theme. The preview shows a mock window, every palette color and the icon set. Press **Apply to whole system**. |
| Command line | `chickenctl theme "#242321" --mode light --icons chickenwm`. Every argument is optional, so `chickenctl theme --mode dark` keeps the accent and switches the mode, and plain `chickenctl theme` re-applies the current theme. Add `-j` for the palette as JSON. |
| By hand | Edit the `theme { }` block, then run `chickenctl theme` to write the palette out. |

Applying a theme:

1. writes the palette into the color variables of `roobar.css`, `launcher.css`, `files.css`,
   `settings.css` and `browser.css`;
2. sets `col.active_border` (the focused-window border) to the accent;
3. redraws the built-in `chickenwm` icon theme in the new colors;
4. saves `theme { }` in `chickenwm.conf`.

Every running module hot-reloads, so the change shows up within a moment.

> [!NOTE]
> Applying a theme only **replaces the color variables** listed for each file below. Your fonts,
> sizes, radii, animation timings, extra variables and custom rules are left alone. To keep a
> color the theme would overwrite, don't re-apply the theme after editing it, or use a variable of
> your own in your rules.

> [!TIP]
> In `chickenwm.conf`, `#` starts a comment, so write the accent as `rgb(242321)`. On the command
> line and in the settings app, `#242321` is fine.

## How the palette is derived

The accent's **hue** and **saturation** are kept and only the **lightness** changes, so every color
in the system is a lighter or darker counterpart of the same color. Very neutral accents (like
`#242321`) still get a faint tint of their hue in the surfaces.

**Light mode**

| Role | Derivation | With `#242321` |
|---|---|---|
| base (window background) | accent hue at 94.5% lightness | `#f2f1f0` |
| surface (panes, cards, fields) | 98.5% | `#fbfbfb` |
| mantle (sidebars, tab strips, status bars) | 90.5% | near `#e8e6e3` |
| text | the accent, if it's dark enough; otherwise a 15% tone | `#242321` |
| accent | the accent (darkened if it would be unreadable on light surfaces) | `#242321` |
| on-accent (text on accent fills) | 96.5% tone, the lighter counterpart | `#f7f6f6` |
| accent soft | 82% tone | light warm grey |

**Dark mode**

| Role | Derivation | With `#242321` |
|---|---|---|
| base | the accent itself (or a 14% tone if the accent is light) | `#242321` |
| surface | base + 4.5% lightness | slightly lighter charcoal |
| mantle | base − 3% | slightly darker charcoal |
| text | 92% tone | warm off-white |
| accent | the lighter counterpart (84% tone) | warm light grey |
| on-accent | base | `#242321` |

Both modes also derive muted, accent-friendly **file-type tints** (folders, apps, archives, media,
code, documents) and **danger/success** colors. A brighter, more colorful accent (say `#3b82f6`)
works too: in light mode it's used for highlights and borders as-is, and text becomes a dark tone
of the same hue.

Contrast is checked in the test suite: body text vs. background stays above 10:1 in both modes for
the default accent, and the accent stays at least 3:1 against light surfaces for any seed.

## Palette reference

Names used in the tables below:

| Token | Used for |
|---|---|
| `base` | Window and page backgrounds |
| `surface` | Panes, cards, inputs, popups |
| `mantle` | Sidebars, tab strips, status bars |
| `fg` | Body text |
| `subtext` | Secondary text and labels |
| `faint` | Placeholder and disabled text |
| `border` | Hairlines and outlines |
| `shadow` | Drop shadows |
| `accent` | Selection, active tab/workspace, focus rings, window border |
| `accent_fg` | Text and icons on an `accent` fill |
| `accent_soft` | Secondary highlights |
| `hover`, `press` | Hover and pressed backgrounds |
| `selected` | Selected rows (translucent accent) |
| `danger`, `success` | Errors and confirmations |
| `dir`, `exec`, `archive`, `media`, `code`, `document` | File-type colors |

## Per-module stylesheets

Every module reads variables from the `:root { }` block of its stylesheet:

```css
:root {
    --accent-color: #242321;
    --border-radius: 12px;
}
```

The **Theme** column shows which palette token a variable gets when you apply a theme; a dash means
the theme leaves it alone.

> The bar, launcher, file explorer and settings app are native (egui) programs: they read **only
> the `:root` variables** below. Other rules in their files (`.launcher-window { … }` etc.) are
> parsed but not used yet. `browser.css` is different: it's real CSS applied to the browser's UI, so
> any rule works there.

### roobar.css

The status bar. Each module (workspaces, window title, clock, CPU, memory…) is a floating island.

| Variable | Theme | Meaning |
|---|---|---|
| `--bar-bg` | – | Bar background behind the islands (default transparent) |
| `--pill-bg` | surface @ 95% | Island background |
| `--fg-color` | fg | Island text |
| `--subtext-color` | subtext | Secondary text |
| `--accent-color` | accent_fg | Hover glow and active text |
| `--accent-secondary` | subtext | Secondary labels |
| `--border-color` | border | Island outline |
| `--shadow-color` | shadow | Island shadow |
| `--ws-active-bg` / `--ws-active-fg` | accent / accent_fg | Active workspace highlight (it slides between workspaces) |
| `--ws-occupied-bg` / `--ws-occupied-fg` | translucent accent / fg | Workspaces with windows |
| `--ws-empty-bg` / `--ws-empty-fg` | – / faint | Empty workspaces |
| `--border-radius` | – | Island corner radius (px, default 12) |
| `--pill-radius` | – | Inner pill radius (px, default 10) |
| `--island-gap` | – | Space between islands (px, default 8) |
| `--bar-height` | – | Bar height (px, default 36) |
| `--bar-margin-top`, `--bar-margin-x` | – | Distance from the screen edges (px) |
| `--font-family` | – | Comma-separated list; the first installed font is used. Nerd Fonts enable icon glyphs. |
| `--font-size` | – | Text size (px, default 22) |
| `--font-pixel-grid` | – | For pixel fonts: snap the size to multiples of this (0 = off) |
| `--font-weight` | – | `bold` or ≥ `600` for bold |
| `--anim-ms` | – | Duration of the workspace slide, width easing and title cross-fade (default 220) |

### launcher.css

The launcher draws rofi's classic layout: a prompt line and a list of results.

| Variable | Theme | Meaning |
|---|---|---|
| `--rofi-background` | surface | Window and even-row background |
| `--rofi-foreground` | fg | Text and window outline |
| `--rofi-lightbg` | mantle | Odd-row (striped) background |
| `--rofi-lightfg` | subtext | Secondary text |
| `--rofi-selected-bg` | accent | Selected row background |
| `--rofi-selected-fg` | accent_fg | Selected row text |
| `--bg-color`, `--surface-color`, `--fg-color`, `--subtext-color`, `--accent-color`, `--accent-secondary`, `--selected-bg`, `--border-color` | base, surface, fg, subtext, accent, accent_soft, selected, border | Kept in sync for future layouts |

Without the `--rofi-*` variables the launcher falls back to rofi's default Solarized-light colors.

### files.css

The file explorer.

| Variable | Theme | Meaning |
|---|---|---|
| `--bg-color` | base | Window background |
| `--pane-bg` | surface | File list, preview pane, active tab |
| `--sidebar-bg` | mantle | Sidebar, toolbar, status bar |
| `--fg-color` | fg | Text (secondary text is derived from it) |
| `--subtext-color` | subtext | Secondary text |
| `--accent-color` | accent | Focus outline, drag-and-drop highlight, buttons |
| `--accent-fg` | accent_fg | Text on accent fills |
| `--selected-bg` | selected | Selected rows |
| `--border-color` | border | Outlines and separators |
| `--danger-color` | danger | Errors |
| `--dir-color` | dir | Folder names |
| `--exec-color` | exec | `.exe` `.bat` `.cmd` `.ps1` `.msi` `.lnk` names |
| `--archive-color` | archive | `.zip` `.7z` `.rar` `.gz` `.tar`… names |
| `--media-color` | media | Image, audio and video names |
| `--border-radius` | – | Corner radius (px, default 8) |
| `--row-height` | – | List row height (px, 20–48, default 28) |
| `--font-size` | – | Text size (px, 10–22, default 14) |

### settings.css

The settings app. A light or dark widget style is picked automatically from the brightness of
`--bg-color`.

| Variable | Theme | Meaning |
|---|---|---|
| `--bg-color` | base | Main area background |
| `--card-bg` | surface | Cards, inputs, popups |
| `--sidebar-bg` | mantle | Sidebar and footer |
| `--fg-color` | fg | Text |
| `--subtext-color` | subtext | Secondary text |
| `--accent-color` | accent | Highlights and active widgets |
| `--accent-fg` | accent_fg | Text on accent |
| `--accent-secondary` | accent_soft | Secondary highlights |
| `--selected-bg` | selected | Hover and selection |
| `--danger-color` | danger | Destructive actions |
| `--border-color` | border | Outlines |

### browser.css

`browser.css` is real CSS, injected into the browser's toolbar, menu and `about:settings` page, so
you can restyle anything there (`.tab`, `.tab.active`, `#field`, `#shield`…). Web pages are never
affected.

| Variable | Theme | Meaning |
|---|---|---|
| `--bg-color` | base | Toolbar background (also the window color before the toolbar loads) |
| `--tab-strip-bg`, `--sidebar-bg` | mantle | Tab strip, settings sidebar |
| `--surface-color`, `--card-bg` | surface | Menus, cards |
| `--field-bg`, `--field-hover-bg`, `--field-focus-bg` | surface variants | Address bar states |
| `--fg-color`, `--subtext-color`, `--faint-color` | fg, subtext, faint | Text |
| `--accent-color`, `--accent-ring` | accent | Active tab, focus ring, loading bar, ad-block badge |
| `--accent-fg` | accent_fg | Text on the active tab and badges |
| `--accent-secondary` | subtext | Secondary labels |
| `--hover-bg`, `--press-bg`, `--active-bg` | hover, press | Button states |
| `--border-color`, `--shadow-color` | border, shadow | Outlines and shadows |
| `--border-radius`, `--tab-radius` | – | Radii |
| `--close-hover-bg`, `--close-hover-fg` | – | Window close button on hover |
| `--font-family`, `--font-size` | – | Typography |
| `--anim-duration`, `--anim-bezier` | – | Motion |

## Window borders

The focused window's border comes from `chickenwm.conf`; applying a theme sets it to the accent:

```ini
general {
    border_size = 2
    col.active_border = rgba(242321ff)
    # or a gradient: two colors and an angle
    # col.active_border = rgba(242321ff) rgba(8a857cff) 45deg
    col.inactive_border = rgba(24232133)
}

decoration {
    rounding = 10
    active_opacity = 1.0
    inactive_opacity = 0.92
}
```

These are also in **Settings → Appearance**.

## Icon themes

Icon themes change the icons in the file explorer (and app icons in the launcher). Pick one with
`theme { icon_theme = … }`, in Settings → Appearance, or with `chickenctl theme --icons <name>`.

| Theme | Description |
|---|---|
| `chickenwm` | **Built-in.** Flat icons drawn from your palette: folders in the folder tint with glyphs for Home, Desktop, Downloads, Documents, Pictures, Music and Videos; file pages with a category glyph; drives. Redrawn every time the theme is applied, so they always match. |
| `system` | No theme: the normal Windows icons everywhere. |
| *anything else* | A folder of PNGs at `.config\chickenwm\icons\<name>\`. |

Programs and shortcuts (`.exe`, `.lnk`) always keep their own icons unless your theme has an
`app-<name>.png` for them. For anything the theme doesn't cover, ChickenWM falls back to the
Windows icon.

### Making an icon theme

1. Create `%USERPROFILE%\.config\chickenwm\icons\my-theme\`. Starting from a copy of `icons\chickenwm\`
   is easiest, but don't edit the built-in folder itself; it's regenerated.
2. Add square PNGs with transparent backgrounds. **64×64** is a good size; they're scaled for
   16 px list icons and 40 px grid icons. Every file is optional.
3. Select `my-theme` in Settings → Appearance (the list shows every folder in `icons\`), or run
   `chickenctl theme --icons my-theme`.

Icons are looked up from most to least specific:

| Item | Files tried, in order |
|---|---|
| Folder | `folder.png` |
| Special folder | `folder-home.png`, `folder-desktop.png`, `folder-downloads.png`, `folder-documents.png`, `folder-pictures.png`, `folder-music.png`, `folder-videos.png` → `folder.png` |
| Drive | `drive.png` |
| File `x.ext` | `ext-<ext>.png` (e.g. `ext-rs.png`, `ext-psd.png`) → `<category>.png` → `file.png` |
| App (explorer and launcher) | `app-<name>.png`, where `<name>` is the lower-case program or shortcut name: `app-code.png`, `app-firefox.png`, and for the shortcut "Visual Studio Code" either `app-visual studio code.png` or `app-visual-studio-code.png` |

Categories and the extensions that map to them:

| Category file | Extensions |
|---|---|
| `image.png` | png jpg jpeg gif webp bmp svg ico tif tiff heic avif raw psd |
| `video.png` | mp4 mkv webm mov avi wmv flv m4v |
| `audio.png` | mp3 flac wav ogg m4a aac opus wma |
| `archive.png` | zip 7z rar gz tgz tar xz bz2 zst cab |
| `code.png` | rs py js ts tsx jsx c h cpp hpp cs go java kt lua rb php sh ps1 bat cmd html css scss json toml yaml yml xml sql wgsl glsl hlsl conf ini |
| `text.png` | txt md log csv rtf nfo |
| `document.png` | doc docx odt xls xlsx ods ppt pptx odp epub |
| `pdf.png` | pdf |
| `executable.png` | exe msi com appx msix dll sys (`.exe` files still show their own icon) |
| `font.png` | ttf otf woff woff2 |
| `disk.png` | iso img vhd vhdx |
| `file.png` | everything else |

## Wallpaper

If `chickenwm.conf` has no `wallpaper = …` line and `.config\chickenwm\bg\` is empty,
`chickenwm-paper` renders a background from the palette: a diagonal gradient between lighter and
darker counterparts of the accent, a few soft glows, topographic contour lines, a light
vignette and fine dithering (so 4K gradients don't band). It's redrawn when the theme changes.

`assets/wallpapers/chickenwm-default-4k.png` is this image for accent `#242321` in dark mode.
Render your own:

```powershell
chickenctl render-wallpaper out.png                       # current theme, 3840×2160
chickenctl render-wallpaper out.png --width 5120 --height 1440 --accent "#242321" --mode dark
```

## CSS support

ChickenWM's own CSS engine (used by the native modules) understands:

- **Custom properties** in `:root { --name: value; }` and `var(--name)` references
- **Colors**: `#rgb`-style hex (`#rrggbb`, `#aarrggbb`), `rgb(rrggbb)`, `rgba(rrggbbaa)`,
  `rgba(r, g, b, a)` with `a` from 0–1 (or 0–255), `0xaarrggbb`
- **Lengths**: plain numbers or `px` (`12px`, `12`)
- **Durations**: `ms` and `s`
- `/* comments */`

Saving a stylesheet applies it within a few milliseconds. If a module doesn't update, run
`chickenctl reload`.

## Recipes

**Dark mode with the default accent**

```powershell
chickenctl theme --mode dark
```

**A blue accent**

```powershell
chickenctl theme "#3b82f6" --mode light
```

**Opaque bar instead of floating islands**: in `roobar.css`:

```css
:root {
    --bar-bg: rgba(242, 241, 240, 0.96);
    --bar-margin-top: 0px;
    --bar-margin-x: 0px;
    --border-radius: 0px;
}
```

**Bigger file explorer rows**: in `files.css`: `--row-height: 34px; --font-size: 15px;`

**Back to the stock Windows icons**

```powershell
chickenctl theme --icons system
```

**Undo everything**: copy the stylesheets back from your own backup, or delete one (for example
`files.css`). Deleted stylesheets are recreated on the next start, already in your current theme.
