//! Artist explorer (plan AR1–AR6, issues #227–#232).
//!
//! Two session-gated pages:
//!
//! * `GET /artists`      — list of every artist with server-side filters
//!   (name search, "played by me", "tagged", "in playlists") and sorting.
//! * `GET /artist/{name}` — profile: most played tracks per user, tag
//!   distribution, playlist membership, collaborations / b2b / album
//!   co-artists and reference scoring.
//!
//! Artists are not a table: they are derived from the free-text
//! `hub_tracks.artists` column by splitting on `;`, `,`, `&` and ` feat. `
//! (case-insensitive). Play counts come from the per-user Traktor ingest
//! (`hub_traktor_tracks`); tags from the materialized `hub_track_resolved_tags`
//! joined to `hub_tag_groups` / `hub_tags`.
//!
//! Assumptions / limitations (documented in the PR):
//! * Splitting a comma inside a single artist name (`Earth, Wind & Fire`) is a
//!   known, accepted limitation — the plan explicitly requires those delimiters.
//! * "b2b" is best-effort: Traktor playlists whose name contains `b2b`
//!   (case-insensitive) that contain at least one track by the artist.
//! * Reference coefficients are constants for now; a follow-up makes them
//!   settings (plan §1 I3 / G3).

use std::collections::{BTreeMap, HashMap, HashSet};

use askama::Template;
use axum::Router;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use serde::Deserialize;
use sqlx::{QueryBuilder, Row, Sqlite, SqlitePool};

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
        .route("/artist/{name}", get(artist_page))
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

fn artist_key(s: &str) -> String {
    s.trim().to_lowercase()
}

