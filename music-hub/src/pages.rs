//! Browse pages: search, user profiles, and playlist detail.
//!
//! Every page requires a session and links into the track detail page
//! (`/track/{id}`). Server-rendered with askama, styled by Pico.css via
//! `base.html`.

use std::collections::{HashMap, HashSet};

use askama::Template;
use axum::Router;
use axum::extract::{Form, Multipart, Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use sqlx::{QueryBuilder, Row};

use crate::api::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/search", get(search_page))
        .route("/overlap", get(overlap_page))
        .route("/overlap/enrich", post(overlap_enrich))
        .route("/playlists/similar", get(similar_page))
        .route("/tags", get(tags_page))
        .route("/backpacks", get(backpacks_page))
        .route("/backpacks/active/clear", post(backpack_active_clear))
        .route("/backpacks/create", post(backpack_create))
        .route("/backpacks/{id}/rename", post(backpack_rename))
        .route("/backpacks/{id}/delete", post(backpack_delete))
        .route("/backpacks/{id}/active", post(backpack_toggle_active))
        .route("/backpacks/{id}/export", get(backpack_export))
        .route("/tags/create", post(tag_create))
        .route("/tag-search", get(tag_search_page))
        .route("/revisit", get(revisit_page))
        .route("/tag/{id}", get(tag_detail_page))
        .route("/tag/{id}/rename", post(tag_rename))
        .route("/tag/{id}/type", post(tag_set_type))
        .route("/tag/{id}/parent/add", post(tag_parent_add))
        .route("/tag/{id}/parent/remove", post(tag_parent_remove))
        .route("/tag/{id}/sync", post(tag_set_sync))
        .route("/tag/{id}/source/keep", post(tag_source_keep))
        .route("/collectives", get(collectives_page))
        .route("/collectives/create", post(collective_create))
        .route("/collectives/{id}", get(collective_page))
        .route("/collectives/{id}/update", post(collective_update))
        .route("/collectives/{id}/join", post(collective_join))
        .route("/collectives/{id}/leave", post(collective_leave))
        .route("/collectives/{id}/member", post(collective_set_member))
        .route(
            "/collectives/{id}/member/remove",
            post(collective_remove_member),
        )
        .route("/collectives/{id}/delete", post(collective_delete))
        .route("/digging", get(digging_page))
        .route("/digging/enrich", post(digging_enrich))
        .route("/admin", get(admin_page).post(admin_save))
        .route("/api/hub/status/panel", get(status_panel))
        .route("/settings", get(settings_page))
        .route("/import", get(import_page).post(import_submit))
        .route("/import/traktor", post(import_traktor_submit))
        .route("/user/{slug}", get(user_page))
        .route("/playlist/{id}", get(playlist_page))
        .route("/playlist/{id}/tag", post(playlist_tag))
        .route("/playlist/{id}/tag/add", post(playlist_tag_add))
        .route("/playlist/{id}/tag/remove", post(playlist_tag_remove))
        .route("/playlist/{id}/order", post(playlist_order))
        .route("/playlist/{id}/prioritize", post(playlist_prioritize))
        .route("/playlist/{id}/refresh", post(playlist_refresh))
        .route("/playlist/{id}/progress", get(playlist_progress))
        .route("/playlist/{id}/download", get(playlist_download))
        .route("/tag/{id}/order", post(tag_order))
        .route("/tag/{id}/download", get(tag_download))
        .route("/tag/{id}/progress", get(tag_progress))
        .route("/track/{id}/download", get(track_download))
        .route("/downloads", get(downloads_page))
        .route("/downloads/refresh", post(downloads_refresh))
        .route("/downloads/order-all", post(downloads_order_all))
        .with_state(state)
}

/// Render a template, mapping a render failure to a 500 with a short message.
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

fn not_found(msg: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        Html(format!(
            "<!doctype html><html><body><h1>404</h1><p>{msg}</p><p><a href=\"/\">Zurück</a></p></body></html>"
        )),
    )
        .into_response()
}

/// One pickable emoji icon with a precomputed `selected` flag — keeps the
/// askama templates free of `==` comparisons inside HTML tags.
struct IconOption {
    value: &'static str,
    selected: bool,
}

fn icon_options(current: &str) -> Vec<IconOption> {
    crate::tags::ICONS
        .iter()
        .map(|i| IconOption {
            value: i,
            selected: *i == current,
        })
        .collect()
}

/// Engine weight of a raw tag-type string (`0.0` for untyped tags), read from
/// the configurable engine settings.
fn type_weight(engine: &crate::settings::Engine, type_value: &str) -> f64 {
    engine.type_weight(type_value)
}

/// A lightweight parent/child tag reference for templates.
struct ParentRef {
    id: i64,
    name: String,
}

// ── search ──────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "search.html")]
struct SearchPage {
    nav: crate::ui::Nav,
    flash: String,
    q: String,
    tracks: Vec<SearchRow>,
    s_title: crate::table::SortHead,
    s_artists: crate::table::SortHead,
    s_album: crate::table::SortHead,
}

struct SearchRow {
    id: i64,
    title: String,
    artists: String,
    artists_html: String,
    album: String,
}

#[derive(Deserialize, Default)]
struct SearchQuery {
    q: Option<String>,
    sort: Option<String>,
    dir: Option<String>,
}

async fn run_search(
    pool: &sqlx::SqlitePool,
    q: &str,
    sort: &str,
    dir: &str,
    allowed: &[(&str, &str)],
    strict: bool,
    limit: i64,
) -> Vec<SearchRow> {
    let mut qb: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new(
        "SELECT * FROM (SELECT id, title, artists, album FROM hub_tracks) WHERE 1 = 1",
    );
    if strict {
        crate::table::push_fuzzy(&mut qb, q, &["title", "artists", "album"]);
    }
    crate::table::order_by(&mut qb, sort, dir, allowed, "artists");
    qb.push(" LIMIT ").push_bind(limit);
    qb.build()
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| SearchRow {
            id: r.get("id"),
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            artists: r.get::<Option<String>, _>("artists").unwrap_or_default(),
            artists_html: String::new(),
            album: r.get::<Option<String>, _>("album").unwrap_or_default(),
        })
        .collect()
}

async fn search_page(
    State(st): State<AppState>,
    Query(params): Query<SearchQuery>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "search").await else {
        return Redirect::to("/login").into_response();
    };

    let q = params.q.unwrap_or_default().trim().to_string();
    let allowed: &[(&str, &str)] = &[
        ("title", "title"),
        ("artists", "artists"),
        ("album", "album"),
    ];
    let (sort, dir) = crate::table::resolve_sort(
        params.sort.as_deref(),
        params.dir.as_deref(),
        allowed,
        "artists",
        "asc",
    );
    let mut tracks = Vec::new();

    if !q.is_empty() {
        // Strict token match first; on no hits, fall back to a broad pass with
        // typo tolerance (order-independent, one edit per token).
        tracks = run_search(&st.pool, &q, &sort, &dir, allowed, true, 100).await;
        if tracks.is_empty() {
            let toks = crate::table::tokens(&q);
            tracks = run_search(&st.pool, &q, &sort, &dir, allowed, false, 5000).await;
            tracks.retain(|r| {
                crate::table::matches_typo(
                    &format!("{} {} {}", r.title, r.artists, r.album).to_lowercase(),
                    &toks,
                )
            });
            tracks.truncate(100);
        }
    }

    // Link artist names (id-based) for the result rows.
    let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    let mut html = crate::artists::artists_html_for_tracks(&st.pool, &ids).await;
    for t in tracks.iter_mut() {
        t.artists_html = html.remove(&t.id).unwrap_or_default();
    }

    let rawq = raw.as_deref().unwrap_or("");
    let s_title = crate::table::sort_head(rawq, "title", "Titel", &sort, &dir);
    let s_artists = crate::table::sort_head(rawq, "artists", "Artists", &sort, &dir);
    let s_album = crate::table::sort_head(rawq, "album", "Album", &sort, &dir);
    render(&SearchPage {
        nav,
        flash: String::new(),
        q,
        tracks,
        s_title,
        s_artists,
        s_album,
    })
}

// ── user profile ────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "user.html")]
struct UserPage {
    nav: crate::ui::Nav,
    flash: String,
    slug: String,
    liked_count: i64,
    playlist_count: i64,
    q: String,
    playlists: Vec<PlaylistRow>,
    s_name: crate::table::SortHead,
    s_owner: crate::table::SortHead,
    s_tracks: crate::table::SortHead,
}

#[derive(Deserialize, Default)]
struct UserFilter {
    q: Option<String>,
    sort: Option<String>,
    dir: Option<String>,
}

struct PlaylistRow {
    id: i64,
    name: String,
    owner: String,
    track_count: String,
    items_available: bool,
}

async fn user_page(
    State(st): State<AppState>,
    Path(slug): Path<String>,
    Query(f): Query<UserFilter>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "user").await else {
        return Redirect::to("/login").into_response();
    };

    let user = sqlx::query_as::<_, (i64, String)>(
        "SELECT id, slug FROM hub_users WHERE slug = ?1 COLLATE NOCASE",
    )
    .bind(&slug)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten();
    let Some((user_id, slug)) = user else {
        return not_found("User nicht gefunden.");
    };

    let liked_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_liked_tracks WHERE user_id = ?1")
            .bind(user_id)
            .fetch_one(&st.pool)
            .await
            .unwrap_or(0);

    let q = f.q.unwrap_or_default().trim().to_string();
    let allowed: &[(&str, &str)] = &[
        ("name", "name"),
        ("owner", "owner"),
        ("tracks", "track_count"),
    ];
    let (sort, dir) =
        crate::table::resolve_sort(f.sort.as_deref(), f.dir.as_deref(), allowed, "name", "asc");
    let mut qb: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new(
        "SELECT * FROM (SELECT p.id AS id, p.name AS name,
                COALESCE(NULLIF(p.owner_name, ''), CASE WHEN p.is_owned = 1 THEN a.display_name END) AS owner,
                p.track_count AS track_count, COALESCE(p.items_available, 0) AS items_available
           FROM hub_playlists p
           LEFT JOIN hub_service_accounts a ON a.user_id = p.user_id AND a.service = 'spotify'
          WHERE p.user_id = ",
    );
    qb.push_bind(user_id);
    qb.push(") WHERE 1 = 1");
    crate::table::push_fuzzy(&mut qb, &q, &["name", "owner"]);
    crate::table::order_by(&mut qb, &sort, &dir, allowed, "name");

    let playlists: Vec<PlaylistRow> = qb
        .build()
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| PlaylistRow {
            id: r.get("id"),
            name: r.get::<Option<String>, _>("name").unwrap_or_default(),
            owner: r
                .get::<Option<String>, _>("owner")
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "—".to_string()),
            track_count: r
                .get::<Option<i64>, _>("track_count")
                .map(|c| c.to_string())
                .unwrap_or_else(|| "—".to_string()),
            items_available: r.get::<Option<i64>, _>("items_available").unwrap_or(0) != 0,
        })
        .collect();

    let playlist_count = playlists.len() as i64;
    let rawq = raw.as_deref().unwrap_or("");
    let s_name = crate::table::sort_head(rawq, "name", "Playlist", &sort, &dir);
    let s_owner = crate::table::sort_head(rawq, "owner", "Besitzer", &sort, &dir);
    let s_tracks = crate::table::sort_head(rawq, "tracks", "Tracks", &sort, &dir);

    render(&UserPage {
        nav,
        flash: String::new(),
        slug,
        liked_count,
        playlist_count,
        q,
        playlists,
        s_name,
        s_owner,
        s_tracks,
    })
}

// ── playlist detail ─────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "playlist.html")]
struct PlaylistPage {
    nav: crate::ui::Nav,
    flash: String,
    id: i64,
    name: String,
    owner: String,
    owner_name: String,
    q: String,
    tracks: Vec<PlaylistTrackRow>,
    /// Tags this playlist currently feeds.
    feeds: Vec<TagFeed>,
    /// The current user's tags, for the picker.
    my_tags: Vec<TagOption>,
    /// Whether the music-api service is configured (download/order buttons).
    music_api: bool,
    ready: usize,
    total: usize,
    s_pos: crate::table::SortHead,
    s_title: crate::table::SortHead,
}

struct TagFeed {
    id: i64,
    name: String,
    owner: String,
}

struct TagOption {
    id: i64,
    name: String,
}

struct PlaylistTrackRow {
    position: String,
    id: i64,
    title: String,
    artists_html: String,
}

#[derive(Deserialize, Default)]
struct PlaylistTableFilter {
    q: Option<String>,
    sort: Option<String>,
    dir: Option<String>,
    msg: Option<String>,
}

async fn playlist_page(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Query(pf): Query<PlaylistTableFilter>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "playlist").await else {
        return Redirect::to("/login").into_response();
    };

    let playlist = sqlx::query_as::<_, (Option<String>, Option<String>, Option<String>)>(
        "SELECT p.name, u.slug,
                COALESCE(NULLIF(p.owner_name, ''), CASE WHEN p.is_owned = 1 THEN a.display_name END) AS owner_name
           FROM hub_playlists p
           JOIN hub_users u ON u.id = p.user_id
           LEFT JOIN hub_service_accounts a ON a.user_id = p.user_id AND a.service = 'spotify'
          WHERE p.id = ?1",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten();
    let Some((name, owner, owner_name)) = playlist else {
        return not_found("Playlist nicht gefunden.");
    };

    let me = crate::web::current_user(&st, &headers).await;
    let feeds: Vec<TagFeed> = crate::tags::tags_feeding_playlist(&st.pool, id)
        .await
        .into_iter()
        .map(|(id, name, owner)| TagFeed { id, name, owner })
        .collect();
    let my_tags: Vec<TagOption> = match &me {
        Some((uid, _)) => crate::tags::list_user_tags(&st.pool, *uid)
            .await
            .into_iter()
            .map(|(id, name)| TagOption { id, name })
            .collect(),
        None => Vec::new(),
    };

    let q = pf.q.unwrap_or_default().trim().to_string();
    let allowed: &[(&str, &str)] = &[
        ("pos", "position"),
        ("title", "title"),
        ("artists", "artists"),
    ];
    let (sort, dir) =
        crate::table::resolve_sort(pf.sort.as_deref(), pf.dir.as_deref(), allowed, "pos", "asc");
    let mut qb: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new(
        "SELECT * FROM (SELECT t.id AS id, hpt.position AS position, t.title AS title, t.artists AS artists
           FROM hub_playlist_tracks hpt
           JOIN hub_tracks t ON t.id = hpt.track_id
          WHERE hpt.playlist_id = ",
    );
    qb.push_bind(id);
    qb.push(") WHERE 1 = 1");
    crate::table::push_fuzzy(&mut qb, &q, &["title", "artists"]);
    crate::table::order_by(&mut qb, &sort, &dir, allowed, "pos");

    let mut tracks: Vec<PlaylistTrackRow> = qb
        .build()
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| PlaylistTrackRow {
            position: r
                .get::<Option<i64>, _>("position")
                .map(|p| p.to_string())
                .unwrap_or_else(|| "—".to_string()),
            id: r.get("id"),
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            artists_html: String::new(),
        })
        .collect();
    let track_ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    let mut track_html = crate::artists::artists_html_for_tracks(&st.pool, &track_ids).await;
    for t in tracks.iter_mut() {
        t.artists_html = track_html.remove(&t.id).unwrap_or_default();
    }

    let isrcs: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT t.isrc FROM hub_playlist_tracks hpt
           JOIN hub_tracks t ON t.id = hpt.track_id
          WHERE hpt.playlist_id = ?1 AND t.isrc IS NOT NULL AND t.isrc <> ''",
    )
    .bind(id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();
    let total = isrcs.len();
    let (ready, _) = crate::music_api::cached_counts(&st.pool, &isrcs).await;

    let rawq = raw.as_deref().unwrap_or("");
    let s_pos = crate::table::sort_head(rawq, "pos", "#", &sort, &dir);
    let s_title = crate::table::sort_head(rawq, "title", "Titel", &sort, &dir);
    render(&PlaylistPage {
        nav,
        flash: pf.msg.clone().unwrap_or_default(),
        id,
        name: name.unwrap_or_default(),
        owner: owner.unwrap_or_default(),
        owner_name: owner_name.filter(|s| !s.is_empty()).unwrap_or_default(),
        q,
        tracks,
        feeds,
        my_tags,
        music_api: st.cfg.music_api_token.is_some(),
        ready,
        total,
        s_pos,
        s_title,
    })
}

#[derive(Deserialize)]
struct DownloadQuery {
    format: Option<String>,
}

fn sanitize_filename(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// Build a ZIP of a playlist's tracks that `music-api` can currently serve.
async fn build_playlist_zip(
    cfg: &crate::config::Config,
    tracks: &[sqlx::sqlite::SqliteRow],
    format: &str,
    path: &std::path::Path,
) -> anyhow::Result<usize> {
    use std::io::Write;
    let file = std::fs::File::create(path)?;
    let mut zw = zip::ZipWriter::new(file);
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut used: HashSet<String> = HashSet::new();
    let mut n = 0usize;
    for r in tracks {
        let isrc = r
            .get::<Option<String>, _>("isrc")
            .unwrap_or_default()
            .trim()
            .to_string();
        if isrc.is_empty() {
            continue;
        }
        let bytes = match crate::music_api::file_bytes_any(cfg, &isrc, format).await {
            Ok(b) => b,
            Err(_) => continue, // not downloaded / not in ledger yet
        };
        let artists = r.get::<Option<String>, _>("artists").unwrap_or_default();
        let title = r.get::<Option<String>, _>("title").unwrap_or_default();
        let base = sanitize_filename(&format!("{artists} - {title}"));
        let base = if base.is_empty() { isrc.clone() } else { base };
        let mut name = format!("{base}.{format}");
        let mut i = 2;
        while used.contains(&name) {
            name = format!("{base} ({i}).{format}");
            i += 1;
        }
        used.insert(name.clone());
        zw.start_file(name, opts)?;
        zw.write_all(&bytes)?;
        n += 1;
    }
    zw.finish()?;
    Ok(n)
}

/// Stream a ZIP temp file as an attachment, unlinking it before the stream (the
/// open fd stays valid on Linux).
async fn zip_response(tmp: std::path::PathBuf, filename: String) -> Response {
    let file = match tokio::fs::File::open(&tmp).await {
        Ok(f) => f,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("open: {e}")).into_response(),
    };
    let _ = std::fs::remove_file(&tmp);
    let stream = tokio_util::io::ReaderStream::new(file);
    Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "application/zip")
        .header(
            axum::http::header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .body(axum::body::Body::from_stream(stream))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Collect the `music_api` status states of a set of ISRCs from cache; drives
/// the progress indicators without hitting the network.
async fn progress_counts(st: &AppState, isrcs: &[String]) -> (usize, usize) {
    let (ready, _) = crate::music_api::cached_counts(&st.pool, isrcs).await;
    (ready, isrcs.len())
}

fn rows_isrcs(rows: &[sqlx::sqlite::SqliteRow]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for r in rows {
        let isrc = r
            .get::<Option<String>, _>("isrc")
            .unwrap_or_default()
            .trim()
            .to_string();
        if !isrc.is_empty() && seen.insert(isrc.clone()) {
            out.push(isrc);
        }
    }
    out
}

const PL_TRACKS_SQL: &str = "SELECT t.isrc, t.title, t.artists FROM hub_playlist_tracks hpt
     JOIN hub_tracks t ON t.id = hpt.track_id WHERE hpt.playlist_id = ?1 ORDER BY hpt.position";
const TAG_TRACKS_SQL: &str = "SELECT t.isrc, t.title, t.artists FROM hub_track_resolved_tags rt
     JOIN hub_tracks t ON t.id = rt.track_id WHERE rt.tag_id = ?1 ORDER BY t.artists, t.title";

/// Order only the ISRCs that music-api does **not** already have (refreshes the
/// cache first). Returns a flash message.)
async fn order_missing(
    pool: &sqlx::SqlitePool,
    cfg: &crate::config::Config,
    isrcs: &[String],
) -> String {
    if cfg.music_api_token.is_none() {
        return "music-api nicht konfiguriert".to_string();
    }
    if isrcs.is_empty() {
        return "Keine ISRCs vorhanden".to_string();
    }
    let _ = crate::music_api::refresh_states(pool, cfg, isrcs, 300).await;
    let missing = crate::music_api::missing_isrcs(pool, isrcs).await;
    if missing.is_empty() {
        return "Alles bereits vorhanden".to_string();
    }
    match crate::music_api::order(cfg, &missing).await {
        Ok(oid) => format!("{} fehlende bestellt (Order {oid})", missing.len()),
        Err(e) => format!("music-api Fehler: {e}"),
    }
}

/// `GET /playlist/{id}/download[?format=flac]` — stream a ZIP of the playlist's
/// music-api-ready tracks.
async fn playlist_download(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<DownloadQuery>,
    headers: HeaderMap,
) -> Response {
    if crate::ui::nav(&st, &headers, "playlist").await.is_none() {
        return Redirect::to("/login").into_response();
    }
    let format = valid_format(q.format);
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM hub_playlists WHERE id = ?1")
        .bind(id)
        .fetch_optional(&st.pool)
        .await
        .ok()
        .flatten();
    let Some(name) = name else {
        return not_found("Playlist nicht gefunden.");
    };
    let tracks = sqlx::query(PL_TRACKS_SQL)
        .bind(id)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    zip_for(&st, &tracks, &format, &sanitize_filename(&name)).await
}

/// `GET /tag/{id}/download[?format=flac]` — stream a ZIP of the tag's tracks.
async fn tag_download(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<DownloadQuery>,
    headers: HeaderMap,
) -> Response {
    if crate::ui::nav(&st, &headers, "tags").await.is_none() {
        return Redirect::to("/login").into_response();
    }
    let format = valid_format(q.format);
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM hub_tags WHERE id = ?1")
        .bind(id)
        .fetch_optional(&st.pool)
        .await
        .ok()
        .flatten();
    let Some(name) = name else {
        return not_found("Tag nicht gefunden.");
    };
    let tracks = sqlx::query(TAG_TRACKS_SQL)
        .bind(id)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    zip_for(&st, &tracks, &format, &sanitize_filename(&name)).await
}

/// Build the ZIP and return it (or a helpful error).
async fn zip_for(
    st: &AppState,
    tracks: &[sqlx::sqlite::SqliteRow],
    format: &str,
    label: &str,
) -> Response {
    let tmp = std::env::temp_dir().join(format!("hub-{}.zip", crate::spotify::random_token()));
    let count = match build_playlist_zip(&st.cfg, tracks, format, &tmp).await {
        Ok(n) => n,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, format!("ZIP: {e}")).into_response(),
    };
    if count == 0 {
        let _ = std::fs::remove_file(&tmp);
        return (
            StatusCode::BAD_GATEWAY,
            "Keine Dateien über music-api verfügbar (erst ordern).",
        )
            .into_response();
    }
    zip_response(tmp, format!("{label}.zip")).await
}

