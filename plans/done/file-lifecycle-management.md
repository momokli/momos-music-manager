## Plan: file-lifecycle-management

**Status**: done ✅
**Branch**: `feat/file-lifecycle-management`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: yes — `009_file_lifecycle.sql`

### Description

File lifecycle management system. Three pillars:

1. **WAV source indexing** — Track nuo-stems WAV source files in the DB, back them up to NAS, delete locally
2. **Tag-based file presence** — "Follow" a tag → its files stay local. "Backpack" tag for quick-add from Tracks page
3. **Backup + prune** — Copy files to NAS via SSH/SCP, verify, then safely prune local copies that are backed up and not "followed"

### Data Model

#### Migration 009: `migrations/009_file_lifecycle.sql`

```sql
-- file_locations: tracks where a file physically exists
CREATE TABLE IF NOT EXISTS file_locations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    location_type TEXT NOT NULL CHECK (location_type IN ('local', 'backup')),
    path TEXT NOT NULL,
    file_size INTEGER,
    last_verified INTEGER,
    created_at INTEGER DEFAULT (unixepoch()),
    UNIQUE(file_id, location_type)
);

-- tags: add followed flag
ALTER TABLE tags ADD COLUMN followed BOOLEAN NOT NULL DEFAULT 0;
-- Default follow for "backpack" tag (created on first use if not present)

-- files: add source_of for WAV→stem parent linking
ALTER TABLE files ADD COLUMN source_of INTEGER REFERENCES files(id);

-- folders: add scan_sources + backup_path
ALTER TABLE folders ADD COLUMN scan_sources BOOLEAN NOT NULL DEFAULT 0;
ALTER TABLE folders ADD COLUMN backup_path TEXT;

CREATE INDEX IF NOT EXISTS idx_file_locations_file_id ON file_locations(file_id);
CREATE INDEX IF NOT EXISTS idx_file_locations_type ON file_locations(location_type);
CREATE INDEX IF NOT EXISTS idx_files_source_of ON files(source_of);
CREATE INDEX IF NOT EXISTS idx_tags_followed ON tags(followed);

SELECT 'Migration 009 applied: file lifecycle management' as status;
```

### Rust Types

New types in `src/db.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct FileLocation {
    pub id: i64,
    pub file_id: i64,
    pub location_type: String,  // 'local' | 'backup'
    pub path: String,
    pub file_size: Option<i64>,
    pub last_verified: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PruneCandidate {
    pub file_id: i64,
    pub file_path: String,
    pub file_type: String,
    pub file_size: i64,
    pub title: String,
    pub artist: String,
    pub isrc: Option<String>,
    pub reason: String,  // "flac_with_stem" | "wav_backed_up" | "not_followed"
    pub backup_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageStatus {
    pub local_file_count: i64,
    pub local_size_bytes: i64,
    pub local_stems: i64,
    pub local_flacs: i64,
    pub backup_count: i64,
    pub wav_source_dirs: i64,
    pub prune_candidate_count: i64,
    pub prune_candidate_bytes: i64,
}
```

### DB Functions (in `src/db.rs`)

```rust
// ─── Tag following ─────────────────────────────────────────────
pub async fn set_tag_followed(pool: &Pool<Sqlite>, tag_id: i64, followed: bool) -> Result<()>
pub async fn get_followed_tags(pool: &Pool<Sqlite>) -> Result<Vec<Tag>>
pub async fn get_backpack_tag(pool: &Pool<Sqlite>) -> Result<Option<Tag>>
pub async fn ensure_backpack_tag(pool: &Pool<Sqlite>) -> Result<Tag>
pub async fn is_file_followed(pool: &Pool<Sqlite>, file_id: i64) -> Result<bool>

// ─── File locations ────────────────────────────────────────────
pub async fn set_file_location(pool: &Pool<Sqlite>, file_id: i64, location_type: &str, path: &str, file_size: i64) -> Result<()>
pub async fn remove_file_location(pool: &Pool<Sqlite>, file_id: i64, location_type: &str) -> Result<()>
pub async fn get_file_locations(pool: &Pool<Sqlite>, file_id: i64) -> Result<Vec<FileLocation>>
pub async fn get_unbacked_up_files(pool: &Pool<Sqlite>, folder_id: i64) -> Result<Vec<File>>
pub async fn record_backup_result(pool: &Pool<Sqlite>, file_id: i64, success: bool, file_size: i64, backup_path: &str) -> Result<()>
pub async fn clear_backup_status(pool: &Pool<Sqlite>, folder_id: i64) -> Result<()>

// ─── Source-of (WAV→stem linking) ─────────────────────────────
pub async fn get_wav_source_subdirs(pool: &Pool<Sqlite>, folder_id: i64) -> Result<Vec<String>>
pub async fn set_file_source_of(pool: &Pool<Sqlite>, file_id: i64, source_file_id: i64) -> Result<()>
pub async fn get_files_by_source(pool: &Pool<Sqlite>, source_file_id: i64) -> Result<Vec<File>>

// ─── Pruning ──────────────────────────────────────────────────
pub async fn get_prune_candidates(pool: &Pool<Sqlite>) -> Result<Vec<PruneCandidate>>
pub async fn delete_local_file_by_id(pool: &Pool<Sqlite>, file_id: i64) -> Result<bool>

// ─── Storage status ───────────────────────────────────────────
pub async fn get_storage_status(pool: &Pool<Sqlite>) -> Result<StorageStatus>

// ─── Folder backup config ─────────────────────────────────────
pub async fn update_folder_backup_config(pool: &Pool<Sqlite>, folder_id: i64, backup_path: Option<&str>, scan_sources: Option<bool>) -> Result<()>
```

