# AGENT EXECUTION POLICY — STRICT MODEL DIVISION, RAG-FIRST & UI ICON RULES

## 1. STRICT MODEL DIVISION: GEMINI 3.8 FLASH MEDIUM vs. GEMINI 4 ARGON

Work is strictly partitioned by domain across models:

1. **Gemini 3.8 Flash Medium (`flash-stylist-explorer` subagent with `Model: "flash"`)**:
   - **Styling, Layouting & Frontend Design**: Strictly and exclusively handles all UI styling, layouting, `egui` frontend design, widget alignment/spacing/sizing, color palettes, animations, CSS stylesheets (`roobar.css`, `launcher.css`, etc.), and visual polish.
   - **Code Exploring & Targeting**: Strictly handles all codebase exploration, RAG queries (`rag_search`, `nav_symbol_resolve`, `rag_get_file_ranges`, `context_bundle`), locating target files/symbols, and narrowing line ranges.
2. **Gemini 4 Argon (Primary Model / `Model: "pro"`)**:
   - Reserved **exclusively for pure logic and systems programming**: Win32 APIs, IPC protocols, concurrency, state machines, networking/download engines, BSP/tiling algorithms, Vulkan/D3D11 compositor logic, and core systems architecture.
   - Must delegate code exploration/targeting and frontend styling/layouting to Gemini 3.8 Flash Medium (`Model: "flash"`).
3. **Cargo Execution & Summarization (`cargo-runner-micro` with `Model: "flash_lite"`)**:
   - Only `cargo build`, `cargo check`, and `cargo test` execution and `< 100` token diagnostic summarization (`PASS` or `FAIL` + `file.rs:line:col: [code] message`).
   - Use `-q --message-format=short -p <specific_crate>`.

---

## 2. PROPER ICONS IN TEXT & UI RULE (NO UNICODE EMOJI / GLYPH HACKS)

When rendering icons alongside text or inside buttons, tabs, bars, and lists:

1. **Strictly Forbidden**:
   - **Never** embed Unicode emojis, dingbats, symbols, or private-use Nerd Font glyphs inside UI strings (e.g., never use `"🔍"`, `"⚙"`, `"✕"`, `"⬇"`, `"🔔"`, `"📁"`, `"▶"`, `"⏸"` in `ui.label`, `ui.button`, `RichText`, or window titles). They render as missing-glyph tofu boxes (`□`), lack anti-aliased vector geometry, or break baseline alignment.
2. **Required Proper Implementation**:
   - **Vector UI Icons**: Always draw icons using `egui::Painter` vector primitives via `chickenwm_core::ui_icons` (`paint_search`, `paint_close`, `paint_download`, `paint_bell`, `paint_gear`, `paint_folder`, `paint_check`, etc.) or custom anti-aliased `Painter` strokes (`line_segment`, `circle_stroke`, `rect_stroke`, `cubic_bezier_stroke`) inside a dedicated icon `Rect` (`ui.allocate_exact_size` or sub-rect of an interactive widget), paired with clean text rendered via `painter.galley` / `ui.label`.
   - **Bitmap / Shell Icons**: Fetch high-DPI 256×256 (`SHIL_JUMBO` / `IShellItemImageFactory`) RGBA bitmaps; bind textures with `egui::TextureOptions::LINEAR` when downscaling for smooth anti-aliasing, and `egui::TextureOptions::NEAREST` when upscaling low-resolution legacy icons so they stay crisp without blurring.
