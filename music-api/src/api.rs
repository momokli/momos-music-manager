//! HTTP surface: orders, per-ISRC status, and file delivery.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use tokio_util::io::ReaderStream;

use crate::models::{CreateArtistOrderRequest, CreateOrderRequest, MetadataPushRequest};
use crate::{db, work, worker, AppState};

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
        // Whole-playlist download: one request → one ZIP of the order (#358).
        .route("/orders/{id}/archive", get(get_order_archive))
        // Artist orders: download a whole Deezer artist's discography.
        .route(
            "/artist-orders",
            post(create_artist_order).get(list_artist_orders),
        )
        .route("/artist-orders/{id}", get(get_artist_order))
        .route("/isrc/{isrc}", get(get_isrc))
        .route("/isrc/{isrc}/objects", get(get_isrc_objects))
        .route("/isrc/{isrc}/retry", post(retry_isrc))
        .route("/isrc/{isrc}/{format}", get(get_file))
        // Bulk metadata/state for many ISRCs at once (hub enrichment).
        .route("/tracks", get(get_tracks))
        // Live deemix queue + recent service events + worker control.
        .route("/queue", get(get_queue))
        .route("/logs", get(get_logs))
        // Deezer account tier behind the configured ARL (protected).
        .route("/account", get(get_account))
        .route("/worker", get(worker_status))
        .route("/worker/pause", post(worker_pause))
        .route("/worker/resume", post(worker_resume))
        // Per-ISRC metadata (BPM/key authority).
        .route("/metadata", post(post_metadata).get(get_metadata_range))
        .route("/metadata/{isrc}", get(get_metadata_one))
        // Data Vault 2.0 consumer surface: resolved BPM/key by precedence.
        .route("/vault/tracks", get(get_vault_tracks))
        .route("/vault/track/{isrc}", get(get_vault_track))
        // The unified priority work-queue (download/transcode/analyze/crawl:*).
        .route("/work", post(create_work).get(get_work))
        // Content-addressed object store (Backpack backup / file home).
        .route("/objects", get(crate::store::list))
        .route("/objects/check", post(crate::store::check))
        .route("/objects/prune", post(crate::store::prune))
        .route("/objects/prune/orphans", post(crate::store::prune_orphans))
        .route("/objects/stats", get(crate::store::stats))
        .route(
            "/objects/{hash}",
            put(crate::store::put)
                .head(crate::store::head)
                .get(crate::store::get)
                .delete(crate::store::delete),
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

/// `POST /artist-orders` — order a whole Deezer artist's discography.
async fn create_artist_order(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateArtistOrderRequest>,
) -> Response {
    let Some(artist_url) = crate::deezer::normalize_artist_url(&req.artist) else {
        return error(
            StatusCode::BAD_REQUEST,
            "artist must be a Deezer artist id or URL",
        );
    };

    let order_id = uuid::Uuid::new_v4().to_string();
    if let Err(e) = db::create_artist_order(
        &state.pool,
        &order_id,
        req.artist.trim(),
        &artist_url,
        req.priority,
    )
    .await
    {
        tracing::error!("create_artist_order failed: {e:#}");
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "failed to create artist order",
        );
    }

    // Best-effort submit; the worker retries while the order stays `open`.
    let deemix_ready = worker::ensure_deemix_login(&state).await;
    let status = worker::submit_artist_order(&state, &order_id, &artist_url, deemix_ready).await;

    state.notify.notify_one();
    state.logs.push(
        "info",
        format!(
            "artist order {order_id}: {artist_url} (prio {})",
            req.priority
        ),
    );

    Json(json!({ "orderId": order_id, "status": status, "artist": artist_url })).into_response()
}

