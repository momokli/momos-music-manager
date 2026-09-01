# Progress: Update-Settings-View

**Spec**: /home/node/.openclaw/workspaces/coding-orchestrator/plans/proposed/update-settings-view.md
**Produktentscheidung (2026-09-01)**: „Auto-Update an" = automatisch anwenden + neu starten (Phase C).
**Repo**: /home/node/repos/momos-music-manager — main @ 03ed42e (v1.1.0)
**PR-Strategie**: PR 1 = Phase A+B (Branch `feat/update-settings-view`), PR 2 = Phase C (Branch `feat/update-auto-apply`, nach Merge von PR 1)

## PR 1 — Phase A+B: Status-View, Check now, Toggle+Persistenz, manuelles Update now

| Stage | Agent | Status | Notizen |
|---|---|---|---|
| 1. Plan | feature-dev-planner | done | Spec → User Stories (7 Stories, 024-Schema, Precedence-Regel, Testplan) |
| 2. Setup | feature-dev-setup | done | Branch `feat/update-settings-view` von main @ 03ed42e; Build-Baseline grün (cargo build 225 s, lib: 29 Warnungen, v. a. dead_code, keine Errors; cargo test --no-run kompiliert, 96 s); letzte Migration: 023; Toolchain: Rust 1.98.0 via rustup installiert (System-cargo 1.65 konnte edition 2024 nicht parsen) |
| 3. Dev | feature-dev-developer | pending | Backend + Frontend + Tests |
| 4. Verify | feature-dev-verifier | pending | Quality Gate |
| 5. Test | feature-dev-tester | pending | Integration/E2E |
| 6. PR | feature-dev-developer | pending | Push + PR |
| 7. Review | feature-dev-reviewer | pending | Final Review |

### PR 1 — Plan (Phase A+B, Code-Analyse-Basis: main @ 03ed42e)

**Scope**: Status-View + „Check now" + Auto-Update-Toggle (Persistenz) + manuelles „Update now" (Apply-Endpoint).
**Phase C** (echtes Auto-Apply + Neustart, macOS Self-Install) ist **PR 2** und wird hier NICHT geplant — Abhängigkeit: PR 2 liest `settings['autoupdate.enabled']` (Toggle-Persistenz) + den Precedence-Helper als Steuergröße für den Auto-Apply-Intervall; der Toggle-Mechanismus aus US-4 ist dafür die Grundlage.

#### Architektur-Fakten aus der Code-Analyse (verbindlich für den Developer)

- **Autoupdater-API** (`src/autoupdate/`): `UpdateSettings::from_config(&ServiceCredentials)` baut Settings (base_url channel-abhängig, `enabled`, `health_grace_secs`, `artifact` via `platform::current_artifact()`); `UpdateStatus::check(&settings, &fetcher)` → `UpdateStatus::{UpToDate, UpdateAvailable(info), ChannelMismatch{..}, UnsupportedPlatform, Disabled}`; `UpdateStatus::apply(...)` → `ApplyOutcome::{Installed{new_version, old_version}, DownloadedOnly{path, version}}`; `swap::read_marker(&swap::exe_dir())` → `Option<UpdateMarker{old_version, new_version, start_count, committed}>`; beides generisch über `Fetcher`-Trait (`HttpFetcher` prod, `verify::tests::MockFetcher` pub(crate) für Tests). **Achtung**: `HttpFetcher::new()` hat KEINEN HTTP-Timeout (reqwest-Default = ∞) — für den API-Handler muss ein Timeout-Client (15–30 s) her.
- **AppState** (`src/lib.rs:54`): `db: Pool<Sqlite>` + `config: ServiceCredentials` + task_manager/embeddings — Handler bekommen beides via `State(state): State<Arc<AppState>>`; **kein neuer Store-Handle nötig**, `state.db` + neues `db::settings`-Modul reichen.
- **Migrationen**: `sqlx::migrate!()` embedded (kompiliert `migrations/` ein, automatisch aktiv) — neue Datei `migrations/024_settings.sql` genügt; Konvention: `NNN_name.sql` + abschließende `SELECT '...' as status;`-Zeile (siehe 022/023).
- **serve()** (`src/main.rs:385`): `au_enabled` wird bei Z. 454–455 aus `config.autoupdate_enabled` gelesen; Start-Check-Task Z. 744–763 (10 s Delay) ruft `ServiceCredentials::load()` **nochmal frisch** auf — hier muss stattdessen der effektive Enabled-Wert (DB) eingelesen und das Check-Ergebnis persistiert werden.
- **Frontend**: Hash-Router `frontend/app.js` PAGE_MAP (Z. 14–40), Nav `frontend/shared/nav.js` (NAV_SECTIONS + TOOLS_ITEMS, `renderSection` rendert 1-Item-Sektion als Direktlink), Pages = `init(container, signal)` (Muster `frontend/pages/storage.js`), `fetchJSON` aus `shared/api.js` (wirft bei !res.ok), Helfer `showToast/showConfirmModal/renderLoading/renderErrorBlock` aus `shared/components.js`. Backend-Antworten folgen `ApiResponse{ data }` / `ErrorResponse{ error }` (camelCase).
- **Tests**: `frontend/playwright.config.js` startet `cargo run serve` auf Port 3001 mit `DATABASE_URL=sqlite:test-playwright.db`; Muster `frontend/tests/storage.spec.js` (page.route-Interception, pageerror-Array). Rust-Unit-Tests: `SqlitePool::connect("sqlite::memory:")` + rohe CREATE TABLE (Muster `src/db/files.rs:2471ff`); es gibt **kein** AppState-Handler-Test-Muster in `src/api/`.

