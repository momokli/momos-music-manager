# Progress: Issue #60 — liked_sync: merge_liked_items + retire_missing_likes

- Repo: momokli/momos-music-manager
- Branch: feature/issue-60-liked-sync (base origin/main @ 6108dd5)
- PR-Ziel: main | Fokus-Milestone: 1.14.0
- Bot-Identity: momo-clanker[bot] via clanker-gh / clanker-git

## Scope

Neues Modul `src/liked_sync.rs` mit getrennter Fetch- und DB-Schicht:

- `merge_liked_items` — upsertet die Likes-Spiegel-Playlist
  (`service='spotify'`, `playlist_id='spotify:liked'`, `name='liked'`,
  `playlist_kind='liked'`) und schreibt **jeden** LikedItem über
  `add_track_to_playlist_with_added_at` (auch bestehende Memberships →
  Relike-Semantik, bewusster Unterschied zum Playlist-Poller).
- `retire_missing_likes` — Tombstone (soft-delete) aller aktiven Memberships
  der liked-Playlist, deren Track nicht in `current_track_ids` liegt; Tracks in
  anderen Playlists bleiben unberührt.
- `sync_liked_songs` — `get_saved_tracks_total` → `get_saved_tracks` streamen →
  Merges → `retire_missing_likes` nur wenn `total <` gespeicherter liked-Count
  → best-effort `refresh_track_tags` / `refresh_file_resolved_tags`. 429 →
  `spotify_cooldown()` via `extract_retry_after_secs`, Zyklus abbricht.
- `pub mod liked_sync;` in `src/lib.rs`.
- `tests/liked_sync.rs` — 7 Integrationstests, alle ohne Netzwerk
  (`common::create_test_db()` + direkt konstruierte `LikedItem`-Fixtures).

Nicht-Scope: Poller-Integration + API-Endpoint (Folge-Issue #61).

## Stage-Log

- [x] 1 planner (Task-Envelope #60, Plan eingelesen)
- [x] 2 setup (Branch vorhanden, Build-Baseline grün)
- [x] 3 developer — implementiert
- [x] 4 verifier — Build + 7/7 Tests grün
- [x] 5 tester — `cargo test --test liked_sync` grün
- [x] 6 developer (PR) — Commit + Push (kein PR, Orchestrator)

## Ergebnis

- `cargo build` EXIT 0.
- `cargo test --test liked_sync`: **7 passed; 0 failed**.
- `cargo clippy --all-targets` informativ (CI continue-on-error), keine neuen
  Fehler im neuen Modul.

## Befunde / Notizen

- Die Progress-Datei existierte initial nicht — hier neu angelegt.
- `TMPDIR=/dev/shm/mm60` gesetzt (bekanntes „pool timed out"-Workaround planet).
- Abweichung zum Poller (immer updaten statt skippen) ist Absicht und im Modul
  kommentiert — nicht „vereinheitlichen".
