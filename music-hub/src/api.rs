//! Read-only HTTP surface over the overlap views + a guarded SQL console.
//!
//! All non-public routes require a web session (`hub_web_sessions`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Column, Row, SqlitePool};

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub ro_pool: SqlitePool,
    pub cfg: Arc<crate::config::Config>,
    /// In-flight Spotify OAuth flows: state token → (user_id, PKCE verifier).
    pub oauth_states: Arc<Mutex<HashMap<String, (i64, String)>>>,
    /// In-memory health of the Hub's own worker loops (issue #353).
    pub worker_health: crate::worker::WorkerHealth,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/hub/health", get(health))
        .route("/api/hub/status", get(status))
        .route("/api/hub/users", get(users))
        .route("/api/hub/tracks/{id}", get(track))
        .route("/api/hub/tracks/{id}/ripeness", get(track_ripeness))
        .route("/api/hub/overlap", get(overlap))
        .route("/api/hub/similar", get(similar))
        .route("/api/hub/playlists", get(playlists))
        .route("/api/hub/query", post(query))
        .route("/api/hub/me", get(me))
        .route("/api/hub/tag-task", get(tag_task))
        .route("/api/hub/services/{service}/sync", post(sync))
        .with_state(state.clone())
        .merge(crate::artists::router(state.clone()))
        .merge(crate::albums::router(state.clone()))
        .merge(crate::history::router(state.clone()))
        .merge(crate::listening::router(state.clone()))
        .merge(crate::clusters::router(state.clone()))
        .merge(crate::music_ops::router(state.clone()))
        .merge(crate::hub_ops::router(state.clone()))
        .merge(crate::tagqueue::router(state.clone()))
        .merge(crate::devices::router(state.clone()))
        .merge(crate::traktor_api::router(state.clone()))
        .merge(crate::tagtaskui::router(state))
}

/// The full application: JSON API + web + pages, with the per-view filter-state
/// middleware layered on top (single entry point so it can't be forgotten).
pub fn app(state: AppState) -> Router {
    router(state.clone())
        .merge(crate::web::router(state.clone()))
        .merge(crate::pages::router(state.clone()))
        .layer(axum::middleware::from_fn_with_state(
            state,
            crate::viewstate::middleware,
        ))
}

type ApiError = (StatusCode, Json<Value>);

fn err(code: StatusCode, msg: impl Into<String>) -> ApiError {
    (code, Json(json!({ "error": msg.into() })))
}

async fn health() -> Json<Value> {
    Json(json!({
        "data": {
            "status": "ok",
            "version": crate::VERSION,
            "gitSha": crate::GIT_SHA,
            "describe": crate::GIT_DESCRIBE,
        }
    }))
}

// ── ops status (issue #353) ─────────────────────────────────────────────────

/// On-demand backlog counts for the status endpoint (read-only pool).
#[derive(Debug, Clone, Default)]
pub struct BacklogCounts {
    pub features_pending: i64,
    pub feature_requests: i64,
    pub genres_pending: i64,
    pub crawl_pending: i64,
    pub curation_due: i64,
    pub likes_queued: i64,
    pub playlists_pending: i64,
}

