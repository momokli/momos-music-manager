# Progress: Issue #506 — periodischer Playlist-Listing-Poll (snapshot_id frisch halten)

Repo: momokli/momos-music-manager
Branch: feat/506-playlist-listing-poll (base: main @ 4acc057)
PR-Ziel: main (unabhängig, frei mergebar). PR-Body MUSS `Closes #506` enthalten.
Fokus-Milestone: hub-tonies-0.32.0

## Issue (Kurzfassung)

Spotify schickt KEINEN Playlist-Change-Webhook. `ingest::fetch_playlists()` (Listing
`/me/playlists`, paged; schreibt `hub_playlists.snapshot_id`) wird nur vom CLI
(`main.rs:634`) und der Web-Route `POST …/fetch` (`web.rs:702`) aufgerufen — **kein Loop**.
Der Worker holt Items nur, wenn `snapshot_id <> IFNULL(last_synced_snapshot,'')`
(`worker.rs:457–470`). ⇒ Playlists stale bis ein Mensch „Fetch" drückt.

## Scope (Soll)

- Neuer Worker-Loop (in `LOOP_NAMES` registrieren, Health-Sichtbarkeit wie andere):
  pro verbundenem User `get_all_items("/me/playlists?limit=50")` → `fetch_playlists`.
- Cadence konfigurierbar über `hub_settings` (Default 10–15 min), 429/Quota-Backoff
  über vorhandene Helfer (`backoff`/`quota_backoff`).
- Ein Lauf darf nicht in den Item-Fetch des `spotify`-Loops hineinlaufen (Reihenfolge
  egal, aber kein Doppel-Fetch im selben Tick).

## Akzeptanz

- [ ] `snapshot_id` wird ohne Klick aktualisiert (Test: gemocktes Listing mit geänderter
      `snapshot_id` → Worker setzt sie)
- [ ] Request-Budget belegbar: 3 User × ~2 Requests / 15 min ≈ 24 Req/h, im Log/Health sichtbar
- [ ] 429 → Backoff statt Retry-Sturm (Test)
- [ ] Kein Verhalten für User ohne gültigen Token außer einem Log-Eintrag

## Code-Fakten (verifiziert @ main 4acc057)

- `music-hub/src/worker.rs:35` `pub const LOOP_NAMES: [&str; 6] = ["enrichment","freqblog","spotify","crawl","curation","tonies"];`
- Loop-Tasks werden in `worker.rs` (~Zeilen 200–400) als eigene `tokio::spawn`-Tasks
  gestartet; jeder ruft `WorkerHealth::record(name, worked, err)` pro Iteration.
- `run_once` (worker.rs:399) = spotify-Domain (likes + playlist items).
- `ingest::fetch_playlists(pool, cfg, slug)` (ingest.rs:303) — bereits vorhanden,
  schreibt via `upsert_playlist_meta`.
- `backoff(retry_after)` (worker.rs:632) / `quota_backoff()` (worker.rs:638).
- `spotify::get_all_items(base, token, "/me/playlists?limit=50")`.
- Config-Overlay aus `hub_settings` via `Config::apply` (config.rs:136) / settings.rs:826.
- Tests im selben File (`mod tests`, worker.rs:649+), prüfen `snap.loops.len() == LOOP_NAMES.len()`.

## Plan

### Betroffene Dateien
- `music-hub/src/worker.rs` — `LOOP_NAMES`, neuer Loop-Task, Poll-Funktion, Tick-Koordination, Tests
- `music-hub/src/settings.rs` — zwei neue `hub_settings`-Keys (Enabled + Cadence)
- `music-hub/src/ingest.rs` — optionaler Helper `fetch_playlists_for_user` (Token-Check + Request-Zähler)
- `music-hub/src/pages.rs` — Status-Panel rendert Loops bereits generisch aus `snapshot().loops` (nur Doku/Anzeige prüfen, keine Logik)
- `music-hub/tests/playlist_poll.rs` — neuer Integrationstest (Muster: `tests/crawl.rs`, axum-Mock)

### Leitplanken (aus Issue + verifiziertem Code)
- `LOOP_NAMES` (`worker.rs:35`) ist `[&str; 6]` → auf `[&str; 7]` mit `"playlists"`. Der Test
  `worker_health_snapshot_is_name_stable` (`worker.rs:679`) prüft `snap.loops.len() == LOOP_NAMES.len()`,
  zieht also automatisch mit; die Doku-Kommentare („six … names", „all five loops") müssen auf 7 aktualisiert werden.
