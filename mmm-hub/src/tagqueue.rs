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
    /// Hide tracks with ripeness above this value (empty = no limit).
    max: Option<String>,
    /// "1" = only tracks with no tags at all.
    untagged: Option<String>,
    /// "1" = only tracks that appear in one of my playlists.
    mine: Option<String>,
    /// Sort key: ripeness (asc, default) | ripeness-desc | title | artist |
    /// tags | tags-desc | meta | traktor.
    sort: Option<String>,
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
    sort: String,
    sort_opts: Vec<SortOpt>,
    rows: Vec<QueueRowView>,
    total_shown: usize,
}

struct SortOpt {
    value: String,
    label: String,
    selected: bool,
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
    let sort = f.sort.unwrap_or_default();
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
    let max = f
        .max
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse::<f64>().ok());
    let rows: Vec<QueueRowView> = crate::scoring::queue(&st.pool, &e, max, untagged, &sort, 2000)
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
    let sort_opts: Vec<SortOpt> = [
        ("", "Ripeness ↑ (am wenigsten getaggt)"),
        ("ripeness-desc", "Ripeness ↓"),
        ("title", "Titel"),
        ("artist", "Künstler"),
        ("tags", "Tags ↑"),
        ("tags-desc", "Tags ↓"),
        ("meta", "Meta ↑"),
        ("traktor", "Traktor-Plays ↓"),
    ]
    .iter()
    .map(|(v, l)| SortOpt {
        value: (*v).to_string(),
        label: (*l).to_string(),
        selected: *v == sort,
    })
    .collect();
    render(&QueuePage {
        nav,
        flash: String::new(),
        q,
        max: f
            .max
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or_default()
            .to_string(),
        untagged,
        mine,
        sort,
        sort_opts,
        rows,
        total_shown,
    })
}

// ── detail panel (htmx partial) ─────────────────────────────────────────────

struct TagChip {
    id: i64,
    name: String,
    mine: bool,
}

struct RecChip {
    name: String,
    reason: String,
}

struct TagCloud {
    group: String,
    icon: String,
    chips: Vec<TagChip>,
}

struct DetailGroupTag {
    id: i64,
    name: String,
}

struct DetailLink {
    name: String,
    href: String,
}

struct PlRow {
    owner: String,
    name: String,
    href: String,
}

#[derive(Template)]
#[template(path = "queue_detail.html")]
struct QueueDetail {
    id: i64,
    title: String,
    artists: Vec<DetailLink>,
    album: String,
    album_href: String,
    album_link: bool,
    cover: String,
    duration: String,
    explicit: String,
    isrc: String,
    bpm: String,
    key: String,
    energy: String,
    genres: String,
    spotify_id: String,
    has_isrc: bool,
    music_api: bool,
    tag_clouds: Vec<TagCloud>,
    untag_action: String,
    tag_action: String,
    htmx: bool,
    playlists: Vec<PlRow>,
    my_tags: Vec<DetailGroupTag>,
    my_groups: Vec<DetailGroupTag>,
    recommended: Vec<RecChip>,
}

