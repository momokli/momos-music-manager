## Plan: auto-backup

**Status**: done ✅
**Branch**: `feat/auto-backup`
**Ready for review**: no
**Depends on**: `feat/file-lifecycle-management` (already merged)
**Migration needed**: yes — `010_auto_backup.sql`

### Description

Auto-backup: when a folder has a `backup_path` configured, it automatically reconciles and rsyncs new files without manual intervention. Enabled by default, toggleable per folder.

### Current State

| What we have                          | What's missing                     |
| ------------------------------------- | ---------------------------------- |
| Reconcile + rsync via "Backup" button | No periodic auto-trigger           |
| Auto-reconcile on server startup      | Only runs once at boot             |
| Folder watcher (scans every 5 min)    | Watcher only scans, doesn't backup |

### Design

#### Migration 010: `migrations/010_auto_backup.sql`

```sql
ALTER TABLE folders ADD COLUMN auto_backup BOOLEAN NOT NULL DEFAULT 1;

SELECT 'Migration 010 applied: auto_backup column on folders' as status;
```

#### Folder struct update

Add `auto_backup: bool` to `Folder` in `src/db.rs`.

#### API: toggle auto_backup

Extend `PUT /api/folders/{id}/backup` (already exists for backup_path + scan_sources) to also accept `autoBackup: bool`.

Or add a new simpler endpoint:

```rust
.route("/api/folders/{id}/auto-backup", put(folder_auto_backup_handler))
```

```rust
async fn folder_auto_backup_handler(...) -> impl IntoResponse {
    // Toggle auto_backup
    sqlx::query("UPDATE folders SET auto_backup = ? WHERE id = ?")
        .bind(auto_backup).bind(id).execute(&state.db).await?;
    Json(ApiResponse { data: json!({ "autoBackup": auto_backup }) })
}
```

#### Auto-backup background task

In `src/main.rs` `serve()`, alongside the folder watcher, add an **auto-backup poller**:

```rust
// Auto-backup poller: periodically reconcile+backup folders with auto_backup enabled
let auto_db = state.db.clone();
let auto_tm = state.task_manager.clone();
tokio::spawn(async move {
    let interval = std::time::Duration::from_secs(600); // 10 minutes
    loop {
        tokio::time::sleep(interval).await;
        let folders: Vec<crate::db::Folder> = sqlx::query_as::<_, crate::db::Folder>(
            "SELECT * FROM folders WHERE auto_backup = 1 AND backup_path IS NOT NULL AND backup_path != ''"
        ).fetch_all(&auto_db).await.unwrap_or_default();

        for folder in folders {
            let unbacked = crate::db::get_unbacked_up_files(&auto_db, folder.id).await.unwrap_or_default();
            if !unbacked.is_empty() {
                tracing::info!("Auto-backup: folder '{}' has {} unbacked files", folder.folder_path, unbacked.len());
                crate::tasks::start_backup_folder_task(&auto_tm, &auto_db, folder.id).await;
            }
        }
    }
});
```

This runs every 10 minutes. For each folder with `auto_backup=true`:

- Checks if unbacked files exist
- If yes, triggers backup task (reconcile first, then rsync only new files)
- If no, skips (minimal overhead: one lightweight SQL query)

#### Frontend

**Folder edit modal** — add a checkbox:

```html
<label class="checkbox-label">
  <input type="checkbox" id="edit-folder-auto-backup" ${f.autoBackup ? "checked" : ""}>
  Auto-backup new files
</label>
<span class="help-text">Automatically reconcile and sync new files to the backup destination</span>
```

**Storage page folder cards** — show auto-backup status with a green dot when enabled.

### Files to modify

- `migrations/010_auto_backup.sql` — new migration
- `src/db.rs` — add `auto_backup` to `Folder` struct
- `src/api.rs` — add `folder_auto_backup_handler` + route
- `src/main.rs` — add auto-backup poller
- `frontend/pages/folders.js` — add checkbox to edit modal
- `frontend/pages/storage.js` — show auto-backup status on folder cards

### Acceptance Criteria

- [ ] Migration 010 applies cleanly (001→010)
- [ ] New folders default to `auto_backup = true`
- [ ] Folder edit modal has auto-backup checkbox
- [ ] Storage page shows auto-backup status per folder
- [ ] Auto-backup poller runs every 10 minutes
- [ ] Poller only triggers backup when unbacked files exist
- [ ] No manual intervention needed: files appear → auto-reconciled → auto-rsynced
- [ ] `cargo build` passes

---

