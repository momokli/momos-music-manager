# Progress: Issue #79 — Migration 033 rediscovery_pushes-Ledger

Branch: `feat/issue-79-rediscovery-ledger` (base `origin/main` @ bd22fd5)
PR target: `main`  ·  Closing keyword: `Closes #79`  ·  Milestone 1.15.0

## Pre-check
- Migration 033 nicht auf `origin/main` (nur 001–032) → Issue NICHT erledigt, Pipeline läuft.
- `src/db/rediscovery.rs` existiert bereits (M1, `v_track_forgotten_facts` aus 032).
- Muster: `migrations/022_task_history.sql`, `migrations/032_playlist_kind.sql`.
- Canary: `tests/migration_integrity.rs`.

## Spec (aus Issue #79)
Migration `033_rediscovery_pushes.sql`:
```sql
CREATE TABLE IF NOT EXISTS rediscovery_pushes (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    track_id    INTEGER NOT NULL REFERENCES service_tracks(id) ON DELETE CASCADE,
    playlist_id INTEGER NOT NULL REFERENCES service_playlists(id) ON DELETE SET NULL,
    pushed_at   INTEGER NOT NULL,
    facet_json  TEXT,
    slot        INTEGER
);
CREATE INDEX idx_rediscovery_pushes_track ON rediscovery_pushes(track_id, pushed_at);
CREATE INDEX idx_rediscovery_pushes_slot  ON rediscovery_pushes(slot, pushed_at);
```

## DoD
- [x] frische DB 001→033 (create_test_db)
- [x] `tests/migration_integrity.rs` grün (Canary erwartet `rediscovery_pushes`)
- [x] Test: gelöschtes Playlist-Pack → playlist_id NULL, Zeile bleibt
- [x] Test: gelöschter service_track → CASCADE räumt Ledger-Zeilen
- [x] EXPLAIN QUERY PLAN nutzt Index bei `pushed_at > now - N`

## Abweichung von Spec (begründet)
- `playlist_id` ist **nullable** (`INTEGER REFERENCES … ON DELETE SET NULL`), nicht NOT NULL.
  SQLite bricht `ON DELETE SET NULL` ab, wenn die Child-Spalte NOT NULL ist
  („NOT NULL constraint failed“), womit die DoD (Playlist-Pack löschen → Zeile
  bleibt, FK NULL) unmöglich wäre. Empirisch verifiziert (sqlite3 + Integrationstest).

## Test
`cargo test --test migration_integrity --test migration_rediscovery_pushes`
→ 5 passed; 0 failed (integrity 1/1, rediscovery 4/4).

## Stages
- [x] developer (impl + tests) — Commit `831bc50`
- [ ] verifier
- [ ] tester
- [ ] developer (push + PR)
