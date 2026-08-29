# Plan: macOS Menu Bar Tray Icon

**Status**: done
**Branch**: `feat/tray-icon`
**Ready for review**: yes
**Depends on**: `feat/macos-app-bundle` (LSUIElement, .app bundle structure)
**Migration needed**: no

---

## Description

Add a macOS menu bar tray icon showing server status with "Open Dashboard"
and "Quit" menu items. Uses `tray-icon` + `tao` (Tauri ecosystem, native
AppKit bindings, zero WebKit) — adds ~3.5 MB to binary size, no GPU overhead.

Restructures `main()` so the Tao event loop owns the main thread (required for
`NSStatusBar`), and the Axum server + all background tasks run on a Tokio
runtime in a spawned thread.

---

## Architecture Change

```
Before (current):                  After:
┌──────────────┐                   ┌──────────────┐
│  main thread │                   │  main thread  │
│  ┌─────────┐ │                   │  ┌──────────┐ │
│  │ tokio   │ │                   │  │ tao      │ │
│  │ runtime │ │                   │  │ event    │ │
│  │  axum   │ │                   │  │ loop     │ │
│  │  poller │ │                   │  │ tray     │ │
│  │  watcher│ │                   │  │ icon     │ │
│  └─────────┘ │                   │  └──────────┘ │
└──────────────┘                   └──────────────┘
                                        ▲  spawn
                                   ┌──────────────┐
                                   │ bg thread    │
                                   │  ┌─────────┐ │
                                   │  │ tokio   │ │
                                   │  │ runtime │ │
                                   │  │  axum   │ │
                                   │  │  poller │ │
                                   │  │  watcher│ │
                                   │  └─────────┘ │
                                   └──────────────┘
```

The tray icon is only on macOS (`#[cfg(target_os = "macos")]`). On other
platforms, `main()` runs tokio directly (current behavior, unchanged).

---

## Tray Menu

```
🍀  Momo's Music Manager    ← tooltip (hover)
────────────────────────
   Open Dashboard          ← opens http://localhost:3000
   ──────────────────
   Quit                    ← shuts down server + exits
```

---

## Files to modify/create

| File                      | Change                                                                                                                              |
| ------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| `Cargo.toml`              | Add `tray-icon = "0.24"`, `tao = "0.36"`, `image = { version = "0.25", default-features = false, features = ["png"] }`              |
| `src/main.rs`             | Restructure `main()`: `#[cfg(macos)]` → tao event loop on main + server on bg thread; `#[cfg(not(macos))]` → existing tokio-on-main |
| `src/tray.rs`             | **New** — tray icon creation, menu event handling, `UserEvent` enum                                                                 |
| `resources/tray-icon.png` | **New** — 22×22 RGBA PNG (template image, adapts to dark/light mode)                                                                |

---

## Implementation Steps

### Step 1: Add dependencies

```toml
[dependencies]
tray-icon = "0.24"
tao = { version = "0.36", features = ["tray"] }

# Only needed at build time for converting PNG to RGBA; embed result via include_bytes!
# Actually: just embed the PNG with include_bytes! and let tray-icon decode it at runtime.
# tray-icon's Icon::from_rgba takes raw RGBA bytes — we can use the `image` crate in a
# build script to pre-convert, or just pass the pre-computed array.
```

Actually, `tray-icon` has `Icon::from_rgba(width, height, rgba_bytes)`. The simplest approach is to use `image` as a build dependency to convert the PNG to raw RGBA, then `include_bytes!` the result. Or just use `image` at runtime — it's a small cost at startup.

Simpler: `tray-icon` v0.24 supports loading from PNG bytes directly via the `png` feature. Enable it and use `include_bytes!("resources/tray-icon.png")`.

### Step 2: Create tray module (`src/tray.rs`)

Handles:

- Creating the tray icon with menu on `StartCause::Init`
- Dispatching menu events (`Open Dashboard` → `webbrowser::open`, `Quit` → shutdown)
- `LSUIElement = true` — no dock icon, menu bar only (already in Info.plist)

### Step 3: Restructure `src/main.rs`

```rust
#[tokio::main]
async fn main() -> Result<()> {
    // ... logging setup (same as before) ...

    #[cfg(target_os = "macos")]
    {
        // Clone config/db setup for the server thread
        // Spawn server on background thread
        let server_thread = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                // existing serve() logic
            });
        });

        // Run tray icon event loop on main thread
        run_tray_event_loop()?;
    }

    #[cfg(not(target_os = "macos"))]
    {
        // Existing behavior: tokio on main, start server directly
        existing_serve().await?;
    }
}
```

### Step 4: Quit flow

When user clicks "Quit" → `ControlFlow::Exit` is set. The tao event loop exits,
`main()` returns, and the process terminates. Tokio background task is killed.
For a graceful shutdown, we could signal the server via `CancellationToken`
before exiting — but for v1, SIGTERM-on-exit is fine (SQLite is crash-safe).

---

## Acceptance Criteria

- [ ] `cargo build` passes on macOS
- [ ] `cargo test` passes
- [ ] `cd frontend && npx playwright test` passes
- [ ] Running `cargo run -- serve` shows tray icon in menu bar (no dock icon)
- [ ] Clicking "Open Dashboard" opens browser to localhost:3000
- [ ] Clicking "Quit" shuts down the server process
- [ ] On Linux/other platforms, server starts normally (no tray, no regression)
- [ ] DMG package script still produces a working .app

---

## Non-Goals

- Cross-platform tray (Windows/Linux) — out of scope for now
- Dynamic icon changes (green/grey dot for status)
- Right-click vs left-click behavior customization
- Graceful shutdown with `CancellationToken` — future enhancement

---

## Risks

| Risk                                         | Mitigation                                                                                  |
| -------------------------------------------- | ------------------------------------------------------------------------------------------- |
| Tao event loop + Tokio thread lifecycle bugs | The pattern is well-tested (Tauri uses it). Test manual quit, SIGTERM, and crash scenarios. |
| `objc2` version conflicts                    | Pin `tray-icon = "0.24"` explicitly; `tao` and `tray-icon` share the same `objc2` ecosystem |
| Background thread panic doesn't kill tray    | Add a channel so the tray can detect server death and show an error state                   |
| DMG still works after restructure            | `.app` bundle just launches the binary — no change to Info.plist or packaging               |
