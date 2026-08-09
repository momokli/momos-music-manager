## Plan: multi-provider-playlists

**Status**: proposed
**Branch**: `feat/multi-provider-playlists`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: yes — `017_canonical_playlist_id.sql`

### Description

Make playlists multi-provider: a logical playlist can exist on multiple services
simultaneously (local, Spotify, future SoundCloud). Add `canonical_playlist_id`
to tie provider rows into a single logical entity. Add `POST /api/playlists/{id}/push-to-spotify`
to mirror a local playlist to Spotify. Add write OAuth scopes to the Spotify client.

### Why `canonical_playlist_id` instead of `spotify_playlist_id`

A playlist isn't "owned" by one service. It's a named collection of tracks that can
be present on multiple providers. Tying two rows together via a shared UUID models
this as a peer relationship rather than a one-way pointer:

```
canonical_playlist_id: "a1b2c3d4-..."
├── service='local'    playlist_id='local-a1b2c3d4'
└── service='spotify'  playlist_id='37i9dQZEVXcJ...'
```

This unlocks: (a) a playlist can be local AND on Spotify, (b) pushing to new
providers adds rows without schema changes, (c) future two-way sync becomes
straightforward — compare tracks across canonical groups.

### Migration 017

```sql
ALTER TABLE service_playlists ADD COLUMN canonical_playlist_id TEXT;
CREATE INDEX IF NOT EXISTS idx_sp_canonical ON service_playlists(canonical_playlist_id);
```

### Backend Changes

#### 1. OAuth scopes — add write permissions

**Files**: `src/spotify/client.rs` (~line 77), `src/api/services.rs` (3 locations),
`src/api/websocket.rs` (1 location)

Add `playlist-modify-public` and `playlist-modify-private` to all `scopes!()` invocations.
Existing tokens without these scopes will get a 403 from Spotify on write operations
(rspotify's token refresh won't silently add scopes). The handler catches this and
returns: "Spotify token needs write permissions. Re-authenticate on the Services page."

#### 2. Spotify client — new write methods (`src/spotify/client.rs`)

```rust
/// Create a new Spotify playlist. Returns (playlist_id, spotify_url).
pub async fn create_playlist(
    &self, user_id: &str, name: &str, public: bool, description: Option<&str>,
) -> Result<(String, String)>

/// Add tracks in batches of 100 (Spotify API limit).
pub async fn add_tracks_to_playlist(
    &self, playlist_id: &str, track_ids: &[String],
) -> Result<()>
```

Both use rspotify's `OAuthClient` trait (`user_playlist_create`, `playlist_add_items`).

#### 3. API endpoint (`src/api/playlists.rs`)

**Route**: `.route("/api/playlists/{id}/push-to-spotify", post(push_to_spotify_handler))`

**Request**:

```json
{ "name": "optional-override", "public": false }
```

**Response**:

```json
{
  "data": {
    "spotifyPlaylistId": "37i9dQZ...",
    "spotifyUrl": "https://open.spotify.com/playlist/37i9dQZ...",
    "tracksPushed": 12,
    "tracksSkipped": 3,
    "skippedReasons": { "no-spotify-link": 3 }
  }
}
```

**Handler logic**:

1. Fetch the playlist + verify it exists
2. If `canonical_playlist_id` is already set and a Spotify row exists → 409 (already pushed)
3. Resolve Spotify track IDs: `SELECT st.service_id FROM service_playlist_tracks spt JOIN service_tracks st ON st.id = spt.track_id AND st.service = 'spotify' WHERE spt.playlist_id = ?`
4. Skip tracks without Spotify links, count them
5. Create `SpotifyClient::from_stored_tokens()`
   - If 403 with "insufficient scopes" → return clear error: "Re-authenticate on Services page"
6. `GET /v1/me` → get current user ID
7. `POST /v1/users/{user_id}/playlists` → create Spotify playlist
8. `POST /v1/playlists/{id}/tracks` in batches of 100
9. Generate a canonical ID: use the local row's existing `canonical_playlist_id` if set, otherwise generate a new UUID
10. If the local row had `canonical_playlist_id = NULL`, UPDATE it to the new UUID
11. INSERT new `service_playlists` row with `service='spotify', playlist_id=<spotify_id>, canonical_playlist_id=<uuid>`
12. Return result

#### 4. DB helpers (`src/db/playlists.rs`)

```rust
/// Get Spotify track IDs for a playlist. Returns Vec<(service_track_id, spotify_id)>.
pub async fn get_playlist_spotify_track_ids(
    pool: &Pool<Sqlite>, playlist_id: i64,
) -> Result<Vec<(i64, String)>>
```

#### 5. `Playlist` struct — add `canonical_playlist_id` + `services`

