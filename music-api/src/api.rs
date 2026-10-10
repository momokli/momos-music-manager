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

use crate::models::{CreateOrderRequest, CreateUrlOrderRequest};
use crate::{AppState, db, ytdlp};

/// Public liveness probe (no auth).
pub async fn health() -> impl IntoResponse {
    Json(json!({ "status": "ok" }))
}

/// All protected routes. The auth layer is applied by the caller.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/orders", post(create_order).get(list_orders))
        .route("/orders/{id}", get(get_order))
        .route("/orders/{id}/prioritize", post(prioritize_order))
        .route("/orders/{id}/status", post(set_order_status))
        .route("/isrc/{isrc}", get(get_isrc))
        .route("/isrc/{isrc}/{format}", get(get_file))
        // URL-based orders (YouTube / SoundCloud via yt-dlp).
        .route("/url-orders", post(create_url_order).get(list_url_orders))
        .route("/url-orders/{id}", get(get_url_order))
        .route("/url-orders/{id}/prioritize", post(prioritize_url_order))
        .route("/url-orders/{id}/status", post(set_url_order_status))
        .route("/url-tracks/{id}/{format}", get(get_url_file))
        // Provider-agnostic metadata.
        .route("/metadata/track", get(metadata_track))
        .route("/metadata/playlist", get(metadata_playlist))
        .route("/queue", get(get_queue))
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
    if let Err(e) = db::create_order(&state.pool, &order_id, &isrcs, req.priority).await {
        tracing::error!("create_order failed: {e:#}");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to create order");
    }

    // Wake the worker so a fresh order starts immediately instead of on the
    // next tick.
    state.notify.notify_one();

    Json(json!({ "orderId": order_id, "status": "open", "count": isrcs.len() })).into_response()
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

// ── URL-based orders (YouTube / SoundCloud) ──────────────────────────────────

