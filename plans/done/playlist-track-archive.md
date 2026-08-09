## Plan: playlist-track-archive

**Status**: done ✅
**Branch**: `feat/playlist-track-archive`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: yes — `008_playlist_track_archive.sql`

### Description

Instead of hard-deleting tracks from `service_playlist_tracks` when they're removed from a Spotify playlist, soft-delete them with a `deleted_at` timestamp. Add a per-playlist `archive_deleted` toggle that controls whether deleted tracks are still treated as active for tag resolution (comment writing, digging, filtering). Followed/subscribed playlists default to `archive_deleted = true` (collect all ever-added entries), personal playlists default to `archive_deleted = false` (respect deletions).

### Why

- Followed playlists like "Beatport Top 100 - Tech House" rotate tracks frequently — users want to keep all historical entries for tagging
- Personal playlists should reflect real state — when you remove a track, it should stop being tagged
- Spotify Discover Weekly / Release Radar are "followed" type — keep all ever as active
- Users can toggle per-playlist if the default doesn't match their intent

### Schema Changes

#### Migration 008 (`migrations/008_playlist_track_archive.sql`)

1. Add `deleted_at INTEGER` to `service_playlist_tracks` (NULL = active, timestamp = deleted)
2. Add `archive_deleted BOOLEAN NOT NULL DEFAULT 0` to `service_playlists`
3. Set `archive_deleted = 1` for all playlists that have a subscription (followed playlists)
4. Drop + recreate all views that depend on `service_playlist_tracks`:
   - `v_file_tags` — add filter: `AND (sp.archive_deleted = 1 OR spt.deleted_at IS NULL)`
   - `v_file_resolved_tags` — same filter
   - `v_tag_file_counts` — already depends on `v_file_tags`, automatically updated

```sql
-- Step 1: Add deleted_at to service_playlist_tracks
ALTER TABLE service_playlist_tracks ADD COLUMN deleted_at INTEGER;

-- Step 2: Add archive_deleted to service_playlists
ALTER TABLE service_playlists ADD COLUMN archive_deleted BOOLEAN NOT NULL DEFAULT 0;

-- Step 3: Set archive_deleted = 1 for subscribed playlists
UPDATE service_playlists SET archive_deleted = 1
WHERE EXISTS (
    SELECT 1 FROM playlist_subscriptions ps
    WHERE ps.service = service_playlists.service
      AND ps.playlist_id = service_playlists.playlist_id
);

-- Step 4: Drop dependent views
DROP VIEW IF EXISTS v_tag_file_counts;
DROP VIEW IF EXISTS v_file_resolved_tags;
DROP VIEW IF EXISTS v_file_tags;

-- Step 5: Recreate v_file_tags with archive_deleted filter
CREATE VIEW v_file_tags AS
SELECT DISTINCT f.id AS file_id,
       t.id AS tag_id, t.name AS tag_name,
       t.sort_order, t.created_at,
       tc.id AS category_id, tc.name AS category_name,
       tc.is_default, tc.prefix
FROM files f
JOIN v_file_track_link v ON v.file_id = f.id
JOIN service_playlist_tracks spt ON spt.track_id = v.track_id
JOIN service_playlists sp ON sp.id = spt.playlist_id
JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
JOIN tag_categories tc ON tc.id = t.category_id
WHERE sp.archive_deleted = 1 OR spt.deleted_at IS NULL;

-- Step 6: Recreate v_file_resolved_tags with archive_deleted filter
CREATE VIEW v_file_resolved_tags AS
SELECT DISTINCT
    f.id AS file_id,
    rt.tag_id,
    rt.tag_name,
    rt.sort_order,
    rt.created_at,
    rt.category_id,
    rt.category_name,
    rt.prefix
FROM files f
JOIN v_file_track_link v ON v.file_id = f.id
JOIN service_playlist_tracks spt ON spt.track_id = v.track_id
JOIN service_playlists sp ON sp.id = spt.playlist_id
JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
JOIN v_resolved_tags rt ON rt.source_tag_id = t.id
WHERE sp.archive_deleted = 1 OR spt.deleted_at IS NULL;

-- Step 7: Recreate v_tag_file_counts
CREATE VIEW v_tag_file_counts AS
SELECT vft.tag_id, COUNT(DISTINCT vft.file_id) AS file_count
FROM v_file_tags vft
GROUP BY vft.tag_id;

SELECT 'Migration 008 applied: soft-delete playlist tracks + archive_deleted toggle' as status;
```

### Backend Changes

#### 1. `src/db.rs` — `ServicePlaylistTrack` struct

Add `deleted_at: Option<i64>` field.

#### 2. `src/db.rs` — `add_track_to_playlist_with_added_at()`