Add `canonical_playlist_id: Option<String>` and `services: Option<String>` to the API response.
Both fields use `#[sqlx(default)]` to avoid runtime errors when other queries use `query_as::<Playlist>`
without these columns. The playlist list handler continues to return one row per
`service_playlists` row (no dedup in v1 — that's a separate frontend concern).
The `services` field is computed with a subquery:

```sql
SELECT sp.*, ...
  COALESCE(
    (SELECT GROUP_CONCAT(DISTINCT sp2.service) FROM service_playlists sp2
     WHERE sp2.canonical_playlist_id = sp.canonical_playlist_id),
    sp.service
  ) as services
FROM service_playlists sp
```

`COALESCE` ensures: canonical group → `"spotify,local"`, no canonical → row's own service.

### Frontend Changes

#### 1. Playlists page (`frontend/pages/playlists.js`)

- **Service badges**: add a `services` column to `PLAYLISTS_COLUMNS` — renders colored badges from the `services` field: `[local] [spotify]`
- **Push button**: extend the existing `sync` cell renderer. Shown when `service='local'` and Spotify is not in `services`
  - Click → small dialog: optional name override + public/private toggle + track count preview
  - On success → toast with clickable Spotify URL, row refreshes with new badges
  - On 403 → toast: "Spotify token needs write permissions. Re-authenticate on the Services page."
- **Open in Spotify button**: also in `sync` cell, shown when `services` includes `spotify` —
  links to `https://open.spotify.com/playlist/{id}`

#### 2. Digging page (`frontend/pages/digging.js`)

- After "Save as Playlist", add a checkbox: "Also push to Spotify"
- When checked, chains the create-local → push-to-spotify calls

#### 3. CSS (`frontend/style.css`)

- `.service-badges` — inline flex row of small colored service badges
- `.push-spotify-dialog` — tiny modal for name/public toggle

### What happens after push

1. New `service_playlists` row with `service='spotify'` exists in DB
2. **Global poller** picks up the new Spotify playlist on its next cycle, syncs tracks
3. **Tag matching** works automatically — same playlist name → same tag
4. **Subscription poller** can subscribe to the Spotify row if user wants live sync
5. Files linked via ISRC stay linked — the new Spotify tracks match existing files

### Why not dedup playlist list yet

Deduping `service_playlists` into one entry per canonical group requires frontend
changes to show per-service actions (push/delete/open) within a unified card.
That's a separate UI plan. V1 keeps the list flat — each service row is a separate
list entry, grouped visually by the shared `canonical_playlist_id`. The `services`
field tells the frontend which badges to show.

### Files to modify

| File                                       | Change                                                                             |
| ------------------------------------------ | ---------------------------------------------------------------------------------- |
| `migrations/017_canonical_playlist_id.sql` | New migration                                                                      |
| `src/spotify/client.rs`                    | Write scopes + `create_playlist()` + `add_tracks_to_playlist()`                    |
| `src/api/services.rs`                      | Write scopes in 3 OAuth locations                                                  |
| `src/api/websocket.rs`                     | Write scopes in OAuth                                                              |
| `src/api/playlists.rs`                     | `push_to_spotify_handler` + route + `Playlist` struct fields + `services` subquery |
| `src/db/playlists.rs`                      | `get_playlist_spotify_track_ids()` helper                                          |
| `frontend/pages/playlists.js`              | Service badges, Push button, Open in Spotify button                                |
| `frontend/pages/digging.js`                | "Also push to Spotify" checkbox                                                    |
| `frontend/style.css`                       | `.service-badges`, `.push-spotify-dialog`                                          |

### Acceptance Criteria

- [ ] Migration 017 runs cleanly on fresh DB (001→017)
- [ ] Migration 017 runs cleanly on existing DB with data
- [ ] `canonical_playlist_id` column added + indexed
- [ ] Write scopes present in all 5 OAuth scope locations
- [ ] `create_playlist()` creates a Spotify playlist and returns ID + URL
- [ ] `add_tracks_to_playlist()` adds tracks in batches of 100
- [ ] `POST /api/playlists/{id}/push-to-spotify` creates Spotify playlist for a local playlist
- [ ] All tracks with Spotify links are added; tracks without links counted as skipped
- [ ] New `service_playlists` row inserted with `service='spotify'` + shared `canonical_playlist_id`
- [ ] Local row gets `canonical_playlist_id` assigned if it was NULL (first push)
- [ ] 400 when playlist has zero Spotify-linked tracks
- [ ] 403 when token lacks write scopes — error message mentions re-auth
- [ ] 409 when a Spotify row already exists for this canonical group
- [ ] `GET /api/playlists` includes `canonicalPlaylistId` + `services` in response
- [ ] `services` subquery returns correct comma-separated list for canonical groups
- [ ] Frontend: Push button on local playlists, dialog with name + public/private
- [ ] Frontend: Success toast with clickable Spotify URL
- [ ] Frontend: Service badges rendered from `services` field
- [ ] Frontend: Digging "Also push to Spotify" checkbox works
- [ ] `cargo build` passes
- [ ] `cargo test` passes (all existing tests + new ones for push handler)

### Out of scope (v2)

- **Deduped playlist list**: One card per canonical group with per-service action buttons
- **Two-way sync**: Comparing tracks across canonical group members, adding missing ones
- **Remove from Spotify**: Deleting a Spotify playlist when the local playlist is deleted
- **Spotify → local pull**: Creating a local mirror from an existing Spotify playlist
- **Pushing to SoundCloud / YouTube**: Same pattern, different API clients
- **Playlist image**: Setting custom cover art on Spotify
- **Track ordering**: Preserving local playlist order on Spotify

---