fn valid_format(f: Option<String>) -> String {
    match f {
        Some(f) if !f.is_empty() && f.chars().all(|c| c.is_ascii_alphanumeric()) => f,
        _ => "flac".to_string(),
    }
}

/// Build a small text/XML body as a download attachment.
fn attachment(body: String, content_type: &str, filename: String) -> Response {
    Response::builder()
        .header(axum::http::header::CONTENT_TYPE, content_type)
        .header(
            axum::http::header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .body(axum::body::Body::from(body))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// `POST /playlist/{id}/order` — order only the playlist's missing ISRCs.
async fn playlist_order(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some((_uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let tracks = sqlx::query(PL_TRACKS_SQL)
        .bind(id)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    let msg = order_missing(&st.pool, &st.cfg, &rows_isrcs(&tracks)).await;
    flash_redirect(&format!("/playlist/{id}"), msg)
}

/// Priority bump for the whole-playlist queue (`higher = earlier`).
const PLAYLIST_PRIORITY: i32 = 10;

/// `POST /playlist/{id}/prioritize` — (re-)order every ISRC of the playlist with
/// an elevated priority so music-api downloads it ahead of unprompted orders.
async fn playlist_prioritize(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some((_uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let target = format!("/playlist/{id}");
    if st.cfg.music_api_token.is_none() {
        return flash_redirect(&target, "music-api nicht konfiguriert".to_string());
    }
    let tracks = sqlx::query(PL_TRACKS_SQL)
        .bind(id)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    let isrcs = rows_isrcs(&tracks);
    if isrcs.is_empty() {
        return flash_redirect(&target, "Keine ISRCs vorhanden".to_string());
    }
    let msg = match crate::music_api::order_with_priority(&st.cfg, &isrcs, PLAYLIST_PRIORITY).await
    {
        Ok(oid) => format!("{} priorisiert (Order {oid})", isrcs.len()),
        Err(e) => format!("music-api Fehler: {e}"),
    };
    flash_redirect(&target, msg)
}

/// `POST /tag/{id}/order` — order only the tag's missing ISRCs.
async fn tag_order(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some((_uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let tracks = sqlx::query(TAG_TRACKS_SQL)
        .bind(id)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    let msg = order_missing(&st.pool, &st.cfg, &rows_isrcs(&tracks)).await;
    flash_redirect(&format!("/tag/{id}"), msg)
}

/// `POST /playlist/{id}/refresh` — refresh the music-api state cache.
async fn playlist_refresh(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some((_uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let tracks = sqlx::query(PL_TRACKS_SQL)
        .bind(id)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    let isrcs = rows_isrcs(&tracks);
    let (ready, total, _) = crate::music_api::refresh_states(&st.pool, &st.cfg, &isrcs, 300).await;
    flash_redirect(
        &format!("/playlist/{id}"),
        format!("{ready}/{total} bereit"),
    )
}

/// `GET /playlist/{id}/progress` — cached ready/total (htmx-pollable).
async fn playlist_progress(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if crate::ui::nav(&st, &headers, "playlist").await.is_none() {
        return Redirect::to("/login").into_response();
    }
    let tracks = sqlx::query(PL_TRACKS_SQL)
        .bind(id)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    let (ready, total) = progress_counts(&st, &rows_isrcs(&tracks)).await;
    format!("{ready}/{total} bereit").into_response()
}

/// `GET /tag/{id}/progress` — cached ready/total (htmx-pollable).
async fn tag_progress(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if crate::ui::nav(&st, &headers, "tags").await.is_none() {
        return Redirect::to("/login").into_response();
    }
    let tracks = sqlx::query(TAG_TRACKS_SQL)
        .bind(id)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    let (ready, total) = progress_counts(&st, &rows_isrcs(&tracks)).await;
    format!("{ready}/{total} bereit").into_response()
}

/// `GET /track/{id}/download[?format=flac]` — stream a single track's file.
async fn track_download(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<DownloadQuery>,
    headers: HeaderMap,
) -> Response {
    if crate::ui::nav(&st, &headers, "track").await.is_none() {
        return Redirect::to("/login").into_response();
    }
    let format = valid_format(q.format);
    let row = sqlx::query("SELECT t.isrc, t.title, t.artists FROM hub_tracks t WHERE t.id = ?1")
        .bind(id)
        .fetch_optional(&st.pool)
        .await
        .ok()
        .flatten();
    let Some(r) = row else {
        return not_found("Track nicht gefunden.");
    };
    let isrc = r
        .get::<Option<String>, _>("isrc")
        .unwrap_or_default()
        .trim()
        .to_string();
    if isrc.is_empty() {
        return (StatusCode::BAD_REQUEST, "Track hat keine ISRC").into_response();
    }
    match crate::music_api::file_bytes_any(&st.cfg, &isrc, &format).await {
        Ok(bytes) => {
            let artists = r.get::<Option<String>, _>("artists").unwrap_or_default();
            let title = r.get::<Option<String>, _>("title").unwrap_or_default();
            let filename = sanitize_filename(&format!("{artists} - {title}.{format}"));
            let ct = audio_content_type(&format);
            Response::builder()
                .header(axum::http::header::CONTENT_TYPE, ct)
                .header(
                    axum::http::header::CONTENT_DISPOSITION,
                    format!("attachment; filename=\"{filename}\""),
                )
                .body(axum::body::Body::from(bytes))
                .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            format!("music-api: nicht verfügbar (erst ordern): {e}"),
        )
            .into_response(),
    }
}

fn audio_content_type(format: &str) -> &'static str {
    match format {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "m4a" | "aac" => "audio/mp4",
        "ogg" => "audio/ogg",
        _ => "audio/flac",
    }
}

// ── downloads (music-api queue + cached state overview + all-tracks table) ──

/// Rows per page in the all-tracks table.
const DOWNLOADS_PER_PAGE: i64 = 100;
/// Cap on how many ISRCs a single "order all" invocation will order.
const DOWNLOADS_ORDER_CAP: usize = 500;

/// `FROM`/`WHERE` shared by the count, page and ISRC-list queries. Only tracks
/// that actually carry an ISRC can be matched against the music-api cache.
const DOWNLOADS_FROM: &str = " FROM hub_tracks t LEFT JOIN hub_music_state ms ON ms.isrc = t.isrc \
     WHERE t.isrc IS NOT NULL AND TRIM(t.isrc) <> ''";

#[derive(Deserialize, Default)]
struct DownloadsQuery {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    q: Option<String>,
    #[serde(default, deserialize_with = "crate::table::qnum::opt_i64")]
    page: Option<i64>,
    #[serde(default)]
    sort: Option<String>,
    #[serde(default)]
    dir: Option<String>,
    #[serde(default)]
    msg: Option<String>,
}

/// Normalized, validated filter state for the downloads table.
struct DownloadFilters {
    status: String,
    format: String,
    q: String,
    page: i64,
    sort: String,
    dir: String,
}

/// Whitelisted sort fields for the downloads table (`field -> SQL expr`).
const DOWNLOAD_SORTS: &[(&str, &str)] = &[
    ("title", "t.title"),
    ("artists", "t.artists"),
    ("isrc", "t.isrc"),
    ("state", "COALESCE(ms.state, 'unknown')"),
    ("format", "ms.source_format"),
];

impl DownloadFilters {
    fn from_query(q: &DownloadsQuery) -> Self {
        let status = match q.status.as_deref() {
            Some(s @ ("ready" | "pending" | "downloading" | "absent" | "failed" | "unknown")) => {
                s.to_string()
            }
            _ => "all".to_string(),
        };
        let format = match q.format.as_deref() {
            Some(f @ ("flac" | "320" | "128")) => f.to_string(),
            _ => "all".to_string(),
        };
        let page = q.page.unwrap_or(1).max(1);
        let text = q.q.clone().unwrap_or_default().trim().to_string();
        let sort = q.sort.clone().unwrap_or_default().trim().to_string();
        let dir = match q.dir.as_deref() {
            Some("desc") => "desc".to_string(),
            _ => "asc".to_string(),
        };
        Self {
            status,
            format,
            q: text,
            page,
            sort,
            dir,
        }
    }

    /// Query string of the active (non-default) filters, without `page`.
    fn query_string(&self) -> String {
        let mut parts = Vec::new();
        if self.status != "all" {
            parts.push(format!("status={}", urlencoding::encode(&self.status)));
        }
        if self.format != "all" {
            parts.push(format!("format={}", urlencoding::encode(&self.format)));
        }
        if !self.q.is_empty() {
            parts.push(format!("q={}", urlencoding::encode(&self.q)));
        }
        if !self.sort.is_empty() {
            parts.push(format!("sort={}", urlencoding::encode(&self.sort)));
            parts.push(format!("dir={}", urlencoding::encode(&self.dir)));
        }
        parts.join("&")
    }

    fn has_filters(&self) -> bool {
        self.status != "all" || self.format != "all" || !self.q.is_empty()
    }
}

/// Append a filter query string to a base URL, picking the right separator.
fn append_qs(base: &str, qs: &str) -> String {
    if qs.is_empty() {
        base.to_string()
    } else if base.contains('?') {
        format!("{base}&{qs}")
    } else {
        format!("{base}?{qs}")
    }
}

/// Append the `WHERE` conditions for the downloads table to a query builder.
/// Format filtering mirrors `music_api::format_bucket` (flac > 320 > 128).
fn push_download_filters(qb: &mut QueryBuilder<'_, sqlx::Sqlite>, f: &DownloadFilters) {
    if f.status != "all" {
        if f.status == "unknown" {
            qb.push(" AND (ms.state IS NULL OR ms.state = 'unknown')");
        } else {
            qb.push(" AND ms.state = ").push_bind(f.status.clone());
        }
    }
    if f.format != "all" {
        let hay = "LOWER(COALESCE(ms.source_format, '') || ' ' || COALESCE(ms.formats, ''))";
        match f.format.as_str() {
            "flac" => {
                qb.push(format!(" AND {hay} LIKE '%flac%'"));
            }
            "320" => {
                qb.push(format!(
                    " AND {hay} NOT LIKE '%flac%' AND {hay} LIKE '%320%'"
                ));
            }
            "128" => {
                qb.push(format!(
                    " AND {hay} NOT LIKE '%flac%' AND {hay} NOT LIKE '%320%' AND {hay} LIKE '%128%'"
                ));
            }
            _ => {}
        }
    }
    if !f.q.is_empty() {
        crate::table::push_fuzzy(
            qb,
            &f.q,
            &[
                "t.title",
                "t.artists",
                "t.isrc",
                "ms.state",
                "ms.source_format",
            ],
        );
    }
}

/// Count the tracks matching the current filters.
async fn download_count(pool: &sqlx::SqlitePool, f: &DownloadFilters) -> i64 {
    let mut qb = QueryBuilder::new(format!("SELECT COUNT(*){DOWNLOADS_FROM}"));
    push_download_filters(&mut qb, f);
    qb.build_query_scalar::<i64>()
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

/// One page of the all-tracks table, ordered so actionable (non-ready) rows
/// come first, then by artist/title/ISRC for a stable order.
async fn download_rows(
    pool: &sqlx::SqlitePool,
    f: &DownloadFilters,
    page: i64,
) -> Vec<DownloadTrackRow> {
    let offset = (page - 1).max(0) * DOWNLOADS_PER_PAGE;
    let mut qb = QueryBuilder::new(format!(
        "SELECT t.id AS track_id, t.isrc AS isrc, COALESCE(t.title, '') AS title, \
         t.service AS service, \
         COALESCE(ms.state, 'unknown') AS state, \
         COALESCE(ms.source_format, '') AS source_format, \
         COALESCE(ms.deezer_id, '') AS deezer_id, \
         COALESCE(ms.error, '') AS error{DOWNLOADS_FROM}"
    ));
    push_download_filters(&mut qb, f);
    if f.sort.is_empty() {
        qb.push(
            " ORDER BY CASE COALESCE(ms.state, 'unknown') \
             WHEN 'unknown' THEN 0 WHEN 'absent' THEN 1 WHEN 'failed' THEN 2 \
             WHEN 'pending' THEN 3 WHEN 'downloading' THEN 4 WHEN 'ready' THEN 5 ELSE 6 END, \
             t.artists, t.title, t.isrc",
        );
    } else {
        crate::table::order_by(&mut qb, &f.sort, &f.dir, DOWNLOAD_SORTS, "title");
    }
    qb.push(" LIMIT ").push_bind(DOWNLOADS_PER_PAGE);
    qb.push(" OFFSET ").push_bind(offset);
    let mut rows: Vec<DownloadTrackRow> = qb
        .build()
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| DownloadTrackRow {
            track_id: r.get::<Option<i64>, _>("track_id").unwrap_or(0),
            isrc: r.get::<Option<String>, _>("isrc").unwrap_or_default(),
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            artists_html: String::new(),
            service: r.get::<Option<String>, _>("service").unwrap_or_default(),
            state: r.get::<Option<String>, _>("state").unwrap_or_default(),
            source_format: r
                .get::<Option<String>, _>("source_format")
                .unwrap_or_default(),
            deezer_id: r.get::<Option<String>, _>("deezer_id").unwrap_or_default(),
            error: r.get::<Option<String>, _>("error").unwrap_or_default(),
        })
        .collect();
    let ids: Vec<i64> = rows.iter().map(|r| r.track_id).collect();
    let mut html = crate::artists::artists_html_for_tracks(pool, &ids).await;
    for r in rows.iter_mut() {
        r.artists_html = html.remove(&r.track_id).unwrap_or_default();
    }
    rows
}

/// Distinct ISRCs matching the current filters (for order/refresh actions).
async fn filtered_isrcs(pool: &sqlx::SqlitePool, f: &DownloadFilters) -> Vec<String> {
    let mut qb = QueryBuilder::new(format!("SELECT t.isrc AS isrc{DOWNLOADS_FROM}"));
    push_download_filters(&mut qb, f);
    qb.push(" ORDER BY t.isrc");
    let rows = qb.build().fetch_all(pool).await.unwrap_or_default();
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for r in rows {
        let isrc = r
            .get::<Option<String>, _>("isrc")
            .unwrap_or_default()
            .trim()
            .to_string();
        if !isrc.is_empty() && seen.insert(isrc.clone()) {
            out.push(isrc);
        }
    }
    out
}

/// Windowed page links (up to 5 numbers) around the current page.
fn page_links(f: &DownloadFilters, pages: i64) -> Vec<PageLink> {
    let qs = f.query_string();
    let href = |p: i64| append_qs(&format!("/downloads?page={p}"), &qs);
    let mut out = Vec::new();
    if pages < 1 {
        return out;
    }
    let start = (f.page - 2).max(1);
    let end = (start + 4).min(pages);
    let start = (end - 4).max(1);
    for p in start..=end {
        out.push(PageLink {
            href: href(p),
            label: p.to_string(),
            active: p == f.page,
        });
    }
    out
}

fn status_options(current: &str) -> Vec<FilterOption> {
    [
        ("all", "Alle"),
        ("ready", "ready"),
        ("pending", "pending"),
        ("downloading", "downloading"),
        ("absent", "absent"),
        ("failed", "failed"),
        ("unknown", "unknown"),
    ]
    .iter()
    .map(|(v, l)| FilterOption {
        value: v.to_string(),
        label: l.to_string(),
        selected: *v == current,
    })
    .collect()
}

fn format_options(current: &str) -> Vec<FilterOption> {
    [
        ("all", "Alle"),
        ("flac", "FLAC"),
        ("320", "MP3 320"),
        ("128", "MP3 128"),
    ]
    .iter()
    .map(|(v, l)| FilterOption {
        value: v.to_string(),
        label: l.to_string(),
        selected: *v == current,
    })
    .collect()
}

#[derive(Template)]
#[template(path = "downloads.html")]
struct DownloadsPage {
    nav: crate::ui::Nav,
    flash: String,
    /// Whether the music-api service is configured (live queue available).
    music_api: bool,
    // Cached state summary (`hub_music_state`).
    total: usize,
    ready: usize,
    ready_flac: usize,
    ready_320: usize,
    ready_128: usize,
    ready_other: usize,
    pending: usize,
    downloading: usize,
    absent: usize,
    failed: usize,
    /// Cached rows that carry an error/reason.
    errors: usize,
    // Live music-api queue.
    queue_ok: bool,
    queue_error: String,
    queue_items: Vec<QueueItem>,
    // All-tracks table (filtered + paginated).
    rows: Vec<DownloadTrackRow>,
    row_total: i64,
    page: i64,
    pages: i64,
    page_links: Vec<PageLink>,
    prev_href: String,
    prev_disabled: bool,
    next_href: String,
    next_disabled: bool,
    status_options: Vec<FilterOption>,
    format_options: Vec<FilterOption>,
    q: String,
    has_filters: bool,
    refresh_action: String,
    order_action: String,
    s_title: crate::table::SortHead,
    s_artists: crate::table::SortHead,
    s_isrc: crate::table::SortHead,
    s_state: crate::table::SortHead,
    s_format: crate::table::SortHead,
}

struct QueueItem {
    isrc: String,
    title: String,
    artist: String,
    priority: String,
    state: String,
}

/// One row of the all-tracks table.
struct DownloadTrackRow {
    track_id: i64,
    isrc: String,
    title: String,
    artists_html: String,
    service: String,
    state: String,
    source_format: String,
    deezer_id: String,
    error: String,
}

/// A pagination link with a precomputed `active` flag (askama-safe).
struct PageLink {
    href: String,
    label: String,
    active: bool,
}

/// A `<select>` option with a precomputed `selected` flag (askama-safe).
struct FilterOption {
    value: String,
    label: String,
    selected: bool,
}

/// `GET /downloads` — cached music-api state overview, all-tracks table and
/// live queue.
async fn downloads_page(
    State(st): State<AppState>,
    Query(q): Query<DownloadsQuery>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "downloads").await else {
        return Redirect::to("/login").into_response();
    };
    let s = crate::music_api::cache_summary(&st.pool).await;
    let music_api = st.cfg.music_api_token.is_some();

    let mut queue_ok = false;
    let mut queue_error = String::new();
    let mut queue_items: Vec<QueueItem> = Vec::new();
    if music_api {
        match crate::music_api::queue(&st.cfg).await {
            Ok(v) => {
                queue_ok = true;
                queue_items = parse_queue(&v);
            }
            Err(e) => queue_error = format!("music-api nicht erreichbar: {e}"),
        }
    }

    let mut f = DownloadFilters::from_query(&q);
    let row_total = download_count(&st.pool, &f).await;
    let pages = if row_total == 0 {
        1
    } else {
        (row_total + DOWNLOADS_PER_PAGE - 1) / DOWNLOADS_PER_PAGE
    };
    f.page = f.page.min(pages).max(1);
    let rows = download_rows(&st.pool, &f, f.page).await;
    let links = page_links(&f, pages);
    let qs = f.query_string();
    let rawq = raw.as_deref().unwrap_or("");
    let s_title = crate::table::sort_head(rawq, "title", "Track", &f.sort, &f.dir);
    let s_artists = crate::table::sort_head(rawq, "artists", "Artists", &f.sort, &f.dir);
    let s_isrc = crate::table::sort_head(rawq, "isrc", "ISRC", &f.sort, &f.dir);
    let s_state = crate::table::sort_head(rawq, "state", "Status", &f.sort, &f.dir);
    let s_format = crate::table::sort_head(rawq, "format", "Format", &f.sort, &f.dir);

    render(&DownloadsPage {
        nav,
        flash: q.msg.clone().unwrap_or_default(),
        music_api,
        total: s.total,
        ready: s.ready,
        ready_flac: s.ready_flac,
        ready_320: s.ready_320,
        ready_128: s.ready_128,
        ready_other: s.ready_other,
        pending: s.pending,
        downloading: s.downloading,
        absent: s.absent,
        failed: s.failed,
        errors: s.errors,
        queue_ok,
        queue_error,
        queue_items,
        rows,
        row_total,
        page: f.page,
        pages,
        page_links: links,
        prev_href: append_qs(&format!("/downloads?page={}", (f.page - 1).max(1)), &qs),
        prev_disabled: f.page <= 1,
        next_href: append_qs(&format!("/downloads?page={}", (f.page + 1).min(pages)), &qs),
        next_disabled: f.page >= pages,
        status_options: status_options(&f.status),
        format_options: format_options(&f.format),
        q: f.q.clone(),
        has_filters: f.has_filters(),
        refresh_action: append_qs("/downloads/refresh", &qs),
        order_action: append_qs("/downloads/order-all", &qs),
        s_title,
        s_artists,
        s_isrc,
        s_state,
        s_format,
    })
}

/// `POST /downloads/refresh` — refresh the cached music-api state for the
/// tracks matching the current filters (all tracks when unfiltered).
async fn downloads_refresh(
    State(st): State<AppState>,
    Query(q): Query<DownloadsQuery>,
    headers: HeaderMap,
) -> Response {
    if crate::ui::nav(&st, &headers, "downloads").await.is_none() {
        return Redirect::to("/login").into_response();
    }
    let f = DownloadFilters::from_query(&q);
    let target = append_qs("/downloads", &f.query_string());
    if st.cfg.music_api_token.is_none() {
        return flash_redirect(&target, "music-api nicht konfiguriert".to_string());
    }
    let isrcs = filtered_isrcs(&st.pool, &f).await;
    if isrcs.is_empty() {
        return flash_redirect(&target, "Keine Tracks für die aktuelle Auswahl".to_string());
    }
    let (ready, total, _) = crate::music_api::refresh_states(&st.pool, &st.cfg, &isrcs, 1000).await;
    flash_redirect(&target, format!("{ready}/{total} bereit"))
}

/// `POST /downloads/order-all` — order every matching track whose cached state
/// is not `ready`. The ISRC list is capped per invocation (the SQLite `IN`
/// chunking happens inside `music_api::missing_isrcs`).
async fn downloads_order_all(
    State(st): State<AppState>,
    Query(q): Query<DownloadsQuery>,
    headers: HeaderMap,
) -> Response {
    if crate::ui::nav(&st, &headers, "downloads").await.is_none() {
        return Redirect::to("/login").into_response();
    }
    let f = DownloadFilters::from_query(&q);
    let target = append_qs("/downloads", &f.query_string());
    if st.cfg.music_api_token.is_none() {
        return flash_redirect(&target, "music-api nicht konfiguriert".to_string());
    }
    let isrcs = filtered_isrcs(&st.pool, &f).await;
    if isrcs.is_empty() {
        return flash_redirect(&target, "Keine Tracks für die aktuelle Auswahl".to_string());
    }
    let matched = isrcs.len();
    let capped: Vec<String> = isrcs.into_iter().take(DOWNLOADS_ORDER_CAP).collect();
    let _ = crate::music_api::refresh_states(&st.pool, &st.cfg, &capped, 300).await;
    let missing = crate::music_api::missing_isrcs(&st.pool, &capped).await;
    if missing.is_empty() {
        return flash_redirect(&target, "Alles bereits vorhanden".to_string());
    }
    let msg = match crate::music_api::order(&st.cfg, &missing).await {
        Ok(oid) => {
            let mut m = format!("{} fehlende bestellt (Order {oid})", missing.len());
            if matched > DOWNLOADS_ORDER_CAP {
                m.push_str(&format!(
                    " — {matched} Treffer, auf {DOWNLOADS_ORDER_CAP} begrenzt"
                ));
            }
            m
        }
        Err(e) => format!("music-api Fehler: {e}"),
    };
    flash_redirect(&target, msg)
}

/// Best-effort parse of the music-api `/queue` payload. Accepts either a bare
/// array or an object with an `items`/`queue` array; unknown fields are ignored.
fn parse_queue(v: &serde_json::Value) -> Vec<QueueItem> {
    let arr = v
        .as_array()
        .or_else(|| v.get("items").and_then(|x| x.as_array()))
        .or_else(|| v.get("queue").and_then(|x| x.as_array()));
    let Some(arr) = arr else {
        return Vec::new();
    };
    let str_at = |it: &serde_json::Value, keys: &[&str]| -> String {
        for k in keys {
            if let Some(s) = it.get(*k).and_then(|x| x.as_str()) {
                if !s.is_empty() {
                    return s.to_string();
                }
            }
        }
        String::new()
    };
    arr.iter()
        .map(|it| {
            let priority = it
                .get("priority")
                .map(|p| match p {
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::String(s) => s.clone(),
                    _ => String::new(),
                })
                .unwrap_or_default();
            QueueItem {
                isrc: str_at(it, &["isrc"]),
                title: str_at(it, &["title", "name"]),
                artist: str_at(it, &["artist", "artists"]),
                priority,
                state: str_at(it, &["state", "status"]),
            }
        })
        .collect()
}

#[derive(Deserialize)]
struct TagForm {
    tag_id: i64,
}

/// Add this playlist as an extra source of one of the current user's tags.
async fn playlist_tag_add(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<TagForm>,
) -> Response {
    let Some((user_id, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tags::add_playlist_to_tag(&st.pool, user_id, f.tag_id, id).await;
    Redirect::to(&format!("/playlist/{id}")).into_response()
}

/// Remove this playlist as a source of a tag the current user owns.
async fn playlist_tag_remove(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<TagForm>,
) -> Response {
    let Some((user_id, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tags::remove_playlist_from_tag(&st.pool, user_id, f.tag_id, id).await;
    Redirect::to(&format!("/playlist/{id}")).into_response()
}

/// Promote a playlist to a tag owned by the current user (1:1, same name).
async fn playlist_tag(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some((user_id, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    match crate::tags::create_from_playlist(&st.pool, user_id, id).await {
        // Jump straight to the freshly created (or existing) tag.
        Ok(tag_id) => Redirect::to(&format!("/tag/{tag_id}")).into_response(),
        Err(e) => Redirect::to(&format!(
            "/playlist/{id}?msg={}",
            urlencoding::encode(&format!("Tag anlegen fehlgeschlagen: {e}"))
        ))
        .into_response(),
    }
}

// ── overlap / discover ──────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct CompareQuery {
    /// Comma-separated user ids; absent = all users.
    users: Option<String>,
    /// `all` | `owned` | `contributed` | `followed`.
    scope: Option<String>,
    /// BPM range (inclusive); only tracks with BPM data survive when set.
    bpm_min: Option<String>,
    bpm_max: Option<String>,
    /// Camelot key filter, e.g. `8A`.
    key: Option<String>,
    /// Phase/energy scale filter (1..5), derived from PHASE_n tags.
    phase_min: Option<String>,
    phase_max: Option<String>,
    /// `1`/`on` = include harmonically compatible keys, not just the exact key.
    key_harmonic: Option<String>,
    /// Sort field: `present` (default) | `bpm` | `key` | `energy`.
    sort: Option<String>,
    /// `asc` | `desc` (default `desc` for `present`, `asc` otherwise).
    dir: Option<String>,
    /// Full-text over track title + artists.
    q: Option<String>,
    /// Substring match on a resolved tag name (any user).
    tag: Option<String>,
    /// Flash message (success/error) shown after a redirect.
    msg: Option<String>,
}

/// The overlap filters that must be carried across the picker/scope links,
/// so toggling a user or scope keeps the BPM/key/sort/playlist selection.
#[derive(Clone, Default)]
struct OverlapFilters {
    scope: String,
    bpm_min: String,
    bpm_max: String,
    key: String,
    key_harmonic: bool,
    phase_min: String,
    phase_max: String,
    sort: String,
    dir: String,
    pl: Vec<i64>,
    q: String,
    tag: String,
}

impl OverlapFilters {
    fn href(&self, ids: &[i64]) -> String {
        let csv = ids
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(",");
        let mut url = format!("/overlap?users={csv}");
        if self.scope != "all" {
            url.push_str(&format!("&scope={}", self.scope));
        }
        if !self.bpm_min.is_empty() {
            url.push_str(&format!("&bpm_min={}", urlencoding::encode(&self.bpm_min)));
        }
        if !self.bpm_max.is_empty() {
            url.push_str(&format!("&bpm_max={}", urlencoding::encode(&self.bpm_max)));
        }
        if !self.key.is_empty() {
            url.push_str(&format!("&key={}", urlencoding::encode(&self.key)));
        }
        if self.key_harmonic {
            url.push_str("&key_harmonic=1");
        }
        if !self.phase_min.is_empty() {
            url.push_str(&format!(
                "&phase_min={}",
                urlencoding::encode(&self.phase_min)
            ));
        }
        if !self.phase_max.is_empty() {
            url.push_str(&format!(
                "&phase_max={}",
                urlencoding::encode(&self.phase_max)
            ));
        }
        if self.sort != "present" {
            url.push_str(&format!("&sort={}", self.sort));
        }
        if self.dir != "desc" {
            url.push_str(&format!("&dir={}", self.dir));
        }
        for id in &self.pl {
            url.push_str(&format!("&pl={id}"));
        }
        if !self.q.is_empty() {
            url.push_str(&format!("&q={}", urlencoding::encode(&self.q)));
        }
        if !self.tag.is_empty() {
            url.push_str(&format!("&tag={}", urlencoding::encode(&self.tag)));
        }
        url
    }
}

/// A clickable column-header sort link.
struct SortLink {
    href: String,
    active: bool,
    arrow: String,
}

/// All 24 Camelot keys for the key picker.
fn camelot_values() -> Vec<String> {
    let mut v = Vec::with_capacity(24);
    for n in 1..=12 {
        v.push(format!("{n}A"));
        v.push(format!("{n}B"));
    }
    v
}

/// Compare two optional numeric values, always sorting `None` last.
fn opt_cmp(a: Option<f64>, b: Option<f64>, desc: bool) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    match (a, b) {
        (Some(x), Some(y)) => {
            let o = x.partial_cmp(&y).unwrap_or(Ordering::Equal);
            if desc { o.reverse() } else { o }
        }
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Sort rank for a Camelot key (`"8A"` -> `(8, 'A')`); `None` for "—"/invalid.
fn camelot_rank(k: &str) -> Option<(i32, char)> {
    if k.len() < 2 {
        return None;
    }
    let (num, letter) = k.split_at(k.len() - 1);
    Some((num.parse().ok()?, letter.chars().next()?))
}

#[derive(Template)]
#[template(path = "overlap.html")]
struct OverlapPage {
    nav: crate::ui::Nav,
    flash: String,
    picker: Vec<PickerUser>,
    scopes: Vec<ScopeLink>,
    columns: Vec<ColumnUser>,
    rows: Vec<CompareRow>,
    track_total: i64,
    pairs: Vec<OverlapPair>,
    /// Filter form state.
    users_csv: String,
    scope: String,
    bpm_min: String,
    bpm_max: String,
    key_harmonic: bool,
    key_options: Vec<KeyOption>,
    phase_min: String,
    phase_max: String,
    has_audio: bool,
    /// Sort headers for the table columns.
    sort_present: SortLink,
    sort_bpm: SortLink,
    sort_key: SortLink,
    sort_energy: SortLink,
    sort_ripeness: SortLink,
    /// Playlist include picker (`?pl=`).
    playlists: Vec<PlaylistOpt>,
    pl_active: bool,
    sort_value: String,
    dir_value: String,
    /// Full-text + tag filter state.
    q: String,
    tag: String,
    tag_options: Vec<String>,
}

struct PlaylistOpt {
    id: i64,
    name: String,
    user: String,
    owned: bool,
    checked: bool,
}

struct KeyOption {
    value: String,
    selected: bool,
}

struct PickerUser {
    slug: String,
    selected: bool,
    href: String,
}

struct ScopeLink {
    label: String,
    href: String,
    active: bool,
}

struct ColumnUser {
    slug: String,
}

struct OverlapPair {
    a: String,
    b: String,
    shared_tracks: i64,
}

struct CompareRow {
    id: i64,
    title: String,
    artists: String,
    artists_html: String,
    isrc: String,
    bpm: String,
    key: String,
    energy: String,
    /// Phase/energy level 1..5 (from PHASE_n tags), display form.
    phase: String,
    bpm_num: Option<f64>,
    energy_num: Option<f64>,
    present_count: i64,
    ripeness_num: f64,
    ripeness_disp: String,
    cells: Vec<CompareCell>,
}

struct CompareCell {
    present: bool,
    liked: bool,
    playlists: Vec<CellPlaylist>,
}

#[derive(Clone)]
struct CellPlaylist {
    name: String,
    owned: bool,
}

struct CellAcc {
    liked: bool,
    playlists: Vec<CellPlaylist>,
}

struct TrackAcc {
    title: String,
    artists: String,
    isrc: String,
    per_user: HashMap<i64, CellAcc>,
}

/// Build the per-track accumulation for the selected users, applying the scope
/// and optional per-playlist include filter. Shared by the page and the
/// on-demand feature request.
async fn overlap_acc(
    pool: &sqlx::SqlitePool,
    selected: &[(i64, String)],
    scope: &str,
    pl_filter: Option<&HashSet<i64>>,
) -> HashMap<i64, TrackAcc> {
    let mut acc: HashMap<i64, TrackAcc> = HashMap::new();
    if selected.is_empty() {
        return acc;
    }

    let mut qb = QueryBuilder::new(
        "SELECT hp.user_id AS uid, hp.id AS pl_id, hp.name AS pl_name, hp.is_owned AS owned,\n                hp.collaborative AS collab,\n                t.id AS tid, t.title AS title, t.artists AS artists, t.isrc AS isrc\n           FROM hub_playlist_tracks hpt\n           JOIN hub_playlists hp ON hp.id = hpt.playlist_id\n           JOIN hub_tracks t ON t.id = hpt.track_id\n          WHERE hp.user_id IN (",
    );
    let mut sep = qb.separated(", ");
    for (id, _) in selected {
        sep.push_bind(*id);
    }
    qb.push(")");
    for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
        let uid: i64 = r.get("uid");
        let pl_id: i64 = r.get("pl_id");
        let owned: i64 = r.get("owned");
        let collab: i64 = r.get("collab");
        if !scope_match(scope, owned == 1, collab == 1) {
            continue;
        }
        if let Some(f) = pl_filter {
            if !f.contains(&pl_id) {
                continue;
            }
        }
        let tid: i64 = r.get("tid");
        let e = acc.entry(tid).or_insert_with(|| TrackAcc {
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            artists: r.get::<Option<String>, _>("artists").unwrap_or_default(),
            isrc: r.get::<Option<String>, _>("isrc").unwrap_or_default(),
            per_user: HashMap::new(),
        });
        let cell = e.per_user.entry(uid).or_insert_with(|| CellAcc {
            liked: false,
            playlists: Vec::new(),
        });
        cell.playlists.push(CellPlaylist {
            name: r.get::<Option<String>, _>("pl_name").unwrap_or_default(),
            owned: owned == 1,
        });
    }

    let mut qb = QueryBuilder::new(
        "SELECT l.user_id AS uid, t.id AS tid, t.title AS title, t.artists AS artists, t.isrc AS isrc\n           FROM hub_liked_tracks l JOIN hub_tracks t ON t.id = l.track_id\n          WHERE l.user_id IN (",
    );
    let mut sep = qb.separated(", ");
    for (id, _) in selected {
        sep.push_bind(*id);
    }
    qb.push(")");
    for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
        let uid: i64 = r.get("uid");
        let tid: i64 = r.get("tid");
        let e = acc.entry(tid).or_insert_with(|| TrackAcc {
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            artists: r.get::<Option<String>, _>("artists").unwrap_or_default(),
            isrc: r.get::<Option<String>, _>("isrc").unwrap_or_default(),
            per_user: HashMap::new(),
        });
        e.per_user
            .entry(uid)
            .or_insert_with(|| CellAcc {
                liked: false,
                playlists: Vec::new(),
            })
            .liked = true;
    }

    acc
}

/// Track ids present for >= 2 of the selected users (the "shared" rows).
fn shared_ids(acc: &HashMap<i64, TrackAcc>) -> Vec<i64> {
    acc.iter()
        .filter(|(_, t)| {
            t.per_user
                .values()
                .filter(|c| c.liked || !c.playlists.is_empty())
                .count()
                >= 2
        })
        .map(|(id, _)| *id)
        .collect()
}

/// Case-insensitive full-text match over title + artists.
fn text_matches(title: &str, artists: &str, q: &str) -> bool {
    let hay = format!("{title} {artists}").to_lowercase();
    crate::table::matches_typo(&hay, &crate::table::tokens(q))
}

/// Ids from `candidate_ids` that resolve to a tag matching `needle` — either a
/// tag whose name contains it or one of its descendants (hierarchy-aware).
async fn tagged_track_ids(
    pool: &sqlx::SqlitePool,
    candidate_ids: &[i64],
    needle: &str,
) -> HashSet<i64> {
    let mut set = HashSet::new();
    let tag_ids: Vec<i64> = crate::tags::matching_tag_ids(pool, needle)
        .await
        .into_iter()
        .collect();
    if tag_ids.is_empty() {
        return set;
    }
    for chunk in candidate_ids.chunks(300) {
        let mut qb = QueryBuilder::new(
            "SELECT DISTINCT track_id FROM hub_track_resolved_tags WHERE track_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        qb.push(") AND tag_id IN (");
        let mut sep = qb.separated(", ");
        for id in &tag_ids {
            sep.push_bind(*id);
        }
        qb.push(")");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            set.insert(r.get::<i64, _>("track_id"));
        }
    }
    set
}

async fn overlap_page(
    State(st): State<AppState>,
    Query(q): Query<CompareQuery>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "overlap").await else {
        return Redirect::to("/login").into_response();
    };

    let all_users =
        sqlx::query_as::<_, (i64, String)>("SELECT id, slug FROM hub_users ORDER BY slug")
            .fetch_all(&st.pool)
            .await
            .unwrap_or_default();
    let slug_of: HashMap<i64, String> = all_users.iter().cloned().collect();

    let scope = match q.scope.as_deref() {
        Some("owned") => "owned",
        Some("followed") => "followed",
        Some("contributed") => "contributed",
        _ => "all",
    };

    // BPM / key filters.
    let bpm_min_raw = q.bpm_min.clone().unwrap_or_default().trim().to_string();
    let bpm_max_raw = q.bpm_max.clone().unwrap_or_default().trim().to_string();
    let key = q.key.clone().unwrap_or_default().trim().to_string();
    let key_harmonic = matches!(
        q.key_harmonic.as_deref(),
        Some("1") | Some("on") | Some("true")
    );
    let bpm_min = bpm_min_raw.parse::<f64>().ok();
    let bpm_max = bpm_max_raw.parse::<f64>().ok();
    // Phase/energy scale filter (1..5).
    let phase_min_raw = q.phase_min.clone().unwrap_or_default().trim().to_string();
    let phase_max_raw = q.phase_max.clone().unwrap_or_default().trim().to_string();
    let phase_min = phase_min_raw.parse::<i64>().ok();
    let phase_max = phase_max_raw.parse::<i64>().ok();
    let q_text = q.q.clone().unwrap_or_default().trim().to_string();
    let tag = q.tag.clone().unwrap_or_default().trim().to_string();

    // Sort field + direction.
    let sort = match q.sort.as_deref() {
        Some("bpm") => "bpm",
        Some("key") => "key",
        Some("energy") => "energy",
        Some("ripeness") => "ripeness",
        _ => "present",
    };
    let dir = match q.dir.as_deref() {
        Some("asc") => "asc",
        Some("desc") => "desc",
        _ if sort == "present" || sort == "ripeness" => "desc",
        _ => "asc",
    };

    // Optional per-playlist include list (?pl=1&pl=2). Absent = include all.
    let mut pl_ids: Vec<i64> = raw
        .as_deref()
        .map(|raw| {
            raw.split('&')
                .filter_map(|kv| kv.split_once('='))
                .filter(|(k, _)| *k == "pl")
                .filter_map(|(_, v)| v.parse::<i64>().ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    pl_ids.sort_unstable();
    pl_ids.dedup();
    let pl_filter: Option<HashSet<i64>> = if pl_ids.is_empty() {
        None
    } else {
        Some(pl_ids.iter().copied().collect())
    };

    let filters = OverlapFilters {
        scope: scope.to_string(),
        bpm_min: bpm_min_raw.clone(),
        bpm_max: bpm_max_raw.clone(),
        key: key.clone(),
        key_harmonic,
        phase_min: phase_min_raw.clone(),
        phase_max: phase_max_raw.clone(),
        sort: sort.to_string(),
        dir: dir.to_string(),
        pl: pl_ids.clone(),
        q: q_text.clone(),
        tag: tag.clone(),
    };

    // Selected users: default all; otherwise the csv (existing ids only).
    let selected_set: HashSet<i64> = match q.users.as_deref() {
        Some(csv) if !csv.trim().is_empty() => csv
            .split(',')
            .filter_map(|s| s.trim().parse::<i64>().ok())
            .collect(),
        _ => all_users.iter().map(|(id, _)| *id).collect(),
    };
    let selected: Vec<(i64, String)> = all_users
        .iter()
        .filter(|(id, _)| selected_set.contains(id))
        .cloned()
        .collect();
    let current_ids: Vec<i64> = selected.iter().map(|(id, _)| *id).collect();

    let acc = overlap_acc(&st.pool, &selected, scope, pl_filter.as_ref()).await;

    // Tag filter: track ids of the candidates that resolve to a matching tag.
    let tagged: HashSet<i64> = if tag.is_empty() {
        HashSet::new()
    } else {
        let ids: Vec<i64> = acc.keys().copied().collect();
        tagged_track_ids(&st.pool, &ids, &tag).await
    };

    // Per-track audio (BPM / key / energy) for filtering + display. Chunked because
    // the id list can hold tens of thousands of tracks (SQLite bind-variable limit).
    let mut audio: HashMap<i64, (Option<f64>, String, Option<f64>)> = HashMap::new();
    if !acc.is_empty() {
        let ids: Vec<i64> = acc.keys().copied().collect();
        for chunk in ids.chunks(900) {
            let mut qb = QueryBuilder::new(
                "SELECT v.track_id AS track_id, v.bpm AS bpm, v.camelot AS camelot, f.energy AS energy\n                   FROM v_track_audio v\n                   LEFT JOIN hub_track_features f ON f.track_id = v.track_id\n                  WHERE v.track_id IN (",
            );
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(*id);
            }
            qb.push(")");
            for r in qb.build().fetch_all(&st.pool).await.unwrap_or_default() {
                audio.insert(
                    r.get("track_id"),
                    (
                        r.get::<Option<f64>, _>("bpm"),
                        r.get::<Option<String>, _>("camelot").unwrap_or_default(),
                        r.get::<Option<f64>, _>("energy"),
                    ),
                );
            }
        }
    }

    // Phase/energy level 1..5 per track, from its PHASE_n tags (empty = none).
    let mut phases: HashMap<i64, i64> = HashMap::new();
    if !acc.is_empty() {
        let ids: Vec<i64> = acc.keys().copied().collect();
        for chunk in ids.chunks(900) {
            let mut qb = QueryBuilder::new(
                "SELECT rt.track_id AS track_id,
                        MAX(CASE t.type
                              WHEN 'PHASE_1' THEN 1 WHEN 'PHASE_2' THEN 2 WHEN 'PHASE_3' THEN 3
                              WHEN 'PHASE_4' THEN 4 WHEN 'PHASE_5' THEN 5 END) AS phase
                   FROM hub_track_resolved_tags rt
                   JOIN hub_tags t ON t.id = rt.tag_id
                  WHERE rt.track_id IN (",
            );
            let mut sep = qb.separated(", ");
            for id in chunk {
                sep.push_bind(*id);
            }
            qb.push(") GROUP BY rt.track_id");
            for r in qb.build().fetch_all(&st.pool).await.unwrap_or_default() {
                if let Some(p) = r.get::<Option<i64>, _>("phase") {
                    phases.insert(r.get("track_id"), p);
                }
            }
        }
    }

    // Assemble rows: keep tracks present for >= 2 of the selected users.
    let mut rows: Vec<CompareRow> = Vec::new();
    let mut pair_counts: HashMap<(i64, i64), i64> = HashMap::new();
    for (tid, t) in acc {
        let (bpm_opt, camelot, energy_opt) =
            audio
                .get(&tid)
                .cloned()
                .unwrap_or((None, String::new(), None));
        // BPM range filter: a set range excludes tracks without BPM data.
        if bpm_min.is_some() || bpm_max.is_some() {
            match bpm_opt {
                Some(b) => {
                    if bpm_min.is_some_and(|min| b < min) {
                        continue;
                    }
                    if bpm_max.is_some_and(|max| b > max) {
                        continue;
                    }
                }
                None => continue,
            }
        }
        // Key filter: exact Camelot, or harmonically compatible when asked.
        if !key.is_empty() {
            let ok = if key_harmonic {
                crate::features::camelot_compatible(&key, &camelot)
            } else {
                camelot.eq_ignore_ascii_case(&key)
            };
            if !ok {
                continue;
            }
        }
        // Phase/energy scale filter: a set range excludes tracks without a phase.
        let phase_opt = phases.get(&tid).copied();
        if phase_min.is_some() || phase_max.is_some() {
            match phase_opt {
                Some(p) => {
                    if phase_min.is_some_and(|m| p < m) {
                        continue;
                    }
                    if phase_max.is_some_and(|m| p > m) {
                        continue;
                    }
                }
                None => continue,
            }
        }
        // Full-text (title + artists) and tag filters.
        if !q_text.is_empty() && !text_matches(&t.title, &t.artists, &q_text) {
            continue;
        }
        if !tag.is_empty() && !tagged.contains(&tid) {
            continue;
        }
        let present: Vec<i64> = t
            .per_user
            .iter()
            .filter(|(_, c)| c.liked || !c.playlists.is_empty())
            .map(|(u, _)| *u)
            .collect();
        if present.len() < 2 {
            continue;
        }
        for i in 0..present.len() {
            for j in (i + 1)..present.len() {
                let a = present[i].min(present[j]);
                let b = present[i].max(present[j]);
                *pair_counts.entry((a, b)).or_insert(0) += 1;
            }
        }
        let mut cells = Vec::with_capacity(selected.len());
        for (uid, _) in &selected {
            match t.per_user.get(uid) {
                Some(c) => {
                    let mut pls = c.playlists.clone();
                    pls.sort_by(|x, y| (!x.owned, &x.name).cmp(&(!y.owned, &y.name)));
                    cells.push(CompareCell {
                        present: c.liked || !pls.is_empty(),
                        liked: c.liked,
                        playlists: pls,
                    });
                }
                None => cells.push(CompareCell {
                    present: false,
                    liked: false,
                    playlists: Vec::new(),
                }),
            }
        }
        rows.push(CompareRow {
            id: tid,
            title: t.title,
            artists: t.artists,
            artists_html: String::new(),
            isrc: t.isrc,
            bpm: bpm_opt
                .map(|b| format!("{b:.0}"))
                .unwrap_or_else(|| "—".into()),
            key: if camelot.is_empty() {
                "—".into()
            } else {
                camelot
            },
            energy: energy_opt
                .map(|e| format!("{e:.2}"))
                .unwrap_or_else(|| "—".into()),
            phase: phase_opt
                .map(|p| p.to_string())
                .unwrap_or_else(|| "—".into()),
            bpm_num: bpm_opt,
            energy_num: energy_opt,
            present_count: present.len() as i64,
            ripeness_num: 0.0,
            ripeness_disp: "—".to_string(),
            cells,
        });
    }

    // Ripeness signal (#223): data completeness per shared track, only when the
    // caller actually sorts by it (keeps the default path unchanged).
    if sort == "ripeness" && !rows.is_empty() {
        let ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
        let e = crate::settings::engine(&st.pool).await;
        let ripe = crate::scoring::ripeness_many(&st.pool, &ids, &e).await;
        for r in rows.iter_mut() {
            r.ripeness_num = ripe.get(&r.id).copied().unwrap_or(0.0);
            r.ripeness_disp = format!("{:.0}", r.ripeness_num);
        }
    }

    // Sort by the chosen field; tracks without the value always go last.
    let desc = dir == "desc";
    rows.sort_by(|a, b| {
        let ord = match sort {
            "bpm" => opt_cmp(a.bpm_num, b.bpm_num, desc),
            "energy" => opt_cmp(a.energy_num, b.energy_num, desc),
            "ripeness" => {
                let o = a
                    .ripeness_num
                    .partial_cmp(&b.ripeness_num)
                    .unwrap_or(std::cmp::Ordering::Equal);
                if desc { o.reverse() } else { o }
            }
            "key" => match (camelot_rank(&a.key), camelot_rank(&b.key)) {
                (Some(x), Some(y)) => {
                    let o = x.cmp(&y);
                    if desc { o.reverse() } else { o }
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            },
            _ => {
                let o = a.present_count.cmp(&b.present_count);
                if desc { o.reverse() } else { o }
            }
        };
        ord.then_with(|| a.artists.cmp(&b.artists))
            .then_with(|| a.title.cmp(&b.title))
    });
    rows.truncate(1000);
    let overlap_ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let mut overlap_html = crate::artists::artists_html_for_tracks(&st.pool, &overlap_ids).await;
    for r in rows.iter_mut() {
        r.artists_html = overlap_html.remove(&r.id).unwrap_or_default();
    }

    let mut pairs: Vec<OverlapPair> = pair_counts
        .into_iter()
        .map(|((a, b), n)| OverlapPair {
            a: slug_of.get(&a).cloned().unwrap_or_default(),
            b: slug_of.get(&b).cloned().unwrap_or_default(),
            shared_tracks: n,
        })
        .collect();
    pairs.sort_by(|x, y| y.shared_tracks.cmp(&x.shared_tracks));

    // Picker + scope links.
    let picker: Vec<PickerUser> = all_users
        .iter()
        .map(|(id, slug)| {
            let is_sel = current_ids.contains(id);
            let mut ids = current_ids.clone();
            if is_sel {
                ids.retain(|x| x != id);
            } else {
                ids.push(*id);
            }
            ids.sort_unstable();
            PickerUser {
                slug: slug.clone(),
                selected: is_sel,
                href: filters.href(&ids),
            }
        })
        .collect();

    let scope_link = |label: &str, sc: &str, active: bool| ScopeLink {
        label: label.to_string(),
        href: OverlapFilters {
            scope: sc.to_string(),
            ..filters.clone()
        }
        .href(&current_ids),
        active,
    };
    let scopes = vec![
        scope_link("Alle", "all", scope == "all"),
        scope_link("Eigene", "owned", scope == "owned"),
        scope_link("Collaborativ", "contributed", scope == "contributed"),
        scope_link("Gefolgt", "followed", scope == "followed"),
    ];

    let key_options: Vec<KeyOption> = camelot_values()
        .into_iter()
        .map(|v| KeyOption {
            selected: v.eq_ignore_ascii_case(&key),
            value: v,
        })
        .collect();
    let has_audio = audio
        .values()
        .any(|(b, c, e)| b.is_some() || !c.is_empty() || e.is_some());

    let tag_options: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT name FROM hub_tags ORDER BY name COLLATE NOCASE",
    )
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    // Playlist include picker across the selected users (respecting scope).
    let mut playlists: Vec<PlaylistOpt> = Vec::new();
    if !current_ids.is_empty() {
        let mut qb = QueryBuilder::new(
            "SELECT hp.id AS id, hp.name AS name, hp.is_owned AS owned,
                    hp.collaborative AS collab, u.slug AS slug
               FROM hub_playlists hp JOIN hub_users u ON u.id = hp.user_id
              WHERE hp.user_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in &current_ids {
            sep.push_bind(*id);
        }
        qb.push(") ORDER BY hp.is_owned DESC, u.slug, hp.name COLLATE NOCASE");
        for r in qb.build().fetch_all(&st.pool).await.unwrap_or_default() {
            let owned = r.get::<i64, _>("owned") == 1;
            let collab = r.get::<i64, _>("collab") == 1;
            if !scope_match(scope, owned, collab) {
                continue;
            }
            let id: i64 = r.get("id");
            playlists.push(PlaylistOpt {
                checked: pl_filter.as_ref().map_or(true, |f| f.contains(&id)),
                id,
                name: r.get::<Option<String>, _>("name").unwrap_or_default(),
                user: r.get::<Option<String>, _>("slug").unwrap_or_default(),
                owned,
            });
        }
    }

    let sort_link = |field: &str| -> SortLink {
        let active = sort == field;
        let next_dir = if active && dir == "asc" {
            "desc"
        } else {
            "asc"
        };
        let mut f = filters.clone();
        f.sort = field.to_string();
        f.dir = next_dir.to_string();
        SortLink {
            href: f.href(&current_ids),
            active,
            arrow: if active {
                if dir == "asc" {
                    "↑".into()
                } else {
                    "↓".into()
                }
            } else {
                String::new()
            },
        }
    };

    let columns: Vec<ColumnUser> = selected
        .iter()
        .map(|(_, slug)| ColumnUser { slug: slug.clone() })
        .collect();

    render(&OverlapPage {
        nav,
        flash: q.msg.clone().unwrap_or_default(),
        picker,
        scopes,
        columns,
        track_total: rows.len() as i64,
        rows,
        pairs,
        users_csv: current_ids
            .iter()
            .map(|i| i.to_string())
            .collect::<Vec<_>>()
            .join(","),
        scope: scope.to_string(),
        bpm_min: bpm_min_raw,
        bpm_max: bpm_max_raw,
        key_harmonic,
        key_options,
        phase_min: phase_min_raw,
        phase_max: phase_max_raw,
        has_audio,
        sort_present: sort_link("present"),
        sort_bpm: sort_link("bpm"),
        sort_key: sort_link("key"),
        sort_energy: sort_link("energy"),
        sort_ripeness: sort_link("ripeness"),
        playlists,
        pl_active: !pl_ids.is_empty(),
        sort_value: sort.to_string(),
        dir_value: dir.to_string(),
        q: q_text,
        tag,
        tag_options,
    })
}

/// Parse `application/x-www-form-urlencoded` into a multimap (handles repeated
/// keys, e.g. the `pl` playlist checkboxes).
fn form_params(body: &str) -> HashMap<String, Vec<String>> {
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for pair in body.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let dec = |s: &str| {
            urlencoding::decode(s)
                .map(|c| c.into_owned())
                .unwrap_or_else(|_| s.to_string())
        };
        out.entry(dec(k)).or_default().push(dec(v));
    }
    out
}

/// On-demand: enqueue the shared tracks of the current selection for BPM/key
/// lookup (the enrichment worker processes these before its backlog).
async fn overlap_enrich(State(st): State<AppState>, headers: HeaderMap, body: String) -> Response {
    if crate::ui::nav(&st, &headers, "overlap").await.is_none() {
        return Redirect::to("/login").into_response();
    }
    let p = form_params(&body);
    let get = |k: &str| {
        p.get(k)
            .and_then(|v| v.first())
            .cloned()
            .unwrap_or_default()
    };

    let all_users =
        sqlx::query_as::<_, (i64, String)>("SELECT id, slug FROM hub_users ORDER BY slug")
            .fetch_all(&st.pool)
            .await
            .unwrap_or_default();
    let scope = match get("scope").as_str() {
        "owned" => "owned",
        "followed" => "followed",
        "contributed" => "contributed",
        _ => "all",
    };
    let users_csv = get("users");
    let selected_set: HashSet<i64> = if users_csv.trim().is_empty() {
        all_users.iter().map(|(id, _)| *id).collect()
    } else {
        users_csv
            .split(',')
            .filter_map(|s| s.trim().parse::<i64>().ok())
            .collect()
    };
    let selected: Vec<(i64, String)> = all_users
        .iter()
        .filter(|(id, _)| selected_set.contains(id))
        .cloned()
        .collect();
    let selected_ids: Vec<i64> = selected.iter().map(|(id, _)| *id).collect();

    let mut pl_ids: Vec<i64> = p
        .get("pl")
        .map(|v| v.iter().filter_map(|s| s.parse::<i64>().ok()).collect())
        .unwrap_or_default();
    pl_ids.sort_unstable();
    pl_ids.dedup();
    let pl_filter: Option<HashSet<i64>> = if pl_ids.is_empty() {
        None
    } else {
        Some(pl_ids.iter().copied().collect())
    };

    let acc = overlap_acc(&st.pool, &selected, scope, pl_filter.as_ref()).await;
    let ids = shared_ids(&acc);
    let queued = crate::features::enqueue_missing(&st.pool, &ids)
        .await
        .unwrap_or(0);

    let sort = match get("sort").as_str() {
        "bpm" => "bpm",
        "key" => "key",
        "energy" => "energy",
        _ => "present",
    };
    let dir = match get("dir").as_str() {
        "asc" => "asc",
        "desc" => "desc",
        _ if sort == "present" => "desc",
        _ => "asc",
    };
    let filters = OverlapFilters {
        scope: scope.to_string(),
        bpm_min: get("bpm_min"),
        bpm_max: get("bpm_max"),
        key: get("key"),
        key_harmonic: matches!(get("key_harmonic").as_str(), "1" | "on" | "true"),
        phase_min: get("phase_min"),
        phase_max: get("phase_max"),
        sort: sort.to_string(),
        dir: dir.to_string(),
        pl: pl_ids,
        q: get("q"),
        tag: get("tag"),
    };
    let back = filters.href(&selected_ids);
    flash_redirect(
        &back,
        format!("{queued} Tracks zur BPM/Key-Abfrage eingereiht — der Worker holt sie jetzt"),
    )
}

// ── similar playlists (playlists as tags) ────────────────────────────────────

#[derive(Deserialize, Default)]
struct SimilarQuery {
    /// `all` | `owned` | `contributed` | `followed`.
    scope: Option<String>,
    q: Option<String>,
    sort: Option<String>,
    dir: Option<String>,
}

#[derive(Template)]
#[template(path = "similar.html")]
struct SimilarPage {
    nav: crate::ui::Nav,
    flash: String,
    q: String,
    scope: String,
    users: Vec<ColumnUser>,
    rows: Vec<SimilarRow>,
    scopes: Vec<ScopeLink>,
    s_label: crate::table::SortHead,
    s_users: crate::table::SortHead,
}

struct SimilarRow {
    label: String,
    user_count: i64,
    shared: bool,
    cells: Vec<SimilarCell>,
}

struct SimilarCell {
    present: bool,
    id: i64,
    name: String,
    owned: bool,
    collaborative: bool,
    /// This exact Spotify playlist (same playlist id) is present for >= 2 users.
    shared: bool,
}

/// Classify a playlist row for the scope filter.
/// `owned` = your own; `contributed` = collaborative; `followed` = subscribed.
fn scope_match(scope: &str, owned: bool, collaborative: bool) -> bool {
    match scope {
        "owned" => owned && !collaborative,
        "contributed" => collaborative,
        "followed" => !owned && !collaborative,
        _ => true,
    }
}

/// Lowercase, keep alphanumerics, collapse everything else to single spaces.
pub(crate) use crate::tags::normalize_name;

struct SimilarEntry {
    uid: i64,
    id: i64,
    name: String,
    owned: bool,
    collaborative: bool,
    spotify_id: String,
}

async fn similar_page(
    State(st): State<AppState>,
    Query(q): Query<SimilarQuery>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "similar").await else {
        return Redirect::to("/login").into_response();
    };
    let scope = match q.scope.as_deref() {
        Some("owned") => "owned",
        Some("contributed") => "contributed",
        Some("followed") => "followed",
        _ => "all",
    };

    let users = sqlx::query_as::<_, (i64, String)>("SELECT id, slug FROM hub_users ORDER BY slug")
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();

    let rows = sqlx::query(
        "SELECT p.user_id AS uid, p.id AS id, p.name AS name, p.is_owned AS owned,
                p.collaborative AS collab, p.playlist_id AS spotify
           FROM hub_playlists p",
    )
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    let mut groups: HashMap<String, Vec<SimilarEntry>> = HashMap::new();
    for r in &rows {
        let owned = r.get::<i64, _>("owned") == 1;
        let collab = r.get::<i64, _>("collab") == 1;
        if !scope_match(scope, owned, collab) {
            continue;
        }
        let name: String = r.get::<Option<String>, _>("name").unwrap_or_default();
        let key = normalize_name(&name);
        if key.is_empty() {
            continue;
        }
        groups.entry(key).or_default().push(SimilarEntry {
            uid: r.get("uid"),
            id: r.get("id"),
            name,
            owned,
            collaborative: collab,
            spotify_id: r.get::<Option<String>, _>("spotify").unwrap_or_default(),
        });
    }

    let mut out: Vec<SimilarRow> = Vec::new();
    for (key, entries) in groups {
        let distinct: HashSet<i64> = entries.iter().map(|e| e.uid).collect();
        if distinct.len() < 2 {
            continue;
        }

        // Same Spotify playlist across users? → "contributed"/shared.
        let mut by_spotify: HashMap<&str, HashSet<i64>> = HashMap::new();
        for e in &entries {
            if !e.spotify_id.is_empty() {
                by_spotify
                    .entry(e.spotify_id.as_str())
                    .or_default()
                    .insert(e.uid);
            }
        }
        let shared_ids: HashSet<&str> = by_spotify
            .iter()
            .filter(|(_, us)| us.len() >= 2)
            .map(|(k, _)| *k)
            .collect();
        let row_shared = !shared_ids.is_empty();

        let cells = users
            .iter()
            .map(|(uid, _)| match entries.iter().find(|e| e.uid == *uid) {
                Some(e) => SimilarCell {
                    present: true,
                    id: e.id,
                    name: e.name.clone(),
                    owned: e.owned,
                    collaborative: e.collaborative,
                    shared: shared_ids.contains(e.spotify_id.as_str()),
                },
                None => SimilarCell {
                    present: false,
                    id: 0,
                    name: String::new(),
                    owned: false,
                    collaborative: false,
                    shared: false,
                },
            })
            .collect();
        out.push(SimilarRow {
            label: key,
            user_count: distinct.len() as i64,
            shared: row_shared,
            cells,
        });
    }
    out.sort_by(|a, b| {
        b.shared
            .cmp(&a.shared)
            .then_with(|| b.user_count.cmp(&a.user_count))
            .then_with(|| a.label.cmp(&b.label))
    });

    // Fuzzy filter + optional header sort.
    let fq = q.q.clone().unwrap_or_default().trim().to_string();
    let toks = crate::table::tokens(&fq);
    out.retain(|r| crate::table::matches_typo(&r.label.to_lowercase(), &toks));
    let allowed: &[(&str, &str)] = &[("label", "label"), ("users", "user_count")];
    let (sort, dir) =
        crate::table::resolve_sort(q.sort.as_deref(), q.dir.as_deref(), allowed, "", "asc");
    if !sort.is_empty() {
        let desc = dir == "desc";
        out.sort_by(|a, b| {
            let o = if sort == "users" {
                a.user_count.cmp(&b.user_count)
            } else {
                a.label.cmp(&b.label)
            };
            if desc { o.reverse() } else { o }
        });
    }
    out.truncate(500);

    let scopes = vec![
        ScopeLink {
            label: "Alle".to_string(),
            href: "/playlists/similar".to_string(),
            active: scope == "all",
        },
        ScopeLink {
            label: "Eigene".to_string(),
            href: "/playlists/similar?scope=owned".to_string(),
            active: scope == "owned",
        },
        ScopeLink {
            label: "Collaborativ".to_string(),
            href: "/playlists/similar?scope=contributed".to_string(),
            active: scope == "contributed",
        },
        ScopeLink {
            label: "Gefolgt".to_string(),
            href: "/playlists/similar?scope=followed".to_string(),
            active: scope == "followed",
        },
    ];

    let rawq = raw.as_deref().unwrap_or("");
    let s_label = crate::table::sort_head(rawq, "label", "Playlist-Name", &sort, &dir);
    let s_users = crate::table::sort_head(rawq, "users", "Nutzer", &sort, &dir);
    render(&SimilarPage {
        nav,
        flash: String::new(),
        q: fq,
        scope: scope.to_string(),
        users: users
            .iter()
            .map(|(_, slug)| ColumnUser { slug: slug.clone() })
            .collect(),
        rows: out,
        scopes,
        s_label,
        s_users,
    })
}

// ── digging ────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct DiggingQuery {
    #[serde(default, deserialize_with = "crate::table::qnum::opt_i64")]
    seed: Option<i64>,
    mine: Option<String>,
    bpm: Option<String>,
    harm: Option<String>,
    #[serde(default, deserialize_with = "crate::table::qnum::opt_f64")]
    tol: Option<f64>,
    /// Filter: tag owner slug.
    towner: Option<String>,
    /// Filter: tag type (e.g. `MOOD`).
    ttype: Option<String>,
    /// Filter: playlist owner (hub user) slug.
    powner: Option<String>,
    /// Filter: tag name (hierarchy-aware: also matches child tags).
    tag: Option<String>,
    /// Filter: minimum tag weight (group-weight signal).
    #[serde(default, deserialize_with = "crate::table::qnum::opt_f64")]
    wmin: Option<f64>,
}

/// A `<select>` option with a precomputed `selected` flag.
struct SelOpt {
    value: String,
    label: String,
    selected: bool,
}

#[derive(Template)]
#[template(path = "digging.html")]
struct DiggingPage {
    nav: crate::ui::Nav,
    flash: String,
    has_seed: bool,
    seed_id: i64,
    seed_title: String,
    seed_artists_html: String,
    rows: Vec<DigRow>,
    lastfm_enabled: bool,
    freqblog_enabled: bool,
    freqblog_remaining: i64,
    seed_bpm: String,
    seed_key: String,
    filters: Vec<ScopeLink>,
    bpm_opts: Vec<ScopeLink>,
    tag_owner_opts: Vec<SelOpt>,
    tag_type_opts: Vec<SelOpt>,
    pl_owner_opts: Vec<SelOpt>,
    tag: String,
    wmin: f64,
    mine: bool,
    bpm: bool,
    harm: bool,
    tol: i64,
    reset_href: String,
}

struct DigRow {
    track_id: i64,
    matched: bool,
    title: String,
    artists: String,
    artists_html: String,
    sources: Vec<String>,
    users: i64,
    playlists: i64,
    likes: i64,
    bpm: Option<f64>,
    bpm_disp: String,
    camelot: String,
    score: i64,
    /// Sum of the group weights of this track's tags (scoring-engine signal).
    tag_weight: f64,
    tag_weight_disp: String,
    /// Seed↔candidate: sum of group weights of tags shared with the seed.
    shared_weight: f64,
    shared_weight_disp: String,
    /// Seed↔candidate similarity v2 (cross-group bonus, Rumpelkiste-aware).
    similarity: f64,
    similarity_disp: String,
    /// Ripeness score of the candidate (data completeness).
    ripeness: f64,
    ripeness_disp: String,
    /// Per-user presence with playlist names (track-detail style).
    presence: Vec<DigUserRow>,
    /// Resolved tags with their groups.
    tags: Vec<DigTag>,
}

#[derive(Clone)]
struct DigUserRow {
    slug: String,
    liked: bool,
    owned: Vec<DigPlaylist>,
    followed: Vec<DigPlaylist>,
}

#[derive(Clone)]
struct DigPlaylist {
    id: i64,
    name: String,
}

#[derive(Clone)]
struct DigTag {
    id: i64,
    name: String,
    owner: String,
    /// Raw type string (for the `ttype` filter).
    type_value: String,
    type_label: String,
    type_icon: String,
}

/// Merge a (possibly external) candidate into the session map: dedupe by hub
/// track id when matched, else by normalised artist|title.
async fn merge_candidate(
    cand: &mut HashMap<String, DigRow>,
    tid: Option<i64>,
    title: &str,
    artists: &str,
    source: &str,
) {
    let key = match tid {
        Some(id) => format!("t{id}"),
        None => format!(
            "x:{}|{}",
            crate::tags::normalize_name(artists),
            crate::tags::normalize_name(title)
        ),
    };
    if let Some(existing) = cand.get_mut(&key) {
        if !existing.sources.iter().any(|s| s == source) {
            existing.sources.push(source.to_string());
            if !existing.matched {
                existing.title = title.to_string();
                existing.artists = artists.to_string();
            }
        }
        return;
    }
    cand.insert(
        key,
        DigRow {
            track_id: tid.unwrap_or(0),
            matched: tid.is_some(),
            title: title.to_string(),
            artists: artists.to_string(),
            artists_html: String::new(),
            sources: vec![source.to_string()],
            users: 0,
            playlists: 0,
            likes: 0,
            bpm: None,
            bpm_disp: String::new(),
            camelot: String::new(),
            score: 0,
            tag_weight: 0.0,
            tag_weight_disp: String::new(),
            shared_weight: 0.0,
            shared_weight_disp: String::new(),
            similarity: 0.0,
            similarity_disp: String::new(),
            ripeness: 0.0,
            ripeness_disp: String::new(),
            presence: Vec::new(),
            tags: Vec::new(),
        },
    );
}

/// Batch-load per-user playlists (owned/followed) and resolved tags (with
/// groups) for the given track ids — the track-detail view's data, in bulk.
async fn dig_details(
    pool: &sqlx::SqlitePool,
    ids: &[i64],
) -> (HashMap<i64, Vec<DigUserRow>>, HashMap<i64, Vec<DigTag>>) {
    let mut presence: HashMap<i64, Vec<DigUserRow>> = HashMap::new();
    let mut tags: HashMap<i64, Vec<DigTag>> = HashMap::new();
    if ids.is_empty() {
        return (presence, tags);
    }

    for chunk in ids.chunks(900) {
        // Playlist memberships (per user).
        let mut qb = QueryBuilder::new(
            "SELECT hpt.track_id AS tid, u.slug AS slug, hp.id AS pl_id, hp.name AS pl_name,
                    hp.is_owned AS owned
               FROM hub_playlist_tracks hpt
               JOIN hub_playlists hp ON hp.id = hpt.playlist_id
               JOIN hub_users u ON u.id = hp.user_id
              WHERE hpt.track_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        qb.push(") ORDER BY u.slug, hp.is_owned DESC, hp.name COLLATE NOCASE");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let tid: i64 = r.get("tid");
            let slug: String = r.get("slug");
            let pl = DigPlaylist {
                id: r.get("pl_id"),
                name: r.get::<Option<String>, _>("pl_name").unwrap_or_default(),
            };
            let owned = r.get::<i64, _>("owned") == 1;
            let list = presence.entry(tid).or_default();
            let u = match list.iter().position(|x| x.slug == slug) {
                Some(i) => &mut list[i],
                None => {
                    list.push(DigUserRow {
                        slug,
                        liked: false,
                        owned: Vec::new(),
                        followed: Vec::new(),
                    });
                    list.last_mut().unwrap()
                }
            };
            if owned {
                u.owned.push(pl);
            } else {
                u.followed.push(pl);
            }
        }

        // Likes (per user).
        let mut qb = QueryBuilder::new(
            "SELECT l.track_id AS tid, u.slug AS slug
               FROM hub_liked_tracks l JOIN hub_users u ON u.id = l.user_id
              WHERE l.track_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        qb.push(")");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let tid: i64 = r.get("tid");
            let slug: String = r.get("slug");
            let list = presence.entry(tid).or_default();
            match list.iter().position(|x| x.slug == slug) {
                Some(i) => list[i].liked = true,
                None => list.push(DigUserRow {
                    slug,
                    liked: true,
                    owned: Vec::new(),
                    followed: Vec::new(),
                }),
            }
        }

        // Resolved tags + their type.
        let mut qb = QueryBuilder::new(
            "SELECT rt.track_id AS tid, t.id AS tag_id, t.name AS tag, u.slug AS owner,
                    COALESCE(t.type, '') AS type
               FROM hub_track_resolved_tags rt
               JOIN hub_tags t ON t.id = rt.tag_id
               JOIN hub_users u ON u.id = t.owner_user_id
              WHERE rt.track_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        qb.push(") ORDER BY u.slug, t.name COLLATE NOCASE");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let tid: i64 = r.get("tid");
            let tag_id: i64 = r.get("tag_id");
            let tag: String = r.get::<Option<String>, _>("tag").unwrap_or_default();
            let owner: String = r.get::<Option<String>, _>("owner").unwrap_or_default();
            let ty: String = r.get::<Option<String>, _>("type").unwrap_or_default();
            let parsed = crate::tagtypes::TagType::parse(&ty);
            let list = tags.entry(tid).or_default();
            if list.iter().all(|x| x.id != tag_id) {
                list.push(DigTag {
                    id: tag_id,
                    name: tag,
                    owner,
                    type_value: if parsed.is_some() { ty } else { String::new() },
                    type_label: parsed.map(|t| t.label().to_string()).unwrap_or_default(),
                    type_icon: parsed.map(|t| t.icon().to_string()).unwrap_or_default(),
                });
            }
        }
    }

    (presence, tags)
}

async fn digging_page(
    State(st): State<AppState>,
    Query(q): Query<DiggingQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "digging").await else {
        return Redirect::to("/login").into_response();
    };
    let lastfm_enabled = st.cfg.lastfm_api_key.is_some();
    let freqblog_enabled = crate::freqblog::enabled(&st.cfg);
    let engine = crate::settings::engine(&st.pool).await;
    let freqblog_remaining = if freqblog_enabled {
        crate::freqblog::remaining(&st.pool, &st.cfg).await
    } else {
        0
    };

    let mut has_seed = false;
    let (mut seed_id, mut seed_title) = (0i64, String::new());
    let mut seed_tag_ids: HashSet<i64> = HashSet::new();
    let mut cand: HashMap<String, DigRow> = HashMap::new();

    if let Some(sid) = q.seed {
        if let Ok(Some(seed)) = crate::digging::load_seed(&st.pool, sid).await {
            has_seed = true;
            seed_id = seed.id;
            seed_title = seed.title.clone();

            // 1. Hub-intern: tracks sharing the seed's playlists (capped so the
            //    external discoveries below still fit the table).
            let seed_spotify = sqlx::query_scalar::<_, String>(
                "SELECT external_id FROM hub_track_external_ids
                  WHERE track_id = ?1 AND service = 'spotify' LIMIT 1",
            )
            .bind(sid)
            .fetch_optional(&st.pool)
            .await
            .ok()
            .flatten();
            // First artist from the entity table (no string splitting).
            let seed_artist = crate::artists::artists_for_track(&st.pool, sid)
                .await
                .into_iter()
                .next()
                .map(|(_, name)| name)
                .unwrap_or_default();
            seed_tag_ids = sqlx::query_scalar::<_, i64>(
                "SELECT tag_id FROM hub_track_resolved_tags WHERE track_id = ?1",
            )
            .bind(sid)
            .fetch_all(&st.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
            if engine.parent_match && !seed_tag_ids.is_empty() {
                let ids: Vec<i64> = seed_tag_ids.iter().copied().collect();
                seed_tag_ids = crate::tags::related_tag_ids(&st.pool, &ids).await;
            }

            // Fetch every source concurrently. External HTTP calls overlap instead
            // of chaining, so the page cost is the *max* latency, not the sum.
            let internal_fut = async {
                crate::digging::internal_suggestions(&st.pool, sid, 100)
                    .await
                    .unwrap_or_default()
            };
            let recco_fut = async {
                match &seed_spotify {
                    Some(sp) => crate::features::recommendations(&st.cfg, sp, 50)
                        .await
                        .unwrap_or_default(),
                    None => Vec::new(),
                }
            };
            let lastfm_fut = async {
                if lastfm_enabled {
                    crate::lastfm::similar_tracks(&st.cfg, &seed.artists, &seed.title)
                        .await
                        .unwrap_or_default()
                } else {
                    Vec::new()
                }
            };
            let cosine_fut = async {
                if crate::cosine::enabled(&st.cfg) {
                    crate::cosine::similar_tracks(&st.cfg, &seed_artist, &seed.title, 60)
                        .await
                        .unwrap_or_default()
                } else {
                    Vec::new()
                }
            };
            let audio_fut = async {
                crate::similar::neighbors(&st.pool, sid, 60)
                    .await
                    .unwrap_or_default()
            };
            let (internal, recco, lastfm_list, cosine_list, audio_list) =
                tokio::join!(internal_fut, recco_fut, lastfm_fut, cosine_fut, audio_fut);

            for s in internal {
                merge_candidate(&mut cand, Some(s.id), &s.title, &s.artists, "Hub-intern").await;
            }
            // Resolve external candidates against a preloaded matcher instead of
            // one full-table scan each.
            let recco_ids: Vec<String> = recco.iter().map(|r| r.spotify_id.clone()).collect();
            let matcher = crate::digging::Matcher::load(&st.pool, &recco_ids).await;
            for r in recco {
                let tid = matcher.by_spotify(&r.spotify_id);
                merge_candidate(&mut cand, tid, &r.title, &r.artists, "ReccoBeats").await;
            }
            for s in lastfm_list {
                let tid = matcher.by_name(&s.artist, &s.name);
                merge_candidate(&mut cand, tid, &s.name, &s.artist, "Last.fm").await;
            }
            for s in cosine_list {
                let tid = matcher.by_name(&s.artist, &s.track);
                merge_candidate(&mut cand, tid, &s.track, &s.artist, "cosine.club").await;
            }
            // Audio neighbours (EffNet): titles fetched in one batched query.
            let audio_ids: Vec<i64> = audio_list.iter().map(|n| n.track_id).collect();
            let mut titles: HashMap<i64, (String, String)> = HashMap::new();
            for chunk in audio_ids.chunks(900) {
                let mut qb =
                    QueryBuilder::new("SELECT id, title, artists FROM hub_tracks WHERE id IN (");
                let mut sep = qb.separated(", ");
                for id in chunk {
                    sep.push_bind(*id);
                }
                qb.push(")");
                for r in qb.build().fetch_all(&st.pool).await.unwrap_or_default() {
                    titles.insert(
                        r.get("id"),
                        (
                            r.get::<Option<String>, _>("title").unwrap_or_default(),
                            r.get::<Option<String>, _>("artists").unwrap_or_default(),
                        ),
                    );
                }
            }
            for n in audio_list {
                if let Some((title, artists)) = titles.get(&n.track_id) {
                    merge_candidate(&mut cand, Some(n.track_id), title, artists, "Audio").await;
                }
            }
        }
    }

    // Batched presence for the matched candidates (one query set, not N).
    let matched_ids: Vec<i64> = cand
        .values()
        .filter(|r| r.matched)
        .map(|r| r.track_id)
        .collect();
    if !matched_ids.is_empty() {
        let (presence_map, tags_map) = dig_details(&st.pool, &matched_ids).await;
        for r in cand.values_mut() {
            if !r.matched {
                continue;
            }
            r.presence = presence_map.get(&r.track_id).cloned().unwrap_or_default();
            r.tags = tags_map.get(&r.track_id).cloned().unwrap_or_default();
            r.users = r
                .presence
                .iter()
                .filter(|u| !u.owned.is_empty() || !u.followed.is_empty())
                .count() as i64;
            r.playlists = r
                .presence
                .iter()
                .map(|u| (u.owned.len() + u.followed.len()) as i64)
                .sum();
            r.likes = r.presence.iter().filter(|u| u.liked).count() as i64;
            // Scoring-engine signal: importance of the track's tags = the type
            // weight of each tag, summed over the track's tags.
            r.tag_weight = r
                .tags
                .iter()
                .map(|t| type_weight(&engine, &t.type_value))
                .sum();
            // Seed↔candidate agreement: only tags the seed also has.
            r.shared_weight = r
                .tags
                .iter()
                .filter(|t| seed_tag_ids.contains(&t.id))
                .map(|t| type_weight(&engine, &t.type_value))
                .sum();
        }

        // Ranking signals (#223): similarity v2 (seed↔candidate, Rumpelkiste-aware)
        // and ripeness (data completeness), both fed by the configurable engine.
        let sim_map =
            crate::similarity::similarity_to_seed(&st.pool, seed_id, &matched_ids, &engine).await;
        let ripe_map = crate::scoring::ripeness_many(&st.pool, &matched_ids, &engine).await;
        for r in cand.values_mut() {
            if !r.matched {
                continue;
            }
            r.similarity = sim_map.get(&r.track_id).copied().unwrap_or(0.0);
            r.ripeness = ripe_map.get(&r.track_id).copied().unwrap_or(0.0);
        }
    }

    // Load BPM/key for matched candidates (for the similarity filters).
    let ids: Vec<i64> = cand
        .values()
        .filter(|r| r.matched)
        .map(|r| r.track_id)
        .collect();
    if !ids.is_empty() {
        let mut qb = QueryBuilder::new(
            "SELECT track_id, bpm, camelot FROM hub_track_features WHERE found = 1 AND track_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in &ids {
            sep.push_bind(*id);
        }
        qb.push(")");
        if let Ok(frows) = qb.build().fetch_all(&st.pool).await {
            let feat: HashMap<i64, (Option<f64>, String)> = frows
                .iter()
                .map(|r| {
                    (
                        r.get::<i64, _>("track_id"),
                        (
                            r.get::<Option<f64>, _>("bpm"),
                            r.get::<Option<String>, _>("camelot").unwrap_or_default(),
                        ),
                    )
                })
                .collect();
            for r in cand.values_mut() {
                if r.matched {
                    if let Some((bpm, c)) = feat.get(&r.track_id) {
                        r.bpm = *bpm;
                        r.camelot = c.clone();
                    }
                }
            }
        }
    }

    // Seed features for the BPM/key filters.
    let (seed_bpm, seed_camelot) = sqlx::query(
        "SELECT bpm, camelot FROM hub_track_features WHERE track_id = ?1 AND found = 1",
    )
    .bind(seed_id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten()
    .map(|r| {
        (
            r.get::<Option<f64>, _>("bpm"),
            r.get::<Option<String>, _>("camelot").unwrap_or_default(),
        )
    })
    .unwrap_or((None, String::new()));

    let mine_only = q.mine.as_deref() == Some("1");
    let bpm_only = q.bpm.as_deref() == Some("1");
    let harm_only = q.harm.as_deref() == Some("1");
    let towner = q.towner.clone().unwrap_or_default().trim().to_string();
    let ttype = q
        .ttype
        .clone()
        .unwrap_or_default()
        .trim()
        .to_ascii_uppercase();
    let powner = q.powner.clone().unwrap_or_default().trim().to_string();
    let tag = q.tag.clone().unwrap_or_default().trim().to_string();
    let wmin = q.wmin.filter(|w| *w > 0.0).unwrap_or(0.0);
    let tag_ids: HashSet<i64> = if tag.is_empty() {
        HashSet::new()
    } else {
        crate::tags::matching_tag_ids(&st.pool, &tag).await
    };
    // Absolute BPM tolerance in whole BPM (0 = rounded-equal).
    let tol = q
        .tol
        .filter(|t| (0.0..=50.0).contains(t))
        .map(|t| t.round() as i64)
        .unwrap_or(1);

    let mut rows: Vec<DigRow> = cand.into_values().collect();
    rows.retain(|r| {
        if mine_only && !(r.users > 0 || r.likes > 0) {
            return false;
        }
        if bpm_only {
            match (seed_bpm, r.bpm) {
                (Some(sb), Some(b)) if sb > 0.0 && b > 0.0 => {
                    if (b.round() as i64 - sb.round() as i64).abs() > tol {
                        return false;
                    }
                }
                _ => return false,
            }
        }
        if harm_only && !crate::features::camelot_compatible(&seed_camelot, &r.camelot) {
            return false;
        }
        // Tag owner, tag type, playlist owner.
        if !towner.is_empty() && !r.tags.iter().any(|t| t.owner.eq_ignore_ascii_case(&towner)) {
            return false;
        }
        if !ttype.is_empty()
            && !r
                .tags
                .iter()
                .any(|t| t.type_value.eq_ignore_ascii_case(&ttype))
        {
            return false;
        }
        if !powner.is_empty()
            && !r.presence.iter().any(|u| {
                u.slug.eq_ignore_ascii_case(&powner)
                    && (!u.owned.is_empty() || !u.followed.is_empty())
            })
        {
            return false;
        }
        if !tag.is_empty() && !r.tags.iter().any(|t| tag_ids.contains(&t.id)) {
            return false;
        }
        if wmin > 0.0 && r.tag_weight < wmin {
            return false;
        }
        true
    });

    for r in rows.iter_mut() {
        // Ranking engine (configurable in the web UI): base signals + weighted
        // seed-match (shared + similarity v2) + candidate curation (own tags) +
        // ripeness (#223).
        let score = r.users as f64 * engine.base_users
            + r.playlists as f64 * engine.base_playlists
            + r.likes as f64 * engine.base_likes
            + r.sources.len() as f64 * engine.base_sources
            + r.shared_weight * engine.shared_factor
            + r.tag_weight * engine.candidate_factor
            + r.similarity * engine.sim_factor
            + r.ripeness * engine.ripeness_factor;
        r.score = score.round() as i64;
        r.bpm_disp = r
            .bpm
            .map(|b| format!("{b:.0}"))
            .unwrap_or_else(|| "—".to_string());
        r.tag_weight_disp = if r.tag_weight > 0.0 {
            format!("{:.0}", r.tag_weight)
        } else {
            "—".to_string()
        };
        r.shared_weight_disp = if r.shared_weight > 0.0 {
            format!("{:.0}", r.shared_weight)
        } else {
            "—".to_string()
        };
        r.similarity_disp = if r.similarity > 0.0 {
            format!("{:.1}", r.similarity)
        } else {
            "—".to_string()
        };
        r.ripeness_disp = if r.ripeness > 0.0 {
            format!("{:.0}", r.ripeness)
        } else {
            "—".to_string()
        };
    }
    rows.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.artists.cmp(&b.artists))
            .then_with(|| a.title.cmp(&b.title))
    });
    let with_hub = rows.iter().filter(|r| r.users > 0 || r.likes > 0).count();
    rows.truncate(500);
    // Link artist names for hub-matched candidates; escape the rest.
    let dig_ids: Vec<i64> = rows
        .iter()
        .filter(|r| r.matched)
        .map(|r| r.track_id)
        .collect();
    let mut dig_html = crate::artists::artists_html_for_tracks(&st.pool, &dig_ids).await;
    for r in rows.iter_mut() {
        r.artists_html = if r.matched {
            dig_html
                .remove(&r.track_id)
                .unwrap_or_else(|| crate::artists::escape_html(&r.artists))
        } else {
            crate::artists::escape_html(&r.artists)
        };
    }

    // Filter toggle links, preserving the other flags.
    let link = |mine: bool, bpm: bool, harm: bool, tol: i64| -> String {
        let mut s = format!("/digging?seed={seed_id}");
        if mine {
            s.push_str("&mine=1");
        }
        if bpm {
            s.push_str(&format!("&bpm=1&tol={tol}"));
        }
        if harm {
            s.push_str("&harm=1");
        }
        if !towner.is_empty() {
            s.push_str(&format!("&towner={}", urlencoding::encode(&towner)));
        }
        if ttype != "" {
            s.push_str(&format!("&ttype={}", urlencoding::encode(&ttype)));
        }
        if !powner.is_empty() {
            s.push_str(&format!("&powner={}", urlencoding::encode(&powner)));
        }
        if !tag.is_empty() {
            s.push_str(&format!("&tag={}", urlencoding::encode(&tag)));
        }
        if wmin > 0.0 {
            s.push_str(&format!("&wmin={wmin}"));
        }
        s
    };
    let bpm_label = if tol == 0 {
        "BPM exakt".to_string()
    } else {
        format!("BPM ±{tol}")
    };
    let filters = vec![
        ScopeLink {
            label: "Alle".to_string(),
            href: link(false, false, false, tol),
            active: !(mine_only || bpm_only || harm_only),
        },
        ScopeLink {
            label: format!("Nur bei uns ({with_hub})"),
            href: link(!mine_only, bpm_only, harm_only, tol),
            active: mine_only,
        },
        ScopeLink {
            label: bpm_label,
            href: link(mine_only, !bpm_only, harm_only, tol),
            active: bpm_only,
        },
        ScopeLink {
            label: "Harmonisch".to_string(),
            href: link(mine_only, bpm_only, !harm_only, tol),
            active: harm_only,
        },
    ];
    let bpm_opts: Vec<ScopeLink> = [0i64, 1, 2, 3, 5]
        .into_iter()
        .map(|n| ScopeLink {
            label: if n == 0 {
                "Exakt".to_string()
            } else {
                format!("±{n}")
            },
            href: link(mine_only, true, harm_only, n),
            active: bpm_only && tol == n,
        })
        .collect();

    // Option lists for the tag-owner / tag-group / playlist-owner selects.
    let tag_owner_opts: Vec<SelOpt> = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT u.slug FROM hub_tags t JOIN hub_users u ON u.id = t.owner_user_id
          ORDER BY u.slug",
    )
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|slug| SelOpt {
        selected: slug.eq_ignore_ascii_case(&towner),
        label: format!("@{slug}"),
        value: slug,
    })
    .collect();
    let tag_type_opts: Vec<SelOpt> = crate::tagtypes::options()
        .into_iter()
        .map(|(value, label, icon)| SelOpt {
            selected: value.eq_ignore_ascii_case(&ttype),
            value: value.to_string(),
            label: format!("{icon} {label}").trim().to_string(),
        })
        .collect();
    let pl_owner_opts: Vec<SelOpt> =
        sqlx::query_scalar::<_, String>("SELECT slug FROM hub_users ORDER BY slug")
            .fetch_all(&st.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|slug| SelOpt {
                selected: slug.eq_ignore_ascii_case(&powner),
                label: format!("@{slug}"),
                value: slug,
            })
            .collect();

    let reset_href = {
        let mut s = format!("/digging?seed={seed_id}");
        if mine_only {
            s.push_str("&mine=1");
        }
        if bpm_only {
            s.push_str(&format!("&bpm=1&tol={tol}"));
        }
        if harm_only {
            s.push_str("&harm=1");
        }
        s
    };

    render(&DiggingPage {
        nav,
        flash: String::new(),
        has_seed,
        seed_id,
        seed_title,
        seed_artists_html: crate::artists::artist_links_html(
            &crate::artists::artists_for_track(&st.pool, seed_id).await,
        ),
        rows,
        lastfm_enabled,
        freqblog_enabled,
        freqblog_remaining,
        seed_bpm: seed_bpm.map(|b| format!("{b:.0}")).unwrap_or_default(),
        seed_key: seed_camelot,
        filters,
        bpm_opts,
        tag_owner_opts,
        tag_type_opts,
        pl_owner_opts,
        tag,
        wmin,
        mine: mine_only,
        bpm: bpm_only,
        harm: harm_only,
        tol,
        reset_href,
    })
}

/// Manual FreqBlog enrichment for the active digging session: the seed plus its
/// hub-internal candidates (the tracks BPM/key filters care about). Bounded by a
/// per-click cap and the remaining monthly budget.
async fn digging_enrich(
    State(st): State<AppState>,
    Query(q): Query<DiggingQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(_nav) = crate::ui::nav(&st, &headers, "digging").await else {
        return Redirect::to("/login").into_response();
    };
    let Some(sid) = q.seed else {
        return Redirect::to("/digging").into_response();
    };
    if crate::freqblog::enabled(&st.cfg) {
        let mut ids = vec![sid];
        for s in crate::digging::internal_suggestions(&st.pool, sid, 100)
            .await
            .unwrap_or_default()
        {
            ids.push(s.id);
        }
        let _ = crate::freqblog::enrich_tracks(&st.pool, &st.cfg, &ids, 40).await;
    }
    Redirect::to(&format!("/digging?seed={sid}")).into_response()
}

// ── ops status panel (issue #353) ───────────────────────────────────────────

#[derive(Template)]
#[template(path = "_partials/hub_status.html")]
struct HubStatusFrag {
    workers: Vec<WorkerRow>,
    backlog: Vec<BacklogRow>,
    music_api_configured: bool,
    music_api_reachable: bool,
    worker_paused: bool,
    interval_secs: u64,
    max_attempts: i64,
    queue_depth: i64,
}

struct WorkerRow {
    name: String,
    last_run: String,
    last_worked: bool,
    iterations: u64,
    last_error: String,
}

struct BacklogRow {
    label: String,
    count: i64,
}

/// `GET /api/hub/status/panel` — admin-only htmx fragment for `/admin`.
async fn status_panel(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "admin").await else {
        return Redirect::to("/login").into_response();
    };
    if !nav.is_admin {
        return (StatusCode::FORBIDDEN, "Nur Admins.").into_response();
    }

    let snap = st.worker_health.snapshot();
    let workers: Vec<WorkerRow> = snap
        .loops
        .iter()
        .map(|l| WorkerRow {
            name: l.name.to_string(),
            last_run: l.last_run_at.clone().unwrap_or_else(|| "—".to_string()),
            last_worked: l.last_worked,
            iterations: l.iterations,
            last_error: l.last_error.clone().unwrap_or_default(),
        })
        .collect();

    let b = crate::api::backlog_counts(&st.ro_pool).await;
    let backlog = vec![
        BacklogRow { label: "Features".to_string(), count: b.features_pending },
        BacklogRow { label: "Requests".to_string(), count: b.feature_requests },
        BacklogRow { label: "Genres".to_string(), count: b.genres_pending },
        BacklogRow { label: "Crawl".to_string(), count: b.crawl_pending },
        BacklogRow { label: "Curation fällig".to_string(), count: b.curation_due },
        BacklogRow { label: "Likes".to_string(), count: b.likes_queued },
        BacklogRow { label: "Playlists".to_string(), count: b.playlists_pending },
    ];

    let configured = st.cfg.music_api_token.is_some();
    let wi = crate::music_api::worker_info(&st.cfg).await;
    let (items, _err, _counts) = crate::music_api::live_queue(&st.cfg).await;

    render(&HubStatusFrag {
        workers,
        backlog,
        music_api_configured: configured,
        music_api_reachable: wi.ok,
        worker_paused: wi.paused,
        interval_secs: wi.interval_secs,
        max_attempts: wi.max_attempts,
        queue_depth: items.len() as i64,
    })
}

// ── admin (settings) ────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct AdminMsg {
    msg: Option<String>,
    /// Optional table filter/sort (shared by pages that reuse this query struct).
    q: Option<String>,
    sort: Option<String>,
    dir: Option<String>,
    /// Secondary table sort (pages with two tables, e.g. collective members).
    msort: Option<String>,
    mdir: Option<String>,
}

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminPage {
    nav: crate::ui::Nav,
    flash: String,
    fields: Vec<AdminField>,
    registration_open: bool,
    q: String,
    users: Vec<AdminUser>,
    s_user: crate::table::SortHead,
    s_admin: crate::table::SortHead,
}

struct AdminField {
    key: String,
    label: String,
    secret: bool,
    /// Widget: "num" | "bool" | "enum" | "text".
    kind: String,
    min: String,
    max: String,
    default: String,
    hint: String,
    set: bool,
    value: String,
    checked: bool,
    options: Vec<AdminOption>,
}

struct AdminOption {
    value: String,
    selected: bool,
}

struct AdminUser {
    slug: String,
    is_admin: bool,
}

async fn admin_page(
    State(st): State<AppState>,
    Query(msg): Query<AdminMsg>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "admin").await else {
        return Redirect::to("/login").into_response();
    };
    if !nav.is_admin {
        return (StatusCode::FORBIDDEN, "Nur Admins.").into_response();
    }

    use crate::settings::SettingKind;
    let s = crate::settings::load_all(&st.pool).await;
    let fmt = |o: Option<f64>| o.map(|x| format!("{x}")).unwrap_or_default();
    let fields = crate::settings::SETTINGS
        .iter()
        .map(|spec| {
            let stored = s.get(spec.key).cloned().unwrap_or_default();
            let is_set = !stored.is_empty();
            let mut options = Vec::new();
            let (kind, min, max, hint) = match spec.kind {
                SettingKind::Num => (
                    "num",
                    fmt(spec.min),
                    fmt(spec.max),
                    format!(
                        "Default {} · {}–{}",
                        spec.default,
                        fmt(spec.min),
                        fmt(spec.max)
                    ),
                ),
                SettingKind::Bool => (
                    "bool",
                    String::new(),
                    String::new(),
                    if spec.default == "1" {
                        "Default an".to_string()
                    } else {
                        "Default aus".to_string()
                    },
                ),
                SettingKind::Csv => (
                    "text",
                    String::new(),
                    String::new(),
                    format!("CSV · Default {}", spec.default),
                ),
                SettingKind::Enum(allowed) => {
                    let cur = if stored.is_empty() {
                        spec.default.to_string()
                    } else {
                        stored.clone()
                    };
                    for a in allowed {
                        options.push(AdminOption {
                            value: (*a).to_string(),
                            selected: *a == cur,
                        });
                    }
                    (
                        "enum",
                        String::new(),
                        String::new(),
                        format!("Default {}", spec.default),
                    )
                }
                SettingKind::Text => (
                    "text",
                    String::new(),
                    String::new(),
                    if spec.secret {
                        "geheim".to_string()
                    } else {
                        String::new()
                    },
                ),
            };
            AdminField {
                key: spec.key.to_string(),
                label: spec.label.to_string(),
                secret: spec.secret,
                kind: kind.to_string(),
                min,
                max,
                default: spec.default.to_string(),
                hint,
                set: is_set,
                value: if spec.secret {
                    String::new()
                } else if matches!(spec.kind, SettingKind::Enum(_)) {
                    String::new()
                } else {
                    stored.clone()
                },
                checked: stored == "1" || (stored.is_empty() && spec.default == "1"),
                options,
            }
        })
        .collect();
    let registration_open = crate::settings::registration_open(&st.pool).await;
    let q = msg.q.clone().unwrap_or_default().trim().to_string();
    let toks = crate::table::tokens(&q);
    let allowed: &[(&str, &str)] = &[("user", "slug"), ("admin", "is_admin")];
    let (sort, dir) = crate::table::resolve_sort(
        msg.sort.as_deref(),
        msg.dir.as_deref(),
        allowed,
        "user",
        "asc",
    );
    let mut users: Vec<AdminUser> =
        sqlx::query_as::<_, (String, i64)>("SELECT slug, is_admin FROM hub_users")
            .fetch_all(&st.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|(slug, admin)| AdminUser {
                slug,
                is_admin: admin == 1,
            })
            .filter(|u| crate::table::matches_typo(&u.slug.to_lowercase(), &toks))
            .collect();
    let desc = dir == "desc";
    users.sort_by(|a, b| {
        let o = if sort == "admin" {
            a.is_admin.cmp(&b.is_admin)
        } else {
            a.slug.to_lowercase().cmp(&b.slug.to_lowercase())
        };
        if desc { o.reverse() } else { o }
    });
    let rawq = raw.as_deref().unwrap_or("");
    let s_user = crate::table::sort_head(rawq, "user", "Nutzer", &sort, &dir);
    let s_admin = crate::table::sort_head(rawq, "admin", "Admin", &sort, &dir);

    render(&AdminPage {
        nav,
        flash: msg.msg.unwrap_or_default(),
        fields,
        registration_open,
        q,
        users,
        s_user,
        s_admin,
    })
}

async fn admin_save(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(map): Form<std::collections::HashMap<String, String>>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "admin").await else {
        return Redirect::to("/login").into_response();
    };
    if !nav.is_admin {
        return (StatusCode::FORBIDDEN, "Nur Admins.").into_response();
    }

    use crate::settings::SettingKind;
    let mut errors: Vec<String> = Vec::new();
    for spec in crate::settings::SETTINGS {
        // Booleans are always written (unchecked boxes are absent from the form),
        // so a flag can be turned back off.
        if matches!(spec.kind, SettingKind::Bool) {
            let on = map
                .get(spec.key)
                .map(|v| {
                    matches!(
                        v.trim().to_lowercase().as_str(),
                        "1" | "true" | "on" | "yes"
                    )
                })
                .unwrap_or(false);
            let _ = crate::settings::set(&st.pool, spec.key, if on { "1" } else { "0" }).await;
            continue;
        }
        // Everything else: non-empty overwrites, empty means "leave unchanged".
        if let Some(raw) = map.get(spec.key) {
            if raw.trim().is_empty() {
                continue;
            }
            match crate::settings::validate(spec, raw) {
                Ok(v) => {
                    let _ = crate::settings::set(&st.pool, spec.key, &v).await;
                }
                Err(e) => errors.push(e),
            }
        }
    }
    let reg = map
        .get(crate::settings::REGISTRATION_OPEN)
        .map(|v| v == "on" || v == "1")
        .unwrap_or(false);
    let _ = crate::settings::set(
        &st.pool,
        crate::settings::REGISTRATION_OPEN,
        if reg { "1" } else { "0" },
    )
    .await;

    let msg = if errors.is_empty() {
        "Gespeichert — Keys greifen nach Neustart".to_string()
    } else {
        format!("Fehler: {}", errors.join("; "))
    };
    Redirect::to(&format!("/admin?msg={}", urlencoding::encode(&msg))).into_response()
}

// ── tags (resolved playlist layer) ──────────────────────────────────────────

#[derive(Deserialize, Default)]
struct TagFilter {
    q: Option<String>,
    /// "1" = only my tags.
    mine: Option<String>,
    // NOTE: `types` (additive, possibly repeated) is read from the raw query in
    // `tags_page` — a duplicate-key field here would make serde reject `?types=A&types=B`.
    /// Filter to tags owned by a member of this collective.
    #[serde(default, deserialize_with = "crate::table::qnum::opt_i64")]
    collective: Option<i64>,
    /// Filter to tags owned by this user slug.
    owner: Option<String>,
    /// Sort field (`table::sort_head` whitelist) and direction.
    sort: Option<String>,
    dir: Option<String>,
}

#[derive(Template)]
#[template(path = "tags.html")]
struct TagsPage {
    nav: crate::ui::Nav,
    flash: String,
    q: String,
    mine: bool,
    type_options: Vec<TypeOption>,
    types_label: String,
    owner_options: Vec<OwnerOption>,
    collective_options: Vec<ParentOption>,
    tags: Vec<TagRow>,
    s_name: crate::table::SortHead,
    s_owner: crate::table::SortHead,
    s_type: crate::table::SortHead,
    s_parents: crate::table::SortHead,
    s_tracks: crate::table::SortHead,
    s_sources: crate::table::SortHead,
}

/// A selectable tag type in the tag-page type filter.
struct TypeOption {
    value: &'static str,
    label: &'static str,
    icon: &'static str,
    selected: bool,
}

struct TagRow {
    id: i64,
    name: String,
    owner: String,
    type_label: String,
    parents: String,
    track_count: i64,
    source_count: i64,
}

/// Append a #331 scope predicate `{lhs} IN (SELECT value FROM json_each(?))`
/// to `qb`.
///
/// The scope is bound as **one** JSON array, so the number of SQL variables is
/// constant regardless of the backpack size. A bound literal `IN (?, ?, …)`
/// list would grow one variable per scoped track and could exceed SQLite's
/// `SQLITE_MAX_VARIABLE_NUMBER` (and in `/tags` the predicate is emitted twice,
/// doubling the count) — `json_each` is the temp-table/JOIN-style solution,
/// without needing a per-request temp table and its connection pinning.
///
/// An empty scope yields the always-false `1 = 0`: SQLite rejects a bare
/// `IN ()` with a syntax error, so an active backpack that resolves no tracks
/// must hide everything instead of breaking the `/tags` / `/albums` query.
fn push_scope_ids(qb: &mut QueryBuilder<'_, sqlx::Sqlite>, lhs: &str, scope: &HashSet<i64>) {
    if scope.is_empty() {
        qb.push("1 = 0");
        return;
    }
    let json = serde_json::to_string(&scope.iter().copied().collect::<Vec<i64>>())
        .expect("serialize scope ids");
    qb.push(lhs)
        .push(" IN (SELECT value FROM json_each(")
        .push_bind(json)
        .push("))");
}

#[allow(clippy::too_many_arguments)]
async fn run_tags(
    pool: &sqlx::SqlitePool,
    q: &str,
    me: i64,
    mine: bool,
    owner: &str,
    collective_id: i64,
    type_values: &[String],
    sort: &str,
    dir: &str,
    allowed: &[(&str, &str)],
    strict: bool,
    limit: i64,
    scope: Option<&HashSet<i64>>,
) -> Vec<TagRow> {
    let mut qb: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new(
        "SELECT * FROM (SELECT t.id AS id, t.name AS name, u.slug AS owner,
                COALESCE(t.type,'') AS type_name,
                (SELECT GROUP_CONCAT(p.name, ' · ')
                   FROM hub_tag_parents tp JOIN hub_tags p ON p.id = tp.parent_tag_id
                  WHERE tp.tag_id = t.id) AS parents,",
    );
    // Track count reflects the active scope when one is set (honest numbers).
    if let Some(scope) = scope {
        qb.push("(SELECT COUNT(*) FROM hub_track_resolved_tags r WHERE r.tag_id = t.id AND ");
        push_scope_ids(&mut qb, "r.track_id", scope);
        qb.push(") AS track_count,");
    } else {
        qb.push(
            "(SELECT COUNT(*) FROM hub_track_resolved_tags r WHERE r.tag_id = t.id) AS track_count,",
        );
    }
    qb.push(
        "(SELECT COUNT(*) FROM hub_tag_sources s WHERE s.tag_id = t.id) AS source_count
           FROM hub_tags t
           JOIN hub_users u ON u.id = t.owner_user_id
          WHERE 1 = 1",
    );
    if mine {
        qb.push(" AND t.owner_user_id = ").push_bind(me);
    }
    if !owner.is_empty() {
        qb.push(" AND u.slug = ")
            .push_bind(owner.to_string())
            .push(" COLLATE NOCASE");
    }
    if collective_id != 0 {
        qb.push(
            " AND t.owner_user_id IN (SELECT user_id FROM hub_collective_members
                                       WHERE collective_id = ",
        )
        .push_bind(collective_id)
        .push(")");
    }
    if !type_values.is_empty() {
        qb.push(" AND t.type IN (");
        {
            let mut sep = qb.separated(", ");
            for tv in type_values {
                sep.push_bind(tv.clone());
            }
        }
        qb.push(")");
    }
    // Scope filter (#331): a tag is listed only when >= 1 of its tracks is in
    // the active backpack's scope. No active backpack -> no extra condition.
    if let Some(scope) = scope {
        qb.push(" AND EXISTS (SELECT 1 FROM hub_track_resolved_tags r WHERE r.tag_id = t.id AND ");
        push_scope_ids(&mut qb, "r.track_id", scope);
        qb.push(")");
    }
    qb.push(") WHERE 1 = 1");
    if strict {
        crate::table::push_fuzzy(&mut qb, q, &["name", "owner", "type_name", "parents"]);
    }
    crate::table::order_by(&mut qb, sort, dir, allowed, "name");
    qb.push(" LIMIT ").push_bind(limit);
    qb.build()
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| {
            let ty: String = r.get::<Option<String>, _>("type_name").unwrap_or_default();
            let type_label = crate::tagtypes::TagType::parse(&ty)
                .map(|t| format!("{} {}", t.icon(), t.label()))
                .unwrap_or_default();
            TagRow {
                id: r.get::<i64, _>("id"),
                name: r.get::<Option<String>, _>("name").unwrap_or_default(),
                owner: r.get::<Option<String>, _>("owner").unwrap_or_default(),
                type_label,
                parents: r.get::<Option<String>, _>("parents").unwrap_or_default(),
                track_count: r.get::<Option<i64>, _>("track_count").unwrap_or(0),
                source_count: r.get::<Option<i64>, _>("source_count").unwrap_or(0),
            }
        })
        .collect()
}

async fn tags_page(
    State(st): State<AppState>,
    Query(f): Query<TagFilter>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tags").await else {
        return Redirect::to("/login").into_response();
    };
    let q = f.q.unwrap_or_default().trim().to_string();
    let mine = f.mine.as_deref() == Some("1");
    let me = nav.id;
    // Multi-type selection, additive: accept both `types=MOOD,VIBE` and repeated
    // `types=MOOD&types=VIBE` (plain checkboxes). Collected from the raw query.
    let mut type_values: Vec<String> = Vec::new();
    for (k, v) in raw
        .as_deref()
        .unwrap_or("")
        .split('&')
        .filter_map(|kv| kv.split_once('='))
    {
        if k != "types" {
            continue;
        }
        for part in v.split(',') {
            let val = part.trim().to_ascii_uppercase();
            if !val.is_empty()
                && crate::tagtypes::TagType::parse(&val).is_some()
                && !type_values.contains(&val)
            {
                type_values.push(val);
            }
        }
    }
    let owner = f.owner.unwrap_or_default().trim().to_string();
    let collective_id = f.collective.unwrap_or(0);

    let allowed: &[(&str, &str)] = &[
        ("name", "name"),
        ("owner", "owner"),
        ("type", "type_name"),
        ("parents", "parents"),
        ("tracks", "track_count"),
        ("sources", "source_count"),
    ];
    let (sort, dir) =
        crate::table::resolve_sort(f.sort.as_deref(), f.dir.as_deref(), allowed, "name", "asc");

    // Active-scope filter (#331): loaded once per request; `None` = unscoped.
    let scope = crate::backpacks::active_scope_for_views(&st.pool, me).await;

    // Strict token match first; on no hits, a broad pass + one-edit typo tolerance.
    let mut tags = run_tags(
        &st.pool,
        &q,
        me,
        mine,
        &owner,
        collective_id,
        &type_values,
        &sort,
        &dir,
        allowed,
        true,
        1000,
        scope.as_ref(),
    )
    .await;
    if tags.is_empty() && !q.is_empty() {
        let toks = crate::table::tokens(&q);
        tags = run_tags(
            &st.pool,
            &q,
            me,
            mine,
            &owner,
            collective_id,
            &type_values,
            &sort,
            &dir,
            allowed,
            false,
            3000,
            scope.as_ref(),
        )
        .await;
        tags.retain(|t| {
            crate::table::matches_typo(
                &format!("{} {} {} {}", t.name, t.owner, t.type_label, t.parents).to_lowercase(),
                &toks,
            )
        });
        tags.truncate(1000);
    }

    let owner_options: Vec<OwnerOption> = crate::tags::tag_owners(&st.pool)
        .await
        .into_iter()
        .map(|slug| OwnerOption {
            selected: slug.eq_ignore_ascii_case(&owner),
            slug,
        })
        .collect();
    let collective_options: Vec<ParentOption> = crate::tags::collectives_i_belong(&st.pool, me)
        .await
        .into_iter()
        .map(|(id, name, icon)| ParentOption {
            selected: id == collective_id,
            id,
            name,
            icon,
        })
        .collect();
    let type_options: Vec<TypeOption> = crate::tagtypes::options()
        .into_iter()
        .map(|(value, label, icon)| TypeOption {
            value,
            label,
            icon,
            selected: type_values.iter().any(|v| v.eq_ignore_ascii_case(value)),
        })
        .collect();
    let types_label = type_options
        .iter()
        .filter(|t| t.selected)
        .map(|t| format!("{} {}", t.icon, t.label).trim().to_string())
        .collect::<Vec<_>>()
        .join(" · ");
    let rawq = raw.as_deref().unwrap_or("");
    let s_name = crate::table::sort_head(rawq, "name", "Tag", &sort, &dir);
    let s_owner = crate::table::sort_head(rawq, "owner", "Besitzer", &sort, &dir);
    let s_type = crate::table::sort_head(rawq, "type", "Typ", &sort, &dir);
    let s_parents = crate::table::sort_head(rawq, "parents", "Parent-Tags", &sort, &dir);
    let s_tracks = crate::table::sort_head(rawq, "tracks", "Tracks", &sort, &dir);
    let s_sources = crate::table::sort_head(rawq, "sources", "Quellen", &sort, &dir);
    render(&TagsPage {
        nav,
        flash: String::new(),
        q,
        mine,
        type_options,
        types_label,
        owner_options,
        collective_options,
        tags,
        s_name,
        s_owner,
        s_type,
        s_parents,
        s_tracks,
        s_sources,
    })
}

#[derive(Deserialize)]
struct TagCreateForm {
    name: String,
    /// Optional tag type to apply to the new tag.
    #[serde(default, rename = "type")]
    tag_type: Option<String>,
}

/// Issue #206: create a tag directly from the tag page (owner = current user).
async fn tag_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<TagCreateForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tags").await else {
        return Redirect::to("/login").into_response();
    };
    if f.name.trim().is_empty() {
        return Redirect::to("/tags").into_response();
    }
    if let Ok(tag_id) = crate::tags::ensure_tag(&st.pool, nav.id, &f.name).await {
        if let Some(ty) = f
            .tag_type
            .as_deref()
            .and_then(crate::tagtypes::TagType::parse)
        {
            // Best-effort: the tag is created regardless.
            let _ = crate::tagtypes::set(&st.pool, nav.id, tag_id, Some(ty)).await;
        }
    }
    Redirect::to("/tags").into_response()
}

