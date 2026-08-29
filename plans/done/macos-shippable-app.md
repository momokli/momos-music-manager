# Plan: macOS Shippable Application (v1.0.0)

**Status**: done
**Branch**: `feat/macos-app-bundle`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

---

## Description

Package Momo's Music Manager as a proper, double-clickable macOS `.app` bundle
distributed as a universal DMG via GitHub Releases. One download, drag to
`/Applications`, double-click — server starts and browser opens. No terminal,
no `cargo build`, no git clone.

**Decision: browser launch, not embedded webview.** Spotify OAuth needs a real
browser, DJ workflows need full audio APIs, and the Plex/Sonarr pattern is
battle-tested. No wry/Tauri overhead.

---

## Architecture

```
Momo's Music Manager.app/           ← cargo-bundle generates this
├── Contents/
│   ├── Info.plist                  ← LSUIElement=true, bundle metadata
│   ├── MacOS/
│   │   └── momos-music-manager     ← universal binary (lipo aarch64 + x86_64)
│   └── Resources/
│       └── AppIcon.icns            ← converted from logo_cutted_out.png
```

**Flow**: Double-click → binary starts → `webbrowser::open("http://localhost:3000")`
→ user interacts via browser. Server persists when browser closes. Re-opening the
app detects the already-running server (port 3000 bound) and just opens the browser.

**Config**:

- Database: `~/.local/share/momos-music-manager/library.db`
- Config: `~/.config/momos-music-manager/config.toml`
- Logs: `~/Library/Logs/momos-music-manager/`
- Version: **1.0.0** (first shippable release)

---

## Implementation Steps

### Step 1: Add `webbrowser` crate + auto-open browser

| File          | Change                                                                                                                 |
| ------------- | ---------------------------------------------------------------------------------------------------------------------- |
| `Cargo.toml`  | Add `webbrowser = "1"` dependency                                                                                      |
| `src/main.rs` | Add `--no-browser` flag to `Serve`; after `serve()` starts, spawn thread that waits 2s then calls `webbrowser::open()` |

### Step 2: Add cargo-bundle metadata

| File         | Change                                                                                     |
| ------------ | ------------------------------------------------------------------------------------------ |
| `Cargo.toml` | Add `[package.metadata.bundle]` section with icon, identifier, LSUIElement, min OS version |

### Step 3: Create app icon

| File                      | Change                                                                      |
| ------------------------- | --------------------------------------------------------------------------- |
| `resources/icon.icns`     | Convert `frontend/logo_cutted_out.png` to `.icns` using `sips` + `iconutil` |
| `resources/icon.iconset/` | Intermediate `.iconset` directory (generated, gitignored)                   |

### Step 4: Create Entitlements for hardened runtime

| File                         | Change                                                               |
| ---------------------------- | -------------------------------------------------------------------- |
| `scripts/entitlements.plist` | Network server/client + file access + JIT/unsigned-memory allowances |

### Step 5: Create package script

| File                       | Change                                                                                        |
| -------------------------- | --------------------------------------------------------------------------------------------- |
| `scripts/package-macos.sh` | One-command: `cargo build --release` (universal) → `cargo bundle` → `create-dmg` → signed DMG |

### Step 6: Create GitHub CI release workflow

| File                            | Change                                                                          |
| ------------------------------- | ------------------------------------------------------------------------------- |
| `.github/workflows/release.yml` | On `v*` tag: build universal binary, bundle .app, create DMG, upload to release |

### Step 7: Update docs

| File           | Change                                                                        |
| -------------- | ----------------------------------------------------------------------------- |
| `README.md`    | Add "macOS Installation" section with download + drag-to-install instructions |
| `CHANGELOG.md` | v1.0.0 entry                                                                  |

---

## Acceptance Criteria

- [ ] `cargo build` passes
- [ ] `cargo test` passes
- [ ] `cd frontend && npx playwright test` passes
- [ ] `scripts/package-macos.sh` produces `target/Momo's-Music-Manager-v1.0.0.dmg` (universal)
- [ ] Opening the DMG shows the app + symlink to /Applications
- [ ] Dragging to /Applications and double-clicking: server starts, browser opens to dashboard
- [ ] Re-opening the app (server already running): just opens browser, no duplicate server
- [ ] Database created at `~/.local/share/momos-music-manager/library.db`
- [ ] GitHub CI runs on tag push and attaches DMG to release

---

## Non-Goals

- Notarization — Apple Developer account required ($99/yr). Ship unsigned first.
- Homebrew Cask — write formula after first release
- Menu bar tray icon — future enhancement via `tray-icon` crate

---

## Open Questions (Resolved)

1. **Database path**: `~/.local/share/momos-music-manager/library.db` ✅
2. **Version**: 1.0.0 ✅
3. **Universal binary**: yes, single DMG via `lipo` ✅
4. **First-run UX**: dashboard as-is ✅