- `hub_ops.rs:139` validiert Pause/Resume gegen `LOOP_NAMES` ⇒ `playlists` wird dadurch automatisch pausierbar.
- `ingest::fetch_playlists(pool, cfg, slug)` existiert (`ingest.rs:303`) und schreibt `snapshot_id`
  über `upsert_playlist_meta`. Es holt Token/Profil selbst und **fehlt hart ohne Token** → im Loop abfangen.
- Backoff: `backoff(retry_after)` (`worker.rs:632`) / `quota_backoff()` (`worker.rs:638`), 429/Quota werden
  über `Outcome::RateLimited`/`Outcome::Quota` aus `paged()` gemappt.
- Cadence Muster: `crawl.rs::num_setting` / `tonies::autosync` lesen pro Iteration `settings::get(pool, KEY)`.

### User Stories (in Reihenfolge)

1. **Loop registrieren + Gerüst**
   - Akzeptanz: `LOOP_NAMES` enthält `"playlists"` (`len == 7`); der bestehende Namens-Stabilitäts-Test
     ist grün; Doku-Kommentare auf 7 Loops aktualisiert. Neuer `tokio::spawn`-Task folgt exakt dem Muster der
     anderen Loops (`health.is_paused("playlists")`-Skip, `health.record("playlists", worked, err)` pro Iteration).
   - Betroffene Dateien: `worker.rs`
   - Geschätzte LOC: ~35

2. **Cadence + Enabled über `hub_settings`**
   - Akzeptanz: neue Konstanten in `settings.rs` — `PLAYLIST_POLL_ENABLED = "playlist_poll_enabled"`
     (Default `true`) und `PLAYLIST_POLL_INTERVAL_SECS = "playlist_poll_interval_secs"` (Default `900`,
     d. h. 15 min; Bereich 10–15 min). Der Loop liest beide Keys **pro Iteration** (kein Neustart nötig),
     clampt auf ein Minimum (z. B. 60 s) und schläft danach die konfigurierte Dauer.
   - Betroffene Dateien: `settings.rs`, `worker.rs`
   - Geschätzte LOC: ~20

3. **Listing-Pass pro verbundenem User (`snapshot_id` frisch)**
   - Akzeptanz: Für jeden User mit Spotify-Account ruft der Loop `get_all_items("/me/playlists?limit=50")`
     → `upsert_playlist_meta` (via `fetch_playlists`) auf; `hub_playlists.snapshot_id` wird ohne Klick
     aktualisiert. Listing-only — **kein** Item-Fetch in diesem Loop.
   - Betroffene Dateien: `worker.rs` (neue `async fn playlist_listing_pass(...)`), ggf. `ingest.rs`
     (Helper `fetch_playlists_for_user(pool, cfg, user_id, slug)`), falls Profil-/Token-Auflösung getrennt
     gezählt werden soll.
   - Geschätzte LOC: ~70

4. **Kein Doppel-Fetch im selben Tick (Koordination mit `spotify`-Loop)**
   - Akzeptanz: Listing-Pass und Item-Pass (`playlist_jobs`) laufen nicht gleichzeitig gegen Spotify.
     Umsetzung: ein in `spawn()` geteilter `Arc<tokio::sync::Mutex<()>>` (`spotify_tick`), den beide
     Spotify-Loops um ihren jeweiligen Pass halten. Reihenfolge egal; nach dem Listing setzt der nächste
     `spotify`-Pass die Items (getriggert über den geänderten `snapshot_id`), nie im selben Tick doppelt.
   - Betroffene Dateien: `worker.rs`
   - Geschätzte LOC: ~15

5. **Request-Budget sichtbar (Log/Health)**
   - Akzeptanz: Jeder Pass loggt `tracing::info!("playlists poll: {users} users, {reqs} requests")`
     (reqs = Profil + Listing-Seiten pro User; ~2/User ⇒ 3 User ≈ 6 Req / 15 min ≈ 24 Req/h). Optional
     (non-breaking) ein Zähler-Feld `requests: u64` in `LoopState`/`LoopSnapshot` + `record`-Variante, damit
     das Admin-Status-Panel (`pages.rs:4060`, generisch über `snapshot().loops`) das Budget anzeigt.
   - Betroffene Dateien: `worker.rs`, optional `pages.rs`
   - Geschätzte LOC: ~30

