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
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/hub/health", get(health))
        .route("/api/hub/users", get(users))
        .route("/api/hub/tracks/{id}", get(track))
        .route("/api/hub/tracks/{id}/ripeness", get(track_ripeness))
        .route("/api/hub/overlap", get(overlap))
        .route("/api/hub/similar", get(similar))
        .route("/api/hub/playlists", get(playlists))
        .route("/api/hub/query", post(query))
        .route("/api/hub/me", get(me))
        .route("/api/hub/services/{service}/sync", post(sync))
        .with_state(state.clone())
        .merge(crate::artists::router(state))
}

type ApiError = (StatusCode, Json<Value>);

fn err(code: StatusCode, msg: impl Into<String>) -> ApiError {
    (code, Json(json!({ "error": msg.into() })))
}

async fn health() -> Json<Value> {
    Json(json!({ "data": { "status": "ok", "version": env!("CARGO_PKG_VERSION") } }))
}

#[derive(Deserialize)]
struct SimilarQuery {
    track: i64,
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
