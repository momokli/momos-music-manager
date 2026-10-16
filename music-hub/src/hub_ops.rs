//! Hub ops console (issue #350): pause/resume the Hub worker loops, retry
//! failed Hub jobs and read the in-memory loop event log.
//!
//! Mirrors the action pattern of `music_ops.rs`: admin-guarded handlers, htmx
//! fragments, and `HX-Trigger` toasts for `hx-swap="none"` actions.

use askama::Template;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Deserialize;

use crate::api::AppState;
use crate::worker::{EVENT_CAPACITY, LOOP_NAMES};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/hub/ops/logs", get(logs_frag))
        .route("/api/hub/ops/workers/{name}/pause", post(worker_pause))
        .route("/api/hub/ops/workers/{name}/resume", post(worker_resume))
        .route("/api/hub/ops/retry", post(retry))
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
    crate::toast::header(msg, kind)
}

/// Action response: htmx → 204 + toast header; plain form → redirect with flash.
fn action_response(headers: &HeaderMap, msg: &str, kind: &str, fallback: &str) -> Response {
    if htmx(headers) {
        (StatusCode::NO_CONTENT, toast_header(msg, kind)).into_response()
    } else {
        Redirect::to(&format!("{fallback}?msg={}", urlencoding::encode(msg))).into_response()
    }
}

/// Admin guard for action handlers: `None` ⇒ redirect to `/login` when not
/// logged in, `Some(403)` when logged in but not an admin.
async fn guard(st: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    let Some(nav) = crate::ui::nav(st, headers, "admin").await else {
        return Err(Redirect::to("/login").into_response());
    };
    if !nav.is_admin {
        return Err((StatusCode::FORBIDDEN, "Nur Admins.").into_response());
    }
    Ok(())
}

// ── event log fragment ───────────────────────────────────────────────────────

struct LogRow {
    time: String,
    loop_name: String,
    worked: bool,
    error: String,
}

#[derive(Template)]
#[template(path = "_partials/hub_logs.html")]
struct LogsFrag {
    rows: Vec<LogRow>,
}

#[derive(Deserialize, Default)]
struct LogsQuery {
    limit: Option<usize>,
}

async fn logs_frag(
    State(st): State<AppState>,
    Query(q): Query<LogsQuery>,
    headers: HeaderMap,
) -> Response {
    // Fragment convention: 401 when not logged in, 403 when not an admin.
    let Some(nav) = crate::ui::nav(&st, &headers, "admin").await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !nav.is_admin {
        return (StatusCode::FORBIDDEN, "Nur Admins.").into_response();
    }
    let limit = q.limit.unwrap_or(120).min(EVENT_CAPACITY);
    let rows = st
        .worker_health
        .events()
        .into_iter()
        .take(limit)
        .map(|e| LogRow {
            time: e.at,
            loop_name: e.loop_name.to_string(),
            worked: e.worked,
            error: e.error.unwrap_or_default(),
        })
        .collect();
    render(&LogsFrag { rows })
}

// ── worker pause / resume ────────────────────────────────────────────────────

async fn worker_pause(
    State(st): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    set_paused(&st, &headers, &name, true).await
}

async fn worker_resume(
    State(st): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    set_paused(&st, &headers, &name, false).await
}

async fn set_paused(st: &AppState, headers: &HeaderMap, name: &str, paused: bool) -> Response {
    if let Err(resp) = guard(st, headers).await {
        return resp;
    }
    // Only the known loops are addressable; anything else is a harmless no-op.
    let Some(loop_name) = LOOP_NAMES.iter().copied().find(|n| *n == name) else {
        return action_response(
            headers,
            &format!("Unbekannter Loop: {name}"),
            "err",
            "/admin",
        );
    };
    st.worker_health.set_paused(loop_name, paused);
    let msg = if paused {
        format!("Loop {loop_name} pausiert")
    } else {
        format!("Loop {loop_name} wieder aktiv")
    };
    action_response(headers, &msg, "ok", "/admin")
}

// ── retry failed jobs ────────────────────────────────────────────────────────

async fn retry(State(st): State<AppState>, headers: HeaderMap) -> Response {
    if let Err(resp) = guard(&st, &headers).await {
        return resp;
    }

    // `fail()` sets `enabled_for_fetch = 0`; without resetting it the worker's
    // playlist filter would ignore the requeued job (risk 3).
    let playlists = sqlx::query(
        "UPDATE hub_playlists
            SET fetch_status = 'queued', fetch_error = NULL, enabled_for_fetch = 1
          WHERE service = 'spotify' AND fetch_status = 'error'",
    )
    .execute(&st.pool)
    .await;
    let playlists = match playlists {
        Ok(r) => r.rows_affected(),
        Err(e) => return action_response(&headers, &format!("Fehler: {e}"), "err", "/admin"),
    };

    // The likes worker picks up `likes_status IS NULL OR = 'queued'`.
    let likes = sqlx::query(
        "UPDATE hub_service_accounts
            SET likes_status = 'queued', likes_error = NULL
          WHERE service = 'spotify' AND likes_status = 'error'",
    )
    .execute(&st.pool)
    .await;
    let likes = match likes {
        Ok(r) => r.rows_affected(),
        Err(e) => return action_response(&headers, &format!("Fehler: {e}"), "err", "/admin"),
    };

    let (msg, kind) = if playlists == 0 && likes == 0 {
        ("Nichts zu tun - keine fehlgeschlagenen Jobs".to_string(), "info")
    } else {
        (
            format!("{playlists} Playlists, {likes} Likes erneut eingereiht"),
            "ok",
        )
    };
    action_response(&headers, &msg, kind, "/admin")
}
