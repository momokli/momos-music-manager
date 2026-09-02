# Platform Support Roadmap

Status: **Stand 2026-08-30** · Ziel: **alle 6 Plattformen**

Momo's Music Manager ist ein **headless Server** (axum + SQLite via sqlx) mit
statischem Web-Frontend (rust-embed). Es gibt **kein natives GUI-Fenster** — die
einzige plattformspezifische UI ist das macOS-Menübar-Tray (hinter
`cfg(target_os = "macos")`). Dadurch ist die App grundsätzlich auf allen
Desktop-Plattformen lauffähig; der Aufwand pro Plattform liegt fast vollständig
in Toolchain, Paketierung und CI, nicht im Code.

## Ziel-Matrix

| # | Plattform         | Target-Triple                | Build | Cross-Toolchain | Paketierung | CI-Runner | Status |
|---|-------------------|------------------------------|-------|-----------------|-------------|-----------|--------|
| 1 | Linux x64         | `x86_64-unknown-linux-gnu`   | ✅ | nativ (keine)   | ✅ tar.gz  | `ubuntu-22.04` | ✅ **fertig** |
| 2 | Linux ARM64       | `aarch64-unknown-linux-gnu`  | ✅ | ✅ `gcc-aarch64-linux-gnu` | ✅ tar.gz | `ubuntu-22.04` (cross) | ✅ **fertig** |
| 3 | Windows x64       | `x86_64-pc-windows-msvc`     | ✅ | nativ (keine)   | ✅ zip     | `windows-latest` | ✅ **fertig** |
| 4 | Windows ARM64     | `aarch64-pc-windows-msvc`    | ✅ | nativ (ARM-Runner) | ✅ zip | `windows-11-arm` (hosted, seit 2025) | ✅ **fertig** |
| 5 | macOS Intel       | `x86_64-apple-darwin`        | ✅ | nativ           | ✅ DMG     | `macos-latest` | ✅ **fertig** (wie bisher) |
| 6 | macOS ARM         | `aarch64-apple-darwin`       | ✅ | nativ           | ✅ DMG (universal) | `macos-latest` | ✅ **fertig** (wie bisher) |

Legende: ✅ geht heute · 🟡 geht mit Einschränkungen / offene Punkte · ❌ fehlt

## Dimensionen im Detail

### Build (crate-kompiliert auf dem Ziel)

| Dimension | Linux x64 | Linux ARM64 | Windows x64 | Windows ARM64 | macOS Intel | macOS ARM |
|---|---|---|---|---|---|---|
| `cargo build --release` | ✅ | ✅ (cross) | ✅ | ✅ | ✅ | ✅ |
| SQLite (sqlx) | ✅ bundled¹ | ✅ bundled | ✅ bundled | ✅ bundled | ✅ bundled | ✅ bundled |
| TLS (reqwest/hf-hub/rspotify) | ✅ rustls² | ✅ rustls | ✅ rustls | ✅ rustls | ✅ rustls | ✅ rustls |
| ML (candle/tokenizers) | ✅ pure Rust | ✅ (C via Cross-CC) | ✅ (MSVC) | ✅ (ARM64 MSVC) | ✅ | ✅ |
| C-Deps (onig/esaxx) | ✅ vendored | ✅ Cross-CC | ✅ MSVC | ✅ ARM64 MSVC | ✅ | ✅ |

¹ `sqlx` baut SQLite über das `bundled`-Feature aus dem Quellcode mit — keine
System-SQLite nötig, identisches Verhalten auf allen Plattformen.
² TLS läuft komplett über `rustls` (native-tls/OpenSSL entfernt) — dadurch kein
`libssl-dev` auf Linux und problemloses Cross-Compiling nach ARM64.

### Paketierung & Verteilung

| Plattform | Format | Script | Inhalt | Checksumme |
|---|---|---|---|---|
| Linux | `tar.gz` | `scripts/package-linux.sh` | Binär, README.md, `deploy/momos-music-manager.service`, VERSION, INSTALL.txt | `SHA256SUMS` + `.sha256` je Datei |
| Windows | `zip` | `scripts/package-windows.ps1` | `momos-music-manager.exe`, README.md, VERSION, RUN.txt | `.sha256` je Datei |
| macOS | DMG (universal) | `scripts/package-macos.sh` | `.app`-Bundle (lipo universal, ad-hoc-signiert) | `.sha256` |

