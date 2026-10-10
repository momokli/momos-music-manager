//! Albums — a first-class view over `hub_tracks.album`, linking
//! Artist → Album → Track. Albums are derived (grouped by album name).

use askama::Template;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use serde::Deserialize;
use sqlx::Row;

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
struct AlbumFilter {
    q: Option<String>,
    /// Exact artist name filter.
    artist: Option<String>,
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
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "albums").await else {
        return Redirect::to("/login").into_response();
    };
    let q = f.q.unwrap_or_default().trim().to_string();
    let artist = f.artist.unwrap_or_default().trim().to_string();
    let needle = q.to_lowercase();
    let a_needle = artist.to_lowercase();

    let raw = sqlx::query(
        "SELECT album,
                COUNT(*) AS tracks,
                MAX(artists) AS artists,
                MAX(image_url) AS cover
           FROM hub_tracks
          WHERE album IS NOT NULL AND TRIM(album) <> ''
          GROUP BY album
          ORDER BY album COLLATE NOCASE",
    )
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    let rows: Vec<AlbumRow> = raw
        .into_iter()
        .filter_map(|r| {
            let name: String = r.get::<Option<String>, _>("album").unwrap_or_default();
            let artists: String = r.get::<Option<String>, _>("artists").unwrap_or_default();
            if !needle.is_empty()
                && !name.to_lowercase().contains(&needle)
                && !artists.to_lowercase().contains(&needle)
            {
                return None;
            }
            if !a_needle.is_empty() && !artists.to_lowercase().contains(&a_needle) {
                return None;
            }
            Some(AlbumRow {
                href: album_href(&name),
                name,
                artists,
                tracks: r.get::<i64, _>("tracks"),
                cover: r.get::<Option<String>, _>("cover").unwrap_or_default(),
            })
        })
        .collect();
    let total = rows.len();
    let mut rows = rows;
    rows.truncate(600);
    render(&AlbumsPage {
        nav,
        flash: String::new(),
        q,
        artist,
        total,
        rows,
    })
}

struct AlbumTrack {
    id: i64,
    title: String,
    artists: String,
    duration: String,
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
}

async fn album_page(
    State(st): State<AppState>,
    Path(name): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "albums").await else {
        return Redirect::to("/login").into_response();
    };
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
        });
    }
    let track_count = tracks.len();
    render(&AlbumPage {
        nav,
        flash: String::new(),
        name,
        cover,
        artists,
        track_count,
        tracks,
    })
}