/// `POST /work` — enqueue a work item (raise/re-arm on conflict). Generic so any
/// producer (the hub, the spider) can seed any kind.
async fn create_work(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateWorkRequest>,
) -> Response {
    let kind = req.kind.trim().to_lowercase();
    let reference = req.reference.trim();
    if kind.is_empty() || reference.is_empty() {
        return error(StatusCode::BAD_REQUEST, "kind and ref are required");
    }
    if let Err(e) = work::enqueue(
        &state.pool,
        &kind,
        reference,
        req.priority,
        req.record_source.as_deref(),
    )
    .await
    {
        tracing::error!("create_work failed: {e:#}");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to enqueue work");
    }
    state.notify.notify_one();
    Json(json!({ "kind": kind, "ref": reference, "priority": req.priority })).into_response()
}

/// `GET /work` — queue overview: `(kind, state)` counts plus the top items.
async fn get_work(State(state): State<Arc<AppState>>, Query(q): Query<WorkQuery>) -> Response {
    let counts = work::counts(&state.pool).await.unwrap_or_default();
    let items = work::items(&state.pool, q.kind.as_deref(), q.limit.unwrap_or(50))
        .await
        .unwrap_or_default();
    Json(json!({ "counts": counts, "items": items })).into_response()
}

/// `POST /work` body. `ref`/`refId` are both accepted.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateWorkRequest {
    /// e.g. `download`, `analyze`, `crawl:deezer`.
    kind: String,
    /// What the work acts on.
    #[serde(rename = "ref", alias = "refId")]
    reference: String,
    /// Higher runs first (default 0).
    #[serde(default)]
    priority: i64,
    /// Which source requested it (free-form; stored for provenance).
    #[serde(default)]
    record_source: Option<String>,
}

/// `GET /work` query.
#[derive(Debug, Deserialize)]
struct WorkQuery {
    kind: Option<String>,
    limit: Option<i64>,
}

/// `GET /artist-orders` — list artist orders, optionally filtered by status.
async fn list_artist_orders(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListQuery>,
) -> Response {
    let limit = q.limit.unwrap_or(50).clamp(1, 500);
    match db::list_artist_orders(&state.pool, q.status.as_deref(), limit).await {
        Ok(orders) => Json(json!({ "orders": orders })).into_response(),
        Err(e) => {
            tracing::error!("list_artist_orders failed: {e:#}");
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to list artist orders",
            )
        }
    }
}

/// `GET /artist-orders/{id}` — artist order status plus the ingested ISRCs.
async fn get_artist_order(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let order = match db::get_artist_order(&state.pool, &id).await {
        Ok(Some(o)) => o,
        Ok(None) => return error(StatusCode::NOT_FOUND, "artist order not found"),
        Err(e) => {
            tracing::error!("get_artist_order failed: {e:#}");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to read artist order",
            );
        }
    };
    let items = db::artist_order_items(&state.pool, &id)
        .await
        .unwrap_or_default();
    Json(json!({
        "orderId": order.id,
        "artist": order.artist,
        "artistUrl": order.artist_url,
        "status": order.status,
        "priority": order.priority,
        "added": order.added,
        "error": order.error,
        "createdAt": order.created_at,
        "updatedAt": order.updated_at,
        "items": items,
    }))
    .into_response()
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
        "maxInflight": state.config.max_inflight,
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
                let objects: Vec<serde_json::Value> = sqlx::query_as::<
                    _,
                    (String, i64, Option<String>, Option<String>, Option<String>),
                >(
                    "SELECT hash, size, name, original_path, stem_type FROM store_objects WHERE isrc = ?1 ORDER BY created_at DESC",
                )
                .bind(isrc)
                .fetch_all(&state.pool)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(hash, size, name, original_path, stem_type)| {
                    let kind = crate::store::classify_kind(
                        stem_type.as_deref(),
                        name.as_deref(),
                        original_path.as_deref(),
                    );
                    json!({ "hash": hash, "size": size, "kind": kind, "stemType": stem_type })
                })
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

