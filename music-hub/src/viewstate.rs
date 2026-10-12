//! Per-user "work settings": remember the last filter/sort query per list view
//! so navigating to a view restores your previous filters (`hub_view_state`).
//!
//! The [`middleware`] runs on GET navigations to known views: a request with no
//! filter params is redirected to the remembered query; a request with params
//! saves them. htmx fragments and non-GET requests are left untouched.

use axum::extract::{Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Redirect, Response};
use sqlx::SqlitePool;

use crate::api::AppState;

/// Map a request path to a stable view key (or `None` if not a list view).
pub fn known_view(path: &str) -> Option<&'static str> {
    Some(match path.trim_end_matches('/') {
        "/tags" => "tags",
        "/tracks" => "tracks",
        "/artists" => "artists",
        "/albums" => "albums",
        "/overlap" => "overlap",
        "/tag-queue" => "tag-queue",
        "/tag-search" => "tag-search",
        "/revisit" => "revisit",
        "/me/playlists" => "me-playlists",
        "/playlists/similar" => "playlists-similar",
        "/history" => "history",
        "/queue" => "queue",
        "/downloads" => "downloads",
        _ => return None,
    })
}

/// Strip the flash param (`msg`) from a raw query. Returns `(cleaned, had_msg)`.
pub fn clean_query(raw: &str) -> (String, bool) {
    let mut kept: Vec<&str> = Vec::new();
    let mut had_msg = false;
    for kv in raw.split('&') {
        if kv.is_empty() {
            continue;
        }
        let key = kv.split('=').next().unwrap_or("");
        if key == "msg" {
            had_msg = true;
        } else {
            kept.push(kv);
        }
    }
    (kept.join("&"), had_msg)
}

/// The remembered query for a view, if any.
pub async fn remembered(pool: &SqlitePool, user_id: i64, view: &str) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT query FROM hub_view_state WHERE user_id = ?1 AND view = ?2",
    )
    .bind(user_id)
    .bind(view)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

/// Persist a view's query string.
pub async fn save(pool: &SqlitePool, user_id: i64, view: &str, query: &str) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO hub_view_state (user_id, view, query, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(user_id, view) DO UPDATE SET query = excluded.query, updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(view)
    .bind(query)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// Axum middleware: restore/save each user's per-view filter state.
pub async fn middleware(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let is_get = req.method() == axum::http::Method::GET;
    let is_htmx = req.headers().contains_key("hx-request");
    let path = req.uri().path().to_string();
    let raw = req.uri().query().unwrap_or("").to_string();

    if is_get && !is_htmx {
        if let Some(view) = known_view(&path) {
            let headers = req.headers().clone();
            if let Some((uid, _)) = crate::web::current_user(&st, &headers).await {
                let (cleaned, had_msg) = clean_query(&raw);
                if cleaned.is_empty() {
                    // No explicit filters: restore the remembered ones (but don't
                    // swallow a flash message).
                    if !had_msg {
                        if let Some(prev) = remembered(&st.pool, uid, view).await {
                            if !prev.is_empty() {
                                let target = format!("{path}?{prev}");
                                return Redirect::to(&target).into_response();
                            }
                        }
                    }
                } else {
                    let _ = save(&st.pool, uid, view, &cleaned).await;
                }
            }
        }
    }
    next.run(req).await
}
