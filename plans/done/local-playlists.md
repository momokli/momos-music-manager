## Plan: local-playlists

**Status**: done ✅
**Branch**: `feat/local-playlists`
**Ready for review**: yes
**Depends on**: `feat/digging-multi-seed` (Phase 1)
**Migration needed**: yes — `005_local_service.sql`

### Description

Add "local" as a first-class service source. A local playlist can contain any `service_track` (Spotify, YouTube, or newly created `local` tracks). The playlist→tag chain works automatically via existing `v_tag_playlist`. This enables the Digging workflow: save suggestions as a persistent local playlist, which creates a Setlist tag, which can be written into file comments.

### Why no new tables

- `service_playlists(service='local')` — already works, no FK constraint on service values
- `service_tracks(service='local')` — already works, `service_id` can be any string
- `service_playlist_tracks` — already works, any track can be in any playlist
- Only needed change: `v_file_track_link` view to match `service='local'` on `service_id = CAST(f.id AS TEXT)`

### Migration: `migrations/005_local_service.sql`

Recreate `v_file_track_link` with the local service match:

```sql
DROP VIEW IF EXISTS v_file_track_link;
CREATE VIEW v_file_track_link AS
SELECT f.id AS file_id, st.id AS track_id
FROM files f
JOIN service_tracks st ON (
    st.isrc = f.isrc
    OR (st.service = 'spotify' AND st.service_id = f.spotify_id)
    OR (st.service = 'soundcloud' AND st.service_id = f.soundcloud_id)
    OR (st.service = 'youtube' AND st.service_id = f.youtube_id)
    OR (st.service = 'local' AND st.service_id = CAST(f.id AS TEXT))
);
```

Also update `v_file_tags` and `v_file_resolved_tags` (in 001/002/004) — they reference `v_file_track_link` indirectly via service_playlist_tracks, so just re-running `DROP VIEW IF EXISTS ... CREATE VIEW ...` for those dependent views is needed. Or simpler: the migration just drops and recreates all affected views.

### Backend: `src/api.rs` — New endpoint

**Route**: `.route("/api/playlists/local", post(create_local_playlist_handler))`

**Request**:

```json
{
  "name": "collapse-capital-v2",
  "fileIds": [4042, 4196, 5757, 65, 831]
}
```

**Handler logic**:

```rust
async fn create_local_playlist_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateLocalPlaylistRequest>,
) -> impl IntoResponse {
    // 1. Für jedes File: service_track existiert? (via ISRC oder local service_id)
    //    Ja → dessen ID merken
    //    Nein → INSERT service_track(service='local', service_id=CAST(file.id AS TEXT),
    //                                title, artist, isrc=file.isrc)
    // 2. INSERT service_playlists(service='local', name=request.name)
    // 3. INSERT service_playlist_tracks(playlist_id, track_id) für alle resolved tracks
    // 4. Return { playlistId, trackCount, newTrackCount }
}
```

### Frontend Integration (in Phase 2)

The digging page's "Save as ..." button calls this endpoint. User types a playlist name, clicks save.

### What happens automatically after save

1. `v_tag_playlist` matches playlist name → creates tag (via `create_tags_from_playlists` or on next poll)
2. `v_file_tags` shows all saved files under the new tag
3. User goes to Files page, filters by the tag, clicks "Write Comments"
4. Files now have `[PMV] tags collapse-capital-v2` in their comment

### Future: mirror to Spotify

Because local playlist contains Spotify-track IDs, we can later:

1. `POST /api/services/spotify/create-playlist` → creates Spotify playlist
2. `POST /api/services/spotify/add-tracks` → adds tracks by Spotify ID
3. Update `service_playlists` with Spotify ID → subscription poller picks it up

### Acceptance Criteria

- [x] `v_file_track_link` matches `service='local'` on `service_id = CAST(f.id AS TEXT)`
- [x] `POST /api/playlists/local` creates playlist + ensures service_tracks + adds track entries
- [x] Creating a local playlist automatically creates a Setlist tag via name match
- [x] Files appear under the tag in `v_file_tags`
- [x] `v_file_resolved_tags` works for local playlists (tag parents supported)
- [x] Duplicate service_tracks not created (ISRC match reuses existing Spotify track)
- [x] No regressions: `v_file_track_link` still matches Spotify/SoundCloud/YouTube correctly
- [x] Backend compiles (`cargo build`)
- [x] Fresh DB: all migrations run cleanly (001→002→003→004→005→006)
- [x] Test with curl: create playlist, verify tag auto-created, verify file-tag-link

---

