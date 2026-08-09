## Plan: dynamic-tag-bundles

**Status**: done ✅
**Branch**: `feat/dynamic-tag-bundles`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: yes — `019_dynamic_bundles.sql`

### Description

Dynamic tag bundles extend the static tag bundle concept. Instead of manually
curating member tags, a dynamic bundle defines **filter criteria**. The system
evaluates them to compute which files belong. The bundle creates a real
Setlist-category tag, so it appears in the backpack just like any other tag.
Backpack sync automatically pulls only files matching the dynamic lens.

**Use case**: "Hard Techno 140-160" — base tags `hammahalle`, `spät`, `bouncy`
(OR logic) + BPM range 140–160 → backpack pulls only those files.

### Architecture Integration

`refresh_file_resolved_tags()` in `src/db/playlists.rs` already has two steps in
a single transaction:

1. **Step 1**: Populate from `v_file_resolved_tags` view (playlist→tag chain)
2. **Step 2**: Resolve static `tag_bundles` (transitive closure, fixed-point)

We add **Step 3**: For each dynamic bundle, compute matching file IDs via
filter criteria and INSERT INTO `file_resolved_tags`. This means dynamic
bundles automatically feed into:

- `is_in_backpack(file_id)` — queries `file_resolved_tags JOIN tags WHERE backpack=1`
- `get_backpack_pull_candidates()` — queries `file_resolved_tags` for backpack file IDs
- Files page tag filter — queries `file_resolved_tags`
- `get_storage_status()` — counts from `file_resolved_tags`

Zero additional integration work needed — the existing materialized table is
the single point of truth.

### Filters (v1)

| Filter          | Type         | Behavior                                              |
| --------------- | ------------ | ----------------------------------------------------- |
| Base tags       | Multi-select | OR logic — file must have at least one                |
| All tracks      | Toggle       | Overrides base tags — every file is base              |
| BPM range       | Min / Max    | `files.bpm BETWEEN ? AND ?` (AND with base)           |
| PMV categories  | Multi-select | P, M, V — file must have tag in that prefix           |
| File types      | Multi-select | stem.m4a, flac, mp3, wav — `IN` filter                |
| Exclude WAV src | Toggle       | Exclude `file_type = 'wav' AND source_of IS NOT NULL` |

All filters are AND'd together: base ∩ BPM ∩ PMV ∩ file_type.

### Schema

#### Migration 019 (`migrations/019_dynamic_bundles.sql`)

```sql
CREATE TABLE IF NOT EXISTS dynamic_bundles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    tag_id INTEGER NOT NULL UNIQUE REFERENCES tags(id) ON DELETE CASCADE,
    base_tags TEXT,               -- JSON array of tag names, e.g. '["hammahalle","spät"]'
    include_all_tracks BOOLEAN NOT NULL DEFAULT 0,
    bpm_min REAL,
    bpm_max REAL,
    pmv_categories TEXT,          -- JSON array, e.g. '["p","m"]'
    file_types TEXT,              -- JSON array, e.g. '["stem.m4a","flac"]'
    exclude_wav_sources BOOLEAN NOT NULL DEFAULT 1,
    created_at INTEGER DEFAULT (unixepoch()),
    updated_at INTEGER DEFAULT (unixepoch())
);

CREATE INDEX IF NOT EXISTS idx_dynamic_bundles_tag_id ON dynamic_bundles(tag_id);

SELECT 'Migration 019 applied: dynamic_bundles table' as status;
```

**Why JSON columns instead of normalized join tables**: Dynamic bundles are
low-cardinality (maybe 5-20 rows). JSON avoids join complexity. The values are
only written/read in Rust, never queried in SQL WHERE clauses (the resolution
step constructs dynamic SQL from the parsed Rust structs).

### Rust Types

#### `src/db/types.rs` — new structs

