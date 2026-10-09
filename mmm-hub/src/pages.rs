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
        .route("/playlists/similar", get(similar_page))
        .route("/tags", get(tags_page))
        .route("/digging", get(digging_page))
        .route("/digging/enrich", post(digging_enrich))
        .route("/admin", get(admin_page).post(admin_save))
        .route("/settings", get(settings_page))
        .route("/user/{slug}", get(user_page))
        .route("/playlist/{id}", get(playlist_page))
        .route("/playlist/{id}/tag", post(playlist_tag))
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
    })
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
    /// `all` | `owned` | `followed`.
    scope: Option<String>,
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

fn scope_href(ids: &[i64], scope: &str) -> String {
    let csv = ids
        .iter()
        .map(|i| i.to_string())
        .collect::<Vec<_>>()
        .join(",");
    if scope == "all" {
        format!("/overlap?users={csv}")
    } else {
        format!("/overlap?users={csv}&scope={scope}")
    }
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
        _ => "all",
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

    // Optional per-playlist include list (?pl=1&pl=2). Absent = include all.
    let pl_filter: Option<HashSet<i64>> = raw.as_deref().and_then(|raw| {
        let ids: HashSet<i64> = raw
            .split('&')
            .filter_map(|kv| kv.split_once('='))
            .filter(|(k, _)| *k == "pl")
            .filter_map(|(_, v)| v.parse::<i64>().ok())
            .collect();
        if ids.is_empty() { None } else { Some(ids) }
    });

    let mut acc: HashMap<i64, TrackAcc> = HashMap::new();

    if !selected.is_empty() {
        let mut qb = QueryBuilder::new(
            "SELECT hp.user_id AS uid, hp.id AS pl_id, hp.name AS pl_name, hp.is_owned AS owned,\n                    hp.collaborative AS collab,\n                    t.id AS tid, t.title AS title, t.artists AS artists, t.isrc AS isrc\n               FROM hub_playlist_tracks hpt\n               JOIN hub_playlists hp ON hp.id = hpt.playlist_id\n               JOIN hub_tracks t ON t.id = hpt.track_id\n              WHERE hp.user_id IN (",
        );
        let mut sep = qb.separated(", ");
        for (id, _) in &selected {
            sep.push_bind(*id);
        }
        qb.push(")");
        let rows = qb.build().fetch_all(&st.pool).await.unwrap_or_default();
        for r in rows {
            let uid: i64 = r.get("uid");
            let pl_id: i64 = r.get("pl_id");
            let owned: i64 = r.get("owned");
            let collab: i64 = r.get("collab");
            if !scope_match(scope, owned == 1, collab == 1) {
                continue;
            }
            if let Some(f) = &pl_filter {
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
            "SELECT l.user_id AS uid, t.id AS tid, t.title AS title, t.artists AS artists, t.isrc AS isrc\n               FROM hub_liked_tracks l JOIN hub_tracks t ON t.id = l.track_id\n              WHERE l.user_id IN (",
        );
        let mut sep = qb.separated(", ");
        for (id, _) in &selected {
            sep.push_bind(*id);
        }
        qb.push(")");
        for r in qb.build().fetch_all(&st.pool).await.unwrap_or_default() {
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
    }

    // Assemble rows: keep tracks present for >= 2 of the selected users.
    let mut rows: Vec<CompareRow> = Vec::new();
    let mut pair_counts: HashMap<(i64, i64), i64> = HashMap::new();
    for (tid, t) in acc {
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
            present_count: present.len() as i64,
            cells,
        });
    }
    rows.sort_by(|a, b| {
        b.present_count
            .cmp(&a.present_count)
            .then_with(|| a.artists.cmp(&b.artists))
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
            let href = if scope == "all" {
                format!(
                    "/overlap?users={}",
                    ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
                )
            } else {
                scope_href(&ids, scope)
            };
            PickerUser {
                slug: slug.clone(),
                selected: is_sel,
                href,
            }
        })
        .collect();

    let scopes = vec![
        ScopeLink {
            label: "Alle".to_string(),
            href: scope_href(&current_ids, "all"),
            active: scope == "all",
        },
        ScopeLink {
            label: "Eigene".to_string(),
            href: scope_href(&current_ids, "owned"),
            active: scope == "owned",
        },
        ScopeLink {
            label: "Collaborativ".to_string(),
            href: scope_href(&current_ids, "contributed"),
            active: scope == "contributed",
        },
        ScopeLink {
            label: "Gefolgt".to_string(),
            href: scope_href(&current_ids, "followed"),
            active: scope == "followed",
        },
    ];

    let columns: Vec<ColumnUser> = selected
        .iter()
        .map(|(_, slug)| ColumnUser { slug: slug.clone() })
        .collect();

    render(&OverlapPage {
        nav,
        flash: String::new(),
        picker,
        scopes,
        columns,
        track_total: rows.len() as i64,
        rows,
        pairs,
    })
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
    slugs: String,
    bpm: Option<f64>,
    bpm_disp: String,
    camelot: String,
    score: i64,
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
            slugs: p.slugs,
            bpm: None,
            bpm_disp: String::new(),
            camelot: String::new(),
            score: 0,
        },
    );
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
    let tol = q.tol.filter(|t| *t > 0.0 && *t <= 50.0).unwrap_or(6.0);

    let mut rows: Vec<DigRow> = cand.into_values().collect();
    rows.retain(|r| {
        if mine_only && !(r.users > 0 || r.likes > 0) {
            return false;
        }
        if bpm_only {
            match (seed_bpm, r.bpm) {
                (Some(sb), Some(b)) if sb > 0.0 => {
                    if (b - sb).abs() / sb * 100.0 > tol {
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

    // Filter toggle links, preserving the other flags.
    let link = |mine: bool, bpm: bool, harm: bool| -> String {
        let mut s = format!("/digging?seed={seed_id}");
        if mine {
            s.push_str("&mine=1");
        }
        if bpm {
            s.push_str("&bpm=1");
        }
        if harm {
            s.push_str("&harm=1");
        }
        s
    };
    let filters = vec![
        ScopeLink {
            label: "Alle".to_string(),
            href: link(false, false, false),
            active: !(mine_only || bpm_only || harm_only),
        },
        ScopeLink {
            label: format!("Nur bei uns ({with_hub})"),
            href: link(!mine_only, bpm_only, harm_only),
            active: mine_only,
        },
        ScopeLink {
            label: format!("BPM ±{tol:.0}%"),
            href: link(mine_only, !bpm_only, harm_only),
            active: bpm_only,
        },
        ScopeLink {
            label: "Harmonisch".to_string(),
            href: link(mine_only, bpm_only, !harm_only),
            active: harm_only,
        },
    ];

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
}

#[derive(Template)]
#[template(path = "tags.html")]
struct TagsPage {
    nav: crate::ui::Nav,
    flash: String,
    q: String,
    tags: Vec<TagRow>,
}

struct TagRow {
    name: String,
    owner: String,
    track_count: i64,
    user_count: i64,
    source_count: i64,
    users: String,
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

    let rows = sqlx::query(
        "SELECT t.id, t.name, u.slug AS owner,
                (SELECT COUNT(*) FROM hub_track_resolved_tags r WHERE r.tag_id = t.id) AS track_count,
                (SELECT COUNT(DISTINCT user_id) FROM hub_tag_sources s WHERE s.tag_id = t.id) AS user_count,
                (SELECT COUNT(*) FROM hub_tag_sources s WHERE s.tag_id = t.id) AS source_count,
                (SELECT GROUP_CONCAT(DISTINCT u2.slug) FROM hub_tag_sources s
                   JOIN hub_users u2 ON u2.id = s.user_id WHERE s.tag_id = t.id) AS users
           FROM hub_tags t
           JOIN hub_users u ON u.id = t.owner_user_id
          WHERE (?1 = '' OR lower(t.name) LIKE '%' || lower(?1) || '%')
          ORDER BY u.slug, track_count DESC, t.name
          LIMIT 1000",
    )
    .bind(&q)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    let tags: Vec<TagRow> = rows
        .iter()
        .map(|r| TagRow {
            name: r.get::<Option<String>, _>("name").unwrap_or_default(),
            owner: r.get::<Option<String>, _>("owner").unwrap_or_default(),
            track_count: r.get::<Option<i64>, _>("track_count").unwrap_or(0),
            user_count: r.get::<Option<i64>, _>("user_count").unwrap_or(0),
            source_count: r.get::<Option<i64>, _>("source_count").unwrap_or(0),
            users: r.get::<Option<String>, _>("users").unwrap_or_default(),
        })
        .collect();

    render(&TagsPage {
        nav,
        flash: String::new(),
        q,
        tags,
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
