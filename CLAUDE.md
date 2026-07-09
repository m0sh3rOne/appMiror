# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

POC for streaming a **single native window** to a web browser with low latency, plus reverse injection of mouse/keyboard events back into that window. Two capture/input backends exist behind a common platform façade: **Windows** (Windows Graphics Capture + `SendInput`) and **Linux/X11** (`x11rb` `GetImage` polling + XTest; works under XWayland, but not native-Wayland windows — a PipeWire backend is the intended next step). It can also write frames to disk on the backend machine (screenshots and image sequences), and exposes an **MCP server** so an external LLM can list/attach/drive windows. See `README.md` for the intended WebRTC/H.264 upgrade path.

## Commands

Backend (from `backend/`):
```bash
cargo run          # builds + serves on http://127.0.0.1:8080 (WebSocket at /ws)
cargo build        # RUST_LOG via env filter, e.g. RUST_LOG=debug cargo run
```

Frontend (from `frontend/`):
```bash
npm install
npm run dev        # Vite dev server on :5173, proxies /ws -> ws://127.0.0.1:8080
npm run build      # tsc typecheck + static build into frontend/dist
```

Two run modes: **dev** = Vite on 5173 (proxy to backend), open `localhost:5173`. **static** = `npm run build`, then backend serves `../frontend/dist` as its fallback route, open `localhost:8080`.

There are no automated tests. `npm run build` runs `tsc` and is the frontend typecheck.

## Operational notes that bite

- **Single backend instance only.** The server binds `127.0.0.1:8080`; a second `cargo run` fails with `os error 10048`. Kill the old one first: `taskkill /F /IM appmirror-backend.exe` (or `Stop-Process -Id <pid>`).
- **`windows-capture` API is version-sensitive.** Code targets 1.5.0 (`Settings::new` takes 8 args including `SecondaryWindowSettings`/`MinimumUpdateIntervalSettings`/`DirtyRegionSettings`; frame bytes via `as_nopadding_buffer`). If a build breaks after a version bump, check `cargo doc -p windows-capture --open`.
- **`ClientToScreen` lives in `windows::Win32::Graphics::Gdi`**, not `WindowsAndMessaging` (needs the `Win32_Graphics_Gdi` feature, already enabled in Cargo.toml).
- **The Linux backend can't run here but must keep type-checking:** `cargo check --target x86_64-unknown-linux-gnu` (target installed; `x11rb` is pure Rust, no linker needed).

## Architecture

### The WebSocket protocol is duplicated by hand and must stay in sync
`backend/src/protocol.rs` (serde, `#[serde(tag = "type", rename_all = "snake_case")]`) and `frontend/src/protocol.ts` (discriminated union on `type`) describe the **same** messages. Any change to one requires the mirror edit in the other — the tag strings and field names must match exactly. Control messages are JSON text frames; **video frames are separate binary frames** (JPEG bytes), not part of these enums.

### Platform abstraction via cfg façades
`capture/mod.rs` and `input/mod.rs` are thin dispatchers exposing platform-neutral functions; `input/mod.rs` aliases the active backend as `imp` (`self::windows`/`self::linux`). Real work lives in `{capture,input}/windows.rs` (`#[cfg(windows)]`) and `{capture,input}/linux.rs` (`#[cfg(target_os = "linux")]`). Both backends honor the same contract: a capture thread encodes each frame to JPEG and pushes it into a `watch` channel; input targets the focused window (hence `focus_window` on select). Nothing above the façade references platform types — the `id`/`hwnd: isize` is an HWND on Windows, an X11 `Window` on Linux. Adding a Wayland/PipeWire backend means another module wired into these façades.