```rust
#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
#[serde(rename_all = "camelCase")]
pub struct DynamicBundle {
    pub id: i64,
    pub name: String,
    pub tag_id: i64,
    pub base_tags: Option<String>,     // JSON
    pub include_all_tracks: bool,
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    pub pmv_categories: Option<String>, // JSON
    pub file_types: Option<String>,     // JSON
    pub exclude_wav_sources: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// API response wrapper for a dynamic bundle with computed stats.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicBundleResponse {
    #[serde(flatten)]
    pub bundle: DynamicBundle,
    pub tag_name: String,
    pub tag_backpack: bool,
    pub matching_file_count: i64,
}
```

### Backend: DB Functions

#### New file: `src/db/dynamic_bundles.rs`

All functions go in this new module to keep the pattern clean (like existing
`src/db/tags.rs`, `src/db/playlists.rs`, `src/db/files.rs`, etc.).

Module declaration in `src/db/mod.rs`:

```rust
pub mod dynamic_bundles;
pub use dynamic_bundles::*;
```

##### `resolve_dynamic_bundle(pool, db) -> Vec<i64>`

Core resolution function. Takes a `DynamicBundle` row and returns matching
file IDs. Constructs dynamic SQL from the filter criteria:

```rust
pub async fn resolve_dynamic_bundle(
    pool: &Pool<Sqlite>,
    db: &DynamicBundle,
) -> Result<Vec<i64>> {
    let mut sql = String::from(
        "SELECT DISTINCT f.id FROM files f WHERE 1=1"
    );
    let mut bind_values: Vec<Box<dyn ToSql<Sqlite>>> = Vec::new();

    // Base filter: either All Tracks or specific tags
    if db.include_all_tracks {
        // No additional WHERE — all files are the base
    } else if let Some(ref base_tags_json) = db.base_tags {
        let base_tags: Vec<String> = serde_json::from_str(base_tags_json)?;
        if !base_tags.is_empty() {
            let placeholders: Vec<&str> = base_tags.iter().map(|_| "?").collect();
            sql.push_str(&format!(
                " AND f.id IN (SELECT DISTINCT frt.file_id FROM file_resolved_tags frt WHERE frt.tag_name IN ({}))",
                placeholders.join(",")
            ));
            for t in &base_tags {
                bind_values.push(Box::new(t.clone()));
            }
        }
    }

    // BPM range
    if let Some(bpm_min) = db.bpm_min {
        sql.push_str(" AND f.bpm >= ?");
        bind_values.push(Box::new(bpm_min));
    }
    if let Some(bpm_max) = db.bpm_max {
        sql.push_str(" AND f.bpm <= ?");
        bind_values.push(Box::new(bpm_max));
    }

    // PMV categories
    if let Some(ref pmv_json) = db.pmv_categories {
        let pmv: Vec<String> = serde_json::from_str(pmv_json)?;
        if !pmv.is_empty() {
            let placeholders: Vec<&str> = pmv.iter().map(|_| "?").collect();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.prefix) IN ({}))",
                placeholders.join(",")
            ));
            for p in &pmv {
                bind_values.push(Box::new(p.to_lowercase()));
            }
        }
    }

    // File types
    if let Some(ref ft_json) = db.file_types {
        let ft: Vec<String> = serde_json::from_str(ft_json)?;
        if !ft.is_empty() {
            let placeholders: Vec<&str> = ft.iter().map(|_| "?").collect();
            sql.push_str(&format!(" AND f.file_type IN ({})", placeholders.join(",")));
            for t in &ft {
                bind_values.push(Box::new(t.clone()));
            }
        }
    }

    // Exclude WAV source files
    if db.exclude_wav_sources {
        sql.push_str(" AND NOT (f.file_type = 'wav' AND f.source_of IS NOT NULL)");
    }

    // Build and execute
    let mut query = sqlx::query_scalar::<_, i64>(&sql);
    for value in bind_values {
        query = query.bind(value.as_ref());
    }
    let file_ids = query.fetch_all(pool).await?;
    Ok(file_ids)
}
```

##### CRUD functions

