# Implementation Roadmap: `coop` & `chickenscope`

---

### Phase 1: Windows System Configuration & Shell Prep
- [x] **1.1 UAC Elevation Policy**
  - [x] Set `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System\PromptOnSecureDesktop = 0`.
  - [x] Verify `consent.exe` spawns on `Default` desktop as standard composite window.
- [x] **1.2 Lock Workstation Interception**
  - [x] Set `HKCU\Software\Microsoft\Windows\CurrentVersion\Policies\System\DisableLockWorkstation = 1`.
  - [x] Verify `Win + L` is not swallowed by `win32k.sys` and can be trapped by low-level keyboard hook.
- [x] **1.3 Winlogon Shell Replacement Scripting**
  - [x] Script registry switch for `HKLM\...\Winlogon\Shell` to `chickenwm-rooster.exe`.
  - [x] Add rollback failsafe hook in installer (`scripts/configure-system.ps1`).
- [x] **1.4 Service Debloating Automation**
  - [x] Disable redundant services: `WSearch`, `BcastDVRUserService`, `WpnService`, `TabletInputService`, `DiagTrack`, `SysMain`, `DPS`.
  - [x] Ensure non-negotiable services remain active: `dwm.exe`, `CryptSvc`, `Winmgmt`, `EventLog`, `Audiosrv`, `RpcSs`.

---

### Phase 2: `coop` Core — WGC to Vulkan Zero-Copy Bridge
- [x] **2.1 Windows Graphics Capture (WGC) Setup**
  - [x] Implement `GraphicsCaptureItem` initialization for target `HWND`s via WinRT/C++/Rust (`IGraphicsCaptureItemInterop::CreateForWindow`).
  - [x] Configure `GraphicsCaptureSession`: `IsBorderRequired = false`, `IsCursorCaptureEnabled = false`.
  - [x] Create `Direct3D11CaptureFramePool` in `B8G8R8A8_UNORM`.
- [x] **2.2 DXGI NT Handle Export**
  - [x] Extract `ID3D11Texture2D` from captured frame.
  - [x] Query `IDXGIResource1`.
  - [x] Call `CreateSharedHandle` with `GENERIC_ALL` and security attributes to produce an NT `HANDLE`.
- [x] **2.3 Vulkan External Memory Import**
  - [x] Enable Vulkan extensions: `VK_KHR_external_memory`, `VK_KHR_external_memory_win32`.
  - [x] Query memory requirements for imported handle (`VkMemoryGetWin32HandlePropertiesKHR`).
  - [x] Bind imported `VkDeviceMemory` to target `VkImage` (`VK_EXTERNAL_MEMORY_HANDLE_TYPE_D3D11_TEXTURE_BIT`).
- [x] **2.4 Cross-API Synchronization**
  - [x] Initialize `IDXGIKeyedMutex` or `ID3D11Fence` on capture swapchain.
  - [x] Import fence into Vulkan using `VK_KHR_external_semaphore_win32`.
  - [x] Implement acquire/release cycle: WGC frame arrived &rarr; DXGI release &rarr; Vulkan wait &rarr; render pass &rarr; Vulkan release &rarr; DXGI acquire.

---

### Phase 3: `coop` Vulkan Rendering Pipeline & Animation Engine
- [x] **3.1 Display Swapchain & Multi-Monitor Support**
  - [x] Enumerate physical displays (`EnumDisplayMonitors`).
  - [x] Spawn dedicated, borderless top-most `WS_EX_TOPMOST` Vulkan window per monitor.
  - [x] Initialize independent `VkSwapchainKHR` per monitor to maintain native maximum refresh rates (e.g., 240Hz independent of secondary 60Hz).
- [x] **3.2 Premultiplied Alpha Blending Pipeline**
  - [x] Set pipeline blend state:
    - `src_color_blend_factor = VK_BLEND_FACTOR_ONE`
    - `dst_color_blend_factor = VK_BLEND_FACTOR_ONE_MINUS_SRC_ALPHA`
    - `src_alpha_blend_factor = VK_BLEND_FACTOR_ONE`
    - `dst_alpha_blend_factor = VK_BLEND_FACTOR_ONE_MINUS_SRC_ALPHA`
  - [x] Verify zero black halo artifacts on translucent window edges.
- [x] **3.3 Decoupled Quad Engine**
  - [x] Implement spring-damper physics model in Rust (`F = -k*x - c*v`).
  - [x] Generate 16x16 vertex mesh quads per window for non-rigid deformation (wobbly/jelly windows).
  - [x] Vertex shader: apply model-view-projection (MVP) matrix + local vertex displacement based on velocity.
  - [x] Fragment shader: Bicubic / Catmull-Rom downsampling filter for 150–200% virtual DPI sharpness.
- [x] **3.4 Live Interactive Resize**
  - [x] On resize drag: apply anchor-and-pad vertex stretch shader on the existing texture.
  - [x] Send throttled (60Hz) Win32 size updates to target `HWND`.
  - [x] Update texture UV coordinates dynamically upon arrival of matching frame dimension.
  - [x] Lazy alignment: execute single `SetWindowPos` for physical hit-box on spring rest.
- [x] **3.5 Wallpaper & Exposé Layer**
  - [x] Render background layer: support high-res static images and custom GLSL/Vulkan fragment shaders.
  - [x] Implement Exposé / 3D bird's-eye window layout switcher triggered via IPC/hotkey.

---

