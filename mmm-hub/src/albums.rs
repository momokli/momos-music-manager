//! Albums — a first-class view over `hub_tracks.album`, linking
//! Artist → Album → Track. Albums are derived (grouped by album name).

use askama::Template;
use axum::Router;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use serde::Deserialize;
use sqlx::{QueryBuilder, Row, Sqlite};

use crate::api::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/albums", get(albums_page))
        .route("/album/{*name}", get(album_page))
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

fn album_href(name: &str) -> String {
    format!("/album/{}", urlencoding::encode(name))
}

fn artist_href(name: &str) -> String {
    format!("/artist/{}", urlencoding::encode(name))
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AlbumFilter {
    q: Option<String>,
    /// Exact artist name filter.
    artist: Option<String>,
    /// Whitelisted sort field: `album` (default) | `artists` | `tracks`.
    sort: Option<String>,
    /// `asc` | `desc`.
    dir: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AlbumTrackFilter {
    /// Whitelisted sort field: `title` (default) | `artists` | `duration`.
    sort: Option<String>,
    /// `asc` | `desc`.
    dir: Option<String>,
}

#[derive(Template)]
#[template(path = "albums.html")]
struct AlbumsPage {
    nav: crate::ui::Nav,
    flash: String,
    q: String,
    artist: String,
    total: usize,
    rows: Vec<AlbumRow>,
    s_album: crate::table::SortHead,
    s_artists: crate::table::SortHead,
    s_tracks: crate::table::SortHead,
}

struct AlbumRow {
    name: String,
    href: String,
    artists: String,
    tracks: i64,
    cover: String,
}

async fn albums_page(
    State(st): State<AppState>,
    Query(f): Query<AlbumFilter>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "albums").await else {
        return Redirect::to("/login").into_response();
    };
    let q = f.q.unwrap_or_default().trim().to_string();
    let artist = f.artist.unwrap_or_default().trim().to_string();
    let a_needle = artist.to_lowercase();

    let allowed: &[(&str, &str)] = &[
        ("album", "album"),
        ("artists", "artists"),
        ("tracks", "tracks"),
    ];
    let (sort, dir) =
        crate::table::resolve_sort(f.sort.as_deref(), f.dir.as_deref(), allowed, "album", "asc");

    // Wrapped so the derived-table aliases can be fuzzy-searched and sorted.
    let mut qb: QueryBuilder<Sqlite> = QueryBuilder::new(
        "SELECT * FROM (SELECT album AS album,
                COUNT(*) AS tracks,
                MAX(artists) AS artists,
                MAX(image_url) AS cover
           FROM hub_tracks
          WHERE album IS NOT NULL AND TRIM(album) <> ''
          GROUP BY album",
    );
    if !a_needle.is_empty() {
        qb.push(" HAVING instr(lower(COALESCE(MAX(artists), '')), ")
            .push_bind(a_needle.clone())
            .push(") > 0");
    }
    qb.push(") WHERE 1 = 1");
    crate::table::push_fuzzy(&mut qb, &q, &["album", "artists"]);
    crate::table::order_by(&mut qb, &sort, &dir, allowed, "album");

    let raw_rows = qb.build().fetch_all(&st.pool).await.unwrap_or_default();

    let rows: Vec<AlbumRow> = raw_rows
        .into_iter()
        .map(|r| {
            let name: String = r.get::<Option<String>, _>("album").unwrap_or_default();
            AlbumRow {
                href: album_href(&name),
                name,
                artists: r.get::<Option<String>, _>("artists").unwrap_or_default(),
                tracks: r.get::<Option<i64>, _>("tracks").unwrap_or(0),
                cover: r.get::<Option<String>, _>("cover").unwrap_or_default(),
            }
        })
        .collect();
    let total = rows.len();
    let mut rows = rows;
    rows.truncate(600);

    let rawq = raw.as_deref().unwrap_or("");
    let s_album = crate::table::sort_head(rawq, "album", "Album", &sort, &dir);
    let s_artists = crate::table::sort_head(rawq, "artists", "Künstler", &sort, &dir);
    let s_tracks = crate::table::sort_head(rawq, "tracks", "Tracks", &sort, &dir);

    render(&AlbumsPage {
        nav,
        flash: String::new(),
        q,
        artist,
        total,
        rows,
        s_album,
        s_artists,
        s_tracks,
    })
}

struct AlbumTrack {
    id: i64,
    title: String,
    artists: String,
    duration: String,
    duration_ms: i64,
}

struct ArtistLink {
    name: String,
    href: String,
}

#[derive(Template)]
#[template(path = "album.html")]
struct AlbumPage {
    nav: crate::ui::Nav,
    flash: String,
    name: String,
    cover: String,
    artists: Vec<ArtistLink>,
    track_count: usize,
    tracks: Vec<AlbumTrack>,
    s_title: crate::table::SortHead,
    s_artists: crate::table::SortHead,
    s_duration: crate::table::SortHead,
}

async fn album_page(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Query(f): Query<AlbumTrackFilter>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "albums").await else {
        return Redirect::to("/login").into_response();
    };

    let allowed: &[(&str, &str)] = &[
        ("title", "title"),
        ("artists", "artists"),
        ("duration", "duration_ms"),
    ];
    let (sort, dir) =
        crate::table::resolve_sort(f.sort.as_deref(), f.dir.as_deref(), allowed, "title", "asc");

    let rows = sqlx::query(
        "SELECT id, COALESCE(title,'') AS title, COALESCE(artists,'') AS artists,
                duration_ms, image_url
           FROM hub_tracks
          WHERE album = ?1 COLLATE NOCASE
          ORDER BY artists, title",
    )
    .bind(&name)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();
    if rows.is_empty() {
        return (StatusCode::NOT_FOUND, "Album nicht gefunden.").into_response();
    }

    let mut cover = String::new();
    let mut artists: Vec<ArtistLink> = Vec::new();
    let mut tracks: Vec<AlbumTrack> = Vec::new();
    for r in &rows {
        if cover.is_empty() {
            cover = r.get::<Option<String>, _>("image_url").unwrap_or_default();
        }
        let a = r.get::<String, _>("artists");
        if !a.trim().is_empty() && !artists.iter().any(|x| x.name == a) {
            artists.push(ArtistLink {
                href: artist_href(&a),
                name: a.clone(),
            });
        }
        let ms = r.get::<Option<i64>, _>("duration_ms").unwrap_or(0);
        let duration = if ms > 0 {
            format!("{}:{:02}", ms / 60000, (ms % 60000) / 1000)
        } else {
            String::new()
        };
        tracks.push(AlbumTrack {
            id: r.get("id"),
            title: r.get("title"),
            artists: a,
            duration,
            duration_ms: ms,
        });
    }

    // Server-side sort of the (bounded) in-memory track list.
    let ci = |s: &str| s.to_lowercase();
    match sort.as_str() {
        "artists" => tracks.sort_by(|a, b| {
            ci(&a.artists)
                .cmp(&ci(&b.artists))
                .then_with(|| ci(&a.title).cmp(&ci(&b.title)))
        }),
        "duration" => tracks.sort_by(|a, b| {
            a.duration_ms
                .cmp(&b.duration_ms)
                .then_with(|| ci(&a.title).cmp(&ci(&b.title)))
        }),
        _ => tracks.sort_by(|a, b| {
            ci(&a.title)
                .cmp(&ci(&b.title))
                .then_with(|| ci(&a.artists).cmp(&ci(&b.artists)))
        }),
    }
    if dir == "desc" {
        tracks.reverse();
    }

    let track_count = tracks.len();
    let rawq = raw.as_deref().unwrap_or("");
    let s_title = crate::table::sort_head(rawq, "title", "Track", &sort, &dir);
    let s_artists = crate::table::sort_head(rawq, "artists", "Künstler", &sort, &dir);
    let s_duration = crate::table::sort_head(rawq, "duration", "Dauer", &sort, &dir);
    render(&AlbumPage {
        nav,
        flash: String::new(),
        name,
        cover,
        artists,
        track_count,
        tracks,
        s_title,
        s_artists,
        s_duration,
    })
}
