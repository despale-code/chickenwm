# Implementation Roadmap: `coop` & `chickenscope`

---

### Phase 1: Windows System Configuration & Shell Prep
- [ ] **1.1 UAC Elevation Policy**
  - [ ] Set `HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System\PromptOnSecureDesktop = 0`.
  - [ ] Verify `consent.exe` spawns on `Default` desktop as standard composite window.
- [ ] **1.2 Lock Workstation Interception**
  - [ ] Set `HKCU\Software\Microsoft\Windows\CurrentVersion\Policies\System\DisableLockWorkstation = 1`.
  - [ ] Verify `Win + L` is not swallowed by `win32k.sys` and can be trapped by low-level keyboard hook.
- [ ] **1.3 Winlogon Shell Replacement Scripting**
  - [ ] Script registry switch for `HKLM\...\Winlogon\Shell` to `chickenwm-rooster.exe`.
  - [ ] Add rollback failsafe hook in installer.
- [ ] **1.4 Service Debloating Automation**
  - [ ] Disable redundant services: `WSearch`, `BcastDVRUserService`, `WpnService`, `TabletInputService`, `DiagTrack`, `SysMain`, `DPS`.
  - [ ] Ensure non-negotiable services remain active: `dwm.exe`, `CryptSvc`, `Winmgmt`, `EventLog`, `Audiosrv`, `RpcSs`.

---

### Phase 2: `coop` Core — WGC to Vulkan Zero-Copy Bridge
- [ ] **2.1 Windows Graphics Capture (WGC) Setup**
  - [ ] Implement `GraphicsCaptureItem` initialization for target `HWND`s via WinRT/C++/Rust (`windows-capture` or native `windows::Graphics::Capture`).
  - [ ] Configure `GraphicsCaptureSession`: `IsBorderRequired = false`, `IsCursorCaptureEnabled = false`.
  - [ ] Create `Direct3D11CaptureFramePool` in `B8G8R8A8_UNORM`.
- [ ] **2.2 DXGI NT Handle Export**
  - [ ] Extract `ID3D11Texture2D` from captured frame.
  - [ ] Query `IDXGIResource1`.
  - [ ] Call `CreateSharedHandle` with `GENERIC_ALL` and security attributes to produce an NT `HANDLE`.
- [ ] **2.3 Vulkan External Memory Import**
  - [ ] Enable Vulkan extensions: `VK_KHR_external_memory`, `VK_KHR_external_memory_win32`.
  - [ ] Query memory requirements for imported handle (`VkMemoryGetWin32HandlePropertiesKHR`).
  - [ ] Bind imported `VkDeviceMemory` to target `VkImage` (`VK_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_WIN32_BIT`).
- [ ] **2.4 Cross-API Synchronization**
  - [ ] Initialize `IDXGIKeyedMutex` or `ID3D11Fence` on capture swapchain.
  - [ ] Import fence into Vulkan using `VK_KHR_external_semaphore_win32`.
  - [ ] Implement acquire/release cycle: WGC frame arrived &rarr; DXGI release &rarr; Vulkan wait &rarr; render pass &rarr; Vulkan release &rarr; DXGI acquire.

---

### Phase 3: `coop` Vulkan Rendering Pipeline & Animation Engine
- [ ] **3.1 Display Swapchain & Multi-Monitor Support**
  - [ ] Enumerate physical displays (`EnumDisplayMonitors`).
  - [ ] Spawn dedicated, borderless top-most `WS_EX_TOPMOST` Vulkan window per monitor.
  - [ ] Initialize independent `VkSwapchainKHR` per monitor to maintain native maximum refresh rates (e.g., 240Hz independent of secondary 60Hz).
- [ ] **3.2 Premultiplied Alpha Blending Pipeline**
  - [ ] Set pipeline blend state:
    - `src_color_blend_factor = VK_BLEND_FACTOR_ONE`
    - `dst_color_blend_factor = VK_BLEND_FACTOR_ONE_MINUS_SRC_ALPHA`
    - `src_alpha_blend_factor = VK_BLEND_FACTOR_ONE`
    - `dst_alpha_blend_factor = VK_BLEND_FACTOR_ONE_MINUS_SRC_ALPHA`
  - [ ] Verify zero black halo artifacts on translucent window edges.
- [ ] **3.3 Decoupled Quad Engine**
  - [ ] Implement spring-damper physics model in Rust (`F = -k*x - c*v`).
  - [ ] Generate 16x16 vertex mesh quads per window for non-rigid deformation (wobbly/jelly windows).
  - [ ] Vertex shader: apply model-view-projection (MVP) matrix + local vertex displacement based on velocity.
  - [ ] Fragment shader: Bicubic / Catmull-Rom downsampling filter for 150–200% virtual DPI sharpness.
- [ ] **3.4 Live Interactive Resize**
  - [ ] On resize drag: apply anchor-and-pad vertex stretch shader on the existing texture.
  - [ ] Send throttled (60Hz) Win32 size updates to target `HWND`.
  - [ ] Update texture UV coordinates dynamically upon arrival of matching frame dimension.
  - [ ] Lazy alignment: execute single `SetWindowPos` for physical hit-box on spring rest.
- [ ] **3.5 Wallpaper & Exposé Layer**
  - [ ] Render background layer: support high-res static images and custom GLSL/Vulkan fragment shaders.
  - [ ] Implement Exposé / 3D bird's-eye window layout switcher triggered via IPC/hotkey.

---

