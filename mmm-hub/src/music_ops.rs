//! Native music-api integration: an ops console showing the live deemix queue
//! and the service event log, with action buttons (refresh states, order missing)
//! that give toast feedback via htmx.

use askama::Template;
use axum::Router;
use axum::extract::{Query, State};
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
        .route("/music-api/refresh", post(refresh))
        .route("/music-api/order-missing", post(order_missing))
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
