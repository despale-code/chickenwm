<div align="center">

# 🐔 ChickenWM

**A Hyprland-style tiling window manager and shell replacement for Windows, written in Rust.**

Dynamic tiling · animations · a Hyprland-compatible config file · its own bar, launcher, file explorer,
browser, wallpaper engine and settings app · one accent color themes all of it.

</div>

---

> [!WARNING]
> **ChickenWM is vibecoded.** It was built almost entirely by prompting an AI coding assistant
> (Claude Code), with a human steering, testing and deciding what to keep. It is not written or
> reviewed line by line the way a traditional project is. It works on the author's machine and
> has tests, but expect rough edges, surprising bugs and code that a careful human would have
> written differently. Read the [Safety](#safety-and-recovery) section before replacing your shell,
> and please report what breaks.

> [!CAUTION]
> ChickenWM can replace `explorer.exe` as your Windows shell. If something goes wrong you can be left
> without a taskbar or Start menu until you recover. The recovery hotkey is
> **`Ctrl` + `Alt` + `Shift` + `Esc`**. Learn it before you start.

## Contents

- [What's in the box](#whats-in-the-box)
- [Requirements](#requirements)
- [Installation](#installation)
- [First run](#first-run)
- [Making ChickenWM your shell](#making-chickenwm-your-shell)
- [Safety and recovery](#safety-and-recovery)
- [Default keybindings](#default-keybindings)
- [Configuration](#configuration)
- [chickenctl (command line)](#chickenctl-command-line)
- [Theming and icons](#theming-and-icons)
- [The file explorer](#the-file-explorer)
- [The browser](#the-browser)
- [Building and developing](#building-and-developing)
- [Troubleshooting](#troubleshooting)
- [Roadmap](#roadmap)
- [License](#license)

## What's in the box

ChickenWM is a Cargo workspace of small programs that talk to each other over a named pipe
(`\\.\pipe\chickenwm-ipc`). The daemon, **rooster**, starts and supervises the core pieces.

| Program | What it does |
|---|---|
| `chickenwm-rooster.exe` | The daemon. Starts and watches the other modules (restarting them if they crash), hosts the IPC server and the system tray, keeps `explorer.exe` away if you ask it to, and runs the emergency failsafe. |
| `chickenwm-wm.exe` | The tiling window manager: dwindle and master layouts, workspaces 1–10, gaps, borders, rounded corners, animations, window rules. |
| `chickenwm-binds.exe` | Global hotkeys and mouse bindings from `chickenwm.conf`. |
| `chickenwm-paper.exe` | Wallpapers: images (PNG/JPEG/WebP) or live WGSL shaders, with transitions, or a stack of layers with blend modes and chroma keying. Without one, it draws a background generated from your theme. |
| `chickenwm-notify.exe` | Notifications: toasts in a screen corner (Windows app notifications, tray balloons, `chickenctl notify`), a notification center with history on `Super` + `N`, do-not-disturb and per-app blocking. |
| `roobar.exe` | The status bar: workspaces, active window title, clock, CPU, memory, layout. Flat floating "islands". |
| `chickenwm-launcher.exe` | Fuzzy app launcher with its own card design. Also a calculator, shell-command runner, window switcher and power menu. |
| `chickenwm-files.exe` | File explorer: details/icons/columns/dual-pane views, tabs, previews, real context menus, drag and drop, Recycle Bin, Explorer-compatible copy/paste. Can be made the system-wide default file manager. |
| `chickenwm-settings.exe` | Settings app in the style of macOS System Settings (searchable sidebar, grouped cards). Edits `chickenwm.conf` without destroying your formatting; has the theme gallery, CSS editor, bezier curve editor, shortcut recorder and window-rule inspector. `--page appearance` opens a page directly. |
| `chickenbrowser.exe` | A lightweight browser on Microsoft Edge WebView2 with tabs, built-in ad/tracker blocking, Chromium extensions (Proton Pass works) and memory-saving options. |
| `chickenctl.exe` | Command-line control: reload, dispatch commands, list windows/monitors, set wallpaper, change the theme, emergency restore. Modeled on Hyprland's `hyprctl`. |
| `chickenwm-dwm-hook.exe` | **Experimental, off by default.** Injects into `dwm.exe` for lower-latency effects. Can trip anti-cheat software; leave it off if you play online games. |

## Requirements

- **Windows 10 or 11, 64-bit.** Developed on Windows 11.
- **Rust 1.85 or newer** (the workspace uses the 2024 edition). Install from <https://rustup.rs>.
- **Visual Studio C++ Build Tools** (the MSVC linker and Windows SDK). The rustup installer offers to
  install them; otherwise get "Desktop development with C++" from the Visual Studio Installer.
- **Microsoft Edge WebView2 Runtime**, for `chickenbrowser` only. It ships with Windows 11 and
  current Windows 10.
- Recommended: **[Windows Terminal](https://aka.ms/terminal)** (`wt.exe`), the default terminal
  for `Super` + `Enter`, and a **[Nerd Font](https://www.nerdfonts.com/)** for bar icons.

## Installation

### Prebuilt

Download `chickenwm-<version>-windows-x64.zip` from
[Releases](https://github.com/despale-code/chickenwm/releases), extract it to a folder such as
`%LOCALAPPDATA%\ChickenWM`, and start `chickenwm-rooster.exe`. Keep all the files together in
that folder.

### From source

```powershell
git clone https://github.com/despale-code/chickenwm.git
cd chickenwm
cargo build --release
```

All programs end up in `target\release\`. You can run them from there, or copy them somewhere
permanent. They find each other by sitting in the **same folder**, so keep them together:

```powershell
$dest = "$env:LOCALAPPDATA\ChickenWM"
New-Item -ItemType Directory -Force $dest | Out-Null
Copy-Item target\release\*.exe, target\release\*.dll $dest
# Optional: make chickenctl available everywhere
[Environment]::SetEnvironmentVariable("Path", "$env:Path;$dest", "User")
```

> `WebView2Loader.dll` must stay next to `chickenbrowser.exe`; the build copies it to
> `target\release\` for you.

On first start ChickenWM creates its configuration in **`%USERPROFILE%\.config\chickenwm\`**:

```
.config\chickenwm\
├── chickenwm.conf        main configuration (Hyprland syntax)
├── roobar.css            status bar style
├── launcher.css          launcher style
├── files.css             file explorer style
├── settings.css          settings app style
├── browser.css           browser style
├── files.json            file explorer preferences (view, sort, pinned folders)
├── icons\chickenwm\      built-in icon theme (regenerated from your accent color)
└── chickenbrowser\       browser profile, extensions, settings
```

## First run

Try ChickenWM **on top of** your normal desktop before making it your shell:

```powershell
cd $env:LOCALAPPDATA\ChickenWM      # or target\release
.\chickenwm-rooster.exe
```

rooster starts the window manager, the hotkeys, the wallpaper and the bar, then runs the
`exec-once` lines from your config (by default the launcher in the background). Open a few
windows and they tile. Press `Super` + `D` for the launcher and `Super` + `I` for settings.

> [!IMPORTANT]
> The default config has `suppress_explorer = true`, which **closes `explorer.exe` while rooster
> runs**: your taskbar disappears, and roobar plus the launcher replace it. To test without that,
> set `suppress_explorer = false` in the `misc` block of `chickenwm.conf` first, or untick it in
> Settings → General.

To stop ChickenWM and bring Explorer back, press **`Ctrl` + `Alt` + `Shift` + `Esc`**.

## Making ChickenWM your shell

Once you're happy with it, make it start at login instead of Explorer. This is per user
(`HKCU`) and needs no admin rights:

```powershell
.\chickenwm-rooster.exe --install-autostart     # ChickenWM is your login shell from next login
.\chickenwm-rooster.exe --uninstall-autostart   # back to explorer.exe
```

The same switch is available in **Settings → General** ("Set ChickenWM as Default Windows Shell").

### Making the file explorer the default

**Settings → General → Default File Manager** makes `chickenwm-files` open folders, drives,
`Win` + `E` and the Start/taskbar "File Explorer" entry. Turning it off, or any emergency restore,
removes every registry key it added. Special views like Control Panel and the Recycle Bin still
open in Explorer, because only Explorer can display them.

## Safety and recovery

| If… | Do this |
|---|---|
| Anything looks wrong | **`Ctrl` + `Alt` + `Shift` + `Esc`**: restores Explorer as the shell, stops every ChickenWM process, unhides windows from other workspaces and relaunches `explorer.exe`. It works even if the hotkey hook is broken (the key state is also polled). |
| You have a terminal | `chickenctl restore-explorer` (same as above), or `chickenwm-rooster.exe --restore-explorer` |
| Windows went missing | `chickenctl restore-windows` or `chickenwm-rooster.exe --restore-windows` unhides windows left on other workspaces |
| Explorer won't come back at login | Press `Ctrl` + `Shift` + `Esc` for Task Manager → *Run new task* → `chickenwm-rooster.exe --uninstall-autostart`, then `explorer.exe` |
| Modules keep crashing | rooster restores Explorer by itself if a module crashes 3 times within 10 seconds |

## Default keybindings

`Super` is the Windows key (`$mainMod` in the config). Everything is changeable in
`chickenwm.conf` or in Settings → Keybinds.

| Keys | Action |
|---|---|
| `Super` + `Enter` | Terminal (`wt.exe`) |
| `Super` + `Q` | Close the active window |
| `Super` + `E` | File explorer |
| `Super` + `D` or `Alt` + `Space` | Launcher |
| `Super` + `I` | Settings |
| `Super` + `N` | Notification center |
| `Super` + `V` | Toggle floating |
| `Super` + `F` | Fullscreen |
| `Super` + `P` | Pseudo-tile |
| `Super` + `J` | Toggle split direction |
| `Super` + `Shift` + `S` | Screenshot (Snipping Tool) |
| `Super` + arrows | Move focus |
| `Super` + `Shift` + arrows | Move the window |
| `Super` + `Ctrl` + arrows | Resize the window |
| `Super` + `1` … `0` | Switch to workspace 1–10 |
| `Super` + `Shift` + `1` … `0` | Move the window to workspace 1–10 |
| `Super` + left-drag / right-drag | Move / resize with the mouse |
| `Ctrl` + `Alt` + `Shift` + `Esc` | **Emergency restore** |

## Configuration

Everything lives in `%USERPROFILE%\.config\chickenwm\chickenwm.conf`, using
[Hyprland's syntax](https://wiki.hyprland.org/Configuring/). Changes are picked up live; the
settings app edits the same file and keeps your comments and formatting.

```ini
$mainMod = SUPER
$term = wt.exe

monitor = , preferred, auto, 1
exec-once = chickenwm-paper.exe
exec-once = roobar.exe
exec-once = chickenwm-launcher.exe --daemon

general {
    gaps_in = 5
    gaps_out = 10
    border_size = 2
    col.active_border = rgba(242321ff)
    col.inactive_border = rgba(24232133)
    layout = dwindle            # or master
}

theme {
    accent = rgb(242321)        # one color themes everything, see docs/STYLING.md
    mode = light                # or dark
    icon_theme = chickenwm      # chickenwm (built-in), system, or your own pack
    app_modes = browser:dark    # optional per-app light/dark; others follow mode
}

decoration {
    rounding = 10
    inactive_opacity = 0.92
    blur {
        enabled = true
        style = acrylic         # acrylic | mica | blur
    }
}

animations {
    enabled = true
    bezier = myBezier, 0.05, 0.9, 0.1, 1.05
    animation = windows, 1, 5, myBezier, popin 80%
    animation = workspaces, 1, 5, myBezier, slide
}

misc {
    fps_limit = 0                       # 0 = follow each monitor's refresh rate
    vfr = true                          # sleep when nothing animates
    suppress_explorer = true            # close explorer.exe while rooster runs
    replace_winlogon_shell = false
    register_files_handler = false      # chickenwm-files as default file manager
    dwm_hook_enabled = false            # experimental; can trip anti-cheat
}

windowrulev2 = float, class:^(Taskmgr)$
bind = $mainMod, Return, exec, $term
bindm = $mainMod, mouse:272, movewindow
```

| Block | Keys |
|---|---|
| `general` | `gaps_in`, `gaps_out`, `border_size`, `col.active_border` (color, or two colors plus an angle for a gradient), `col.inactive_border`, `layout`, `allow_tearing`, `resize_on_border`, `no_titlebars` |
| `theme` | `accent`, `mode`, `icon_theme` (see [docs/STYLING.md](docs/STYLING.md)) |
| `decoration` | `rounding`, `active_opacity`, `inactive_opacity`, `fullscreen_opacity`, `drop_shadow`, `border_anim_speed`, and a `blur { enabled, size, passes, style }` block |
| `animations` | `enabled`, `first_launch_animation`, `bezier = name, x0, y0, x1, y1`, `animation = name, on, speed, curve[, style]` |
| `dwindle` | `pseudotile`, `preserve_split`, `force_split`, `default_split_ratio`, `smart_gaps` |
| `master` | `mfact`, `orientation`, `new_status`, `smart_gaps` |
| `misc` | `vfr`, `fps_limit`, `suppress_explorer`, `replace_winlogon_shell`, `register_files_handler`, `dwm_hook_enabled`, `resize_preview_classes` |
| `input` | `kb_layout`, `follow_mouse`, `sensitivity`, `repeat_rate`, `repeat_delay` |
| top level | `monitor`, `exec-once`, `bind` / `binde` / `bindr` / `bindm`, `windowrulev2`, `wallpaper`, `$variables` |

> In `chickenwm.conf`, `#` starts a comment, so write colors as `rgb(242321)` or
> `rgba(242321ff)`, not `#242321`.

**Window rules** use Hyprland's `windowrulev2` syntax: `windowrulev2 = <effect>, <matcher>`.
Effects include `float`, `opacity 0.9 0.8`, `workspace 3`, `size 800 600`, `center` and `nofocus`;
matchers are `class:^(regex)$` or `title:^(regex)$`. Settings → Window Rules has a crosshair that
reads the class and title of any window you click.

**Dispatchers** (for `bind` and `chickenctl dispatch`):

| Dispatcher | Argument |
|---|---|
| `exec` | command line |
| `killactive` | – |
| `togglefloating` | – |
| `fullscreen` | `0` (fullscreen) or a mode number |
| `pseudo`, `togglesplit` | – |
| `movefocus`, `movewindow` | `l` `r` `u` `d` (`movewindow` with no argument = mouse drag) |
| `resizewindow` | – (mouse drag) |
| `workspace` | `1`–`10`, `+1` / `-1` (relative), `previous` |
| `movetoworkspace`, `movetoworkspacesilent` | `1`–`10` |
| `resizeactive`, `moveactive` | `dx dy` in pixels, e.g. `-40 0` |
| `layout` | `dwindle` or `master` |
| `launcher`, `settings`, `notifications` | – (`notifications` toggles the notification center) |
| `files` | optional folder to open |
| `exit` | – (quit ChickenWM) |

## chickenctl (command line)

```text
chickenctl reload                          reload chickenwm.conf and all CSS
chickenctl dispatch <dispatcher> [arg]     e.g. chickenctl dispatch workspace 3
chickenctl clients | monitors | workspaces list windows / monitors / workspaces
chickenctl status                          daemon and module status
chickenctl fps <n>                         frame rate target (0 = monitor refresh rate)
chickenctl launcher                        open the launcher
chickenctl wallpaper <path> [-m MONITOR] [-t fade|wipe|outer|grow]
chickenctl theme ["#rrggbb"] [--mode light|dark] [--preset NAME|none] [--icons NAME]
chickenctl notify "title" ["body"] [--app NAME] [--timeout SECS] [--critical]
chickenctl notifications                   toggle the notification center
chickenctl render-wallpaper <out.png>      generated wallpaper for any theme/size
chickenctl restore-explorer                emergency restore
chickenctl restore-windows                 unhide windows from other workspaces
```

Add `-j` to any command for JSON output.

## Theming and icons

One theme colors the whole system: bar, launcher, file explorer, browser, settings, notifications,
window borders and icons. The default ChickenWM theme is generated from one accent color,
**`#242321`** (a warm charcoal), in **light** mode: surfaces are light tints of the same hue, and
text and highlights use the accent itself. **Dark** mode uses the accent as the background, with
its lighter counterpart for highlights.

Or pick one of 10 popular color schemes, each with a light and a dark variant: **Dracula**
(Alucard / Dracula), **Catppuccin** (Latte / Mocha), **Nord**, **Gruvbox**, **Tokyo Night**
(Day / Night), **Rosé Pine** (Dawn / Rosé Pine), **Solarized**, **One** (One Light / One Dark),
**Everforest** and **Kanagawa** (Lotus / Wave).

- **Settings → Appearance**: a gallery of all themes, light/dark, accent color (for the ChickenWM
  theme) and icon theme. Changes apply to the whole system right away.
- **Command line**: `chickenctl theme --preset catppuccin --mode dark`, `chickenctl theme "#3b5bdb"`
  (back to a generated theme), or one app: `chickenctl theme --app browser --app-mode dark`.
- **In the config**: `theme { preset = nord }` (`none` = generated from `accent`).
- **Per-app light/dark**: every app can follow the system mode or be light or dark on its own;
  the same option is in each app's settings and in one table in ChickenWM Settings, all stored as
  `app_modes` by one master color provider.
- **Icon themes**: `chickenwm` (built-in, redrawn from your accent), `system` (Windows icons), or
  your own PNG pack in `.config\chickenwm\icons\<name>\`.

### Wallpaper

With no wallpaper configured, `chickenwm-paper` draws a generated background in your theme's colors:
a soft gradient with contour lines that follows your accent and light/dark mode. A ready-made 4K
copy (3840×2160, `#242321` in dark mode) is included as
[`assets/wallpapers/chickenwm-default-4k.png`](assets/wallpapers/chickenwm-default-4k.png), and you
can render it for any theme and size:

```powershell
chickenctl render-wallpaper my-wall.png --width 2560 --height 1440 --accent "#3b82f6" --mode dark
```

To use your own image, drop it in `.config\chickenwm\bg\`, add `wallpaper = , C:\path\image.png, fade`
to `chickenwm.conf`, or run `chickenctl wallpaper C:\path\image.png`. Images and live `.wgsl`
shaders both work.

You can also stack **layers**, bottom to top, each with its own fit, opacity, blend mode and an
optional **chroma key** that turns a color (a green screen, say) transparent:

```ini
paper {
    layer = image, ~/bg/sky.png
    layer = image, ~/bg/character.png, key = rgb(00ff00), tolerance = 0.25, softness = 0.1
    layer = color, rgba(0, 0, 0, 0.25), blend = multiply
}
```

All the options are in [docs/STYLING.md](docs/STYLING.md#wallpaper-layers).

Everything about styling (every CSS variable of every module, how the palette is derived, and how
to make icon packs) is in **[docs/STYLING.md](docs/STYLING.md)**.

## Notifications

`chickenwm-notify` shows notifications as cards in a corner of the screen and keeps a history.

- **Where they come from**: Windows app notifications (Discord, Teams, browsers, Windows itself),
  read through Windows' notification platform while Explorer isn't running; legacy tray-icon
  balloons; and `chickenctl notify`. The first time, Windows may ask to let apps access your
  notifications (Settings → Privacy & security → Notifications).
- **On screen**: hover to pause the timers, click to dismiss, right-click for *Dismiss all* and
  *Block notifications from <app>*. Critical notifications stay until dismissed.
- **Notification center** (`Super` + `N`): past notifications, *Clear all*, *Do not disturb* (only
  critical ones pop up), and your blocked apps (click one to unblock).
- **Config**:

```ini
notifications {
    position = top-right        # top-left | bottom-right | bottom-left | top-center | bottom-center
    timeout = 6                 # seconds, 0 = until dismissed
    max_visible = 4
    margin_x = 16
    margin_y = 12
    windows_toasts = true
    blocked = Discord, Steam    # apps whose notifications are dropped (also set from the menus)
}
```

Styling (colors, size, radius, motion) is in `notifications.css`, see [docs/STYLING.md](docs/STYLING.md#notificationscss).
History is kept in `.config\chickenwm\notifications.json`.

## The file explorer

`chickenwm-files` (`Super` + `E`):

- **Views**: Details (sortable columns), Icons, Columns (parent / current / preview), Dual pane.
  Plus tabs, breadcrumbs (click to navigate, or type a path with `Ctrl` + `L`), search, and a
  status bar with selection size and free disk space.
- **Preview**: a card that fills the preview pane or column: images fitted and centered (with a
  checkerboard behind transparency), code with syntax highlighting and line numbers for ~30
  languages, wrapped text, and folder contents.
- **Right-click menus**: Open, Open with, Run as administrator, Extract, Cut/Copy/Paste, Copy as
  path, Rename, Delete (Recycle Bin) or Delete permanently, Compress to ZIP, Create shortcut, Pin to
  Quick access, Open in Terminal, Properties, and **Show more options** for the full Windows menu
  (7-Zip, Git, Send to…).
- **Drag and drop**: onto folders, the sidebar, tabs or breadcrumbs. Same drive moves, other drive
  copies; `Ctrl` = copy, `Shift` = move, `Alt` = shortcut. Right-drag asks. Drag out of the window
  to drop into other apps; drop files in from other apps.
- **Windows integration**: copy, move and delete go through the Windows shell (Recycle Bin, conflict
  dialogs, undo), and the clipboard is shared with Explorer.
- **Keyboard**: arrows, `Enter`, `Backspace`, `F2`, `Del` / `Shift` + `Del`, `Ctrl` + `C`/`X`/`V`/`A`,
  `Ctrl` + `Shift` + `N`, `Alt` + arrows, `Ctrl` + `T`/`W`/`Tab`, plus vim keys (`j` `k` `h` `l`,
  `gg`/`G`, `yy`, `dd`, `p`, `cw`, `/`, `x`).

## The browser

`chickenbrowser` is a small browser on Edge WebView2:

- Tabs, address bar search (DuckDuckGo by default), and built-in EasyList/EasyPrivacy ad blocking.
- **Extensions**: unpack a Chromium extension into `.config\chickenwm\chickenbrowser\extensions\<name>\`
  and restart the browser. Proton Pass is supported: known store extensions get their official
  extension ID, so their web sign-in works.
- **Memory & performance** (`about:settings`): presets (Balanced / Memory saver / Maximum
  savings), background-tab suspension and unloading, renderer process limits, and live memory usage.
- **Appearance** (`about:settings`): *Dark mode* (same accent palette) or *Follow ChickenWM theme*,
  *Match website colors* (the toolbar follows each page's background), vertical tabs (collapsible,
  or *auto-hide*: tucks into a thin strip and slides out over the page when you hover the left
  edge), tab icons, and animations (the browser's own switch, independent of Windows' animation
  setting).
- **When a site breaks**: click the shield in the address bar to turn ad blocking off for that
  site. Edge's tracking prevention defaults to *Basic* (changeable under Privacy), since stricter
  levels can break sign-in pages. Pop-up windows that sites open with `window.open` (sign-in,
  payment, exams) open as real pop-ups so they keep working.

## Building and developing

```powershell
cargo build                 # debug build of everything
cargo test --workspace      # run the tests
cargo run -p chickenwm-files -- C:\some\folder
cargo run -p chickenwm-rooster -- --no-children   # daemon only, start modules yourself
```

| Folder | Crate |
|---|---|
| `chickenwm-core/` | Shared library: config parser and lossless writer, CSS engine, theme and icon engine, IPC, failsafe, monitors, frame scheduler |
| `chickenwm-rooster/` | Daemon / supervisor |
| `chickenwm-wm/` | Window manager |
| `chickenwm-binds/` | Hotkeys |
| `chickenwm-paper/` | Wallpaper |
| `roobar/` | Status bar |
| `chickenwm-launcher/` | Launcher |
| `chickenwm-files/` | File explorer |
| `chickenwm-settings/` | Settings |
| `chickenbrowser/` | Browser |
| `chickenctl/` | CLI |
| `chickenwm-dwm-hook/` | Experimental DWM injection |

Logs go to stdout; run a module from a terminal to see them. Set `RUST_LOG=debug` for more detail.

## Troubleshooting

- **No taskbar after starting**: expected with `suppress_explorer = true`. roobar is your bar and
  `Super` + `D` opens the launcher. To get Explorer back, press `Ctrl` + `Alt` + `Shift` + `Esc`.
- **A window refuses to tile**: some apps (games, installers, UWP popups) are skipped on purpose.
  Force it with `windowrulev2 = float, class:^(...)$`, or pick it with the crosshair in Settings →
  Window Rules.
- **Task Manager or another admin window floats**: Windows doesn't let a normal program move the
  windows of a program running as administrator. ChickenWM leaves those windows floating and shows
  a notification. Run rooster as administrator to tile them too.
- **Windows on other workspaces**: they are moved far off-screen ("parked") instead of hidden, so
  Task Manager still groups them under their app. Exiting ChickenWM puts them all back.
- **The browser doesn't start**: install the
  [WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) and keep
  `WebView2Loader.dll` next to `chickenbrowser.exe`.
- **Anti-cheat warnings**: make sure `dwm_hook_enabled = false` (Settings → Display → Anti-Cheat Safe
  Switch).
- **Theme changes don't show**: every module hot-reloads its CSS; if one doesn't, run
  `chickenctl reload` or restart that program.

## Roadmap

See [Todo.md](Todo.md) for the full specification and progress checklist. A documentation website
on GitHub Pages is planned.

## License

[MIT](LICENSE). ChickenWM is not affiliated with Hyprland; it borrows its config syntax and
workflow because they're great.