```rust
/// List all dynamic bundles, enriched with matching file count.
pub async fn get_dynamic_bundles(
    pool: &Pool<Sqlite>,
) -> Result<Vec<DynamicBundleResponse>>;

/// Get a single dynamic bundle by ID.
pub async fn get_dynamic_bundle(
    pool: &Pool<Sqlite>,
    id: i64,
) -> Result<Option<DynamicBundle>>;

/// Create a dynamic bundle. Also creates the tag (Setlist category).
pub async fn create_dynamic_bundle(
    pool: &Pool<Sqlite>,
    name: &str,
    base_tags: Option<Vec<String>>,
    include_all_tracks: bool,
    bpm_min: Option<f64>,
    bpm_max: Option<f64>,
    pmv_categories: Option<Vec<String>>,
    file_types: Option<Vec<String>>,
    exclude_wav_sources: bool,
) -> Result<DynamicBundle>;

/// Update a dynamic bundle's filter criteria.
pub async fn update_dynamic_bundle(
    pool: &Pool<Sqlite>,
    id: i64,
    name: Option<&str>,
    base_tags: Option<Option<Vec<String>>>,
    include_all_tracks: Option<bool>,
    bpm_min: Option<Option<f64>>,
    bpm_max: Option<Option<f64>>,
    pmv_categories: Option<Option<Vec<String>>>,
    file_types: Option<Option<Vec<String>>>,
    exclude_wav_sources: Option<bool>,
) -> Result<DynamicBundle>;

/// Delete a dynamic bundle and its associated tag (CASCADE).
pub async fn delete_dynamic_bundle(
    pool: &Pool<Sqlite>,
    id: i64,
) -> Result<()>;

/// Get the matching file count for a dynamic bundle (without full resolution).
pub async fn get_dynamic_bundle_file_count(
    pool: &Pool<Sqlite>,
    db: &DynamicBundle,
) -> Result<i64>;
```

### Backend: Resolution Hook in `refresh_file_resolved_tags()`

#### `src/db/playlists.rs` — add Step 3 after bundle resolution

After Step 2 (static bundle resolution), add:

```rust
// Step 3: Resolve dynamic bundles
// For each dynamic bundle, compute matching file IDs and insert
// the bundle's tag into file_resolved_tags for those files.
let dynamic_changed: i64 = {
    let mut total: i64 = 0;
    let bundles: Vec<DynamicBundle> = sqlx::query_as::<_, DynamicBundle>(
        "SELECT * FROM dynamic_bundles"
    )
    .fetch_all(&mut *tx)
    .await?;

    for db in &bundles {
        // Get the tag info for this bundle's tag
        let tag: Tag = sqlx::query_as::<_, Tag>(
            "SELECT t.* FROM tags t WHERE t.id = ?"
        )
        .bind(db.tag_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Dynamic bundle tag {} not found", db.tag_id))?;

        let cat: TagCategory = sqlx::query_as::<_, TagCategory>(
            "SELECT * FROM tag_categories WHERE id = ?"
        )
        .bind(tag.category_id)
        .fetch_one(&mut *tx)
        .await?;

        // Resolve matching file IDs (uses the pool, not the transaction —
        // but since we're in a transaction, use a sub-query or compute in Rust)
        // For transactional safety, we do the resolution query against the same connection
        let file_ids = resolve_dynamic_bundle_in_tx(&mut *tx, db).await?;

        for chunk in file_ids.chunks(500) {
            let placeholders: Vec<String> = chunk.iter().map(|_| "?".to_string()).collect();
            let sql = format!(
                "INSERT OR IGNORE INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at, is_default) SELECT ?, ?, ?, ?, ?, ?, ?, ?, ? WHERE NOT EXISTS (SELECT 1 FROM file_resolved_tags WHERE file_id = ? AND tag_id = ?)",
            );
            // Actually, batch INSERT from VALUES:
            // Better approach: use a single INSERT with a VALUES list
        }

        // Simplified: bulk insert matching files as tag rows
        let inserted = sqlx::query(
            r#"
            INSERT OR IGNORE INTO file_resolved_tags (file_id, tag_id, tag_name, category_id, category_name, prefix, sort_order, created_at, is_default)
            SELECT f.id, ?, ?, ?, ?, ?, ?, ?, ?
            FROM files f
            WHERE f.id IN (...)
            "#,
        )
        // ...
        .execute(&mut *tx)
        .await?;
        total += inserted.rows_affected() as i64;
    }
    total
};
```

