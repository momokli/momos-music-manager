//! Native music-api integration: an ops console showing the live deemix queue
//! and the service event log, with action buttons (refresh states, order missing)
//! that give toast feedback via htmx.

use askama::Template;
use axum::Router;
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Deserialize;

use crate::api::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/music-api", get(page))
        .route("/music-api/queue", get(queue_frag))
        .route("/music-api/logs", get(logs_frag))
        .route("/music-api/orders", get(orders_frag))
        .route("/music-api/worker", get(worker_frag))
        .route("/music-api/refresh", post(refresh))
        .route("/music-api/order-missing", post(order_missing))
        .route("/music-api/worker/pause", post(worker_pause))
        .route("/music-api/worker/resume", post(worker_resume))
        .route("/music-api/order/cancel", post(order_cancel))
        .route("/music-api/order/prioritize", post(order_prioritize))
        // Per-track music-api panel (fetched lazily into the track page).
        .route("/track/{id}/music-api", get(track_panel))
        .route("/track/{id}/music-api/order", post(track_order))
        .route("/track/{id}/music-api/retry", post(track_retry))
        .with_state(state)
}

fn render<T: Template>(t: &T) -> Response {
    match t.render() {
        Ok(s) => Html(s).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("template error: {e}"),
        )
            .into_response(),
    }
}

fn htmx(headers: &HeaderMap) -> bool {
    headers.contains_key("hx-request")
}

/// Add an `HX-Trigger` toast to a response header map.
fn toast_header(msg: &str, kind: &str) -> HeaderMap {
    let mut h = HeaderMap::new();
    let body = serde_json::json!({ "toast": { "msg": msg, "kind": kind } }).to_string();
    if let Ok(hv) = HeaderValue::from_str(&body) {
        h.insert(HeaderName::from_static("hx-trigger"), hv);
    }
    h
}

/// Action response: htmx → 204 + toast header; plain form → redirect with flash.
fn action_response(headers: &HeaderMap, msg: &str, kind: &str, fallback: &str) -> Response {
    if htmx(headers) {
        (StatusCode::NO_CONTENT, toast_header(msg, kind)).into_response()
    } else {
        Redirect::to(&format!("{fallback}?msg={}", urlencoding::encode(msg))).into_response()
    }
}

#[derive(Deserialize, Default)]
struct Msg {
    msg: Option<String>,
}

#[derive(Template)]
#[template(path = "music_api.html")]
struct OpsPage {
    nav: crate::ui::Nav,
    flash: String,
    configured: bool,
    // summary counts
    tracks_with_isrc: i64,
    ready: i64,
    errors: i64,
}

async fn page(State(st): State<AppState>, Query(m): Query<Msg>, headers: HeaderMap) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "music-api").await else {
        return Redirect::to("/login").into_response();
    };
    let tracks_with_isrc: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_tracks WHERE isrc IS NOT NULL AND TRIM(isrc) <> ''",
    )
    .fetch_one(&st.pool)
    .await
    .unwrap_or(0);
    let summary = crate::music_api::cache_summary(&st.pool).await;
    render(&OpsPage {
        nav,
        flash: m.msg.unwrap_or_default(),
        configured: st.cfg.music_api_token.is_some(),
        tracks_with_isrc,
        ready: summary.ready as i64,
        errors: summary.errors as i64,
    })
}

// ── fragments (htmx-polled) ──────────────────────────────────────────────────

struct QueueRow {
    title: String,
    status: String,
    progress: String,
}

#[derive(Template)]
#[template(path = "_partials/music_queue.html")]
struct QueueFrag {
    items: Vec<QueueRow>,
    error: String,
}

async fn queue_frag(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if crate::ui::nav(&st, &headers, "music-api").await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (items, error) = crate::music_api::live_queue(&st.cfg).await;
    let items = items
        .into_iter()
        .map(|q| QueueRow {
            title: q.title.unwrap_or_default(),
            status: q.status,
            progress: q.progress.map(|p| format!("{p:.0}%")).unwrap_or_default(),
        })
        .collect();
    render(&QueueFrag {
        items,
        error: error.unwrap_or_default(),
    })
}

struct LogRow {
    time: String,
    level: String,
    msg: String,
}

#[derive(Template)]
#[template(path = "_partials/music_logs.html")]
struct LogsFrag {
    rows: Vec<LogRow>,
}

