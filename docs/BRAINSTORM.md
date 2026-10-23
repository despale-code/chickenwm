# Architecture Brainstorm: `coop` & `chickenscope`

> Architectural specification for decoupling ChickenWM from DWM/Explorer via a custom Vulkan compositor (**`coop`**), a low-latency game passthrough engine (**`chickenscope`**), and a stripped Windows service footprint.

---

## 1. High-Level System Overview

```
                      [ Winlogon Shell: chickenwm-rooster ]
                                      │
               ┌──────────────────────┴──────────────────────┐
               ▼                                             ▼
     [ Desktop Mode: coop ]                      [ Game Mode: chickenscope ]
   • Pure User-Mode Vulkan Compositor          • Native DirectFlip Scanout (0ms lag)
   • Zero-copy GPU Texture Bridge (WGC)        • 100% Raw Hardware Input (Anti-Cheat Safe)
   • Decoupled Quad Animation Engine           • G-Sync / FreeSync & HDR Native
   • NO Drivers Required (100% Anti-Cheat Safe)• In-Game Transparent Vulkan HUD Overlay
   • Custom DPMS Sleep & Lock Screen           • Zero Compositor GPU Overhead
```

---

## 2. `coop`: Pure User-Mode Vulkan Viewspace Compositor

`coop` replaces DWM's presentation layer for desktop productivity apps, hosting windows on a freeform GPU canvas.

### 2.1 The "Zero-Driver" Breakthrough
- **No Custom Drivers Needed**: We avoid Indirect Display Drivers (IDD) and kernel drivers entirely.
  - No Microsoft WHQL driver certification or expensive EV certificates needed ($0 cost).
  - No `testsigning` required &rarr; **100% compatible with Secure Boot and kernel anti-cheats (CS2, Valorant/Vanguard, Fortnite/EAC, BattlEye)**.
  - Zero risk of Blue Screens of Death (BSOD); crashes are pure user-mode process panics caught by `chickenwm-rooster`.
- **Window Positioning Strategy**: Real Win32 windows exist directly on the physical monitor, sized and placed silently **behind** `coop`'s top-most transparent Vulkan window. Because they reside on an active physical display, Windows renders them at full GPU acceleration (144Hz–240Hz+) without sleep throttling.

### 2.2 Surface Capture & Zero-Copy GPU Bridge
- **Capture Source**: `Windows.Graphics.Capture` (WGC) API targetting app `HWND`s.
  - Flags: `IsBorderRequired = false` (strips yellow privacy box), `IsCursorCaptureEnabled = false`.
- **Zero-Copy VRAM Handoff**:
  - WGC outputs Direct3D 11 / DXGI surfaces (`ID3D11Texture2D`).
  - Surface exported as a Windows NT shared handle (`IDXGIResource1::CreateSharedHandle`).
  - Vulkan imports the shared handle directly into a `VkImage` using `VK_KHR_external_memory_win32`.
  - **Zero CPU copying and zero RAM transfers**: Texture remains in VRAM throughout.

### 2.3 Cross-API Synchronization
- Direct3D 11/12 and Vulkan access the shared texture concurrently.
- **Mechanism**: Win32 Keyed Mutex (`IDXGIKeyedMutex`) or Shared Fences (`ID3D11Fence` / `VkSemaphore` via `VK_KHR_external_semaphore_win32`).
- WGC signals on new frame arrival &rarr; Vulkan acquires mutex &rarr; Fragment shader renders &rarr; Vulkan releases mutex. Prevents tearing, micro-stutter, and GPU driver crashes.

### 2.4 Decoupled Quad Animation Engine & Resizing
- **Visuals in Vulkan**: The window quad animates at the monitor's native refresh rate (144Hz, 240Hz, 360Hz) with zero stutter.
- **Spring-Damper Physics**: Custom Hooke's Law simulations in Rust drive natural momentum, organic bounce, and rubber-banding.
- **Wobbly / Jelly Windows**: 16x16 vertex mesh displacement calculated directly in a Vulkan vertex shader.
- **Interactive Live Resizing**: Instead of ugly raw stretching or waiting for slow apps (Electron/VS Code) to re-render, `coop` applies dynamic elastic stretch shaders and smooth spring curves during resize drag, snapping to the final native resolution cleanly once the drag settles.
- **Lazy Alignment**: When the animation settles, `coop` issues a single, silent Win32 `SetWindowPos` to snap the real window underneath for physical hit-testing.

### 2.5 Multi-Monitor & Framerate Strategy
- Default to the highest available display refresh rate (e.g. 240Hz) with configurable overrides in `chickenwm.conf`.
- Each physical monitor operates an independent Vulkan swapchain to guarantee mixed-refresh-rate setups (e.g. 60Hz secondary + 240Hz primary) never throttle the high-speed display.

