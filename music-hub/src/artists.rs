//! Artist explorer (plan AR1–AR6, issues #227–#232).
//!
//! Two session-gated pages:
//!
//! * `GET /artists`      — list of every artist with server-side filters
//!   (name search, "played by me", "tagged", "in playlists") and sorting.
//! * `GET /artist/{id}` — profile: most played tracks per user, tag
//!   distribution, playlist membership, collaborations / b2b / album
//!   co-artists and reference scoring. `/artist/{name}` redirects to the id.
//!
//! Artists are **first-class entities** (`hub_artists`, migration 047): tracks
//! link to them via `hub_track_artists` (M:N, ordered). The free-text
//! `hub_tracks.artists` column is kept as a display cache only. Play counts come
//! from the per-user Traktor ingest (`hub_traktor_tracks`); tags from the
//! materialized `hub_track_resolved_tags` joined to `hub_tags` (bucketed by type).
//!
//! Assumptions / limitations (documented in the PR):
//! * URLs are **id-based** (`/artist/{id}`); `/artist/{name}` redirects to the id.
//! * Splitting the historical free-text artist strings happens **once**, in
//!   [`backfill_artist_entities`] — nowhere else in application code.
//! * "b2b" is best-effort: Traktor playlists whose name contains `b2b`
//!   (case-insensitive) that contain at least one track by the artist.
//! * Reference coefficients are constants for now; a follow-up makes them
//!   settings (plan §1 I3 / G3).

use std::collections::{BTreeMap, HashMap, HashSet};

use anyhow::Result;
use askama::Template;
use axum::Router;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use serde::Deserialize;
use serde_json::Value;
use sqlx::{Executor, QueryBuilder, Row, Sqlite, SqliteConnection, SqlitePool};

use crate::api::AppState;

/// Max ids per `IN (…)` clause (SQLite bind-variable limit; see AGENT.md).
const CHUNK: usize = 400;
/// Cap for the artist list (server-side; keeps the page dense and bounded).
const LIST_LIMIT: usize = 2000;
/// Reference-scoring coefficients (AR6). Constants on purpose for now.
const REF_TAG_W: f64 = 3.0;
const REF_COLLAB_W: f64 = 2.0;
const REF_B2B_W: f64 = 1.0;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/artists", get(artists_page))
        // Canonical, id-based route. A name is resolved and redirected to the id
        // (see `artist_page`).
        .route("/artist/{key}", get(artist_page))
        .with_state(state)
}

// ── shared helpers ───────────────────────────────────────────────────────────

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
            "<!doctype html><meta charset=\"utf-8\"><p>{msg}</p>"
        )),
    )
        .into_response()
}

/// Canonical, id-based artist URL (collision-safe).
fn artist_href(id: i64) -> String {
    format!("/artist/{id}")
}

fn spotify_href(name: &str) -> String {
    format!(
        "https://open.spotify.com/search/{}",
        urlencoding::encode(name)
    )
}

/// Case-insensitive (ASCII) `String::replace` that never mis-slices non-ASCII.
fn replace_ci(hay: &str, needle: &str, with: &str) -> String {
    let nl: Vec<char> = needle.chars().collect();
    let hl: Vec<char> = hay.chars().collect();
    if nl.is_empty() || nl.len() > hl.len() {
        return hay.to_string();
    }
    let mut out = String::with_capacity(hay.len());
    let mut i = 0;
    while i < hl.len() {
        let hit = i + nl.len() <= hl.len()
            && hl[i..i + nl.len()]
                .iter()
                .zip(&nl)
                .all(|(a, b)| a.eq_ignore_ascii_case(b));
        if hit {
            out.push_str(with);
            i += nl.len();
        } else {
            out.push(hl[i]);
            i += 1;
        }
    }
    out
}

/// Split a raw `hub_tracks.artists` string into individual artist names.
///
/// Delimiters: `;`, `,`, `&` and the `feat.` / `ft.` / `featuring` markers
/// (case-insensitive). Results are trimmed, de-duplicated case-insensitively
/// within the track, and keep their first-seen spelling.
fn split_artists(raw: &str) -> Vec<String> {
    let mut s = raw.to_string();
    for pat in [" feat. ", " feat ", " ft. ", " ft ", " featuring "] {
        s = replace_ci(&s, pat, ";");
    }
    let mut out: Vec<String> = Vec::new();
    for part in s.split([';', ',', '&']) {
        let name = part.trim();
        if name.is_empty() {
            continue;
        }
        if out.iter().any(|e| e.eq_ignore_ascii_case(name)) {
            continue;
        }
        out.push(name.to_string());
    }
    out
}

fn bind_ids(qb: &mut QueryBuilder<'_, Sqlite>, ids: &[i64]) {
    qb.push("(");
    {
        let mut sep = qb.separated(", ");
        for id in ids {
            sep.push_bind(*id);
        }
    }
    qb.push(")");
}

// ── entity helpers (plan P0: artists/albums as first-class rows) ─────────────

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Minimal HTML escaping for text embedded in a template via `|safe`.
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Render `[(artist_id, name)]` as comma-separated links to `/artist/{id}`.
pub fn artist_links_html(artists: &[(i64, String)]) -> String {
    artists
        .iter()
        .map(|(id, name)| format!("<a href=\"/artist/{id}\">{}</a>", escape_html(name)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Resolve an artist id by (case-insensitive) name. `None` when unknown.
pub async fn artist_id_by_name(pool: &SqlitePool, name: &str) -> Option<i64> {
    sqlx::query_scalar(
        "SELECT id FROM hub_artists WHERE name = ?1 COLLATE NOCASE ORDER BY id LIMIT 1",
    )
    .bind(name.trim())
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

/// Find-or-create an artist by name (case-insensitive). Returns its id.
///
/// Generic over the executor so it runs on a pool **or** inside a transaction
/// (`&mut *tx`): the backfill batches all of its upserts in one transaction.
/// The single `INSERT … ON CONFLICT … RETURNING` keeps the first-seen spelling
/// (the no-op `DO UPDATE` only exists to make `RETURNING` emit the row).
pub async fn upsert_artist_by_name<'e, E>(ex: E, name: &str) -> Result<i64>
where
    E: Executor<'e, Database = Sqlite>,
{
    let name = name.trim();
    let now = now_iso();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_artists (name, first_seen_at, updated_at) VALUES (?1, ?2, ?2)
         ON CONFLICT(name COLLATE NOCASE) DO UPDATE SET name = hub_artists.name
         RETURNING id",
    )
    .bind(name)
    .bind(&now)
    .fetch_one(ex)
    .await?;
    Ok(id)
}

/// Upsert an artist keyed by its Spotify id (the real identity), refreshing the
/// display name and recording the external id. Falls back to name matching when
/// the Spotify id is new but the name already exists.
pub async fn upsert_artist_by_spotify(
    pool: &SqlitePool,
    spotify_id: &str,
    name: &str,
) -> Result<i64> {
    let name = name.trim();
    let now = now_iso();
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT artist_id FROM hub_artist_external_ids
          WHERE service = 'spotify' AND external_id = ?1 LIMIT 1",
    )
    .bind(spotify_id)
    .fetch_optional(pool)
    .await?;
    let id = match existing {
        Some(id) => {
            sqlx::query("UPDATE hub_artists SET name = ?2, updated_at = ?3 WHERE id = ?1")
                .bind(id)
                .bind(name)
                .bind(&now)
                .execute(pool)
                .await?;
            id
        }
        None => upsert_artist_by_name(pool, name).await?,
    };
    sqlx::query(
        "INSERT INTO hub_artist_external_ids (artist_id, service, external_id, url, source, fetched_at)
         VALUES (?1, 'spotify', ?2, ?3, 'ingest', ?4)
         ON CONFLICT(artist_id, service, external_id) DO UPDATE SET
             url = excluded.url, fetched_at = excluded.fetched_at",
    )
    .bind(id)
    .bind(spotify_id)
    .bind(format!("https://open.spotify.com/artist/{spotify_id}"))
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(id)
}

/// Find-or-create an album by title (case-insensitive). Used by the backfill.
///
/// Takes a connection rather than a pool because it issues two statements and
/// is always called on the backfill's transaction (`&mut *tx`).
pub async fn upsert_album_by_title(conn: &mut SqliteConnection, title: &str) -> Result<i64> {
    let title = title.trim();
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM hub_albums WHERE title = ?1 COLLATE NOCASE ORDER BY id LIMIT 1",
    )
    .bind(title)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(id) = existing {
        return Ok(id);
    }
    let now = now_iso();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_albums (title, first_seen_at, updated_at) VALUES (?1, ?2, ?2) RETURNING id",
    )
    .bind(title)
    .bind(&now)
    .fetch_one(&mut *conn)
    .await?;
    Ok(id)
}

