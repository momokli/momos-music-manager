//! HTTP surface: orders, per-ISRC status, and file delivery.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tokio_util::io::ReaderStream;

use crate::models::{CreateOrderRequest, MetadataPushRequest};
use crate::{db, AppState};

/// Public liveness probe (no auth).
pub async fn health() -> impl IntoResponse {
    Json(json!({
        "status": "ok",
        "version": crate::VERSION,
        "gitSha": crate::GIT_SHA,
        "describe": crate::GIT_DESCRIBE
    }))
}

/// All protected routes. The auth layer is applied by the caller.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/orders", post(create_order).get(list_orders))
        .route("/orders/{id}", get(get_order))
        .route("/orders/{id}/cancel", post(cancel_order))
        .route("/orders/{id}/priority", post(set_priority))
        .route("/isrc/{isrc}", get(get_isrc))
        .route("/isrc/{isrc}/retry", post(retry_isrc))
        .route("/isrc/{isrc}/{format}", get(get_file))
        // Bulk metadata/state for many ISRCs at once (hub enrichment).
        .route("/tracks", get(get_tracks))
        // Live deemix queue + recent service events + worker control.
        .route("/queue", get(get_queue))
        .route("/logs", get(get_logs))
        .route("/worker", get(worker_status))
        .route("/worker/pause", post(worker_pause))
        .route("/worker/resume", post(worker_resume))
        // Per-ISRC metadata (BPM/key authority).
        .route("/metadata", post(post_metadata).get(get_metadata_range))
        .route("/metadata/{isrc}", get(get_metadata_one))
        // Data Vault 2.0 consumer surface: resolved BPM/key by precedence.
        .route("/vault/tracks", get(get_vault_tracks))
        .route("/vault/track/{isrc}", get(get_vault_track))
        // Content-addressed object store (Backpack backup / file home).
        .route("/objects", get(crate::store::list))
        .route("/objects/check", post(crate::store::check))
        .route(
            "/objects/{hash}",
            put(crate::store::put)
                .head(crate::store::head)
                .get(crate::store::get),
        )
}

fn error(code: StatusCode, msg: impl Into<String>) -> Response {
    (code, Json(json!({ "error": msg.into() }))).into_response()
}

/// `POST /orders` — place an order for a batch of ISRCs.
async fn create_order(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateOrderRequest>,
) -> Response {
    let mut isrcs: Vec<String> = req.items.iter().filter_map(|i| i.key()).collect();
    isrcs.sort();
    isrcs.dedup();

    if isrcs.is_empty() {
        return error(StatusCode::BAD_REQUEST, "no ISRCs in order");
    }

    let order_id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = db::create_order(&state.pool, &order_id, &isrcs, req.priority).await {
        tracing::error!("create_order failed: {e:#}");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to create order");
    }

    // Wake the worker so a fresh order starts immediately instead of on the
    // next tick.
    state.notify.notify_one();
    state.logs.push(
        "info",
        format!(
            "order {order_id}: {} ISRC(s) (prio {})",
            isrcs.len(),
            req.priority
        ),
    );

    Json(json!({ "orderId": order_id, "status": "open", "count": isrcs.len() })).into_response()
}