/// `POST /url-orders` — place an order for a batch of URLs.
async fn create_url_order(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateUrlOrderRequest>,
) -> Response {
    let mut urls: Vec<String> = req
        .items
        .iter()
        .map(|i| i.url.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    urls.sort();
    urls.dedup();

    if urls.is_empty() {
        return error(StatusCode::BAD_REQUEST, "no URLs in order");
    }
    for url in &urls {
        if ytdlp::provider_for_url(url) == "unknown" {
            return error(StatusCode::BAD_REQUEST, format!("unsupported URL: {url}"));
        }
    }

    let order_id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = db::create_url_order(&state.pool, &order_id, &urls).await {
        tracing::error!("create_url_order failed: {e:#}");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to create order");
    }
    state.notify.notify_one();
    Json(json!({ "orderId": order_id, "status": "open", "count": urls.len() })).into_response()
}

/// `GET /url-orders` — list URL orders.
async fn list_url_orders(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    match db::list_url_orders(&state.pool, q.status.as_deref(), limit).await {
        Ok(orders) => Json(json!({ "orders": orders })).into_response(),
        Err(e) => {
            tracing::error!("list_url_orders failed: {e:#}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "failed to list orders")
        }
    }
}

/// `GET /url-orders/{id}` — order status plus per-URL state.
async fn get_url_order(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let order = match db::get_url_order(&state.pool, &id).await {
        Ok(Some(o)) => o,
        Ok(None) => return error(StatusCode::NOT_FOUND, "order not found"),
        Err(e) => {
            tracing::error!("get_url_order failed: {e:#}");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to read order");
        }
    };
    let items = match db::url_order_items(&state.pool, &id).await {
        Ok(i) => i,
        Err(e) => {
            tracing::error!("url_order_items failed: {e:#}");
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

#[derive(Debug, Deserialize)]
struct PriorityBody {
    priority: i64,
}

/// `POST /url-orders/{id}/prioritize` — set the order + track priority.
async fn prioritize_url_order(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<PriorityBody>,
) -> Response {
    match db::set_url_order_priority(&state.pool, &id, body.priority).await {
        Ok(()) => Json(json!({ "orderId": id, "priority": body.priority })).into_response(),
        Err(e) => {
            tracing::error!("prioritize_url_order failed: {e:#}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "failed to set priority")
        }
    }
}

#[derive(Debug, Deserialize)]
struct StatusBody {
    status: String,
}

/// `POST /url-orders/{id}/status` — pause/resume/cancel a URL order.
async fn set_url_order_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<StatusBody>,
) -> Response {
    if !matches!(body.status.as_str(), "open" | "paused" | "cancelled") {
        return error(
            StatusCode::BAD_REQUEST,
            "status must be open, paused or cancelled",
        );
    }
    match db::set_url_order_status(&state.pool, &id, &body.status).await {
        Ok(()) => Json(json!({ "orderId": id, "status": body.status })).into_response(),
        Err(e) => {
            tracing::error!("set_url_order_status failed: {e:#}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "failed to set status")
        }
    }
}

/// `GET /url-tracks/{id}/{flac|320|128}` — stream a URL track's file.
async fn get_url_file(
    State(state): State<Arc<AppState>>,
    Path((id, format)): Path<(i64, String)>,
) -> Response {
    let format = format.to_lowercase();
    if !matches!(format.as_str(), "flac" | "320" | "128") {
        return error(StatusCode::BAD_REQUEST, "format must be flac, 320 or 128");
    }
    let track = match db::get_url_track_by_id(&state.pool, id).await {
        Ok(Some(t)) => t,
        Ok(None) => return error(StatusCode::NOT_FOUND, "unknown url track"),
        Err(e) => {
            tracing::error!("get_url_file failed: {e:#}");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to read track");
        }
    };
    let Some(path) = track.path_for(&format) else {
        return error(
            StatusCode::NOT_FOUND,
            format!("no {format} for this track (state: {})", track.state),
        );
    };
    stream_file(path, &format).await
}

async fn stream_file(path: &str, format: &str) -> Response {
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

// ── Metadata ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct UrlQuery {
    url: String,
}

/// `GET /metadata/track?url=...` — provider-agnostic track metadata.
async fn metadata_track(Query(q): Query<UrlQuery>) -> Response {
    match ytdlp::fetch_track(&q.url).await {
        Ok(m) => Json(m).into_response(),
        Err(e) => {
            tracing::warn!("metadata_track failed: {e:#}");
            error(
                StatusCode::BAD_GATEWAY,
                format!("metadata lookup failed: {e}"),
            )
        }
    }
}

/// `GET /metadata/playlist?url=...` — provider-agnostic playlist metadata.
async fn metadata_playlist(Query(q): Query<UrlQuery>) -> Response {
    match ytdlp::fetch_playlist(&q.url).await {
        Ok(m) => Json(m).into_response(),
        Err(e) => {
            tracing::warn!("metadata_playlist failed: {e:#}");
            error(
                StatusCode::BAD_GATEWAY,
                format!("metadata lookup failed: {e}"),
            )
        }
    }
}

// ── Queue control (ISRC orders) ──────────────────────────────────────────────

/// `POST /orders/{id}/prioritize` — set the ISRC order priority.
async fn prioritize_order(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<PriorityBody>,
) -> Response {
    match db::set_order_priority(&state.pool, &id, body.priority).await {
        Ok(()) => Json(json!({ "orderId": id, "priority": body.priority })).into_response(),
        Err(e) => {
            tracing::error!("prioritize_order failed: {e:#}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "failed to set priority")
        }
    }
}

/// `POST /orders/{id}/status` — pause/resume/cancel an ISRC order.
async fn set_order_status(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<StatusBody>,
) -> Response {
    if !matches!(body.status.as_str(), "open" | "paused" | "cancelled") {
        return error(
            StatusCode::BAD_REQUEST,
            "status must be open, paused or cancelled",
        );
    }
    match db::set_order_status(&state.pool, &id, &body.status).await {
        Ok(()) => Json(json!({ "orderId": id, "status": body.status })).into_response(),
        Err(e) => {
            tracing::error!("set_order_status failed: {e:#}");
            error(StatusCode::INTERNAL_SERVER_ERROR, "failed to set status")
        }
    }
}

/// `GET /queue` — current pending/downloading work, highest priority first.
async fn get_queue(State(state): State<Arc<AppState>>) -> Response {
    let isrc_pending = db::pending_tracks(&state.pool, 100)
        .await
        .unwrap_or_default();
    let url_pending = db::pending_url_tracks(&state.pool, 100)
        .await
        .unwrap_or_default();
    Json(json!({
        "isrc": isrc_pending.iter().map(|t| json!({
            "isrc": t.isrc, "state": t.state, "title": t.title, "artist": t.artist,
            "priority": t.priority,
        })).collect::<Vec<_>>(),
        "url": url_pending.iter().map(|t| json!({
            "url": t.url, "provider": t.provider, "state": t.state,
            "title": t.title, "artist": t.artist, "priority": t.priority,
        })).collect::<Vec<_>>(),
    }))
    .into_response()
}