### Phase 4: `coop` Window Management & Hierarchy Tracking
- [ ] **4.1 Physical Window Management**
  - [ ] Position real Win32 windows silently behind the Vulkan canvas on the active display.
  - [ ] Strip non-client borders and decorations from managed windows (`chickenwm-core` sys helpers).
  - [ ] Occlusion culling: pause/throttle capture sessions on inactive workspaces to 0–1 FPS.
- [ ] **4.2 Popup & Menu Hierarchy Tracker**
  - [ ] Listen to WinEvents: `EVENT_OBJECT_CREATE`, `EVENT_SYSTEM_MENUPOPUPSTART`, `EVENT_OBJECT_DESTROY`.
  - [ ] Filter window class `#32768` (menus), combo boxes, and tooltips.
  - [ ] Resolve root owner via `GetAncestor(hwnd, GA_ROOTOWNER)`.
  - [ ] Position popup quad relative to parent window quad in Vulkan coordinate space.

---

### Phase 5: Input & Power Subsystems
- [ ] **5.1 Hardware Cursor Plane Integration**
  - [ ] Leave OS hardware cursor plane enabled (no software shader cursor).
  - [ ] On pointer move: hit-test quad tree in `coop`.
  - [ ] Query underlying target's cursor shape (`WM_SETCURSOR` / `GetCursorInfo`) and call `SetCursor()`.
- [ ] **5.2 High-Polling Mouse Optimization (1000–8000Hz)**
  - [ ] Receive mouse input via Raw Input (`RegisterRawInputDevices`).
  - [ ] Store coordinates in atomic integers (`AtomicI32`).
  - [ ] Decouple hit-test evaluation to run synchronously with monitor VSync (144–240Hz).
- [ ] **5.3 Custom DPMS Sleep & Wake**
  - [ ] Detect system idle timeout.
  - [ ] Sleep: send `SendMessageW(HWND_BROADCAST, WM_SYSCOMMAND, SC_MONITORPOWER, 2)`.
  - [ ] Drop Vulkan rendering to 0 FPS (`vkDeviceWaitIdle`).
  - [ ] Wake: low-level input hook catches any key/mouse &rarr; sends `SC_MONITORPOWER, -1` &rarr; restores swapchain.
- [ ] **5.4 Custom Lock Screen**
  - [ ] Intercept `Win + L`.
  - [ ] Render fullscreen lock canvas in Vulkan (clock, background blur, auth prompt).

---

### Phase 6: `chickenscope` — Game Mode & DirectFlip Passthrough
- [ ] **6.1 Game Detection Engine**
  - [ ] Heuristic 1: Window rect equals physical monitor bounds (`GetWindowRect == MonitorRect`) without `WS_CAPTION`.
  - [ ] Heuristic 2: Config matchers in `chickenwm.conf` (`game_rules` regex matching process names).
  - [ ] Heuristic 3: Manual toggle hotkey (`Super + G`).
- [ ] **6.2 Passthrough Execution State**
  - [ ] Sleep `coop`: pause render loop, drop Z-order below game.
  - [ ] Yield display: permit game swapchain to enter native DirectFlip (`DXGI_SWAP_EFFECT_FLIP_DISCARD`).
  - [ ] Window unhooking: omit all `borderless_resize_subclass`, `WM_NCCALCSIZE`, and acrylic styles on game `HWND`.
  - [ ] Ensure game is true foreground window (`SetForegroundWindow`).
- [ ] **6.3 Input Passthrough & Anti-Cheat Safety**
  - [ ] Disable all desktop hotkey intercepts in `chickenwm-binds` while in Game Mode.
  - [ ] Preserve single master chord (`Super + Esc` / overlay trigger).
  - [ ] Zero synthetic input injection: game receives 100% native raw hardware HID packets.
- [ ] **6.4 System Priority Tuning**
  - [ ] Call `AvSetMmThreadCharacteristicsW("Games", &task_index)`.
  - [ ] Set game process priority: `SetPriorityClass(game_proc, ABOVE_NORMAL_PRIORITY_CLASS)`.
  - [ ] Lower background ChickenWM daemons to `BELOW_NORMAL_PRIORITY_CLASS`.

---

### Phase 7: `chickenscope` — In-Game Transparent HUD Overlay
- [ ] **7.1 Overlay Trigger & Window Setup**
  - [ ] Register non-conflicting chord (`Super + Tab` / `Shift + Tab`).
  - [ ] Create topmost transparent overlay window directly above active game.
- [ ] **7.2 HUD Compositing**
  - [ ] Render partial `coop` viewspace (browser tab, chat, roobar status, notifications).
  - [ ] Route mouse/keyboard events to HUD while active; forward no clicks to game.
  - [ ] Dismiss overlay on chord toggle &rarr; return instant raw focus to game (zero Alt-Tab latency).

---

### Phase 8: Supervisor, Resilience & Packaging
- [ ] **8.1 Watchdog & Failsafe (`chickenwm-rooster`)**
  - [ ] Monitor `coop` process lifecycle.
  - [ ] On unexpected panic / device-loss: automatically unhide all real windows, restore standard styles, and reboot `coop` within 100ms.
  - [ ] Dedicated zero-dependency thread for emergency hotkey (`Ctrl + Alt + Shift + Esc`) to restore native shell.
- [ ] **8.2 System Tray Broker**
  - [ ] Verify `Shell_TrayWnd` compatibility in rooster collects all background app icons (Steam, Discord, Nvidia, audio).
  - [ ] Expose tray items via IPC to `roobar` or `coop` HUD.
- [ ] **8.3 Scope Constraints Enforcement**
  - [ ] English/Latin keyboard input only (IME / `TextInputHost.exe` disabled/bypassed).
  - [ ] Hardware cursor clipping (`ClipCursor`) passed directly to target window.