/// Extract the path portion of a `Referer` header (absolute or relative).
fn referer_path(headers: &HeaderMap) -> Option<String> {
    let r = headers.get(axum::http::header::REFERER)?.to_str().ok()?;
    let path = match r.find("://") {
        Some(i) => {
            let after = &r[i + 3..];
            match after.find('/') {
                Some(j) => &after[j..],
                None => return None,
            }
        }
        None => r,
    };
    // Only accept a genuine single-slash-rooted path. Reject protocol-relative
    // (`//evil.test/x`) and backslash (`/\evil.test`) vectors so the value can
    // never be reflected into a `Location` header as an open redirect.
    let mut chars = path.chars();
    if chars.next() != Some('/') {
        return None;
    }
    if matches!(chars.next(), Some('/') | Some('\\')) {
        return None;
    }
    Some(path.to_string())
}

/// One source row inside a backpack card: kind literal + plain-text label.
struct SourceRow {
    kind: String,
    label: String,
}

/// One backpack card of the `/backpacks` view.
struct BackpackCard {
    id: i64,
    name: String,
    is_active: bool,
    source_count: usize,
    track_count: usize,
    sources: Vec<SourceRow>,
}

#[derive(Template)]
#[template(path = "backpacks.html")]
struct BackpacksPage {
    nav: crate::ui::Nav,
    flash: String,
    cards: Vec<BackpackCard>,
}

