# Progress — Issue #57 (Fokus-Milestone 1.14.0)

Branch: `fix/issue-57-liked-generated` (Basis: `origin/main` @ 774cf22, unabhängig mergebar)
PR-Ziel: `main` · PR-Body MUSS `Closes #57` enthalten.

Repo-Worktree: `/srv/openclaw/workspace/momos-music-manager-issue57`
Host: planet · Bot-Identity: `momo-clanker[bot]` über `clanker-git` / `clanker-gh` (NIE nacktes git/gh).

## Issue

`[Fix] Global Poller: liked/generated aus Snapshot-Query und Deleted-Detection ausschliessen`

Zwei Stellen setzen voraus, dass alle `service='spotify'`-Zeilen der Ausgabe von
`GET /me/playlists` entsprechen. Die `spotify:liked`-Zeile liegt aber nie in
`/me/playlists` → Step 4 markiert sie jeden Zyklus als gelöscht und nullt `snapshot_id`.

## Ist-Stand (verifiziert an origin/main 774cf22)

- `src/db/playlists.rs` `get_spotify_playlist_snapshots` (Z. ~709):
  `SELECT id, playlist_id, snapshot_id FROM service_playlists WHERE service = 'spotify'`
  → KEIN `playlist_kind`-Filter. (Migration 032 existiert: `playlist_kind` DEFAULT 'curated',
  Werte curated|liked|generated; liked=Name 'liked'/'likes', generated=local 'Daily-%'.)
- `get_playlists_without_tags` (Z. ~12) und `create_tags_from_playlists` (Z. ~31):
  `WHERE TRIM(sp.name) != '' AND NOT EXISTS(...)` → kein kind-Filter.
- `refresh_track_tags` (Z. ~400): `SELECT DISTINCT sp.name ... WHERE t.id IS NULL` → kein kind-Filter.
- `src/global_poller.rs`: Step 1 (Z. ~138) und Step 4 (Z. ~347-363) nutzen
  `get_spotify_playlist_snapshots`; Step 4 ruft `mark_playlist_inactive` für unbekannte IDs.

Kein gemergter Fix vorhanden → Issue NICHT already-done.

## Umsetzung (Soll)

1. `get_spotify_playlist_snapshots`: `... WHERE service = 'spotify' AND playlist_kind = 'curated'`.
   (Damit schließen Step 1 UND Step 4 liked + generated aus; liked wird nie als gelöscht gemeldet.)