**Optimization note**: Instead of doing N queries (one per dynamic bundle),
we do ONE query per bundle using the SQL constructed by
`resolve_dynamic_bundle()` adapted to run within the transaction.

**`resolve_dynamic_bundle_in_tx()`**: A variant of `resolve_dynamic_bundle()`
that takes `&mut SqliteConnection` instead of `&Pool`. This is needed because
Step 3 runs inside the existing transaction.

### Backend: API Endpoints

#### New file: `src/api/dynamic_bundles.rs`

```rust
// ── Router ─────────────────────────────────────────────────────────────────
pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/dynamic-bundles", get(list_handler).post(create_handler))
        .route(
            "/api/dynamic-bundles/{id}",
            get(get_handler).put(update_handler).delete(delete_handler),
        )
        .route("/api/dynamic-bundles/{id}/resolve", post(resolve_handler))
}
```

##### `GET /api/dynamic-bundles`

Returns list with `tagName`, `tagBackpack`, `matchingFileCount`.

##### `POST /api/dynamic-bundles`

Request body:

```json
{
  "name": "Hard Techno 140-160",
  "baseTags": ["hammahalle", "spät", "bouncy"],
  "includeAllTracks": false,
  "bpmMin": 140,
  "bpmMax": 160,
  "pmvCategories": ["m"],
  "fileTypes": ["stem.m4a", "flac"],
  "excludeWavSources": true
}
```

Handler logic:

1. Validate: `name` required, non-empty; at least one filter criterion set
2. Create a Setlist-category tag with the same name via `create_tag()`
3. INSERT into `dynamic_bundles` with `tag_id`
4. Call `refresh_file_resolved_tags()` to populate initial matching files
5. Return the created bundle with `matchingFileCount`

##### `PUT /api/dynamic-bundles/{id}`

Partial update. Same body fields, all optional. After update:

1. If `name` changed, update the tag name too
2. Call `refresh_file_resolved_tags()` to re-resolve

##### `DELETE /api/dynamic-bundles/{id}`

Deletes the bundle row. Tag deleted via `ON DELETE CASCADE`. Calls
`refresh_file_resolved_tags()` to clean up stale entries.

##### `POST /api/dynamic-bundles/{id}/resolve`

Force re-resolution. Calls `refresh_file_resolved_tags()`. Returns the
current matching file count. Useful for debugging or after external
data changes (new files scanned, tags changed).

#### Hook into app router (`src/main.rs` or `src/api/mod.rs`)

```rust
// In the main router, merge dynamic bundles routes:
let app = Router::new()
    // ... existing routes ...
    .merge(crate::api::dynamic_bundles::router());
```

### Frontend: New `#dynamic-bundles` Page

#### New file: `frontend/pages/dynamic-bundles.js`

Follows the same two-panel pattern as `tag-bundles.js`:

```
┌──────────────────────────────┬───────────────────────────────────────┐
│ DYNAMIC BUNDLES              │ EDIT BUNDLE                           │
│                              │                                       │
│ [🔍 Search...]               │ Name: [Hard Techno 140-160_______]   │
│                              │                                       │
│ ┌────────────────────────┐  │ Base:                                 │
│ │ Hard Techno 140-160    │  │  ○ All tracks   ● Specific tags      │
│ │ 847 files · 🎒 backpack│  │  [hammahalle ✕] [spät ✕] [bouncy ✕] │
│ └────────────────────────┘  │  [🔍 add tag...___________________]  │
│ ┌────────────────────────┐  │                                       │
│ │ Deep House 120-128     │  │ BPM:  [140]  to  [160]               │
│ │ 312 files              │  │                                       │
│ └────────────────────────┘  │ PMV:  ☐ P  ☑ M  ☐ V                 │
│                              │                                       │
│ [+ New Dynamic Bundle]      │ Types: ☑ stem.m4a  ☑ flac  ☐ mp3    │
│                              │        ☑ Exclude WAV source files   │
│                              │                                       │
│                              │ Preview: first 20 of 847 files:      │
│                              │ ┌─────────────────────────────────┐  │
│                              │ │ Artist - Title  140BPM  4m      │  │
│                              │ │ Artist2 - Title2 155BPM 6m      │  │
│                              │ │ ...                             │  │
│                              │ └─────────────────────────────────┘  │
│                              │                                       │
│                              │ [Save]  [Delete]                     │
└──────────────────────────────┴───────────────────────────────────────┘
```