async fn logs_frag(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if crate::ui::nav(&st, &headers, "music-api").await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let rows = crate::music_api::logs(&st.cfg, 120)
        .await
        .into_iter()
        .map(|l| LogRow {
            time: l.at.to_string(),
            level: l.level,
            msg: l.msg,
        })
        .collect();
    render(&LogsFrag { rows })
}

// ── actions ──────────────────────────────────────────────────────────────────

/// All distinct non-empty ISRCs in the hub (bounded by the caller's cap).
async fn all_isrcs(st: &AppState) -> Vec<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT isrc FROM hub_tracks WHERE isrc IS NOT NULL AND TRIM(isrc) <> ''",
    )
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default()
}

async fn refresh(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if crate::ui::nav(&st, &headers, "music-api").await.is_none() {
        return Redirect::to("/login").into_response();
    }
    if st.cfg.music_api_token.is_none() {
        return action_response(
            &headers,
            "music-api nicht konfiguriert",
            "err",
            "/music-api",
        );
    }
    let isrcs = all_isrcs(&st).await;
    let (ready, total, refreshed) =
        crate::music_api::refresh_states(&st.pool, &st.cfg, &isrcs, 500).await;
    let msg = format!("{refreshed} States aktualisiert · {ready}/{total} ready");
    action_response(&headers, &msg, "ok", "/music-api")
}

async fn order_missing(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if crate::ui::nav(&st, &headers, "music-api").await.is_none() {
        return Redirect::to("/login").into_response();
    }
    if st.cfg.music_api_token.is_none() {
        return action_response(
            &headers,
            "music-api nicht konfiguriert",
            "err",
            "/music-api",
        );
    }
    let isrcs = all_isrcs(&st).await;
    let missing = crate::music_api::missing_isrcs(&st.pool, &isrcs).await;
    let capped: Vec<String> = missing.into_iter().take(500).collect();
    let (msg, kind) = if capped.is_empty() {
        ("Keine fehlenden ISRCs — alles ready".to_string(), "info")
    } else {
        match crate::music_api::order(&st.cfg, &capped).await {
            Ok(id) => (
                format!("{} ISRCs bestellt (Order {})", capped.len(), id),
                "ok",
            ),
            Err(e) => (format!("Order fehlgeschlagen: {e}"), "err"),
        }
    };
    action_response(&headers, &msg, kind, "/music-api")
}

// ── worker control ───────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "_partials/music_worker.html")]
struct WorkerFrag {
    paused: bool,
}

async fn worker_frag(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if crate::ui::nav(&st, &headers, "music-api").await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let paused = crate::music_api::worker_paused(&st.cfg).await;
    render(&WorkerFrag { paused })
}

async fn set_worker(st: &AppState, headers: &HeaderMap, paused: bool) -> Response {
    if crate::ui::nav(st, headers, "music-api").await.is_none() {
        return Redirect::to("/login").into_response();
    }
    if st.cfg.music_api_token.is_none() {
        return action_response(headers, "music-api nicht konfiguriert", "err", "/music-api");
    }
    match crate::music_api::set_worker_paused(&st.cfg, paused).await {
        Ok(()) => action_response(
            headers,
            if paused {
                "Worker pausiert"
            } else {
                "Worker läuft wieder"
            },
            "ok",
            "/music-api",
        ),
        Err(e) => action_response(headers, &format!("Fehler: {e}"), "err", "/music-api"),
    }
}

async fn worker_pause(State(st): State<AppState>, headers: HeaderMap) -> Response {
    set_worker(&st, &headers, true).await
}

async fn worker_resume(State(st): State<AppState>, headers: HeaderMap) -> Response {
    set_worker(&st, &headers, false).await
}

// ── orders ───────────────────────────────────────────────────────────────────

struct OrderRowView {
    id: String,
    status: String,
    priority: i64,
    created: String,
}

#[derive(Template)]
#[template(path = "_partials/music_orders.html")]
struct OrdersFrag {
    rows: Vec<OrderRowView>,
}

async fn orders_frag(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if crate::ui::nav(&st, &headers, "music-api").await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let rows = crate::music_api::orders(&st.cfg, 25)
        .await
        .into_iter()
        .map(|o| OrderRowView {
            id: o.id,
            status: o.status,
            priority: o.priority,
            created: o.created_at.to_string(),
        })
        .collect();
    render(&OrdersFrag { rows })
}

#[derive(Deserialize)]
struct OrderAction {
    id: String,
}