Namensschema (CI): `momos-music-manager-<version>-<os>-<arch>.<ext>`

### CI-Runner (GitHub Actions)

| Plattform | Runner | Anmerkung |
|---|---|---|
| Linux x64 | `ubuntu-22.04` | nativ; glibc 2.35 → läuft auf Ubuntu 22.04+/Debian 12+ (portabler als ubuntu-latest/glibc 2.39) |
| Linux ARM64 | `ubuntu-22.04` + `aarch64-unknown-linux-gnu` | Cross-Compile, braucht `gcc-aarch64-linux-gnu` + `g++-aarch64-linux-gnu` |
| Windows x64 | `windows-latest` | nativ MSVC |
| Windows ARM64 | `windows-11-arm` | **hosted ARM64-Runner existiert seit 2025** — wird direkt genutzt (kein Cross, kein self-hosted nötig) |
| macOS Intel + ARM | `macos-latest` | ein Runner baut beide Targets + lipo |

Workflow: `.github/workflows/build-all.yml` — läuft bei Push auf `main`
(rolling `latest-main`-Release) und bei Tags `v*` (Release-Assets), plus
`workflow_dispatch`. PRs laufen die Build-Matrix als Check (ohne Publish).

### Plattform-Besonderheiten

| Thema | Linux | Windows | macOS |
|---|---|---|---|
| Tray/UI | kein Tray (headless Server) | kein Tray (headless Server) | Tray via `tray-icon`/`objc2`, `cfg(target_os = "macos")` |
| Browser-Auto-Open | `webbrowser` → `xdg-open`; headless: `--no-browser` | `webbrowser` → `start`; headless: `--no-browser` | Launch-Server + Tray |
| Signing | keins nötig | **offen:** SmartScreen-Warnung bei unsigned `.exe` (kein Code-Signing-Zertifikat im Projekt) | Ad-hoc-Signatur; **offen:** Gatekeeper-Rechtsklick-Öffnen, nicht notarized (kein Developer-ID-Account im Projekt) |
| Autostart/Service | `deploy/momos-music-manager.service` (systemd) ✅ | offen: NSSM/Task-Scheduler (nur dokumentiert, kein Script) | `install-launch-agent` (launchd) ✅ |
| Defender | — | **offen:** unsigned Binär kann von Defender/SmartScreen als „unbekannte App" markiert werden (kein Malware-Befund erwartet; Nutzerhinweis nötig) | Gatekeeper siehe oben |

## Prioritäten-Empfehlung

1. **Linux x64** (fertig) — primäre Server-Plattform, headless, systemd.
2. **Linux ARM64** (fertig) — NAS/Raspberry-Pi-DJ-Setups; Cross-Build im CI.
3. **Windows x64** (fertig) — größte Desktop-Zielgruppe nach macOS.
4. **Windows ARM64** (fertig) — Surface/ARM-Laptops; hosted `windows-11-arm`-Runner.
5. **macOS Intel + ARM** (fertig, unverändert) — Universal-DMG.

## Was heute schon geht

- Alle 6 Targets bauen in CI grün (Build-Matrix).
- Linux/Windows/macOS-Artefakte landen im `latest-main`-Release und bei Tag-Releases, inkl. Checksummen.
- Linux: Binär startet headless, Health/API-Endpunkte antworten (Server-Smoke-Test).
- Linux-Server-Modus via mitgelieferter systemd-Unit.
- Autoupdater (M6 v1): Update-Check gegen `latest-main` (Ed25519-signiertes
  `SHA256SUMS` via minisign), SHA256-Verifikation vor Austausch, atomarer Swap
  mit `.bak` + Health-Grace + Auto-Rollback, Opt-out — siehe
  [README](README.md) und [RELEASE-ROADMAP.md](RELEASE-ROADMAP.md) M6.

## Was fehlt / offen (ehrlich markiert)