**State**:

```javascript
const state = {
  bundles: [], // DynamicBundleResponse[]
  selectedId: null,
  // Edit form state
  editName: "",
  editAllTracks: false,
  editBaseTags: [], // string[] of tag names
  editBpmMin: null,
  editBpmMax: null,
  editPmvCategories: [], // string[] of 'p','m','v'
  editFileTypes: [], // string[] of 'stem.m4a','flac','mp3','wav'
  editExcludeWav: true,
  // Typeahead
  tagSearch: "",
  typeaheadResults: [],
  typeaheadOpen: false,
  // File preview
  previewFiles: [],
  previewLoading: false,
  // General
  loading: false,
  saving: false,
};
```

**Key functions**:

| Function                    | Purpose                                           |
| --------------------------- | ------------------------------------------------- |
| `init(container, signal)`   | Fetch bundles, render page                        |
| `renderFullPage(container)` | Two-panel layout                                  |
| `renderBundleList()`        | Left panel list with file counts + backpack badge |
| `renderEditForm()`          | Right panel form with all filter controls         |
| `renderPreview()`           | First 20 matching files table                     |
| `selectBundle(id)`          | Load bundle into edit form + fetch preview        |
| `wireEvents(container)`     | All click/input handlers                          |
| `wireTagTypeahead()`        | Tag search for base tags                          |
| `saveBundle()`              | POST (create) or PUT (update)                     |
| `deleteBundle(id)`          | DELETE with confirmation                          |
| `toggleBackpack(tagId)`     | PUT /api/tags/{id}/backpack inline                |

**Registration**:

- `frontend/app.js` — add `"dynamic-bundles": "dynamic-bundles"` to PAGE_MAP
- `frontend/shared/nav.js` — add to TOOLS_ITEMS:
  ```javascript
  { href: "#dynamic-bundles", icon: "fa-filter-list", label: "Dynamic Bundles" }
  ```

### CSS: `frontend/style.css`

Add classes for the dynamic bundles page layout:

```css
/* Dynamic Bundles page */
.db-layout {
  display: flex;
  gap: 1.5rem;
  height: calc(100vh - 180px);
}
.db-list {
  width: 35%;
  overflow-y: auto;
}
.db-edit {
  width: 65%;
  overflow-y: auto;
}
.db-card {
  background: var(--bg-card);
  border: 1px solid var(--border);
  border-radius: 6px;
  padding: 1rem;
  margin-bottom: 0.75rem;
  cursor: pointer;
  transition: border-color 0.15s;
}
.db-card:hover {
  border-color: var(--primary);
}
.db-card.active {
  border-color: var(--primary);
  background: var(--bg-active);
}
.db-card-name {
  font-weight: 600;
  margin-bottom: 0.25rem;
}
.db-card-meta {
  font-size: 0.8rem;
  color: var(--muted);
  display: flex;
  gap: 0.75rem;
}
.db-edit-section {
  margin-bottom: 1.25rem;
}
.db-edit-label {
  font-weight: 600;
  font-size: 0.85rem;
  margin-bottom: 0.4rem;
  color: var(--muted);
  text-transform: uppercase;
  letter-spacing: 0.05em;
}
.db-preview-table {
  width: 100%;
  font-size: 0.85rem;
}
.db-preview-table th {
  text-align: left;
  color: var(--muted);
  font-weight: 500;
  padding: 0.3rem 0.5rem;
  border-bottom: 1px solid var(--border);
}
.db-preview-table td {
  padding: 0.25rem 0.5rem;
}
```

### Tests

#### Integration Tests: `tests/api_dynamic_bundles.rs` — NEW FILE (~8 tests)

All tests use the standard pattern: spawn app, seed basic data, call
`refresh_file_resolved_tags()`, then hit the API.