6. **Tests (Akzeptanzkriterien aus dem Issue)**
   - Akzeptanz:
     - `snapshot_id`-Test: axum-Mock-Spotify (Muster `tests/crawl.rs`) liefert `/me` + `/me/playlists`
       mit geänderter `snapshot_id`; einmaliger Poll-Pass setzt `hub_playlists.snapshot_id` auf den neuen Wert.
     - 429-Backoff-Test: Mock antwortet `429` (mit `Retry-After`) → Pass bricht ab, **ein** Request, kein
       Retry-Sturm; via `tokio::time::pause()`/Zähler belegt.
     - no-token-Test: User ohne `access_token` → Pass `Ok`, **kein** HTTP-Request, nur Log-Eintrag.
     - Registry-Test: `LOOP_NAMES.len() == 7`, `"playlists"` im Snapshot, pausierbar.
   - Betroffene Dateien: `music-hub/tests/playlist_poll.rs`, `worker.rs` (`mod health_tests`)
   - Geschätzte LOC: ~180

### Implementierungs-Reihenfolge
1 → 2 → 3 → 4 → 5 → 6 (jede Story grün, bevor die nächste beginnt; 4 erst nach 3, weil die Koordination
auf dem bestehenden Listing-Pass aufsetzt).

### Risiko-Hinweise
- `fetch_playlists` holt Profil + Listing und **erroriert ohne Token** → in Story 3 in ein
  `Outcome`-artiges Ergebnis wandeln, damit „kein Token ⇒ nur Log" exakt erfüllt ist.
- `get_all_items` liefert keine Request-Anzahl → Request-Budget aus Seitenzahl (`ceil(items/50)`) + 1 Profil
  ableiten oder Zähler im Helper führen.
- Kein Produktionscode in diesem Schritt; nur Plan.

## Baseline

Stand: 2026-11-17, Branch `feat/506-playlist-listing-poll`, HEAD `4acc057` (= base main).
Toolchain: rustc/cargo 1.97.1. Alles im Crate-Verzeichnis `music-hub/` ausgeführt (`--locked`,
Cargo.lock unverändert).

| Kommando | Ergebnis |
| --- | --- |
| `cargo check --locked` | ✅ grün, `Finished dev profile ... in 22.43s`, 0 Fehler, 0 Warnungen |
| `cargo test --locked` | ✅ grün, Exit `0`, **714 Tests bestanden, 0 failed, 0 ignored** (+ 0 doctests) |

Details:

- Unit-Tests (`music-hub` lib, 148 Tests) grün; ~68 Test-Binaries in `tests/` alle grün
  (inkl. `tag_scope`, `ui`, `views`, `crawl`, `worker`-Flatfiles).
- **2 vorbestehende Warnungen** (nicht durch diese Arbeit verursacht, nur in Test-Targets,
  kein `-D warnings` im Testlauf):
  - `tests/tag_scope.rs`: `fields 'tag_alice' and 'tag_carol' are never read`
  - `tests/ui.rs`: `unused variable: 'cookie'`
  Beide im `cargo check --locked` (lib/bin) **nicht** sichtbar — nur beim Test-Build.
- Kein Produktions-/Feature-Code geändert; Baseline dient als grüner Ausgangspunkt für Stage 3 (developer).

## Implementierung

Stand: 2026-11-17, Branch `feat/506-playlist-listing-poll`, nicht committet (Auftrag: nur Code + Tests + Progress-Datei).

### User Stories

