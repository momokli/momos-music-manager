//! Tag queue — tracks sorted by ripeness (least-tagged first), a right-hand
//! detail panel with inline tagging, and an audio player (music-api).
//!
//! Issues: #219 (queue), #220 (tag-task UI), plus direct track tagging.

use askama::Template;
use axum::Router;
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use sqlx::Row;

use crate::api::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/tag-queue", get(queue_page))
        .route("/tag-queue/{id}", get(queue_detail))
        .route("/tag-queue/{id}/tag", post(queue_tag))
        .route("/tag-queue/{id}/untag", post(queue_untag))
        .route("/track/{id}/stream", get(track_stream))
        .with_state(state)
}

fn render<T: Template>(t: &T) -> Response {
    match t.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Template-Fehler: {e}"),
        )
            .into_response(),
    }
}

#[derive(Deserialize, Default)]
struct QueueFilter {
    q: Option<String>,
    /// Hide tracks with ripeness above this value ("don't show above X").
    max: Option<f64>,
    /// "1" = only tracks with no tags at all.
    untagged: Option<String>,
    /// "1" = only tracks that appear in one of my playlists.
    mine: Option<String>,
}

#[derive(Template)]
#[template(path = "queue.html")]
struct QueuePage {
    nav: crate::ui::Nav,
    flash: String,
    q: String,
    max: String,
    untagged: bool,
    mine: bool,
    rows: Vec<QueueRowView>,
    total_shown: usize,
}

struct QueueRowView {
    id: i64,
    title: String,
    artists: String,
    album: String,
    total: i64,
    tag_score: i64,
    meta_score: i64,
    traktor: String,
    tag_count: i64,
}

async fn queue_page(
    State(st): State<AppState>,
    Query(f): Query<QueueFilter>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "queue").await else {
        return Redirect::to("/login").into_response();
    };
    let q = f.q.unwrap_or_default().trim().to_string();
    let untagged = f.untagged.as_deref() == Some("1");
    let mine = f.mine.as_deref() == Some("1");
    let e = crate::settings::engine(&st.pool).await;

    // Tracks in my playlists (for the "mine" filter).
    let mine_ids: std::collections::HashSet<i64> = if mine {
        sqlx::query_scalar::<_, i64>(
            "SELECT DISTINCT hpt.track_id FROM hub_playlist_tracks hpt
               JOIN hub_playlists hp ON hp.id = hpt.playlist_id
              WHERE hp.user_id = ?1",
        )
        .bind(nav.id)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
    } else {
        Default::default()
    };

    let needle = q.to_lowercase();
    let rows: Vec<QueueRowView> = crate::scoring::queue(&st.pool, &e, f.max, untagged, 2000)
        .await
        .into_iter()
        .filter(|r| {
            (needle.is_empty()
                || r.title.to_lowercase().contains(&needle)
                || r.artists.to_lowercase().contains(&needle))
                && (!mine || mine_ids.contains(&r.track_id))
        })
        .take(500)
        .map(|r| QueueRowView {
            id: r.track_id,
            title: r.title,
            artists: r.artists,
            album: r.album,
            total: r.total.round() as i64,
            tag_score: r.tag_score.round() as i64,
            meta_score: r.meta_score.round() as i64,
            traktor: format!("{:.1}", r.traktor_score),
            tag_count: r.tag_count,
        })
        .collect();
    let total_shown = rows.len();
    render(&QueuePage {
        nav,
        flash: String::new(),
        q,
        max: f.max.map(|m| m.to_string()).unwrap_or_default(),
        untagged,
        mine,
        rows,
        total_shown,
    })
}

// ── detail panel (htmx partial) ─────────────────────────────────────────────

struct DetailTag {
    id: i64,
    name: String,
    owner: String,
    groups: String,
    mine: bool,
}

struct DetailGroupTag {
    id: i64,
    name: String,
}

#[derive(Template)]
#[template(path = "queue_detail.html")]
struct QueueDetail {
    id: i64,
    title: String,
    artists: String,
    album: String,
    duration: String,
    isrc: String,
    has_isrc: bool,
    music_api: bool,
    tags: Vec<DetailTag>,
    my_tags: Vec<DetailGroupTag>,
}

