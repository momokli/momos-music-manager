# Progress — Issue #299 (feat(hub): Playlists im Tool bauen/kuratieren)

Milestone: hub-smartplay-0.23.0 (#29) · Base: main @ dc6a472 · Branch: feat/issue-299-playlist-curation

## Recon (bereits in main vorhanden)
- #317 (gemergt): curated playlists — anlegen (`/curate`), fill_mode manual|tag|lens,
  Limit, Beschreibung, öffentlich, täglich pushen, Spotify-Push (write-scope).
  Dateien: music-hub/src/curate.rs, templates/curate.html, curate_detail.html, tests/curate.rs
- #283 (gemergt): bi-way sync write tagged tracks to Spotify playlists.
- #298 (gemergt): Smart-Tags / Dynamic Lenses.
- Vorhandene Bausteine für Vorschläge: scoring.rs (ripeness, queue), similarity.rs, similar.rs,
  recommend.rs (recommend_tags), cosine.rs.

## Gap für #299 (Deliverable dieses PRs)
- [ ] Track-Vorschläge für eine kuratierte Playlist anhand der bereits enthaltenen Tracks
      (Metrik/Score, analog Spotify) — Kernstück.
- [ ] Vorschlag → in Playlist übernehmen (manual item, position).
- [ ] Integrationstest.

## Stages
- [x] 1 planner
- [x] 2 setup
- [x] 3 developer
- [x] 4 verifier
- [x] 5 tester
- [ ] 6 developer (PR)
- [ ] 7 reviewer

## Setup
- Branch: `feat/issue-299-playlist-curation` (ausgecheckt, Basis `main @ dc6a472`).
- Working Tree: sauber; nur `progress-issue-299.md` untracked (erwartet, Dokumentationsdatei).
- Host: planet. `cargo` im PATH (`/home/momo/.cargo/bin/cargo`), kein rustup-PATH-Setup nötig.
- Crate: `music-hub` ist eigenständig (kein Workspace-Root) → `cargo check` **im** `music-hub/`-Verzeichnis.
- Baseline: `cargo check` (music-hub) → **GRÜN**, `Finished dev profile ... in 41.69s`, keine Fehler/Warnungen.
- Ergebnis: Baseline ok — Setup-Stage abgeschlossen, bereit für Developer.

## Develop

Status: **Umgesetzt** (US1–US3 + Tests). US4 (Mood-Feld) bewusst **nicht** angefasst
(optional, „nur wenn risikoarm" → kein Schema-/Form-Touch, keine Migration 056).
Keine bestehenden Funktionen umbenannt (nur additiv erweitert).

### Geänderte Dateien
- `music-hub/src/curate.rs`
  - neu: `pub struct Suggestion { track_id, title, artists, artists_html, score, why: Vec<(String,i64)> }`.
  - neu: `pub async fn suggestions(pool, cp: &CuratedPlaylist, limit: usize, viewer_id: i64) -> Result<Vec<Suggestion>>`.
    - Seeds = `resolve_track_ids`; leere Seeds → leeres Ergebnis.
    - Kandidaten = alle `hub_tracks` **minus Seeds** **minus** Tracks, deren einzige Tags `SETLIST`/`RUMPELKISTE` sind (untagged bleiben drin) **minus** Tracks, die der Viewer schon in einer eigenen kuratierten Playlist hat (`viewer_id`).
    - Score additiv, Seed-Aggregation = **max**: `w_sim·similarity_to_seed + w_occ·shared_playlists(internal_suggestions) + w_audio·max_cosine(similar::neighbors) + w_key·camelot_compatible + w_ripe·ripeness_many`.
    - Gewichte **live aus `settings::engine()`** (reuse, kein Hardcode): `w_sim=e.sim_factor`, `w_occ=e.shared_factor`, `w_audio=e.rec_cluster`, `w_key=e.tag_overlap_base`, `w_ripe=e.ripeness_factor`.
    - Batched (kein N+1): `similarity_to_seed`/`ripeness_many` batched; Cosine-Seed-Stichprobe gecappt (`SEED_SAMPLE_CAP = 20`, deterministisch = resolve-Reihenfolge); Camelot-Keys einmal via `camelot_map` (View `v_track_audio`).
    - Deterministische Sortierung: score desc → artists → title → id.
    - `artists_html` nur für die zurückgegebenen ≤ `limit` Tracks via `artists::artists_html_for_tracks`.
  - neu: `pub async fn add_item_at(pool, playlist_id, track_id, position)` (explizite Position, idempotent); `add_item` bleibt unverändert.
- `music-hub/src/web.rs`
  - Routen: `GET /curate/{id}/suggestions`, `POST /curate/{id}/items`.
  - Handler `curate_suggestions` (Session-Gate → `/login`; fremd/unbekannt → `track_not_found()` = 404) und `curate_add_item` (`CurateItemForm { track_id, position: Option<i64> }`, Owner-Check; antwortet mit neu gerendertem Fragment + Toast).
  - `SuggRow`/`CurateSuggestions` (Askama-Partial), `sugg_rows()` mit vorberechneten Textfeldern (**kein `==` in HTML-Attributen**).
- `music-hub/templates/_partials/curate_suggestions.html` (neu): Score/Titel/Künstler/Warum + „Übernehmen"-Form (`hx-post /curate/{id}/items`, `hx-target="#curate-suggestions"`, `hx-swap="innerHTML"`); leer → `hub-empty`.
- `music-hub/templates/curate_detail.html`: Sektions-Container `#curate-suggestions` mit `hx-get`/`hx-trigger="load"`/`hx-swap="innerHTML"`.
- `music-hub/tests/curate.rs`: 5 neue Tests (siehe unten).

### Tests
- `suggestions_require_login` — GET ohne Cookie → 303 `/login`.
- `suggestions_foreign_playlist_404` — fremde Playlist → 404.
- `suggestions_exclude_existing` — Seed nie in Vorschlägen, Tag-Nachbar schon.
- `suggestions_rank_shared_tag_higher` — Tag-Teiler rankt über Playlist-Co-Occurrence (Score 5.0 vs 3.0).
- `adopt_suggestion_adds_item` — POST `/curate/{id}/items` → Zeile angelegt, Fragment enthält Track nicht mehr, zweites POST idempotent (COUNT bleibt 1).

### Testergebnis
- `cargo check` → **GRÜN** (Finished dev profile, keine Fehler/Warnungen).
- `cargo test --test curate` → **18 passed; 0 failed** (13 bestehende + 5 neue).
- `cargo test` (volle Suite) → **exit 0**, alle Binaries grün (u. a. `ui.rs` 53, `views.rs` 5).

### Offene Punkte
- US4 (Mood-Feld) nicht umgesetzt → optionale Folge-Issue, falls gewünscht.
- Suggestion-Gewichte reusen vorhandene Engine-Knobs; dedizierte `/admin`-Faktoren (z. B. `engine_sugg_*`) wären sauberer, wurden aber bewusst vermieden (Reuse laut Plan ok).
- Nicht committet/gepusht (macht der Orchestrator).

## Plan

### Betroffene Dateien
- `music-hub/src/curate.rs` — neue Suggestions-Engine (Kern).
- `music-hub/templates/curate_detail.html` — Vorschlags-Sektion + „Übernehmen".
- `music-hub/templates/_partials/curate_suggestions.html` — neues htmx-Fragment.
- `music-hub/src/web.rs` — Routen/Handler `/curate/{id}/suggestions` + `/curate/{id}/items`.
- `music-hub/src/settings.rs` — nur falls ein neuer Engine-Faktor gewünscht (sonst reuse).
- `music-hub/migrations/056_curated_mood.sql` — **nur optional** (Mood-Feld), sonst entfällt.
- `music-hub/tests/curate.rs` — Integrationstests (common::spawn-Muster).

### Wiederverwendete Bausteine (nicht duplizieren — gibt es schon)
- `curate::resolve_track_ids` (`src/curate.rs:107`) → Seed-Menge der Playlist (manual + tag/lens).
- `curate::add_item` (`src/curate.rs:185`) → Übernahme eines Vorschlags (setzt `position` = MAX+1).
- `similarity::similarity_to_seed` (`src/similarity.rs:111`) → Tag-Overlap-Score Seed↔Kandidat, Rumpelkiste-aware, batched (kein N+1).
- `similarity::tag_similarity` (`src/similarity.rs:82`) → Paar-Score, falls wir Seed-Aggregation pro Kandidat brauchen.
- `scoring::ripeness_many` (`src/scoring.rs:168`) → Datenqualität als (optionaler) Tiebreak/Kandidaten-Gate.
- `digging::internal_suggestions` (`src/digging.rs:28`) → Playlist-Co-Occurrence-Signal (schließt Rumpelkiste aus).
- `similar::neighbors` (`src/similar.rs:71`) → EffNet-Cosine-Kandidaten ab einem Seed.
- `features::camelot_compatible` (`src/features.rs:120`) → Key-Harmonie Seed↔Kandidat aus `v_track_audio`/`hub_track_features`.
- `settings::Engine` Felder `tag_overlap_base`/`cross_group_bonus` (`src/settings.rs:632`) und `rec_*` (`src/settings.rs:656`) → Gewichte live, kein Hardcode.
- Muster: `similar_page` (`src/pages.rs:2950`) und `track_actions_playlist_post` (`src/web.rs:2569`) für Session-Gate/Owner-Check/Template/Toast.

### User Stories (Reihenfolge)

1. **Suggestion-Core `curate::suggestions`** (anlegen/befüllen existiert bereits → nur dieser Teil neu)
   - Akzeptanzkriterien:
     - Signatur `pub async fn suggestions(pool, cp: &CuratedPlaylist, limit: usize, viewer_id: i64) -> Result<Vec<Suggestion>>`.
     - Seed-Menge = `resolve_track_ids(pool, cp)`; Kandidaten = alle `hub_tracks`, abzüglich Seeds, abzüglich Rumpelkiste (`rumpelkiste_tag_ids`) und `SETLIST`/`RUMPELKISTE`-only Tracks.
     - Score pro Kandidat (additiv, Gewichte aus `settings::engine()`):
       `score = sim_weight * max_similarity_to_seed + occ_weight * shared_playlists + audio_weight * max_cosine + key_weight * camelot_bonus`.
     - Seed-Aggregation: **max** (nicht Summe), damit große Playlists nicht systematisch höher scoren.
     - `Suggestion { track_id, title, artists, score: f64, why: Vec<(label,count)> }`; deterministische Sortierung (score desc, artists, title).
     - Keine N+1: `similarity_to_seed` + `ripeness_many` batched; Embedding-Neighbors über eine gecappte Seed-Stichprobe (z. B. jüngste ≤ 20 Seeds), danach Union.
   - Betroffene Dateien: `src/curate.rs`.
   - Geschätzte LOC: ~140.

2. **Routen + htmx-Fragment: Vorschläge anzeigen**
   - Akzeptanzkriterien:
     - `GET /curate/{id}/suggestions` → Session-Gate; `cp.user_id != nav.id` → 404 (`track_not_found()`); rendert `_partials/curate_suggestions.html` (Askama, kein `==` in Attributen → vorberechnete Bool-Felder).
     - Fragment listet Score, `why()`-Text und „Übernehmen"-Button; leere Liste → `hub-empty`.
     - Detailsicht `/curate/{id}` bindet das Fragment per `hx-get`/`hx-trigger="load"` ein.
   - Betroffene Dateien: `src/web.rs`, `templates/curate_detail.html`, `templates/_partials/curate_suggestions.html`.
   - Geschätzte LOC: ~90.

3. **Übernahme eines Vorschlags in die Playlist**
   - Akzeptanzkriterien:
     - `POST /curate/{id}/items` mit Formfeld `track_id`; Owner-Check; idempotent via `curate::add_item` (PRIMARY KEY playlist_id+track_id).
     - Antwort = neu gerendertes Vorschlags-Fragment **ohne** den übernommenen Track (htmx swap); `updated_at` wird von `add_item` gebumpt.
     - optionales Feld `position` für Setlist-Reihenfolge: wenn gesetzt, `add_item`-Variante mit expliziter Position (kleine Erweiterung), sonst MAX+1.
   - Betroffene Dateien: `src/web.rs`, `src/curate.rs`.
   - Geschätzte LOC: ~60.

4. **(Optional) Playlist-Mood-Feld**
   - Akzeptanzkriterien:
     - Neue additive Migration `056_curated_mood.sql`: `ALTER TABLE hub_curated_playlists ADD COLUMN mood TEXT`.
     - Mood ins `CurateForm`/`parse_curate`/INSERT/UPDATE + Detailformular aufnehmen; optional als Filter/Signal in `suggestions` (z. B. Tracks mit passendem Mood-Tag höher).
     - Nur wenn günstig — Migration ist append-only, `001` nie anfassen.
   - Betroffene Dateien: `migrations/056_curated_mood.sql`, `src/web.rs`, `templates/curate*.html`.
   - Geschätzte LOC: ~50.

5. **Testplan (Integration, `music-hub/tests/curate.rs`)**
   - Akzeptanzkriterien (Muster `common::spawn()` + `app.user_with_password`):
     - `suggestions_require_login`: GET `/curate/{id}/suggestions` ohne Cookie → 303 `/login` bzw. UNAUTHORIZED wie `curate_requires_login`.
     - `suggestions_exclude_existing` (Einheiten-Level via `curate::suggestions` auf Temp-Pool wie `resolve_*_lens`): Seeds tauchen nie in Vorschlägen auf.
     - `suggestions_rank_shared_tag_higher`: zwei Kandidaten, einer teilt ein Tag/Playlist mit Seed → höherer Score/Reihenfolge.
     - `adopt_suggestion_adds_item`: POST `/curate/{id}/items` → Zeile in `hub_curated_items`, Fragment enthält Track nicht mehr; erneutes POST bleibt idempotent (COUNT = 1).
     - `suggestions_foreign_playlist_404`: fremde Playlist-id → 404.
   - Betroffene Dateien: `tests/curate.rs`.
   - Geschätzte LOC: ~150.

### DB-Änderungen
- Kern (US1–3): **keine**. `hub_curated_items.position` existiert bereits (`migrations/041_hub_curation.sql`) → Setlist-Reihenfolge ohne Schemaänderung machbar.
- Nur optional (US4): eine additive Migration **056** (`mood`). Aktuell letzte Migration: `055_smart_tags.sql`.

### Offene Fragen / Risiken
- **Kandidaten-Universum**: `hub_tracks` kann groß sein; Embedding-Cosine ist O(n) pro Seed → Seed-Stichprobe + Cap, sonst Laufzeitrisiko. Alternative: nur Kandidaten aus Tag-/Playlist-Nachbarschaft (`internal_suggestions`), dann Cosine nur darauf.
- **Seed-Aggregation**: max vs. mean vs. sum — max gewählt (Fairness); benötigt Abstimmung, falls die Engine es konfigurierbar machen soll (neuer `ENGINE_*`-Faktor + `/admin`).
- **Doppel-Quelle**: Anlegen/Befüllen/Push existieren (#316/#317) — dieser PR darf `curate.rs` nur **erweitern**, keine bestehenden Funktionen umbenennen (sonst Testbrüche in `tests/curate.rs`).
- **Askama-Falle**: `==` in HTML-Attributen vermeiden (Gotcha in `music-hub/AGENT.md`) → Bools vorberechnen.
- **Migration-Checksum**: applizierte Migrationen nie editieren; nur `056` neu anlegen.
- **Rumpelkiste/Smart-Tags**: `similarity`-Helfer sind Rumpelkiste-aware; Smart-Tags (`hub_smart_tags`) haben keine resolved tags → dürfen Seeds verzerren, ggf. Seeds ohne resolved tags tolerieren.

## Verify

Status: **PASS** (Quality Gate bestanden, weiter zu Test).

Geprüft: `git diff origin/main...HEAD` (4 Dateien + progress-Doc), `cargo test --test curate`.

### Sicherheit / Autorisierung
- `GET /curate/{id}/suggestions` (`web.rs:curate_suggestions`) und `POST /curate/{id}/items`
  (`web.rs:curate_add_item`): erst `ui::nav` → ohne Session `303 /login`; dann
  `curate::get(...).ok().flatten()` → unbekannte **und** fremde id → `track_not_found()` = **404**
  (kein Existenz-/Daten-Leak, konsistent zu `curate_push`/`curate_delete`). Owner-Check `cp.user_id != nav.id`.
- SQL durchgängig parametrisiert (`bind`/`QueryBuilder::push_bind`, IN-Klausel in 900er-Chunks).
  Keine String-Interpolation von Nutzereingaben → **keine SQL-Injection**.
- Kein Zugriff auf fremde Playlists/Tracks möglich; `viewer_id` stammt aus der Session.

### Korrektheit
- Seed-Ausschluss: `seed_set.contains` im Kandidaten-Loop + Skip in occ/cos → Seeds nie vorgeschlagen (Test belegt).
- Rumpelkiste/SETLIST-Ausschluss: Kandidaten = untagged ODER ≥1 Nicht-`SETLIST`/`RUMPELKISTE`-Tag; Helfer
  (`similarity_to_seed`, `internal_suggestions`) zusätzlich Rumpelkiste-aware. Deckt US1 ab.
- Deterministische Sortierung: score desc → artists → title → id. Embedding-Seed-Stichprobe deterministisch
  (resolve-Reihenfolge, `SEED_SAMPLE_CAP=20`).
- Idempotenz: `add_item_at` nutzt `INSERT OR IGNORE` (PK playlist_id+track_id); Test `adopt_suggestion_adds_item`
  prüft COUNT bleibt 1 nach zweitem POST. `add_item` unverändert.
- Kein N+1: `similarity_to_seed`/`ripeness_many` batched; Cosine/Occ nur über gecappte Seed-Stichprobe (≤20).
  Restliche Pro-Seed-Schleifen sind durch den Cap begrenzt — akzeptabel, Laufzeitrisiko dokumentiert.
- Askama-Falle: `grep '=='` in neuen Templates → **keine** `==` in Attributen; `hx-post="/curate/{{ id }}/items"`.
- Migrationen: `git diff ... -- migrations` **leer** → unangetastet; US4 (Mood) bewusst weggelassen.

### Akzeptanz #299
Anlegen/Befüllen (bereits in main) + Vorschläge-Engine + htmx-Fragment/„Übernehmen" + Integrationstest vorhanden.

### Testergebnis
`cd music-hub && cargo test --test curate` → **18 passed; 0 failed** (13 bestehende + 5 neue:
`suggestions_require_login`, `suggestions_foreign_playlist_404`, `suggestions_exclude_existing`,
`suggestions_rank_shared_tag_higher`, `adopt_suggestion_adds_item`).

### Empfehlung
- Keine Blocker. Optionale Härtung (nicht blockierend): einige `unwrap_or_default()` auf DB-Fehlern
  (z. B. `camelot_map`, `owned`, `cand_rows`) verschlucken Fehler still → bei DB-Problem leere Vorschläge
  statt Fehler; bewusst tolerant, aber ggf. später loggen.
Bottom line: PASS.

## Test

Verdikt: **PASS** — Branch: `feat/issue-299-playlist-curation` @ `c1b3cf4`, Host: planet, Rust stable, `cargo` im PATH.

### Ausgeführt
- `cd music-hub && cargo test --test curate` → **18 passed; 0 failed** (13 bestehende + 5 neue).
- `cd music-hub && cargo test` (volle Crate-Suite, alle Binaries) → **exit 0**, **612 passed; 0 failed; 0 ignored**.
  Keine Regression in `ui.rs` (53), `views.rs` (5), `similar.rs`, `recommend.rs`, `ranking.rs`, `similarity.rs` o. a.
- Warnungen (nicht blockierend, **vorbestehend**, nicht aus dem neuen Code): `unused variable: cookie` (`tests/ui.rs`),
  `fields tag_alice/tag_carol never read` (`tests/tag_scope.rs`).

### Abdeckung Akzeptanz #299 (neue Tests `tests/curate.rs`)
- Login-Gate: `suggestions_require_login` — GET `/curate/1/suggestions` ohne Cookie → **303 `/login`**. ✔
- Fremd-Playlist: `suggestions_foreign_playlist_404` — fremde Playlist-id → **404**. ✔
- Seed-Ausschluss: `suggestions_exclude_existing` — Seed (Tag-Teiler) nie in Vorschlägen, Tag-Nachbar schon. ✔
- Ranking: `suggestions_rank_shared_tag_higher` — Tag-Teiler (`score >`) rankt über Playlist-Co-Occurrence; Top = Tag-Match. ✔
- Übernahme idempotent: `adopt_suggestion_adds_item` — POST `/curate/{id}/items` legt Zeile an, adoptierter Track fällt aus
  dem Fragment, zweites POST → COUNT bleibt 1. ✔
- Anlegen/Befüllen (bereits in main): `create_then_render_detail`, `add_to_existing_playlist_links_item`,
  `add_to_new_playlist_creates_playlist_and_item`, `picker_lists_playlists` etc. ✔

### Gemeldete Lücken (nicht selbst ergänzt)
1. **Explizite `position` bei Übernahme** (US3, `add_item_at` + Formfeld `position`) ist **ungetestet**.
   `adopt_suggestion_adds_item` setzt keine Position; die Setlist-Reihenfolge (explizite Position vs. MAX+1) ist
   damit nicht abgesichert.
2. **Kandidaten-Universum**: der Ausschluss reiner `SETLIST`/`RUMPELKISTE`-Tracks (untagged bleiben drin) hat keinen Test.
3. **Leere Seeds** → leeres Vorschlagsergebnis ist nicht getestet.
4. **`viewer_id`-Ausschluss** (Tracks, die der Viewer bereits in eigener kuratierter Playlist hat) ist nicht getestet.
   Diese drei Punkte sind wichtige Edge Cases der neuen `curate::suggestions`-Engine, aber nicht in der Akzeptanzliste; als Lücke dokumentiert.