#### Migration 024 — Schema

`migrations/024_settings.sql` (KV-Tabelle, Keys als TEXT):

```sql
-- Migration 024: App-Settings (KV) — z. B. Autoupdate-Toggle + letzter Check
CREATE TABLE IF NOT EXISTS settings (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
SELECT 'Migration 024 applied: settings KV table' as status;
```

Keys (Namespace `autoupdate.`): `autoupdate.enabled` ("true"/"false"), `autoupdate.last_check_at` (unix seconds als INTEGER-String), `autoupdate.last_check_status` ("ok"/"error"), `autoupdate.last_check_result` (JSON des serialisierten Status/Fehlers).

#### Precedence-Regel (verbindlich): Env > UI > TOML > Default **true**

1. `MOMOS_AUTOUPDATE_ENABLED` gesetzt **und** parsebar → gewinnt (source=`env`)
2. sonst `settings['autoupdate.enabled']` (DB/UI) vorhanden → gewinnt (source=`ui`)
3. sonst `config.autoupdate_enabled` aus config.rs — der ist bereits Env(>TOML)>TOML>Default true; da Env in Schritt 1 abgefangen wurde, ist hier TOML bzw. Default (source=`toml` bzw. `default`)
4. Unparsebare Env-Werte fallen wie bisher in config.rs durch (Verhalten beibehalten)

UI-Hinweis: `enabledSource` in der Status-Response; bei `env`/`toml` rendert die Settings-Page den Toggle **disabled** + Hinweis („von config.toml gesetzt" / „von Umgebungsvariable gesetzt"). TOML-Detection braucht eine neue Methode in `src/config.rs` (Muster `credential_source()`): z. B. `pub fn autoupdate_enabled_source(&self) -> &'static str` (TomlConfig ist privat).

#### User Stories (in Reihenfolge)

1. **[US-1] DB-Foundation: Migration 024 + `db::settings`-KV-Modul**
   - Akzeptanzkriterien: `migrations/024_settings.sql` angelegt (Schema s. o.); neues Modul `src/db/settings.rs` mit `get_setting(pool, key) -> Result<Option<String>>`, `set_setting(pool, key, value)` (UPSERT), `get_bool`/`set_bool`; in `src/db/mod.rs` als `pub mod settings` + Re-Export registriert; `cargo test` grün; frische DB migriert sauber (sqlx::migrate läuft automatisch durch).
   - Betroffene Dateien: `migrations/024_settings.sql` (neu), `src/db/settings.rs` (neu), `src/db/mod.rs`
   - Geschätzte Lines of Code: ~70 (SQL ~10, Modul ~40, Tests ~20)