/// `POST /orders/{id}/cancel` — mark an order cancelled and drop its item links.
async fn cancel_order(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    match db::cancel_order(&state.pool, &id).await {
        Ok(true) => {
            state.logs.push("warn", format!("order {id}: cancelled"));
            Json(json!({ "orderId": id, "status": "cancelled" })).into_response()
        }
        Ok(false) => error(StatusCode::NOT_FOUND, "order not found"),
        Err(e) => {
            tracing::error!("cancel_order failed: {e:#}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "failed to cancel order")
        }
    }
}

#[derive(Debug, Deserialize)]
struct PriorityBody {
    priority: i64,
}

/// `POST /orders/{id}/priority` — set an order's priority.
async fn set_priority(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<PriorityBody>,
) -> Response {
    match db::set_order_priority(&state.pool, &id, body.priority).await {
        Ok(true) => {
            state.notify.notify_one();
            Json(json!({ "orderId": id, "priority": body.priority })).into_response()
        }
        Ok(false) => error(StatusCode::NOT_FOUND, "order not found"),
        Err(e) => {
            tracing::error!("set_order_priority failed: {e:#}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "failed to set priority")
        }
    }
}

/// `POST /isrc/{isrc}/retry` — reset a non-ready track to pending.
async fn retry_isrc(State(state): State<Arc<AppState>>, Path(isrc): Path<String>) -> Response {
    let isrc = db::normalize_key(&isrc);
    match db::reset_track_to_pending(&state.pool, &isrc).await {
        Ok(true) => {
            state.logs.push("info", format!("{isrc}: retry queued"));
            state.notify.notify_one();
            Json(json!({ "isrc": isrc, "state": "pending" })).into_response()
        }
        Ok(false) => error(
            StatusCode::CONFLICT,
            "nothing to retry (unknown ISRC or already ready)",
        ),
        Err(e) => {
            tracing::error!("retry_isrc failed: {e:#}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "failed to retry")
        }
    }
}

/// `GET /worker` — worker pause state plus the retry cap and cadence.
async fn worker_status(State(state): State<Arc<AppState>>) -> Response {
    use std::sync::atomic::Ordering;
    Json(json!({
        "paused": state.paused.load(Ordering::Relaxed),
        "intervalSecs": state.config.worker_interval.as_secs(),
        "maxAttempts": state.config.queue_max_attempts,
    }))
    .into_response()
}

/// `POST /worker/pause` — stop the worker from picking up new work.
async fn worker_pause(State(state): State<Arc<AppState>>) -> Response {
    use std::sync::atomic::Ordering;
    state.paused.store(true, Ordering::Relaxed);
    state.logs.push("warn", "worker paused");
    Json(json!({ "paused": true })).into_response()
}

/// `POST /worker/resume` — resume the worker.
async fn worker_resume(State(state): State<Arc<AppState>>) -> Response {
    use std::sync::atomic::Ordering;
    state.paused.store(false, Ordering::Relaxed);
    state.logs.push("info", "worker resumed");
    state.notify.notify_one();
    Json(json!({ "paused": false })).into_response()
}

#[derive(Debug, Deserialize)]
struct TracksQuery {
    /// Comma-separated ISRCs (bounded).
    isrcs: Option<String>,
    limit: Option<usize>,
}

/// `GET /tracks?isrcs=a,b,c` — bulk metadata + state + delivered formats + object
/// presence for many ISRCs in one call. Unknown ISRCs come back with
/// `"unknown": true` (never omitted) so indexes stay aligned.
async fn get_tracks(State(state): State<Arc<AppState>>, Query(q): Query<TracksQuery>) -> Response {
    let limit = q.limit.unwrap_or(200).clamp(1, 500);
    let raw = q.isrcs.unwrap_or_default();
    let mut isrcs: Vec<String> = raw
        .split(',')
        .map(db::normalize_key)
        .filter(|s| !s.is_empty())
        .collect();
    isrcs.sort();
    isrcs.dedup();
    isrcs.truncate(limit);

    let mut out: Vec<serde_json::Value> = Vec::with_capacity(isrcs.len());
    for isrc in &isrcs {
        match db::get_track(&state.pool, isrc).await {
            Ok(Some(t)) => {
                let objects: Vec<serde_json::Value> = sqlx::query_as::<_, (String, i64)>(
                    "SELECT hash, size FROM store_objects WHERE isrc = ?1 ORDER BY created_at DESC",
                )
                .bind(isrc)
                .fetch_all(&state.pool)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(hash, size)| json!({ "hash": hash, "size": size }))
                .collect();
                out.push(json!({
                    "isrc": t.isrc,
                    "state": t.state,
                    "deezerId": t.deezer_id,
                    "title": t.title,
                    "artist": t.artist,
                    "album": t.album,
                    "sourceFormat": t.source_format,
                    "formats": t.formats(),
                    "error": t.error,
                    "objects": objects,
                }));
            }
            _ => out.push(json!({ "isrc": isrc, "unknown": true })),
        }
    }
    Json(json!({ "tracks": out })).into_response()
}

/// `GET /queue` — the live deemix queue plus per-state ISRC counts.
async fn get_queue(State(state): State<Arc<AppState>>) -> Response {
    let counts = db::queue_counts(&state.pool).await.unwrap_or_default();
    match crate::deemix::queue(&state.http, &state.config.deemix_url).await {
        Ok(q) => {
            let items: Vec<serde_json::Value> = q
                .into_iter()
                .map(|(id, it)| {
                    json!({
                        "id": id,
                        "title": it.title,
                        "status": it.status,
                        "progress": it.progress,
                    })
                })
                .collect();
            Json(json!({ "items": items, "counts": counts })).into_response()
        }
        Err(e) => {
            Json(json!({ "items": [], "counts": counts, "error": e.to_string() })).into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct LogsQuery {
    limit: Option<usize>,
}

/// `GET /logs?limit=n` — recent service events, newest first.
async fn get_logs(State(state): State<Arc<AppState>>, Query(q): Query<LogsQuery>) -> Response {
    let limit = q.limit.unwrap_or(100).clamp(1, 500);
    Json(json!({ "logs": state.logs.recent(limit) })).into_response()
}

// ── Per-ISRC metadata (BPM/key authority) ────────────────────────────────────

/// `POST /metadata` — bulk upsert ISRC → {bpm, key, camelot, …}.
async fn post_metadata(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MetadataPushRequest>,
) -> Response {
    match db::upsert_metadata(&state.pool, &req.items).await {
        Ok(n) => {
            // Mirror the push into the vault (analysis + metadata satellites).
            // Best-effort: the legacy table above is the compat read surface.
            if let Err(e) = crate::vault::ingest_metadata(&state.pool, &req.items).await {
                tracing::warn!("vault ingest_metadata failed: {e:#}");
            }
            Json(json!({ "upserted": n })).into_response()
        }
        Err(e) => {
            tracing::error!("upsert_metadata failed: {e:#}");
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to store metadata",
            )
        }
    }
}

#[derive(Debug, Deserialize)]
struct MetadataQuery {
    /// Comma-separated ISRCs (range lookup).
    isrcs: Option<String>,
    limit: Option<usize>,
}

/// `GET /metadata?isrcs=a,b,c` — metadata for a range of ISRCs. Unknown ISRCs
/// come back in `missing` so callers can tell "no data" from "not requested".
async fn get_metadata_range(
    State(state): State<Arc<AppState>>,
    Query(q): Query<MetadataQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(1000).clamp(1, 2000);
    let mut isrcs: Vec<String> = q
        .isrcs
        .unwrap_or_default()
        .split(',')
        .map(db::normalize_key)
        .filter(|s| !s.is_empty())
        .collect();
    isrcs.sort();
    isrcs.dedup();
    isrcs.truncate(limit);
    let items = db::metadata_bulk(&state.pool, &isrcs)
        .await
        .unwrap_or_default();
    let found: std::collections::HashSet<&str> = items.iter().map(|m| m.isrc.as_str()).collect();
    let missing: Vec<&String> = isrcs
        .iter()
        .filter(|i| !found.contains(i.as_str()))
        .collect();
    Json(json!({ "items": items, "missing": missing })).into_response()
}

/// `GET /metadata/{isrc}` — metadata for a single ISRC.
async fn get_metadata_one(
    State(state): State<Arc<AppState>>,
    Path(isrc): Path<String>,
) -> Response {
    match db::get_metadata(&state.pool, &isrc).await {
        Ok(Some(m)) => Json(m).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "unknown ISRC"),
        Err(e) => {
            tracing::error!("get_metadata failed: {e:#}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "failed to read metadata")
        }
    }
}

// ── Data Vault 2.0 consumer surface ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct VaultQuery {
    /// Comma-separated ISRCs (range lookup).
    isrcs: Option<String>,
    limit: Option<usize>,
}

/// `GET /vault/tracks?isrcs=a,b,c` — the resolved analysis for many ISRCs. One
/// row per **known** ISRC (unknown ISRCs are omitted); BPM/key/camelot are picked
/// by precedence over the latest per-analyzer satellite rows.
async fn get_vault_tracks(
    State(state): State<Arc<AppState>>,
    Query(q): Query<VaultQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(1000).clamp(1, 2000);
    let mut isrcs: Vec<String> = q
        .isrcs
        .unwrap_or_default()
        .split(',')
        .map(db::normalize_key)
        .filter(|s| !s.is_empty())
        .collect();
    isrcs.sort();
    isrcs.dedup();
    isrcs.truncate(limit);
    match crate::vault::resolve_tracks(&state.pool, &isrcs, &state.config.analysis_precedence).await
    {
        Ok(tracks) => Json(json!({ "tracks": tracks })).into_response(),
        Err(e) => {
            tracing::error!("resolve_tracks failed: {e:#}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "failed to read vault")
        }
    }
}

/// `GET /vault/track/{isrc}` — the resolved analysis for one ISRC (`404` if the
/// ISRC is unknown to the vault).
async fn get_vault_track(State(state): State<Arc<AppState>>, Path(isrc): Path<String>) -> Response {
    match crate::vault::resolve_track(&state.pool, &isrc, &state.config.analysis_precedence).await {
        Ok(Some(t)) => Json(t).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "unknown ISRC"),
        Err(e) => {
            tracing::error!("resolve_track failed: {e:#}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "failed to read vault")
        }
    }
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    status: Option<String>,
    limit: Option<i64>,
}

/// `GET /orders` — list orders, optionally filtered by status.
async fn list_orders(State(state): State<Arc<AppState>>, Query(q): Query<ListQuery>) -> Response {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    match db::list_orders(&state.pool, q.status.as_deref(), limit).await {
        Ok(orders) => Json(json!({ "orders": orders })).into_response(),
        Err(e) => {
            tracing::error!("list_orders failed: {e:#}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "failed to list orders")
        }
    }
}

/// `GET /orders/{id}` — order status plus per-ISRC state.
async fn get_order(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let order = match db::get_order(&state.pool, &id).await {
        Ok(Some(o)) => o,
        Ok(None) => return error(StatusCode::NOT_FOUND, "order not found"),
        Err(e) => {
            tracing::error!("get_order failed: {e:#}");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to read order");
        }
    };
    let items = match db::order_items(&state.pool, &id).await {
        Ok(i) => i,
        Err(e) => {
            tracing::error!("order_items failed: {e:#}");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to read order items",
            );
        }
    };
    Json(json!({
        "orderId": order.id,
        "status": order.status,
        "priority": order.priority,
        "createdAt": order.created_at,
        "updatedAt": order.updated_at,
        "items": items,
    }))
    .into_response()
}

/// `GET /isrc/{isrc}` — current state of a single ISRC.
async fn get_isrc(State(state): State<Arc<AppState>>, Path(isrc): Path<String>) -> Response {
    let isrc = db::normalize_key(&isrc);
    match db::get_track(&state.pool, &isrc).await {
        Ok(Some(track)) => Json(json!({
            "isrc": track.isrc,
            "state": track.state,
            "deezerId": track.deezer_id,
            "title": track.title,
            "artist": track.artist,
            "album": track.album,
            "sourceFormat": track.source_format,
            "formats": track.formats(),
            "error": track.error,
        }))
        .into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "unknown ISRC"),
        Err(e) => {
            tracing::error!("get_isrc failed: {e:#}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "failed to read track")
        }
    }
}

/// `GET /isrc/{isrc}/{flac|320|128}` — stream the delivered file.
async fn get_file(
    State(state): State<Arc<AppState>>,
    Path((isrc, format)): Path<(String, String)>,
) -> Response {
    let isrc = db::normalize_key(&isrc);
    let format = format.to_lowercase();
    if !matches!(format.as_str(), "flac" | "320" | "128") {
        return error(StatusCode::BAD_REQUEST, "format must be flac, 320 or 128");
    }

    let track = match db::get_track(&state.pool, &isrc).await {
        Ok(Some(t)) => t,
        Ok(None) => return error(StatusCode::NOT_FOUND, "unknown ISRC"),
        Err(e) => {
            tracing::error!("get_file failed: {e:#}");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to read track");
        }
    };

    let Some(path) = track.path_for(&format) else {
        return error(
            StatusCode::NOT_FOUND,
            format!("no {format} for this ISRC (state: {})", track.state),
        );
    };

    let file = match tokio::fs::File::open(path).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("open {path} failed: {e:#}");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "file missing on disk");
        }
    };

    let content_type = if format == "flac" {
        "audio/flac"
    } else {
        "audio/mpeg"
    };

    let stream = ReaderStream::new(file);
    let mut resp = Body::from_stream(stream).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static(content_type),
    );
    resp
}
