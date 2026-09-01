# Versioning, Channels & Releases

Dieses Dokument beschreibt das nachhaltige Versioning-Konzept: wie die
Version eines Builds bestimmt wird, welche Kanäle der Autoupdater kennt und
wie ein Release ausgerollt wird. Stand: 2026-08-31.

## 1. Version eines Builds

| Build-Typ | Auslöser | Formel | Beispiel |
|---|---|---|---|
| **Release** | Tag `v*` auf `main` | Tag ohne führendes `v` | `v1.2.0` → `1.2.0` |
| **Dev** (rolling) | Push auf `main` (CI), jeder lokale Build | `<Cargo.toml-Version>-dev+<sha8>` | `1.1.0-dev+4eaa1d93` |

**Der Tag ist die Wahrheit für Releases.** Die CI liest die Version nie aus
`Cargo.toml`, sondern aus `scripts/resolve-version.sh` — der einzigen
Versionsquelle für CI und Packaging:

- `GITHUB_REF == refs/tags/v*` → Tag-SemVer (z. B. `v1.1.0` → `1.1.0`)
- `GITHUB_REF == refs/heads/main` (oder kein `GITHUB_REF`, z. B. lokal) →
  `<cargo-metadata-Version>-dev+<git rev-parse --short=8 HEAD>`

`Cargo.toml` bleibt die Basis für Dev-Builds und wird per Runbook (unten)
„lose synchron" vor dem Tagen auf die Release-Version gehoben. Ein
vergessener Bump bricht nichts: Der Tag-Build ignoriert ihn, Dev-Builds
bleiben nur bis zum nächsten Bump auf der alten Basis (harmlos, da
Dev-Updates über den SHA-Vergleich laufen, siehe Abschnitt 3).

### Mechanik

- `build.rs` liest die Env-Var `MMM_VERSION` (Fallback:
  `CARGO_PKG_VERSION`) und setzt `cargo:rustc-env=MMM_VERSION` — damit ist
  `env!("MMM_VERSION")` überall verfügbar (CLI `--version`, `/api/version`,
  Startup-Log, Telemetrie, Autoupdater).
- Die Packaging-Skripte (`scripts/package-{linux,macos}.sh`,
  `scripts/package-windows.ps1`) bevorzugen `MMM_VERSION` und fallen lokal
  auf `cargo metadata` zurück. Damit enthalten Dateinamen, `Info.plist` und
  die `VERSION`-Datei im Archiv die volle Versionszeichenkette inkl.
  Build-Metadaten (`1.1.0-dev+abc1234`).
- SemVer-Konformität: `1.1.0-dev+4eaa1d93` = pre-release `dev` +
  build-metadata `4eaa1d93`. Precedence: `1.1.0-dev+*` < `1.1.0`; alle
  `1.1.0-dev+*` sind untereinander **precedence-gleich** (build-metadata
  wird von SemVer ignoriert) — genau das nutzt der rolling Vergleich.

## 2. Asset-Namen in Releases

| Kanal | Schema | Beispiel |
|---|---|---|
| Release (Tag `v1.2.0`) | `momos-music-manager-<semver>-<os-arch>.<ext>` (+ `.sha256`) | `momos-music-manager-1.2.0-linux-x64.tar.gz` |
| Dev (main, SHA `abc1234`, Basis `1.1.0`) | `momos-music-manager-<basis>-dev+<sha8>-<os-arch>.<ext>` (+ `.sha256`) | `momos-music-manager-1.1.0-dev+abc1234-linux-x64.tar.gz` |
| Dev, stabile Namen (nur `latest-main`, für Landing Page + Autoupdater) | `momos-music-manager-latest-<os-arch>.<ext>`, `Momo-s-Music-Manager-latest.dmg` (+ `.sha256`) | `momos-music-manager-latest-linux-x64.tar.gz` |
| Beide | `SHA256SUMS` (aggregiert) + `SHA256SUMS.minisig` (signiert) | — |