2. **[US-2] Precedence-Helper + `GET /api/update/status`**
   - Akzeptanzkriterien: Precedence-Helper `effective_autoupdate_enabled(config, db)` + `enabled_source` (Regel s. o.); neues Modul `src/api/update.rs` (Router mit 4 Routen, in `src/api/mod.rs` via `.merge(update::router())`); `GET /api/update/status` liefert 200 mit JSON (s. u.); Marker via `swap::read_marker(&swap::exe_dir())`; `platformSelfInstall = artifact.ext != "dmg"`; Kanal „dev" wenn `MMM_VERSION` Pre-Release hat, sonst „release" (semver-parse); Fehler beim Marker-Lesen → `pendingUpdate: null` + `pendingUpdateError` statt 500.
   - Response `GET /api/update/status` → 200 `{ "data": { "currentVersion": "1.1.0-dev+abc1234", "channel": "dev"|"release", "baseUrl": "…", "enabled": true, "enabledSource": "env"|"ui"|"toml"|"default", "artifact": { "osArch": "linux-x64", "ext": "tar.gz" }, "lastCheckAt": 1756742400|null, "lastCheckStatus": "ok"|"error"|null, "lastCheckError": "…"|null, "lastCheckResult": { "state": "upToDate"|"updateAvailable"|"channelMismatch"|"disabled"|"unsupportedPlatform"|"error", "availableVersion": "…"|null, "currentVersion": "…"|null, "artifactName": "…"|null }, "updateAvailable": false, "pendingUpdate": { "oldVersion": "…", "newVersion": "…", "committed": false }|null, "platformSelfInstall": true|false } }`
   - Betroffene Dateien: `src/api/update.rs` (neu), `src/api/mod.rs`, `src/config.rs` (source-Methode)
   - Geschätzte Lines of Code: ~170 (Helper ~40, Handler+Types ~60, Tests ~70)

3. **[US-3] `POST /api/update/check` + Start-Check-Integration in `serve()`**
   - Akzeptanzkriterien: Handler baut `UpdateSettings::from_config(&state.config)` (Klone liegen in AppState) + `HttpFetcher` mit `.timeout(30s)` (Timeout-Client in `verify.rs` ergänzen ODER im Handler bauen); führt `UpdateStatus::check` aus; persistiert `last_check_at` (unix), `last_check_status` (ok/error), `last_check_result` (JSON) via `db::settings`; gibt dieselbe Status-JSON wie US-2 zurück (aus persistiertem Wert); `serve()`-Start-Check (Z. 744–763): liest effektiven Enabled-Wert aus DB (nicht mehr nur `config.autoupdate_enabled`), übergibt `db`-Clone in den Task, persistiert ebenfalls lastCheck; `--no-autoupdate`-Flag bleibt höchste Priorität; Verhalten: Disabled/ChannelMismatch sind **keine** Fehler (lastCheckStatus ok, state entsprechend); Netz-/HTTP-Fehler → lastCheckStatus=error, HTTP 200 mit error-State im JSON (fetchJSON-Konvention), kein 5xx für Check-Fehler.
   - Response `POST /api/update/check` → 200 `{ "data": { …Status-JSON wie US-2, frisch aktualisiert… } }`
   - Betroffene Dateien: `src/api/update.rs`, `src/autoupdate/verify.rs` (Timeout), `src/main.rs` (serve)
   - Geschätzte Lines of Code: ~150 (Handler ~50, serve-Änderung ~30, Tests ~70)

4. **[US-4] `POST /api/update/settings` — Toggle-Persistenz**
   - Akzeptanzkriterien: Body `{ "autoUpdateEnabled": true|false }` (camelCase, `#[derive(Deserialize)]`); persistiert `settings['autoupdate.enabled']` via `set_bool`; Response enthält effektiven Wert + source; wenn source `env`|`toml` (Override) → **409** `{ "error": "autoupdate.enabled wird über config.toml/Umgebungsvariable gesetzt — im UI nicht änderbar" }`, nichts wird geschrieben; Body fehlt/ungültig → 400; Roundtrip über Neustart (DB); Precedence-Regel unverändert.
   - Response `POST /api/update/settings` → 200 `{ "data": { "autoUpdateEnabled": true, "enabled": true, "enabledSource": "ui" } }`; 400/409 `{ "error": "…" }`
   - Betroffene Dateien: `src/api/update.rs`
   - Geschätzte Lines of Code: ~100 (Handler ~50, Tests ~50)

5. **[US-5] `POST /api/update/apply` — manuelles „Update now"**
   - Akzeptanzkriterien: führt `UpdateStatus::apply` aus (eigener Check inkl. Download+Verifikation, bestehende Logik); effektiv disabled → 409; `ApplyOutcome::Installed` → 200 `{ "data": { "outcome": "installed", "newVersion": "…", "oldVersion": "…", "restartNeeded": true } }` (Linux/Windows, Swap passiert wie im CLI); `DownloadedOnly` → 200 `{ "data": { "outcome": "downloaded", "path": "/Users/x/Downloads/momos-music-manager-….dmg", "version": "…" } }` (macOS — nur verifizierter Download + Anleitung, **kein** DMG-Mount/Self-Install, das ist Phase C); `UpdateError::ChannelMismatch` → 409 mit erklärendem error-Text (dev↔release, Muster CLI-Text main.rs Z. 302–312); `NoUpdate` → 404 `{ "error": "no update available" }`; andere Fehler → 500 via `internal_error`; aktualisiert lastCheck wie US-3.
   - Betroffene Dateien: `src/api/update.rs`
   - Geschätzte Lines of Code: ~130 (Handler ~60, Tests ~70)