1. **Loop registriert** — `LOOP_NAMES` ist jetzt `[&str; 7]` mit `"playlists"`; Doku-Kommentare
   („seven", Registry-Doku) aktualisiert. Neuer `tokio::spawn`-Task folgt dem Muster der anderen
   Loops (`is_paused("playlists")`-Skip, `record`/`record_with_requests` pro Iteration).
2. **Cadence + Enabled** — neu in `settings.rs`: `PLAYLIST_POLL_ENABLED` (`playlist_poll_enabled`,
   Default `true`) und `PLAYLIST_POLL_INTERVAL_SECS` (`playlist_poll_interval_secs`, Default `900`,
   Clamp auf min. `60` s). Beide werden pro Iteration gelesen; disabled schläft die aktuelle Cadence.
3. **Listing-Pass** — `worker::playlist_listing_pass(pool, cfg)` läuft über alle Spotify-Accounts,
   holt `/me` (persistiert remote id) und `/me/playlists?limit=50`, upsertet Metadaten über die
   vorhandenen `ingest`-Helfer (`is_owned`, `upsert_playlist_meta`; nur Sichtbarkeit auf
   `pub(crate)` erweitert). Kein Item-Fetch in diesem Loop.
4. **Kein Doppel-Fetch** — geteilter `Arc<tokio::sync::Mutex<()>>` (`spotify_tick`) in `spawn()`;
   `spotify`- und `playlists`-Loop halten ihn nur um ihren jeweiligen Pass.
5. **Request-Budget** — `PlaylistPollStats { users, requests, updated }`, Log
   `playlists poll: N users, R requests, U playlists updated`; `LoopState`/`LoopSnapshot` und
   `/api/hub/status` haben ein kumulatives `requests`-Feld, das Admin-Panel eine `Requests`-Spalte.
6. **Tests** — `music-hub/tests/playlist_poll.rs` (snapshot_id-Update, 429-Backoff/kein Retry-Sturm,
   no-token ohne HTTP-Request) + Registry-/Request-Zähler-Unit-Tests in `worker.rs`.

### Geänderte Dateien

- `music-hub/src/worker.rs` — Loop, `playlist_listing_pass`, `paged_counted`, `spotify_tick`,
  `record_with_requests`, Loop-Registry + Tests
- `music-hub/src/settings.rs` — zwei neue Settings-Konstanten
- `music-hub/src/ingest.rs` — `remote_profile`, `is_owned`, `upsert_playlist_meta` auf `pub(crate)`
- `music-hub/src/api.rs` — `requests` im Status-JSON
- `music-hub/src/pages.rs` + `templates/_partials/hub_status.html` — Requests-Spalte
- `music-hub/tests/status.rs` — Loop-Anzahl 6 → 7, `requests`-Feld geprüft
- `music-hub/tests/playlist_poll.rs` — neu (3 Integrationstests)

### Verifikation

| Kommando | Ergebnis |
| --- | --- |
| `cargo check --locked` | ✅ grün, 0 Fehler/Warnungen |
| `cargo test --locked --test playlist_poll --test status` | ✅ 3 + 4 Tests grün |
| `cargo test --locked` | ✅ Exit `0`, **719 passed, 0 failed**, 0 ignored |

## Verify

**Verdikt: PASS** — uncommitteter Diff (`api.rs`, `ingest.rs`, `pages.rs`, `settings.rs`,
`worker.rs`, `hub_status.html`, `tests/status.rs`) + untracked `tests/playlist_poll.rs`
erfüllen die Akzeptanz aus Plan/Issue. Kein Blocker, keine Issues.

Geprüft mit `clanker-git diff` (base `main` @ `4acc057`) und `cargo test --locked`.

### Prüfpunkte

| # | Kriterium | Befund |
| --- | --- | --- |
| a | `LOOP_NAMES` enthält `"playlists"`; Konsumenten/Tests angepasst | ✅ `[&str; 7]` (worker.rs:41); `hub_ops.rs:139` validiert gegen `LOOP_NAMES` ⇒ automatisch pausierbar; `tests/status.rs` 6→7 + `requests`-Feld; Unit-Test `loop_registry_has_seven_including_playlists`; Doku „seven" aktualisiert. Keine stale „six loops"-Referenzen (verbleibende „all five" beziehen sich auf Tag-Familien, unabhängig). |
| b | Cadence/Enabled pro Iteration aus `hub_settings`, Clamp | ✅ `playlist_poll_enabled`/`playlist_poll_interval` lesen pro Iteration `settings::get` (worker.rs:632/642); Default `true`/`900`, Clamp `.max(PLAYLIST_POLL_MIN_SECS = 60)`. Disabled schläft die aktuelle Cadence. |
| c | 429/Quota-Backoff über vorhandene Helfer, kein Retry-Sturm | ✅ `paged_counted` kehrt bei 429/Quota sofort zurück (keine Schleife); Aufrufer nutzt bestehende `backoff(retry_after)`/`quota_backoff()` **außerhalb** des Mutex-Guards. Test `rate_limit_backs_off_without_retry_storm` belegt exakt **1** Listing-Request, `snapshot_id` unverändert. |
| d | Kein Doppel-Fetch im selben Tick (shared Mutex) | ✅ `spotify_tick: Arc<tokio::sync::Mutex<()>>` in `spawn()`, in beide Loops geklont und nur um den jeweiligen Pass gehalten, danach `drop(guard)` **vor** Sleep/Backoff ⇒ keine Blockade während Backoff, kein Deadlock. |
| e | no-token-Pfad ohne HTTP-Request | ✅ NULL/leerer Token ⇒ Log + `continue` vor jedem Netzaufruf; `ingest::access_token` (könnte Refresh = HTTP) wird nur bei vorhandenem Token gerufen. Test `user_without_token_makes_no_request` prüft 0 Hits. |
| f | Security | ✅ Keine Secrets im Log (nur `slug`/Statistiken); alle SQL-Statements mit `sqlx`-Binds (inkl. neuer Account-Query); Settings-Werte mit Fallback + Clamp, kein unvalidierter Input. |
| g | Tests aussagekräftig (kein Smoke) | ✅ 3 Integrationstests (snapshot_id-Update, 429/kein Retry-Sturm, no-token/kein Request) + 2 Unit-Tests (Registry = 7, kumulativer Request-Zähler). Prüfen echtes Verhalten (Request-Zähler, DB-`snapshot_id`, Hit-Pfade). |

### Build/Test

| Kommando | Ergebnis |
| --- | --- |
| `cargo test --locked --test playlist_poll --test status` | ✅ 3 + 4 Tests grün |
| `cargo test --locked` | ✅ Exit `0`, **719 passed, 0 failed, 0 ignored** |
| `cargo clippy --locked` | ✅ Exit `0`; nur vorbestehende Style-Warnungen, **keine** im neuen `worker.rs`-Code |

### Findings (nicht blockierend)

- `worker.rs` `playlist_listing_pass`: bei transport-seitigem `Err` aus `paged_counted`
  (`continue` vor `stats.requests += pages`) wird das Request-Budget leicht **unterzählt** —
  kosmetische Abweichung nur bei Netzfehlern, kein Korrektheitsproblem.
- `stats.updated` zählt jeden erfolgreich upserteten Playlist-Eintrag (`upsert_playlist_meta`
  liefert immer `Some(id)`), daher ist Health-`last_worked` bei ≥1 Playlist stets `true`.
  Semantisch als „Pass hat gearbeitet" korrekt, Feldname „updated" ist optimistisch.
- Der geteilte Mutex serialisiert nur `spotify` + `playlists`; die Spotify-Touch-Cascade des
  `crawl`-Loops bleibt außen vor — deckt sich mit dem Plan-Scope (eigene Domain), keine Regression.

### Empfehlung

PASS → weiter zu Stage 5 (tester). Die drei Minor-Nits sind optional und blockieren nicht.

## Test

Stand: 2026-11-17, Branch `feat/506-playlist-listing-poll` (uncommitteter Diff + untracked
`tests/playlist_poll.rs`). Toolchain rustc/cargo 1.97.1, alles in `music-hub/` mit `--locked`.

**Verdikt: PASS** — vollständige Suite grün, alle vier Issue-Akzeptanzkriterien praktisch belegt,
kein Blocker, keine Issues.

### Test Results

| Kommando | Ergebnis |
| --- | --- |
| `cargo test --locked --test playlist_poll --test status` | ✅ **3 + 4** Tests grün, Exit `0` |
| `cargo test --locked` (gesamte Suite) | ✅ Exit `0`, **719 passed, 0 failed, 0 ignored** (0 doctests) |

Integrationstests (`tests/playlist_poll.rs`, axum-Mock-Spotify, je ein `playlist_listing_pass`):

- `poll_updates_snapshot_id` ... ok
- `rate_limit_backs_off_without_retry_storm` ... ok
- `user_without_token_makes_no_request` ... ok

Unit-Tests (`worker.rs mod health_tests`): `loop_registry_has_seven_including_playlists`,
`record_accumulates_request_budget` — grün. `tests/status.rs`: 4 grün.

### Akzeptanzkriterien — belegt

| # | Kriterium | Beleg |
| --- | --- | --- |
| 1 | `snapshot_id` wird durch einen Poll-Pass aktualisiert (ohne Klick) | ✅ `poll_updates_snapshot_id`: Seed `old-snap` → nach Pass DB-`snapshot_id == "new-snap"`; `stats {users:1, requests:2, updated:1}`; Hit-Pfade exakt `["/me", "/me/playlists"]` (Listing-only, kein Item-Fetch). |
| 2 | 429 → Backoff statt Retry-Sturm | ✅ `rate_limit_backs_off_without_retry_storm`: Mock antwortet 429 + `Retry-After: 0`; Ergebnis `RateLimited{retry_after:Some(0)}`, **genau 1** `/me/playlists`-Hit (kein Retry-Loop), `snapshot_id` unverändert. Aufrufer nutzt `backoff(retry_after)`/`quota_backoff()` außerhalb des Mutex-Guards. |
| 3 | User ohne gültigen Token: kein HTTP-Request, nur Log | ✅ `user_without_token_makes_no_request`: Token `NULL` → `stats {users:0, requests:0, updated:0}` und `hits.is_empty()` (0 Netzaufrufe); im Loop `tracing::info!("playlists poll: skipping {slug} (no Spotify token)")` + `continue` vor jedem Request. |
| 4 | Request-Budget in Log/Health/Status-JSON sichtbar | ✅ `requests` kumulativ in `LoopState`/`LoopSnapshot` (`record_with_requests`), geloggt (`playlists poll: N users, R requests, U playlists updated`), in `/api/hub/status` JSON-Feld `requests` (`api.rs:204`) und als Admin-Panel-Spalte „Requests" (`pages.rs` + `hub_status.html`). `tests/status.rs` asserted `w["requests"].is_number()` für alle 7 Loops; Unit-Test `record_accumulates_request_budget` belegt Kumulation (6+4=10, plain `record` ändert Zähler nicht). |

### Zusatzprüfungen

- Registry: `LOOP_NAMES == [&str; 7]` inkl. `"playlists"`; `snapshot().loops.len() == 7`; `playlists`
  pausierbar; `hub_ops.rs` validiert gegen `LOOP_NAMES` ⇒ automatisch pausierbar.
- Cadence/Enabled: `playlist_poll_enabled` (Default `true`) + `playlist_poll_interval_secs`
  (Default `900`, Clamp `.max(60)`) pro Iteration gelesen.
- Kein Doppel-Fetch: geteilter `spotify_tick`-Mutex um beide Spotify-Pässe; Guard-Drop vor Sleep/Backoff.
- Keine Regression: Suite-Delta gegenüber Baseline 714 → 719 (+5 neue Tests), 0 failed.
- Kein Produktionscode in diesem Schritt geändert (nur Progress-Datei).

## PR

- PR: **#570** — https://github.com/momokli/momos-music-manager/pull/570
- Base: `main` ← Head: `feat/506-playlist-listing-poll`
- Titel: `feat(hub): periodischer Playlist-Listing-Poll hält snapshot_id frisch (#506)`
- Body enthält `Closes #506` (verifiziert).
- Commit: `44da888` (9 Dateien, +802/−16), gepusht nach origin.

## Status

- [x] Dispatch-Validität geprüft: kein gemergter PR für #506, Feature fehlt → Pipeline.
- [x] Branch erstellt.
- [ ] Stage 1 planner
- [ ] Stage 2 setup
- [x] Stage 3 developer (Code + Tests grün, nicht committet)
- [ ] Stage 4 verifier
- [ ] Stage 5 tester
- [x] Stage 6 PR (#570)
- [x] Stage 7 review → REQUEST_CHANGES (Prozess-Gaps: Screenshot + CHANGELOG)

## Review

Stand: 2026-11-17, Branch `feat/506-playlist-listing-poll`, HEAD `3c05cb8`, PR #570
(base `main` ← head; `Closes #506`). Geprüft mit `clanker-git diff origin/main...HEAD`,
`clanker-gh pr view 570` und lokalem `cargo test --locked` (musik-hub).

**Verdikt: REQUEST_CHANGES** — Funktion/Code/Tests sind einwandfrei (alle Fakten-Punkte
PASS), aber der PR ist nach Repo-Regel **unvollständig** und darf so nicht gemergt werden:

### Blocker (merge-blockierend, beide nicht funktional)

1. **Frontend-Regel verletzt — kein Screenshot der geänderten Ansicht.** Der PR ändert die
   sichtbare Admin-/Ops-Statusansicht: neue Spalte „Requests" in
   `music-hub/templates/_partials/hub_status.html` (+ `pages.rs::WorkerRow`/`status_panel`).
   Das ist eine UI-/Layout-Änderung im Sinne der Frontend-Pflicht („UI, Komponenten,
   Layout … sonst gilt der PR als unvollständig und wird nicht gemergt"). Der PR-Body
   hat **keine** Screenshots-Sektion. Präzedenz: die früheren PRs auf dieselbe Ansicht
   (#438 `hub-ops-status-panel.png`, #439 `hub-ops-console.png`) lieferten Screenshots mit;
   `docs/screenshots/` ist für diese Ansicht etabliert. → Vorher/Nachher- bzw. Neu-Screenshot
   der Status-Panel-Tabelle mit der neuen „Requests"-Spalte im PR + Body ergänzen.

2. **CHANGELOG-Eintrag fehlt.** #506 ist ein nutzersichtbares Feature (Playlists stale bis
   „Fetch"). `music-hub/CHANGELOG.md` ist **nicht** Teil des Diffs. Konvention der letzten
   Feature-PRs (#565, #552): `## [Unreleased] → ### Added`-Eintrag. → Eintrag ergänzen.

### Fakten-Prüfung (alle PASS)

| # | Kriterium | Befund |
| --- | --- | --- |
| 1 | Closing-Keyword | ✅ PR-Body enthält `Closes #506` (erste Zeile); CI-Check „Issue-Referenz im PR" pass. |
| 2 | Akzeptanzkriterien abgedeckt | ✅ alle vier: `snapshot_id`-Update (`poll_updates_snapshot_id`), Request-Budget (`requests` in Log/JSON/Panel), 429→Backoff (`rate_limit_backs_off_without_retry_storm`, genau 1 Listing-Hit), no-token→nur Log (`user_without_token_makes_no_request`, 0 Hits). |
| 3 | Code-Qualität / Konsistenz | ✅ neuer Loop folgt exakt dem Muster der bestehenden Loops (`is_paused`-Skip, `record`/`record_with_requests` pro Iteration). `paged` → `paged_counted` sauber via Wrapper (keine Duplikation, `paged` bleibt genutzt). Backoff/Quota off-guard (`drop(guard)` vor `backoff`) ⇒ kein Deadlock/Blockade. Cadence/Enabled pro Iteration aus `hub_settings`, Clamp `.max(60)`. Kleines `pub(crate)`-Sichtbarkeits-Öffnen in `ingest.rs` (kein Verhaltensänderung). |
| 4 | Keine Regressionen | ✅ `LOOP_NAMES` `[&str; 7]`; einziger Fremd-Konsument `hub_ops.rs:139` validiert generisch gegen `LOOP_NAMES` (automatisch pausierbar, kein Code-Change nötig). `tests/status.rs` 6→7 + `requests`-Feld; `worker_health_snapshot_is_name_stable` zieht generisch mit. Verbleibende „all five"-Referenzen betreffen Tag-Familien, unabhängig. Keine stale „six loops"-Texte. |
| 5 | Testbeleg nachvollziehbar | ✅ **lokal reproduziert**: `cargo test --locked --test playlist_poll --test status` → 3 + 4 grün; `cargo test --locked` (volle Suite) → Exit 0, alle Binaries `0 failed` (398 Testresult-Zeilen, 0 Fehler). Delta Baseline 714 → 719 (+5). |
| 6 | Sicherheit | ✅ keine Secrets in Logs (nur `slug`/Statistiken); alle SQL via `sqlx`-Binds (inkl. neuer Account-Query); Settings mit Fallback + Clamp; Token wird nie ausgegeben. |

### Findings (nicht blockierend)

- `playlist_listing_pass`: bei Transport-`Err` aus `paged_counted` wird `stats.requests` unter­gezählt (kosmetisch, nur bei Netzfehlern).
- `stats.updated` zählt jeden upserteten Eintrag (`upsert_playlist_meta` liefert immer `Some`), Feldname „updated" ist optimistisch; Health-`last_worked` dadurch bei ≥1 Playlist stets `true`.
- `spotify_tick` serialisiert nur `spotify` + `playlists` (Scope-konform); `crawl`-Touch-Cascade bleibt außen vor.

### Nächster Schritt

Blocker 1 (Screenshot) + 2 (CHANGELOG) nachreichen → dann ist der PR mergebar
(Funktion, Tests, Akzeptanz bereits PASS). CI-Build/Test waren zum Review-Zeitpunkt noch
`pending` (`mergeStateStatus: BLOCKED`), daher kein Merge-Versuch.

## Rework 1

Stand: 2026-11-17, Branch `feat/506-playlist-listing-poll`, HEAD `3fc2885`, PR #570.
Die beiden merge-blockierenden, **nicht-funktionalen** Review-Blocker sind behoben
(Funktion/Tests/Akzeptanz waren bereits PASS).

### Blocker 1 — Screenshot-Beleg der geänderten UI (behoben)

Skill `music-hub-ui-screenshot` befolgt:

1. Debug-Binary gebaut: `cargo build` → Exit 0.
2. Test-DB + Demo-Daten unter `/home/momo/hub-shot` (`<bin> users`, `<bin> seed-demo`).
3. Session injiziert (eine `sqlite3`-Invocation, `INSERT OR REPLACE`, `hub_session=shot1`);
   `momo` zusätzlich `is_admin=1` (Status-Panel ist admin-only).
4. Freier Port `3011` (`ss -ltn`), Server per `nohup … serve` gestartet (`/login` → 200).
5. `/admin` und `/api/hub/status/panel` per `curl -H "Cookie: hub_session=shot1"` geholt;
   das per htmx nachgeladene Panel-Fragment (+ Ereignis-Log) ins gespeicherte HTML injiziert.
6. Headless Chromium (chromium-1243) auf `file://…/page.html` gerendert
   (`--window-size=1440,1300 --force-device-scale-factor=2`).
7. PNG geprüft: Tabelle „Hub-Worker" mit **allen 7 Loops** (crawl, curation, enrichment,
   freqblog, **playlists**, spotify, tonies) und der neuen Spalte **„Requests"**, nichts
   abgeschnitten.
8. Abgelegt unter `docs/screenshots/hub-ops-status-panel-requests.png` (Repo-Konvention;
   Vorgänger `hub-ops-status-panel.png`), `clanker-git` commit+push.
9. PR-Body per REST-PATCH um `## Screenshots`-Sektion mit raw-Bildlink ergänzt.
10. Server per `kill <pid>` beendet (Port 3011 wieder frei; **kein** `pkill -f`).

### Blocker 2 — CHANGELOG-Eintrag (behoben)

`music-hub/CHANGELOG.md` → `## [Unreleased] → ### Added` erster Eintrag:
„**Periodischer Playlist-Listing-Poll hält `snapshot_id` frisch** (#506)" (analog #504/#503:
Kontext, Loop/Cadence, Backoff, Request-Zähler, **Kein Version-Bump.**).

### Belege

| Prüfpunkt | Ergebnis |
| --- | --- |
| Screenshot-raw-URL | `https://raw.githubusercontent.com/momokli/momos-music-manager/feat/506-playlist-listing-poll/docs/screenshots/hub-ops-status-panel-requests.png` → **HTTP 200** |
| Screenshot im PR-Body | ✅ `## Screenshots`-Sektion mit Bildlink (REST-PATCH, `.body` gegengelesen) |
| CHANGELOG-Eintrag | ✅ im Diff (Commit `3fc2885`), unter `## [Unreleased] → ### Added` |
| `Closes #506` | ✅ weiterhin im PR-Body (erste Zeile, verifiziert) |
| Commit/Push | ✅ `3fc2885` → origin (`clanker-git`, Bot-Identität) |
| Server beendet | ✅ Port 3011 frei, kein `music-hub serve`-Prozess |

### Status

- [x] Blocker 1 (UI-Screenshot) behoben
- [x] Blocker 2 (CHANGELOG) behoben
- [x] PR-Body aktualisiert (Screenshot + `Closes #506`)