/// Upsert an album from a Spotify album object (title/type/release/tracks/cover/
/// label) and record its external id. Returns the album id (or `None` when the
/// object carries no usable title).
pub async fn upsert_album(pool: &SqlitePool, album: &Value) -> Result<Option<i64>> {
    let title = album["name"].as_str().unwrap_or("").trim();
    if title.is_empty() {
        return Ok(None);
    }
    let spotify_id = album["id"].as_str().filter(|s| !s.is_empty());
    let album_type = album["album_type"].as_str();
    let release_date = album["release_date"].as_str();
    let total_tracks = album["total_tracks"].as_i64();
    let label = album["label"].as_str();
    let image_url = album["images"]
        .as_array()
        .and_then(|i| i.first())
        .and_then(|x| x["url"].as_str());
    let now = now_iso();

    let mut id: Option<i64> = None;
    if let Some(sid) = spotify_id {
        id = sqlx::query_scalar(
            "SELECT album_id FROM hub_album_external_ids
              WHERE service = 'spotify' AND external_id = ?1 LIMIT 1",
        )
        .bind(sid)
        .fetch_optional(pool)
        .await?;
    }
    if id.is_none() {
        id = sqlx::query_scalar(
            "SELECT id FROM hub_albums WHERE title = ?1 COLLATE NOCASE ORDER BY id LIMIT 1",
        )
        .bind(title)
        .fetch_optional(pool)
        .await?;
    }
    let id = match id {
        Some(id) => {
            sqlx::query(
                "UPDATE hub_albums SET title = ?2,
                     album_type   = COALESCE(?3, album_type),
                     release_date = COALESCE(?4, release_date),
                     total_tracks = COALESCE(?5, total_tracks),
                     label        = COALESCE(?6, label),
                     image_url    = COALESCE(?7, image_url),
                     updated_at   = ?8
                   WHERE id = ?1",
            )
            .bind(id)
            .bind(title)
            .bind(album_type)
            .bind(release_date)
            .bind(total_tracks)
            .bind(label)
            .bind(image_url)
            .bind(&now)
            .execute(pool)
            .await?;
            id
        }
        None => {
            sqlx::query_scalar(
                "INSERT INTO hub_albums
                     (title, album_type, release_date, total_tracks, label, image_url, first_seen_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7) RETURNING id",
            )
            .bind(title)
            .bind(album_type)
            .bind(release_date)
            .bind(total_tracks)
            .bind(label)
            .bind(image_url)
            .bind(&now)
            .fetch_one(pool)
            .await?
        }
    };
    if let Some(sid) = spotify_id {
        sqlx::query(
            "INSERT INTO hub_album_external_ids (album_id, service, external_id, url, source, fetched_at)
             VALUES (?1, 'spotify', ?2, ?3, 'ingest', ?4)
             ON CONFLICT(album_id, service, external_id) DO UPDATE SET
                 url = excluded.url, fetched_at = excluded.fetched_at",
        )
        .bind(id)
        .bind(sid)
        .bind(format!("https://open.spotify.com/album/{sid}"))
        .bind(&now)
        .execute(pool)
        .await?;
    }
    Ok(Some(id))
}