/// GET `/backpacks` (#332): read-only overview of the caller's backpacks, each
/// with its source count, live track count (unclamped resolve) and labelled
/// sources. No session -> `/login`. Reads the optional `?msg=` flash set by the
/// mutating handlers (#328).
async fn backpacks_page(
    State(st): State<AppState>,
    Query(msg): Query<AdminMsg>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "backpacks").await else {
        return Redirect::to("/login").into_response();
    };

    let backpacks = match crate::backpacks::list_backpacks(&st.pool, nav.id).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("backpacks list failed for user {}: {e:#}", nav.id);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Backpacks konnten nicht geladen werden",
            )
                .into_response();
        }
    };

    let mut cards = Vec::with_capacity(backpacks.len());
    for bp in backpacks {
        let views = crate::backpacks::list_source_views(&st.pool, bp.id)
            .await
            .unwrap_or_default();
        let track_count = crate::backpacks::resolve_count(&st.pool, bp.id)
            .await
            .unwrap_or(0);
        cards.push(BackpackCard {
            id: bp.id,
            name: bp.name,
            is_active: bp.is_active != 0,
            source_count: views.len(),
            track_count,
            sources: views
                .into_iter()
                .map(|v| SourceRow {
                    kind: v.kind,
                    label: v.label,
                })
                .collect(),
        });
    }

    render(&BackpacksPage {
        nav,
        flash: msg.msg.unwrap_or_default(),
        cards,
    })
}