/// Run one `COUNT(*)`, tolerating failure (0 instead of an error) so the status
/// endpoint can never 500 on a slow/absent table.
async fn count(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

/// Backlog depths drained by the Hub loops — each number is one bounded COUNT
/// on the read-only pool; a failing query degrades to `0` (issue #353).
pub async fn backlog_counts(pool: &SqlitePool) -> BacklogCounts {
    let features_pending = count(
        pool,
        "SELECT COUNT(*) FROM hub_tracks t
           JOIN hub_track_external_ids e ON e.track_id = t.id AND e.service = 'spotify'
           LEFT JOIN hub_track_features f ON f.track_id = t.id
          WHERE f.track_id IS NULL AND e.external_id <> ''",
    )
    .await;
    let feature_requests = count(pool, "SELECT COUNT(*) FROM hub_feature_requests").await;
    let genres_pending = count(
        pool,
        "SELECT COUNT(*) FROM hub_tracks t
          WHERE t.artists IS NOT NULL AND t.artists <> ''
            AND t.title IS NOT NULL AND t.title <> ''
            AND NOT EXISTS (SELECT 1 FROM hub_track_genres g
                             WHERE g.track_id = t.id AND g.source = 'lastfm')",
    )
    .await;
    let crawl_pending =
        count(pool, "SELECT COUNT(*) FROM hub_crawl_state WHERE state = 'pending'").await;
    let cutoff = (chrono::Utc::now() - chrono::Duration::hours(20)).to_rfc3339();
    let curation_due = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM hub_curated_playlists
          WHERE daily = 1 AND spotify_playlist_id IS NOT NULL
            AND (last_push_at IS NULL OR last_push_at < ?1)",
    )
    .bind(&cutoff)
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let likes_queued = count(
        pool,
        "SELECT COUNT(*) FROM hub_service_accounts
          WHERE service = 'spotify' AND access_token IS NOT NULL
            AND (likes_status IS NULL OR likes_status = 'queued')",
    )
    .await;
    let playlists_pending = count(
        pool,
        "SELECT COUNT(*) FROM hub_playlists
          WHERE service = 'spotify' AND enabled_for_fetch = 1
            AND (items_available = 0
                 OR (snapshot_id IS NOT NULL
                     AND snapshot_id <> IFNULL(last_synced_snapshot, '')))",
    )
    .await;

    BacklogCounts {
        features_pending,
        feature_requests,
        genres_pending,
        crawl_pending,
        curation_due,
        likes_queued,
        playlists_pending,
    }
}

/// A loop is stale when its last run is older than 3× its expected cycle
/// (60 s for the fast loops, 2 h for curation). Never run yet → not stale, so a
/// freshly booted hub does not report `degraded`.
fn is_stale(name: &str, last_run_at: Option<&str>) -> bool {
    let Some(ts) = last_run_at else { return false };
    let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts) else {
        return false;
    };
    let age = chrono::Utc::now().signed_duration_since(dt).num_seconds();
    let cycle_secs: i64 = if name == "curation" { 2 * 3600 } else { 60 };
    age > 3 * cycle_secs
}

/// Build the `data` object for `GET /api/hub/status` (issue #353 §3.2).
///
/// Always a complete shape: the music-api block is present even when
/// unconfigured/unreachable, and every backlog count degrades to `0`.
pub async fn status_data(st: &AppState) -> Value {
    let snap = st.worker_health.snapshot();
    let backlog = backlog_counts(&st.ro_pool).await;

    let mut degraded = false;
    let workers: Vec<Value> = snap
        .loops
        .iter()
        .map(|l| {
            if l.last_error.is_some() || is_stale(l.name, l.last_run_at.as_deref()) {
                degraded = true;
            }
            json!({
                "name": l.name,
                "lastRunAt": l.last_run_at,
                "lastWorked": l.last_worked,
                "iterations": l.iterations,
                "lastError": l.last_error,
            })
        })
        .collect();

    // music-api: aggregate the already-proxied worker + live queue state.
    let configured = st.cfg.music_api_token.is_some();
    let (items, _err, counts) = crate::music_api::live_queue(&st.cfg).await;
    let queue_depth = items.len() as i64;
    let wi = crate::music_api::worker_info(&st.cfg).await;
    let reachable = wi.ok;
    if configured && !reachable {
        degraded = true;
    }
    let counts: Vec<Value> = counts
        .iter()
        .map(|(state, n)| json!({ "state": state, "count": n }))
        .collect();

    json!({
        "status": if degraded { "degraded" } else { "ok" },
        "version": crate::VERSION,
        "gitSha": crate::GIT_SHA,
        "describe": crate::GIT_DESCRIBE,
        "startedAt": snap.started_at,
        "uptimeSecs": st.worker_health.uptime_secs(),
        "hub": {
            "workers": workers,
            "backlog": {
                "featuresPending": backlog.features_pending,
                "featureRequests": backlog.feature_requests,
                "genresPending": backlog.genres_pending,
                "crawlPending": backlog.crawl_pending,
                "curationDue": backlog.curation_due,
                "likesQueued": backlog.likes_queued,
                "playlistsPending": backlog.playlists_pending,
            },
        },
        "musicApi": {
            "configured": configured,
            "reachable": reachable,
            "worker": {
                "ok": wi.ok,
                "paused": wi.paused,
                "intervalSecs": wi.interval_secs,
                "maxAttempts": wi.max_attempts,
            },
            "queue": { "depth": queue_depth, "counts": counts },
        },
    })
}