Change from `INSERT OR IGNORE` to `INSERT ... ON CONFLICT(playlist_id, track_id) DO UPDATE`:

```rust
sqlx::query(
    r#"
    INSERT INTO service_playlist_tracks (playlist_id, track_id, position, added_at, deleted_at)
    VALUES (?, ?, ?, ?, NULL)
    ON CONFLICT(playlist_id, track_id) DO UPDATE SET
        position = excluded.position,
        added_at = excluded.added_at,
        deleted_at = NULL
    "#,
)
```

This handles re-adds: a track that was previously soft-deleted gets `deleted_at = NULL` (re-activated).

#### 3. `src/db.rs` — New functions

```rust
/// Mark all tracks in a playlist as deleted (used before re-syncing from Spotify)
pub async fn mark_playlist_tracks_deleted(
    conn: &mut SqliteConnection,
    playlist_id: i64,
) -> Result<u64> {
    let now = chrono::Utc::now().timestamp();
    let rows = sqlx::query(
        "UPDATE service_playlist_tracks SET deleted_at = ? WHERE playlist_id = ? AND deleted_at IS NULL"
    )
    .bind(now)
    .bind(playlist_id)
    .execute(conn)
    .await?;
    Ok(rows.rows_affected())
}

/// Toggle archive_deleted for a playlist
pub async fn set_playlist_archive_deleted(
    pool: &Pool<Sqlite>,
    playlist_id: i64,
    archive: bool,
) -> Result<()> {
    sqlx::query("UPDATE service_playlists SET archive_deleted = ? WHERE id = ?")
        .bind(archive)
        .bind(playlist_id)
        .execute(pool)
        .await?;
    Ok(())
}
```

#### 4. `src/db.rs` — `ServicePlaylist` struct

Add `archive_deleted: bool` field.

#### 5. `src/db.rs` — `update_playlist_fetch_tracking()`

The unique count query currently counts all `service_playlist_tracks` rows. When `archive_deleted = true`, we want the count to reflect ALL tracks (including soft-deleted). When `archive_deleted = false`, only active tracks. For consistency, keep counting all rows (the unique count is about what's stored, not what's active). The views handle the filtering.

Actually, we should count active-only for `remote_unique_count` comparison purposes. Let `remote_unique_count` reflect only active (non-deleted) tracks to match the frontend display. Update the count query:

```sql
SELECT COUNT(*) FROM service_playlist_tracks spt
JOIN service_playlists sp ON sp.id = spt.playlist_id
WHERE sp.service = ? AND sp.playlist_id = ? AND spt.deleted_at IS NULL
```

#### 6. `src/spotify/sync_worker.rs` — `sync_tracks_for_playlist()`

Replace the `DELETE FROM service_playlist_tracks WHERE playlist_id = ?` with:

```rust
// Soft-delete: mark all existing tracks as deleted, then re-insert from stream.
// Re-added tracks will get deleted_at = NULL via ON CONFLICT DO UPDATE.
if let Ok(Some((pl_id,))) = sqlx::query_as::<_, (i64,)>(
    "SELECT id FROM service_playlists WHERE service = 'spotify' AND playlist_id = ?",
)
.bind(playlist_id)
.fetch_optional(&self.db)
.await
{
    let deleted_count = crate::db::mark_playlist_tracks_deleted(&mut *self.db.acquire().await?, pl_id).await.unwrap_or(0);
    if deleted_count > 0 {
        debug!("Soft-deleted {} track(s) from playlist '{}'", deleted_count, playlist_name);
    }
}
```

When `archive_deleted = false`, the views will exclude these soft-deleted tracks. When `archive_deleted = true`, they remain visible.

Optionally, if the playlist has `archive_deleted = false`, we could still do a hard delete for efficiency. But soft-delete is simpler and consistent.

#### 7. `src/api.rs` — `Playlist` response struct

Add `archive_deleted: bool` field.

#### 8. `src/api.rs` — `playlists_handler()`

Add `sp.archive_deleted` to the SELECT:

```sql
SELECT sp.*, COUNT(spt.track_id) as track_count, vtp.tag_name, sp.archive_deleted
FROM service_playlists sp ...
```

When `archive_deleted = false`, the `track_count` should only count active tracks. Currently it's `COUNT(spt.track_id)`. Update to:

```sql
COUNT(CASE WHEN spt.deleted_at IS NULL THEN 1 END) as track_count
```

For playlists with `archive_deleted = true`, we might want to show both active and total. That's a UI consideration.

#### 9. `src/api.rs` — New endpoint: toggle archive

**Route**: `.route("/api/playlists/{id}/archive", put(toggle_playlist_archive_handler))`

**Request**: `{ archiveDeleted: true }`

**Handler**:

```rust
async fn toggle_playlist_archive_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let archive = body.get("archiveDeleted").and_then(|v| v.as_bool()).unwrap_or(false);
    match crate::db::set_playlist_archive_deleted(&state.db, id, archive).await {
        Ok(()) => Json(json!({"data": {"id": id, "archiveDeleted": archive}})).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}
```

#### 10. `src/api.rs` — `PlaylistsQuery`

Add `archive: Option<String>` filter (`"archived"` / `"active"` / `"all"`).

### Frontend Changes (`frontend/pages/playlists.js`)

#### 1. Add `archiveDeleted` to the adapted playlist object

```javascript
archiveDeleted: p.archiveDeleted ?? p.archive_deleted ?? false,
```

#### 2. Add Archive toggle button in each row

Add to `PLAYLISTS_COLUMNS`:

```javascript
{ id: "archive", label: "Archive", sortable: false, defaultWidth: 60 },
```

Add renderer in `PLAYLISTS_CELL_RENDERERS`:

```javascript
archive(r) {
  const icon = r.archiveDeleted ? "fa-archive" : "fa-box-open";
  const title = r.archiveDeleted
    ? "Archiving: deleted tracks remain active for tagging"
    : "Active: deleted tracks are removed from tagging";
  return `<button class="btn btn-sm btn-icon archive-toggle-btn"
    data-id="${r.id}" data-archive="${r.archiveDeleted ? "1" : "0"}"
    title="${title}">
    <i class="fas ${icon}"></i>
  </button>`;
}
```

#### 3. Wire archive toggle click

In `wireContentEvents`, delegate click on `.archive-toggle-btn`:

- Toggle the boolean
- `PUT /api/playlists/{id}/archive` with `{ archiveDeleted: !current }`
- Update button icon + tooltip inline (no full re-render needed)
- Toast: "Archive mode {enabled/disabled} for '{playlistName}'"

#### 4. Add Archive filter to toolbar

In the RIGHT column (Classification section), add a filter row:

```html
<div class="filter-row">
  <span class="filter-row-label toggleable" data-filter="archive">Archive</span>
  <div class="filter-group">
    <button class="filter-btn" data-value="archived">
      <i class="fas fa-archive"></i> Archiving
    </button>
    <button class="filter-btn" data-value="active">
      <i class="fas fa-box-open"></i> Active
    </button>
    <button class="filter-btn active" data-value="all">All</button>
  </div>
</div>
```

Add `archive: "all"` to state and hash schema.

#### 5. Track count display

When `archiveDeleted = true`, show both active + total in the Tracks column:

```
142 / 287
```

(active / total including soft-deleted). When `archiveDeleted = false`, just show the active count.

### Files to modify

- `migrations/008_playlist_track_archive.sql` — new migration
- `src/db.rs` — `ServicePlaylistTrack` + `ServicePlaylist` structs, `add_track_to_playlist_with_added_at`, new `mark_playlist_tracks_deleted` + `set_playlist_archive_deleted` functions
- `src/spotify/sync_worker.rs` — replace DELETE with soft-delete in `sync_tracks_for_playlist()`
- `src/api.rs` — `Playlist` struct + `playlists_handler` query + `toggle_playlist_archive_handler` endpoint + `PlaylistsQuery` archive filter
- `frontend/pages/playlists.js` — archive column + toggle button + wire click + toolbar filter + track count display
- `frontend/style.css` — `.archive-toggle-btn` styles

### Acceptance Criteria

- [ ] Migration 008 runs cleanly on fresh DB (001→008)
- [ ] Migration 008 runs cleanly on existing DB with data
- [ ] Subscribed playlists default to `archive_deleted = true`
- [ ] Non-subscribed playlists default to `archive_deleted = false`
- [ ] Full sync marks removed tracks with `deleted_at` instead of deleting
- [ ] Re-added tracks get `deleted_at = NULL` (re-activated)
- [ ] When `archive_deleted = true`: `v_file_tags` + `v_file_resolved_tags` include all tracks regardless of `deleted_at`
- [ ] When `archive_deleted = false`: only active (non-deleted) tracks appear in tag resolution
- [ ] Toggle button in playlist row switches between archive/active modes
- [ ] PUT `/api/playlists/{id}/archive` toggles the flag
- [ ] Archive filter in toolbar works (archived/active/all)
- [ ] Track count column shows active/total for archiving playlists
- [ ] `compute_target_comment()` correctly resolves tags based on archive status (via updated views)
- [ ] Digging suggestions respect archive status (via updated views)
- [ ] No regressions: subscription poller + global poller still work (they only add, don't delete)
- [ ] Backend compiles (`cargo build`)
- [ ] Test with curl: toggle archive, verify `v_file_tags` returns correct counts

---