2. `create_tags_from_playlists`, `get_playlists_without_tags`, `refresh_track_tags`:
   `AND sp.playlist_kind != 'generated'` (liked DARF ein Tag erzeugen — gewollt, #50-Epic).
3. Kuratierte Playlists: kein Verhalten ändert sich.

## Definition of Done (Tests Pflicht)

- [ ] Test: Snapshot-Query liefert nur `curated` (liked + generated ausgeschlossen).
- [ ] Test: `liked` erzeugt ein Tag; `Today's Selection` / `Daily-*` erzeugen keines.
- [ ] Test: nach einem Poll-Zyklus bleibt `snapshot_id` der liked-Zeile unverändert
      (wird nie als gelöscht → `mark_playlist_inactive`).
- [ ] Bestehende Tests für kuratierte Playlists bleiben grün.

Referenzen: `src/global_poller.rs:135-148`, `:342-363`, `src/db/playlists.rs:709`, Migration 032.

## Pipeline-Stages

- [ ] plan
- [ ] setup (branch/baseline)
- [ ] develop (impl + Tests)
- [ ] verify
- [ ] test
- [ ] PR
- [ ] review

---

## Plan (planner)

Verifiziert gegen `origin/main` @ `774cf22` im Worktree `/srv/openclaw/workspace/momos-music-manager-issue57`.
Alle im Ist-Stand genannten Stellen existieren unverändert. Kein gemergter Fix. Migration 032
liefert `playlist_kind TEXT NOT NULL DEFAULT 'curated'` (`curated|liked|generated`); die Test-Helper
`test_db()` in `src/db/playlists.rs:980` legt `playlist_kind` bereits an, legt aber **keine** `tags`-,
`tag_categories`-Tabelle und **keine** `v_tag_playlist`-View an — die Tag-Tests brauchen daher einen
erweiterten Test-Helper.

### Betroffene Dateien
- `src/db/playlists.rs` (4 Queries + neue Tests)
- `src/global_poller.rs` (nur indirekt — keine Code-Änderung nötig, Consumer der Query)

### Exakte SQL-Änderungen (Ist-Stelle → Soll-Filter)

1. **`get_spotify_playlist_snapshots`** (`src/db/playlists.rs:709`, SQL-Zeile ~712)
   - Ist: `SELECT id, playlist_id, snapshot_id FROM service_playlists WHERE service = 'spotify'`
   - Soll: `... WHERE service = 'spotify' AND playlist_kind = 'curated'`
   - Wirkung: Step 1 (`src/global_poller.rs:138`) und Step 4 (`:347-363`) sehen liked/generated
     nicht mehr → liked wird nie als gelöscht erkannt, `mark_playlist_inactive` wird nicht aufgerufen.

2. **`get_playlists_without_tags`** (`src/db/playlists.rs:12`, SQL-Zeile ~17-18)
   - Ist: `WHERE TRIM(sp.name) != '' AND NOT EXISTS (...)`
   - Soll: `WHERE TRIM(sp.name) != '' AND sp.playlist_kind != 'generated' AND NOT EXISTS (...)`
   - liked darf hier bleiben (soll Tag erzeugen, #50-Epic).

3. **`create_tags_from_playlists`** (`src/db/playlists.rs:31`, SQL-Zeile ~45-46)
   - Ist: `WHERE TRIM(sp.name) != '' AND NOT EXISTS (...)`
   - Soll: `WHERE TRIM(sp.name) != '' AND sp.playlist_kind != 'generated' AND NOT EXISTS (...)`

4. **`refresh_track_tags`** (`src/db/playlists.rs:400`, SQL-Zeile ~411-417)
   - Ist: `... WHERE t.id IS NULL ORDER BY sp.name`
   - Soll: `... WHERE t.id IS NULL AND sp.playlist_kind != 'generated' ORDER BY sp.name`

Hinweis: `!= 'generated'` (nicht `= 'curated'`), damit liked weiterhin Tags erzeugt und
NULL-Altbestände (vor 032) nicht versehentlich ausgeschlossen werden.

### User Stories (in Reihenfolge)

**US1 — Snapshot-Query auf curated beschränken**
- SQL-Filter in `get_spotify_playlist_snapshots` (Stelle 1).
- Akzeptanz: liked-/generated-Zeilen tauchen nicht im Ergebnis auf; kuratierte unverändert.
- Dateien: `src/db/playlists.rs:709`. LOC: ~1.
- Abhängigkeit: keine. Zuerst bauen.

**US2 — Tag-Erzeugung aus generated ausschliessen**
- Filter in `create_tags_from_playlists`, `get_playlists_without_tags`, `refresh_track_tags`
  (Stellen 2-4).
- Akzeptanz: liked erzeugt Tag; generated (`Daily-%`, `Today's Selection`) nicht; curated unverändert.
- Dateien: `src/db/playlists.rs:12,31,400`. LOC: ~3.
- Abhängigkeit: US1 unabhängig, aber Tests brauchen gemeinsamen erweiterten Helper.

**US3 — Tests (DoD)**
- Vier Testfälle (siehe unten), inkl. Erweiterung des Test-Helfers um `tags`/`tag_categories`/
  `v_tag_playlist`.
- Dateien: `src/db/playlists.rs` (Testmodul). LOC: ~120.
- Abhängigkeit: US1 + US2.

**US4 — Regression kuratierte Playlists**
- Bestehende Tests `test_get_spotify_playlist_snapshots_*` (Z.1856/1863), `test_mark_playlist_inactive`
  (Z.1276) und Tag-Flows müssen grün bleiben.
- Dateien: keine Änderung. LOC: 0.
- Abhängigkeit: US1-US3.

### DoD-Tests als konkrete Testfälle (`src/db/playlists.rs`, `mod tests`)

Ort: direkt neben `test_get_spotify_playlist_snapshots_with_data` (Z.1863). Helper-Erweiterung:
zweiter Helper `test_db_with_tags()` analog `test_db()`, zusätzlich:

```sql
CREATE TABLE tag_categories (id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL,
  icon TEXT NOT NULL DEFAULT '', prefix CHAR(1) UNIQUE NOT NULL,
  sort_order INTEGER DEFAULT 0, is_default BOOLEAN DEFAULT FALSE,
  created_at INTEGER DEFAULT (unixepoch()));
CREATE TABLE tags (id INTEGER PRIMARY KEY, name TEXT UNIQUE NOT NULL,
  category_id INTEGER NOT NULL, sort_order INTEGER DEFAULT 0,
  created_at INTEGER DEFAULT (unixepoch()), reviewed_at INTEGER);
CREATE VIEW v_tag_categories AS SELECT tc.*,
  (SELECT COUNT(*) FROM tags WHERE category_id = tc.id) AS tag_count FROM tag_categories tc;
CREATE VIEW v_tag_playlist AS SELECT t.id AS tag_id, t.name AS tag_name, t.category_id,
  sp.id AS playlist_id, sp.name AS playlist_name, sp.service
  FROM tags t JOIN service_playlists sp
  ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name));
INSERT INTO tag_categories (id,name,prefix,is_default) VALUES (1,'Setlist','s',1);
```

1. **`test_get_spotify_playlist_snapshots_excludes_liked_and_generated`**
   - Setup: `test_db()`; INSERT 3 Spotify-Zeilen: (`curated`, `pl-c`, 'C', snap 'c1'),
     (`liked`, `spotify:liked`, 'liked', snap 'l1'), (`generated`, `gen-1`, 'Daily-1', snap 'g1').
   - Assertion: `snapshots.len() == 1`; `snapshots[0].1 == "pl-c"`; enthält weder `spotify:liked`
     noch `gen-1`. (Ergänzt den bestehenden Test um die kind-Dimension.)

2. **`test_tag_creation_liked_but_not_generated`**
   - Setup: `test_db_with_tags()`; INSERT 3 Zeilen: (`liked`, `spotify:liked`, 'liked'),
     (`generated`, `gen-1`, "Today's Selection"), (`curated`, `pl-c`, 'C') — alle ohne Tag.
   - Act: `let created = create_tags_from_playlists(&pool).await.unwrap();`
   - Assertion: `created == 2` (liked + curated); Tag `liked` existiert
     (`SELECT COUNT(*) FROM tags WHERE name='liked'` == 1); Tag `Today's Selection` == 0.
   - Zusatz: `get_playlists_without_tags` liefert die generated-Zeile nicht;
     `refresh_track_tags` legt für `Daily-X` kein Tag an.

3. **`test_liked_playlist_not_marked_deleted_in_poll_cycle`**
   - Setup: `test_db()`; INSERT liked-Zeile mit `snapshot_id='liked-snap'` und curated-Zeile.
   - Act: Step-4-Logik nachbilden: `rows = get_spotify_playlist_snapshots(&pool)`; `spotify_ids`
     = nur kuratierte Remote-IDs (simuliert `/me/playlists`, das liked nicht enthält); für jede
     `(db_id,pid,_)` mit `!spotify_ids.contains(pid)` → `mark_playlist_inactive(&pool, db_id)`.
   - Assertion: `SELECT snapshot_id FROM service_playlists WHERE playlist_id='spotify:liked'` ==
     `Some("liked-snap")` (unverändert); curated-Zeile unverändert; `deleted_count == 0`.
   - Alternativ (stärker an Step 4 gekoppelt): `mark_playlist_inactive`-Aufruf zählen und == 0 erwarten.

4. **`test_curated_playlists_still_detected_as_deleted`** (Regression/Positiv-Kontrolle)
   - Setup: `test_db()`; INSERT curated-Zeile `pl-gone` mit snapshot; `spotify_ids` leer/ohne `pl-gone`.
   - Assertion: Step-4-Nachbildung ruft `mark_playlist_inactive` auf → `snapshot_id` der Zeile wird
     NULL; `deleted_count == 1`. Beweist, dass der Filter nur liked/generated trifft.

### Implementierungs-Reihenfolge
1. US1 (Snapshot-Filter) → 2. US2 (Tag-Filter) → 3. Helper `test_db_with_tags()` →
4. Tests 1-4 → 5. `cargo test` (Playlists-Tests + global_poller-Kompilierung) → 6. Regression grün.

### Offene Punkte / Annahmen
- `test_db()` legt heute weder `tags` noch `v_tag_playlist` an → Helper-Erweiterung ist Pflicht für Test 2.
- Test 3 bildet die Step-4-Schleife nach (kein End-to-End-Poll-Test ohne Spotify-Client);
  die eigentliche Absicherung leistet US1 über die Query. Bewusst so gewählt.
- `!= 'generated'` statt `= 'curated'` bei Tag-Queries, um liked (gewollt, #50) und NULL-Altbestand zu erhalten.

---

## Setup (setup)

- **Branch:** `fix/issue-57-liked-generated` — trackt `origin/main`, up to date. Identity: `momo-clanker[bot]` via `clanker-git`.
- **HEAD:** `774cf22` (`feat(#56): Migration 032 — playlist_kind + v_track_forgotten_facts (#103)`)
- **Baseline-Build:** 🟢 GRÜN. `cargo check --all-targets` → `Finished` in 53.18s (nur Warnings, keine Errors).
- **Test-Kompilierung:** 🟢 GRÜN. `cargo test --lib --no-run` → `Finished` in 1m12s, Executable `momos_music_manager-dc0a4b0a1dc7e4c9` erzeugt (nur Warnings).
- **Arbeitsbaum:** clean außer untracked `progress-fix-issue-57-liked-generated.md`. Kein Produktionscode geändert.
- **Blocker:** keine.

---

## Develop (developer)

- **Geänderte Datei:** `src/db/playlists.rs` (4 SQL-Filter + Test-Helper + 4 Tests)
  1. `get_spotify_playlist_snapshots`: `WHERE service = 'spotify' AND playlist_kind = 'curated'`.
  2. `get_playlists_without_tags`: `AND sp.playlist_kind != 'generated'`.
  3. `create_tags_from_playlists`: `AND sp.playlist_kind != 'generated'`.
  4. `refresh_track_tags`: `AND sp.playlist_kind != 'generated'`.
  - liked bleibt bei den Tag-Queries drin (erzeugt weiter Tag, #50); nur `generated` ausgeschlossen.
- **Neuer Test-Helper:** `test_db_with_tags()` (test_db + tag_categories/tags + v_tag_playlist/v_tag_categories).
- **Neue Tests (alle grün):**
  - `test_get_spotify_playlist_snapshots_excludes_liked_and_generated`
  - `test_tag_creation_liked_but_not_generated`
  - `test_liked_playlist_not_marked_deleted_in_poll_cycle`
  - `test_curated_playlists_still_detected_as_deleted`
- **Testergebnis:** `cargo test --lib playlists` → 36 passed / 0 failed.
  Volle `cargo test --lib` → 762 passed / 0 failed. Keine Regression.
- **Commit-Hash:** f491c6d
- **Push:** PLACEHOLDER
