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
   • Vulkan 2D/3D Viewspace Canvas             • Native DirectFlip Scanout (0ms lag)
   • Zero-copy GPU Texture Bridge              • 100% Raw Hardware Input (Anti-Cheat Safe)
   • Custom Shaders, Blurs, Fluid Transforms   • G-Sync / FreeSync & HDR Native
   • Win32 Virtual Display (IDD) Parking       • Transparent In-Game Vulkan HUD Overlay
```

---

## 2. `coop`: Vulkan Viewspace Compositor

`coop` replaces DWM's presentation layer for desktop productivity apps, hosting windows on a freeform GPU canvas.

### 2.1 Surface Capture & Zero-Copy GPU Bridge
- **Capture Source**: `Windows.Graphics.Capture` (WGC) API targetting app `HWND`s.
  - Flags: `IsBorderRequired = false` (strips yellow privacy box), `IsCursorCaptureEnabled = false`.
- **Zero-Copy VRAM Handoff**:
  - WGC outputs Direct3D 11 / DXGI surfaces (`ID3D11Texture2D`).
  - Surface exported as a Windows NT shared handle (`IDXGIResource1::CreateSharedHandle`).
  - Vulkan imports the shared handle directly into a `VkImage` using `VK_KHR_external_memory_win32`.
  - **Zero CPU copying and zero RAM transfers**: Texture remains in VRAM throughout.

### 2.2 Cross-API Synchronization
- Direct3D 11/12 and Vulkan access the shared texture concurrently.
- **Mechanism**: Win32 Keyed Mutex (`IDXGIKeyedMutex`) or Shared Fences (`ID3D11Fence` / `VkSemaphore` via `VK_KHR_external_semaphore_win32`).
- WGC signals on new frame arrival &rarr; Vulkan acquires mutex &rarr; Fragment shader renders &rarr; Vulkan releases mutex. Prevents tearing, micro-stutter, and GPU driver crashes.

### 2.3 Window Off-Screen Parking (The IDD Pattern)
- **Problem**: Minimized, cloaked (`DWMWA_CLOAK`), or far off-screen (`-32000, -32000`) windows are throttled by Windows to 0 FPS.
- **Solution**: A lightweight headless **Indirect Display Driver (IDD)** creating a virtual high-resolution monitor (e.g. 4K/8K virtual display).
  - Real Win32 windows exist at native resolution on the virtual display.
  - Windows OS renders them at full GPU acceleration.
  - `coop` captures their frames and projects them onto the physical monitor in Vulkan.

### 2.4 Popups, Context Menus & File Pickers
- Win32 menus (`#32768`), dropdowns, tooltips, and file dialogs spawn as independent `WS_POPUP` windows at raw screen coordinates.
- **Hierarchy Tracker**:
  - Hooks `EVENT_OBJECT_CREATE` and `EVENT_SYSTEM_MENUPOPUPSTART`.
  - Resolves root parent via `GetAncestor(hwnd, GA_ROOTOWNER)`.
  - Attaches popup texture as a relative floating sub-quad above the parent window in the Vulkan scene.

### 2.5 Text Sharpness & DPI Scaling
- Subpixel ClearType degrades if scaled arbitrarily on a fractional grid.
- Windows on the virtual display are rendered at 150%–200% DPI.
- `coop`'s Vulkan sampling shader uses high-order reconstruction (Bicubic / Catmull-Rom filtering) for clean downsampling.

### 2.6 Occlusion & Resource Culling
- Only windows on the active workspace inside the viewport retain active capture sessions.
- Inactive/occluded workspaces drop to static texture caches or throttled 1 FPS capture.

---

## 3. `chickenscope`: Low-Latency Game Mode & Overlay

`chickenscope` handles games by completely bypassing the compositor layer, eliminating anti-cheat collisions, input lag, and frame pacing issues.

### 3.1 Why Games Must Bypass Compositing
1. **Kernel Anti-Cheat (Vanguard, EAC, BattlEye, Ricochet)**:
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
| **Game Anti-Cheat Status** | Native | **100% Compliant (Zero synthetic input)** |
| **Desktop Visual Freedom** | Restricted to DWM window frames | **Unrestricted (Custom shaders, 3D viewspaces)** |
