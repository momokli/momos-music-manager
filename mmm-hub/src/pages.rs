//! Browse pages: search, user profiles, and playlist detail.
//!
//! Every page requires a session and links into the track detail page
//! (`/track/{id}`). Server-rendered with askama, styled by Pico.css via
//! `base.html`.

use std::collections::{HashMap, HashSet};

use askama::Template;
use axum::extract::{Form, Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
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
        .route("/tag/{id}", get(tag_detail_page))
        .route("/tag/{id}/rename", post(tag_rename))
        .route("/tag/{id}/group/add", post(tag_group_add))
        .route("/tag/{id}/group/remove", post(tag_group_remove))
        .route("/groups", get(groups_page))
        .route("/groups/create", post(group_create))
        .route("/groups/{id}", get(group_page))
        .route("/groups/{id}/update", post(group_update))
        .route("/groups/{id}/subscribe", post(group_subscribe))
        .route("/groups/{id}/unsubscribe", post(group_unsubscribe))
        .route("/groups/{id}/role", post(group_set_role))
        .route("/groups/{id}/member/remove", post(group_remove_member))
        .route("/groups/{id}/collective", post(group_set_collective))
        .route("/groups/{id}/delete", post(group_delete))
        .route("/collectives", get(collectives_page))
        .route("/collectives/create", post(collective_create))
        .route("/collectives/{id}", get(collective_page))
        .route("/collectives/{id}/update", post(collective_update))
        .route("/collectives/{id}/join", post(collective_join))
        .route("/collectives/{id}/leave", post(collective_leave))
        .route("/collectives/{id}/member", post(collective_set_member))
        .route("/collectives/{id}/member/remove", post(collective_remove_member))
        .route("/collectives/{id}/delete", post(collective_delete))
        .route("/digging", get(digging_page))
        .route("/digging/enrich", post(digging_enrich))
        .route("/admin", get(admin_page).post(admin_save))
        .route("/settings", get(settings_page))
        .route("/user/{slug}", get(user_page))
        .route("/playlist/{id}", get(playlist_page))
        .route("/playlist/{id}/tag", post(playlist_tag))
        .route("/playlist/{id}/tag/add", post(playlist_tag_add))
        .route("/playlist/{id}/tag/remove", post(playlist_tag_remove))
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

// ── search ──────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "search.html")]
struct SearchPage {
    nav: crate::ui::Nav,
    flash: String,
    q: String,
    tracks: Vec<SearchRow>,
}

struct SearchRow {
    id: i64,
    title: String,
    artists: String,
    album: String,
}

#[derive(Deserialize, Default)]
struct SearchQuery {
    q: Option<String>,
}

async fn search_page(
    State(st): State<AppState>,
    Query(params): Query<SearchQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "search").await else {
        return Redirect::to("/login").into_response();
    };

    let q = params.q.unwrap_or_default().trim().to_string();
    let mut tracks = Vec::new();

    if !q.is_empty() {
        let rows = sqlx::query(
            "SELECT id, title, artists, album
               FROM hub_tracks
              WHERE lower(title || ' ' || artists || ' ' || album) LIKE '%' || lower(?1) || '%'
              ORDER BY artists, title
              LIMIT 100",
        )
        .bind(&q)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();

        for r in rows {
            tracks.push(SearchRow {
                id: r.get("id"),
                title: r.get::<Option<String>, _>("title").unwrap_or_default(),
                artists: r.get::<Option<String>, _>("artists").unwrap_or_default(),
                album: r.get::<Option<String>, _>("album").unwrap_or_default(),
            });
        }
    }

    render(&SearchPage {
        nav,
        flash: String::new(),
        q,
        tracks,
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
    playlists: Vec<PlaylistRow>,
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

    let liked_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_liked_tracks WHERE user_id = ?1")
        .bind(user_id)
        .fetch_one(&st.pool)
        .await
        .unwrap_or(0);

    let rows = sqlx::query(
        "SELECT p.id, p.name,
                COALESCE(NULLIF(p.owner_name, ''), CASE WHEN p.is_owned = 1 THEN a.display_name END) AS owner_name,
                p.track_count, p.items_available
           FROM hub_playlists p
           LEFT JOIN hub_service_accounts a ON a.user_id = p.user_id AND a.service = 'spotify'
          WHERE p.user_id = ?1
          ORDER BY p.name",
    )
    .bind(user_id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    let playlists: Vec<PlaylistRow> = rows
        .iter()
        .map(|r| PlaylistRow {
            id: r.get("id"),
            name: r.get::<Option<String>, _>("name").unwrap_or_default(),
            owner: r
                .get::<Option<String>, _>("owner_name")
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

    render(&UserPage {
        nav,
        flash: String::new(),
        slug,
        liked_count,
        playlist_count,
        playlists,
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
    tracks: Vec<PlaylistTrackRow>,
    /// Tags this playlist currently feeds.
    feeds: Vec<TagFeed>,
    /// The current user's tags, for the picker.
    my_tags: Vec<TagOption>,
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
    artists: String,
}

async fn playlist_page(
    State(st): State<AppState>,
    Path(id): Path<i64>,
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

    let rows = sqlx::query(
        "SELECT t.id, hpt.position, t.title, t.artists
           FROM hub_playlist_tracks hpt
           JOIN hub_tracks t ON t.id = hpt.track_id
          WHERE hpt.playlist_id = ?1
          ORDER BY hpt.position",
    )
    .bind(id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    let tracks: Vec<PlaylistTrackRow> = rows
        .iter()
        .map(|r| PlaylistTrackRow {
            position: r
                .get::<Option<i64>, _>("position")
                .map(|p| p.to_string())
                .unwrap_or_else(|| "—".to_string()),
            id: r.get("id"),
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            artists: r.get::<Option<String>, _>("artists").unwrap_or_default(),
        })
        .collect();

    render(&PlaylistPage {
        nav,
        flash: String::new(),
        id,
        name: name.unwrap_or_default(),
        owner: owner.unwrap_or_default(),
        owner_name: owner_name.filter(|s| !s.is_empty()).unwrap_or_default(),
        tracks,
        feeds,
        my_tags,
    })
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
    let _ = crate::tags::create_from_playlist(&st.pool, user_id, id).await;
    Redirect::to(&format!("/playlist/{id}")).into_response()
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
            if desc {
                o.reverse()
            } else {
                o
            }
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
    has_audio: bool,
    /// Sort headers for the table columns.
    sort_present: SortLink,
    sort_bpm: SortLink,
    sort_key: SortLink,
    sort_energy: SortLink,
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
    isrc: String,
    bpm: String,
    key: String,
    energy: String,
    bpm_num: Option<f64>,
    energy_num: Option<f64>,
    present_count: i64,
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
    let q = q.to_lowercase();
    title.to_lowercase().contains(&q) || artists.to_lowercase().contains(&q)
}

/// Ids from `candidate_ids` that resolve to a tag whose name contains `needle`
/// (case-insensitive, any owner). Chunked for the bind-variable limit.
async fn tagged_track_ids(
    pool: &sqlx::SqlitePool,
    candidate_ids: &[i64],
    needle: &str,
) -> HashSet<i64> {
    let mut set = HashSet::new();
    let like = format!("%{}%", needle.to_lowercase());
    for chunk in candidate_ids.chunks(900) {
        let mut qb = QueryBuilder::new(
            "SELECT DISTINCT rt.track_id AS tid
               FROM hub_track_resolved_tags rt
               JOIN hub_tags t ON t.id = rt.tag_id
              WHERE lower(t.name) LIKE ",
        );
        qb.push_bind(&like);
        qb.push(" AND rt.track_id IN (");
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        qb.push(")");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            set.insert(r.get::<i64, _>("tid"));
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

    let all_users = sqlx::query_as::<_, (i64, String)>(
        "SELECT id, slug FROM hub_users ORDER BY slug",
    )
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
    let key_harmonic = matches!(q.key_harmonic.as_deref(), Some("1") | Some("on") | Some("true"));
    let bpm_min = bpm_min_raw.parse::<f64>().ok();
    let bpm_max = bpm_max_raw.parse::<f64>().ok();
    let q_text = q.q.clone().unwrap_or_default().trim().to_string();
    let tag = q.tag.clone().unwrap_or_default().trim().to_string();

    // Sort field + direction.
    let sort = match q.sort.as_deref() {
        Some("bpm") => "bpm",
        Some("key") => "key",
        Some("energy") => "energy",
        _ => "present",
    };
    let dir = match q.dir.as_deref() {
        Some("asc") => "asc",
        Some("desc") => "desc",
        _ if sort == "present" => "desc",
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

    // Assemble rows: keep tracks present for >= 2 of the selected users.
    let mut rows: Vec<CompareRow> = Vec::new();
    let mut pair_counts: HashMap<(i64, i64), i64> = HashMap::new();
    for (tid, t) in acc {
        let (bpm_opt, camelot, energy_opt) = audio
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
            bpm_num: bpm_opt,
            energy_num: energy_opt,
            present_count: present.len() as i64,
            cells,
        });
    }

    // Sort by the chosen field; tracks without the value always go last.
    let desc = dir == "desc";
    rows.sort_by(|a, b| {
        let ord = match sort {
            "bpm" => opt_cmp(a.bpm_num, b.bpm_num, desc),
            "energy" => opt_cmp(a.energy_num, b.energy_num, desc),
            "key" => match (camelot_rank(&a.key), camelot_rank(&b.key)) {
                (Some(x), Some(y)) => {
                    let o = x.cmp(&y);
                    if desc {
                        o.reverse()
                    } else {
                        o
                    }
                }
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            },
            _ => {
                let o = a.present_count.cmp(&b.present_count);
                if desc {
                    o.reverse()
                } else {
                    o
                }
            },
        };
        ord.then_with(|| a.artists.cmp(&b.artists))
            .then_with(|| a.title.cmp(&b.title))
    });
    rows.truncate(1000);

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
        let next_dir = if active && dir == "asc" { "desc" } else { "asc" };
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
        has_audio,
        sort_present: sort_link("present"),
        sort_bpm: sort_link("bpm"),
        sort_key: sort_link("key"),
        sort_energy: sort_link("energy"),
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
    let get = |k: &str| p.get(k).and_then(|v| v.first()).cloned().unwrap_or_default();

    let all_users = sqlx::query_as::<_, (i64, String)>("SELECT id, slug FROM hub_users ORDER BY slug")
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
}

#[derive(Template)]
#[template(path = "similar.html")]
struct SimilarPage {
    nav: crate::ui::Nav,
    flash: String,
    users: Vec<ColumnUser>,
    rows: Vec<SimilarRow>,
    scopes: Vec<ScopeLink>,
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
                by_spotify.entry(e.spotify_id.as_str()).or_default().insert(e.uid);
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

    render(&SimilarPage {
        nav,
        flash: String::new(),
        users: users
            .iter()
            .map(|(_, slug)| ColumnUser { slug: slug.clone() })
            .collect(),
        rows: out,
        scopes,
    })
}

// ── digging ────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct DiggingQuery {
    seed: Option<i64>,
    mine: Option<String>,
    bpm: Option<String>,
    harm: Option<String>,
    tol: Option<f64>,
}

#[derive(Template)]
#[template(path = "digging.html")]
struct DiggingPage {
    nav: crate::ui::Nav,
    flash: String,
    has_seed: bool,
    seed_id: i64,
    seed_title: String,
    seed_artists: String,
    rows: Vec<DigRow>,
    lastfm_enabled: bool,
    freqblog_enabled: bool,
    freqblog_remaining: i64,
    seed_bpm: String,
    seed_key: String,
    filters: Vec<ScopeLink>,
    bpm_opts: Vec<ScopeLink>,
}

struct DigRow {
    track_id: i64,
    matched: bool,
    title: String,
    artists: String,
    sources: Vec<String>,
    users: i64,
    playlists: i64,
    likes: i64,
    bpm: Option<f64>,
    bpm_disp: String,
    camelot: String,
    score: i64,
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
    name: String,
    owner: String,
    groups: Vec<DigGroup>,
}

#[derive(Clone)]
struct DigGroup {
    id: i64,
    icon: String,
    name: String,
}

/// Merge a (possibly external) candidate into the session map: dedupe by hub
/// track id when matched, else by normalised artist|title.
async fn merge_candidate(
    cand: &mut HashMap<String, DigRow>,
    pool: &sqlx::SqlitePool,
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
    let (matched, p) = match tid {
        Some(id) => (true, crate::digging::presence(pool, id).await),
        None => (false, crate::digging::Presence::default()),
    };
    cand.insert(
        key,
        DigRow {
            track_id: tid.unwrap_or(0),
            matched,
            title: title.to_string(),
            artists: artists.to_string(),
            sources: vec![source.to_string()],
            users: p.users,
            playlists: p.playlists,
            likes: p.likes,
            bpm: None,
            bpm_disp: String::new(),
            camelot: String::new(),
            score: 0,
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

        // Resolved tags + their groups (LEFT JOIN, so a tag can repeat per group).
        let mut qb = QueryBuilder::new(
            "SELECT rt.track_id AS tid, t.name AS tag, u.slug AS owner,
                    g.id AS gid, g.name AS gname, COALESCE(g.icon, '') AS gicon
               FROM hub_track_resolved_tags rt
               JOIN hub_tags t ON t.id = rt.tag_id
               JOIN hub_users u ON u.id = t.owner_user_id
               LEFT JOIN hub_group_tags gt ON gt.tag_id = t.id
               LEFT JOIN hub_tag_groups g ON g.id = gt.group_id
              WHERE rt.track_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        qb.push(") ORDER BY u.slug, t.name COLLATE NOCASE, g.name COLLATE NOCASE");
        for r in qb.build().fetch_all(pool).await.unwrap_or_default() {
            let tid: i64 = r.get("tid");
            let tag: String = r.get::<Option<String>, _>("tag").unwrap_or_default();
            let owner: String = r.get::<Option<String>, _>("owner").unwrap_or_default();
            let gid: Option<i64> = r.get("gid");
            let gname: String = r.get::<Option<String>, _>("gname").unwrap_or_default();
            let gicon: String = r.get::<Option<String>, _>("gicon").unwrap_or_default();
            let list = tags.entry(tid).or_default();
            let t = match list.iter().position(|x| x.name == tag && x.owner == owner) {
                Some(i) => &mut list[i],
                None => {
                    list.push(DigTag {
                        name: tag,
                        owner,
                        groups: Vec::new(),
                    });
                    list.last_mut().unwrap()
                }
            };
            if gid.is_some() && !t.groups.iter().any(|g| g.name == gname) {
                t.groups.push(DigGroup {
                    id: gid.unwrap_or(0),
                    icon: gicon,
                    name: gname,
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
    let freqblog_remaining = if freqblog_enabled {
        crate::freqblog::remaining(&st.pool, &st.cfg).await
    } else {
        0
    };

    let mut has_seed = false;
    let (mut seed_id, mut seed_title, mut seed_artists) = (0i64, String::new(), String::new());
    let mut cand: HashMap<String, DigRow> = HashMap::new();

    if let Some(sid) = q.seed {
        if let Ok(Some(seed)) = crate::digging::load_seed(&st.pool, sid).await {
            has_seed = true;
            seed_id = seed.id;
            seed_title = seed.title.clone();
            seed_artists = seed.artists.clone();

            // 1. Hub-intern: tracks sharing the seed's playlists (capped so the
            //    external discoveries below still fit the table).
            for s in crate::digging::internal_suggestions(&st.pool, sid, 100)
                .await
                .unwrap_or_default()
            {
                merge_candidate(
                    &mut cand,
                    &st.pool,
                    Some(s.id),
                    &s.title,
                    &s.artists,
                    "Hub-intern",
                )
                .await;
            }

            // 2. ReccoBeats recommendations (free, no auth).
            let seed_spotify = sqlx::query_scalar::<_, String>(
                "SELECT external_id FROM hub_track_external_ids
                  WHERE track_id = ?1 AND service = 'spotify' LIMIT 1",
            )
            .bind(sid)
            .fetch_optional(&st.pool)
            .await
            .ok()
            .flatten();
            if let Some(sp) = &seed_spotify {
                for r in crate::features::recommendations(&st.cfg, sp, 50)
                    .await
                    .unwrap_or_default()
                {
                    let tid = crate::digging::match_track(
                        &st.pool,
                        Some(&r.spotify_id),
                        None,
                        &r.artists,
                        &r.title,
                    )
                    .await;
                    merge_candidate(&mut cand, &st.pool, tid, &r.title, &r.artists, "ReccoBeats")
                        .await;
                }
            }

            // 3. Last.fm similar (needs LASTFM_API_KEY).
            if lastfm_enabled {
                for s in crate::lastfm::similar_tracks(&st.cfg, &seed.artists, &seed.title)
                    .await
                    .unwrap_or_default()
                {
                    let tid =
                        crate::digging::match_track(&st.pool, None, None, &s.artist, &s.name).await;
                    merge_candidate(&mut cand, &st.pool, tid, &s.name, &s.artist, "Last.fm").await;
                }
            }

            // 4. cosine.club similar (free API key): audio-similarity over 2M+
            //    underground tracks — the breadth our own catalog lacks.
            if crate::cosine::enabled(&st.cfg) {
                let seed_artist = seed.artists.split(',').next().unwrap_or("").trim();
                for s in crate::cosine::similar_tracks(&st.cfg, seed_artist, &seed.title, 60)
                    .await
                    .unwrap_or_default()
                {
                    let tid =
                        crate::digging::match_track(&st.pool, None, None, &s.artist, &s.track).await;
                    merge_candidate(&mut cand, &st.pool, tid, &s.track, &s.artist, "cosine.club")
                        .await;
                }
            }

            // 5. Audio-ähnlich: EffNet-Embedding-Nachbarn aus unserer eigenen DB.
            for n in crate::similar::neighbors(&st.pool, sid, 60)
                .await
                .unwrap_or_default()
            {
                if let Ok(Some(r)) = sqlx::query("SELECT title, artists FROM hub_tracks WHERE id = ?1")
                    .bind(n.track_id)
                    .fetch_optional(&st.pool)
                    .await
                {
                    let title = r.get::<Option<String>, _>("title").unwrap_or_default();
                    let artists = r.get::<Option<String>, _>("artists").unwrap_or_default();
                    merge_candidate(&mut cand, &st.pool, Some(n.track_id), &title, &artists, "Audio")
                        .await;
                }
            }
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
        true
    });

    for r in rows.iter_mut() {
        r.score = r.users * 10 + r.playlists * 3 + r.likes * 2 + r.sources.len() as i64;
        r.bpm_disp = r
            .bpm
            .map(|b| format!("{b:.0}"))
            .unwrap_or_else(|| "—".to_string());
    }
    rows.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.artists.cmp(&b.artists))
            .then_with(|| a.title.cmp(&b.title))
    });
    let with_hub = rows.iter().filter(|r| r.users > 0 || r.likes > 0).count();
    rows.truncate(500);

    // Attach per-user playlists + tags (track-detail style) for the shown rows.
    let matched_ids: Vec<i64> = rows.iter().filter(|r| r.matched).map(|r| r.track_id).collect();
    let (presence_map, tags_map) = dig_details(&st.pool, &matched_ids).await;
    for r in rows.iter_mut() {
        if r.matched {
            r.presence = presence_map.get(&r.track_id).cloned().unwrap_or_default();
            r.tags = tags_map.get(&r.track_id).cloned().unwrap_or_default();
        }
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

    render(&DiggingPage {
        nav,
        flash: String::new(),
        has_seed,
        seed_id,
        seed_title,
        seed_artists,
        rows,
        lastfm_enabled,
        freqblog_enabled,
        freqblog_remaining,
        seed_bpm: seed_bpm.map(|b| format!("{b:.0}")).unwrap_or_default(),
        seed_key: seed_camelot,
        filters,
        bpm_opts,
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

// ── admin (settings) ────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct AdminMsg {
    msg: Option<String>,
}

#[derive(Template)]
#[template(path = "admin.html")]
struct AdminPage {
    nav: crate::ui::Nav,
    flash: String,
    fields: Vec<AdminField>,
    registration_open: bool,
    users: Vec<AdminUser>,
}

struct AdminField {
    key: String,
    label: String,
    secret: bool,
    set: bool,
    value: String,
}

struct AdminUser {
    slug: String,
    is_admin: bool,
}

async fn admin_page(
    State(st): State<AppState>,
    Query(msg): Query<AdminMsg>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "admin").await else {
        return Redirect::to("/login").into_response();
    };
    if !nav.is_admin {
        return (StatusCode::FORBIDDEN, "Nur Admins.").into_response();
    }

    let s = crate::settings::load_all(&st.pool).await;
    let fields = crate::settings::ADMIN_FIELDS
        .iter()
        .map(|(key, label, secret)| {
            let v = s.get(*key).cloned().unwrap_or_default();
            AdminField {
                key: (*key).to_string(),
                label: (*label).to_string(),
                secret: *secret,
                set: !v.is_empty(),
                value: if *secret { String::new() } else { v },
            }
        })
        .collect();
    let registration_open = crate::settings::registration_open(&st.pool).await;
    let users = sqlx::query_as::<_, (String, i64)>("SELECT slug, is_admin FROM hub_users ORDER BY slug")
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(slug, admin)| AdminUser {
            slug,
            is_admin: admin == 1,
        })
        .collect();

    render(&AdminPage {
        nav,
        flash: msg.msg.unwrap_or_default(),
        fields,
        registration_open,
        users,
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

    // Non-empty values are stored; empty means "leave unchanged".
    for (key, _label, _secret) in crate::settings::ADMIN_FIELDS {
        if let Some(v) = map.get(*key) {
            let v = v.trim();
            if !v.is_empty() {
                let _ = crate::settings::set(&st.pool, key, v).await;
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

    Redirect::to("/admin?msg=Gespeichert%20%E2%80%94%20Keys%20greifen%20nach%20Neustart").into_response()
}

// ── tags (resolved playlist layer) ──────────────────────────────────────────

#[derive(Deserialize, Default)]
struct TagFilter {
    q: Option<String>,
    /// "1" = only my tags.
    mine: Option<String>,
    /// Filter to tags in this group.
    group: Option<i64>,
    /// Filter to tags in any group of this collective.
    collective: Option<i64>,
    /// Filter to tags owned by this user slug.
    owner: Option<String>,
}

#[derive(Template)]
#[template(path = "tags.html")]
struct TagsPage {
    nav: crate::ui::Nav,
    flash: String,
    q: String,
    mine: bool,
    group_id: i64,
    group_label: String,
    collective_id: i64,
    owner: String,
    owner_options: Vec<OwnerOption>,
    collective_options: Vec<ParentOption>,
    tags: Vec<TagRow>,
}

/// A lightweight group reference for templates.
struct GroupRef {
    id: i64,
    name: String,
    icon: String,
}

struct TagRow {
    id: i64,
    name: String,
    owner: String,
    groups: String,
    track_count: i64,
    source_count: i64,
}

async fn tags_page(
    State(st): State<AppState>,
    Query(f): Query<TagFilter>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tags").await else {
        return Redirect::to("/login").into_response();
    };
    let q = f.q.unwrap_or_default().trim().to_string();
    let mine = f.mine.as_deref() == Some("1");
    let me = nav.id;
    let (group_id, group_label) = match f.group {
        Some(gid) => match crate::tags::group_detail(&st.pool, me, gid).await {
            Some(g) => (g.id, format!("{} {}", g.icon, g.name).trim().to_string()),
            None => (0, String::new()),
        },
        None => (0, String::new()),
    };
    let owner = f.owner.unwrap_or_default().trim().to_string();
    let collective_id = f.collective.unwrap_or(0);

    let rows = sqlx::query(
        "SELECT t.id, t.name, u.slug AS owner,
                (SELECT GROUP_CONCAT(g.icon || ' ' || g.name, ' · ')
                   FROM hub_group_tags gt JOIN hub_tag_groups g ON g.id = gt.group_id
                  WHERE gt.tag_id = t.id) AS groups,
                (SELECT COUNT(*) FROM hub_track_resolved_tags r WHERE r.tag_id = t.id) AS track_count,
                (SELECT COUNT(*) FROM hub_tag_sources s WHERE s.tag_id = t.id) AS source_count
           FROM hub_tags t
           JOIN hub_users u ON u.id = t.owner_user_id
          WHERE (?1 = '' OR lower(t.name) LIKE '%' || lower(?1) || '%')
            AND (?2 = 0 OR t.owner_user_id = ?3)
            AND (?4 = 0 OR EXISTS (SELECT 1 FROM hub_group_tags gt2
                                    WHERE gt2.tag_id = t.id AND gt2.group_id = ?4))
            AND (?5 = '' OR u.slug = ?5 COLLATE NOCASE)
            AND (?6 = 0 OR EXISTS (SELECT 1 FROM hub_group_tags gt3
                                    JOIN hub_tag_groups g3 ON g3.id = gt3.group_id
                                   WHERE gt3.tag_id = t.id AND g3.collective_id = ?6))
          ORDER BY u.slug, t.name
          LIMIT 1000",
    )
    .bind(&q)
    .bind(mine as i64)
    .bind(me)
    .bind(f.group.unwrap_or(0))
    .bind(&owner)
    .bind(collective_id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    let tags: Vec<TagRow> = rows
        .iter()
        .map(|r| TagRow {
            id: r.get::<i64, _>("id"),
            name: r.get::<Option<String>, _>("name").unwrap_or_default(),
            owner: r.get::<Option<String>, _>("owner").unwrap_or_default(),
            groups: r.get::<Option<String>, _>("groups").unwrap_or_default(),
            track_count: r.get::<Option<i64>, _>("track_count").unwrap_or(0),
            source_count: r.get::<Option<i64>, _>("source_count").unwrap_or(0),
        })
        .collect();

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
    render(&TagsPage {
        nav,
        flash: String::new(),
        q,
        mine,
        group_id,
        group_label,
        collective_id,
        owner,
        owner_options,
        collective_options,
        tags,
    })
}

// ── tag detail ───────────────────────────────────────────────────────────────

struct TagTrack {
    id: i64,
    title: String,
    artists: String,
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
    groups: Vec<GroupRef>,
    my_groups: Vec<GroupRef>,
    source_count: i64,
    tracks: Vec<TagTrack>,
}

async fn tag_detail_page(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Query(msg): Query<AdminMsg>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "tags").await else {
        return Redirect::to("/login").into_response();
    };
    let Some(d) = crate::tags::tag_detail(&st.pool, id).await else {
        return not_found("Tag nicht gefunden.");
    };
    let groups: Vec<GroupRef> = d
        .groups
        .into_iter()
        .map(|(id, name, icon)| GroupRef { id, name, icon })
        .collect();
    let my_groups: Vec<GroupRef> = crate::tags::groups_i_contribute(&st.pool, nav.id)
        .await
        .into_iter()
        .map(|(id, name, icon)| GroupRef { id, name, icon })
        .collect();
    let tracks = d
        .tracks
        .into_iter()
        .map(|(id, title, artists)| TagTrack {
            id,
            title,
            artists,
        })
        .collect();
    let is_owner = d.owner.eq_ignore_ascii_case(&nav.slug);
    render(&TagDetailPage {
        nav,
        flash: msg.msg.unwrap_or_default(),
        id: d.id,
        name: d.name,
        owner: d.owner,
        is_owner,
        groups,
        my_groups,
        source_count: d.source_count,
        tracks,
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
struct GroupIdForm {
    group_id: i64,
}

async fn tag_group_add(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<GroupIdForm>,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tags::add_tag_to_group(&st.pool, uid, id, f.group_id).await;
    Redirect::to(&format!("/tag/{id}")).into_response()
}

async fn tag_group_remove(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<GroupIdForm>,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tags::remove_tag_from_group(&st.pool, uid, id, f.group_id).await;
    Redirect::to(&format!("/tag/{id}")).into_response()
}

// ── groups ───────────────────────────────────────────────────────────────────

struct GroupRow {
    id: i64,
    name: String,
    icon: String,
    owner: String,
    role: String,
    inherited: bool,
    tag_count: i64,
    members: i64,
}

fn group_row(g: crate::tags::Group) -> GroupRow {
    GroupRow {
        id: g.id,
        name: g.name,
        icon: g.icon,
        owner: g.owner,
        role: g.role,
        inherited: g.inherited,
        tag_count: g.tag_count,
        members: g.members,
    }
}

#[derive(Template)]
#[template(path = "groups.html")]
struct GroupsPage {
    nav: crate::ui::Nav,
    flash: String,
    groups: Vec<GroupRow>,
    discover: Vec<GroupRow>,
    icons: &'static [&'static str],
    collective_options: Vec<GroupRef>,
}

async fn groups_page(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "groups").await else {
        return Redirect::to("/login").into_response();
    };
    let groups = crate::tags::list_groups_for(&st.pool, nav.id)
        .await
        .into_iter()
        .map(group_row)
        .collect();
    let discover = crate::tags::list_discover_groups(&st.pool, nav.id)
        .await
        .into_iter()
        .map(group_row)
        .collect();
    let collective_options: Vec<GroupRef> = crate::tags::collectives_i_belong(&st.pool, nav.id)
        .await
        .into_iter()
        .map(|(id, name, icon)| GroupRef { id, name, icon })
        .collect();
    render(&GroupsPage {
        nav,
        flash: String::new(),
        groups,
        discover,
        icons: crate::tags::ICONS,
        collective_options,
    })
}

#[derive(Deserialize)]
struct GroupForm {
    name: String,
    icon: Option<String>,
    collective_id: Option<String>,
}

async fn group_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<GroupForm>,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    if let Ok(gid) =
        crate::tags::create_group(&st.pool, uid, &f.name, f.icon.as_deref().unwrap_or("")).await
    {
        let cid = f
            .collective_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| s.parse::<i64>().ok());
        if let Some(cid) = cid {
            let _ = crate::tags::set_group_collective(&st.pool, uid, gid, Some(cid)).await;
        }
    }
    Redirect::to("/groups").into_response()
}

async fn group_subscribe(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tags::subscribe(&st.pool, uid, id).await;
    Redirect::to("/groups").into_response()
}

async fn group_unsubscribe(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tags::unsubscribe(&st.pool, uid, id).await;
    Redirect::to("/groups").into_response()
}

#[derive(Deserialize)]
struct RoleForm {
    slug: String,
    role: String,
}

async fn group_set_role(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<RoleForm>,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tags::set_member_role(&st.pool, uid, id, &f.slug, &f.role).await;
    Redirect::to(&format!("/groups/{id}")).into_response()
}

#[derive(Deserialize)]
struct SlugForm {
    slug: String,
}

async fn group_remove_member(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<SlugForm>,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tags::remove_member(&st.pool, uid, id, &f.slug).await;
    Redirect::to(&format!("/groups/{id}")).into_response()
}

async fn group_delete(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = crate::tags::delete_group(&st.pool, uid, id).await;
    Redirect::to("/groups").into_response()
}

async fn group_update(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<UpdateNameForm>,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let icon = f.icon.as_deref().unwrap_or("").trim().to_string();
    let msg = match crate::tags::update_group(&st.pool, uid, id, &f.name, &icon).await {
        Ok(()) => "Gruppe gespeichert".to_string(),
        Err(e) => format!("Fehler: {e}"),
    };
    flash_redirect(&format!("/groups/{id}"), msg)
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

struct MemberRow {
    slug: String,
    role: String,
}

struct GroupTag {
    id: i64,
    name: String,
    owner: String,
}

#[derive(Template)]
#[template(path = "group.html")]
struct GroupPage {
    nav: crate::ui::Nav,
    flash: String,
    id: i64,
    name: String,
    icon: String,
    owner: String,
    is_owner: bool,
    can_edit: bool,
    role_inherited: bool,
    collective_id: i64,
    collective_label: String,
    collective_options: Vec<ParentOption>,
    members: Vec<MemberRow>,
    tags: Vec<GroupTag>,
    users: Vec<String>,
    icons: Vec<IconOption>,
}

async fn group_page(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Query(msg): Query<AdminMsg>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "groups").await else {
        return Redirect::to("/login").into_response();
    };
    let Some(d) = crate::tags::group_detail(&st.pool, nav.id, id).await else {
        return not_found("Gruppe nicht gefunden.");
    };
    let is_owner = d.role == crate::tags::ROLE_OWNER;
    let can_edit = matches!(
        d.role.as_str(),
        crate::tags::ROLE_OWNER | crate::tags::ROLE_CONTRIBUTOR
    );
    let members = d
        .members
        .into_iter()
        .map(|(slug, role)| MemberRow { slug, role })
        .collect();
    let tags = d
        .tags
        .into_iter()
        .map(|(id, name, owner)| GroupTag { id, name, owner })
        .collect();
    let users = sqlx::query_scalar::<_, String>("SELECT slug FROM hub_users ORDER BY slug")
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    let collective_label = if d.collective_id != 0 {
        format!("{} {}", d.collective_icon, d.collective_name)
            .trim()
            .to_string()
    } else {
        String::new()
    };
    let icons = icon_options(&d.icon);
    let collective_options: Vec<ParentOption> = crate::tags::collectives_i_belong(&st.pool, nav.id)
        .await
        .into_iter()
        .map(|(id, name, icon)| ParentOption {
            selected: id == d.collective_id,
            id,
            name,
            icon,
        })
        .collect();
    render(&GroupPage {
        nav,
        flash: msg.msg.unwrap_or_default(),
        id: d.id,
        name: d.name,
        icon: d.icon,
        owner: d.owner,
        is_owner,
        can_edit,
        role_inherited: d.role_inherited,
        collective_id: d.collective_id,
        collective_label,
        collective_options,
        members,
        tags,
        users,
        icons,
    })
}

#[derive(Deserialize)]
struct CollectiveIdForm {
    collective_id: String,
}

async fn group_set_collective(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<CollectiveIdForm>,
) -> Response {
    let Some((uid, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let cid = f.collective_id.trim().parse::<i64>().ok().filter(|p| *p != 0);
    let _ = crate::tags::set_group_collective(&st.pool, uid, id, cid).await;
    Redirect::to(&format!("/groups/{id}")).into_response()
}

// ── collectives ──────────────────────────────────────────────────────────────

struct CollRow {
    id: i64,
    name: String,
    icon: String,
    owner: String,
    role: String,
    groups: i64,
    members: i64,
}

fn coll_row(c: crate::tags::Collective) -> CollRow {
    CollRow {
        id: c.id,
        name: c.name,
        icon: c.icon,
        owner: c.owner,
        role: c.role,
        groups: c.groups,
        members: c.members,
    }
}

#[derive(Template)]
#[template(path = "collectives.html")]
struct CollectivesPage {
    nav: crate::ui::Nav,
    flash: String,
    collectives: Vec<CollRow>,
    discover: Vec<CollRow>,
    icons: &'static [&'static str],
}

async fn collectives_page(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "collectives").await else {
        return Redirect::to("/login").into_response();
    };
    let collectives = crate::tags::list_collectives_for(&st.pool, nav.id)
        .await
        .into_iter()
        .map(coll_row)
        .collect();
    let discover = crate::tags::list_discover_collectives(&st.pool, nav.id)
        .await
        .into_iter()
        .map(coll_row)
        .collect();
    render(&CollectivesPage {
        nav,
        flash: String::new(),
        collectives,
        discover,
        icons: crate::tags::ICONS,
    })
}

async fn collective_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<GroupForm>,
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

struct CollGroup {
    id: i64,
    name: String,
    icon: String,
    tag_count: i64,
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
    groups: Vec<CollGroup>,
    available_groups: Vec<GroupRef>,
    users: Vec<String>,
    icons: Vec<IconOption>,
}

async fn collective_page(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Query(msg): Query<AdminMsg>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "collectives").await else {
        return Redirect::to("/login").into_response();
    };
    let Some(d) = crate::tags::collective_detail(&st.pool, nav.id, id).await else {
        return not_found("Collective nicht gefunden.");
    };
    let member = !d.role.is_empty();
    let members = d
        .members
        .into_iter()
        .map(|(slug, role)| CollMember { slug, role })
        .collect();
    let groups = d
        .groups
        .into_iter()
        .map(|(id, name, icon, tag_count)| CollGroup {
            id,
            name,
            icon,
            tag_count,
        })
        .collect();
    // The owner's own groups not yet in this collective.
    let available_groups: Vec<GroupRef> = if d.is_owner {
        sqlx::query_as::<_, (i64, String, String)>(
            "SELECT id, name, COALESCE(icon,'') FROM hub_tag_groups
              WHERE owner_user_id = ?1 AND (collective_id IS NULL OR collective_id <> ?2)
              ORDER BY name",
        )
        .bind(nav.id)
        .bind(id)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, name, icon)| GroupRef { id, name, icon })
        .collect()
    } else {
        Vec::new()
    };
    let users = sqlx::query_scalar::<_, String>("SELECT slug FROM hub_users ORDER BY slug")
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    let icons = icon_options(&d.icon);
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
        groups,
        available_groups,
        users,
        icons,
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