| Test                             | Endpoint                                 | What it proves                                                    |
| -------------------------------- | ---------------------------------------- | ----------------------------------------------------------------- |
| `dynamic_bundles_create`         | `POST /api/dynamic-bundles`              | Creates bundle + tag, resolves matching files, returns with count |
| `dynamic_bundles_list`           | `GET /api/dynamic-bundles`               | Returns all bundles with tag info + counts                        |
| `dynamic_bundles_get`            | `GET /api/dynamic-bundles/{id}`          | Returns single bundle detail                                      |
| `dynamic_bundles_update`         | `PUT /api/dynamic-bundles/{id}`          | Updates filters, re-resolves, count changes                       |
| `dynamic_bundles_delete`         | `DELETE /api/dynamic-bundles/{id}`       | Deletes bundle + tag, 404 on re-fetch                             |
| `dynamic_bundles_resolve`        | `POST /api/dynamic-bundles/{id}/resolve` | Force re-resolve, returns count                                   |
| `dynamic_bundles_create_no_name` | `POST /api/dynamic-bundles`              | 400 on missing name                                               |
| `dynamic_bundles_in_backpack`    | `PUT /api/tags/{id}/backpack`            | Bundle's tag toggles backpack, `is_in_backpack()` reflects it     |

#### Seed Data: `src/db/testing.rs` — new scenario `dynamic_bundles`

Extends `seed_basic_scenario`. Adds:

- Files with BPM values at 120, 140, 155 (to test BPM range filter)
- Tags "hammahalle" (id 50, Mood), "spät" (id 51, Vibe), "bouncy" (id 52, Vibe)
- Playlists matching those tag names → auto-resolution into `file_resolved_tags`
- One file linked to all three tags → base tag filter proves OR logic

Register in `testing_seed_handler` match block: `"dynamic_bundles" => testing::seed_dynamic_bundles_scenario(&state.db).await`

#### Playwright Tests: `frontend/tests/dynamic-bundles.spec.js` — NEW FILE (~3 tests)

```javascript
import { test, expect } from "@playwright/test";

test.describe("Dynamic Bundles Page", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("/api/testing/seed", {
      data: { scenario: "dynamic_bundles" },
    });
  });

  test("page loads without errors", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));
    await page.goto("/#dynamic-bundles");
    await page.waitForSelector(".db-layout", { timeout: 8000 });
    await expect(page.locator(".db-layout")).toBeVisible();
    expect(errors).toEqual([]);
  });

  test("can create a dynamic bundle with BPM filter", async ({ page }) => {
    // Fill name, add base tag, set BPM range, save
    // Assert the bundle appears in the list with matching file count
  });

  test("created bundle can be toggled as backpack", async ({ page }) => {
    // Create bundle, navigate to Tags page, find the bundle tag,
    // toggle backpack, verify backpack icon appears
  });
});
```

### Files to create

- `migrations/019_dynamic_bundles.sql` — new migration
- `src/db/dynamic_bundles.rs` — 7 DB functions
- `src/api/dynamic_bundles.rs` — 5 handlers + router
- `frontend/pages/dynamic-bundles.js` — new SPA page (~550 lines)
- `frontend/tests/dynamic-bundles.spec.js` — Playwright tests (~3 tests)
- `tests/api_dynamic_bundles.rs` — integration tests (~8 tests)

### Files to modify

- `src/db/mod.rs` — add `pub mod dynamic_bundles;`
- `src/db/playlists.rs` — add Step 3 to `refresh_file_resolved_tags()`
- `src/db/testing.rs` — add `seed_dynamic_bundles_scenario()`
- `src/api/mod.rs` — merge `dynamic_bundles::router()` into main router
- `src/api/infrastructure.rs` — register `"dynamic_bundles"` seed scenario
- `frontend/app.js` — register `"dynamic-bundles"` in PAGE_MAP
- `frontend/shared/nav.js` — add "Dynamic Bundles" to TOOLS_ITEMS
- `frontend/style.css` — add `.db-*` styles

### Agent Decomposition (TDD, 3 agents, zero file conflicts)

