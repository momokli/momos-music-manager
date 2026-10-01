# Progress: Issue #59 — Spotify-Client get_saved_tracks + get_saved_tracks_total

- Repo: momokli/momos-music-manager
- Branch: feature/issue-59-saved-tracks (base origin/main @ 3775f37)
- PR-Ziel: main | Fokus-Milestone: 1.14.0
- Bot-Identity: momo-clanker[bot] via clanker-gh / clanker-git

## Triage-Vorbefund
- Kein PR referenziert #59 (`gh pr list --search` leer).
- `origin/main` enthält keine `saved_tracks`-Methode in `src/spotify/client.rs`.
- => Nicht ALREADY-DONE, normale Pipeline.

## Scope (nur Part B des Plans liked-songs-sync.md)
`src/spotify/client.rs`:
- `get_saved_tracks_total(&self) -> Result<i64>`
- `get_saved_tracks<'a>(&'a self) -> Result<impl Stream<Item = Result<SavedTrack>> + 'a>`
- Muster: `refresh_token_if_needed()` → rspotify-Call → anyhow-Kontext.
- `SavedTrack` kommt aus `rspotify_model` (rspotify 0.15.1).
- `current_user_saved_tracks_manual(None, Some(1), Some(0))` → `page.total: u32`.

Nicht-Scope: Migration, liked_sync.rs, poller, API — Folge-Issues.

## Stage-Log
- [x] Setup: Branch angelegt, Umgebung geprüft.
- [x] 1 planner
- [x] 2 setup (build baseline)
- [x] 3 developer
- [x] 4 verifier
- [x] 5 tester
- [ ] 6 developer (PR)
- [ ] 7 reviewer

## Befunde / Notizen
- Gateway-Host == planet (gleiche Maschine, /home/momo/repos/momos-music-manager geteilt).
- cargo 1.97.1 unter /home/momo/.cargo/bin.

## Plan (planner)