async fn build_detail(st: &AppState, me: i64, id: i64) -> Option<QueueDetail> {
    let row = sqlx::query(
        "SELECT COALESCE(t.title,'') AS title, COALESCE(t.artists,'') AS artists,
                COALESCE(t.album,'') AS album, t.duration_ms,
                COALESCE(t.isrc,'') AS isrc, COALESCE(t.explicit,0) AS explicit,
                COALESCE(t.image_url,'') AS image_url,
                CAST((SELECT bpm FROM v_track_audio WHERE track_id = t.id) AS TEXT) AS bpm,
                CAST((SELECT camelot FROM v_track_audio WHERE track_id = t.id) AS TEXT) AS musickey,
                CAST((SELECT energy FROM hub_track_features WHERE track_id = t.id) AS TEXT) AS energy,
                (SELECT COUNT(*) FROM hub_track_genres WHERE track_id = t.id) AS genres
           FROM hub_tracks t WHERE t.id = ?1",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten()?;

    let ms = row.get::<Option<i64>, _>("duration_ms").unwrap_or(0);
    let duration = if ms > 0 {
        format!("{}:{:02}", ms / 60000, (ms % 60000) / 1000)
    } else {
        String::new()
    };
    let nonnull = |k: &str| -> String {
        row.get::<Option<String>, _>(k)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "—".to_string())
    };
    let album = row.get::<String, _>("album");
    let album_href = if album.trim().is_empty() {
        String::new()
    } else {
        format!("/album/{}", urlencoding::encode(&album))
    };
    let artists_str = row.get::<String, _>("artists");
    let artists: Vec<DetailLink> = artists_str
        .split(",")
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .map(|a| DetailLink {
            name: a.to_string(),
            href: format!("/artist/{}", urlencoding::encode(a)),
        })
        .collect();
    let genres = {
        let n = row.get::<i64, _>("genres");
        if n > 0 {
            n.to_string()
        } else {
            "—".to_string()
        }
    };

    let my_tag_ids: std::collections::HashSet<i64> =
        sqlx::query_scalar::<_, i64>("SELECT id FROM hub_tags WHERE owner_user_id = ?1")
            .bind(me)
            .fetch_all(&st.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();

    // Tags, grouped into clouds by their (first) group name.
    let tag_rows = sqlx::query_as::<_, (i64, String, String, String)>(
        "SELECT t.id, t.name,
                COALESCE((SELECT g.name FROM hub_group_tags gt JOIN hub_tag_groups g ON g.id = gt.group_id
                           WHERE gt.tag_id = t.id ORDER BY g.name LIMIT 1), '') AS gname,
                COALESCE((SELECT g.icon FROM hub_group_tags gt JOIN hub_tag_groups g ON g.id = gt.group_id
                           WHERE gt.tag_id = t.id ORDER BY g.name LIMIT 1), '') AS gicon
           FROM hub_track_resolved_tags rt
           JOIN hub_tags t ON t.id = rt.tag_id
          WHERE rt.track_id = ?1
          ORDER BY gname COLLATE NOCASE, t.name COLLATE NOCASE LIMIT 400",
    )
    .bind(id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();
    let mut map: std::collections::BTreeMap<String, (String, Vec<TagChip>)> =
        std::collections::BTreeMap::new();
    for (id, name, gname, gicon) in tag_rows {
        let key = if gname.trim().is_empty() {
            "Ohne Gruppe".to_string()
        } else {
            gname
        };
        let icon = if gicon.trim().is_empty() {
            "•".to_string()
        } else {
            gicon
        };
        let e = map.entry(key).or_insert_with(|| (icon, Vec::new()));
        e.1.push(TagChip {
            id,
            name,
            mine: my_tag_ids.contains(&id),
        });
    }
    let mut tag_clouds: Vec<TagCloud> = map
        .into_iter()
        .map(|(group, (icon, chips))| TagCloud { group, icon, chips })
        .collect();
    // Push the ungrouped cloud to the end.
    if let Some(pos) = tag_clouds.iter().position(|c| c.group == "Ohne Gruppe") {
        let last = tag_clouds.remove(pos);
        tag_clouds.push(last);
    }

    let playlists: Vec<PlRow> = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT u.slug, COALESCE(hp.name,''), hp.id
           FROM hub_playlist_tracks hpt
           JOIN hub_playlists hp ON hp.id = hpt.playlist_id
           JOIN hub_users u ON u.id = hp.user_id
          WHERE hpt.track_id = ?1 ORDER BY u.slug, hp.name LIMIT 200",
    )
    .bind(id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(owner, name, pid)| PlRow {
        owner,
        name,
        href: format!("/playlist/{pid}"),
    })
    .collect();

    let my_tags: Vec<DetailGroupTag> = crate::tags::list_user_tags(&st.pool, me)
        .await
        .into_iter()
        .map(|(id, name)| DetailGroupTag { id, name })
        .collect();
    let my_groups: Vec<DetailGroupTag> = crate::tags::groups_i_contribute(&st.pool, me)
        .await
        .into_iter()
        .map(|(id, name, _icon)| DetailGroupTag { id, name })
        .collect();
    let recommended: Vec<RecChip> = crate::recommend::recommend_tags(&st.pool, id, 16)
        .await
        .into_iter()
        .map(|r| RecChip {
            reason: r.why(),
            name: r.name,
        })
        .collect();

    let spotify_id: String = sqlx::query_scalar(
        "SELECT external_id FROM hub_track_external_ids WHERE track_id = ?1 AND service = 'spotify' LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten()
    .unwrap_or_default();

    let isrc = row.get::<String, _>("isrc");
    let has_isrc = !isrc.trim().is_empty();
    Some(QueueDetail {
        id,
        title: row.get("title"),
        artists,
        album_link: !album_href.is_empty(),
        album,
        album_href,
        cover: row.get("image_url"),
        duration,
        explicit: if row.get::<i64, _>("explicit") != 0 {
            "ja"
        } else {
            "nein"
        }
        .to_string(),
        isrc,
        has_isrc,
        music_api: st.cfg.music_api_token.is_some(),
        bpm: nonnull("bpm"),
        key: nonnull("musickey"),
        energy: nonnull("energy"),
        genres,
        spotify_id,
        tag_clouds,
        untag_action: format!("/tag-queue/{id}/untag"),
        tag_action: format!("/tag-queue/{id}/tag"),
        htmx: true,
        playlists,
        my_tags,
        my_groups,
        recommended,
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
    /// Optional group (free text) to place the new tag into, e.g. "Mood".
    #[serde(default)]
    group: Option<String>,
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
            if let Some(g) = f.group.as_deref().map(str::trim).filter(|g| !g.is_empty()) {
                if let Ok(gid) = crate::tags::create_group(&st.pool, nav.id, g, "").await {
                    let _ = crate::tags::add_tag_to_group(&st.pool, nav.id, tag_id, gid).await;
                }
            }
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
    match crate::music_api::file_bytes_any(&st.cfg, &isrc, "128").await {
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