/// `GET /api/hub/status` — session-gated ops view of the Hub loops, backlog and
/// the downstream music-api worker/queue (issue #353). Never 500s on backlog.
async fn status(
    State(st): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    if crate::web::current_user(&st, &headers).await.is_none() {
        return Err(err(StatusCode::UNAUTHORIZED, "login required"));
    }
    Ok(Json(json!({ "data": status_data(&st).await })))
}

#[derive(Deserialize)]
struct SimilarQuery {
    track: i64,
    #[serde(default, deserialize_with = "crate::table::qnum::opt_i64")]
    limit: Option<i64>,
}

/// `GET /api/hub/similar?track=ID&limit=50` — EffNet embedding neighbours.
async fn similar(
    State(st): State<AppState>,
    Query(q): Query<SimilarQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    if crate::web::current_user(&st, &headers).await.is_none() {
        return Err(err(StatusCode::UNAUTHORIZED, "login required"));
    }
    let limit = q.limit.unwrap_or(50).clamp(1, 200);
    let nb = crate::similar::neighbors(&st.pool, q.track, limit)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut data: Vec<Value> = Vec::with_capacity(nb.len());
    for n in nb {
        let row = sqlx::query("SELECT title, artists FROM hub_tracks WHERE id = ?1")
            .bind(n.track_id)
            .fetch_optional(&st.pool)
            .await
            .ok()
            .flatten();
        let (title, artists) = row
            .map(|r| {
                (
                    r.get::<Option<String>, _>("title").unwrap_or_default(),
                    r.get::<Option<String>, _>("artists").unwrap_or_default(),
                )
            })
            .unwrap_or_default();
        data.push(json!({
            "trackId": n.track_id,
            "score": n.score,
            "title": title,
            "artists": artists,
        }));
    }
    Ok(Json(json!({ "data": data })))
}

/// `GET /api/hub/tracks/{id}/ripeness` — on-the-fly ripeness score (issue #214).
async fn track_ripeness(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    if crate::web::current_user(&st, &headers).await.is_none() {
        return Err(err(StatusCode::UNAUTHORIZED, "login required"));
    }
    let e = crate::settings::engine(&st.pool).await;
    let r = crate::scoring::ripeness(&st.pool, id, &e).await;
    let groups: Vec<Value> = r
        .tags_per_group
        .iter()
        .map(|(gid, name, k)| json!({ "groupId": gid, "name": name, "tags": k }))
        .collect();
    let meta: Vec<Value> = r
        .meta_present
        .iter()
        .map(|(n, ok)| json!({ "field": n, "present": ok }))
        .collect();
    Ok(Json(json!({ "data": {
        "trackId": id,
        "total": r.total,
        "tagScore": r.tag_score,
        "metaScore": r.meta_score,
        "traktorScore": r.traktor_score,
        "groups": groups,
        "meta": meta,
    }})))
}