Das `+` ist auf allen Zielplattformen und in GitHub-URLs sicher. `latest-main`
wird bei jedem main-Push per `--clobber` aktualisiert; ein Cleanup-Step
entfernt anschließend alte versionierte Assets (Version ≠ aktuell) und
Legacy-Namen (`Momo.s-Music-Manager-*`). Stabile Namen, `SHA256SUMS` und
`SHA256SUMS.minisig` bleiben erhalten.

## 3. Autoupdater-Kanäle

Der Kanal eines Builds ist eine Eigenschaft seiner **Version**: Version mit
pre-release (`-dev+`) → Dev-Build; ohne → Release-Build.

| Kanal | Basis-URL (Default) |
|---|---|
| Dev-Build | `…/releases/download/latest-main` (`DEFAULT_BASE_URL`) |
| Release-Build | `https://github.com/momokli/momos-music-manager/releases/latest` (`DEFAULT_RELEASE_BASE_URL`; GitHub leitet auf das neueste Non-Prerelease-Release um) |

`MOMOS_AUTOUPDATE_BASE_URL` / `[autoupdate] base_url` überschreibt den
Kanal-Default.

Verhalten:

- **Kanal-Guards:** Ein Dev-Build aktualisiert **nie** automatisch auf eine
  Stable-Release-Version und umgekehrt (dev ↔ release → `ChannelMismatch`,
  in `update check` und `update apply` mit Handlungs-Hinweis ausgegeben).
- **Rolling-Vergleich (Dev):** `latest > current` → Update verfügbar;
  precedence-gleich **und** andere Versionszeichenkette (neuer SHA) → Update
  verfügbar; sonst up to date. Release: nur `latest > current`.
- **Artefakt-Auflösung:** über den **versionierten** Namen
  (`momos-music-manager-<version>-<os-arch>.<ext>`), abgeleitet aus der
  geparsten Manifest-Version (`Version::to_string()` erhält die
  build-metadata). Der stabile `-latest-`-Name bleibt nur für die
  Download-Buttons der Landing Page relevant (CI-seitig).
- `update status` zeigt u. a. den Kanal (Basis-URL).

## 4. Release-Runbook

1. `CHANGELOG.md` pflegen (Unreleased-Eintrag nach `## [Unreleased]`).
2. `Cargo.toml` auf die Release-Version heben (z. B. `1.2.0`) — derselbe
   Commit hebt auch `site/index.html` (Hero + Footer) an.
3. Commit + Push auf `main`.
4. Tag erstellen und pushen:

   ```bash
   git tag v1.2.0 && git push origin v1.2.0
   ```

   `build-all.yml` baut alle Plattformen und publiziert die Assets
   `momos-music-manager-1.2.0-*` plus signiertes Manifest.
5. Verifizieren:

   ```bash
   gh release view v1.2.0 --json assets -q '.assets[].name'
   gh release view v1.2.0 -p SHA256SUMS.minisig
   ```

**Warnung:** Nie `build-all.yml` per `workflow_dispatch` auf einem **alten
Tag** ausführen — alter Code + neuer Workflow erzeugt Assets `1.2.0` mit
Binaries, die eine andere Version melden (Fehllabeling). Für Alt-Tag-Repairs
existiert `repair-release.yml` (Abschnitt 5).

## 5. v1.1.0-Repair (Alt-Tag-Nachbesserung)

Das Release `v1.1.0` trägt Assets `momos-music-manager-1.0.1-*`, weil der
Tag-Build die Version noch aus `Cargo.toml` (1.0.1) las. Der Workflow
`.github/workflows/repair-release.yml` benennt die Assets auf den Tag um,
regeneriert `.sha256` + `SHA256SUMS`, signiert neu und löscht die alten
Assets:

```bash
# GitHub UI: Actions → Repair release assets → Run workflow
# tag: v1.1.0, old_version: 1.0.1 (Default)
```

Danach zeigt `gh release view v1.1.0 --json assets -q '.assets[].name'` nur
noch `momos-music-manager-1.1.0-*` + `SHA256SUMS`(+`.minisig`).

