# Progress — Issue #333: Backpack export (m3u8 / Traktor / ZIP)

- Repo: momokli/momos-music-manager (crate `music-hub/`)
- Basis: `main` @ 3540dbe (== origin/main)
- Fokus-Milestone: hub-backpacks-0.24.0
- Deliverable: gepushter Branch + offener PR gegen `main` mit `Closes #333`
- Branch: `feat/issue-333-backpack-export`
- Arbeitskopie: ~/repos/momos-music-manager-333

## Dispatch-Gültigkeit

Deps #327/#329/#330/#331/#332 gemergt (PRs #379/#380/#381/#382/#387), #328 via #388.
Kein PR/Branch für #333 → normale Pipeline.

## Scope (Milestone-Outcome)

> a backpack resolves to an ordered, deduped, score-sorted track list and can be
> exported (**m3u8 / Traktor / ZIP**).

#333 = Export der **resolveten** Backpack-Trackliste in drei Formaten. Kein CRUD
(#328), keine View (#332), Tests sind #334 (aber eigener Test mitliefern).

## IST-Stand (RE, 2026-10-13)

- `music-hub/src/backpacks.rs`:
  - `resolve(pool, backpack_id, limit)` / `resolve_unbounded(...)` →
    `Vec<ResolvedTrack { track_id: i64, score: i64, sources: Vec<ResolvedSource> }>`
    (bereits sortiert/deduped/score-sorted). **Das ist die Export-Quelle.**
  - `list_backpacks(pool, user_id)`, `Backpack { id, name, is_active, ... }`.
- Track-Metadaten für Playlist-Einträge: `hub_tracks` (id, title, artists) + ISRC via
  `hub_track_external_ids` (Migration 006). Tracks-Download nutzt `t.isrc, t.title, t.artists`.
- Download-/ZIP-Muster bereits vorhanden in `music-hub/src/pages.rs`:
  - Route `GET /playlist/{id}/download` → `playlist_download`; `GET /tag/{id}/download`.
  - `DownloadQuery { format: Option<String> }`, `valid_format()` (default `flac`),
    `zip_for()` → `build_playlist_zip(cfg, tracks, format, tmp)` lädt Bytes über
    `music_api::file_bytes_any(cfg, isrc, format)` und schreibt `{artists} - {title}.{format}`.
  - `zip_response(tmp, filename)` liefert `Content-Disposition: attachment`.
- Route-Registrierung: `pages.rs` `router(state)` (Z. 22–73), Handler-Muster
  `crate::ui::nav(&st, &headers, "...")` → `None` ⇒ `Redirect /login`.
- Kein bestehender m3u/m3u8- oder Traktor-NML-Export im Hub (nur Import in
  `traktor.rs` / `legacy/mmm/src/traktor.rs`). NML-Import kennt das Format.
- Tests: `music-hub/tests/backpack_*.rs` + `tests/common` (temp SQLite, `spawn()`,
  `session_cookie(user_id)`).

## Stage-Status

| Stage | Status | Ergebnis |
|-------|--------|----------|
| 0 RE | done | siehe oben |
| 1 plan | pending | |
| 2 setup | done | Branch `feat/issue-333-backpack-export` von `main` @ 3540dbe (nur lokal). Baseline in `music-hub/`: `cargo build --locked` EXIT 0, `cargo test --locked --no-run` EXIT 0. Vorbestehende Warnungen: `music-hub` (lib) dead_code `tags`/`me` in `src/web.rs:986`; `tests/ui.rs:1713` unused var `cookie`. |
| 3 dev | done | US1–US6 umgesetzt. Dateien: `music-hub/src/backpacks.rs` (ExportTrack, export_tracks, backpack_owned_by, m3u8_playlist, traktor_nml, xml_escape + 7 Unit-Tests), `music-hub/src/pages.rs` (ExportQuery, valid_export_format, backpack_zip_rows, attachment, Route `/backpacks/{id}/export`, Handler `backpack_export`), `music-hub/templates/backpacks.html` (Export-Buttons), `music-hub/tests/backpack_export.rs` (neu, 10 Integrationstests). Verifikation (Host planet, aus `music-hub/`): `cargo build --locked` EXIT 0; `cargo test --locked -p music-hub` EXIT 0 (39 `test result: ok`-Blöcke, 0 failed; darunter alle 7 neuen Unit- + 10 neuen Integrationstests); `cargo clippy --locked -p music-hub --all-targets` EXIT 0 (keine neuen Warnungen aus neuem Code); neue Datei `cargo fmt --check`-clean (Repo-Baseline ist breit fmt-dirty, unverändert). Commits: `ff231f9` (US1–US3 backpacks.rs), `f3c206a` (US4 pages.rs), `dd99d07` (US5 Template), `e9f428c` (US6 Tests). Nicht gepusht (PR-Stage pusht). |
| 4 verify | done | **PASS** (unabhängige Verifikation, Host planet). Scope isoliert, Auth/Owner korrekt, Security ok, Build/Tests unabhängig reproduziert. 0 blocking, 3 non-blocking. Details siehe Stage-4-Report unten. |
| 5 test | done | **PASS** (Host planet). Neuer Test `backpack_export` EXIT 0 (11/11 nach Coverage-Ergänzung). Regression `backpack_{schema,sources,resolver,active,view}` je EXIT 0 (51/51). Crate-Smoke `cargo test --locked -p music-hub` EXIT 0 (39 `test result: ok`-Blöcke, 321 passed, 0 failed). Coverage: alle 6 Akzeptanzkriterien abgedeckt; 1 Lücke (Integration-Level `#EXTINF`) additiv geschlossen. Details siehe Stage-5-Report unten. |
| 6 PR | pending | |
| 7 review | pending | |

## Stage 4 — Verify (unabhängige Verifikation, Host planet)

**Verdict: PASS** — weiter zu Test. Diff `origin/main..HEAD` (`2b4d339`, `e9f428c`,
`dd99d07`, `f3c206a`, `ff231f9`, `dcdd4c9`) vollständig geprüft. 0 blocking, 3 non-blocking.

### Build / Tests (unabhängig reproduziert)
- `cd music-hub && cargo build --locked` → **EXIT 0** (nur vorbestehende Warnung `src/web.rs:986` dead_code `tags`/`me`).
- `cargo test --locked -p music-hub --test backpack_export` → **EXIT 0**, 10/10 passed.
- `cargo test --locked -p music-hub --lib export_tests` → **EXIT 0**, 7/7 passed.

### Scope (PASS)
- `git diff --name-only origin/main..HEAD` = nur `backpacks.rs`, `pages.rs`,
  `templates/backpacks.html`, `tests/backpack_export.rs`, `progress-issue-333.md` — reine Additionen.
- Keine Migration verändert (`music-hub/migrations/` unberührt).
- Keine Änderung an `resolve` / `resolve_unbounded` / `resolve_with_limit` (diff-Check leer); `export_tracks` liest `resolve_unbounded` nur.
- Keine #332-CRUD-Änderungen; `backpack_owned_by` ist neu und read-only.

### Auth / Owner (PASS)
- Ohne Session: `crate::ui::nav(...)` → `None` ⇒ `Redirect::to("/login")` (`pages.rs:~4560`); Integrationstest `export_requires_login` bestätigt 303 `/login`.
- Owner-Check im SQL über `backpack_owned_by` mit `nav.id` (`backpacks.rs:~773`, `WHERE id = ?1 AND user_id = ?2`) → fremdes/unbekanntes Backpack `None` ⇒ `not_found` (404), kein Cross-User-Leak. Test `export_foreign_backpack_is_404_and_own_is_200` bestätigt.

### Security (PASS)
- SQL durchgängig parametrisiert (`bind`), keine String-Interpolation.
- NML: `xml_escape` für `& < > " '` auf `TITLE`/`ARTIST`/`NODE NAME`/`FILE`/`KEY`; Unit-Test `nml_escapes_special_chars` parst `A&B <C>`/`D"e'f` korrekt und prüft `KEY`-Basename == `FILE`-Basename.
- Kein `unwrap()` auf User-/DB-Daten in neuen Pfaden: Handler nutzt `.ok().flatten()` / `.unwrap_or_default()`; `attachment` nutzt `unwrap_or_else`.
- Dateiname via `sanitize_filename` (ersetzt `/\:*?"<>|`), `Content-Disposition: attachment; filename="…"`.

### Korrektheit (PASS)
- m3u8: `#EXTM3U` → optional `#PLAYLIST:{name}` → je Track `#EXTINF:-1,{artists} - {title}` + `/track/{id}/stream`, `\n`-Enden, trailing newline, Reihenfolge = Resolve-Reihenfolge. Tests grün.
- NML: wohlgeformt (roxmltree parst), `COLLECTION/ENTRY` mit `LOCATION/FILE`+`TITLE`+`ARTIST`, `NODE TYPE="PLAYLIST"` mit `PRIMARYKEY`; `KEY`-Basename == `FILE`-Basename = `{artists} - {title}.{ext}` — deckt sich mit `traktor.rs::import_str` (Map `by_file` über `LOCATION@FILE`, PLAYLIST-Match via `key.rsplit('/')`).
- ZIP-Reuse: `backpack_zip_rows` füttert Rows in Resolve-Reihenfolge, `zip_for(&st, &rows, &audio, &label)` unverändert (502 bei 0 Dateien) — Test `export_zip_is_502_without_music_api` bestätigt.

### Non-blocking (3) — kein Blocker, zur Kenntnis
1. `music-hub/src/backpacks.rs:~800` — m3u8 ist ein Plaintext-Format ohne Escape-Konzept; `title`/`artists`/`name` werden unverändert geschrieben. Ein eingebettetes `\n` (aus DB-Metadaten bzw. Backpack-Name) könnte theoretisch zusätzliche Playlist-Zeilen einschleusen. Nur konsumentenseitig (eigene VLC-Session), keine Server-Auswirkung ⇒ unkritisch, aber notierenswert.
2. `music-hub/src/pages.rs:542` (`sanitize_filename`) entfernt keine `\r`/`\n`; ein Backpack-Name mit Zeilenumbruch macht den `CONTENT_DISPOSITION`-Headerwert invalid → `Response::builder().body()` schlägt fehl → `unwrap_or_else` liefert 500. Keine Header-Injection (http-Crate validiert), nur Degradation ⇒ non-blocking.
3. `music-hub/src/pages.rs:~4521` — `backpack_zip_rows` schluckt einen Resolve-Fehler zu `Vec::new()` ⇒ ZIP-Pfad meldet dann 502. Bewusst (spiegelt übrige Download-Handler), dokumentiert.

### Empfehlung
PASS → Stage 5 (Test). Non-blocking-Punkte optional als Folge-Ticket (m3u8-Newline-Sanitizing / Header-Härtung).


## Stage 5 — Test (Host planet)

**Verdict: PASS** — Akzeptanzkriterien vollständig abgedeckt (1 Coverage-Lücke additiv geschlossen).

### 1. Neuer Integrationstest
- `cd music-hub && cargo test --locked --test backpack_export` → **EXIT 0**
- `test result: ok. 11 passed; 0 failed; 0 ignored` (nach Coverage-Ergänzung; vorher 10/10).

### 2. Regression Backpack-Suite (je EXIT 0)
| Suite | test result |
|-------|-------------|
| backpack_schema | ok. 7 passed; 0 failed |
| backpack_sources | ok. 18 passed; 0 failed |
| backpack_resolver | ok. 11 passed; 0 failed |
| backpack_active | ok. 8 passed; 0 failed |
| backpack_view | ok. 7 passed; 0 failed |
| **Summe** | **51 passed; 0 failed** |

### 3. Crate-Smoke
- `cargo test --locked -p music-hub > /tmp/test333.log 2>&1; echo "EXIT: $?"` → **EXIT: 0**
- `grep -c "^test result: ok"` = **39**; `grep "FAILED\|failed"` = nur `0 failed`-Zeilen (keine echten Fehler); kein `test result` mit `!= 0 failed`.
- Summe über alle Blöcke: **321 passed, 0 failed**.

### 4. Coverage-Bewertung gegen die Akzeptanzkriterien
| Kriterium | Abgedeckt durch | Status |
|-----------|-----------------|--------|
| Login-Gate 303 `/login` | `export_requires_login` | ✅ |
| Owner-404 (fremd) / 200 (eigen) | `export_foreign_backpack_is_404_and_own_is_200` | ✅ |
| m3u8: `#EXTM3U`-Start, `#EXTINF` je Track, `/track/{id}/stream`, Reihenfolge | `export_m3u8_body_has_resolved_tracks_in_order` + **neu** `export_m3u8_has_extinf_per_track_in_order` | ✅ (ergänzt) |
| NML: XML-parsebar + `PRIMARYKEY`-Einträge | `export_traktor_body_parses_as_xml` | ✅ |
| ZIP: 502 ohne music-api (Testenv) | `export_zip_is_502_without_music_api` | ✅ |
| Leeres Backpack: 200, eintragsfreie gültige Doku | `export_empty_backpack_yields_valid_empty_documents` | ✅ |

**Gefundene Lücke:** Der HTTP-Integrationstest prüfte `#EXTINF` nicht explizit
(nur `#EXTM3U`/`#PLAYLIST`/Stream-Zeilen+Reihenfolge); US6 fordert aber „enthält
`#EXTINF` je resolvetem Track in korrekter Reihenfolge". Die Unit-Tests in
`backpacks.rs` deckten `#EXTINF` ab, der Integrationspfad nicht.
→ **Additiv geschlossen**: neuer Test `export_m3u8_has_extinf_per_track_in_order`
in `music-hub/tests/backpack_export.rs` prüft genau eine
`#EXTINF:-1,{artists} - {title}`-Zeile je resolvetem Track, direkt gefolgt von der
zugehörigen `/track/{id}/stream`-Zeile, in Resolve-Reihenfolge (2 Tracks,
`Aaa - Shared Anthem` vor `Ccc - Playlist Only`). Test 11/11 grün.

### Non-blocking (aus Stage 4, unverändert)
m3u8-Newline-Sanitizing / `CONTENT_DISPOSITION`-Header-Härtung / `backpack_zip_rows`-Fehler-Schlucken — kein Blocker, optionales Folge-Ticket.

### Ergebnis
PASS → Stage 6 (PR).