### 2.6 Hardware Mouse Cursor & High-Polling Mice (1000Hz–8000Hz)
- **Hardware Cursor Plane**: We utilize the GPU's native Win32 hardware cursor plane (`WM_SETCURSOR` / `SetCursor`) for 1000Hz+ instantaneous cursor response (no 16ms software shader lag).
- **High-Polling Decoupling**: For 1000Hz–8000Hz gaming mice, cursor movement coordinates are stored in atomic variables. Window hit-testing is evaluated at the display frame rate (144Hz–240Hz), preventing CPU core saturation.

### 2.7 Premultiplied Alpha Blending
- WGC textures output premultiplied alpha.
- Vulkan blend state is explicitly configured:
  ```rust
  src_color_blend_factor = vk::BlendFactor::ONE;
  dst_color_blend_factor = vk::BlendFactor::ONE_MINUS_SRC_ALPHA;
  ```
- Guarantees translucent window edges, drop shadows, and popup dialogs blend with zero dark/black halos.

### 2.8 Stealing Windows Sleep, Lock & Hangs
Instead of relying on Windows native lock screens and sleep timeouts:
- **Steal `Win + L`**: Disable Windows kernel lock workstation via registry:
  ```ini
  [HKCU\Software\Microsoft\Windows\CurrentVersion\Policies\System]
  "DisableLockWorkstation"=dword:00000001
  ```
  `chickenwm-binds` now intercepts `Win + L` directly.
- **Custom DPMS Sleep**: When idle or requested, `coop` shuts down monitor signal directly via Win32 DPMS:
  ```rust
  SendMessageW(HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER, 2); // Power off displays
  ```
- **Instant Wake on Input**: Low-level hooks listen for any mouse motion or keypress, instantly waking displays (`SC_MONITORPOWER, -1`) and restoring the Vulkan viewspace without Windows lock screen delays.
- **Custom Lock Screen**: ChickenWM renders its own custom aesthetic lock screen directly in Vulkan (PAM/Windows credentials auth or simple PIN/password).

### 2.9 UAC & Elevation as Standard Windows
- Running `coop` as Administrator is rejected: it breaks UIPI, prevents drag-and-drop, and creates security risks.
- **The Solution**: Set Windows Registry policy:
  ```ini
  [HKEY_LOCAL_MACHINE\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System]
  "PromptOnSecureDesktop"=dword:00000000
  ```
- **Result**: UAC prompts spawn as standard Win32 windows on the `Default` desktop. `coop` captures and composites the elevation dialog like any other window with full blur shaders and zero black screen locks.

### 2.10 Popups, Menus & File Pickers
- Win32 menus (`#32768`), dropdowns, tooltips, and file dialogs spawn as independent `WS_POPUP` windows at raw screen coordinates.
- **Hierarchy Tracker**:
  - Hooks `EVENT_OBJECT_CREATE` and `EVENT_SYSTEM_MENUPOPUPSTART`.
  - Resolves root parent via `GetAncestor(hwnd, GA_ROOTOWNER)`.
  - Attaches popup texture as a relative floating sub-quad above the parent window in the Vulkan scene.

### 2.11 Built-in Shader Wallpaper & Alt-Tab Exposé
- **Native Vulkan Wallpaper**: `coop` renders wallpapers directly as the bottom-most Vulkan layer. Supports high-res static images, animated video loops, and interactive GLSL/Vulkan compute shaders (Shadertoy/Lively style) at zero extra process cost.
- **Exposé / Alt-Tab**: Native 3D window switcher rendering all open window quads in a fluid bird's-eye view.

---

## 3. `chickenscope`: Low-Latency Game Mode & Overlay

`chickenscope` handles games by completely bypassing the compositor layer, eliminating anti-cheat collisions, input lag, and frame pacing issues.

### 3.1 Why Games Must Bypass Compositing
1. **Kernel Anti-Cheat (Vanguard, EAC, BattlEye, FACEIT)**:
   - Synthetic input injection (`SendInput`, `PostMessage`) is flagged as botting/macroing.
   - Games demand unadulterated OS hardware input packets.
2. **Raw Input & 3D Camera Controls**:
   - First-person games use unconstrained raw mouse deltas (`GetRawInputData`). Non-foreground windows lose raw input or spin erratically.
3. **Display Scanout & VRR**:
   - Games need DirectFlip (`DXGI_SWAP_EFFECT_FLIP_DISCARD`) to scan out directly to the display, unlocking G-Sync/FreeSync, Nvidia Reflex/AMD Anti-Lag, and sub-1ms response times.