/// `GET /account` — the Deezer account tier behind the configured ARL.
///
/// Protected route. Serves the cached tier when available, otherwise probes
/// gw-light on demand (bounded timeout) and caches the result.
async fn get_account(State(state): State<Arc<AppState>>) -> Response {
    let arl = state.config.deemix_arl.trim();
    if arl.is_empty() {
        return error(StatusCode::CONFLICT, "DEEMIX_ARL not configured");
    }

    if let Some(tier) = state.arl_tier.read().await.clone() {
        return account_response(&tier);
    }

    match crate::deezer::fetch_arl_tier(&state.http, &state.config.deezer_gw_base, arl).await {
        Ok(Some(tier)) => {
            *state.arl_tier.write().await = Some(tier.clone());
            account_response(&tier)
        }
        Ok(None) => error(StatusCode::BAD_GATEWAY, "Deezer account data unavailable"),
        Err(e) => error(
            StatusCode::BAD_GATEWAY,
            format!("deezer tier lookup failed: {e}"),
        ),
    }
}

fn account_response(tier: &crate::deezer::ArlTier) -> Response {
    Json(json!({
        "offer": tier.offer,
        "lossless": tier.lossless,
        "high": tier.high,
        "low": tier.low,
        "userId": tier.user_id,
        "canStreamFlac": tier.can_stream_flac(),
    }))
    .into_response()
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

/// `GET /vault/track/{isrc}` — the **atomic** view of one track: hub, links and
/// every satellite, transparently. `404` if the ISRC is unknown to the vault.
async fn get_vault_track(State(state): State<Arc<AppState>>, Path(isrc): Path<String>) -> Response {
    match crate::vault::track_detail(&state.pool, &isrc, &state.config.analysis_precedence).await {
        Ok(Some(t)) => Json(t).into_response(),
        Ok(None) => error(StatusCode::NOT_FOUND, "unknown ISRC"),
        Err(e) => {
            tracing::error!("track_detail failed: {e:#}");
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

/// `GET /isrc/{isrc}/objects` — resolve an ISRC to its object variants (#359).
async fn get_isrc_objects(state: State<Arc<AppState>>, Path(isrc): Path<String>) -> Response {
    crate::store::isrc_objects(state, Path(isrc)).await
}

/// Where a single track's bytes can currently be served from (#358).
enum Servable {
    /// An openable file under `data_dir` (`X-Source: local`).
    Local(PathBuf),
    /// A content-addressed object in the backup store (`X-Source: store`).
    Store(String),
}

/// Resolve `isrc` + delivery `format` to a servable source. Local-first: an
/// openable data_dir file wins; otherwise the permanent object store (#359).
/// `None` when neither is available. Shared by the single-file download and the
/// order archive so both expose the same "servable" view.
async fn resolve_servable(
    state: &Arc<AppState>,
    track: Option<&crate::models::Track>,
    isrc: &str,
    format: &str,
) -> Option<Servable> {
    if let Some(t) = track {
        if let Some(path) = t.path_for(format) {
            match tokio::fs::File::open(path).await {
                Ok(_) => return Some(Servable::Local(PathBuf::from(path))),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => tracing::error!("open {path} failed: {e:#}"),
            }
        }
    }
    let obj = crate::store::resolve_isrc_object(&state.pool, &state.store, isrc, format).await?;
    Some(Servable::Store(obj.hash))
}

/// First servable format from `formats` (in order), as `(format, source)`.
async fn resolve_first(
    state: &Arc<AppState>,
    track: Option<&crate::models::Track>,
    isrc: &str,
    formats: &[&str],
) -> Option<(String, Servable)> {
    for format in formats {
        if let Some(s) = resolve_servable(state, track, isrc, format).await {
            return Some(((*format).to_string(), s));
        }
    }
    None
}

/// `GET /isrc/{isrc}/{flac|320|128}` — stream the delivered file. Local-first:
/// an openable data_dir file wins (`X-Source: local`); otherwise fall back to
/// the permanent object store (`X-Source: store`). Neither → `404` (#359).
async fn get_file(
    State(state): State<Arc<AppState>>,
    Path((isrc, format)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let isrc = db::normalize_key(&isrc);
    let format = format.to_lowercase();
    if !matches!(format.as_str(), "flac" | "320" | "128") {
        return error(StatusCode::BAD_REQUEST, "format must be flac, 320 or 128");
    }

    let track = match db::get_track(&state.pool, &isrc).await {
        Ok(t) => t,
        Err(e) => {
            tracing::error!("get_file failed: {e:#}");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to read track");
        }
    };

    let content_type = if format == "flac" {
        "audio/flac"
    } else {
        "audio/mpeg"
    };

    match resolve_servable(&state, track.as_ref(), &isrc, &format).await {
        Some(Servable::Local(path)) => match tokio::fs::File::open(&path).await {
            Ok(file) => {
                let stream = ReaderStream::new(file);
                let mut resp = Body::from_stream(stream).into_response();
                resp.headers_mut().insert(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static(content_type),
                );
                resp.headers_mut()
                    .insert("x-source", header::HeaderValue::from_static("local"));
                resp
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                error(StatusCode::NOT_FOUND, format!("no {format} for this ISRC"))
            }
            Err(e) => {
                tracing::error!("open {} failed: {e:#}", path.display());
                error(StatusCode::INTERNAL_SERVER_ERROR, "file missing on disk")
            }
        },
        // Store fallback: the content-addressed object is the permanent source.
        Some(Servable::Store(hash)) => {
            let mut resp =
                crate::store::stream_object(&state.store, &hash, content_type, &headers).await;
            if matches!(resp.status(), StatusCode::OK | StatusCode::PARTIAL_CONTENT) {
                resp.headers_mut()
                    .insert("x-source", header::HeaderValue::from_static("store"));
            }
            resp
        }
        None if track.is_none() => error(StatusCode::NOT_FOUND, "unknown ISRC"),
        None => error(StatusCode::NOT_FOUND, format!("no {format} for this ISRC")),
    }
}

/// `GET /orders/{id}/archive[?format=flac|320|128]` — the whole order as one
/// ZIP (#358). Tracks that are not currently servable are skipped; `format`
/// pins a single delivery format, otherwise each track contributes its
/// best-available one (`flac` → `320` → `128`).
#[derive(Debug, Deserialize)]
struct ArchiveQuery {
    format: Option<String>,
}

async fn get_order_archive(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<ArchiveQuery>,
) -> Response {
    // Validate the optional format up front (400 before touching the order).
    let requested = match q.format.as_deref() {
        None => None,
        Some(f) => {
            let f = f.to_lowercase();
            if !matches!(f.as_str(), "flac" | "320" | "128") {
                return error(StatusCode::BAD_REQUEST, "format must be flac, 320 or 128");
            }
            Some(f)
        }
    };

    match db::get_order(&state.pool, &id).await {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::NOT_FOUND, "order not found"),
        Err(e) => {
            tracing::error!("get_order_archive get_order failed: {e:#}");
            return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to read order");
        }
    }

    let items = match db::order_items(&state.pool, &id).await {
        Ok(i) => i,
        Err(e) => {
            tracing::error!("get_order_archive order_items failed: {e:#}");
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "failed to read order items",
            );
        }
    };

    // Resolve each item to `(zip entry name, on-disk source path)`. Items that
    // are not servable in the selected/best format are skipped, and colliding
    // entry names get a ` (n)` suffix.
    const BEST_AVAILABLE: [&str; 3] = ["flac", "320", "128"];
    let mut entries: Vec<(String, PathBuf)> = Vec::new();
    let mut used: HashMap<String, u32> = HashMap::new();
    for item in &items {
        let track = match db::get_track(&state.pool, &item.isrc).await {
            Ok(t) => t,
            Err(e) => {
                tracing::error!("get_order_archive get_track failed: {e:#}");
                return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to read track");
            }
        };
        let one: [&str; 1];
        let formats: &[&str] = match &requested {
            Some(f) => {
                one = [f.as_str()];
                &one
            }
            None => &BEST_AVAILABLE,
        };
        let Some((format, servable)) =
            resolve_first(&state, track.as_ref(), &item.isrc, formats).await
        else {
            continue; // not currently servable → skip silently
        };
        let path = match servable {
            Servable::Local(p) => p,
            Servable::Store(hash) => state.store.path_for(&hash),
        };
        let stem = sanitize_filename(&track_stem(
            &item.isrc,
            track.as_ref().and_then(|t| t.artist.as_deref()),
            track.as_ref().and_then(|t| t.title.as_deref()),
        ));
        let name = unique_entry_name(&mut used, &stem, entry_ext(&format));
        entries.push((name, path));
    }

    if entries.is_empty() {
        return error(StatusCode::CONFLICT, "no downloadable tracks");
    }

    // Build the ZIP into a temp file (Stored, no recompression), then stream it.
    let tmp_dir = state.config.data_dir.join("tmp");
    if let Err(e) = tokio::fs::create_dir_all(&tmp_dir).await {
        tracing::error!("archive tmp dir failed: {e:#}");
        return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to build archive");
    }
    let tmp_path = tmp_dir.join(format!("order-archive-{}.zip", uuid::Uuid::new_v4()));
    let build_path = tmp_path.clone();
    let len = match tokio::task::spawn_blocking(move || build_zip(&build_path, &entries)).await {
        Ok(Ok(len)) => len,
        Ok(Err(e)) => {
            tracing::error!("archive zip build failed: {e:#}");
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to build archive");
        }
        Err(e) => {
            tracing::error!("archive zip task failed: {e:#}");
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to build archive");
        }
    };

    let file = match tokio::fs::File::open(&tmp_path).await {
        Ok(f) => f,
        Err(e) => {
            tracing::error!("archive tmp open failed: {e:#}");
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return error(StatusCode::INTERNAL_SERVER_ERROR, "failed to build archive");
        }
    };
    // Unlink before streaming: the open fd stays valid on Linux, so the temp
    // file cannot leak once the response ends (mirrors the hub's zip_response).
    let _ = tokio::fs::remove_file(&tmp_path).await;

    let stream = ReaderStream::new(file);
    let mut resp = Body::from_stream(stream).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/zip"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        header::HeaderValue::from_str(&format!("attachment; filename=\"order-{id}.zip\"")).unwrap(),
    );
    resp.headers_mut().insert(
        header::CONTENT_LENGTH,
        header::HeaderValue::from_str(&len.to_string()).unwrap(),
    );
    resp
}