/// Atomically upsert a track's artists from a Spotify `artists[]` array and link
/// them (position = array index). No string splitting — each entry is atomic.
pub async fn upsert_track_artists(pool: &SqlitePool, track_id: i64, artists: &Value) -> Result<()> {
    let Some(arr) = artists.as_array() else {
        return Ok(());
    };
    for (pos, a) in arr.iter().enumerate() {
        let name = a["name"].as_str().unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        let sid = a["id"].as_str().filter(|s| !s.is_empty());
        let aid = match sid {
            Some(sid) => upsert_artist_by_spotify(pool, sid, name).await?,
            None => upsert_artist_by_name(pool, name).await?,
        };
        sqlx::query(
            "INSERT INTO hub_track_artists (track_id, artist_id, position) VALUES (?1, ?2, ?3)
             ON CONFLICT(track_id, artist_id) DO UPDATE SET position = excluded.position",
        )
        .bind(track_id)
        .bind(aid)
        .bind(pos as i64)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// Upsert a track's album from a Spotify album object and set `hub_tracks.album_id`.
pub async fn upsert_track_album(pool: &SqlitePool, track_id: i64, album: &Value) -> Result<()> {
    if album.is_null() {
        return Ok(());
    }
    if let Some(aid) = upsert_album(pool, album).await? {
        sqlx::query("UPDATE hub_tracks SET album_id = ?1 WHERE id = ?2")
            .bind(aid)
            .bind(track_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Link a track to a single name-keyed artist (no provider id). Used by
/// non-Spotify sources (yt-dlp / SoundCloud) which carry no artist ids.
pub async fn link_track_artist_name(pool: &SqlitePool, track_id: i64, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        return Ok(());
    }
    let aid = upsert_artist_by_name(pool, name).await?;
    sqlx::query(
        "INSERT INTO hub_track_artists (track_id, artist_id, position) VALUES (?1, ?2, 0)
         ON CONFLICT(track_id, artist_id) DO NOTHING",
    )
    .bind(track_id)
    .bind(aid)
    .execute(pool)
    .await?;
    Ok(())
}

/// Process up to `limit` tracks that still lack artist links, all inside one
/// transaction (`pool.begin()` … `tx.commit()`), and return how many tracks were
/// processed. Returns `0` when nothing is left to do.
///
/// Selection is `NOT EXISTS (hub_track_artists)`, so a track that already has
/// links is never touched again — the backfill is resumable. The extra guard
/// skips tracks that have nothing to backfill (and album-only tracks whose
/// `album_id` is already set) so the caller's `while > 0` loop always terminates.
pub async fn backfill_artist_entities_batch(pool: &SqlitePool, limit: i64) -> Result<usize> {
    let rows = sqlx::query(
        "SELECT t.id AS id, COALESCE(t.artists, '') AS artists, COALESCE(t.album, '') AS album
           FROM hub_tracks t
          WHERE NOT EXISTS (SELECT 1 FROM hub_track_artists ta WHERE ta.track_id = t.id)
            AND (TRIM(COALESCE(t.artists, '')) != ''
                 OR (TRIM(COALESCE(t.album, '')) != '' AND t.album_id IS NULL))
          ORDER BY t.id
          LIMIT ?1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }
    let mut tx = pool.begin().await?;
    let mut n = 0usize;
    for r in rows {
        let tid: i64 = r.get("id");
        let artists_raw: String = r.get("artists");
        let album: String = r.get("album");
        for (pos, name) in split_artists(&artists_raw).iter().enumerate() {
            let aid = upsert_artist_by_name(&mut *tx, name).await?;
            sqlx::query(
                "INSERT INTO hub_track_artists (track_id, artist_id, position) VALUES (?1, ?2, ?3)
                 ON CONFLICT(track_id, artist_id) DO NOTHING",
            )
            .bind(tid)
            .bind(aid)
            .bind(pos as i64)
            .execute(&mut *tx)
            .await?;
        }
        if !album.trim().is_empty() {
            let aid = upsert_album_by_title(&mut *tx, &album).await?;
            sqlx::query("UPDATE hub_tracks SET album_id = ?1 WHERE id = ?2 AND album_id IS NULL")
                .bind(aid)
                .bind(tid)
                .execute(&mut *tx)
                .await?;
        }
        n += 1;
    }
    tx.commit().await?;
    Ok(n)
}

/// Backfill every track that still lacks artist links, in batches of 1000, and
/// return the total number of tracks processed. Idempotent: a second run over an
/// already-backfilled DB returns `0`.
pub async fn backfill_artist_entities(pool: &SqlitePool) -> Result<usize> {
    const BATCH: i64 = 1000;
    let mut total = 0usize;
    loop {
        let n = backfill_artist_entities_batch(pool, BATCH).await?;
        if n == 0 {
            break;
        }
        total += n;
    }
    Ok(total)
}

/// Load a track's artists as `(artist_id, name)`, ordered by position.
pub async fn artists_for_track(pool: &SqlitePool, track_id: i64) -> Vec<(i64, String)> {
    sqlx::query(
        "SELECT a.id AS id, a.name AS name
           FROM hub_track_artists ta JOIN hub_artists a ON a.id = ta.artist_id
          WHERE ta.track_id = ?1
          ORDER BY ta.position, a.name",
    )
    .bind(track_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .iter()
    .map(|r| {
        (
            r.get::<i64, _>("id"),
            r.get::<Option<String>, _>("name").unwrap_or_default(),
        )
    })
    .collect()
}

/// Batch: track id -> linked-artist HTML. Tracks without entity links fall back
/// to the escaped display-cache string (`hub_tracks.artists`), so pages never
/// render empty while the entity tables are still being populated.
pub async fn artists_html_for_tracks(pool: &SqlitePool, ids: &[i64]) -> HashMap<i64, String> {
    let mut out: HashMap<i64, String> = HashMap::new();
    if ids.is_empty() {
        return out;
    }
    let mut by_track: HashMap<i64, Vec<(i64, String)>> = HashMap::new();
    for chunk in ids.chunks(CHUNK) {
        let mut qb = QueryBuilder::new(
            "SELECT ta.track_id AS track_id, a.id AS id, a.name AS name
               FROM hub_track_artists ta JOIN hub_artists a ON a.id = ta.artist_id
              WHERE ta.track_id IN ",
        );
        bind_ids(&mut qb, chunk);
        qb.push(" ORDER BY ta.track_id, ta.position, a.name");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            by_track
                .entry(r.get::<i64, _>("track_id"))
                .or_default()
                .push((
                    r.get::<i64, _>("id"),
                    r.get::<Option<String>, _>("name").unwrap_or_default(),
                ));
        }
    }
    let mut fallback: HashMap<i64, String> = HashMap::new();
    for chunk in ids.chunks(CHUNK) {
        let mut qb = QueryBuilder::new(
            "SELECT id, COALESCE(artists, '') AS artists FROM hub_tracks WHERE id IN ",
        );
        bind_ids(&mut qb, chunk);
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            fallback.insert(
                r.get::<i64, _>("id"),
                r.get::<Option<String>, _>("artists").unwrap_or_default(),
            );
        }
    }
    for id in ids {
        let html = match by_track.get(id) {
            Some(list) if !list.is_empty() => artist_links_html(list),
            _ => escape_html(fallback.get(id).map(String::as_str).unwrap_or("")),
        };
        out.insert(*id, html);
    }
    out
}

/// Distinct artists across an album's tracks (entity-based), ordered by name.
pub async fn album_artists(pool: &SqlitePool, album: &str) -> Vec<(i64, String)> {
    sqlx::query(
        "SELECT DISTINCT a.id AS id, a.name AS name
           FROM hub_tracks t
           JOIN hub_track_artists ta ON ta.track_id = t.id
           JOIN hub_artists a ON a.id = ta.artist_id
          WHERE t.album = ?1 COLLATE NOCASE
          ORDER BY a.name",
    )
    .bind(album)
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .iter()
    .map(|r| {
        (
            r.get::<i64, _>("id"),
            r.get::<Option<String>, _>("name").unwrap_or_default(),
        )
    })
    .collect()
}

/// Batch: album title -> linked-artist HTML (entity-based).
pub async fn artists_html_by_album(
    pool: &SqlitePool,
    albums: &[String],
) -> HashMap<String, String> {
    let mut by_album: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    for chunk in albums.chunks(CHUNK) {
        let mut qb = QueryBuilder::new(
            "SELECT DISTINCT t.album AS album, a.id AS id, a.name AS name
               FROM hub_tracks t
               JOIN hub_track_artists ta ON ta.track_id = t.id
               JOIN hub_artists a ON a.id = ta.artist_id
              WHERE t.album IN (",
        );
        {
            let mut sep = qb.separated(", ");
            for a in chunk {
                sep.push_bind(a.clone());
            }
        }
        qb.push(") ORDER BY t.album, a.name");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            by_album
                .entry(r.get::<Option<String>, _>("album").unwrap_or_default())
                .or_default()
                .push((
                    r.get::<i64, _>("id"),
                    r.get::<Option<String>, _>("name").unwrap_or_default(),
                ));
        }
    }
    by_album
        .into_iter()
        .map(|(album, list)| (album, artist_links_html(&list)))
        .collect()
}

struct TrackInfo {
    id: i64,
    album: String,
    artists: Vec<(i64, String)>,
}

async fn load_tracks(pool: &SqlitePool) -> Vec<TrackInfo> {
    let rows = sqlx::query("SELECT id, album FROM hub_tracks")
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    let mut by_track: HashMap<i64, Vec<(i64, String)>> = HashMap::new();
    for r in sqlx::query(
        "SELECT ta.track_id AS track_id, a.id AS id, a.name AS name
           FROM hub_track_artists ta JOIN hub_artists a ON a.id = ta.artist_id
          ORDER BY ta.track_id, ta.position, a.name",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    {
        by_track
            .entry(r.get::<i64, _>("track_id"))
            .or_default()
            .push((
                r.get::<i64, _>("id"),
                r.get::<Option<String>, _>("name").unwrap_or_default(),
            ));
    }
    rows.iter()
        .map(|r| {
            let id = r.get::<i64, _>("id");
            TrackInfo {
                id,
                album: r.get::<Option<String>, _>("album").unwrap_or_default(),
                artists: by_track.remove(&id).unwrap_or_default(),
            }
        })
        .collect()
}

// ── AR1: list page ───────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
struct ArtistFilter {
    q: Option<String>,
    /// `1` = only artists I have played (Traktor).
    plays: Option<String>,
    /// `1` = only artists with at least one tagged track.
    tags: Option<String>,
    /// `1` = only artists appearing in at least one playlist.
    pl: Option<String>,
    /// `name` (default) | `plays` | `tracks`.
    sort: Option<String>,
    /// `asc` | `desc`.
    dir: Option<String>,
}

#[derive(Template)]
#[template(path = "artists.html")]
struct ArtistsPage {
    nav: crate::ui::Nav,
    flash: String,
    q: String,
    plays_only: bool,
    tags_only: bool,
    pl_only: bool,
    sort_name: bool,
    sort_plays: bool,
    sort_tracks: bool,
    total: usize,
    rows: Vec<ArtistRow>,
    s_name: crate::table::SortHead,
    s_tracks: crate::table::SortHead,
    s_plays: crate::table::SortHead,
}

struct ArtistRow {
    name: String,
    href: String,
    spotify: String,
    tracks: i64,
    tagged: i64,
    in_playlists: i64,
    my_plays: i64,
    all_plays: i64,
}

struct ArtistAgg {
    id: i64,
    name: String,
    track_ids: Vec<i64>,
    tagged: i64,
    in_playlists: i64,
    plays_by_user: HashMap<i64, i64>,
}

async fn artists_page(
    State(st): State<AppState>,
    Query(f): Query<ArtistFilter>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "artists").await else {
        return Redirect::to("/login").into_response();
    };
    let me = nav.id;
    let q = f.q.unwrap_or_default().trim().to_string();
    let plays_only = f.plays.as_deref() == Some("1");
    let tags_only = f.tags.as_deref() == Some("1");
    let pl_only = f.pl.as_deref() == Some("1");
    let sort = match f.sort.as_deref() {
        Some("plays") => "plays",
        Some("tracks") => "tracks",
        _ => "name",
    }
    .to_string();
    // Preserve the legacy defaults (name asc, plays/tracks desc) when no
    // explicit direction is given; header links always carry one.
    let dir = match f.dir.as_deref() {
        Some("asc") => "asc",
        Some("desc") => "desc",
        _ if sort == "name" => "asc",
        _ => "desc",
    }
    .to_string();

    let tracks = load_tracks(&st.pool).await;

    // Active-scope filter (#331): keep only tracks in the active backpack's
    // scope before folding artists, so aggregates reflect the scoped set.
    let scope = crate::backpacks::active_scope_for_views(&st.pool, me).await;
    let tracks: Vec<TrackInfo> = match &scope {
        Some(set) => tracks.into_iter().filter(|t| set.contains(&t.id)).collect(),
        None => tracks,
    };

    // Per-track lookups (bounded, single scans — no N+1).
    let tag_counts: HashMap<i64, i64> = sqlx::query(
        "SELECT track_id, COUNT(*) AS c FROM hub_track_resolved_tags GROUP BY track_id",
    )
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default()
    .iter()
    .map(|r| (r.get::<i64, _>("track_id"), r.get::<i64, _>("c")))
    .collect();

    let pl_tracks: HashSet<i64> = sqlx::query("SELECT DISTINCT track_id FROM hub_playlist_tracks")
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| r.get::<i64, _>("track_id"))
        .collect();

    let traktor: Vec<(i64, i64, i64)> =
        sqlx::query("SELECT track_id, user_id, play_count FROM hub_traktor_tracks")
            .fetch_all(&st.pool)
            .await
            .unwrap_or_default()
            .iter()
            .map(|r| {
                (
                    r.get::<i64, _>("track_id"),
                    r.get::<i64, _>("user_id"),
                    r.get::<i64, _>("play_count"),
                )
            })
            .collect();

    // Fold every track into its (possibly several) artists (entity ids).
    let mut map: HashMap<i64, ArtistAgg> = HashMap::new();
    for t in &tracks {
        for (aid, name) in &t.artists {
            let a = map.entry(*aid).or_insert_with(|| ArtistAgg {
                id: *aid,
                name: name.clone(),
                track_ids: Vec::new(),
                tagged: 0,
                in_playlists: 0,
                plays_by_user: HashMap::new(),
            });
            a.track_ids.push(t.id);
        }
    }
    for a in map.values_mut() {
        let mut seen_pl = HashSet::new();
        let mut seen_tag = HashSet::new();
        for tid in &a.track_ids {
            if tag_counts.get(tid).copied().unwrap_or(0) > 0 {
                seen_tag.insert(*tid);
            }
            if pl_tracks.contains(tid) {
                seen_pl.insert(*tid);
            }
        }
        a.tagged = seen_tag.len() as i64;
        a.in_playlists = seen_pl.len() as i64;
        for (tid, uid, pc) in &traktor {
            if a.track_ids.contains(tid) {
                *a.plays_by_user.entry(*uid).or_insert(0) += *pc;
            }
        }
    }

    // Order-independent tokenised search: every token must appear in the name.
    let toks = crate::table::tokens(&q);
    let mut rows: Vec<ArtistRow> = map
        .into_values()
        .filter(|a| {
            let lname = a.name.to_lowercase();
            (toks.iter().all(|t| lname.contains(t.as_str())))
                && (!plays_only || a.plays_by_user.get(&me).copied().unwrap_or(0) > 0)
                && (!tags_only || a.tagged > 0)
                && (!pl_only || a.in_playlists > 0)
        })
        .map(|a| {
            let my_plays = a.plays_by_user.get(&me).copied().unwrap_or(0);
            let all_plays: i64 = a.plays_by_user.values().sum();
            ArtistRow {
                spotify: spotify_href(&a.name),
                href: artist_href(a.id),
                tracks: a.track_ids.len() as i64,
                tagged: a.tagged,
                in_playlists: a.in_playlists,
                my_plays,
                all_plays,
                name: a.name,
            }
        })
        .collect();

    match sort.as_str() {
        "plays" => rows.sort_by(|a, b| {
            a.my_plays
                .cmp(&b.my_plays)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        }),
        "tracks" => rows.sort_by(|a, b| {
            a.tracks
                .cmp(&b.tracks)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        }),
        _ => rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
    }
    if dir == "desc" {
        rows.reverse();
    }

    let total = rows.len();
    rows.truncate(LIST_LIMIT);

    let rawq = raw.as_deref().unwrap_or("");
    let s_name = crate::table::sort_head(rawq, "name", "Künstler", &sort, &dir);
    let s_tracks = crate::table::sort_head(rawq, "tracks", "Tracks", &sort, &dir);
    let s_plays = crate::table::sort_head(rawq, "plays", "Plays (mir)", &sort, &dir);

    render(&ArtistsPage {
        nav,
        flash: String::new(),
        q,
        plays_only,
        tags_only,
        pl_only,
        sort_name: sort == "name",
        sort_plays: sort == "plays",
        sort_tracks: sort == "tracks",
        total,
        rows,
        s_name,
        s_tracks,
        s_plays,
    })
}

// ── AR2–AR6: profile page ────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
struct ArtistQuery {
    /// Selected reference user (defaults to me).
    user: Option<String>,
    /// Whitelisted sort field for the track tables (`t_*`), the album table
    /// (`al_*`) or the playlist tables (`pl_*`); unknown fields fall back to
    /// each table's own default.
    sort: Option<String>,
    /// `asc` | `desc`.
    dir: Option<String>,
}

#[derive(Template)]
#[template(path = "artist.html")]
struct ArtistPage {
    nav: crate::ui::Nav,
    flash: String,
    name: String,
    track_count: i64,
    total_plays: i64,
    user_count: i64,
    spotify: String,
    self_href: String,

    // AR2
    has_me: bool,
    my_plays: Vec<PlayRow>,
    others: Vec<OtherUser>,

    // AR3
    has_tags: bool,
    tag_types: Vec<TypeDist>,
    top_tags: Vec<TagCount>,

    // AR4
    my_playlists: Vec<PlaylistRow>,
    other_playlist_groups: Vec<OwnerPlaylists>,

    // AR5
    collabs: Vec<CollabRow>,
    b2b: Vec<B2bRow>,
    albums: Vec<AlbumRow>,

    // AR6
    ref_user: String,
    ref_user_options: Vec<RefUserOption>,
    references: Vec<RefRow>,

    // sortable headers (AR2 tracks, AR4 playlists, AR5c albums)
    s_t_title: crate::table::SortHead,
    s_t_album: crate::table::SortHead,
    s_t_plays: crate::table::SortHead,
    s_t_last: crate::table::SortHead,
    s_al_album: crate::table::SortHead,
    s_al_tracks: crate::table::SortHead,
    s_pl_name: crate::table::SortHead,
    s_pl_owner: crate::table::SortHead,
    s_pl_shared: crate::table::SortHead,
    s_pl_scope: crate::table::SortHead,
}

struct PlayRow {
    track_id: i64,
    title: String,
    album: String,
    plays: i64,
    last: String,
}

struct OtherUser {
    slug: String,
    total_plays: i64,
    rows: Vec<PlayRow>,
}

struct TypeDist {
    name: String,
    icon: String,
    tracks: i64,
}

struct TagCount {
    id: i64,
    name: String,
    tracks: i64,
}

struct PlaylistRow {
    id: i64,
    name: String,
    owner_name: String,
    shared: i64,
    owned: bool,
}

struct OwnerPlaylists {
    slug: String,
    rows: Vec<PlaylistRow>,
}

struct CollabRow {
    name: String,
    href: String,
    tracks: i64,
    plays: i64,
}

struct B2bRow {
    playlist: String,
    user_slug: String,
    tracks: i64,
    others: String,
}

struct AlbumRow {
    album: String,
    tracks: i64,
    others: String,
}

struct RefRow {
    name: String,
    href: String,
    score: String,
    shared_tags: i64,
    collab: i64,
    b2b: i64,
}

struct RefUserOption {
    slug: String,
    selected: bool,
    count: i64,
}

/// Sort a per-user play table in place (server-side, header-driven).
fn sort_playrows(rows: &mut [PlayRow], field: &str, dir: &str) {
    let ci = |s: &str| s.to_lowercase();
    match field {
        "t_title" => rows.sort_by(|a, b| ci(&a.title).cmp(&ci(&b.title))),
        "t_album" => rows.sort_by(|a, b| {
            ci(&a.album)
                .cmp(&ci(&b.album))
                .then_with(|| ci(&a.title).cmp(&ci(&b.title)))
        }),
        "t_last" => rows.sort_by(|a, b| {
            a.last
                .cmp(&b.last)
                .then_with(|| ci(&a.title).cmp(&ci(&b.title)))
        }),
        _ => rows.sort_by(|a, b| {
            a.plays
                .cmp(&b.plays)
                .then_with(|| ci(&a.title).cmp(&ci(&b.title)))
        }),
    }
    if dir.eq_ignore_ascii_case("desc") {
        rows.reverse();
    }
}

/// Sort a playlist table in place (server-side, header-driven).
fn sort_playlists(rows: &mut [PlaylistRow], field: &str, dir: &str) {
    let ci = |s: &str| s.to_lowercase();
    match field {
        "pl_owner" => rows.sort_by(|a, b| {
            ci(&a.owner_name)
                .cmp(&ci(&b.owner_name))
                .then_with(|| ci(&a.name).cmp(&ci(&b.name)))
        }),
        "pl_shared" => rows.sort_by(|a, b| {
            a.shared
                .cmp(&b.shared)
                .then_with(|| ci(&a.name).cmp(&ci(&b.name)))
        }),
        "pl_scope" => rows.sort_by(|a, b| {
            (a.owned as i64)
                .cmp(&(b.owned as i64))
                .then_with(|| ci(&a.name).cmp(&ci(&b.name)))
        }),
        _ => rows.sort_by(|a, b| ci(&a.name).cmp(&ci(&b.name))),
    }
    if dir.eq_ignore_ascii_case("desc") {
        rows.reverse();
    }
}

async fn artist_page(
    State(st): State<AppState>,
    Path(key): Path<String>,
    Query(f): Query<ArtistQuery>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "artists").await else {
        return Redirect::to("/login").into_response();
    };
    let me = nav.id;

    // Canonical route is `/artist/{id}` (numeric). A name is resolved and
    // redirected to the id; an unknown key 404s.
    let target_id: i64 = if let Ok(id) = key.trim().parse::<i64>() {
        match sqlx::query_scalar::<_, i64>("SELECT id FROM hub_artists WHERE id = ?1")
            .bind(id)
            .fetch_optional(&st.pool)
            .await
            .ok()
            .flatten()
        {
            Some(id) => id,
            None => return not_found("Künstler nicht gefunden."),
        }
    } else {
        match artist_id_by_name(&st.pool, &key).await {
            Some(id) => return Redirect::to(&format!("/artist/{id}")).into_response(),
            None => return not_found("Künstler nicht gefunden."),
        }
    };

    // Per-table whitelists (disjoint field names so the shared `?sort=` only
    // ever activates one table; the others keep their own default order).
    let tracks_allowed: &[(&str, &str)] = &[
        ("t_title", "title"),
        ("t_album", "album"),
        ("t_plays", "plays"),
        ("t_last", "last"),
    ];
    let albums_allowed: &[(&str, &str)] = &[("al_album", "album"), ("al_tracks", "tracks")];
    let pl_allowed: &[(&str, &str)] = &[
        ("pl_name", "name"),
        ("pl_owner", "owner_name"),
        ("pl_shared", "shared"),
        ("pl_scope", "owned"),
    ];
    let (t_sort, t_dir) = crate::table::resolve_sort(
        f.sort.as_deref(),
        f.dir.as_deref(),
        tracks_allowed,
        "t_plays",
        "desc",
    );
    let (al_sort, al_dir) = crate::table::resolve_sort(
        f.sort.as_deref(),
        f.dir.as_deref(),
        albums_allowed,
        "al_tracks",
        "desc",
    );
    let (pl_sort, pl_dir) = crate::table::resolve_sort(
        f.sort.as_deref(),
        f.dir.as_deref(),
        pl_allowed,
        "pl_name",
        "asc",
    );

    let tracks = load_tracks(&st.pool).await;
    let artist_tracks: Vec<&TrackInfo> = tracks
        .iter()
        .filter(|t| t.artists.iter().any(|(id, _)| *id == target_id))
        .collect();
    if artist_tracks.is_empty() {
        return not_found("Künstler nicht gefunden.");
    }
    let display_name: String =
        sqlx::query_scalar::<_, String>("SELECT name FROM hub_artists WHERE id = ?1")
            .bind(target_id)
            .fetch_optional(&st.pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| key.clone());
    let artist_ids: Vec<i64> = artist_tracks.iter().map(|t| t.id).collect();

    // track -> artists (entity ids), for the collab/b2b/album/reference panels.
    let artists_by_track: HashMap<i64, Vec<(i64, String)>> =
        tracks.iter().map(|t| (t.id, t.artists.clone())).collect();

    // Users (id ↔ slug) for grouping / selectors.
    let users: Vec<(i64, String)> = sqlx::query("SELECT id, slug FROM hub_users ORDER BY slug")
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| (r.get::<i64, _>("id"), r.get::<String, _>("slug")))
        .collect();
    let slug_of = |uid: i64| -> String {
        users
            .iter()
            .find(|(id, _)| *id == uid)
            .map(|(_, s)| s.clone())
            .unwrap_or_else(|| format!("user{uid}"))
    };

    // ── AR2: per-user play counts ────────────────────────────────────────────
    let mut traktor_rows: Vec<(i64, i64, i64, String, String, String)> = Vec::new(); // uid, track, plays, title, album, last
    for chunk in artist_ids.chunks(CHUNK) {
        let mut qb = QueryBuilder::new(
            "SELECT tt.user_id AS user_id, tt.track_id AS track_id, tt.play_count AS play_count,
                    t.title AS title, t.album AS album, tt.last_played AS last_played
               FROM hub_traktor_tracks tt
               JOIN hub_tracks t ON t.id = tt.track_id
              WHERE tt.play_count > 0 AND tt.track_id IN ",
        );
        bind_ids(&mut qb, chunk);
        traktor_rows.extend(
            qb.build()
                .fetch_all(&st.pool)
                .await
                .unwrap_or_default()
                .iter()
                .map(|r| {
                    (
                        r.get::<i64, _>("user_id"),
                        r.get::<i64, _>("track_id"),
                        r.get::<i64, _>("play_count"),
                        r.get::<Option<String>, _>("title").unwrap_or_default(),
                        r.get::<Option<String>, _>("album").unwrap_or_default(),
                        r.get::<Option<String>, _>("last_played")
                            .unwrap_or_default(),
                    )
                }),
        );
    }
    // track -> [(uid, plays)]
    let mut plays_by_track: HashMap<i64, Vec<(i64, i64)>> = HashMap::new();
    // uid -> [(track, plays, title, album, last)]
    let mut plays_by_user: HashMap<i64, Vec<(i64, i64, String, String, String)>> = HashMap::new();
    for (uid, tid, pc, title, album, last) in &traktor_rows {
        plays_by_track.entry(*tid).or_default().push((*uid, *pc));
        plays_by_user.entry(*uid).or_default().push((
            *tid,
            *pc,
            title.clone(),
            album.clone(),
            last.clone(),
        ));
    }
    let mut my_plays: Vec<PlayRow> = Vec::new();
    let mut others: Vec<OtherUser> = Vec::new();
    let user_count = plays_by_user.len() as i64;
    for (uid, list) in plays_by_user {
        let total: i64 = list.iter().map(|x| x.1).sum();
        let mut rows: Vec<PlayRow> = list
            .into_iter()
            .map(|(tid, pc, title, album, last)| PlayRow {
                track_id: tid,
                title,
                album,
                plays: pc,
                last,
            })
            .collect();
        sort_playrows(&mut rows, &t_sort, &t_dir);
        rows.truncate(20);
        if uid == me {
            my_plays = rows;
        } else {
            others.push(OtherUser {
                slug: slug_of(uid),
                total_plays: total,
                rows,
            });
        }
    }
    others.sort_by(|a, b| {
        b.total_plays
            .cmp(&a.total_plays)
            .then_with(|| a.slug.cmp(&b.slug))
    });
    let has_me = !my_plays.is_empty();

    let total_plays: i64 = traktor_rows.iter().map(|r| r.2).sum();

    // ── AR3: tags of the artist's tracks ─────────────────────────────────────
    let mut tag_types: Vec<TypeDist> = Vec::new();
    let mut top_tags: Vec<TagCount> = Vec::new();
    let mut artist_tag_ids: HashSet<i64> = HashSet::new();
    for chunk in artist_ids.chunks(CHUNK) {
        let mut qb = QueryBuilder::new(
            "SELECT COALESCE(t.type,'') AS type, COUNT(DISTINCT rt.track_id) AS tracks
               FROM hub_track_resolved_tags rt
               JOIN hub_tags t ON t.id = rt.tag_id
              WHERE t.type IS NOT NULL AND rt.track_id IN ",
        );
        bind_ids(&mut qb, chunk);
        qb.push(" GROUP BY t.type ORDER BY tracks DESC, type");
        tag_types.extend(
            qb.build()
                .fetch_all(&st.pool)
                .await
                .unwrap_or_default()
                .iter()
                .filter_map(|r| {
                    let ty: String = r.get("type");
                    let t = crate::tagtypes::TagType::parse(&ty)?;
                    // Only types that *describe* a track belong in this view.
                    if !t.describes() {
                        return None;
                    }
                    Some(TypeDist {
                        name: t.label().to_string(),
                        icon: t.icon().to_string(),
                        tracks: r.get::<Option<i64>, _>("tracks").unwrap_or(0),
                    })
                }),
        );

        let mut qb = QueryBuilder::new(
            "SELECT rt.tag_id AS tag_id, rt.track_id AS track_id
               FROM hub_track_resolved_tags rt WHERE rt.track_id IN ",
        );
        bind_ids(&mut qb, chunk);
        for r in qb.build().fetch_all(&st.pool).await.unwrap_or_default() {
            artist_tag_ids.insert(r.get::<i64, _>("tag_id"));
        }

        let mut qb = QueryBuilder::new(
            "SELECT t.id AS id, t.name AS name, COUNT(DISTINCT rt.track_id) AS tracks
               FROM hub_track_resolved_tags rt
               JOIN hub_tags t ON t.id = rt.tag_id
              WHERE rt.track_id IN ",
        );
        bind_ids(&mut qb, chunk);
        qb.push(" GROUP BY t.id ORDER BY tracks DESC, t.name LIMIT 30");
        top_tags.extend(
            qb.build()
                .fetch_all(&st.pool)
                .await
                .unwrap_or_default()
                .iter()
                .map(|r| TagCount {
                    id: r.get::<i64, _>("id"),
                    name: r.get::<Option<String>, _>("name").unwrap_or_default(),
                    tracks: r.get::<Option<i64>, _>("tracks").unwrap_or(0),
                }),
        );
    }
    // Merge per-chunk duplicates (chunk size can split identical rows).
    top_tags.sort_by(|a, b| b.tracks.cmp(&a.tracks).then_with(|| a.name.cmp(&b.name)));
    top_tags.dedup_by(|a, b| a.id == b.id);
    top_tags.truncate(30);
    let has_tags = !artist_tag_ids.is_empty() || !top_tags.is_empty();

    // ── AR4: playlist membership ─────────────────────────────────────────────
    let mut my_playlists: Vec<PlaylistRow> = Vec::new();
    let mut pl_by_user: BTreeMap<String, Vec<PlaylistRow>> = BTreeMap::new();
    for chunk in artist_ids.chunks(CHUNK) {
        let mut qb = QueryBuilder::new(
            "SELECT hp.id AS id, hp.name AS name, hp.user_id AS user_id, hp.is_owned AS is_owned,
                    u.slug AS slug,
                    COALESCE(NULLIF(hp.owner_name, ''), u.display_name) AS owner_name,
                    COUNT(DISTINCT hpt.track_id) AS shared
               FROM hub_playlist_tracks hpt
               JOIN hub_playlists hp ON hp.id = hpt.playlist_id
               JOIN hub_users u ON u.id = hp.user_id
              WHERE hpt.track_id IN ",
        );
        bind_ids(&mut qb, chunk);
        qb.push(" GROUP BY hp.id ORDER BY u.slug, hp.name");
        for r in qb.build().fetch_all(&st.pool).await.unwrap_or_default() {
            let uid: i64 = r.get("user_id");
            let row = PlaylistRow {
                id: r.get::<i64, _>("id"),
                name: r.get::<Option<String>, _>("name").unwrap_or_default(),
                owner_name: r.get::<Option<String>, _>("owner_name").unwrap_or_default(),
                shared: r.get::<Option<i64>, _>("shared").unwrap_or(0),
                owned: r.get::<Option<i64>, _>("is_owned").unwrap_or(0) == 1,
            };
            if uid == me {
                my_playlists.push(row);
            } else {
                pl_by_user
                    .entry(r.get::<String, _>("slug"))
                    .or_default()
                    .push(row);
            }
        }
    }
    sort_playlists(&mut my_playlists, &pl_sort, &pl_dir);
    my_playlists.dedup_by(|a, b| a.id == b.id);
    let other_playlist_groups: Vec<OwnerPlaylists> = pl_by_user
        .into_iter()
        .map(|(slug, mut rows)| {
            sort_playlists(&mut rows, &pl_sort, &pl_dir);
            rows.dedup_by(|a, b| a.id == b.id);
            OwnerPlaylists { slug, rows }
        })
        .collect();

    // ── AR5a: collaboration partners ─────────────────────────────────────────
    // artist id -> (name, tracks, plays, track ids)
    let mut collab: HashMap<i64, (String, i64, i64, HashSet<i64>)> = HashMap::new();
    for t in &artist_tracks {
        if t.artists.len() < 2 {
            continue;
        }
        let plays: i64 = plays_by_track
            .get(&t.id)
            .map(|v| v.iter().map(|(_, p)| *p).sum())
            .unwrap_or(0);
        for (aid, name) in &t.artists {
            if *aid == target_id {
                continue;
            }
            let e = collab
                .entry(*aid)
                .or_insert((name.clone(), 0, 0, HashSet::new()));
            e.1 += 1;
            e.2 += plays;
            e.3.insert(t.id);
        }
    }
    let mut collabs: Vec<CollabRow> = collab
        .iter()
        .map(|(aid, (name, t, p, _))| CollabRow {
            href: artist_href(*aid),
            name: name.clone(),
            tracks: *t,
            plays: *p,
        })
        .collect();
    collabs.sort_by(|a, b| b.tracks.cmp(&a.tracks).then_with(|| b.plays.cmp(&a.plays)));

    // ── AR5b: b2b sets (Traktor playlists named *b2b*) ───────────────────────
    let mut b2b: Vec<B2bRow> = Vec::new();
    // partner artist id -> (set count, track ids, display name) for the AR6 pool.
    let mut b2b_pairs: HashMap<i64, (i64, HashSet<i64>, String)> = HashMap::new();
    for chunk in artist_ids.chunks(CHUNK) {
        let mut qb = QueryBuilder::new(
            "SELECT p.user_id AS user_id, p.id AS pid, p.name AS pname,
                    u.slug AS slug, t.id AS track_id
               FROM hub_traktor_playlists p
               JOIN hub_users u ON u.id = p.user_id
               JOIN hub_traktor_playlist_tracks pt
                 ON pt.user_id = p.user_id AND pt.playlist_id = p.id
               JOIN hub_tracks t ON t.id = pt.track_id
              WHERE lower(p.name) LIKE '%b2b%'
                AND EXISTS (SELECT 1 FROM hub_traktor_playlist_tracks x
                             WHERE x.user_id = p.user_id AND x.playlist_id = p.id
                               AND x.track_id IN ",
        );
        bind_ids(&mut qb, chunk);
        qb.push(")");
        let rows = qb.build().fetch_all(&st.pool).await.unwrap_or_default();
        let mut grouped: BTreeMap<(i64, i64), (String, String, i64, HashSet<String>)> =
            BTreeMap::new();
        for r in &rows {
            let uid: i64 = r.get("user_id");
            let pid: i64 = r.get("pid");
            let track_id: i64 = r.get("track_id");
            let entry = grouped.entry((uid, pid)).or_insert_with(|| {
                (
                    r.get::<Option<String>, _>("pname").unwrap_or_default(),
                    r.get::<Option<String>, _>("slug").unwrap_or_default(),
                    0,
                    HashSet::new(),
                )
            });
            for (aid, name) in artists_by_track
                .get(&track_id)
                .map(Vec::as_slice)
                .unwrap_or(&[])
            {
                if *aid == target_id {
                    entry.2 += 1;
                } else {
                    entry.3.insert(name.clone());
                    let bp = b2b_pairs
                        .entry(*aid)
                        .or_insert((0, HashSet::new(), name.clone()));
                    bp.0 += 1;
                    bp.1.insert(track_id);
                }
            }
        }
        for (_, (pname, slug, tracks, others)) in grouped {
            let mut names: Vec<String> = others.into_iter().collect();
            names.sort();
            b2b.push(B2bRow {
                playlist: pname,
                user_slug: slug,
                tracks,
                others: names.join(", "),
            });
        }
    }
    b2b.sort_by(|a, b| {
        b.tracks
            .cmp(&a.tracks)
            .then_with(|| a.playlist.cmp(&b.playlist))
    });
    b2b.dedup_by(|a, b| a.playlist == b.playlist && a.user_slug == b.user_slug);

    // ── AR5c: artists on the same albums ─────────────────────────────────────
    let mut album_set: HashSet<String> = HashSet::new();
    for t in &artist_tracks {
        if !t.album.is_empty() {
            album_set.insert(t.album.clone());
        }
    }
    let my_album_tracks: HashMap<String, i64> = {
        let mut m: HashMap<String, i64> = HashMap::new();
        for t in &artist_tracks {
            if !t.album.is_empty() {
                *m.entry(t.album.clone()).or_insert(0) += 1;
            }
        }
        m
    };
    let album_list: Vec<String> = album_set.into_iter().collect();
    let mut album_others: HashMap<String, HashSet<String>> = HashMap::new();
    for chunk in album_list.chunks(CHUNK) {
        let mut qb = QueryBuilder::new("SELECT id, album FROM hub_tracks WHERE album IN (");
        {
            let mut sep = qb.separated(", ");
            for a in chunk {
                sep.push_bind(a.clone());
            }
        }
        qb.push(")");
        for r in qb.build().fetch_all(&st.pool).await.unwrap_or_default() {
            let album: String = r.get::<Option<String>, _>("album").unwrap_or_default();
            let tid: i64 = r.get("id");
            for (aid, name) in artists_by_track.get(&tid).map(Vec::as_slice).unwrap_or(&[]) {
                if *aid != target_id {
                    album_others
                        .entry(album.clone())
                        .or_default()
                        .insert(name.clone());
                }
            }
        }
    }
    let mut albums: Vec<AlbumRow> = my_album_tracks
        .into_iter()
        .map(|(album, count)| {
            let mut names: Vec<String> = album_others
                .get(&album)
                .map(|s| s.iter().cloned().collect())
                .unwrap_or_default();
            names.sort();
            AlbumRow {
                album,
                tracks: count,
                others: names.join(", "),
            }
        })
        .collect();
    match al_sort.as_str() {
        "al_album" => albums.sort_by(|a, b| a.album.to_lowercase().cmp(&b.album.to_lowercase())),
        _ => albums.sort_by(|a, b| {
            a.tracks
                .cmp(&b.tracks)
                .then_with(|| a.album.to_lowercase().cmp(&b.album.to_lowercase()))
        }),
    }
    if al_dir.eq_ignore_ascii_case("desc") {
        albums.reverse();
    }

    // ── AR6: reference artists ───────────────────────────────────────────────
    // shared-tag candidates → artist id -> (name, shared tag ids, track ids)
    let mut cand: HashMap<i64, (String, HashSet<i64>, HashSet<i64>)> = HashMap::new();
    let tag_id_list: Vec<i64> = artist_tag_ids.iter().copied().collect();
    for chunk in tag_id_list.chunks(CHUNK) {
        let mut qb = QueryBuilder::new(
            "SELECT rt.tag_id AS tag_id, rt.track_id AS track_id
               FROM hub_track_resolved_tags rt
              WHERE rt.tag_id IN ",
        );
        bind_ids(&mut qb, chunk);
        for r in qb.build().fetch_all(&st.pool).await.unwrap_or_default() {
            let tag_id: i64 = r.get("tag_id");
            let track_id: i64 = r.get("track_id");
            for (aid, name) in artists_by_track
                .get(&track_id)
                .map(Vec::as_slice)
                .unwrap_or(&[])
            {
                if *aid == target_id {
                    continue;
                }
                let e = cand
                    .entry(*aid)
                    .or_insert_with(|| (name.clone(), HashSet::new(), HashSet::new()));
                e.1.insert(tag_id);
                e.2.insert(track_id);
            }
        }
    }
    // collaboration partners feed the same candidate pool.
    for (aid, (name, _, _, tids)) in &collab {
        let e = cand
            .entry(*aid)
            .or_insert_with(|| (name.clone(), HashSet::new(), HashSet::new()));
        e.2.extend(tids.iter().copied());
    }
    // b2b co-artists feed the same pool.
    for (aid, (_, tids, name)) in &b2b_pairs {
        let e = cand
            .entry(*aid)
            .or_insert_with(|| (name.clone(), HashSet::new(), HashSet::new()));
        e.2.extend(tids.iter().copied());
    }

    // Possession: which users "have" a candidate (like / own playlist / played).
    let mut all_cand_tracks: HashSet<i64> = HashSet::new();
    for (_, _, tids) in cand.values() {
        all_cand_tracks.extend(tids.iter().copied());
    }
    let cand_track_list: Vec<i64> = all_cand_tracks.into_iter().collect();
    let mut track_users: HashMap<i64, HashSet<i64>> = HashMap::new();
    for chunk in cand_track_list.chunks(CHUNK) {
        let mut qb =
            QueryBuilder::new("SELECT user_id, track_id FROM hub_liked_tracks WHERE track_id IN ");
        bind_ids(&mut qb, chunk);
        for r in qb.build().fetch_all(&st.pool).await.unwrap_or_default() {
            track_users
                .entry(r.get::<i64, _>("track_id"))
                .or_default()
                .insert(r.get::<i64, _>("user_id"));
        }

        let mut qb = QueryBuilder::new(
            "SELECT DISTINCT hp.user_id AS user_id, hpt.track_id AS track_id
               FROM hub_playlist_tracks hpt JOIN hub_playlists hp ON hp.id = hpt.playlist_id
              WHERE hpt.track_id IN ",
        );
        bind_ids(&mut qb, chunk);
        for r in qb.build().fetch_all(&st.pool).await.unwrap_or_default() {
            track_users
                .entry(r.get::<i64, _>("track_id"))
                .or_default()
                .insert(r.get::<i64, _>("user_id"));
        }

        let mut qb = QueryBuilder::new(
            "SELECT DISTINCT user_id, track_id FROM hub_traktor_tracks
              WHERE play_count > 0 AND track_id IN ",
        );
        bind_ids(&mut qb, chunk);
        for r in qb.build().fetch_all(&st.pool).await.unwrap_or_default() {
            track_users
                .entry(r.get::<i64, _>("track_id"))
                .or_default()
                .insert(r.get::<i64, _>("user_id"));
        }
    }
    let cand_users: HashMap<i64, HashSet<i64>> = cand
        .iter()
        .map(|(aid, (_, _, tids))| {
            let mut u: HashSet<i64> = HashSet::new();
            for tid in tids {
                if let Some(set) = track_users.get(tid) {
                    u.extend(set.iter().copied());
                }
            }
            (*aid, u)
        })
        .collect();

    // Deterministic score over shared tags + collaborations + b2b.
    let mut scored: Vec<(i64, String, f64, i64, i64, i64)> = cand
        .iter()
        .map(|(aid, (name, tags, _))| {
            let shared = tags.len() as i64;
            let c = collab.get(aid).map(|(_, t, _, _)| *t).unwrap_or(0);
            let b = b2b_pairs.get(aid).map(|(cnt, _, _)| *cnt).unwrap_or(0);
            let score = REF_TAG_W * shared as f64 + REF_COLLAB_W * c as f64 + REF_B2B_W * b as f64;
            (*aid, name.clone(), score, shared, c, b)
        })
        .collect();
    scored.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
    });

    // Selected user (default: me).
    let selected_slug = f
        .user
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| nav.slug.clone());
    let selected_uid = users
        .iter()
        .find(|(_, s)| s.eq_ignore_ascii_case(&selected_slug))
        .map(|(id, _)| *id)
        .unwrap_or(me);

    let ref_user_options: Vec<RefUserOption> = users
        .iter()
        .map(|(id, slug)| RefUserOption {
            slug: slug.clone(),
            selected: *id == selected_uid,
            count: scored
                .iter()
                .filter(|row| {
                    cand_users
                        .get(&row.0)
                        .map(|u| u.contains(id))
                        .unwrap_or(false)
                })
                .count() as i64,
        })
        .collect();

    let references: Vec<RefRow> = scored
        .iter()
        .filter(|row| {
            cand_users
                .get(&row.0)
                .map(|u| u.contains(&selected_uid))
                .unwrap_or(false)
        })
        .take(30)
        .map(|row| RefRow {
            name: row.1.clone(),
            href: artist_href(row.0),
            score: format!("{:.1}", row.2),
            shared_tags: row.3,
            collab: row.4,
            b2b: row.5,
        })
        .collect();

    let rawq = raw.as_deref().unwrap_or("");
    let s_t_title = crate::table::sort_head(rawq, "t_title", "Track", &t_sort, &t_dir);
    let s_t_album = crate::table::sort_head(rawq, "t_album", "Album", &t_sort, &t_dir);
    let s_t_plays = crate::table::sort_head(rawq, "t_plays", "Plays", &t_sort, &t_dir);
    let s_t_last = crate::table::sort_head(rawq, "t_last", "Zuletzt", &t_sort, &t_dir);
    let s_al_album = crate::table::sort_head(rawq, "al_album", "Album", &al_sort, &al_dir);
    let s_al_tracks = crate::table::sort_head(rawq, "al_tracks", "Tracks", &al_sort, &al_dir);
    let s_pl_name = crate::table::sort_head(rawq, "pl_name", "Playlist", &pl_sort, &pl_dir);
    let s_pl_owner = crate::table::sort_head(rawq, "pl_owner", "Owner", &pl_sort, &pl_dir);
    let s_pl_shared = crate::table::sort_head(rawq, "pl_shared", "Tracks", &pl_sort, &pl_dir);
    let s_pl_scope = crate::table::sort_head(rawq, "pl_scope", "Scope", &pl_sort, &pl_dir);

    render(&ArtistPage {
        nav,
        flash: String::new(),
        name: display_name.clone(),
        track_count: artist_ids.len() as i64,
        total_plays,
        user_count,
        spotify: spotify_href(&display_name),
        self_href: artist_href(target_id),
        has_me,
        my_plays,
        others,
        has_tags,
        tag_types,
        top_tags,
        my_playlists,
        other_playlist_groups,
        collabs,
        b2b,
        albums,
        ref_user: selected_slug,
        ref_user_options,
        references,
        s_t_title,
        s_t_album,
        s_t_plays,
        s_t_last,
        s_al_album,
        s_al_tracks,
        s_pl_name,
        s_pl_owner,
        s_pl_shared,
        s_pl_scope,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_handles_all_delimiters() {
        assert_eq!(split_artists("A; B"), vec!["A", "B"]);
        assert_eq!(split_artists("A, B"), vec!["A", "B"]);
        assert_eq!(split_artists("A & B"), vec!["A", "B"]);
        assert_eq!(split_artists("A feat. B"), vec!["A", "B"]);
        assert_eq!(split_artists("A FEAT B"), vec!["A", "B"]);
        assert_eq!(split_artists("A ft. B"), vec!["A", "B"]);
    }

    #[test]
    fn split_dedupes_and_trims() {
        assert_eq!(split_artists("  A ; a ;B "), vec!["A", "B"]);
        assert!(split_artists("").is_empty());
    }

    #[test]
    fn hrefs_are_id_based() {
        assert_eq!(artist_href(42), "/artist/42");
        assert!(spotify_href("A/B").starts_with("https://open.spotify.com/search/"));
    }

    #[test]
    fn links_are_escaped() {
        let html = artist_links_html(&[(1, "A & B".into()), (2, "<X>".into())]);
        assert_eq!(
            html,
            "<a href=\"/artist/1\">A &amp; B</a>, <a href=\"/artist/2\">&lt;X&gt;</a>"
        );
    }
}