6. **[US-6] Frontend: Settings-Page mit Update-Karte**
   - Akzeptanzkriterien: `PAGE_MAP` + `settings: "settings"`; neue Nav-Sektion (z. B. „System" mit Item `{ id: "settings", label: "Settings", icon: "fa-gear" }` — 1-Item-Sektion wird als Direktlink gerendert); neue Page `frontend/pages/settings.js` mit `export async function init(container, signal)` (Muster storage.js): Karte „Updates" mit Version (aus Status-Endpoint, inkl. Build-Meta), Kanal-Badge (dev/release + baseUrl), Status-Badge (up to date / Update verfügbar / Kanal-Mismatch / deaktiviert / Fehler), „Letzter Check" (Zeit + Ergebnis), Auto-Update-Toggle (default on; **disabled + Hinweis bei enabledSource env/toml**), Buttons „Check now" (Spinner, Ergebnis inline, Fehler inline) und „Update now" (nur sichtbar/enabled bei updateAvailable && !channelMismatch && enabled; `showConfirmModal` vor Apply; danach: restartNeeded-Hinweis bzw. macOS-DMG-Anleitung mit Pfad); Kanal-Mismatch-Erklärtext (dev↔release) statt Apply-Button; kein JS-Fehler auf der Page.
   - Betroffene Dateien: `frontend/app.js`, `frontend/shared/nav.js`, `frontend/pages/settings.js` (neu)
   - Geschätzte Lines of Code: ~260

7. **[US-7] Playwright-E2E + Doku**
   - Akzeptanzkriterien: `frontend/tests/update-settings.spec.js` (Details Testplan); `docs/versioning.md` neuer Abschnitt **„6. Update-Status in der UI"** (nach Abschnitt 5): Endpoints, Precedence-Regel Env > UI > TOML > Default true, Toggle-Persistenz in SQLite, Hinweis dass macOS v1 nur verifizierten Download liefert, Verweis auf Phase C (Auto-Apply + Neustart) als kommendes Feature; `npm test` (Playwright) grün, `cargo test` grün, `cargo build` grün.
   - Betroffene Dateien: `frontend/tests/update-settings.spec.js` (neu), `docs/versioning.md`
   - Geschätzte Lines of Code: ~200 (Spec ~160, Docs ~40)

#### Reihenfolge + Abhängigkeiten

`US-1 → US-2 → US-3 → US-4 → US-5 → US-6 → US-7` (strikt sequenziell; jede Story hinterlässt kompilierbaren, testbaren Stand):
- US-2 braucht US-1 (lastCheck aus DB), definiert Precedence-Helper + Status-JSON-Shape (von US-3/US-4/US-5 wiederverwendet)
- US-3 braucht US-2 (Shape + Helper); US-4 braucht US-2 (Precedence); US-5 unabhängig bis auf Router, aber erst nach US-3 sinnvoll (lastCheck-Updates)
- US-6 braucht US-2…US-5 (alle Endpoints); US-7 braucht alles

#### Offene Fragen / Entscheidungen für den Developer

1. **AppState-Handler-Tests**: Es gibt kein Test-Muster für Handler in `src/api/` (kein AppState-Konstruktor ohne TaskManager/Embeddings). Empfehlung: Check-/Precedence-/Persistenz-Logik in pure Funktionen (`&Pool`, `&ServiceCredentials`, `&dyn Fetcher`) extrahieren und diese unit-testen; Handler dünn halten. ODER AppState-Test-Helper bauen (mehr Aufwand).
2. **Timeout-Fetcher**: `HttpFetcher` global auf 30 s Timeout setzen (beeinflusst auch CLI) vs. separater Timeout-Client nur im API-Handler. Empfehlung: global in `HttpFetcher::new()` (CLI profitiert ebenfalls).
3. **`update now` synchron vs. Hintergrund-Task**: Download kann &gt;30 s dauern. PR 1-Empfehlung: synchron + UI-Spinner (einfach, kein Task-Pattern); wenn der Verifier Latenz/Blocking bemängelt → Task + `/api/tasks`-Polling (Muster storage prune).
4. **Check bei deaktiviertem Update (Toggle off)**: `check()` liefert `Disabled` (kein Fehler) — soll `POST /api/update/check` trotzdem laufen (Status „deaktiviert" anzeigen) oder 409? Empfehlung: 200 mit state=disabled (ehrliche Anzeige, kein Fehler).
5. **TOML-Detection für `enabledSource`**: TomlConfig ist privat; `autoupdate_enabled_source()` in config.rs nötig. Auch: Env gesetzt aber unparsebar → aktuell Fallthrough zu TOML/Default — beibehalten oder als `env` (unparsebar) behandeln? Empfehlung: beibehalten (kein Breaking Change).
6. **`lastCheckResult`-Persistenz-Format**: JSON-String in `settings`-KV (einfach) vs. eigene Spalten/Zeile. Empfehlung: KV-JSON (Tabelle bleibt generisch, Phase C nutzt dieselbe Tabelle).
7. **`platformSelfInstall`-Benennung**: Feld heißt laut Spec so — Semantik = „Plattform kann Binary selbst austauschen" (true Linux/Windows, false macOS). Falls Verifier/Reviewer es missverständlich finden → Umbenennung vor Merge abstimmen.

#### Testplan-Skizze

**Rust-Unit (`cargo test`)**:
- `db::settings`: set/get-Roundtrip, Upsert überschreibt, fehlender Key → None, bool-Parse ("true"/"false"/Müll → Err)
- Precedence-Matrix: (env gesetzt/fehlt) × (ui gesetzt/fehlt) × (toml gesetzt/fehlt) → erwarteter effektiver Wert + `enabledSource`; Default true ohne alles
- Status-Endpoint-Logik: Marker vorhanden (`swap::read_marker` mit Temp-Dir, `install_dir`-Muster aus `verify.rs`-Tests) / fehlt; lastCheck leer vs. gefüllt; channel dev/release aus MMM_VERSION-Parse
- Check-Persistenz: `MockFetcher` (aus `verify::tests`, pub(crate)) mit signiertem Fixture (Muster `signed_fixture`) → lastCheckStatus ok + state updateAvailable; kaputtes Fixture → error
- Apply-Mapping: `Installed` → restartNeeded true; `DownloadedOnly` → outcome downloaded + path (macOS-Zweig; auf CI nur Linux, also Logik-Test statt echter DMG)

**Playwright (`frontend/tests/update-settings.spec.js`, Muster storage.spec.js)**:
- Page lädt ohne pageerror; Nav-Eintrag „Settings" existiert, `#settings`-Route erreichbar
- Status-Card zeigt Version + Kanal (echte API, test-playwright.db frisch → default true, kein lastCheck → „nie")
- „Check now": Klick → Spinner → Ergebnis inline (realer Check gegen latest-main; auf Offline-CI via `page.route("**/api/update/check")` stubben)
- Toggle: an/aus → API-Aufruf `POST /api/update/settings` sichtbar (page.route capture); nach Reload persistiert (DB)
- Override-Hinweis: `page.route` liefert `enabledSource: "toml"` → Toggle disabled + Hinweistext sichtbar
- „Update now": Button nur bei `updateAvailable: true` sichtbar; Stub-Response outcome=installed → Restart-Hinweis; outcome=downloaded → DMG-Pfad + Anleitung; channelMismatch → Erklärtext, kein Apply-Button

#### Phase-C-Abhängigkeit (PR 2, NICHT hier umsetzen)

PR 2 (Auto-Apply + Neustart) konsumiert: `settings['autoupdate.enabled']` + Precedence-Helper (effektiver Wert), `last_check_*` (für Intervall-Logik), und erweitert macOS-Apply um DMG-Mount + `.app`-Ersetzung + launchctl-kickstart. Der PR-1-Code muss diese Stellen sauber exportiert lassen (kein Refactoring-Bedarf für C).

## PR 2 — Phase C: echtes Auto-Update (Auto-Apply + Neustart, macOS Self-Install)

| Stage | Agent | Status | Notizen |
|---|---|---|---|
| 1. Plan | feature-dev-planner | pending | Nach Merge von PR 1 |
| 2. Setup | feature-dev-setup | pending | Branch `feat/update-auto-apply` |
| 3. Dev | feature-dev-developer | pending | Intervall-Check, Auto-Apply, Self-Restart, macOS DMG-Swap |
| 4. Verify | feature-dev-verifier | pending | Quality Gate |
| 5. Test | feature-dev-tester | pending | CLI/Unit-Tests für Apply-Pfade; macOS nur kompilieren |
| 6. PR | feature-dev-developer | pending | Push + PR |
| 7. Review | feature-dev-reviewer | pending | Final Review |