| Agent | Files                                                                                                                                            | Work                                                      |
| ----- | ------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------- |
| **A** | `migrations/019_dynamic_bundles.sql`, `src/db/dynamic_bundles.rs`, `src/db/playlists.rs`, `src/db/mod.rs`                                        | Migration + DB functions + resolution hook                |
| **B** | `src/api/dynamic_bundles.rs`, `src/api/mod.rs`, `src/api/infrastructure.rs`, `src/db/testing.rs`, `tests/api_dynamic_bundles.rs`                 | API handlers + router + seed scenario + integration tests |
| **C** | `frontend/pages/dynamic-bundles.js`, `frontend/app.js`, `frontend/shared/nav.js`, `frontend/style.css`, `frontend/tests/dynamic-bundles.spec.js` | Frontend page + nav + styles + Playwright tests           |

**Execution order**: Agents A and B can run in parallel (A defines DB layer,
B defines API + tests that use that DB layer — B's tests will fail until A's
migration creates the table, but B can write the code against the planned API).
Agent C runs after A+B (needs working API to test against).

Actually, all three can run in parallel since they have **completely disjoint
write scopes** — no file is touched by more than one agent.

### Acceptance Criteria

**Backend:**

- [ ] Migration 019 runs cleanly on fresh DB (001→019)
- [ ] Migration 019 runs cleanly on existing DB with data
- [ ] `create_dynamic_bundle()` creates tag (Setlist category) + bundle row
- [ ] `resolve_dynamic_bundle()` returns correct file IDs for each filter combination
- [ ] Base tags filter: OR logic (file with any base tag is included)
- [ ] All tracks toggle: includes every file (when on)
- [ ] BPM range: files outside range excluded
- [ ] PMV categories: files without matching prefix excluded
- [ ] File types: files not in selected types excluded
- [ ] Exclude WAV sources: `source_of IS NOT NULL` WAVs excluded when toggle on
- [ ] `refresh_file_resolved_tags()` Step 3: inserts bundle tag for matching files
- [ ] `is_in_backpack()` returns true for files matching a backpack-enabled dynamic bundle
- [ ] `get_backpack_pull_candidates()` includes files from backpack-enabled dynamic bundles
- [ ] Deleting a dynamic bundle deletes the tag (ON DELETE CASCADE) and cleans `file_resolved_tags`

**API:**

- [ ] `GET /api/dynamic-bundles` returns list with tagName, tagBackpack, matchingFileCount
- [ ] `POST /api/dynamic-bundles` creates bundle + tag, returns with count
- [ ] `POST` with empty name → 400
- [ ] `GET /api/dynamic-bundles/{id}` returns single bundle detail
- [ ] `PUT /api/dynamic-bundles/{id}` updates filters, re-resolves
- [ ] `DELETE /api/dynamic-bundles/{id}` deletes, 404 on re-fetch
- [ ] `POST /api/dynamic-bundles/{id}/resolve` force-re-resolves, returns count

**Frontend:**

- [ ] `#dynamic-bundles` page loads with two-panel layout
- [ ] Left panel: bundle list with file counts + backpack badge
- [ ] Right panel: edit form for selected bundle
- [ ] All tracks / Specific tags toggle works
- [ ] Base tag typeahead + chips (add/remove)
- [ ] BPM min/max inputs
- [ ] PMV category checkboxes
- [ ] File type checkboxes + exclude WAV toggle
- [ ] Preview table shows first 20 matching files
- [ ] Save persists changes
- [ ] Delete with confirmation
- [ ] New Dynamic Bundle button opens blank form
- [ ] Registered in PAGE_MAP + nav TOOLS_ITEMS

**Tests:**

- [ ] 8 integration tests pass (`cargo test --test api_dynamic_bundles`)
- [ ] 3 Playwright tests pass (`cd frontend && npx playwright test -- tests/dynamic-bundles.spec.js`)
- [ ] Seed scenario `dynamic_bundles` registered and functional

**Validation:**

- [ ] `cargo build` passes
- [ ] `cargo test` passes (all existing + new tests)
- [ ] `cd frontend && npx playwright test` passes (all existing + new tests)

---

