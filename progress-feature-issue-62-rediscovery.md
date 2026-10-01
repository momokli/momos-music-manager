# Progress — Issue #62: db/rediscovery.rs TrackFacts

Repo: momokli/momos-music-manager
Base: main @ 3eb7e0d
Branch: feature/issue-62-rediscovery-track-facts
Milestone: 1.14.0
PR target: main

## Dispatch-Validierung
- Kein PR für #62 (offen/merged) -> normaler Pipeline-Run.
- `src/db/rediscovery.rs` existiert nicht.
- Deps #56–#61 gemergt in main.

## Stage-Status
| Stage | Status | Ergebnis |
| --- | --- | --- |
| 1 planner | pending | |
| 2 setup | done | Baseline grün — siehe Abschnitt „Setup-Baseline" |
| 3 developer | done | 6/6 Unit-Tests grün; `cargo check --lib` grün; Commit `28e571d` (+ Docs) |
| 4 verifier | pending | |
| 5 tester | pending | |
| 6 developer (PR) | pending | |
| 7 reviewer | pending | |

## Setup-Baseline (Stage 2)
- Branch: `feature/issue-62-rediscovery-track-facts` (bestätigt)
- Arbeitsbaum: sauber bis auf untracked `progress-feature-issue-62-rediscovery.md`
- HEAD: `3eb7e0d` (main-basiert, unverändert)
- Toolchain: cargo 1.97.1 (c980f4866 2026-06-30), rustc 1.97.1 (8bab26f4f 2026-07-14)
- Build: `cargo check --all-targets` → **EXIT_CODE=0** in 49.41s (grün; nur vorbestehende Warnungen, keine Fehler)

## Issue-Zusammenfassung
`src/db/rediscovery.rs` neu: typisierter Zugang über `v_track_forgotten_facts` + File-Join
über `v_file_track_link`.
- `TrackFacts` (FromRow): track_id, playlist_count, last_touched_at, liked_at, liked,
  bpm, musical_key, genre, play_count, last_played, in_backpack (file-seitig nullable).
- `get_track_facts(pool, track_id) -> Result<Option<TrackFacts>>`
- `count_touched_before(pool, days) -> Result<i64>`
- `in_backpack` über Namen-Match aus `src/backpack.rs` (Backpack-Definition), nicht neu implementieren.
- mod.rs registrieren.
- Kein `SELECT *`.

### DoD-Tests
1. get_track_facts liefert Seed-Werte (playlist_count 1/0/2, liked_at, BPM/Key aus File-Join).
2. Track ohne File -> bpm/musical_key None, kein Fehler.
3. count_touched_before(365) zählt nur kuratierte+liked-Kontakte älter als Grenze.