fn artist_href(name: &str) -> String {
    format!("/artist/{}", urlencoding::encode(name))
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

struct TrackInfo {
    id: i64,
    album: String,
    artists: String,
}

async fn load_tracks(pool: &SqlitePool) -> Vec<TrackInfo> {
    let rows = sqlx::query("SELECT id, title, album, artists FROM hub_tracks")
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    rows.iter()
        .map(|r| TrackInfo {
            id: r.get::<i64, _>("id"),
            album: r.get::<Option<String>, _>("album").unwrap_or_default(),
            artists: r.get::<Option<String>, _>("artists").unwrap_or_default(),
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

    // Fold every track into its (possibly several) artists.
    let mut map: HashMap<String, ArtistAgg> = HashMap::new();
    for t in &tracks {
        for name in split_artists(&t.artists) {
            let key = artist_key(&name);
            let a = map.entry(key).or_insert_with(|| ArtistAgg {
                name,
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
                href: artist_href(&a.name),
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
    tag_groups: Vec<GroupDist>,
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

struct GroupDist {
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
    Path(name): Path<String>,
    Query(f): Query<ArtistQuery>,
    RawQuery(raw): RawQuery,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "artists").await else {
        return Redirect::to("/login").into_response();
    };
    let me = nav.id;
    let target = artist_key(&name);

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
        .filter(|t| {
            split_artists(&t.artists)
                .iter()
                .any(|n| artist_key(n) == target)
        })
        .collect();
    if artist_tracks.is_empty() {
        return not_found("Künstler nicht gefunden.");
    }
    let display_name = artist_tracks
        .iter()
        .find_map(|t| {
            split_artists(&t.artists)
                .into_iter()
                .find(|n| artist_key(n) == target)
        })
        .unwrap_or_else(|| name.clone());
    let artist_ids: Vec<i64> = artist_tracks.iter().map(|t| t.id).collect();

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
    let mut tag_groups: Vec<GroupDist> = Vec::new();
    let mut top_tags: Vec<TagCount> = Vec::new();
    let mut artist_tag_ids: HashSet<i64> = HashSet::new();
    for chunk in artist_ids.chunks(CHUNK) {
        let mut qb = QueryBuilder::new(
            "SELECT g.name AS name, g.icon AS icon, COUNT(DISTINCT rt.track_id) AS tracks
               FROM hub_track_resolved_tags rt
               JOIN hub_group_tags gt ON gt.tag_id = rt.tag_id
               JOIN hub_tag_groups g ON g.id = gt.group_id
              WHERE rt.track_id IN ",
        );
        bind_ids(&mut qb, chunk);
        qb.push(" GROUP BY g.id ORDER BY tracks DESC, g.name");
        tag_groups.extend(
            qb.build()
                .fetch_all(&st.pool)
                .await
                .unwrap_or_default()
                .iter()
                .map(|r| GroupDist {
                    name: r.get::<Option<String>, _>("name").unwrap_or_default(),
                    icon: r.get::<Option<String>, _>("icon").unwrap_or_default(),
                    tracks: r.get::<Option<i64>, _>("tracks").unwrap_or(0),
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
    // key -> (name, tracks, plays, track ids)
    let mut collab: HashMap<String, (String, i64, i64, HashSet<i64>)> = HashMap::new();
    for t in &artist_tracks {
        let toks = split_artists(&t.artists);
        if toks.len() < 2 {
            continue;
        }
        let plays: i64 = plays_by_track
            .get(&t.id)
            .map(|v| v.iter().map(|(_, p)| *p).sum())
            .unwrap_or(0);
        for tok in &toks {
            if artist_key(tok) == target {
                continue;
            }
            let e = collab
                .entry(artist_key(tok))
                .or_insert((tok.clone(), 0, 0, HashSet::new()));
            e.1 += 1;
            e.2 += plays;
            e.3.insert(t.id);
        }
    }
    let mut collabs: Vec<CollabRow> = collab
        .values()
        .map(|(name, t, p, _)| CollabRow {
            href: artist_href(name),
            name: name.clone(),
            tracks: *t,
            plays: *p,
        })
        .collect();
    collabs.sort_by(|a, b| b.tracks.cmp(&a.tracks).then_with(|| b.plays.cmp(&a.plays)));

    // ── AR5b: b2b sets (Traktor playlists named *b2b*) ───────────────────────
    let mut b2b: Vec<B2bRow> = Vec::new();
    // partner key -> (set count, track ids, display name) for the AR6 candidate pool.
    let mut b2b_pairs: HashMap<String, (i64, HashSet<i64>, String)> = HashMap::new();
    for chunk in artist_ids.chunks(CHUNK) {
        let mut qb = QueryBuilder::new(
            "SELECT p.user_id AS user_id, p.id AS pid, p.name AS pname,
                    u.slug AS slug, t.id AS track_id, t.artists AS artists
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
            for tok in split_artists(&r.get::<Option<String>, _>("artists").unwrap_or_default()) {
                let k = artist_key(&tok);
                if k == target {
                    entry.2 += 1;
                } else {
                    entry.3.insert(tok.clone());
                    let bp = b2b_pairs
                        .entry(k)
                        .or_insert((0, HashSet::new(), tok.clone()));
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
        let mut qb = QueryBuilder::new("SELECT album, artists FROM hub_tracks WHERE album IN (");
        {
            let mut sep = qb.separated(", ");
            for a in chunk {
                sep.push_bind(a.clone());
            }
        }
        qb.push(")");
        for r in qb.build().fetch_all(&st.pool).await.unwrap_or_default() {
            let album: String = r.get::<Option<String>, _>("album").unwrap_or_default();
            for tok in split_artists(&r.get::<Option<String>, _>("artists").unwrap_or_default()) {
                if artist_key(&tok) != target {
                    album_others.entry(album.clone()).or_default().insert(tok);
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
    // shared-tag candidates → candidate -> (name, shared tag ids, track ids)
    let mut cand: HashMap<String, (String, HashSet<i64>, HashSet<i64>)> = HashMap::new();
    let tag_id_list: Vec<i64> = artist_tag_ids.iter().copied().collect();
    for chunk in tag_id_list.chunks(CHUNK) {
        let mut qb = QueryBuilder::new(
            "SELECT rt.tag_id AS tag_id, rt.track_id AS track_id, t.artists AS artists
               FROM hub_track_resolved_tags rt
               JOIN hub_tracks t ON t.id = rt.track_id
              WHERE rt.tag_id IN ",
        );
        bind_ids(&mut qb, chunk);
        for r in qb.build().fetch_all(&st.pool).await.unwrap_or_default() {
            let tag_id: i64 = r.get("tag_id");
            let track_id: i64 = r.get("track_id");
            for tok in split_artists(&r.get::<Option<String>, _>("artists").unwrap_or_default()) {
                let k = artist_key(&tok);
                if k == target {
                    continue;
                }
                let e = cand
                    .entry(k)
                    .or_insert_with(|| (tok.clone(), HashSet::new(), HashSet::new()));
                e.1.insert(tag_id);
                e.2.insert(track_id);
            }
        }
    }
    // collaboration partners feed the same candidate pool.
    for (key, (name, _, _, tids)) in &collab {
        let e = cand
            .entry(key.clone())
            .or_insert_with(|| (name.clone(), HashSet::new(), HashSet::new()));
        e.2.extend(tids.iter().copied());
    }
    // b2b co-artists feed the same pool.
    for (key, (_, tids, name)) in &b2b_pairs {
        let e = cand
            .entry(key.clone())
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
    let cand_users: HashMap<String, HashSet<i64>> = cand
        .iter()
        .map(|(k, (_, _, tids))| {
            let mut u: HashSet<i64> = HashSet::new();
            for tid in tids {
                if let Some(set) = track_users.get(tid) {
                    u.extend(set.iter().copied());
                }
            }
            (k.clone(), u)
        })
        .collect();

    // Deterministic score over shared tags + collaborations + b2b.
    let mut scored: Vec<(String, String, f64, i64, i64, i64)> = cand
        .values()
        .map(|(name, tags, _)| {
            let key = artist_key(name);
            let shared = tags.len() as i64;
            let c = collab.get(&key).map(|(_, t, _, _)| *t).unwrap_or(0);
            let b = b2b_pairs.get(&key).map(|(cnt, _, _)| *cnt).unwrap_or(0);
            let score = REF_TAG_W * shared as f64 + REF_COLLAB_W * c as f64 + REF_B2B_W * b as f64;
            (key, name.clone(), score, shared, c, b)
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
            href: artist_href(&row.1),
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
        name: display_name,
        track_count: artist_ids.len() as i64,
        total_plays,
        user_count,
        spotify: spotify_href(&name),
        self_href: artist_href(&name),
        has_me,
        my_plays,
        others,
        has_tags,
        tag_groups,
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
    fn hrefs_are_encoded() {
        assert_eq!(artist_href("A B"), "/artist/A%20B");
        assert!(spotify_href("A/B").starts_with("https://open.spotify.com/search/"));
    }
}
