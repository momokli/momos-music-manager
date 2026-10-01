# Progress — Issue #58: Test-Seed-Szenario liked_songs

Repo: momokli/momos-music-manager · Branch: feature/issue-58-liked-songs-seed · Base: origin/main 60f00a1 · Ziel: main

## Vertrag (aus Issue #58 — NICHT ohne Grund ändern)
`seed_liked_songs_scenario` erweitert `seed_basic_scenario` (Playlists 1 `Groovy`, 2 `Deep Mix`; Tracks 1–3):

| Zeile | Zweck |
| --- | --- |
| Playlist 5, Name `liked`, `playlist_kind='liked'`, `playlist_id='spotify:liked'` | Likes-Spiegel |
| Playlist 6, Name `Today's Selection`, `playlist_kind='generated'` | darf nie zählen/Tag werden |
| Playlist 7, Name `Likes`, `playlist_kind='liked'` | Name-Variante manueller Spiegel |
| Track 1 in Playlist 5, added_at=1500000000 | altes Like |
| Track 1 in Playlist 1, added_at=1600000000 | playlist_count=1, last_touched_at=1600000000 |
| Track 2 nur in Playlist 5, added_at=1400000000 | geliked, playlist_count=0 |
| Track 3 in Playlists 1+2, added_at=1700000000 | playlist_count=2, nicht geliked |

Danach `refresh_file_resolved_tags()` wie in `seed_lab_scenario`.

Registrierung:
- `src/api/infrastructure.rs` → `testing_seed_handler`: `"liked_songs" => testing::seed_liked_songs_scenario(&state.db).await`
- `tests/common/mod.rs` → `pub async fn seed_liked_songs_data(pool)`

## DoD
- [ ] POST /api/testing/seed {"scenario":"liked_songs"} → 200, plausible Zählungen
- [ ] unbekanntes Szenario bleibt 400
- [ ] `seed_liked_songs_data()` in tests/common/mod.rs, idempotent
- [ ] `clear_all_tables()` räumt neue Zeilen mit ab (kein Leak)

## Plan: Test-Seed-Szenario `liked_songs`

### Betroffene Dateien
- `src/db/testing.rs` — neue `seed_liked_songs_scenario`
- `src/api/infrastructure.rs` — `testing_seed_handler`-Match + Fehlertext
- `tests/common/mod.rs` — Wrapper `seed_liked_songs_data`
- `tests/api_infrastructure.rs` — DoD-Tests (Seed-Endpoint)
- `CHANGELOG.md` — `[Unreleased]`-Eintrag (kein Versionsbump)

### Vertrags-Abgleich (kritisch)
`v_track_forgotten_facts` (Migration 032) liefert pro Track `playlist_count` (nur `curated`),
`last_touched_at` (`MAX(added_at)` über `curated`+`liked`) und `liked`. `seed_basic_scenario`
setzt aber bereits `service_playlist_tracks`: `(1,1,·,1700000000)` und `(2,2,·,1700000000)`.
Damit die Soll-Zahlen **1 / 0 / 2** herauskommen, muss `seed_liked_songs_scenario` diesen
Bestand gezielt überschreiben:
- `UPDATE` Zeile Playlist 1 / Track 1 → `added_at=1600000000`
- `DELETE` Zeile Playlist 2 / Track 2 (sonst wäre Track 2 `playlist_count=1`, nicht 0)
- `INSERT` Track 3 → Playlist 1 **und** 2 (`added_at=1700000000`)

### User Stories (in Reihenfolge)

1. **Seed-Funktion `seed_liked_songs_scenario` implementieren**
   - Akzeptanzkriterien: ruft zuerst `seed_basic_scenario(pool)`; legt Playlist 5
     (`name='liked'`, `playlist_id='spotify:liked'`, `playlist_kind='liked'`), Playlist 6
     (`name='Today's Selection'`, `playlist_kind='generated'`) und Playlist 7
     (`name='Likes'`, `playlist_kind='liked'`) an; setzt die Track-Verknüpfungen exakt gemäß
     Vertrag (Track 1/PL5 `1500000000`, Track 1/PL1 `1600000000`, Track 2/PL5 `1400000000`,
     Track 3/PL1+PL2 `1700000000`) inkl. der o. g. `UPDATE`/`DELETE`-Reconciliation;
     ruft abschließend `refresh_file_resolved_tags()` (wie `seed_lab_scenario`); alle INSERTs
     `INSERT OR IGNORE` (idempotent).
   - Betroffene Dateien: `src/db/testing.rs`
   - Geschätzte Lines of Code: ~70

