//! Browse pages: search, user profiles, and playlist detail.
//!
//! Every page requires a session and links into the track detail page
//! (`/track/{id}`). Server-rendered with askama, styled by Pico.css via
//! `base.html`.

use std::collections::{HashMap, HashSet};

use askama::Template;
use axum::extract::{Path, Query, RawQuery, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;
use serde::Deserialize;
use sqlx::{QueryBuilder, Row};

use crate::api::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/search", get(search_page))
        .route("/overlap", get(overlap_page))
        .route("/playlists/similar", get(similar_page))
        .route("/settings", get(settings_page))
        .route("/user/{slug}", get(user_page))
        .route("/playlist/{id}", get(playlist_page))
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
        name: name.unwrap_or_default(),
        owner: owner.unwrap_or_default(),
        owner_name: owner_name.filter(|s| !s.is_empty()).unwrap_or_default(),
        tracks,
    })
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
            "SELECT hp.user_id AS uid, hp.id AS pl_id, hp.name AS pl_name, hp.is_owned AS owned,\n                    t.id AS tid, t.title AS title, t.artists AS artists, t.isrc AS isrc\n               FROM hub_playlist_tracks hpt\n               JOIN hub_playlists hp ON hp.id = hpt.playlist_id\n               JOIN hub_tracks t ON t.id = hpt.track_id\n              WHERE hp.user_id IN (",
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
            if scope == "owned" && owned != 1 {
                continue;
            }
            if scope == "followed" && owned == 1 {
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

#[derive(Template)]
#[template(path = "similar.html")]
struct SimilarPage {
    nav: crate::ui::Nav,
    flash: String,
    users: Vec<ColumnUser>,
    rows: Vec<SimilarRow>,
}

struct SimilarRow {
    label: String,
    user_count: i64,
    cells: Vec<SimilarCell>,
}

struct SimilarCell {
    present: bool,
    id: i64,
    name: String,
    owned: bool,
}

/// Lowercase, keep alphanumerics, collapse everything else to single spaces.
fn normalize_name(s: &str) -> String {
    let mut out = String::new();
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            for lc in ch.to_lowercase() {
                out.push(lc);
            }
            prev_space = false;
        } else if !prev_space {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}

async fn similar_page(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "similar").await else {
        return Redirect::to("/login").into_response();
    };

    let users = sqlx::query_as::<_, (i64, String)>("SELECT id, slug FROM hub_users ORDER BY slug")
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();

    let rows = sqlx::query(
        "SELECT p.user_id AS uid, p.id AS id, p.name AS name, p.is_owned AS owned
           FROM hub_playlists p",
    )
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    let mut groups: HashMap<String, HashMap<i64, SimilarCell>> = HashMap::new();
    for r in &rows {
        let name: String = r.get::<Option<String>, _>("name").unwrap_or_default();
        let key = normalize_name(&name);
        if key.is_empty() {
            continue;
        }
        let uid: i64 = r.get("uid");
        groups.entry(key).or_default().entry(uid).or_insert(SimilarCell {
            present: true,
            id: r.get("id"),
            name,
            owned: r.get::<i64, _>("owned") == 1,
        });
    }

    let mut out: Vec<SimilarRow> = Vec::new();
    for (key, by_user) in groups {
        if by_user.len() < 2 {
            continue;
        }
        let cells = users
            .iter()
            .map(|(uid, _)| match by_user.get(uid) {
                Some(c) => SimilarCell {
                    present: true,
                    id: c.id,
                    name: c.name.clone(),
                    owned: c.owned,
                },
                None => SimilarCell {
                    present: false,
                    id: 0,
                    name: String::new(),
                    owned: false,
                },
            })
            .collect();
        out.push(SimilarRow {
            label: key,
            user_count: by_user.len() as i64,
            cells,
        });
    }
    out.sort_by(|a, b| {
        b.user_count
            .cmp(&a.user_count)
            .then_with(|| a.label.cmp(&b.label))
    });
    out.truncate(500);

    render(&SimilarPage {
        nav,
        flash: String::new(),
        users: users
            .iter()
            .map(|(_, slug)| ColumnUser { slug: slug.clone() })
            .collect(),
        rows: out,
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
