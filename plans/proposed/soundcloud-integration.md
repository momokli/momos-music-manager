## Plan: soundcloud-integration

**Status**: proposed
**Branch**: `feat/soundcloud-integration`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no (SoundCloud already in schema since 001)

### Description

Implement SoundCloud as a first-class service — full playlist + track sync, matching
the Spotify integration pattern. The `soundcloud-rs` crate (v0.14.0) is already a
dependency. SoundCloud uses a simpler auth model (no OAuth — auto-discovers
`client_id` from their site, or uses the user's `api_key`), so the implementation
is simpler than Spotify: no token refresh, no subscription poller for v1.

### Current State (verified 2026-06-11)

- **Schema** (001_initial_schema): `service_tracks` CHECK includes `'soundcloud'`,
  `files.soundcloud_id` column, `v_file_track_link` matches `service='soundcloud'
AND service_id = f.soundcloud_id`, index `idx_files_soundcloud_id`
- **Config**: `SoundcloudToml { api_key, user_id }` + `ServiceCredentials
{ soundcloud_api_key, soundcloud_user_id }` + `is_soundcloud_configured()`
- **Frontend** (`frontend/pages/services.js`): SoundCloud service meta (name,
  icon `fa-brands fa-soundcloud`, color `#ff7700`); renders as "Not Configured" / "Auth Needed"
- **API stubs** (`src/api/services.rs`):
  - `service_auth_handler` line ~202-210 → returns 501 "SoundCloud OAuth not yet implemented"
  - `service_sync_handler` line ~563-565 → returns 501 "SoundCloud sync not yet implemented"
- **Dependency**: `soundcloud-rs = "0.14.0"` in Cargo.toml, compiles cleanly
- **Module structure**: codebase uses domain modules (`src/soundcloud/` to be created);
  API routes merged via `src/api/mod.rs` → `router()`; services live in
  `src/api/services.rs` with sub-router in `pub(super) fn router()`

### What `soundcloud-rs` provides (verified from crate source)

| Method                                   | Returns                       | Notes                                               |
| ---------------------------------------- | ----------------------------- | --------------------------------------------------- |
| `Client::new()`                          | `Client`                      | Auto-discovers SC `client_id` by scraping site HTML |
| `get_user(Identifier)`                   | `User`                        | Full user profile                                   |
| `get_user_playlists(id, Option<Paging>)` | `Playlists(PagingCollection)` | `PagingCollection.collection: Vec<T>` — all-in-one  |
| `get_playlist(Identifier)`               | `Playlist`                    | Includes `tracks: Option<Vec<Track>>`               |
| `health_check()`                         | `bool`                        | Calls `/me`, returns true on 2xx                    |

Key models (all fields `Option`):

- **Playlist**: `id: Option<i32>`, `title: Option<String>`, `track_count: Option<i32>`,
  `tracks: Option<Vec<Track>>`, `user: Option<UserSummary>`, `urn`, `permalink_url`,
  `description`
- **Track**: `id: Option<i64>`, `title: Option<String>`, `isrc: Option<String>`,
  `bpm: Option<f64>`, `genre: Option<String>`, `duration: Option<i64>`,
  `user: Option<UserSummary>`, `urn`, `permalink_url`, `artwork_url`
- **UserSummary**: `id: Option<i64>`, `username: Option<String>`,
  `permalink_url: Option<String>`, `avatar_url`
- **PagingCollection<T>**: `collection: Vec<T>` — bundled, no manual pagination
- `Client.client_id: RwLock<String>` — publicly settable for manual api_key injection

### Auth model

SoundCloud has **no OAuth**. Auth flow:

1. `soundcloud-rs` auto-discovers a public `client_id` by fetching soundcloud.com
   HTML, extracting JS `<script>` URLs, and regex-matching `client_id[:=]"?(\w{32})`
2. If the user provides `api_key` in config, we manually inject it into the
   `Client.client_id` RwLock (bypassing auto-discovery)
3. For SoundCloud, "auth" = validate config + verify `health_check()` passes →
   set `service_config.is_connected = true` — **no redirect, no OAuth dance**
4. The `user_id` from config identifies whose playlists to sync

Config (`~/.config/momos-music-manager/config.toml` or env var):

```toml
[soundcloud]
api_key = "your_client_id"   # optional — overrides auto-discovery
user_id = "12345"            # required — SC user ID (numeric) or permalink
```

Env override: `SOUNDCLOUD_API_KEY=...` / `SOUNDCLOUD_USER_ID=...`

### Design decisions

1. **No new DB migration.** Schema already supports SoundCloud since 001.
2. **BPM stored in `metadata_json`.** The `service_tracks` table has no `bpm` column
   (only `files` does). Serialize BPM + genre into `metadata_json`:
   `{"bpm": 128.0, "genre": "Techno"}`
3. **Reuse existing DB functions.** `upsert_service_playlist()`, `upsert_service_track()`,
   `add_track_to_playlist_with_added_at()` are all service-agnostic and already handle
   `service='soundcloud'` via the CHECK constraint.
4. **Follow `src/api/spotify_sync.rs` pattern.** New `src/soundcloud/` module with
   the same structure: models, client wrapper, sync worker.
5. **SoundCloud sync routes** follow Spotify's URL scheme:
   `/api/services/soundcloud/sync/playlists`, `/sync/full`,
   `/sync/playlists/{id}/tracks`, `/sync/{task_id}`.
6. **Frontend auth flow differs from Spotify.** Since SC has no OAuth,
   `authorizeService("soundcloud")` → `POST /api/services/soundcloud/auth` →
   backend validates config + health-checks → if ok, sets `is_connected=true`
   and returns success JSON → frontend reloads the services list (no redirect).
   The existing `authorizeService()` function already handles `resp.data` as
   redirect URL; we add a branch for SoundCloud that detects the response shape.

### Backend Changes

#### 1. New module: `src/soundcloud/` (4 files)

```
src/soundcloud/
├── mod.rs          # pub mod client; pub mod models; pub mod sync_worker;
├── client.rs       # ScClient — thin wrapper over soundcloud_rs::Client
├── models.rs       # ScPlaylistInfo, ScTrackInfo + From impls
└── sync_worker.rs  # ScSyncWorker — TaskManager-integrated background sync
```

#### 1a. `src/soundcloud/models.rs` — internal types

```rust
/// Our internal playlist representation.
#[derive(Debug, Clone)]
pub struct ScPlaylistInfo {
    pub id: i32,
    pub title: String,
    pub description: Option<String>,
    pub track_count: i32,
    pub urn: Option<String>,
    pub permalink_url: Option<String>,
    pub user_id: Option<i64>,
    pub username: Option<String>,
}

/// Our internal track representation.
#[derive(Debug, Clone)]
pub struct ScTrackInfo {
    pub id: i64,
    pub title: String,
    pub artist: String,       // from Track.user.username
    pub isrc: Option<String>,
    pub bpm: Option<f64>,
    pub genre: Option<String>,
    pub duration_ms: i64,
    pub urn: Option<String>,
    pub permalink_url: Option<String>,
}

// impl From<&soundcloud_rs::Playlist> for ScPlaylistInfo
// impl From<&soundcloud_rs::Track> for ScTrackInfo
```

#### 1b. `src/soundcloud/client.rs` — client wrapper

```rust
use crate::config::ServiceCredentials;
use soundcloud_rs::{Client, Identifier};

pub struct ScClient {
    client: Client,
    user_id: String,
}

impl ScClient {
    /// Create client. If `api_key` is set, injects it directly into the
    /// Client's client_id RwLock. Otherwise auto-discovers.
    pub async fn new(config: &ServiceCredentials) -> Result<Self>;

    /// Calls `client.health_check()`
    pub async fn health_check(&self) -> bool;

    /// Fetch all user playlists (PagingCollection bundles all pages).
    pub async fn get_user_playlists(&self) -> Result<Vec<ScPlaylistInfo>>;

    /// Fetch a single playlist including its tracks.
    pub async fn get_playlist(
        &self, playlist_id: i32
    ) -> Result<(ScPlaylistInfo, Vec<ScTrackInfo>)>;

    /// Build a soundcloud_rs::Identifier from config user_id.
    fn user_identifier(&self) -> Identifier;
}
```

#### 1c. `src/soundcloud/sync_worker.rs` — background sync

Follows the `SpotifySyncWorker` pattern (`src/spotify/sync_worker.rs`):

```rust
use crate::tasks::{SyncProgress, SyncResult, SyncType, TaskStatus};
use sqlx::Pool;
use tokio_util::sync::CancellationToken;

pub struct ScSyncWorker {
    db: Pool<Sqlite>,
    sc_client: ScClient,
    task_id: String,
    sync_type: SyncType,
    cancel_token: CancellationToken,
    progress: Arc<tokio::sync::RwLock<SyncProgress>>,
}

impl ScSyncWorker {
    pub fn new(db, client, task_id, sync_type, cancel_token) -> Self;
    pub async fn run(&self) -> Result<SyncResult>;

    async fn sync_playlists_only(&self) -> Result<usize>;
    async fn sync_single_playlist(&self, playlist_id: i32) -> Result<usize>;
    async fn sync_full(&self) -> Result<(usize, usize)>;
}
```

**Sync algorithm (full)**:

1. Create task in `TaskManager` with `SyncType::Full`, status `Running`
2. Fetch user playlists via `get_user_playlists()` → `Vec<ScPlaylistInfo>`
3. For each playlist:
   a. Check cancel token
   b. Upsert into `service_playlists` (service='soundcloud', playlist_id=sc_id)
   c. Fetch full playlist (with tracks) via `get_playlist(id)`
   d. For each track: upsert `service_tracks` with `metadata_json = {bpm, genre}`
   e. Link tracks to playlist in `service_playlist_tracks` (use existing
   `add_track_to_playlist_with_added_at`)
   f. Update task progress: `syncCurrentPlaylist`, `syncCurrentTrack`
4. Set task status `Completed`, return `SyncResult`

**Sync modes**: `PlaylistsOnly`, `Full`, `SinglePlaylist`

#### 2. Module registration

**`src/main.rs`** — add `pub mod soundcloud;` alongside existing `pub mod spotify;`

**`src/soundcloud/mod.rs`**:

```rust
pub mod client;
pub mod models;
pub mod sync_worker;
```

#### 3. API: SoundCloud sync routes — new file `src/api/soundcloud_sync.rs`

Following the pattern of `src/api/spotify_sync.rs`:

```rust
use axum::{Json, Router, extract::{Path, State}, response::IntoResponse, routing::{get, post, delete}};
use std::sync::Arc;
use crate::AppState;
use crate::api::types::{ApiResponse, internal_error};
use crate::tasks::SyncType;

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/services/soundcloud/sync/playlists", post(sc_sync_playlists_handler))
        .route("/api/services/soundcloud/sync/full", post(sc_sync_full_handler))
        .route("/api/services/soundcloud/sync/playlists/{playlist_id}/tracks", post(sc_sync_playlist_tracks_handler))
        .route("/api/services/soundcloud/sync/{task_id}", get(sc_sync_task_handler).delete(sc_sync_cancel_handler))
}

async fn sc_sync_full_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // 1. Validate SC is configured (is_soundcloud_configured + user_id present)
    // 2. Create ScClient::new(&state.config)
    // 3. Spawn ScSyncWorker with SyncType::Full via state.task_manager
    // 4. Return { taskId, status: "running" }
}

// ... other handlers follow same pattern
```

#### 4. Integrate into API router

**`src/api/mod.rs`** — add to `router()`:

```rust
pub mod soundcloud_sync;
// ...
.merge(soundcloud_sync::router())
```

#### 5. Fix stub handlers in `src/api/services.rs`

**`service_auth_handler`** (line ~202-210): Replace 501 stub with actual SC auth:

```rust
"soundcloud" => {
    // SoundCloud has no OAuth — validate config, health-check, mark connected
    let api_key = state.config.soundcloud_api_key.clone();
    let user_id = state.config.soundcloud_user_id.clone()
        .ok_or_else(|| anyhow::anyhow!("SoundCloud user_id not configured"))?;

    // Create client and health-check
    let client = crate::soundcloud::client::ScClient::new(&state.config).await
        .map_err(|e| anyhow::anyhow!("SoundCloud client creation failed: {}", e))?;

    if !client.health_check().await {
        return (StatusCode::BAD_REQUEST,
            Json(ApiResponse { data: "SoundCloud API unreachable".to_string() })).into_response();
    }

    // Mark as connected in DB
    update_service_connection_status(&state.db, "soundcloud", true).await
        .map_err(|e| anyhow::anyhow!("Failed to update connection status: {}", e))?;

    Json(ApiResponse { data: serde_json::json!({"connected": true, "service": "soundcloud"}) }).into_response()
}
```

**`service_sync_handler`** (line ~563-565): Replace 501 stub with dispatch to
`soundcloud_sync::sc_sync_full_handler`.

The handler currently has this match arm for soundcloud:

```rust
"soundcloud" => (
    StatusCode::NOT_IMPLEMENTED,
    Json(ApiResponse {
        data: "SoundCloud sync not yet implemented".to_string(),
    }),
).into_response(),
```

Replace with a call to the internal sync handler (same pattern as spotify):

```rust
"soundcloud" => {
    // Delegate to soundcloud_sync module's full-sync handler
    soundcloud_sync::sc_sync_full_handler(State(state)).await.into_response()
}
```

#### 6. DB functions — no new functions needed

The sync worker calls existing, service-agnostic DB functions:

| Function                                | Location               | Used for                         |
| --------------------------------------- | ---------------------- | -------------------------------- |
| `upsert_service_playlist()`             | `src/db/playlists.rs`  | INSERT or UPDATE playlist row    |
| `upsert_service_track()`                | `src/db/tracks.rs`     | INSERT or UPDATE track row       |
| `add_track_to_playlist_with_added_at()` | `src/db/playlists.rs`  | Link track ↔ playlist            |
| `get_service_config()` / `update_...()` | `src/db/connection.rs` | Read/update `service_config` row |
| `update_service_connection_status()`    | `src/db/connection.rs` | Set `is_connected` flag          |

**metadata_json construction** (in sync_worker, not DB):

```rust
let metadata = serde_json::json!({
    "bpm": track.bpm,
    "genre": track.genre,
});
let metadata_json = serde_json::to_string(&metadata).ok();
```

### Frontend Changes (`frontend/pages/services.js`)

#### 1. Fix `authorizeService()` to handle SoundCloud's non-OAuth flow

Currently `authorizeService()` always does `window.location.href = resp.data`.
For SoundCloud, the response is `{connected: true, service: "soundcloud"}`,
not a URL. Add a branch:

```javascript
async function authorizeService(service) {
  const btn = document.querySelector(`[data-action="authorize"][data-id="${service}"]`);
  if (!btn) return;
  const originalHtml = btn.innerHTML;
  btn.disabled = true;
  btn.innerHTML = '<i class="fa-solid fa-spinner fa-spin"></i> Connecting...';

  try {
    const resp = await fetchJSON(`/api/services/${service}/auth`, { method: "POST" });
    if (service === "soundcloud") {
      // SoundCloud has no OAuth — auth just verifies config and marks connected
      showSuccess("SoundCloud connected");
      loadServices(); // refresh the page
      return;
    }
    // Spotify/YouTube: resp.data is the redirect URL
    window.location.href = resp.data;
  } catch (err) {
    showError(`OAuth failed: ${err.message}`);
    btn.disabled = false;
    btn.innerHTML = originalHtml;
  }
}
```

#### 2. No other frontend changes needed

- `renderServiceRow()` already handles the "connected" state correctly for
  any service — when `s.status === "connected"`, it shows resync/reset buttons
- `resyncService()` calls `POST /api/services/{service}/sync` which now
  dispatches to the soundcloud sync handler
- The config modal (`openConfigModal`) already works generically

### Tests (TDD — write tests FIRST, then implement)

#### Integration tests: `tests/api_soundcloud.rs` — NEW FILE (~6 tests)

All tests follow the standard pattern: spawn app, seed basic data, hit the API.
Since SoundCloud requires a real API for full sync, test the **error paths**
and **auth flow**. Full sync is tested manually against the live API.

| Test                                  | Endpoint                                        | What it proves                                  |
| ------------------------------------- | ----------------------------------------------- | ----------------------------------------------- |
| `soundcloud_auth_not_configured`      | `POST /api/services/soundcloud/auth`            | 400 when SC not configured (no api_key/user_id) |
| `soundcloud_auth_no_user_id`          | `POST /api/services/soundcloud/auth`            | 400 when api_key set but user_id missing        |
| `soundcloud_sync_not_configured`      | `POST /api/services/soundcloud/sync`            | 400 when SC not configured                      |
| `soundcloud_sync_playlists_error`     | `POST /api/services/soundcloud/sync/playlists`  | Error response when SC client can't reach API   |
| `soundcloud_sync_task_not_found`      | `GET /api/services/soundcloud/sync/nonexistent` | 404 for non-existent task ID                    |
| `soundcloud_sync_full_not_configured` | `POST /api/services/soundcloud/sync/full`       | 400 when SC not configured                      |

#### Seed data

No new seed data needed — tests use unconfigured state by default (the test
app starts with `ServiceCredentials::defaults_for_test()` which has no SC keys).

#### Playwright tests: `frontend/tests/services.spec.js` — NEW FILE (~2 tests)

```javascript
import { test, expect } from "@playwright/test";

test.describe("Services Page — SoundCloud", () => {
  test.beforeEach(async ({ request }) => {
    await request.post("/api/testing/seed", {
      data: { scenario: "basic" },
    });
  });

  test("SoundCloud shows on services page", async ({ page }) => {
    const errors = [];
    page.on("pageerror", (err) => errors.push(err));
    await page.goto("/#services");
    await page.waitForSelector('[data-service-id="soundcloud"]', { timeout: 8000 });
    await expect(page.locator('[data-service-id="soundcloud"]')).toBeVisible();
    expect(errors).toEqual([]);
  });

  test("SoundCloud shows Auth Needed when configured but not authed", async ({
    page,
  }) => {
    // SoundCloud is in the service list with configured=false by default
    await page.goto("/#services");
    await page.waitForSelector('[data-service-id="soundcloud"]', { timeout: 8000 });
    const row = page.locator('[data-service-id="soundcloud"]');
    await expect(row.locator(".status-badge")).toContainText("Not Configured");
  });
});
```

### Agent Decomposition (TDD — 4 agents, zero file conflicts)

All agents write tests FIRST, then implement. All can run in parallel — zero
overlapping files.

| Agent | Files                                                                                            | Work                                                                                | Tests          |
| ----- | ------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------------- | -------------- |
| **A** | `src/soundcloud/mod.rs`, `src/soundcloud/models.rs`, `src/soundcloud/client.rs`, `src/main.rs`   | SoundCloud module skeleton, client wrapper, types + From impls, module registration | ~3 unit        |
| **B** | `src/soundcloud/sync_worker.rs`, `src/tasks/mod.rs` (if TaskType enum needs SC variant)          | Sync worker implementation, TaskManager integration                                 | ~2 unit        |
| **C** | `src/api/soundcloud_sync.rs`, `src/api/mod.rs`, `src/api/services.rs`, `tests/api_soundcloud.rs` | Sync routes + handlers, router integration, fix stub handlers, integration tests    | ~6 integration |
| **D** | `frontend/pages/services.js`, `frontend/tests/services.spec.js`                                  | Fix authorizeService for SC, add Playwright tests                                   | ~2 Playwright  |

**Write scope verification — zero overlap:**

- Agent A: `src/soundcloud/mod.rs`, `models.rs`, `client.rs`, `src/main.rs`
- Agent B: `src/soundcloud/sync_worker.rs`, `src/tasks/mod.rs`
- Agent C: `src/api/soundcloud_sync.rs`, `src/api/mod.rs`, `src/api/services.rs`, `tests/api_soundcloud.rs`
- Agent D: `frontend/pages/services.js`, `frontend/tests/services.spec.js`

### Per-Agent Task Briefs

#### Agent A: SoundCloud module skeleton + client

1. Create `src/soundcloud/mod.rs`, `src/soundcloud/models.rs`, `src/soundcloud/client.rs`
2. `models.rs`: define `ScPlaylistInfo`, `ScTrackInfo` with `From<&soundcloud_rs::Playlist>` and `From<&soundcloud_rs::Track>` impls
3. `client.rs`: `ScClient` struct + `new()`, `health_check()`, `get_user_playlists()`,
   `get_playlist()`, `user_identifier()`. Handle `api_key` override: if config has
   `soundcloud_api_key`, do `client.client_id.write().await.clone_from(&api_key)`
   after `Client::new().await`
4. `mod.rs`: `pub mod client; pub mod models; pub mod sync_worker;`
5. Add `pub mod soundcloud;` to `src/main.rs`
6. Run `cargo build` — must compile

#### Agent B: Sync worker

1. Read `src/spotify/sync_worker.rs` to understand the pattern
2. Create `src/soundcloud/sync_worker.rs`: `ScSyncWorker` struct + `run()`, `sync_full()`,
   `sync_playlists_only()`, `sync_single_playlist()`
3. Uses `crate::tasks::{SyncProgress, SyncResult, SyncType, TaskStatus}`
4. Uses `crate::db::{upsert_service_playlist, upsert_service_track,
add_track_to_playlist_with_added_at}`
5. `sync_full()`: fetch playlists → for each, fetch tracks → upsert → link
6. Check cancel token between playlists
7. Run `cargo build` — must compile

#### Agent C: API routes + integration tests (TDD)

1. Write `tests/api_soundcloud.rs` FIRST (~6 tests, see table above)
2. Create `src/api/soundcloud_sync.rs` with router + 5 handlers
3. Fix `src/api/services.rs`:
   - `service_auth_handler`: replace SC 501 with actual auth (config check + health-check + mark connected)
   - `service_sync_handler`: replace SC 501 with `soundcloud_sync::sc_sync_full_handler(state).await`
4. Add `pub mod soundcloud_sync;` to `src/api/mod.rs` + `.merge(soundcloud_sync::router())`
5. Run `cargo test --test api_soundcloud` — all 6 must pass
6. Run `cargo build` — must compile

#### Agent D: Frontend + Playwright tests

1. Write `frontend/tests/services.spec.js` with 2 Playwright tests
2. Fix `authorizeService()` in `frontend/pages/services.js` for SoundCloud
   (detect non-URL response, show success toast + reload)
3. Run `cd frontend && npx playwright test -- tests/services.spec.js` — must pass

### Acceptance Criteria

**Backend:**

- [ ] `ScClient::new()` works with auto-discovery (no api_key set)
- [ ] `ScClient::new()` works with manual `api_key` override
- [ ] `ScClient::health_check()` returns bool (true when SC API reachable)
- [ ] `POST /api/services/soundcloud/auth` validates config + health-checks + marks connected
- [ ] `POST /api/services/soundcloud/auth` returns 400 when not configured
- [ ] `POST /api/services/soundcloud/auth` returns 400 when user_id missing
- [ ] `POST /api/services/soundcloud/sync` delegates to full-sync handler (no more 501)
- [ ] `POST /api/services/soundcloud/sync/full` returns `{ taskId, status: "running" }`
- [ ] `POST /api/services/soundcloud/sync/playlists` fetches playlists only
- [ ] `POST /api/services/soundcloud/sync/playlists/{id}/tracks` fetches one playlist's tracks
- [ ] `GET /api/services/soundcloud/sync/{taskId}` returns task status
- [ ] `DELETE /api/services/soundcloud/sync/{taskId}` cancels running task
- [ ] SoundCloud tracks stored with correct `service='soundcloud'`
- [ ] BPM + genre serialized into `metadata_json`
- [ ] Tag matching works automatically: SC playlist names → tags via `v_tag_playlist`
- [ ] Files with `soundcloud_id` match SC tracks via `v_file_track_link`
- [ ] Sync progress visible in Tasks page UI

**Frontend:**

- [ ] SoundCloud shows "Auth Needed" when configured but not connected
- [ ] Clicking Authorize on SoundCloud calls auth endpoint and refreshes to "Connected"
- [ ] SoundCloud shows "Connected" after successful auth
- [ ] Sync button visible and triggers SC sync (no "not implemented" error)
- [ ] No regressions: Spotify auth/sync still works, Deemix config still works

**Tests:**

- [ ] 6 integration tests pass (`cargo test --test api_soundcloud`)
- [ ] 2 Playwright tests pass (`cd frontend && npx playwright test -- tests/services.spec.js`)

**Validation:**

- [x] `cargo build` passes (zero new warnings)
- [ ] `cargo test` passes (all 414 existing + ~6 new tests)
- [ ] `cd frontend && npx playwright test` passes (5 existing + 2 new tests)
- [ ] Test against real SoundCloud API with `curl`:
  ```bash
  curl -s -X POST http://localhost:3000/api/services/soundcloud/auth | jq
  curl -s -X POST http://localhost:3000/api/services/soundcloud/sync/full | jq
  ```

### Out of scope (v2)

- Subscription poller for SC (Spotify-only for now)
- Global poller for SC (no snapshot-based change detection on SC)
- Auto-creating SC playlists from local playlists
- SC audio streaming in the digging page
- SC track search integration in digging suggestions
- SC user likes/reposts syncing

---