/// File extension for a delivery format (`flac` → `.flac`, else `.mp3`).
fn entry_ext(format: &str) -> &'static str {
    if format == "flac" {
        "flac"
    } else {
        "mp3"
    }
}

/// `<artist> - <title>`, falling back to the track key when both are empty.
fn track_stem(isrc: &str, artist: Option<&str>, title: Option<&str>) -> String {
    let parts: Vec<&str> = [artist, title]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        isrc.to_string()
    } else {
        parts.join(" - ")
    }
}

/// Strip path/control characters, collapse whitespace, trim and cap length so
/// the result is a safe ZIP entry name.
fn sanitize_filename(raw: &str) -> String {
    let mut cleaned = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            cleaned.push('_');
        } else {
            cleaned.push(c);
        }
    }
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim_matches(|c: char| c == '.' || c == ' ');
    let mut out: String = trimmed.chars().take(150).collect();
    if out.is_empty() {
        out.push_str("track");
    }
    out
}

/// Unique `{stem}.{ext}` within the archive, appending ` (n)` from 2 on.
fn unique_entry_name(used: &mut HashMap<String, u32>, stem: &str, ext: &str) -> String {
    let key = format!("{stem}.{ext}");
    let n = used.entry(key.clone()).or_insert(0);
    *n += 1;
    if *n == 1 {
        key
    } else {
        format!("{stem} ({}).{ext}", *n)
    }
}

/// Write a Stored ZIP of `entries` to `path` (blocking; run on a worker thread).
/// Returns the finished file's byte length.
fn build_zip(path: &std::path::Path, entries: &[(String, PathBuf)]) -> anyhow::Result<u64> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    let file = std::fs::File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    for (name, src) in entries {
        let bytes = std::fs::read(src)?;
        zip.start_file(name.as_str(), opts)?;
        zip.write_all(&bytes)?;
    }
    zip.finish()?;
    Ok(std::fs::metadata(path)?.len())
}