2. **`counts`-Rückgabe korrekt fortschreiben**
   - Akzeptanzkriterien: `service_playlists` 2→5, `service_playlist_tracks` 2→5
     (2 − 1 gelöscht + 2 für PL5 + 2 für Track 3); übrige Keys unverändert.
   - Betroffene Dateien: `src/db/testing.rs`
   - Geschätzte Lines of Code: ~5

3. **Endpoint-Registrierung**
   - Akzeptanzkriterien: `"liked_songs" => testing::seed_liked_songs_scenario(&state.db).await`
     im Match; Fehlertext-Liste um `liked_songs` ergänzen; unbekanntes Szenario bleibt 400.
   - Betroffene Dateien: `src/api/infrastructure.rs`
   - Geschätzte Lines of Code: ~3

4. **Test-Wrapper `seed_liked_songs_data`**
   - Akzeptanzkriterien: `pub async fn seed_liked_songs_data(pool)` in `tests/common/mod.rs`,
     delegiert an `db::testing::seed_liked_songs_scenario`; idempotent (doppelter Aufruf
     ändert Ergebnis nicht).
   - Betroffene Dateien: `tests/common/mod.rs`
   - Geschätzte Lines of Code: ~6

5. **DoD-Tests am Seed-Endpoint**
   - Akzeptanzkriterien: `POST /api/testing/seed {"scenario":"liked_songs"}` → 200 + plausible
     `rows`; unbekanntes Szenario → 400; gegen `v_track_forgotten_facts`: Track 1
     `playlist_count=1`/`last_touched_at=1600000000`/`liked`, Track 2 `0`/`1400000000`/`liked`,
     Track 3 `2`/`1700000000`/nicht liked; Playlist 6 (`generated`) wird nie gezählt/Tag;
     `clear_all_tables()` hinterlässt keine liked-Zeilen (kein Leak Richtung Folgetest).
   - Betroffene Dateien: `tests/api_infrastructure.rs`
   - Geschätzte Lines of Code: ~60

### Offene Punkte / nicht erfinden
- Für Playlists 6 und 7 nennt der Vertrag **keine** Track-Mitgliedschaften → keine erfinden;
  die „generated zählt nie"-Aussage wird über `playlist_kind` von PL6 geprüft, nicht über
  zusätzliche Zeilen. Falls die Verifikation eine Mitgliedschaft in PL6 braucht: im Issue klären.
- Scope-Verbot beachten: kein Versionsbump, keine Migration, kein `global_poller`/`liked_sync`.