/// POST `/backpacks/active/clear` (#331): deactivate the caller's active
/// backpack scope, then redirect back to the referring view (or `/tags`).
async fn backpack_active_clear(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tags").await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::backpacks::clear_active(&st.pool, nav.id).await;
    let back = referer_path(&headers).unwrap_or_else(|| "/tags".to_string());
    Redirect::to(&back).into_response()
}

// ── backpack CRUD (#328) ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct BackpackCreateForm {
    name: String,
}

#[derive(Deserialize)]
struct BackpackRenameForm {
    name: String,
}

/// POST `/backpacks/create` (#328): create a backpack for the caller, then
/// redirect back to `/backpacks`. Errors (empty/duplicate name) become a
/// `?msg=`-flash, never a 500.
async fn backpack_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<BackpackCreateForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "backpacks").await else {
        return Redirect::to("/login").into_response();
    };
    match crate::backpacks::create_backpack(&st.pool, nav.id, &f.name).await {
        Ok(_) => Redirect::to("/backpacks").into_response(),
        Err(e) => flash_redirect("/backpacks", format!("Fehler: {e}")),
    }
}

/// POST `/backpacks/{id}/rename` (#328): rename one of the caller's backpacks.
/// Foreign/missing targets and empty/duplicate names become a `?msg=`-flash.
async fn backpack_rename(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<BackpackRenameForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "backpacks").await else {
        return Redirect::to("/login").into_response();
    };
    match crate::backpacks::rename_backpack(&st.pool, nav.id, id, &f.name).await {
        Ok(()) => Redirect::to("/backpacks").into_response(),
        Err(e) => flash_redirect("/backpacks", format!("Fehler: {e}")),
    }
}