### Betroffene Dateien
- `src/spotify/client.rs` — **nur** diese Datei (Scope Issue #59).
- Kein `Cargo.toml`, keine Migration/DB/Poller/API.

### Verifizierte Fakten (gegen die Voreinstellungen geprüft)
- `SavedTrack { added_at: DateTime<Utc>, track: FullTrack }` in `rspotify-model-0.15.1/src/track.rs:90`.
- `rspotify` re-exportiert das Model: `rspotify-0.15.1/src/lib.rs:155` = `pub use rspotify_model as model;`. **`rspotify_model` ist KEINE direkte Dependency** (nur transitiv; `Cargo.toml` listet nur `rspotify`). => Import zwingend über `rspotify::model::SavedTrack`; ein direkter `rspotify_model::SavedTrack`-Pfad würde nicht kompilieren, ausser man fügt eine neue direkte Dependency hinzu (unerwünscht).
- `current_user_saved_tracks(&self, market: Option<Market>) -> Paginator<'_, ClientResult<SavedTrack>>` (oauth.rs:651).
- `current_user_saved_tracks_manual(&self, market, limit: Option<u32>, offset: Option<u32>) -> ClientResult<Page<SavedTrack>>` (oauth.rs:664); `Page.total: u32` (page.rs:27).
- `SpotifyClient` ist public re-exportiert (`spotify/mod.rs:14 pub use client::SpotifyClient`), Methode ist Teil der oeffentlichen Lib-API => **kein** `dead_code`-Warn zu erwarten. `#[allow(dead_code)]` nur ergaenzen, falls der Build wider Erwarten warnt.
- Vorhandene Muster: `get_user_playlists` (client.rs:230) fuer den Stream-Map; `get_playlist`/`get_track` fuer `refresh_token_if_needed -> Call -> .context(...)`. `StreamExt`, `anyhow::Context/Result`, `error!` sind bereits importiert.

### Signatur-Skizzen (kein Code)
```rust
use rspotify::model::SavedTrack; // in den bestehenden `model::{...}`-Block aufnehmen

pub async fn get_saved_tracks_total(&self) -> Result<i64>

pub async fn get_saved_tracks<'a>(
    &'a self,
) -> Result<impl tokio_stream::Stream<Item = Result<SavedTrack>> + 'a>
```

### User Stories (in Reihenfolge)

1. **Model-Import ergaenzen**
   - `SavedTrack` in den vorhandenen `use rspotify::{ ... model::{ ... } }`-Block (Zeile 8–15) aufnehmen.
   - AK: `cargo build` kompiliert; keine neue direkte Dependency.
   - Datei: `src/spotify/client.rs`.

2. **`get_saved_tracks_total(&self) -> Result<i64>`**
   - `self.refresh_token_if_needed().await?`; dann `self.spotify.current_user_saved_tracks_manual(None, Some(1), Some(0)).await.context("Failed to fetch liked songs page")?`; `Ok(page.total as i64)`.
   - AK: liefert `i64` aus `Page.total` (u32-Cast); nutzt `limit=1/offset=0`; `context(...)` statt `unwrap`; folgt exakt dem `get_playlist`-Muster.
   - Datei/Umfang: ~10 LOC.

3. **`get_saved_tracks<'a>(&'a self) -> Result<impl Stream<Item = Result<SavedTrack>> + 'a>`**
   - `refresh_token_if_needed`; `let stream = self.spotify.current_user_saved_tracks(None);`; `Ok(stream.map(|item| match item { Ok(t) => Ok(t), Err(e) => { error!("Failed to fetch saved track: {}", e); Err(anyhow::Error::from(e)).context("Spotify API error") } }))`.
   - AK: Signatur/Lifetime identisch zum Interface; Markt `None` (wie `get_user_playlists`); Fehler werden geloggt und als `anyhow`-Error mit Kontext durchgereicht; `async fn` wegen Token-Refresh.
   - Datei/Umfang: ~18 LOC.

4. **Definition of Done / Verifikation**
   - `cargo build` erfolgreich; **keine neuen Warnungen** (mit Baseline vergleichen; nur falls Warnung auftritt `#[allow(dead_code)]` ergaenzen).
   - Keine HTTP-/Integrations-Tests (konsistent zum Rest; kein Spotify-Mock vorhanden).
   - Reihenfolge fuer den Developer: 1 → 2 → 3 → 4.

### Nicht-Scope (Folge-Issues)
Migration/DB, `liked_sync.rs`, Poller, API-Endpoint, `playlist_kind` — explizit aus #59 herausgehalten.

### Blocker / Hinweise fuer Developer
- **Import-Falle:** `SavedTrack` ueber `rspotify::model::SavedTrack` (Re-Export), nicht ueber einen direkten `rspotify_model`-Pfad (keine direkte Dep).
- Der zurueckgegebene `Paginator` lebt von `&'a self.spotify`; Return-`impl Stream + 'a` beibehalten.

## Setup (setup)

- Repo: `/home/momo/repos/momos-music-manager`
- Branch: `feature/issue-59-saved-tracks` (HEAD OK, base origin/main @ 3775f37)
- `git status`: sauber; einzige Aenderung ist die untracked Progress-Datei selbst.
- cargo 1.97.1 (`$HOME/.cargo/bin`), nproc=20, Gateway-Host == planet.
- Baseline: **green, 44 warnings** (alle aus der lib `momos-music-manager`).
- `cargo build` exit 0, `Finished dev profile ... in 1m 43s` (kalter Build; inkrementell schneller).

### Baseline-Warnungen (44, Kategorien)
- 7x ambiguous glob re-exports
- ~14x unused imports/variables (u. a. `Sqlite`, `Sha256`, `Digest`, `serde::Serialize`, `std::sync::Arc`, `put`, `Row`)
- 3x variable does not need to be mutable
- je 1x never used/never constructed/never read: `handle_websocket`, `router`, `check_key_compatibility`, `update_tag_category_metadata_handler`, `is_empty`, `telemetry_toml_present`, `TELEMETRY_TOML_KEYS`, `Db`, `deemix_base_url`, `comment_statuses`
- je 1x private-interface (`TelemetryToml`, `PutCorrectionsRequest`), 1x unexpected `cfg` value `test-util`

=> Kein Blocker, keine neuen Warnungen fuer #59 zu erwarten (Baseline dient als Vergleich).
Nichts committet.

## Developer (1)

- Datei geaendert: `src/spotify/client.rs` (nur diese; kein `Cargo.toml`, keine neue Dependency).
- US1: `SavedTrack` in den bestehenden `use rspotify::{ ... model::{ ... } }`-Block aufgenommen (Re-Export-Pfad `rspotify::model::SavedTrack`, verifiziert gegen `rspotify-model-0.15.1/src/lib.rs:26` = `track::*`).
- US2: `pub async fn get_saved_tracks_total(&self) -> Result<i64>` — `refresh_token_if_needed` → `current_user_saved_tracks_manual(None, Some(1), Some(0)).await.context("Failed to fetch liked songs page")?` → `Ok(page.total as i64)`.
- US3: `pub async fn get_saved_tracks<'a>(&'a self) -> Result<impl tokio_stream::Stream<Item = Result<SavedTrack>> + 'a>` — Muster wie `get_user_playlists`: `spotify.current_user_saved_tracks(None)` + `stream.map(...)` mit `error!`-Log und `anyhow`-Kontext.
- Platzierung: direkt nach `get_user_playlists`, vor `get_playlist` (Naehe `get_user_playlists`/`get_playlist_tracks`).
- Doc-Kommentare auf Deutsch.
- Keine HTTP-/Integrations-Tests (konsistent zum Rest, kein Spotify-Mock vorhanden).

Verifikation `cargo build`:
- exit 0, `Finished dev profile ... in 19.82s`.
- **44 Warnungen** — identisch zur Baseline (keine neuen Warnungen).
- Kein `#[allow(dead_code)]` noetig (oeffentliche Lib-API).

Nicht committet (uebernimmt der Orchestrator / spaetere Stage).

## Verifier

**Ergebnis: PASS**

### Prüfung gegen Issue #59
- **Signaturen exakt**: `get_saved_tracks_total(&self) -> Result<i64>` und `get_saved_tracks<'a>(&'a self) -> Result<impl tokio_stream::Stream<Item = Result<SavedTrack>> + 'a>` — identisch zur Vorgabe.
- **`refresh_token_if_needed()`**: in beiden Methoden vorhanden.
- **Fehler via anyhow-Kontext**: `get_saved_tracks_total` → `.context("Failed to fetch liked songs page")`; `get_saved_tracks` → `Err(anyhow::Error::from(e)).context("Spotify API error")` + `error!`-Log, exakt wie `get_user_playlists`.
- **`limit=1, offset=0`**: `current_user_saved_tracks_manual(None, Some(1), Some(0))`; `page.total as i64` (u32→i64 ok).
- **Stream-Map**: `current_user_saved_tracks(None)` + `stream.map(...)` — analog `get_user_playlists`.
- **Model-Import**: `SavedTrack` über Re-Export `rspotify::model::SavedTrack` (keine neue Dep).

### Scope
Nur `src/spotify/client.rs` geändert (38 insertions, 2 deletions). Keine Migration/DB/Poller/API/Cargo.toml, keine neue Dependency. ✓

### Security
Keine Secrets. Kein `unwrap`/`expect` auf Netzwerkpfaden (die bestehenden Vorkommen Zeilen 150/198/390/529 sind `unwrap_or*`, 565/569 Test-Code). Keine ungeprüften Casts ausser dem erlaubten `u32 as i64`. ✓

### Build
`cargo build` exit 0; `generated 44 warnings` — identisch zur Baseline (44), keine neuen Warnungen. ✓

### DoD
Alle vier Punkte erfüllt: kompiliert ohne neue Warnungen; total nutzt limit=1/offset=0; Rückgabetypen erlauben `.next()`-Streaming; keine HTTP-Mocks (DB-Seite in Folge-Issues). ✓

## Tester

**Ergebnis: PASS**

### Test Results
- `cargo build`: **exit 0**, `Finished dev profile`, 44 Warnungen = Baseline (keine neuen).
- `cargo test` (vollständige Suite, ~2 min): **exit 0** — **1139 passed, 0 failed, 1 ignored** (31 Test-Binaries inkl. Doctests). Kein rot/panicked/error.
- Kein neuer HTTP-/Netzwerk-Test — wie im DoD erwartet (keine Netzwerk-Mocks; DB-Seite in Folge-Issues).
- `cargo clippy --all-targets`: **exit 101**, aber ausschliesslich **vorbestehende**, von #59 unberührte Befunde:
  - deny-Level-Fehler `unused_comparisons` in `tests/api_tasks.rs:299,300` und `src/tasks/mod.rs:3091` (Dateien nicht von #59 angefasst → vorbestehend).
  - 168 Warnings (Baseline-Kategorien); die zwei `client.rs`-Warnings (Z. 411 `redundant_closure`, Z. 507 `map_err`) stammen aus Alt-Code, **nicht** aus den neuen Methoden (Z. 257–290).

### Scope / Edge-Cases
- `git diff --stat`: nur `src/spotify/client.rs`, 38 insertions, 2 deletions. Keine neue Dependency/Cargo.toml.
- Neue Lib-API (`get_saved_tracks_total`, `get_saved_tracks`) bricht Build/Testsuite nicht; keine neuen Warnungen (rustc oder clippy).

### Issues
- Keine durch #59 verursachten Regressionen. Die roten Clippy-Läufe sind vorbestehend und unabhängig von #59.
