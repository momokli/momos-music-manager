## Plan: push-to-spotify-ui

**Status**: done ✅
**Branch**: `feat/push-to-spotify-ui`
**Ready for review**: yes
**Depends on**: `feat/daily-tagging-queue`
**Migration needed**: no

### Description

Add "Push to Spotify" button and service badges to the Playlists page. Any
local playlist can be pushed to Spotify. Pushed playlists show an "Open in
Spotify" link. Uses the existing `POST /api/playlists/{id}/push-to-spotify`
endpoint and the new `services` field on the playlist list response.

### What was built

#### Backend: `services` field

- Added `services: Option<String>` to `Playlist` struct (with `#[sqlx(default)]`)
- SQL subquery: `COALESCE(GROUP_CONCAT(DISTINCT sp2.service), sp.service)` grouped by `canonical_playlist_id`
- Included in API response

#### Frontend: Playlists page

- **Service badges**: `services` field adapted into row data
- **Push button**: shown on local playlists not yet mirrored to Spotify
- **Open in Spotify**: green Spotify link when `services` includes `spotify`
- **Click handler**: calls `POST /api/playlists/{id}/push-to-spotify`, refreshes on success

#### Tests

- **Rust**: `playlists_list_includes_services_field` — creates local playlist, links Spotify row, asserts `services` contains both
- **Playwright**: `shows push-to-spotify button for local playlists` — seeds local playlist, asserts button visible

### Files modified

- `src/api/playlists.rs` — `Playlist.services` field, SQL subquery, JSON response
- `frontend/pages/playlists.js` — adapted `services`, push button in `actions()`, click handler
- `frontend/style.css` — `.btn-spotify` style
- `tests/api_playlists.rs` — `playlists_list_includes_services_field` test
- `frontend/tests/playlists.spec.js` — NEW FILE, 2 tests

### Acceptance Criteria

- [x] `GET /api/playlists` includes `services` field
- [x] Local-only playlist shows "Push to Spotify" button
- [x] Pushed playlist shows green Spotify button
- [x] Push button calls endpoint, refreshes on success
- [x] `cargo build` passes
- [x] `cargo test` passes (661 tests)
- [x] `cd frontend && npx playwright test` passes (15 tests)

---