### Frame pipeline (latency-critical design)
Windows: WGC runs on its own thread (`start_free_threaded`) and pushes frames on arrival. Linux: a thread polls `GetImage` at ~30fps (X11 has no frame-arrival signal). Each frame: native pixels → strip alpha/reorder to RGB → JPEG (`image` crate) → pushed into a `tokio::sync::watch` channel. `watch` is chosen deliberately as **latest-frame-wins**: a slow consumer drops stale frames instead of building a backlog. `server.rs` spawns one forward task per connection that reads `watch` and sends binary frames; `stream.ts` decodes via `createImageBitmap` onto a canvas whose internal buffer tracks the native source resolution.

### Disk capture reuses the same JPEG frames (`recorder.rs`)
`Screenshot`/`StartRecording`/`StopRecording` write the **already-encoded** JPEG bytes from the `watch` channel straight to disk — no re-encode. The session keeps a clone of the frame receiver (`Session.frames`) for this. Recording spawns a task that writes the latest frame on each change, fps-capped (default 10, clamped 1–60), into a timestamped `rec_<ts>/` folder; it self-terminates when the capture stops (watch sender dropped). **Files land on the backend host**, not the browser — the optional `dir` in these messages is a server-side path (default `<Pictures>/appMirror` via the `dirs` crate).

### MCP server for external agents (`mcp/`)
A second, independent control surface for an LLM, kept off the human WebSocket path. `mcp/mod.rs` is a JSON-RPC 2.0 router (`handle_mcp_request` → `Option<McpResponse>`; `None` = notification). Tools: `list_windows`, `attach_to_window` (stores the target + client size in shared `AppState.attached`; the `dir`-less `pid` arg is reserved/unsupported — use `window_id`), `inject_input` (normalized `[0,1]` coords → client px via `denorm`, dispatched to the `input` façade; supports `mouse_move`/`button`/`click`/`wheel`/`key`/`text`). `resources/list`+`resources/read` expose the capture folder (base64 JPEG blobs, with an anti-traversal `canonicalize` check). **Two error channels, per MCP spec:** protocol errors (unknown method/param) → JSON-RPC `error`; tool-execution failures → a normal `result` with `isError: true` so the model can read them.

MCP JSON-RPC/tool-arg types live in `protocol.rs` (`McpRequest`/`McpResponse`/`McpError`/`McpMethod`/`AttachArgs`/`InjectArgs`) — separate from the WebSocket enums. Transport (`mcp/transport.rs`) is MCP's **HTTP+SSE** flavor (2024-11-05): `GET /api/mcp/sse` opens the downstream stream and first emits an `endpoint` event carrying `?sessionId=…`; `POST /api/mcp/message?sessionId=…` returns `202` and the JSON-RPC reply is **pushed on that SSE stream** (sessions map `sessionId → mpsc::Sender` in `AppState.mcp_sessions`, lazily reaped when the SSE receiver is gone). A POST **without** a live `sessionId` returns the reply directly in the HTTP body — this is the `curl`-testable path.