/// POST `/backpacks/{id}/delete` (#328): delete one of the caller's backpacks
/// (items go via CASCADE). Foreign/missing targets become a `?msg=`-flash.
async fn backpack_delete(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "backpacks").await else {
        return Redirect::to("/login").into_response();
    };
    match crate::backpacks::delete_backpack(&st.pool, nav.id, id).await {
        Ok(true) => Redirect::to("/backpacks").into_response(),
        Ok(false) => flash_redirect("/backpacks", "Backpack nicht gefunden".to_string()),
        Err(e) => flash_redirect("/backpacks", format!("Fehler: {e}")),
    }
}

/// POST `/backpacks/{id}/active` (#328): toggle the caller's active backpack.
/// Activates a currently inactive target (via [`backpacks::set_active`], which
/// enforces ownership and shifts the single active row) or deactivates an
/// already active one (via [`backpacks::clear_active`]). A foreign backpack is a
/// flash error; the "at most one active" invariant is left to the existing
/// helpers.
async fn backpack_toggle_active(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "backpacks").await else {
        return Redirect::to("/login").into_response();
    };
    let is_active = match crate::backpacks::active_backpack(&st.pool, nav.id).await {
        Ok(active) => active.map(|bp| bp.id == id).unwrap_or(false),
        Err(e) => return flash_redirect("/backpacks", format!("Fehler: {e}")),
    };
    let result = if is_active {
        crate::backpacks::clear_active(&st.pool, nav.id)
            .await
            .map(|_| ())
    } else {
        crate::backpacks::set_active(&st.pool, nav.id, id).await
    };
    match result {
        Ok(()) => Redirect::to("/backpacks").into_response(),
        Err(e) => flash_redirect("/backpacks", format!("Fehler: {e}")),
    }
}