**Grenze:** Die *eingebettete* Versionszeichenkette (`--version`,
`/api/version`) der v1.1.0-Binaries bleibt `1.0.1` — sie wurde beim Tag-Build
mit altem Code gebacken und kann ohne Rebuild nicht korrigiert werden (ein
Rebuild vom Tag erzeugt wieder `1.0.1`, da der alte `build.rs` `MMM_VERSION`
ignoriert). Der Repair ist daher Kosmetik/Konsistenz (Asset-Namen +
signiertes Manifest), kein inhaltlicher Fix — das behebt sich erst mit dem
nächsten Release.

## 6. Update-Status in der UI

Seit v1.1.1 zeigt die **Settings-Seite** (`#settings`, Nav-Sektion
„System") den Update-Status und steuert die Autoupdater-Einstellungen.
Backend: Modul `src/api/update.rs`, Persistenz in der SQLite-KV-Tabelle
`settings` (Migration 024).

### Endpoints

| Endpoint | Zweck |
|---|---|
| `GET /api/update/status` | Version, Kanal (dev/release), effektiver Enabled-Wert + Quelle, Artifact, letzter Check (`lastCheckAt`, `lastCheckStatus`, `lastCheckResult`, `lastCheckError`), `pendingUpdate` (aus `update-state.json`), `platformSelfInstall` |
| `POST /api/update/check` | Führt einen verifizierten Check aus (30-s-HTTP-Timeout), persistiert das Ergebnis und gibt den frischen Status zurück. Netz-/Signaturfehler sind **kein** HTTP-Fehler: `lastCheckStatus: "error"` bei 200 |
| `POST /api/update/settings` | Body `{"autoUpdateEnabled": bool}` — persistiert den Toggle. **409**, wenn Env/TOML den Wert fixieren; **400** bei ungültigem Body |
| `POST /api/update/apply` | Manuelles „Update now": Linux/Windows → Swap + `{outcome: "installed", restartNeeded: true}`; macOS → verifizierter DMG-Download + Pfad (`outcome: "downloaded"`, **kein** Self-Install). 409 bei disabled/Kanal-Mismatch, 404 wenn kein Update verfügbar |

### Precedence-Regel (Env > UI > TOML > Default **true**)

1. `MOMOS_AUTOUPDATE_ENABLED` gesetzt **und** parsebar → gewinnt (`env`)
2. sonst UI-Wert in `settings['autoupdate.enabled']` → gewinnt (`ui`)
3. sonst `[autoupdate] enabled` aus `config.toml` → gewinnt (`toml`)
4. sonst Default **true** (`default`)

Die Status-Response enthält `enabledSource`; bei `env`/`toml` rendert die
UI den Toggle **disabled** mit Hinweis („von Umgebungsvariable gesetzt" /
„von config.toml gesetzt"). Unparsebare Env-Werte fallen wie bisher durch
(kein Breaking Change).

### Toggle-Persistenz

Der Toggle wird in SQLite persistiert (`settings`-KV, Key
`autoupdate.enabled`, Werte `"true"`/`"false"`) und überlebt Neustarts.
Der Start-Check in `serve()` liest den **effektiven** Wert aus der DB
(nicht mehr nur `config.autoupdate_enabled`) und persistiert sein Ergebnis
(`autoupdate.last_check_*`) — `--no-autoupdate` hat weiterhin höchste
Priorität.

### macOS v1-Limitation

„Update now" auf macOS lädt nur den verifizierten DMG nach `~/Downloads`
und zeigt eine Installations-Anleitung — DMG-Mount, `.app`-Ersetzung und
Neustart sind **Phase C** (Auto-Apply + Self-Restart, geplant als eigenes
Feature nach v1.1.1). `platformSelfInstall: false` kennzeichnet das in der
Status-Response.

### Kanal-Mismatch

Dev- und Release-Builds wechseln nie automatisch die Kanäle: Bei Mismatch
(dev ↔ release) zeigt die UI einen Erklärtext, der Apply-Button erscheint
nicht (`POST /api/update/apply` → 409).
