# Plan: Auto-Update Phase C — Auto-Apply + Self-Restart + macOS DMG-Self-Install

Branch: `feat/auto-update-phase-c` · Basis: `main` @ e7a4384 (PR #19 gemergt)
Stand: 2026-09-02 · Produktentscheidung (2026-09-01): „Auto-Update an" =
automatisch anwenden + neu starten (Phase C).

---

## 0. Rekonstruktion (Alt-Arbeit gesucht)

Frühere Phase-C-Arbeit (Module `restart.rs`/`dmg.rs`/`macos.rs`/
`update_auto.rs`, Scheduler-Config, Interval-Precedence) lag uncommitted in
`/tmp/mmm-phase-c` und ist weg (tmp geleert). Geprüft:

- `git ls-remote --heads` (origin): **kein** Phase-C-/Auto-Apply-Branch.
- Lokale Clones (`/home/node/repos/*`): Commits/Reflogs/Dangling Objects ohne
  Phase-C-Artefakte; keine Datei `restart.rs|dmg.rs|macos.rs|update_auto.rs`
  in irgendeinem Commit.
- Workspace-Clone (`/home/node/.openclaw/workspace/momos-music-manager`, READ
  ONLY — battery-friendly-tray-Arbeit): nichts zu Phase C.

**Ergebnis: keine Alt-Arbeit übernommen — sauber neu aufgesetzt** und aus
diesem Plan + Task-Spec rekonstruiert. (Der Fortschritt von
`feat/update-settings-view` dokumentiert dieselbe Produktentscheidung und
bestätigt den Scope: „PR 2 liest `settings['autoupdate.enabled']` als
Steuergröße für den Auto-Apply-Intervall".)

---

## 1. Zielbild

1. **Scheduler**: periodischer Update-Check/Apply im konfigurierbaren
   Intervall. Precedence konsistent zu Toggle/Kanal:
   **Env > UI (DB) > TOML > Default 4 h** (`0` = aus; Startup-Check läuft
   weiter). Konfig: `MOMOS_AUTOUPDATE_INTERVAL_SECS`, `[autoupdate]
   interval_secs`, UI-Dropdown (Settings-KV `autoupdate.interval_secs`).
2. **Self-Installing**: nach erfolgreichem Apply automatischer Neustart mit
   Sicherheits-Guards (kein Endlos-Restart-Loop bei fehlgeschlagenem Update):
   In-Flight-Guard (Swap-Marker), Crash-Loop-Breaker (persistierter
   Auto-Apply-State, aktiviert beim Startup-Auto-Rollback), systemd-Erkennung.
3. **DMG-Handling (macOS)**: verifizierten DMG mounten, `.app`-Bundle
   atomar ersetzen (Ziel `/Applications` oder konfigurierbar via
   `MOMOS_AUTOUPDATE_APP_DIR` / `[autoupdate] app_dir`), unmounten,
   Alt-Version als `.updater-bak` aufbewahren + beim nächsten Install
   aufräumen; Fallback = verifizierter Download nach `~/Downloads`.

Kompatibilität Telemetry-PR #20 (`app.updated` bei Versionswechsel):
`verify::apply`-Signatur, Swap-Pfad und `ApplyOutcome`-Schema unverändert;
der DMG-Self-Install-Zweig gibt dasselbe `Installed` zurück; seit #20
(gemergt während Phase C) wird `app.updated` in **beiden** Zweigen emittiert.

## 2. Umsetzung

| Datei | Änderung |
|---|---|
| `src/autoupdate/update_auto.rs` (neu) | Intervall-Precedence (`effective_auto_apply_interval_with_env`), Auto-Apply-State (KV `autoupdate.auto_apply_state`), Breaker (`record_rollback`, `skip_reason`), `AutoApplyOutcome` |
| `src/autoupdate/restart.rs` (neu) | `plan_auto_restart` (systemd / exec / macOS `open` / Skip), detachter Relauncher mit 2-s-Delay |
| `src/autoupdate/macos.rs` (neu) | `.app`-Bundle-Discovery (`running_app_bundle`, `find_app_bundle`), Installations-Verzeichnis (`/Applications`), Bundle-Layout |
| `src/autoupdate/dmg.rs` (neu) | `install_dmg`: hdiutil attach → Bundle suchen → `ditto`/Copy → Staging + Backup-Swap mit Restore → detach; `parse_mount_point` |
| `src/autoupdate/verify.rs` | `UpdateSettings.app_install_dir`; DMG-Zweig: Self-Install-Versuch, bei Fehler Fallback `DownloadedOnly` (unverändertes Outcome-Schema) |
| `src/config.rs` | `autoupdate_interval_secs`/`autoupdate_interval_toml` (env>toml>Default 4 h), `autoupdate_app_dir`; `interval_source()`; Doku-Header; Log-Zeile |
| `src/db/settings.rs` | Keys `autoupdate.interval_secs`, `autoupdate.auto_apply_state` |
| `src/api/update.rs` | Status-JSON + `autoApplyIntervalSecs`/`Source`; `POST /api/update/settings` akzeptiert `autoApplyIntervalSecs` (Pinning 409); `run_auto_apply_cycle` (Scheduler-Orchestrierung); `platformSelfInstall` true auf macOS |
| `src/main.rs` | Scheduler-Loop in `serve()` (Intervall je Zyklus neu lesen, 0 = stop); Self-Restart nach `Installed`; Breaker-Events im Startup-Recovery (Commit → State leeren, Rollback → Breaker aktivieren) |
| `frontend/pages/settings.js` | Auto-Apply-Intervall-Select (Presets Off/1 h/4 h/12 h/24 h, Pinning-Hinweise, Auto-Restart-Erklärtext), Modal-Text macOS |
| `frontend/tests/update-settings.spec.js` | Status-Stub erweitert + 5 neue Playwright-Tests (Intervall) |
| Doku | CHANGELOG `[Unreleased]`, README, `.env.example`, `docs/versioning.md` §6/§7, `docs/PLATFORM-SUPPORT.md` |

## 3. Guards gegen Restart-Loops (Entscheidungen)

- **In-Flight-Guard**: unbestätigter `update-state.json`-Marker → kein
  weiterer Auto-Apply (kein Stapeln auf laufende Health-Grace).
- **Crash-Loop-Breaker**: Nach Auto-Rollback (Startup-Event) wird der
  fehlgeschlagene Versionsstand gesperrt (`failures = MAX`, analog
  `MAX_UNHEALTHY_STARTS = 2`); dieselbe Version wird nicht erneut
  automatisch angewendet, bis eine **neuere** Version erscheint. Manuelles
  `update apply` bleibt immer möglich.
- **Waiting-for-Activation**: bereits installierte Version (State ohne
  Fehler) wird nicht erneut heruntergeladen, solange der Prozess noch die
  alte Version läuft.
- **systemd**: `INVOCATION_ID` gesetzt → kein eigener Relauncher, Prozess
  beendet sich; `Restart=always` (ausgeliefertes Unit) startet die neue
  Version. Kein Doppelstart.
- **macOS `.app`**: Relaunch nur, wenn das laufende Bundle im
  Installations-Verzeichnis liegt (kein Auto-Start einer falschen Kopie).
- Health-Grace/Commit/Rollback (`swap.rs`) bleibt die erste Verteidigungslinie
  für Linux/Windows; `.updater-bak` auf macOS dient der manuellen
  Wiederherstellung.

## 4. Testplan

- `cargo test`: Intervall-Precedence-Matrix (env×ui×toml×default), Breaker-
  State-Machine, KV-Roundtrip, Status-JSON-Felder, Settings-Write/Pinning,
  Auto-Cycle (Disabled, 404-Fehlerpfad persistiert, Installed zeichnet State
  vor dem Restart auf), DMG-Fallback auf Nicht-macOS, DMG-Unit-Tests
  (Mount-Point-Parsing, Bundle-Ersetzung/Backup/Restore), verify-Tests grün.
- Playwright (`npm test`): bestehende Update-Settings-Specs + 5 neue
  Intervall-Specs grün.
- Screenshots: Settings-Seite mit Intervall-Select (docs/screenshots/).

## 5. Risiken / bewusst offen

- DMG-Mount/`ditto`/`open` laufen nur auf macOS (CI = Linux) → Runtime-Pfad
  durch Unit-Tests der Logik + Fallback abgesichert; manueller E2E auf dem
  Mac steht aus (Momo verifiziert lokal).
- macOS `.app`-Ersetzung braucht Schreibrechte im Ziel-Verzeichnis; ohne
  Rechte greift der `~/Downloads`-Fallback.
- Versionsschema/Kein Bump: unverändert (CHANGELOG `[Unreleased]`).