### Backup Engine (`src/backup/mod.rs`)

New module with SSH-based backup:

```rust
pub struct BackupEngine {
    ssh_host: String,
    ssh_key_path: Option<String>,
}

impl BackupEngine {
    pub fn new(ssh_host: String) -> Self;

    /// Copy a local file to the backup destination. Returns (success, remote_size).
    pub async fn copy_file(
        &self,
        local_path: &Path,
        remote_path: &str,
    ) -> Result<(bool, i64)>;

    /// Verify a file exists on backup with matching size.
    pub async fn verify_file(
        &self,
        remote_path: &str,
        expected_size: i64,
    ) -> Result<bool>;

    /// Get size of a remote file (None if doesn't exist).
    pub async fn remote_file_size(&self, remote_path: &str) -> Result<Option<i64>>;

    /// List files in a remote directory.
    pub async fn list_remote_files(&self, remote_dir: &str) -> Result<Vec<String>>;

    /// Run rsync in dry-run mode to show what would be transferred.
    pub async fn dry_run_sync(&self, local_dir: &str, remote_dir: &str) -> Result<Vec<String>>;
}
```

The engine shells out to `scp` and `ssh` commands using `tokio::process::Command`, reading `~/.ssh/config` for host resolution. The `backup` host is passed as `ssh_host` (your `~/.ssh/config` maps `backup` → your NAS).

### API Endpoints (in `src/api.rs`)

```
GET    /api/storage/status               → StorageStatus
POST   /api/storage/backup/{folder_id}   → BackupResult { copied: usize, verified: usize, errors: usize }
POST   /api/storage/prune-preview        → [PruneCandidate]
POST   /api/storage/prune                → PruneResult { deleted: usize, freedBytes: i64 }
PUT    /api/tags/{id}/follow             → { followed: bool }
POST   /api/tracks/{id}/backpack          → { inBackpack: bool }
GET    /api/files/{id}/backup-status      → { backedUp: bool, locations: [FileLocation] }
PUT    /api/folders/{id}/backup           → { backupPath: string, scanSources: bool }
POST   /api/folders/{id}/scan-sources     → { wavIndexed: usize, linkedToStems: usize }
POST   /api/storage/backup-wavs/{folder_id} → { wavDirsBackedUp: usize, localWavsDeleted: usize }
```

### Frontend

#### Tags page (`frontend/pages/tags.js`)

- New column: "Follow" with toggle button per row
- Calls `PUT /api/tags/{id}/follow` to toggle
- Shows "Backpack" icon for tracks with backpack tag
- Followed tags show a filled 👁 or pinned icon
- Followed state included in API tag response

#### Tracks page (`frontend/pages/tracks.js`)

- New column: "Backpack" with toggle button per row (like subscribe bell on playlists)
- Click toggles the "backpack" Setlist tag on the track via `POST /api/tracks/{id}/backpack`
- Shows filled/empty backpack icon based on whether the track has the "backpack" tag

#### Storage page (`frontend/pages/storage.js`) — NEW

- Storage status cards (local vs backup)
- Backup buttons per folder (stems, flacs)
- WAV backup + cleanup button
- Prune preview table with checkboxes + execute button
- Dry-run before any destructive action

#### Folders page (`frontend/pages/folders.js`)

- Edit folder modal: add "Backup Path" text input + "Scan Sources" checkbox
- Show backup status per folder in table

### Files to modify

- `migrations/009_file_lifecycle.sql` — new migration
- `src/db.rs` — FileLocation, PruneCandidate, StorageStatus types + 15 new functions
- `src/backup/mod.rs` — BackupEngine (new module)
- `src/api.rs` — 10 new handler functions + routes
- `src/main.rs` — add `mod backup;`
- `frontend/pages/tags.js` — Follow toggle column
- `frontend/pages/tracks.js` — Backpack toggle column
- `frontend/pages/storage.js` — new page module
- `frontend/pages/folders.js` — Backup Path + Scan Sources fields
- `frontend/app.js` — register storage page
- `frontend/shared/nav.js` — add Storage nav link
- `frontend/style.css` — storage page styles

### Acceptance Criteria

- [ ] Migration 009 runs cleanly on fresh DB (001→009)
- [ ] Migration 009 runs cleanly on existing DB with data
- [ ] Tags page shows Follow toggle; clicking it persists and filters correctly
- [ ] Tracks page shows Backpack toggle; clicking adds/removes "backpack" tag
- [ ] Following a tag prevents its files from being pruned
- [ ] Storage page shows local vs backup stats
- [ ] Backup folder operation copies unbacked-up files via SCP, verifies
- [ ] WAV backup operation indexes subdirectories, backs up WAVs, records locations
- [ ] Prune preview shows candidates with reasons (flac_with_stem, wav_backed_up, not_followed)
- [ ] Prune execute deletes local files only if they have confirmed backup
- [ ] Folders page lets you set backup_path and scan_sources
- [ ] `cargo build` passes
- [ ] Frontend loads without errors

---