| Punkt | Status | Aufwand | Blockiert durch |
|---|---|---|---|
| **Windows Code-Signing** (SmartScreen) | ❌ offen | mittel | kostenpflichtiges EV/OV-Zertifikat; Alternativen: `osslsigncode` + Azure Trusted Signing |
| **macOS Notarization** (Gatekeeper ohne Rechtsklick) | ❌ offen | mittel | Apple Developer ID Account (99 $/Jahr) + notarytool |
| **AppImage / Flatpak** für Linux | 🟡 geplant, nicht jetzt | mittel | Entscheidung ob portable tar.gz reicht (aktuell: ja) |
| **Windows-Dienst-Installation** (NSSM-Script) | 🟡 nur dokumentiert | klein | kein Blocker |
| **Windows ARM64 auf älteren Runnern** | ✅ gelöst | — | `windows-11-arm` ist Standard-Hosted-Runner; Fallback wäre self-hosted |
| **Linux ARM64 nativ bauen** | 🟡 Cross reicht | klein | Alternativ-Runner `ubuntu-24.04-arm` existiert (hosted) — aktuell Cross gewählt, da schneller/konsistenter Cache |
| **Linux glibc-Baseline** | 🟡 Ubuntu 22.04 (glibc 2.35) | klein | statische musl-Builds wären noch portabler (offen, nicht priorisiert) |
| **Landing Page** (`site/`) | ✅ alle 6 Artefakte + SHA256 | klein | erledigt (PR #13): Download-Buttons für macOS/Windows/Linux + Verifikation; stabile Artefakt-Namen im CI. Versionierte Releases: [RELEASE-ROADMAP.md](RELEASE-ROADMAP.md) M2 |
| **Autoupdater (M6 v1 + Phase C)** | ✅ Linux/Windows: Check + signierter Download + atomarer Austausch + Rollback; ✅ macOS: DMG-Self-Install (Mount → `.app`-Ersetzung → Unmount); ✅ Auto-Apply-Scheduler + Self-Restart | mittel | erledigt (PR #14 + Phase C): Ed25519-signiertes `SHA256SUMS` (minisign) im Publish-Job; Opt-out (`--no-autoupdate`/Env/Config); `.bak` + Health-Grace + Auto-Rollback + Crash-Loop-Breaker. Offen: Delta-Updates, macOS-Notarization (M4, siehe Zeile oben) |

## Verifikations-Stand

- `cargo test` auf Linux: grün (siehe CI + lokale Läufe).
- Server-Smoke auf Linux: `serve --no-browser` bootet, `/api/storage/status` antwortet.
- CI: 5 Build-Jobs (Linux x64/arm64, Windows x64/arm64, macOS universal) + Publish-Job.
- Artefakt-Nachweis: `latest-main`-Release enthält Dateien im Schema
  `momos-music-manager-<version>-<os>-<arch>.<ext>` inkl. `.sha256` und aggregierter `SHA256SUMS`.
- Autoupdater (M6 v1): Update-/Verifikations-Logik unit-getestet (minisign-Fixtures,
  Signatur/SHA256/Manifest-Parsing, Swap-/Rollback-State-Machine) + End-to-End gegen
  lokalen HTTP-Server und `serve`-Startup (Health-Commit & Auto-Rollback manuell
  verifiziert, siehe PR #14).

## Repo-Struktur (relevant)

```
scripts/package-linux.sh      # Linux tar.gz + SHA256SUMS (nativ oder cross)
scripts/package-windows.ps1   # Windows zip + SHA256
scripts/package-macos.sh      # macOS Universal-DMG (bestehend)
scripts/minisign.pub          # Öffentlicher Ed25519-Schlüssel (Autoupdater, M6)
.github/workflows/build-all.yml  # Matrix: 5 Build-Jobs + Publish (inkl. stabiler `-latest-`-Namen + Manifest-Signatur)
docs/PLATFORM-SUPPORT.md      # dieses Dokument
docs/RELEASE-ROADMAP.md       # iterative Roadmap: Downloads, Signing, Notarization, AppImage, Autoupdate
deploy/momos-music-manager.service  # systemd-Unit (Server-Modus)
```