async fn order_cancel(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<OrderAction>,
) -> Response {
    if crate::ui::nav(&st, &headers, "music-api").await.is_none() {
        return Redirect::to("/login").into_response();
    }
    if st.cfg.music_api_token.is_none() {
        return action_response(
            &headers,
            "music-api nicht konfiguriert",
            "err",
            "/music-api",
        );
    }
    match crate::music_api::cancel_order(&st.cfg, &f.id).await {
        Ok(()) => action_response(&headers, "Order abgebrochen", "ok", "/music-api"),
        Err(e) => action_response(&headers, &format!("Fehler: {e}"), "err", "/music-api"),
    }
}

async fn order_prioritize(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<OrderAction>,
) -> Response {
    if crate::ui::nav(&st, &headers, "music-api").await.is_none() {
        return Redirect::to("/login").into_response();
    }
    if st.cfg.music_api_token.is_none() {
        return action_response(
            &headers,
            "music-api nicht konfiguriert",
            "err",
            "/music-api",
        );
    }
    match crate::music_api::set_order_priority(&st.cfg, &f.id, 100).await {
        Ok(()) => action_response(&headers, "Order priorisiert (100)", "ok", "/music-api"),
        Err(e) => action_response(&headers, &format!("Fehler: {e}"), "err", "/music-api"),
    }
}

// ── per-track panel ──────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "_partials/track_music_api.html")]
struct TrackPanel {
    track_id: i64,
    configured: bool,
    isrc: String,
    found: bool,
    state: String,
    deezer_id: String,
    album: String,
    artist: String,
    title: String,
    formats: String,
    objects: String,
    error: String,
}

async fn isrc_of(st: &AppState, id: i64) -> String {
    sqlx::query_scalar::<_, String>("SELECT COALESCE(isrc, '') FROM hub_tracks WHERE id = ?1")
        .bind(id)
        .fetch_optional(&st.pool)
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

async fn track_panel(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if crate::ui::nav(&st, &headers, "search").await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let configured = st.cfg.music_api_token.is_some();
    let isrc = isrc_of(&st, id).await;
    let mut p = TrackPanel {
        track_id: id,
        configured,
        isrc: isrc.clone(),
        found: false,
        state: String::new(),
        deezer_id: String::new(),
        album: String::new(),
        artist: String::new(),
        title: String::new(),
        formats: String::new(),
        objects: String::new(),
        error: String::new(),
    };
    if configured && !isrc.is_empty() {
        if let Ok(list) = crate::music_api::tracks(&st.cfg, std::slice::from_ref(&isrc)).await {
            if let Some(m) = list.into_iter().next() {
                p.found = !m.unknown;
                p.state = m.state.clone().unwrap_or_default();
                p.deezer_id = m.deezer_id.clone().unwrap_or_default();
                p.album = m.album.clone().unwrap_or_default();
                p.artist = m.artist.clone().unwrap_or_default();
                p.title = m.title.clone().unwrap_or_default();
                p.formats = m.formats.join(", ");
                p.objects = if m.objects.is_empty() {
                    "—".to_string()
                } else {
                    format!("{} Objekt(e) · {} B", m.objects.len(), m.object_bytes())
                };
                p.error = m.error.unwrap_or_default();
            }
        }
    }
    render(&p)
}

async fn track_action(st: &AppState, headers: &HeaderMap, id: i64, retry: bool) -> Response {
    let back = format!("/track/{id}");
    if crate::ui::nav(st, headers, "search").await.is_none() {
        return Redirect::to("/login").into_response();
    }
    if st.cfg.music_api_token.is_none() {
        return action_response(headers, "music-api nicht konfiguriert", "err", &back);
    }
    let isrc = isrc_of(st, id).await;
    if isrc.is_empty() {
        return action_response(headers, "Track hat keine ISRC", "err", &back);
    }
    let res = if retry {
        crate::music_api::retry_isrc(&st.cfg, &isrc).await
    } else {
        crate::music_api::order(&st.cfg, std::slice::from_ref(&isrc))
            .await
            .map(|_| ())
    };
    match res {
        Ok(()) => action_response(
            headers,
            if retry {
                "Retry eingereiht"
            } else {
                "Order erstellt"
            },
            "ok",
            &back,
        ),
        Err(e) => action_response(headers, &format!("Fehler: {e}"), "err", &back),
    }
}

async fn track_order(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    track_action(&st, &headers, id, false).await
}

async fn track_retry(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    track_action(&st, &headers, id, true).await
}