// ── backpack export (#333) ───────────────────────────────────────────────────

#[derive(Deserialize)]
struct ExportQuery {
    /// `m3u8` (default) | `traktor` | `zip`.
    format: Option<String>,
    /// Audio format for `traktor`/`zip`, validated by [`valid_format`].
    audio: Option<String>,
}

/// Normalize the export format: `traktor` / `zip` pass through, anything else
/// (including `None`) defaults to `m3u8`.
fn valid_export_format(f: Option<String>) -> String {
    match f.as_deref() {
        Some("traktor") => "traktor".to_string(),
        Some("zip") => "zip".to_string(),
        _ => "m3u8".to_string(),
    }
}

/// Load the backpack's resolved tracks as ZIP rows, in resolve order.
///
/// Columns match what [`build_playlist_zip`] reads (`isrc`, `title`, `artists`).
/// A resolve failure yields an empty `Vec` (the ZIP path then reports the 502
/// "no files" branch, mirroring the other download handlers).
async fn backpack_zip_rows(
    pool: &sqlx::SqlitePool,
    backpack_id: i64,
) -> Vec<sqlx::sqlite::SqliteRow> {
    let resolved = match crate::backpacks::resolve_unbounded(pool, backpack_id).await {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let mut rows = Vec::with_capacity(resolved.len());
    for rt in resolved {
        if let Ok(Some(row)) =
            sqlx::query("SELECT isrc, title, artists FROM hub_tracks WHERE id = ?1")
                .bind(rt.track_id)
                .fetch_optional(pool)
                .await
        {
            rows.push(row);
        }
    }
    rows
}

/// `GET /backpacks/{id}/export?format=m3u8|traktor|zip[&audio=flac]` (#333):
/// export the caller's resolved backpack track list.
///
/// No session -> `/login`; a foreign/unknown backpack -> `404` (owner scoping
/// via [`crate::backpacks::backpack_owned_by`], never a cross-user leak). The
/// body is the resolve-ordered list: `m3u8` text playlist (`audio/x-mpegurl`),
/// Traktor NML (`application/xml`), or — for `zip` — the actual audio bytes via
/// the existing [`zip_for`] path (`502` when music-api has none). The m3u8
/// stream URLs are session-bound (see [`crate::backpacks::m3u8_playlist`]).
async fn backpack_export(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Query(q): Query<ExportQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "backpacks").await else {
        return Redirect::to("/login").into_response();
    };
    let Some(bp) = crate::backpacks::backpack_owned_by(&st.pool, id, nav.id)
        .await
        .ok()
        .flatten()
    else {
        return not_found("Backpack nicht gefunden.");
    };

    let format = valid_export_format(q.format);
    let audio = valid_format(q.audio);
    let label = sanitize_filename(&bp.name);

    match format.as_str() {
        "traktor" => {
            let tracks = crate::backpacks::export_tracks(&st.pool, id)
                .await
                .unwrap_or_default();
            let body = crate::backpacks::traktor_nml(&bp.name, &tracks, &audio);
            attachment(
                body,
                "application/xml; charset=utf-8",
                format!("{label}.nml"),
            )
        }
        "zip" => {
            let rows = backpack_zip_rows(&st.pool, id).await;
            zip_for(&st, &rows, &audio, &label).await
        }
        _ => {
            let tracks = crate::backpacks::export_tracks(&st.pool, id)
                .await
                .unwrap_or_default();
            let body = crate::backpacks::m3u8_playlist(&bp.name, &tracks);
            attachment(
                body,
                "audio/x-mpegurl; charset=utf-8",
                format!("{label}.m3u8"),
            )
        }
    }
}

// ── weighted tag search + revisit (#295, #297) ────────────────────────────────

#[derive(Deserialize, Default)]
struct TagSearchQuery {
    /// `name:strength` pairs, comma-separated. Strength optional (default 1).
    tags: Option<String>,
    /// Optional seed track ids (comma-separated): their tags become the query.
    seed: Option<String>,
    msg: Option<String>,
}

struct TagSearchRow {
    id: i64,
    title: String,
    artists_html: String,
    score: String,
    matched: i64,
}

#[derive(Template)]
#[template(path = "tag_search.html")]
struct TagSearchPage {
    nav: crate::ui::Nav,
    flash: String,
    tags_input: String,
    seed_input: String,
    results: Vec<TagSearchRow>,
    count: usize,
    searched: bool,
}

fn parse_tag_query(raw: &str) -> Vec<(String, f64)> {
    raw.split(',')
        .filter_map(|p| {
            let p = p.trim();
            if p.is_empty() {
                return None;
            }
            match p.split_once(':') {
                Some((name, s)) => Some((
                    name.trim().to_string(),
                    s.trim().parse::<f64>().unwrap_or(1.0).max(1.0),
                )),
                None => Some((p.to_string(), 1.0)),
            }
        })
        .filter(|(n, _)| !n.is_empty())
        .collect()
}

async fn tag_search_page(
    State(st): State<AppState>,
    Query(q): Query<TagSearchQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tag-search").await else {
        return Redirect::to("/login").into_response();
    };
    let tags_input = q.tags.clone().unwrap_or_default();
    let seed_input = q.seed.clone().unwrap_or_default();
    let explicit = parse_tag_query(&tags_input);
    let seed: Vec<i64> = seed_input
        .split(&[',', ' '][..])
        .filter_map(|s| s.trim().parse::<i64>().ok())
        .collect();
    let searched = !explicit.is_empty() || !seed.is_empty();
    let hits = if searched {
        crate::tagsearch::weighted_search(&st.pool, &explicit, &seed, 500).await
    } else {
        Vec::new()
    };
    let mut results: Vec<TagSearchRow> = hits
        .into_iter()
        .map(|h| TagSearchRow {
            id: h.track_id,
            title: h.title,
            artists_html: String::new(),
            score: format!("{:.2}", h.score),
            matched: h.matched,
        })
        .collect();
    let ts_ids: Vec<i64> = results.iter().map(|r| r.id).collect();
    let mut ts_html = crate::artists::artists_html_for_tracks(&st.pool, &ts_ids).await;
    for r in results.iter_mut() {
        r.artists_html = ts_html.remove(&r.id).unwrap_or_default();
    }
    render(&TagSearchPage {
        nav,
        flash: q.msg.unwrap_or_default(),
        tags_input,
        seed_input,
        count: results.len(),
        results,
        searched,
    })
}

#[derive(Deserialize, Default)]
struct RevisitQuery {
    q: Option<String>,
}

struct RevisitRow {
    id: i64,
    title: String,
    artists_html: String,
    last_tagged: String,
    missed: i64,
}

#[derive(Template)]
#[template(path = "revisit.html")]
struct RevisitPage {
    nav: crate::ui::Nav,
    flash: String,
    q: String,
    rows: Vec<RevisitRow>,
    count: usize,
}

async fn revisit_page(
    State(st): State<AppState>,
    Query(f): Query<RevisitQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "revisit").await else {
        return Redirect::to("/login").into_response();
    };
    let q = f.q.unwrap_or_default().trim().to_string();
    let toks = crate::table::tokens(&q);
    let mut rows: Vec<RevisitRow> = crate::tagsearch::revisit(&st.pool, nav.id, 2000)
        .await
        .into_iter()
        .filter(|r| {
            toks.is_empty()
                || crate::table::matches_typo(
                    &format!("{} {}", r.title, r.artists).to_lowercase(),
                    &toks,
                )
        })
        .take(500)
        .map(|r| RevisitRow {
            id: r.track_id,
            title: r.title,
            artists_html: String::new(),
            last_tagged: r.last_tagged.get(..10).unwrap_or("—").to_string(),
            missed: r.missed,
        })
        .collect();
    let revisit_ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let mut revisit_html = crate::artists::artists_html_for_tracks(&st.pool, &revisit_ids).await;
    for r in rows.iter_mut() {
        r.artists_html = revisit_html.remove(&r.id).unwrap_or_default();
    }
    render(&RevisitPage {
        nav,
        flash: String::new(),
        q,
        count: rows.len(),
        rows,
    })
}

// ── tag detail ───────────────────────────────────────────────────────────────

struct TagTrack {
    id: i64,
    title: String,
    artists: String,
    artists_html: String,
}

/// A source playlist feeding a tag, plus its keep-on-remove policy.
struct TagSource {
    id: i64,
    name: String,
    owner: String,
    keep: bool,
}

/// One row of the "top artists" insight table (pre-formatted percentages).
struct TopArtist {
    name: String,
    count: i64,
    /// Share of the tag: tagged tracks of this artist / all tracks with the tag.
    tag_pct: String,
    /// Share of the artist: tagged tracks of this artist / all tracks by them.
    artist_pct: String,
}

/// One row of the tag co-occurrence table (pre-formatted metric + links).
struct CooccurRow {
    name: String,
    type_label: String,
    same_type: bool,
    lift: String,
    both: i64,
    support: i64,
    tag_href: String,
    overlap_href: String,
    digging_href: String,
}

#[derive(Template)]
#[template(path = "tag.html")]
struct TagDetailPage {
    nav: crate::ui::Nav,
    flash: String,
    id: i64,
    name: String,
    owner: String,
    is_owner: bool,
    /// Human label of the tag's type (empty when untyped).
    type_label: String,
    type_icon: String,
    type_options: Vec<TypeOption>,
    parents: Vec<ParentRef>,
    children: Vec<ParentRef>,
    /// True when the tag has the `GENRE` type.
    genre_mode: bool,
    source_count: i64,
    /// Source playlists feeding this tag, with their keep-on-remove policy.
    sources: Vec<TagSource>,
    /// Bi-way sync: mirror tagged tracks onto the source playlists.
    sync: bool,
    q: String,
    tracks: Vec<TagTrack>,
    s_title: crate::table::SortHead,
    s_artists: crate::table::SortHead,
    top_artists: Vec<TopArtist>,
    cooccur: Vec<CooccurRow>,
    music_api: bool,
    ready: usize,
    total: usize,
}

