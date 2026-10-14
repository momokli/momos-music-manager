//! Listening history (#323): recent and top listens per user, plus what the
//! collective is playing. Backed by `hub_listening_history` (recorded by the
//! now-playing poll).

use askama::Template;
use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use serde::Deserialize;
use sqlx::Row;

use crate::api::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/listening", get(listening_page))
        .with_state(state)
}

fn render<T: Template>(tpl: &T) -> Response {
    match tpl.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Template-Fehler: {e}"),
        )
            .into_response(),
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ScopeQuery {
    /// `me` (default) or `all`.
    scope: Option<String>,
}

#[derive(Template)]
#[template(path = "listening.html")]
struct ListeningPage {
    nav: crate::ui::Nav,
    flash: String,
    scope: String,
    recent: Vec<ListenRow>,
    top_tracks: Vec<CountRow>,
    top_artists: Vec<CountRow>,
}

struct ListenRow {
    when: String,
    who: String,
    track_id: i64,
    title: String,
    artists_html: String,
    album: String,
}

struct CountRow {
    label: String,
    sub_html: String,
    count: i64,
    track_id: i64,
}

/// `2026-10-11T18:22:33+00:00` -> `2026-10-11 18:22`.
fn fmt_ts(ts: Option<String>) -> String {
    match ts {
        Some(s) if s.len() >= 16 => s[..16].replace('T', " "),
        Some(s) => s,
        None => "—".to_string(),
    }
}

async fn listening_page(
    State(st): State<AppState>,
    Query(q): Query<ScopeQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "listening").await else {
        return Redirect::to("/login").into_response();
    };
    let scope = match q.scope.as_deref() {
        Some("all") => "all",
        _ => "me",
    }
    .to_string();
    let all = scope == "all";

    // Recent listens (mine, or everyone's when scope=all).
    let recent_rows = if all {
        sqlx::query(
            "SELECT h.played_at, u.slug AS who, t.id AS track_id, t.title, t.artists, t.album
               FROM hub_listening_history h
               JOIN hub_tracks t ON t.id = h.track_id
               JOIN hub_users u ON u.id = h.user_id
              ORDER BY h.played_at DESC LIMIT 100",
        )
        .fetch_all(&st.pool)
        .await
    } else {
        sqlx::query(
            "SELECT h.played_at, '' AS who, t.id AS track_id, t.title, t.artists, t.album
               FROM hub_listening_history h
               JOIN hub_tracks t ON t.id = h.track_id
              WHERE h.user_id = ?1
              ORDER BY h.played_at DESC LIMIT 100",
        )
        .bind(nav.id)
        .fetch_all(&st.pool)
        .await
    }
    .unwrap_or_default();
    let mut recent: Vec<ListenRow> = recent_rows
        .into_iter()
        .map(|r| ListenRow {
            when: fmt_ts(r.get::<Option<String>, _>("played_at")),
            who: r.get::<Option<String>, _>("who").unwrap_or_default(),
            track_id: r.get::<Option<i64>, _>("track_id").unwrap_or(0),
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            artists_html: String::new(),
            album: r.get::<Option<String>, _>("album").unwrap_or_default(),
        })
        .collect();
    let recent_ids: Vec<i64> = recent.iter().map(|r| r.track_id).collect();
    let mut recent_html = crate::artists::artists_html_for_tracks(&st.pool, &recent_ids).await;
    for r in recent.iter_mut() {
        r.artists_html = recent_html.remove(&r.track_id).unwrap_or_default();
    }

    let mut top_tracks: Vec<CountRow> = sqlx::query(
        "SELECT t.id AS track_id, t.title, COUNT(*) AS c
           FROM hub_listening_history h
           JOIN hub_tracks t ON t.id = h.track_id
          WHERE h.user_id = ?1
          GROUP BY t.id
          ORDER BY c DESC, MAX(h.played_at) DESC
          LIMIT 25",
    )
    .bind(nav.id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| CountRow {
        label: r.get::<Option<String>, _>("title").unwrap_or_default(),
        sub_html: String::new(),
        count: r.get::<Option<i64>, _>("c").unwrap_or(0),
        track_id: r.get::<Option<i64>, _>("track_id").unwrap_or(0),
    })
    .collect();
    let top_ids: Vec<i64> = top_tracks.iter().map(|r| r.track_id).collect();
    let mut top_html = crate::artists::artists_html_for_tracks(&st.pool, &top_ids).await;
    for r in top_tracks.iter_mut() {
        r.sub_html = top_html.remove(&r.track_id).unwrap_or_default();
    }

    let top_artists = sqlx::query(
        "SELECT t.artists AS artist, COUNT(*) AS c
           FROM hub_listening_history h
           JOIN hub_tracks t ON t.id = h.track_id
          WHERE h.user_id = ?1 AND t.artists IS NOT NULL AND t.artists <> ''
          GROUP BY t.artists
          ORDER BY c DESC
          LIMIT 25",
    )
    .bind(nav.id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| CountRow {
        label: r.get::<Option<String>, _>("artist").unwrap_or_default(),
        sub_html: String::new(),
        count: r.get::<Option<i64>, _>("c").unwrap_or(0),
        track_id: 0,
    })
    .collect();

    render(&ListeningPage {
        nav,
        flash: String::new(),
        scope,
        recent,
        top_tracks,
        top_artists,
    })
}