async fn build_detail(st: &AppState, me: i64, id: i64) -> Option<QueueDetail> {
    let row = sqlx::query(
        "SELECT COALESCE(title,'') AS title, COALESCE(artists,'') AS artists,
                COALESCE(album,'') AS album, duration_ms, COALESCE(isrc,'') AS isrc
           FROM hub_tracks WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten()?;

    let duration = {
        let ms = row.get::<Option<i64>, _>("duration_ms").unwrap_or(0);
        if ms > 0 {
            format!("{}:{:02}", ms / 60000, (ms % 60000) / 1000)
        } else {
            String::new()
        }
    };

    let tag_rows = sqlx::query_as::<_, (i64, String, String, String)>(
        "SELECT t.id, t.name, u.slug,
                (SELECT COALESCE(GROUP_CONCAT(g.icon || ' ' || g.name, ' · '), '')
                   FROM hub_group_tags gt JOIN hub_tag_groups g ON g.id = gt.group_id
                  WHERE gt.tag_id = t.id) AS groups
           FROM hub_track_resolved_tags rt
           JOIN hub_tags t ON t.id = rt.tag_id
           JOIN hub_users u ON u.id = t.owner_user_id
          WHERE rt.track_id = ?1
          ORDER BY t.name LIMIT 200",
    )
    .bind(id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();
    let my_tag_ids: std::collections::HashSet<i64> =
        sqlx::query_scalar::<_, i64>("SELECT id FROM hub_tags WHERE owner_user_id = ?1")
            .bind(me)
            .fetch_all(&st.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
    let tags: Vec<DetailTag> = tag_rows
        .into_iter()
        .map(|(id, name, owner, groups)| DetailTag {
            mine: my_tag_ids.contains(&id),
            id,
            name,
            owner,
            groups,
        })
        .collect();

    let my_tags: Vec<DetailGroupTag> = crate::tags::list_user_tags(&st.pool, me)
        .await
        .into_iter()
        .map(|(id, name)| DetailGroupTag { id, name })
        .collect();

    let isrc = row.get::<String, _>("isrc");
    Some(QueueDetail {
        id,
        title: row.get("title"),
        artists: row.get("artists"),
        album: row.get("album"),
        duration,
        has_isrc: !isrc.trim().is_empty(),
        isrc,
        music_api: st.cfg.music_api_token.is_some(),
        tags,
        my_tags,
    })
}

async fn queue_detail(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "queue").await else {
        return Redirect::to("/login").into_response();
    };
    match build_detail(&st, nav.id, id).await {
        Some(d) => render(&d),
        None => (StatusCode::NOT_FOUND, "Track nicht gefunden.").into_response(),
    }
}

#[derive(Deserialize)]
struct TagForm {
    name: String,
}

async fn queue_tag(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<TagForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "queue").await else {
        return Redirect::to("/login").into_response();
    };
    if !f.name.trim().is_empty() {
        if let Ok(tag_id) = crate::tags::ensure_tag(&st.pool, nav.id, &f.name).await {
            let _ = crate::tags::tag_track(&st.pool, nav.id, id, tag_id).await;
        }
    }
    match build_detail(&st, nav.id, id).await {
        Some(d) => render(&d),
        None => (StatusCode::NOT_FOUND, "Track nicht gefunden.").into_response(),
    }
}

async fn queue_untag(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<TagForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "queue").await else {
        return Redirect::to("/login").into_response();
    };
    if let Some(tag_id) = crate::tags::find_tag_id_by_name(&st.pool, &f.name).await {
        let _ = crate::tags::untag_track(&st.pool, nav.id, id, tag_id).await;
    }
    match build_detail(&st, nav.id, id).await {
        Some(d) => render(&d),
        None => (StatusCode::NOT_FOUND, "Track nicht gefunden.").into_response(),
    }
}

// ── audio stream (player) ───────────────────────────────────────────────────

async fn track_stream(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if crate::ui::nav(&st, &headers, "queue").await.is_none() {
        return Redirect::to("/login").into_response();
    }
    let isrc =
        sqlx::query_scalar::<_, String>("SELECT COALESCE(isrc,'') FROM hub_tracks WHERE id = ?1")
            .bind(id)
            .fetch_optional(&st.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
    if isrc.trim().is_empty() {
        return (StatusCode::BAD_REQUEST, "Track hat keine ISRC").into_response();
    }
    match crate::music_api::file_bytes(&st.cfg, &isrc, "mp3").await {
        Ok(bytes) => Response::builder()
            .header(axum::http::header::CONTENT_TYPE, "audio/mpeg")
            .header(axum::http::header::ACCEPT_RANGES, "none")
            .body(axum::body::Body::from(bytes))
            .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response()),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            format!("music-api: nicht verfügbar (erst ordern): {e}"),
        )
            .into_response(),
    }
}