/// Current session user plus their linked service accounts (issue M2-6).
async fn me(State(st): State<AppState>, headers: HeaderMap) -> Result<Json<Value>, ApiError> {
    let Some((user_id, slug)) = crate::web::current_user(&st, &headers).await else {
        return Err(err(StatusCode::UNAUTHORIZED, "login required"));
    };

    let rows = sqlx::query(
        "SELECT service, remote_user_id, display_name, access_token, refresh_token, authorized_at, likes_status\n           FROM hub_service_accounts WHERE user_id = ?1 ORDER BY service",
    )
    .bind(user_id)
    .fetch_all(&st.pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let services: Vec<Value> = rows
        .iter()
        .map(|r| {
            let access: Option<String> = r.get("access_token");
            let refresh: Option<String> = r.get("refresh_token");
            let authorized_at: Option<String> = r.get("authorized_at");
            let likes_status: Option<String> = r.get("likes_status");
            let connected = access.as_deref().map(|a| !a.is_empty()).unwrap_or(false);
            json!({
                "service": r.get::<String, _>("service"),
                "remoteUserId": r.get::<Option<String>, _>("remote_user_id"),
                "displayName": r.get::<Option<String>, _>("display_name"),
                "connected": connected,
                "needsReconnect": needs_reconnect(connected, refresh.as_deref(), authorized_at.as_deref(), likes_status.as_deref()),
                "likesStatus": likes_status,
            })
        })
        .collect();

    Ok(Json(json!({ "data": {
        "id": user_id,
        "slug": slug,
        "services": services,
    } })))
}

/// Queue a fresh Spotify sync for the current user (issues M3-7 #144, M3-8 #138).
///
/// The background worker in `src/worker.rs` does the actual Spotify calls; it
/// picks up accounts with `likes_status = 'queued'` and playlists with
/// `enabled_for_fetch = 1 AND items_available = 0`. We only reset those flags
/// here so the next worker pass re-fetches everything for THIS user.
async fn sync(
    State(st): State<AppState>,
    Path(service): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let Some((user_id, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Err(err(StatusCode::UNAUTHORIZED, "login required"));
    };

    if service != "spotify" {
        return Err(err(StatusCode::BAD_REQUEST, "unsupported service"));
    }

    sqlx::query(
        "UPDATE hub_service_accounts
            SET likes_status = 'queued', likes_synced_at = NULL, likes_error = NULL
          WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(user_id)
    .execute(&st.pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    sqlx::query(
        "UPDATE hub_playlists
            SET enabled_for_fetch = 1, fetch_status = 'queued',
                items_available = 0, fetch_error = NULL
          WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(user_id)
    .execute(&st.pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(json!({ "data": { "started": true } })))
}

/// Query for `GET /api/hub/tag-task`.
#[derive(Deserialize)]
struct TagTaskQuery {
    cadence: Option<String>,
}

/// `GET /api/hub/tag-task?cadence=daily|weekly` — the current period's
/// uncomplete-track queue (issue #346), most-played first, refreshed per
/// period. `cadence` defaults to the `engine_tag_task_cadence` setting.
async fn tag_task(
    State(st): State<AppState>,
    Query(q): Query<TagTaskQuery>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let Some((user_id, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Err(err(StatusCode::UNAUTHORIZED, "login required"));
    };

    let cadence = match q.cadence.as_deref() {
        None | Some("") => {
            let configured =
                crate::settings::get(&st.pool, crate::settings::ENGINE_TAG_TASK_CADENCE).await;
            configured
                .as_deref()
                .and_then(crate::tagtasks::Cadence::parse)
                .unwrap_or(crate::tagtasks::Cadence::Daily)
        }
        Some(raw) => match crate::tagtasks::Cadence::parse(raw) {
            Some(c) => c,
            None => return Err(err(StatusCode::BAD_REQUEST, "invalid cadence")),
        },
    };

    let limit = crate::settings::get(&st.pool, crate::settings::ENGINE_TAG_TASK_LIMIT)
        .await
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(50);

    let task = crate::tagtasks::task_for(&st.pool, user_id, cadence, chrono::Utc::now(), limit)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let rows = sqlx::query(
        "SELECT i.track_id, i.rank, i.plays, i.missing, t.title, t.artists
           FROM hub_tag_task_items i
           LEFT JOIN hub_tracks t ON t.id = i.track_id
          WHERE i.task_id = ?1
          ORDER BY i.rank ASC, i.track_id ASC",
    )
    .bind(task.id)
    .fetch_all(&st.pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            let missing: String = r.get("missing");
            let missing: Vec<String> = missing
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            json!({
                "trackId": r.get::<i64, _>("track_id"),
                "rank": r.get::<i64, _>("rank"),
                "plays": r.get::<i64, _>("plays"),
                "missing": missing,
                "title": r.get::<Option<String>, _>("title"),
                "artists": r.get::<Option<String>, _>("artists"),
            })
        })
        .collect();

    Ok(Json(json!({ "data": {
        "id": task.id,
        "cadence": task.cadence.as_str(),
        "periodStart": task.period_start,
        "generatedAt": task.generated_at,
        "items": items,
    }})))
}

/// A linked account needs re-auth when its refresh token is gone or the
/// authorization is older than Spotify's ~6-month refresh-token lifetime
/// (issue M3-8). A recorded likes error is treated as a soft signal too.
fn needs_reconnect(
    connected: bool,
    refresh_token: Option<&str>,
    authorized_at: Option<&str>,
    likes_status: Option<&str>,
) -> bool {
    if !connected {
        return false;
    }
    if refresh_token.map(|r| r.is_empty()).unwrap_or(true) {
        return true;
    }
    if likes_status == Some("error") {
        return true;
    }
    const SIX_MONTHS_SECS: i64 = 180 * 24 * 60 * 60;
    if let Some(at) = authorized_at {
        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(at) {
            return chrono::Utc::now().signed_duration_since(dt).num_seconds() > SIX_MONTHS_SECS;
        }
    }
    false
}

async fn users(State(st): State<AppState>) -> Result<Json<Value>, ApiError> {
    let rows = sqlx::query("SELECT id, slug, display_name FROM hub_users ORDER BY id")
        .fetch_all(&st.pool)
        .await
        .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let users: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.get::<i64, _>("id"),
                "slug": r.get::<Option<String>, _>("slug"),
                "displayName": r.get::<Option<String>, _>("display_name"),
            })
        })
        .collect();
    Ok(Json(json!({ "data": users })))
}

async fn track(State(st): State<AppState>, Path(id): Path<i64>) -> Result<Json<Value>, ApiError> {
    let row = sqlx::query(
        "SELECT id, service, service_track_id, title, artists, album, duration_ms, isrc
           FROM hub_tracks WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    let Some(row) = row else {
        return Err(err(StatusCode::NOT_FOUND, "track not found"));
    };

    let presence = sqlx::query(
        "SELECT p.user_id, u.slug, u.display_name, p.source, p.playlist_id, p.playlist_name, hp.is_owned\n           FROM hub_v_track_presence p\n           JOIN hub_users u ON u.id = p.user_id\n           LEFT JOIN hub_playlists hp ON hp.id = p.playlist_id\n          WHERE p.track_id = ?1\n          ORDER BY u.slug, hp.is_owned DESC, p.playlist_name",
    )
    .bind(id)
    .fetch_all(&st.pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let presence: Vec<Value> = presence
        .iter()
        .map(|r| {
            json!({
                "userId": r.get::<i64, _>("user_id"),
                "user": r.get::<Option<String>, _>("slug"),
                "displayName": r.get::<Option<String>, _>("display_name"),
                "source": r.get::<Option<String>, _>("source"),
                "playlistId": r.get::<Option<i64>, _>("playlist_id"),
                "playlistName": r.get::<Option<String>, _>("playlist_name"),
                "isOwned": r.get::<Option<i64>, _>("is_owned").map(|v| v == 1),
            })
        })
        .collect();

    Ok(Json(json!({
        "data": {
            "track": {
                "id": row.get::<i64, _>("id"),
                "service": row.get::<String, _>("service"),
                "serviceTrackId": row.get::<String, _>("service_track_id"),
                "title": row.get::<Option<String>, _>("title"),
                "artists": row.get::<Option<String>, _>("artists"),
                "album": row.get::<Option<String>, _>("album"),
                "durationMs": row.get::<Option<i64>, _>("duration_ms"),
                "isrc": row.get::<Option<String>, _>("isrc"),
            },
            "presence": presence,
        }
    })))
}

async fn overlap(State(st): State<AppState>) -> Result<Json<Value>, ApiError> {
    let shared = sqlx::query(
        "SELECT s.track_id, t.title, t.artists, s.user_count, s.user_ids
           FROM hub_v_shared_tracks s
           JOIN hub_tracks t ON t.id = s.track_id
          ORDER BY s.user_count DESC, t.artists, t.title",
    )
    .fetch_all(&st.pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let shared: Vec<Value> = shared
        .iter()
        .map(|r| {
            json!({
                "trackId": r.get::<i64, _>("track_id"),
                "title": r.get::<Option<String>, _>("title"),
                "artists": r.get::<Option<String>, _>("artists"),
                "userCount": r.get::<i64, _>("user_count"),
                "userIds": r.get::<Option<String>, _>("user_ids"),
            })
        })
        .collect();

    let pairs = sqlx::query(
        "SELECT o.user_a_id, ua.slug AS a, o.user_b_id, ub.slug AS b, o.shared_tracks
           FROM hub_v_user_overlap o
           JOIN hub_users ua ON ua.id = o.user_a_id
           JOIN hub_users ub ON ub.id = o.user_b_id
          ORDER BY o.shared_tracks DESC",
    )
    .fetch_all(&st.pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let pairs: Vec<Value> = pairs
        .iter()
        .map(|r| {
            json!({
                "userA": r.get::<Option<String>, _>("a"),
                "userB": r.get::<Option<String>, _>("b"),
                "sharedTracks": r.get::<i64, _>("shared_tracks"),
            })
        })
        .collect();

    Ok(Json(
        json!({ "data": { "shared": shared, "pairs": pairs } }),
    ))
}

async fn playlists(State(st): State<AppState>) -> Result<Json<Value>, ApiError> {
    let rows = sqlx::query(
        "SELECT p.id, u.slug AS user, p.name, p.track_count, p.items_available, p.is_liked,
                p.owner_id,
                COALESCE(NULLIF(p.owner_name, ''), CASE WHEN p.is_owned = 1 THEN a.display_name END) AS owner_name
           FROM hub_playlists p
           JOIN hub_users u ON u.id = p.user_id
           LEFT JOIN hub_service_accounts a ON a.user_id = p.user_id AND a.service = 'spotify'
          ORDER BY u.slug, p.name",
    )
    .fetch_all(&st.pool)
    .await
    .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items: Vec<Value> = rows
        .iter()
        .map(|r| {
            json!({
                "id": r.get::<i64, _>("id"),
                "user": r.get::<Option<String>, _>("user"),
                "name": r.get::<Option<String>, _>("name"),
                "trackCount": r.get::<Option<i64>, _>("track_count"),
                "itemsAvailable": r.get::<i64, _>("items_available") != 0,
                "ownerId": r.get::<Option<String>, _>("owner_id"),
                "ownerName": r.get::<Option<String>, _>("owner_name"),
            })
        })
        .collect();
    Ok(Json(json!({ "data": items })))
}

#[derive(Deserialize)]
struct QueryRequest {
    sql: String,
}

async fn query(
    State(st): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<QueryRequest>,
) -> Result<Json<Value>, ApiError> {
    if crate::web::current_user(&st, &headers).await.is_none() {
        return Err(err(StatusCode::UNAUTHORIZED, "login required"));
    }
    match run_readonly_query(&st.ro_pool, &req.sql).await {
        Ok((columns, rows)) => Ok(Json(
            json!({ "data": { "columns": columns, "rows": rows } }),
        )),
        Err(e) => Err(err(StatusCode::BAD_REQUEST, e.to_string())),
    }
}

/// Execute a read-only `SELECT`/`WITH` statement, returning column names + rows.
pub async fn run_readonly_query(
    pool: &SqlitePool,
    sql: &str,
) -> anyhow::Result<(Vec<String>, Vec<Value>)> {
    let trimmed = sql.trim().trim_end_matches(';').trim();
    let head = trimmed
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    if head != "SELECT" && head != "WITH" {
        anyhow::bail!("only SELECT/WITH statements are allowed");
    }

    let rows = sqlx::query(trimmed).fetch_all(pool).await?;
    let columns: Vec<String> = rows
        .first()
        .map(|r| r.columns().iter().map(|c| c.name().to_string()).collect())
        .unwrap_or_default();

    let json_rows: Vec<Value> = rows
        .iter()
        .map(|r| {
            let mut obj = serde_json::Map::new();
            for (i, col) in r.columns().iter().enumerate() {
                obj.insert(col.name().to_string(), cell_to_json(r, i));
            }
            Value::Object(obj)
        })
        .collect();

    Ok((columns, json_rows))
}

/// Best-effort SQLite cell → JSON (try TEXT, then INTEGER, then REAL).
fn cell_to_json(row: &sqlx::sqlite::SqliteRow, i: usize) -> Value {
    if let Ok(v) = row.try_get::<Option<String>, _>(i) {
        return v.map(Value::String).unwrap_or(Value::Null);
    }
    if let Ok(v) = row.try_get::<Option<i64>, _>(i) {
        return v.map(Value::from).unwrap_or(Value::Null);
    }
    if let Ok(v) = row.try_get::<Option<f64>, _>(i) {
        return v.map(Value::from).unwrap_or(Value::Null);
    }
    Value::Null
}
