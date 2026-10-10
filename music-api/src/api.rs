//! HTTP surface: orders, per-ISRC status, and file delivery.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tokio_util::io::ReaderStream;

use crate::models::CreateOrderRequest;
use crate::{AppState, db};

/// Public liveness probe (no auth).
pub async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

/// All protected routes. The auth layer is applied by the caller.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/orders", post(create_order).get(list_orders))
        .route("/orders/{id}", get(get_order))
        .route("/isrc/{isrc}", get(get_isrc))
        .route("/isrc/{isrc}/{format}", get(get_file))
        // Bulk metadata/state for many ISRCs at once (hub enrichment).
        .route("/tracks", get(get_tracks))
        // Live deemix queue + recent service events.
        .route("/queue", get(get_queue))
        .route("/logs", get(get_logs))
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
    let mut isrcs: Vec<String> = req
        .items
        .iter()
        .map(|i| db::normalize_isrc(&i.isrc))
        .filter(|s| !s.is_empty())
        .collect();
    isrcs.sort();
    isrcs.dedup();

    if isrcs.is_empty() {
        return error(StatusCode::BAD_REQUEST, "no ISRCs in order");
    }

    let order_id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = db::create_order(&state.pool, &order_id, &isrcs).await {
        tracing::error!("create_order failed: {e:#}");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to create order");
    }

    // Wake the worker so a fresh order starts immediately instead of on the
    // next tick.
    state.notify.notify_one();
    state
        .logs
        .push("info", format!("order {order_id}: {} ISRC(s)", isrcs.len()));

    Json(json!({ "orderId": order_id, "status": "open", "count": isrcs.len() })).into_response()
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
        .map(db::normalize_isrc)
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

/// `GET /queue` — the live deemix queue (bounded to what deemix returns).
async fn get_queue(State(state): State<Arc<AppState>>) -> Response {
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
            Json(json!({ "items": items })).into_response()
        }
        Err(e) => Json(json!({ "items": [], "error": e.to_string() })).into_response(),
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
        "createdAt": order.created_at,
        "updatedAt": order.updated_at,
        "items": items,
    }))
    .into_response()
}

/// `GET /isrc/{isrc}` — current state of a single ISRC.
async fn get_isrc(State(state): State<Arc<AppState>>, Path(isrc): Path<String>) -> Response {
    let isrc = db::normalize_isrc(&isrc);
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
    let isrc = db::normalize_isrc(&isrc);
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