### 3.2 Game Mode Execution State
When a game takes focus:
- **Compositor Sleep**: `coop` pauses its render loop (`vkDeviceWaitIdle` or 0 FPS) and yields display ownership. Consumes **0% GPU** during gameplay.
- **Input Unhooking**: All window management chords are disabled except for a single master escape key.
- **Native OS Focus**: Game holds true OS foreground status (`SetForegroundWindow`).
- **Hands-Off Subclassing**: No `borderless_resize_subclass`, no `WM_NCCALCSIZE` modifications, no DWM acrylic attributes. Game controls resolution and state natively.
- **MMCSS Scheduling**: Registers the game process under Windows Multimedia Class Scheduler Service (`AvSetMmThreadCharacteristicsW("Games")`) and elevates thread priority.

### 3.3 In-Game HUD Overlay
- When pressing the overlay chord (e.g. `Super + Tab` / `Shift + Tab`):
- `chickenscope` renders a borderless, transparent Vulkan overlay directly above the game without minimizing or altering the game's fullscreen state.
- Lets user access Hen Browser, Roobar, notifications, or chat, then instantly resume gaming with zero Alt-Tab delay.

---

## 4. Windows Services & Shell Stripping Matrix

Because `explorer.exe` is completely replaced as the shell, the operating system can be aggressively stripped.

### 4.1 Winlogon Shell Replacement
Set `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon`:
```ini
Shell = "%LOCALAPPDATA%\Programs\ChickenWM\chickenwm-rooster.exe"
```
- Completely halts `explorer.exe`, taskbar, desktop icons, and shell hooks from initializing.
- Cuts Windows boot time in half and saves 300MB–700MB of baseline RAM.

### 4.2 Services Safe to Disable
| Service / Process | Canonical Name | Why Safe to Disable |
| :--- | :--- | :--- |
| **Windows Search** | `WSearch` | Replaced by `chickenwm-launcher`. Cuts continuous background disk I/O. |
| **Start Menu Host** | `StartMenuExperienceHost.exe` | No Windows Start Menu used. |
| **Taskbar Search** | `SearchHost.exe` | Pure telemetry & web search bloat. |
| **Windows Widgets** | `Widgets.exe` / Web Experience | Roobar handles widgets and status. |
| **Xbox Game Bar** | `BcastDVRUserService`, `GameBar.exe` | Replaced by `chickenscope` and `chickenwm-shot`. |
| **Windows Push Toasts**| `WpnUserService`, `WpnService` | Replaced by `chickenwm-notify`. |
| **Touch / Pen Keyboard**| `TabletInputService` | Redundant for keyboard-first tiling WM. |
| **Telemetry** | `DiagTrack` | Unnecessary telemetry tracking. |
| **Superfetch** | `SysMain` | Redundant RAM pre-caching on NVMe SSDs. |
| **Diagnostics** | `DPS` (Diagnostic Policy) | Disabling lowers DPC latency spikes. |

### 4.3 Critical Services: DO NOT TOUCH
| Component | Function | Consequence if Disabled |
| :--- | :--- | :--- |
| **`dwm.exe`** | Kernel Display Arbiter | **Black screen**. DWM manages WDDM presentation queues for DirectX & Vulkan. |
| **`CryptSvc`** | Cryptographic Services | **Anti-Cheat Failure**. Vanguard, EAC, and BattlEye fail driver/binary verification. |
| **`Winmgmt`** | WMI Repository | Anti-cheat integrity checks fail; hardware monitoring breaks. |
| **`EventLog`** | Windows Event Log | Anti-cheat (EAC) explicitly blocks game launch if EventLog is stopped. |
| **`Audiosrv`** | Windows Audio Service | Total audio loss across games and browsers. |
| **`RpcSs` / `DcomLaunch`** | Core Kernel IPC | System instability / BSOD. |

---

## 5. Performance Benchmark Projections

| Metric | Stock Windows 11 | ChickenWM + `coop` + `chickenscope` |
| :--- | :--- | :--- |
| **Idle Process Count** | 160 – 220 processes | **65 – 85 processes** |
| **Idle Memory Footprint** | 3.5 GB – 4.5 GB | **1.4 GB – 1.8 GB** |
| **Gaming Input Latency** | DWM composition buffer (~8-16ms) | **0 ms added (Hardware DirectFlip)** |
| **Game Anti-Cheat Status** | Native | **100% Compliant (Zero drivers, zero synthetic input)** |
| **Desktop Visual Freedom** | Restricted to DWM window frames | **Unrestricted (Vulkan shaders, spring physics, 3D viewspaces)** |
