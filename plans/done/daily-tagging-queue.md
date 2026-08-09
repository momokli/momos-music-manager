## Plan: daily-tagging-queue

**Status**: done ✅
**Branch**: `feat/daily-tagging-queue`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: yes — `018_canonical_playlist_id.sql`

### Description

"Daily Tagging Queue" — pick source tags, set a BPM range, generate a narrowed Spotify
playlist for on-the-go listening. Tagging happens BY adding tracks to tag-named playlists
in the Spotify mobile app. Two-way sync: tracks added on phone flow back via global poller.
The loop: curate → push → listen → tag (on phone) → sync back.

### What was built

#### Phase A: Push-to-Spotify

- **Migration 018**: `canonical_playlist_id` column + index on `service_playlists`
- **Write OAuth scopes**: Added `playlist-modify-public` + `playlist-modify-private` in 5 locations
  (`src/spotify/client.rs`, `src/api/services.rs` ×3, `src/api/websocket.rs`)
- **SpotifyClient methods**: `get_current_user_id()`, `create_playlist()`, `add_tracks_to_playlist()`
  in `src/spotify/client.rs`
- **Shared push function**: `push_playlist_to_spotify()` in `src/api/playlists.rs` — creates
  Spotify playlist, adds tracks in batches of 100, links via `canonical_playlist_id`
- **HTTP handler**: `POST /api/playlists/{id}/push-to-spotify` with `{ name?, public? }`

#### Phase B: Daily Generate Endpoint

- **New module**: `src/api/daily.rs`
- **`POST /api/daily/generate`**: Takes `{ tags, bpmMin, bpmMax, limit, excludeFullyTagged }`
  - Resolves tags → tracks via `track_resolved_tags`, filters by BPM, random sample + limit
  - Creates local playlist: `Daily-{tag}-{bpmMin}-{bpmMax}-{date}` (no spaces)
  - Best-effort push to Spotify via `push_playlist_to_spotify()`
  - Returns `{ playlistId, playlistName, trackCount, spotifyUrl }`

#### Phase C: Frontend Daily Page

- **New page**: `frontend/pages/daily.js` — tag typeahead, BPM presets, limit, exclude toggle, result card, localStorage history
- Registered in `frontend/app.js` PAGE_MAP + `frontend/shared/nav.js` TOOLS_ITEMS

### Acceptance Criteria

- [x] `cargo build` passes (zero new warnings)
- [x] `cargo test` passes (659 tests)
- [x] Migration 018 runs cleanly (001→018)
- [x] Write OAuth scopes in all 5 locations
- [x] `POST /api/daily/generate` creates playlist + pushes to Spotify
- [x] `POST /api/playlists/{id}/push-to-spotify` works independently
- [x] BPM filter, exclude-fully-tagged, random sample all work
- [x] `#daily` page renders with full form + history
- [ ] **User must re-authenticate Spotify** on Services page for write scopes

---