### Shared `AppState` is additive (`app.rs`)
Introduced only for MCP; the WebSocket keeps its per-connection `Session` model. Holds `attached` (MCP's target window), `mcp_sessions` (SSE registry), and a `broadcast` channel `notices`. When MCP attaches, it publishes a notice; `handle_socket` now `select!`s between the client stream and `notices.recv()`, forwarding each as a `ServerMessage::Notice` (`"notice"` in `protocol.ts`) — informational only, it never touches the frame `watch` pipeline. This is why MCP tool calls don't perturb the JPEG stream.

### Coordinate translation happens in two hops
1. **Frontend** (`input.ts` `toNative`): pointer position relative to the canvas's CSS display size (`getBoundingClientRect`) → native window resolution reported by the backend's `selected` message. Sent as native client pixels.
2. **Backend** (`input/windows.rs`): client pixels → screen coords (`ClientToScreen`) → absolute virtual-desktop coords (0..65535) for `SendInput`.

### Annotation overlay is frontend-only (`annotate.ts`, `rulers.ts`)
Vector annotation tools (text, rect/ellipse/line/arrow, pen, marker, rulers+guides+measure, Adobe-style slices) live in an SVG overlay stacked on the video canvas inside `#stage`. Nothing here touches the WebSocket protocol. Key invariants:
- **Geometry is stored in _frame_ pixels** — the dimensions of the decoded JPEG (`canvas.width/height`), which is what `onFrameSize` feeds to `annot.setNativeSize`. The SVG `viewBox` is set to those same frame dims, so its aspect ratio **always** equals `#stage`'s (which `main.ts` also sizes from `canvas.width/height`). This is load-bearing: the overlay uses `preserveAspectRatio="none"`, so if the viewBox aspect differed from the rendered box, strokes would scale non-uniformly (a rectangle's horizontal edges rendering thicker than its vertical ones). Do **not** re-base the overlay on the `selected` message's client size — that size can diverge wildly from the frame (minimized window, DPI, quality downscale) and caused exactly that stroke-distortion bug.
- UI chrome (handles, dashes, labels) is sized in frame units via `u() = cssPx / scale` to stay visually constant; the annotation scale passed to `layoutChanged` is `cssWidth / frameWidth`. Input injection is the separate concern: `InputController` keeps the **native** client size from `selected`, because `SendInput`/`PostMessage` need true window pixels. Rulers/measure therefore read out in frame px (exact at HD quality, downscaled at lower quality).
- Selecting any tool other than `cursor` suspends remote input injection (`InputController.setEnabled(false)`) and flips the overlay's `pointer-events` — annotation gestures must never leak into the remote window.

### Two input modes: foreground (`SendInput`) and background (`PostMessage`)
Default is **foreground**: `SendInput` routes to whatever window has focus, so selecting a window calls `SetForegroundWindow` on it (moves the physical cursor, steals the host's focus). The `set_input_mode` WebSocket message flips a per-`Session` `input_bg` flag; when on, `select_window` skips `SetForegroundWindow` and mouse/key dispatch uses the parallel `input::*_bg` façade functions (`mouse_move_bg`/`mouse_button_bg`/`wheel_bg`/`key_bg`) which `PostMessage` directly to the target HWND — no focus steal, physical cursor untouched. The MCP path always uses foreground and is untouched by this.

Background-mode targeting (Windows, `input/windows.rs`): mouse messages resolve the **deepest child control under the point** (`ChildWindowFromPointEx`, point reprojected per hop) so clicks land on buttons/fields; keyboard routes to `LAST_KBD_TARGET` — the child of the **last background click** (a real background window has no OS "focused control"), validated with `IsChild`. `key_bg` posts **only** `WM_KEYDOWN`/`WM_KEYUP` and lets the target app's own `TranslateMessage` produce the layout-correct `WM_CHAR` — synthesizing our own `WM_CHAR` doubles input and ignores the keyboard layout. **Known limits:** `PostMessage` can't drive DirectX/fullscreen-exclusive apps; and because it doesn't update the target thread's keyboard state, `TranslateMessage` can't see Shift — so **shifted/uppercase characters don't render in background mode** (unshifted chars, layout base symbols, and navigation keys do). Linux has no true per-window injection (XTest is global), so `*_bg` there falls back to the foreground XTest path.

### Cursor visibility toggle (`show_cursor` on `select_window`)
WGC's `CursorCaptureSettings` is fixed at capture start, so the frontend "Curseur" checkbox toggles it by **re-sending `select_window`** (restarting the stream) with `show_cursor`. `capture::start` takes the flag through to `CursorCaptureSettings::WithCursor`/`WithoutCursor`. On Linux it's a **no-op**: X11 `GetImage` never composites the cursor into the window image anyway.

### Session model
One WebSocket connection = one `Session` in `server.rs` holding the current capture control, forward task, target window id, a clone of the frame receiver (for disk capture), and any active recording. `select_window` calls `stop_capture` first, which also stops an in-progress recording — so switching windows ends recording. No auth/TLS — localhost only.