### Test-Seed
`db::testing::seed_liked_songs_scenario` (Issue #58, gemergt):
- Track 1: playlist_count=1, last_touched_at=1600000000, liked
- Track 2: playlist_count=0, last_touched_at=1400000000, liked
- Track 3: playlist_count=2, last_touched_at=1700000000, not liked
- Files: file1/2 -> track1 (bpm 128.0, key 4m), file3 -> track2 (bpm 140.0, key 8m),
  file4 unlinked. Track 3 hat kein File.

## Plan

### Betroffene Dateien
- `src/db/rediscovery.rs` (neu)
- `src/db/mod.rs` (2 Zeilen)

### User Stories (in Reihenfolge)

1. **Modul-Gerüst + `TrackFacts` (FromRow)**
   - Akzeptanzkriterien: Datei existiert; Struct hat genau die 11 Felder
     (`track_id: i64`, `playlist_count: i64`, `last_touched_at: Option<i64>`,
     `liked_at: Option<i64>`, `liked: bool`, `bpm: Option<f64>`,
     `musical_key: Option<String>`, `genre: Option<String>`,
     `play_count: Option<i64>`, `last_played: Option<i64>`,
     `in_backpack: Option<bool>`); `#[derive(Debug, Clone, FromRow)]`; Modul kompiliert.
   - Betroffene Dateien: `src/db/rediscovery.rs`
   - LOC: ~30

2. **`get_track_facts` — Basis-Query ohne File-Join**
   - Akzeptanzkriterien: Signatur `pub async fn get_track_facts(pool: &Pool<Sqlite>, track_id: i64) -> Result<Option<TrackFacts>>`;
     liest `track_id, playlist_count, last_touched_at, liked_at, liked` aus
     `v_track_forgotten_facts WHERE track_id = ?`; keine Zeile -> `Ok(None)`;
     **kein `SELECT *`**, Spalten explizit.
   - Betroffene Dateien: `src/db/rediscovery.rs`
   - LOC: ~25

3. **File-Join via `v_file_track_link` (LEFT JOIN, nullable)**
   - Akzeptanzkriterien: LEFT JOIN auf ein File über den Link-View;
     liefert `f.bpm, f.musical_key, f.genre, f.play_count, f.last_played`.
     Track ohne File -> diese Felder `None`, kein Fehler. Bei mehreren Files pro
     Track deterministisch (`ORDER BY f.id LIMIT 1`).
   - Betroffene Dateien: `src/db/rediscovery.rs`
   - LOC: ~25

4. **`in_backpack` aus `crate::backpack::get_backpack_track_ids`**
   - Akzeptanzkriterien: `in_backpack` ist file-seitig: `None` wenn kein File
     verknüpft, sonst `Some(backpack_ids.contains(track_id))`. Backpack-Set via
     `get_backpack_track_ids(pool)` holen (Reuse, keine Neuimplementierung).
   - Betroffene Dateien: `src/db/rediscovery.rs`
   - LOC: ~15

5. **`count_touched_before(pool, days) -> Result<i64>`**
   - Akzeptanzkriterien: Signatur `pub async fn count_touched_before(pool: &Pool<Sqlite>, days: i64) -> Result<i64>`;
     zählt aus `v_track_forgotten_facts WHERE liked = 1 AND last_touched_at IS NOT NULL
     AND last_touched_at < (strftime('%s','now') - days*86400)`; nutzt nur die View
     (curated+liked-Guard liegt dort). Rückgabe `i64` (COUNT).
   - Betroffene Dateien: `src/db/rediscovery.rs`
   - LOC: ~15

6. **mod.rs-Registrierung**
   - Akzeptanzkriterien: `pub mod rediscovery;` alphabetisch einsortiert und
     `pub use rediscovery::*;` analog zum bestehenden Muster; `crate::db::get_track_facts`
     und `crate::db::count_touched_before` erreichbar.
   - Betroffene Dateien: `src/db/mod.rs`
   - LOC: ~2

7. **Unit-Tests (DoD) gegen `db::testing::seed_liked_songs_scenario`**
   - Akzeptanzkriterien:
     a) `get_track_facts(track1)` -> `playlist_count=1`, `last_touched_at=1600000000`,
        `liked=true`, `liked_at=Some(1500000000)`, `bpm=Some(128.0)`, `musical_key=Some("4m")`;
        `get_track_facts(track2)` -> `playlist_count=0`, `liked=true`, `bpm=Some(140.0)`,
        `musical_key=Some("8m")`; `get_track_facts(track3)` -> `playlist_count=2`, `liked=false`.
     b) Track 3 (kein File) -> `bpm`/`musical_key` sind `None`, `in_backpack=Some(false)`,
        kein Fehler.
     c) `count_touched_before(365)` zählt nur kuratierte+liked-Kontakte älter als die
        Grenze; im Seed zählen nur die beiden liked-Tracks (Track 3 `liked=false` nicht).
   - Betroffene Dateien: `src/db/rediscovery.rs` (`#[cfg(test)] mod tests`)
   - LOC: ~70

### Abhängigkeiten / Reihenfolge
1 -> 2 -> 3 -> 4 -> 5 -> 6 -> 7. Story 4 kann parallel zu 3 begonnen werden,
setzt aber Struct (1) und Query (2) voraus. Story 7 erst nach 2–6.

### Offene Punkte
- Mehrere Files pro Track: deterministische Auswahl (`ORDER BY f.id LIMIT 1`).
- `count_touched_before`: "Kontakt" = View-Zeile (`last_touched_at`), nicht File-Zeit.

## Stage 3 — Developer

### Ergebnis
- Neu: `src/db/rediscovery.rs` mit `TrackFacts` (FromRow, 11 Felder),
  `get_track_facts(pool, track_id) -> Result<Option<TrackFacts>>`,
  `count_touched_before(pool, days) -> Result<i64>`.
- `src/db/mod.rs`: `pub mod rediscovery;` + `pub use rediscovery::*;` (alphabetisch).
- Basis-Query auf `v_track_forgotten_facts` (ADR-072-Guard kommt aus der View).
- File-Join über `v_file_track_link` → `files` (Spalten explizit, kein `SELECT *`),
  deterministisch `ORDER BY f.id LIMIT 1`.
- `in_backpack` file-seitig: `None` ohne verknüpftes File, sonst
  `Some(crate::backpack::get_backpack_track_ids(pool).contains(&track_id))`.

### Tests
- `TMPDIR=/dev/shm cargo test --lib db::rediscovery` → **EXIT=0**,
  `6 passed; 0 failed` (Log: `/tmp/rediscovery-test.log`).
- Seed: `db::testing::seed_liked_songs_scenario`; geprüft: Track 1/2/3-Werte,
  Track 3 ohne File → File-Felder `None`, unbekannter Track → `Ok(None)`,
  `count_touched_before(365) == 2`.

### Commits
- `28e571d` feat: add db::rediscovery TrackFacts access

### Hinweis / Abweichung zum Plan
- Plan-Story 7b nannte für Track 3 (kein File) `in_backpack=Some(false)`; Story 4
  und der Task-Auftrag verlangen dagegen file-seitig `None`. Umgesetzt wurde
  `None` (maßgeblich: Story 4 + Deliverable); Test entsprechend angepasst.