### Phase 4: `coop` Window Management & Hierarchy Tracking
- [x] **4.1 Physical Window Management**
  - [x] Position real Win32 windows silently behind the Vulkan canvas on the active display.
  - [x] Strip non-client borders and decorations from managed windows (`chickenwm-core` sys helpers).
  - [x] Occlusion culling: pause/throttle capture sessions on inactive workspaces to 0–1 FPS.
- [x] **4.2 Popup & Menu Hierarchy Tracker**
  - [x] Listen to WinEvents: `EVENT_OBJECT_CREATE`, `EVENT_SYSTEM_MENUPOPUPSTART`, `EVENT_OBJECT_DESTROY`.
  - [x] Filter window class `#32768` (menus), combo boxes, and tooltips.
  - [x] Resolve root owner via `GetAncestor(hwnd, GA_ROOTOWNER)`.
  - [x] Position popup quad relative to parent window quad in Vulkan coordinate space.

---

### Phase 5: Input & Power Subsystems
- [x] **5.1 Hardware Cursor Plane Integration**
  - [x] Leave OS hardware cursor plane enabled (no software shader cursor).
  - [x] On pointer move: hit-test quad tree in `coop`.
  - [x] Query underlying target's cursor shape (`WM_SETCURSOR` / `GetCursorInfo`) and call `SetCursor()`.
- [x] **5.2 High-Polling Mouse Optimization (1000–8000Hz)**
  - [x] Receive mouse input via Raw Input (`RegisterRawInputDevices`).
  - [x] Store coordinates in atomic integers (`AtomicI32`).
  - [x] Decouple hit-test evaluation to run synchronously with monitor VSync (144–240Hz).
- [x] **5.3 Custom DPMS Sleep & Wake**
  - [x] Detect system idle timeout.
  - [x] Sleep: send `SendMessageW(HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER, 2)`.
  - [x] Drop Vulkan rendering to 0 FPS (`vkDeviceWaitIdle`).
  - [x] Wake: low-level input hook catches any key/mouse &rarr; sends `SC_MONITORPOWER, -1` &rarr; restores swapchain.
- [x] **5.4 Custom Lock Screen**
  - [x] Intercept `Win + L`.
  - [x] Render fullscreen lock canvas in Vulkan (clock, background blur, auth prompt).

---

### Phase 6: `chickenscope` — Game Mode & DirectFlip Passthrough
- [x] **6.1 Game Detection Engine**
  - [x] Heuristic 1: Window rect equals physical monitor bounds (`GetWindowRect == MonitorRect`) without `WS_CAPTION`.
  - [x] Heuristic 2: Config matchers in `chickenwm.conf` (`game_rules` regex matching process names).
  - [x] Heuristic 3: Manual toggle hotkey (`Super + G`).
- [x] **6.2 Passthrough Execution State**
  - [x] Sleep `coop`: pause render loop, drop Z-order below game.
  - [x] Yield display: permit game swapchain to enter native DirectFlip (`DXGI_SWAP_EFFECT_FLIP_DISCARD`).
  - [x] Window unhooking: omit all `borderless_resize_subclass`, `WM_NCCALCSIZE`, and acrylic styles on game `HWND`.
  - [x] Ensure game is true foreground window (`SetForegroundWindow`).
- [x] **6.3 Input Passthrough & Anti-Cheat Safety**
  - [x] Disable all desktop hotkey intercepts in `chickenwm-binds` while in Game Mode.
  - [x] Preserve single master chord (`Super + Esc` / overlay trigger).
  - [x] Zero synthetic input injection: game receives 100% native raw hardware HID packets.
- [x] **6.4 System Priority Tuning**
  - [x] Call `AvSetMmThreadCharacteristicsW("Games", &task_index)`.
  - [x] Set game process priority: `SetPriorityClass(game_proc, ABOVE_NORMAL_PRIORITY_CLASS)`.
  - [x] Lower background ChickenWM daemons to `BELOW_NORMAL_PRIORITY_CLASS`.

---

### Phase 7: `chickenscope` — In-Game Transparent HUD Overlay
- [x] **7.1 Overlay Trigger & Window Setup**
  - [x] Register non-conflicting chord (`Super + Tab` / `Shift + Tab`).
  - [x] Create topmost transparent overlay window directly above active game.
- [x] **7.2 HUD Compositing**
  - [x] Render partial `coop` viewspace (browser tab, chat, roobar status, notifications).
  - [x] Route mouse/keyboard events to HUD while active; forward no clicks to game.
  - [x] Dismiss overlay on chord toggle &rarr; return instant raw focus to game (zero Alt-Tab latency).

---

### Phase 8: Supervisor, Resilience & Packaging
- [x] **8.1 Watchdog & Failsafe (`chickenwm-rooster`)**
  - [x] Monitor `coop` process lifecycle.
  - [x] On unexpected panic / device-loss: automatically unhide all real windows, restore standard styles, and reboot `coop` within 100ms.
  - [x] Dedicated zero-dependency thread for emergency hotkey (`Ctrl + Alt + Shift + Esc`) to restore native shell.
- [x] **8.2 System Tray Broker**
  - [x] Verify `Shell_TrayWnd` compatibility in rooster collects all background app icons (Steam, Discord, Nvidia, audio).
  - [x] Expose tray items via IPC to `roobar` or `coop` HUD.
- [x] **8.3 Scope Constraints Enforcement**
  - [x] English/Latin keyboard input only (IME / `TextInputHost.exe` disabled/bypassed).
  - [x] Hardware cursor clipping (`ClipCursor`) passed directly to target window.