async fn tag_detail_page(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Query(msg): Query<AdminMsg>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tags").await else {
        return Redirect::to("/login").into_response();
    };
    let Some(d) = crate::tags::tag_detail(&st.pool, id).await else {
        return not_found("Tag nicht gefunden.");
    };
    let parsed = crate::tagtypes::TagType::parse(&d.type_name);
    let type_label = parsed.map(|t| t.label().to_string()).unwrap_or_default();
    let type_icon = parsed.map(|t| t.icon().to_string()).unwrap_or_default();
    let type_options: Vec<TypeOption> = crate::tagtypes::options()
        .into_iter()
        .map(|(value, label, icon)| TypeOption {
            selected: d.type_name.eq_ignore_ascii_case(value),
            value,
            label,
            icon,
        })
        .collect();
    let q = msg.q.clone().unwrap_or_default().trim().to_string();
    let allowed: &[(&str, &str)] = &[("title", "title"), ("artists", "artists")];
    let (sort, dir) = crate::table::resolve_sort(
        msg.sort.as_deref(),
        msg.dir.as_deref(),
        allowed,
        "artists",
        "asc",
    );
    let toks = crate::table::tokens(&q);
    let mut tracks: Vec<TagTrack> = d
        .tracks
        .into_iter()
        .map(|(id, title, artists)| TagTrack {
            id,
            title,
            artists,
            artists_html: String::new(),
        })
        .filter(|t| {
            crate::table::matches_typo(&format!("{} {}", t.title, t.artists).to_lowercase(), &toks)
        })
        .collect();
    {
        let desc = dir == "desc";
        tracks.sort_by(|a, b| {
            let o = if sort == "title" {
                a.title.to_lowercase().cmp(&b.title.to_lowercase())
            } else {
                a.artists
                    .to_lowercase()
                    .cmp(&b.artists.to_lowercase())
                    .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            };
            if desc { o.reverse() } else { o }
        });
    }
    let tag_track_ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    let mut tag_track_html =
        crate::artists::artists_html_for_tracks(&st.pool, &tag_track_ids).await;
    for t in tracks.iter_mut() {
        t.artists_html = tag_track_html.remove(&t.id).unwrap_or_default();
    }
    let is_owner = d.owner.eq_ignore_ascii_case(&nav.slug);
    let isrcs: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT t.isrc FROM hub_track_resolved_tags rt
           JOIN hub_tracks t ON t.id = rt.track_id
          WHERE rt.tag_id = ?1 AND t.isrc IS NOT NULL AND t.isrc <> ''",
    )
    .bind(id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();
    let total = isrcs.len();
    let (ready, _) = crate::music_api::cached_counts(&st.pool, &isrcs).await;
    let parents: Vec<ParentRef> = crate::tags::tag_parents_of(&st.pool, id)
        .await
        .into_iter()
        .map(|(id, name)| ParentRef { id, name })
        .collect();
    let children: Vec<ParentRef> = crate::tags::tag_children_of(&st.pool, id)
        .await
        .into_iter()
        .map(|(id, name)| ParentRef { id, name })
        .collect();
    let top_artists: Vec<TopArtist> = crate::tags::tag_top_artists(&st.pool, id)
        .await
        .into_iter()
        .map(|(artist, tagged, with_tag, by_artist)| {
            let tag_pct = if with_tag > 0 {
                tagged as f64 * 100.0 / with_tag as f64
            } else {
                0.0
            };
            let artist_pct = if by_artist > 0 {
                tagged as f64 * 100.0 / by_artist as f64
            } else {
                0.0
            };
            TopArtist {
                name: artist,
                count: tagged,
                tag_pct: format!("{tag_pct:.1}%"),
                artist_pct: format!("{artist_pct:.1}%"),
            }
        })
        .collect();
    let cooccur: Vec<CooccurRow> = crate::tags::tag_cooccurrence(&st.pool, id, 5)
        .await
        .into_iter()
        .take(20)
        .map(|c| CooccurRow {
            tag_href: format!("/tag/{}", c.tag_id),
            overlap_href: format!("/overlap?tag={}", urlencoding::encode(&c.name)),
            digging_href: format!("/digging?seed={}", c.sample_track_id),
            name: c.name,
            type_label: c.type_label,
            same_type: c.same_type,
            lift: format!("{:.2}", c.lift),
            both: c.both,
            support: c.support,
        })
        .collect();
    let sources: Vec<TagSource> = sqlx::query_as::<_, (i64, String, String, i64)>(
        "SELECT ts.playlist_id, COALESCE(hp.name,''), COALESCE(u.slug,''),
                COALESCE(ts.keep_on_remove, 0)
           FROM hub_tag_sources ts
           JOIN hub_playlists hp ON hp.id = ts.playlist_id
           JOIN hub_users u ON u.id = hp.user_id
          WHERE ts.tag_id = ?1 ORDER BY hp.name",
    )
    .bind(id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, name, owner, keep)| TagSource {
        id,
        name,
        owner,
        keep: keep != 0,
    })
    .collect();
    let sync: bool = sqlx::query_scalar::<_, i64>(
        "SELECT COALESCE(sync_playlist,0) FROM hub_tags WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten()
    .unwrap_or(0)
        != 0;
    let genre_mode = d.type_name == "GENRE";
    render(&TagDetailPage {
        nav,
        flash: msg.msg.unwrap_or_default(),
        id: d.id,
        name: d.name,
        owner: d.owner,
        is_owner,
        type_label,
        type_icon,
        type_options,
        parents,
        children,
        genre_mode,
        source_count: d.source_count,
        sources,
        sync,
        q,
        tracks,
        s_title: crate::table::sort_head(
            raw.as_deref().unwrap_or(""),
            "title",
            "Titel",
            &sort,
            &dir,
        ),
        s_artists: crate::table::sort_head(
            raw.as_deref().unwrap_or(""),
            "artists",
            "Artists",
            &sort,
            &dir,
        ),
        top_artists,
        cooccur,
        music_api: st.cfg.music_api_token.is_some(),
        ready,
        total,
    })
}

#[derive(Deserialize)]
struct UpdateNameForm {
    name: String,
    #[serde(default)]
    icon: Option<String>,
}

fn flash_redirect(to: &str, msg: String) -> Response {
    let sep = if to.contains('?') { '&' } else { '?' };
    Redirect::to(&format!("{to}{sep}msg={}", urlencoding::encode(&msg))).into_response()
}

async fn tag_rename(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<UpdateNameForm>,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let msg = match crate::tags::rename_tag(&st.pool, uid, id, &f.name).await {
        Ok(()) => "Tag umbenannt".to_string(),
        Err(e) => format!("Fehler: {e}"),
    };
    flash_redirect(&format!("/tag/{id}"), msg)
}

#[derive(Deserialize)]
struct TagTypeForm {
    #[serde(default, rename = "type")]
    tag_type: Option<String>,
}

/// Owner-only: set (or clear) the tag's type.
async fn tag_set_type(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<TagTypeForm>,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let ty = f
        .tag_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .and_then(crate::tagtypes::TagType::parse);
    let msg = match crate::tagtypes::set(&st.pool, uid, id, ty).await {
        Ok(()) => "Typ gespeichert".to_string(),
        Err(e) => format!("Fehler: {e}"),
    };
    flash_redirect(&format!("/tag/{id}"), msg)
}

#[derive(Deserialize)]
struct TagSyncForm {
    on: i64,
}

#[derive(Deserialize)]
struct TagSourceKeepForm {
    playlist_id: i64,
    keep: i64,
}

/// Owner-only: toggle bi-way sync for the tag.
async fn tag_set_sync(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<TagSyncForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tags").await else {
        return Redirect::to("/login").into_response();
    };
    if tag_owner_slug(&st, id).await.as_deref() != Some(&nav.slug) {
        return flash_redirect(
            &format!("/tag/{id}"),
            "Nur der Besitzer kann das ändern".into(),
        );
    }
    let msg = match crate::tags::set_tag_sync(&st.pool, nav.id, id, f.on != 0).await {
        Ok(()) => "Sync aktualisiert".to_string(),
        Err(e) => format!("Fehler: {e}"),
    };
    flash_redirect(&format!("/tag/{id}"), msg)
}

/// Owner-only: set the keep-on-remove (archive) policy of a source playlist.
async fn tag_source_keep(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<TagSourceKeepForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tags").await else {
        return Redirect::to("/login").into_response();
    };
    if tag_owner_slug(&st, id).await.as_deref() != Some(&nav.slug) {
        return flash_redirect(
            &format!("/tag/{id}"),
            "Nur der Besitzer kann das ändern".into(),
        );
    }
    let msg = match crate::tags::set_source_keep_on_remove(
        &st.pool,
        nav.id,
        id,
        f.playlist_id,
        f.keep != 0,
    )
    .await
    {
        Ok(()) => "Archiv-Policy aktualisiert".to_string(),
        Err(e) => format!("Fehler: {e}"),
    };
    flash_redirect(&format!("/tag/{id}"), msg)
}

#[derive(Deserialize)]
struct TagParentForm {
    parent: String,
}

#[derive(Deserialize)]
struct TagParentIdForm {
    parent_id: i64,
}

/// Owner-only: tag name -> slug for the tag with `id`.
async fn tag_owner_slug(st: &AppState, id: i64) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT u.slug FROM hub_tags t JOIN hub_users u ON u.id = t.owner_user_id WHERE t.id = ?1",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten()
}

/// Issue #207: set the "Hauptrichtung" (parent) of a genre variation tag.
async fn tag_parent_add(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<TagParentForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tags").await else {
        return Redirect::to("/login").into_response();
    };
    if tag_owner_slug(&st, id).await.as_deref() != Some(&nav.slug) {
        return flash_redirect(
            &format!("/tag/{id}"),
            "Nur der Besitzer kann das ändern".into(),
        );
    }
    if f.parent.trim().is_empty() {
        return flash_redirect(&format!("/tag/{id}"), "Name fehlt".into());
    }
    let parent_id = match crate::tags::find_tag_id_by_name(&st.pool, &f.parent).await {
        Some(pid) => pid,
        None => match crate::tags::ensure_tag(&st.pool, nav.id, &f.parent).await {
            Ok(pid) => pid,
            Err(_) => {
                return flash_redirect(&format!("/tag/{id}"), "Ungültiger Name".into());
            }
        },
    };
    let msg = match crate::tags::add_tag_parent(&st.pool, id, parent_id).await {
        Ok(()) => "Hauptrichtung gesetzt".to_string(),
        Err(e) => format!("Fehler: {e}"),
    };
    flash_redirect(&format!("/tag/{id}"), msg)
}

async fn tag_parent_remove(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<TagParentIdForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tags").await else {
        return Redirect::to("/login").into_response();
    };
    if tag_owner_slug(&st, id).await.as_deref() != Some(&nav.slug) {
        return flash_redirect(
            &format!("/tag/{id}"),
            "Nur der Besitzer kann das ändern".into(),
        );
    }
    let msg = match crate::tags::remove_tag_parent(&st.pool, id, f.parent_id).await {
        Ok(()) => "Hauptrichtung entfernt".to_string(),
        Err(e) => format!("Fehler: {e}"),
    };
    flash_redirect(&format!("/tag/{id}"), msg)
}

struct ParentOption {
    id: i64,
    name: String,
    icon: String,
    selected: bool,
}

struct OwnerOption {
    slug: String,
    selected: bool,
}

#[derive(Deserialize)]
struct RoleForm {
    slug: String,
    role: String,
}

#[derive(Deserialize)]
struct SlugForm {
    slug: String,
}

// ── collectives ──────────────────────────────────────────────────────────────

struct CollRow {
    id: i64,
    name: String,
    icon: String,
    owner: String,
    role: String,
    members: i64,
}

fn coll_row(c: crate::tags::Collective) -> CollRow {
    CollRow {
        id: c.id,
        name: c.name,
        icon: c.icon,
        owner: c.owner,
        role: c.role,
        members: c.members,
    }
}

#[derive(Template)]
#[template(path = "collectives.html")]
struct CollectivesPage {
    nav: crate::ui::Nav,
    flash: String,
    q: String,
    collectives: Vec<CollRow>,
    discover: Vec<CollRow>,
    icons: &'static [&'static str],
    s_name: crate::table::SortHead,
    s_owner: crate::table::SortHead,
    s_members: crate::table::SortHead,
}

#[derive(Deserialize, Default)]
struct CollectiveFilter {
    q: Option<String>,
    sort: Option<String>,
    dir: Option<String>,
}

async fn collectives_page(
    State(st): State<AppState>,
    Query(f): Query<CollectiveFilter>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "collectives").await else {
        return Redirect::to("/login").into_response();
    };
    let q = f.q.unwrap_or_default().trim().to_string();
    let allowed: &[(&str, &str)] = &[("name", "name"), ("owner", "owner"), ("members", "members")];
    let (sort, dir) =
        crate::table::resolve_sort(f.sort.as_deref(), f.dir.as_deref(), allowed, "name", "asc");
    let toks = crate::table::tokens(&q);
    let mut collectives: Vec<CollRow> = crate::tags::list_collectives_for(&st.pool, nav.id)
        .await
        .into_iter()
        .map(coll_row)
        .filter(|c| {
            crate::table::matches_typo(
                &format!("{} {} {}", c.name, c.owner, c.role).to_lowercase(),
                &toks,
            )
        })
        .collect();
    let desc = dir == "desc";
    collectives.sort_by(|a, b| {
        let o = match sort.as_str() {
            "owner" => a.owner.to_lowercase().cmp(&b.owner.to_lowercase()),
            "members" => a.members.cmp(&b.members),
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        };
        if desc { o.reverse() } else { o }
    });
    let discover = crate::tags::list_discover_collectives(&st.pool, nav.id)
        .await
        .into_iter()
        .map(coll_row)
        .collect();
    let rawq = raw.as_deref().unwrap_or("");
    let s_name = crate::table::sort_head(rawq, "name", "Collective", &sort, &dir);
    let s_owner = crate::table::sort_head(rawq, "owner", "Owner", &sort, &dir);
    let s_members = crate::table::sort_head(rawq, "members", "Mitglieder", &sort, &dir);
    render(&CollectivesPage {
        nav,
        flash: String::new(),
        q,
        collectives,
        discover,
        icons: crate::tags::ICONS,
        s_name,
        s_owner,
        s_members,
    })
}

async fn collective_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<UpdateNameForm>,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tags::create_collective(&st.pool, uid, &f.name, f.icon.as_deref().unwrap_or(""))
        .await;
    Redirect::to("/collectives").into_response()
}

async fn collective_join(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tags::join_collective(&st.pool, uid, id).await;
    Redirect::to("/collectives").into_response()
}

async fn collective_leave(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tags::leave_collective(&st.pool, uid, id).await;
    Redirect::to("/collectives").into_response()
}

async fn collective_set_member(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<RoleForm>,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tags::set_collective_member(&st.pool, uid, id, &f.slug, &f.role).await;
    Redirect::to(&format!("/collectives/{id}")).into_response()
}

async fn collective_remove_member(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<SlugForm>,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tags::remove_collective_member(&st.pool, uid, id, &f.slug).await;
    Redirect::to(&format!("/collectives/{id}")).into_response()
}

async fn collective_delete(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tags::delete_collective(&st.pool, uid, id).await;
    Redirect::to("/collectives").into_response()
}

async fn collective_update(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<UpdateNameForm>,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let icon = f.icon.as_deref().unwrap_or("").trim().to_string();
    let msg = match crate::tags::update_collective(&st.pool, uid, id, &f.name, &icon).await {
        Ok(()) => "Collective gespeichert".to_string(),
        Err(e) => format!("Fehler: {e}"),
    };
    flash_redirect(&format!("/collectives/{id}"), msg)
}

struct CollMember {
    slug: String,
    role: String,
}

#[derive(Template)]
#[template(path = "collective.html")]
struct CollectivePage {
    nav: crate::ui::Nav,
    flash: String,
    id: i64,
    name: String,
    icon: String,
    owner: String,
    is_owner: bool,
    member: bool,
    members: Vec<CollMember>,
    users: Vec<String>,
    icons: Vec<IconOption>,
    s_muser: crate::table::SortHead,
}

async fn collective_page(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Query(msg): Query<AdminMsg>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "collectives").await else {
        return Redirect::to("/login").into_response();
    };
    let Some(d) = crate::tags::collective_detail(&st.pool, nav.id, id).await else {
        return not_found("Collective nicht gefunden.");
    };
    let member = !d.role.is_empty();
    // Members table: sortable by user slug.
    let (msort, mdir) = crate::table::resolve_sort(
        msg.msort.as_deref(),
        msg.mdir.as_deref(),
        &[("user", "slug")],
        "user",
        "asc",
    );
    let mut members: Vec<CollMember> = d
        .members
        .into_iter()
        .map(|(slug, role)| CollMember { slug, role })
        .collect();
    {
        let desc = mdir == "desc";
        members.sort_by(|a, b| {
            let o = a.slug.to_lowercase().cmp(&b.slug.to_lowercase());
            if desc { o.reverse() } else { o }
        });
        let _ = msort;
    }
    let users = sqlx::query_scalar::<_, String>("SELECT slug FROM hub_users ORDER BY slug")
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    let icons = icon_options(&d.icon);
    let rawq = raw.as_deref().unwrap_or("");
    render(&CollectivePage {
        nav,
        flash: msg.msg.unwrap_or_default(),
        id: d.id,
        name: d.name,
        icon: d.icon,
        owner: d.owner,
        is_owner: d.is_owner,
        member,
        members,
        users,
        icons,
        s_muser: crate::table::sort_head(rawq, "user", "User", &msort, &mdir),
    })
}

// ── settings ────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsPage {
    nav: crate::ui::Nav,
    flash: String,
}

async fn settings_page(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "settings").await else {
        return Redirect::to("/login").into_response();
    };

    render(&SettingsPage {
        nav,
        flash: String::new(),
    })
}

// ── manual import (YouTube / SoundCloud) ─────────────────────────────────────

#[derive(Template)]
#[template(path = "import.html")]
struct ImportPage {
    nav: crate::ui::Nav,
    flash: String,
    services: Vec<ServiceOption>,
    url: String,
    playlists: Vec<ImportPlaylistRow>,
}

/// One selectable service with a precomputed `selected` flag — keeps the
/// template free of `==` comparisons inside HTML tags.
struct ServiceOption {
    value: &'static str,
    label: &'static str,
    selected: bool,
}

fn service_options(current: &str) -> Vec<ServiceOption> {
    [("youtube", "YouTube"), ("soundcloud", "SoundCloud")]
        .into_iter()
        .map(|(value, label)| ServiceOption {
            value,
            label,
            selected: value == current,
        })
        .collect()
}

struct ImportPlaylistRow {
    id: i64,
    name: String,
    service: String,
    track_count: String,
    fetched_at: String,
}

#[derive(Deserialize, Default)]
struct ImportQuery {
    msg: Option<String>,
    service: Option<String>,
}

#[derive(Deserialize)]
struct ImportForm {
    service: String,
    url: String,
}

/// The user's already-imported YouTube/SoundCloud playlists.
async fn import_playlists(st: &AppState, user_id: i64) -> Vec<ImportPlaylistRow> {
    let rows = sqlx::query(
        "SELECT id, name, service, track_count, fetched_at
           FROM hub_playlists
          WHERE user_id = ?1 AND service IN ('youtube', 'soundcloud')
          ORDER BY service, name",
    )
    .bind(user_id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    rows.iter()
        .map(|r| ImportPlaylistRow {
            id: r.get("id"),
            name: r.get::<Option<String>, _>("name").unwrap_or_default(),
            service: r.get::<Option<String>, _>("service").unwrap_or_default(),
            track_count: r
                .get::<Option<i64>, _>("track_count")
                .map(|c| c.to_string())
                .unwrap_or_else(|| "—".to_string()),
            fetched_at: r
                .get::<Option<String>, _>("fetched_at")
                .unwrap_or_else(|| "—".to_string()),
        })
        .collect()
}

async fn import_page(
    State(st): State<AppState>,
    Query(q): Query<ImportQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "import").await else {
        return Redirect::to("/login").into_response();
    };
    let service = q.service.unwrap_or_else(|| "youtube".to_string());
    let playlists = import_playlists(&st, nav.id).await;
    render(&ImportPage {
        nav,
        flash: q.msg.unwrap_or_default(),
        services: service_options(&service),
        url: String::new(),
        playlists,
    })
}

async fn import_submit(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<ImportForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "import").await else {
        return Redirect::to("/login").into_response();
    };
    let url = f.url.trim().to_string();
    if url.is_empty() {
        return flash_redirect("/import", "Bitte eine URL angeben.".to_string());
    }

    let result = match f.service.trim().to_lowercase().as_str() {
        "youtube" => crate::ingest::ingest_youtube_public(&st.pool, &nav.slug, &url).await,
        "soundcloud" => crate::ingest::ingest_soundcloud_public(&st.pool, &nav.slug, &url).await,
        other => {
            return flash_redirect("/import", format!("Unbekannter Dienst: {other}"));
        }
    };

    let msg = match result {
        Ok(s) => format!(
            "Import fertig: {} Playlist(s) gefunden, {} mit Tracks, {} Track-Verknüpfungen.",
            s.playlists, s.owned_with_items, s.memberships
        ),
        Err(e) => format!("Import fehlgeschlagen: {e:#}"),
    };
    flash_redirect("/import", msg)
}

/// Upload a Traktor `collection.nml` (multipart) for the current user.
async fn import_traktor_submit(
    State(st): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "import").await else {
        return Redirect::to("/login").into_response();
    };
    let mut xml: Option<String> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        if field.name() == Some("file") {
            xml = field.text().await.ok();
        }
    }
    let Some(xml) = xml.filter(|x| !x.trim().is_empty()) else {
        return flash_redirect("/import", "Keine NML-Datei empfangen.".to_string());
    };
    match crate::traktor::import_str(&st.pool, nav.id, &xml).await {
        Ok(s) => flash_redirect(
            "/import",
            format!(
                "Traktor-Import: {} Einträge, {} gematcht, {} Playlists, {} mit BPM, {} mit Key.",
                s.entries, s.matched, s.playlists, s.with_bpm, s.with_key
            ),
        ),
        Err(e) => flash_redirect("/import", format!("Traktor-Import fehlgeschlagen: {e:#}")),
    }
}

#[cfg(test)]
mod scope_sql_tests {
    use super::*;
    use sqlx::Execute;

    #[test]
    fn empty_scope_predicate_never_emits_in_empty() {
        let mut qb: QueryBuilder<'_, sqlx::Sqlite> =
            QueryBuilder::new("SELECT 1 FROM hub_tracks WHERE ");
        push_scope_ids(&mut qb, "id", &HashSet::new());
        let sql = qb.build().sql().to_string();
        assert!(
            !sql.contains("IN ()"),
            "empty scope must not emit invalid `IN ()`: {sql}"
        );
        assert!(
            sql.contains("1 = 0"),
            "empty scope must become the always-false predicate: {sql}"
        );
    }

    #[test]
    fn nonempty_scope_predicate_binds_a_single_json_array() {
        let ids: HashSet<i64> = (1_i64..=1500).collect();
        let mut qb: QueryBuilder<'_, sqlx::Sqlite> =
            QueryBuilder::new("SELECT 1 FROM hub_tracks WHERE ");
        push_scope_ids(&mut qb, "id", &ids);
        let sql = qb.build().sql().to_string();
        assert!(
            sql.contains("id IN (SELECT value FROM json_each(?))"),
            "expects the json_each predicate: {sql}"
        );
        assert!(
            !sql.contains("1 = 0"),
            "non-empty scope keeps the predicate: {sql}"
        );
        assert_eq!(
            sql.matches('?').count(),
            1,
            "scope must bind exactly one variable regardless of size: {sql}"
        );
    }
}