## Stage-Status
| Stage | Status | Ergebnis |
| --- | --- | --- |
| planner | pending | |
| setup | done | Branch `feature/issue-58-liked-songs-seed` auf 60f00a1 angelegt (remote noch ohne Branch → lokal erstellt). Baseline grün: `cargo check --all-targets` EXIT=0 (48s), `cargo test --no-run` EXIT=0 (1m11s, nur Warnungen). Test-Subset mit `TMPDIR=/dev/shm/mmm-tmp`: api_infrastructure 9/9 ok, migration_integrity 1/1 ok (EXIT=0). CARGO_TARGET_DIR=/tmp/mmm-base/target (Deps-Cache) genutzt. Keine Code-Änderungen. |
| developer | done | `seed_liked_songs_scenario` in src/db/testing.rs (ruft seed_basic, PL5/6/7, Reconciliation UPDATE (1,1)+DELETE (2,2), 4 INSERT OR IGNORE, refresh_file_resolved_tags, counts 5/5); Endpoint `liked_songs` + Fehlertext in src/api/infrastructure.rs; Wrapper `seed_liked_songs_data` in tests/common/mod.rs; 3 DoD-Tests in tests/api_infrastructure.rs. `cargo check --all-targets` EXIT=0; `cargo test --test api_infrastructure --test migration_integrity` 12+1 passed EXIT=0 (TMPDIR=/dev/shm/mmm-tmp58). |
| verifier | done | **PASS** (Checkpoint 679c425). Diff-Scope: erlaubte 5 Dateien ok; **Hinweis:** `progress-feature-issue-58-liked-songs-seed.md` ist in 679c425 mitcommittet (untracked erwartet) — nicht in origin/main, aber in Repo-Historie (main) gibt es bereits getrackte `progress-*.md`, daher konsistent; vor PR optional `git rm --cached`. Vertragstreue vs. `v_track_forgotten_facts`: Track1 (1,1600000000,liked), Track2 (0,liked), Track3 (2,1700000000,not liked) — bestätigt durch Test. PL6 generated zählt nie (View filtert `playlist_kind IN ('curated','liked')`). Idempotenz doppelt stabil, `clear_all_tables()` entfernt PL5-7 + Tracks (kein Leak). Endpoint 200/400 + Fehlertext konsistent. Secret-Scan: keine Treffer. Debug: nur `eprintln!` im Test (Repo-Konvention). `cargo test --test api_infrastructure --test migration_integrity` → 12+1 passed, EXIT=0 (TMPDIR=/dev/shm/momo-58). |
| tester | done | **PASS** (Checkpoint 679c425). Integration (TMPDIR=/dev/shm/momo-58-tmp, CARGO_TARGET_DIR=/tmp/mmm-base/target): `cargo test --lib --test api_infrastructure --test migration_integrity --test api_playlists --test migration_032_forgotten_facts` → **EXIT=0**; `test result`-Zeilen: lib 762 passed/0 failed, api_infrastructure 12/12, api_playlists 29/29, migration_032_forgotten_facts 4/4, migration_integrity 1/1. Doc-Tests separat ohne tmpfs: `cargo test --doc` → EXIT=0, 2 passed. E2E über Test-App: `POST /api/testing/seed {"scenario":"liked_songs"}` → **200**, Body `{"ok":true,"scenario":"liked_songs","rows":{"service_playlists":5,"service_playlist_tracks":5,"service_tracks":3,"files":4,"file_locations":6,"folders":1,"tags":3}}`; unbekanntes Szenario (`nope_not_a_scenario`) → **400**, Fehlertext listet `liked_songs`. Vertrag via `v_track_forgotten_facts`: Track1 (playlist_count=1, last_touched_at=1600000000, liked), Track2 (0, 1400000000, liked), Track3 (2, 1700000000, not liked); PL6 `generated` liefert 0 View-Zeilen. Leak: doppeltes Seeden idempotent (5 Playlists), `clear_all_tables()` → 0 Zeilen in PL 5–7 und 0 `service_playlist_tracks`. Flaky-Check: 5 Wiederholungen der liked_songs-Tests grün (0 failed) → keine Flakes, keine Reproduktion auf origin/main nötig. `api_rediscovery`-Test existiert im Repo nicht (kein `rediscover`-Treffer); nächster fachlicher Bezug `migration_032_forgotten_facts.rs` mitgetestet. Nebenbefund: dead_code-Warnung `seed_liked_songs_data` in Test-Binaries, die den Helper nicht nutzen (erwartet, kein Blocker). Keine Code-Änderungen. |
| developer-pr | done | PR #105 gegen `main`: https://github.com/momokli/momos-music-manager/pull/105 (Body mit Closing-Keyword `Closes #58`). Branch `feature/issue-58-liked-songs-seed` auf 679c425, 1 Ahead / 0 Behind ggü. origin/main 60f00a1. |
| reviewer | pending | |

## Scope-Verbot
Kein Bump von CHANGELOG-Version (nur `[Unreleased]`-Eintrag), keine Migration, kein global_poller/liked_sync (Issues #59–#62), keine Doku außerhalb.
