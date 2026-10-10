//! Minimal web UI: username/password accounts, sessions, and "Connect Spotify".
//!
//! Uses the public HTTPS redirect (`https://…/api/hub/services/spotify/callback`)
//! instead of a loopback listener, so it works for any logged-in user in a browser.

use askama::Template;
use axum::Router;
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::Value;
use sqlx::{Row, SqlitePool};
use std::collections::HashMap;

use crate::api::AppState;
use crate::spotify;

const SESSION_COOKIE: &str = "hub_session";
const SESSION_TTL_SECS: i64 = 60 * 60 * 24 * 30;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/login", get(login_form).post(login_submit))
        .route("/signup", get(signup_form).post(signup_submit))
        .route("/logout", post(logout))
        .route("/me/playlists", get(playlists_page))
        .route("/sql", get(sql_page).post(sql_run))
        .route("/track/{id}", get(track_page))
        .route("/track/{id}/fetch", post(track_fetch))
        .route("/track/{id}/tag", post(track_tag))
        .route("/track/{id}/untag", post(track_untag))
        .route("/api/hub/services/{service}/connect", get(connect))
        .route(
            "/api/hub/services/{service}/fetch-playlists",
            post(fetch_playlists_handler),
        )
        .route("/api/hub/services/{service}/sync-html", post(sync_html))
        .route("/api/hub/playlists/{id}/toggle", post(toggle_playlist))
        .route("/api/hub/playlists/enable-all", post(enable_all))
        .route("/api/hub/playlists/disable-all", post(disable_all))
        .route("/api/hub/services/{service}/callback", get(callback))
        .route("/api/hub/services/{service}/disconnect", post(disconnect))
        .with_state(state)
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ── sessions ────────────────────────────────────────────────────────────────

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some((k, v)) = part.split_once('=') {
            if k == name {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Resolve the session cookie to `(user_id, slug)`.
pub async fn current_user(st: &AppState, headers: &HeaderMap) -> Option<(i64, String)> {
    let token = cookie_value(headers, SESSION_COOKIE)?;
    let row = sqlx::query_as::<_, (i64, String)>(
        "SELECT u.id, u.slug
           FROM hub_web_sessions s
           JOIN hub_users u ON u.id = s.user_id
          WHERE s.id = ?1 AND s.expires_at > ?2",
    )
    .bind(&token)
    .bind(now_iso())
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten()?;
    Some(row)
}

async fn create_session(pool: &SqlitePool, user_id: i64) -> anyhow::Result<String> {
    let token = spotify::random_token();
    let expires = (chrono::Utc::now() + chrono::Duration::seconds(SESSION_TTL_SECS)).to_rfc3339();
    sqlx::query(
        "INSERT INTO hub_web_sessions (id, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&token)
    .bind(user_id)
    .bind(now_iso())
    .bind(expires)
    .execute(pool)
    .await?;
    Ok(token)
}

fn session_cookie(token: &str) -> String {
    format!("{SESSION_COOKIE}={token}; HttpOnly; Path=/; Max-Age={SESSION_TTL_SECS}; SameSite=Lax")
}

fn clear_cookie() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; Path=/; Max-Age=0; SameSite=Lax")
}

fn with_cookie(mut resp: Response, cookie: String) -> Response {
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        resp.headers_mut().insert(header::SET_COOKIE, v);
    }
    resp
}

// ── HTML ────────────────────────────────────────────────────────────────────

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render an askama template, mapping a render failure to a short 500.
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

fn error_page(msg: &str) -> Response {
    render(&ErrorPage {
        message: msg.to_string(),
    })
}

// ── templates ────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorPage {
    message: String,
}

#[derive(Template)]
#[template(path = "login.html")]
struct LoginPage {
    error: String,
    registration_open: bool,
}

#[derive(Template)]
#[template(path = "signup.html")]
struct SignupPage {
    error: String,
    open: bool,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardPage {
    nav: crate::ui::Nav,
    flash: String,
    connected: bool,
    spotify_label: String,
    playlists_count: i64,
    fetched_count: i64,
    likes_count: i64,
    likes_error: String,
    recent: Vec<RecentRow>,
}

struct RecentRow {
    id: i64,
    title: String,
    artists: String,
}

#[derive(Template)]
#[template(path = "playlists.html")]
struct PlaylistsPage {
    nav: crate::ui::Nav,
    flash: String,
    filter: String,
    q: String,
    owner_options: Vec<OwnerOpt>,
    count: usize,
    playlists: Vec<PlaylistRow>,
    counts: PlaylistCounts,
}

/// Per-filter playlist counts for the filter-bar labels.
struct PlaylistCounts {
    all: i64,
    owned: i64,
    with_items: i64,
    not_fetched: i64,
    error: i64,
    tagged: i64,
    untagged: i64,
}

/// One selectable owner value with a precomputed `selected` flag (keeps the
/// askama template free of `==` comparisons inside HTML tags).
struct OwnerOpt {
    name: String,
    selected: bool,
}

struct PlaylistRow {
    id: i64,
    name: String,
    user: String,
    owner: String,
    owned: bool,
    tracks: String,
    status: String,
    status_class: String,
    enabled: bool,
    /// Names of the current user's tags fed by this playlist (comma-joined).
    tags: String,
}

/// A single `<tr>` for the htmx out-of-band row swap (see `toggle_playlist`).
#[derive(Template)]
#[template(path = "_partials/playlist_row.html")]
struct PlaylistRowPartial {
    p: PlaylistRow,
}

#[derive(Template)]
#[template(path = "sql.html")]
struct SqlPage {
    nav: crate::ui::Nav,
    flash: String,
    sql: String,
    presets: Vec<(String, String)>,
    presets_json: String,
    error: String,
    table_html: String,
}

fn login_form_html(err: Option<&str>, registration_open: bool) -> Response {
    render(&LoginPage {
        error: err.unwrap_or("").to_string(),
        registration_open,
    })
}

fn signup_form_html(err: Option<&str>, registration_open: bool) -> Response {
    render(&SignupPage {
        error: err.unwrap_or("").to_string(),
        open: registration_open,
    })
}

// ── handlers ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Creds {
    username: String,
    password: String,
}

#[derive(Deserialize)]
struct Flash {
    msg: Option<String>,
}

/// Server-side playlist filter (issue #163). Unknown values fall back to `all`.
#[derive(Deserialize)]
struct PlaylistFilterQuery {
    filter: Option<String>,
    q: Option<String>,
    owner: Option<String>,
}

/// Whitelist the known filter keys; anything else means `all`.
fn normalise_filter(filter: Option<&str>) -> &'static str {
    match filter {
        Some("owned") => "owned",
        Some("with_items") => "with_items",
        Some("not_fetched") => "not_fetched",
        Some("error") => "error",
        Some("tagged") => "tagged",
        Some("untagged") => "untagged",
        _ => "all",
    }
}

/// The `WHERE` fragment for a normalised filter. Control-flow only — never user
/// input. `?1` is the current user id, so the tag filters can key on it.
fn filter_clause(filter: &str) -> &'static str {
    match filter {
        "owned" => " AND p.is_owned = 1",
        "with_items" => " AND p.items_available = 1",
        "not_fetched" => " AND p.items_available = 0 AND p.enabled_for_fetch = 1",
        "error" => " AND p.fetch_error IS NOT NULL",
        "tagged" => {
            " AND EXISTS (SELECT 1 FROM hub_tag_sources s JOIN hub_tags t ON t.id = s.tag_id WHERE s.playlist_id = p.id AND t.owner_user_id = ?1)"
        }
        "untagged" => {
            " AND NOT EXISTS (SELECT 1 FROM hub_tag_sources s JOIN hub_tags t ON t.id = s.tag_id WHERE s.playlist_id = p.id AND t.owner_user_id = ?1)"
        }
        _ => "",
    }
}

async fn index(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(flash): Query<Flash>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "dashboard").await else {
        return Redirect::to("/login").into_response();
    };
    let uid = nav.id;
    let connected = nav.spotify_connected;
    let spotify_label = nav.spotify_label.clone();

    let playlists_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_playlists WHERE user_id = ?1")
            .bind(uid)
            .fetch_one(&st.pool)
            .await
            .unwrap_or(0);
    let fetched_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_playlists WHERE user_id = ?1 AND items_available = 1",
    )
    .bind(uid)
    .fetch_one(&st.pool)
    .await
    .unwrap_or(0);

    let (likes_count, likes_error) = likes_status(&st, uid).await;
    let recent = recent_tracks(&st).await;

    render(&DashboardPage {
        nav,
        flash: flash.msg.unwrap_or_default(),
        connected,
        spotify_label,
        playlists_count,
        fetched_count,
        likes_count,
        likes_error,
        recent,
    })
}

async fn playlists_page(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(flash): Query<Flash>,
    Query(pf): Query<PlaylistFilterQuery>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "playlists").await else {
        return Redirect::to("/login").into_response();
    };
    let filter = normalise_filter(pf.filter.as_deref());
    let q = pf.q.unwrap_or_default().trim().to_string();
    let owner = pf.owner.unwrap_or_default().trim().to_string();
    let playlists = playlist_rows(&st, nav.id, filter, &q, &owner).await;
    let counts = playlist_counts(&st, nav.id).await;
    let owner_options = playlist_owners(&st, nav.id, &owner).await;
    render(&PlaylistsPage {
        nav,
        flash: flash.msg.unwrap_or_default(),
        filter: filter.to_string(),
        q,
        owner_options,
        count: playlists.len(),
        playlists,
        counts,
    })
}

/// Liked-tracks count + an optional sync error for the dashboard.
async fn likes_status(st: &AppState, user_id: i64) -> (i64, String) {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_liked_tracks WHERE user_id = ?1")
        .bind(user_id)
        .fetch_one(&st.pool)
        .await
        .unwrap_or(0);

    let row = sqlx::query(
        "SELECT likes_status, likes_error FROM hub_service_accounts
          WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(user_id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten();
    let (status, err): (Option<String>, Option<String>) = match row {
        Some(r) => (r.get("likes_status"), r.get("likes_error")),
        None => (None, None),
    };

    let error = match err {
        Some(e) if !e.is_empty() => e,
        _ if status.as_deref() == Some("error") => "Sync fehlgeschlagen".to_string(),
        _ => String::new(),
    };
    (count, error)
}

/// Map one DB row from the playlist query to a `PlaylistRow`.
fn map_playlist_row(r: &sqlx::sqlite::SqliteRow) -> PlaylistRow {
    let track_count: Option<i64> = r.get("track_count");
    let items_available: i64 = r.get("items_available");
    let enabled: i64 = r.get("enabled_for_fetch");
    let err: Option<String> = r.get("fetch_error");
    let fetched: i64 = r.get("fetched");

    let (status, status_class) = if let Some(e) = err {
        (format!("Fehler: {e}"), "hub-flash-err".to_string())
    } else if items_available == 1 {
        ("✓ geholt".to_string(), "".to_string())
    } else if enabled == 1 {
        ("wartet…".to_string(), "hub-muted".to_string())
    } else {
        ("—".to_string(), "hub-muted".to_string())
    };

    PlaylistRow {
        id: r.get("id"),
        name: r.get::<Option<String>, _>("name").unwrap_or_default(),
        user: r.get::<Option<String>, _>("user_slug").unwrap_or_default(),
        owner: r
            .get::<Option<String>, _>("owner_name")
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "—".to_string()),
        owned: r.get::<i64, _>("is_owned") == 1,
        tracks: format!(
            "{} / {}",
            fetched,
            track_count
                .map(|n| n.to_string())
                .unwrap_or_else(|| "?".into())
        ),
        status,
        status_class,
        enabled: enabled == 1,
        tags: r.get::<Option<String>, _>("my_tags").unwrap_or_default(),
    }
}

/// The user's playlists (no pagination) as rows for `playlists.html`.
///
/// `filter` is one of the keys returned by [`normalise_filter`]; the matching
/// `WHERE` fragment is appended before the stable ordering.
async fn playlist_rows(
    st: &AppState,
    user_id: i64,
    filter: &str,
    q: &str,
    owner: &str,
) -> Vec<PlaylistRow> {
    let sql = format!(
        "SELECT p.id, p.name, p.is_owned, p.track_count, p.items_available,
                p.enabled_for_fetch, p.fetch_error,
                COALESCE(NULLIF(p.owner_name, ''), CASE WHEN p.is_owned = 1 THEN a.display_name END) AS owner_name,
                u.slug AS user_slug,
                (SELECT COUNT(*) FROM hub_playlist_tracks t WHERE t.playlist_id = p.id) AS fetched,
                (SELECT GROUP_CONCAT(tg.name, ', ') FROM hub_tag_sources s
                   JOIN hub_tags tg ON tg.id = s.tag_id
                  WHERE s.playlist_id = p.id AND tg.owner_user_id = ?1) AS my_tags
           FROM hub_playlists p
           JOIN hub_users u ON u.id = p.user_id
           LEFT JOIN hub_service_accounts a ON a.user_id = p.user_id AND a.service = 'spotify'
          WHERE p.user_id = ?1
            AND (?2 = '' OR lower(p.name) LIKE '%' || lower(?2) || '%')
            AND (?3 = '' OR COALESCE(NULLIF(p.owner_name, ''), CASE WHEN p.is_owned = 1 THEN a.display_name END) = ?3){}
          ORDER BY p.is_owned DESC, p.name COLLATE NOCASE",
        filter_clause(filter)
    );
    let rows = sqlx::query(&sql)
        .bind(user_id)
        .bind(q)
        .bind(owner)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();

    rows.iter().map(map_playlist_row).collect()
}

/// Distinct owner names across the user's playlists, for the owner filter.
async fn playlist_owners(st: &AppState, user_id: i64, current: &str) -> Vec<OwnerOpt> {
    let rows = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT COALESCE(NULLIF(p.owner_name, ''), CASE WHEN p.is_owned = 1 THEN a.display_name END) AS oname
           FROM hub_playlists p
           LEFT JOIN hub_service_accounts a ON a.user_id = p.user_id AND a.service = 'spotify'
          WHERE p.user_id = ?1 AND oname IS NOT NULL AND oname <> ''
          ORDER BY oname COLLATE NOCASE",
    )
    .bind(user_id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .map(|name| OwnerOpt {
            selected: name == current,
            name,
        })
        .collect()
}

/// One playlist row by id (for the htmx single-row swap). `None` if it doesn't
/// exist or doesn't belong to `user_id`.
async fn playlist_row(st: &AppState, user_id: i64, id: i64) -> Option<PlaylistRow> {
    let row = sqlx::query(
        "SELECT p.id, p.name, p.is_owned, p.track_count, p.items_available,
                p.enabled_for_fetch, p.fetch_error,
                COALESCE(NULLIF(p.owner_name, ''), CASE WHEN p.is_owned = 1 THEN a.display_name END) AS owner_name,
                u.slug AS user_slug,
                (SELECT COUNT(*) FROM hub_playlist_tracks t WHERE t.playlist_id = p.id) AS fetched,
                (SELECT GROUP_CONCAT(tg.name, ', ') FROM hub_tag_sources s
                   JOIN hub_tags tg ON tg.id = s.tag_id
                  WHERE s.playlist_id = p.id AND tg.owner_user_id = ?1) AS my_tags
           FROM hub_playlists p
           JOIN hub_users u ON u.id = p.user_id
           LEFT JOIN hub_service_accounts a ON a.user_id = p.user_id AND a.service = 'spotify'
          WHERE p.user_id = ?1 AND p.id = ?2",
    )
    .bind(user_id)
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten()?;
    Some(map_playlist_row(&row))
}

/// Counts per filter key for the filter-bar labels (single scan).
async fn playlist_counts(st: &AppState, user_id: i64) -> PlaylistCounts {
    let row = sqlx::query(
        "SELECT
            COUNT(*) AS all_c,
            COALESCE(SUM(CASE WHEN is_owned = 1 THEN 1 ELSE 0 END), 0) AS owned_c,
            COALESCE(SUM(CASE WHEN items_available = 1 THEN 1 ELSE 0 END), 0) AS items_c,
            COALESCE(SUM(CASE WHEN items_available = 0 AND enabled_for_fetch = 1 THEN 1 ELSE 0 END), 0) AS not_fetched_c,
                    COALESCE(SUM(CASE WHEN fetch_error IS NOT NULL THEN 1 ELSE 0 END), 0) AS error_c,
            COALESCE(SUM(CASE WHEN EXISTS (SELECT 1 FROM hub_tag_sources s JOIN hub_tags t ON t.id = s.tag_id WHERE s.playlist_id = hub_playlists.id AND t.owner_user_id = ?1) THEN 1 ELSE 0 END), 0) AS tagged_c,
            COALESCE(SUM(CASE WHEN NOT EXISTS (SELECT 1 FROM hub_tag_sources s JOIN hub_tags t ON t.id = s.tag_id WHERE s.playlist_id = hub_playlists.id AND t.owner_user_id = ?1) THEN 1 ELSE 0 END), 0) AS untagged_c
           FROM hub_playlists
          WHERE user_id = ?1",
    )
    .bind(user_id)
    .fetch_one(&st.pool)
    .await
    .ok();

    match row {
        Some(r) => PlaylistCounts {
            all: r.get("all_c"),
            owned: r.get("owned_c"),
            with_items: r.get("items_c"),
            not_fetched: r.get("not_fetched_c"),
            error: r.get("error_c"),
            tagged: r.get("tagged_c"),
            untagged: r.get("untagged_c"),
        },
        None => PlaylistCounts {
            all: 0,
            owned: 0,
            with_items: 0,
            not_fetched: 0,
            error: 0,
            tagged: 0,
            untagged: 0,
        },
    }
}

async fn fetch_playlists_handler(
    State(st): State<AppState>,
    Path(service): Path<String>,
    headers: HeaderMap,
) -> Response {
    if service != "spotify" {
        return error_page("Diesen Dienst gibt es (noch) nicht.");
    }
    let Some((_, slug)) = current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    match crate::ingest::fetch_playlists(&st.pool, &st.cfg, &slug).await {
        Ok(f) => {
            let msg = format!(
                "{} Playlists geholt ({} eigene, {} gefolgt)",
                f.total, f.owned, f.followed
            );
            Redirect::to(&format!("/me/playlists?msg={}", urlencoding::encode(&msg)))
                .into_response()
        }
        Err(e) => error_page(&format!("Playlists holen fehlgeschlagen: {e}")),
    }
}

async fn toggle_playlist(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some((uid, _)) = current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    // Flip the flag; when turning ON, reset so the worker re-fetches it.
    let _ = sqlx::query(
        "UPDATE hub_playlists
            SET enabled_for_fetch = CASE WHEN enabled_for_fetch = 1 THEN 0 ELSE 1 END,
                fetch_status      = CASE WHEN enabled_for_fetch = 1 THEN fetch_status ELSE 'queued' END,
                items_available   = CASE WHEN enabled_for_fetch = 1 THEN items_available ELSE 0 END,
                fetch_error       = NULL
          WHERE id = ?1 AND user_id = ?2",
    )
    .bind(id)
    .bind(uid)
    .execute(&st.pool)
    .await;

    // htmx path: return just the refreshed <tr> for `hx-swap="outerHTML"`.
    if headers.get("hx-request").is_some() {
        return match playlist_row(&st, uid, id).await {
            Some(p) => render(&PlaylistRowPartial { p }),
            None => error_page("Playlist nicht gefunden."),
        };
    }

    Redirect::to("/me/playlists").into_response()
}

/// htmx-friendly twin of `POST /api/hub/services/{service}/sync`.
///
/// Runs the same DB queueing logic as [`crate::api::sync`], but returns a tiny
/// HTML badge instead of JSON so a button can htmx-swap it in place (and it
/// still degrades to a plain form post when JS is off).
async fn sync_html(
    State(st): State<AppState>,
    Path(service): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some((uid, _)) = current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    if service != "spotify" {
        return error_page("Diesen Dienst gibt es (noch) nicht.");
    }

    let _ = sqlx::query(
        "UPDATE hub_service_accounts
            SET likes_status = 'queued', likes_synced_at = NULL, likes_error = NULL
          WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(uid)
    .execute(&st.pool)
    .await;

    let _ = sqlx::query(
        "UPDATE hub_playlists
            SET enabled_for_fetch = 1, fetch_status = 'queued',
                items_available = 0, fetch_error = NULL
          WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(uid)
    .execute(&st.pool)
    .await;

    // htmx swaps this in; a plain form post redirects somewhere sensible.
    if headers.get("hx-request").is_some() {
        Html("<span class=\"hub-badge hub-badge-ok\">synchronisiert ✓</span>").into_response()
    } else {
        Redirect::to(
            "/me/playlists?msg=Synchronisation+eingericht+%E2%80%94+l%C3%A4uft+im+Hintergrund",
        )
        .into_response()
    }
}

async fn enable_all(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some((uid, _)) = current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = sqlx::query(
        "UPDATE hub_playlists
            SET enabled_for_fetch = 1, fetch_status = 'queued', items_available = 0, fetch_error = NULL
          WHERE user_id = ?1",
    )
    .bind(uid)
    .execute(&st.pool)
    .await;
    Redirect::to("/me/playlists").into_response()
}

async fn disable_all(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some((uid, _)) = current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = sqlx::query("UPDATE hub_playlists SET enabled_for_fetch = 0 WHERE user_id = ?1")
        .bind(uid)
        .execute(&st.pool)
        .await;
    Redirect::to("/me/playlists").into_response()
}

// ── SQL console ─────────────────────────────────────────────────────────────

const DEFAULT_SQL: &str = "SELECT t.artists, t.title, t.album, s.user_count, s.user_ids\nFROM hub_v_shared_tracks s\nJOIN hub_tracks t ON t.id = s.track_id\nORDER BY s.user_count DESC, t.artists\nLIMIT 100";

const PRESETS: &[(&str, &str)] = &[
    ("Geteilte Tracks (alle User)", DEFAULT_SQL),
    (
        "Overlaps pro Paar",
        "SELECT ua.slug AS a, ub.slug AS b, o.shared_tracks\nFROM hub_v_user_overlap o\nJOIN hub_users ua ON ua.id = o.user_a_id\nJOIN hub_users ub ON ub.id = o.user_b_id\nORDER BY o.shared_tracks DESC",
    ),
    (
        "Liked-Tracks pro User",
        "SELECT u.slug, COUNT(*) AS likes\nFROM hub_liked_tracks l JOIN hub_users u ON u.id = l.user_id\nGROUP BY u.slug ORDER BY likes DESC",
    ),
    (
        "Playlists pro User",
        "SELECT u.slug, COUNT(*) AS playlists, SUM(p.items_available) AS fetched\nFROM hub_playlists p JOIN hub_users u ON u.id = p.user_id\nGROUP BY u.slug ORDER BY playlists DESC",
    ),
    (
        "Wer hat Track #1?",
        "SELECT u.slug, p.source, p.playlist_name, p.added_at\nFROM hub_v_track_presence p JOIN hub_users u ON u.id = p.user_id\nWHERE p.track_id = 1 ORDER BY u.slug",
    ),
    (
        "Alle Playlists (Suche)",
        "SELECT u.slug AS user, p.name, p.track_count\nFROM hub_playlists p JOIN hub_users u ON u.id = p.user_id\nWHERE lower(p.name) LIKE '%house%'\nORDER BY u.slug, p.name",
    ),
];

#[derive(Deserialize)]
struct SqlForm {
    sql: Option<String>,
}

async fn sql_page(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "sql").await else {
        return Redirect::to("/login").into_response();
    };
    render(&sql_page_struct(nav, DEFAULT_SQL, None, None))
}

async fn sql_run(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<SqlForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "sql").await else {
        return Redirect::to("/login").into_response();
    };
    let sql = form.sql.unwrap_or_default();
    match crate::api::run_readonly_query(&st.ro_pool, &sql).await {
        Ok((columns, rows)) => render(&sql_page_struct(nav, &sql, Some((columns, rows)), None)),
        Err(e) => render(&sql_page_struct(nav, &sql, None, Some(e.to_string()))),
    }
}

fn sql_page_struct(
    nav: crate::ui::Nav,
    sql: &str,
    results: Option<(Vec<String>, Vec<Value>)>,
    error: Option<String>,
) -> SqlPage {
    let presets: Vec<(String, String)> = PRESETS
        .iter()
        .enumerate()
        .map(|(i, (name, _))| (i.to_string(), name.to_string()))
        .collect();
    let presets_json: Vec<String> = PRESETS.iter().map(|(_, q)| q.to_string()).collect();
    let presets_json = serde_json::to_string(&presets_json).unwrap_or_else(|_| "[]".to_string());
    let table_html = results
        .map(|(cols, rows)| render_table(&cols, &rows))
        .unwrap_or_default();
    SqlPage {
        nav,
        flash: String::new(),
        sql: sql.to_string(),
        presets,
        presets_json,
        error: error.unwrap_or_default(),
        table_html,
    }
}

fn render_table(columns: &[String], rows: &[Value]) -> String {
    let cap = 2000usize;
    let shown = rows.len().min(cap);
    let note = if rows.len() > cap {
        format!(" (erste {cap} angezeigt)")
    } else {
        String::new()
    };
    let mut out = format!(
        "<p class=\"muted\">{} Zeile(n){note}</p><table>",
        rows.len()
    );
    out.push_str("<tr>");
    for c in columns {
        out.push_str(&format!("<th>{}</th>", esc(c)));
    }
    out.push_str("</tr>");
    for r in rows.iter().take(shown) {
        out.push_str("<tr>");
        for c in columns {
            let cell = match r.get(c) {
                None | Some(Value::Null) => "<span class=\"muted\">∅</span>".to_string(),
                Some(Value::String(s)) => esc(s),
                Some(v) => esc(&v.to_string()),
            };
            out.push_str(&format!("<td>{cell}</td>"));
        }
        out.push_str("</tr>");
    }
    out.push_str("</table>");
    out
}

// ── track detail ────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "track.html")]
struct TrackPage {
    nav: crate::ui::Nav,
    id: i64,
    title: String,
    artists: String,
    album: String,
    duration: String,
    isrc: String,
    explicit: bool,
    bpm: String,
    music_key: String,
    energy: String,
    feat_note: String,
    users: Vec<UserGroup>,
    avail_text: String,
    deezer_id: String,
    fetchable: bool,
    external_ids: Vec<ExternalId>,
    tags: Vec<TrackTag>,
    genres: Vec<String>,
    flash: String,
    music_api: bool,
    // Ripeness (issues #214/#215).
    ripeness_total: i64,
    ripeness_tag: i64,
    ripeness_meta: i64,
    ripeness_trak: String,
    meta_present: Vec<(String, bool)>,
    // Core-element additions: direct tagging + player + recommendations.
    my_tags: Vec<(i64, String)>,
    recommended: Vec<RecTag>,
    can_stream: bool,
    me: String,
    spotify_id: String,
}

struct RecTag {
    id: i64,
    name: String,
    owner: String,
    score: i64,
}

struct UserGroup {
    slug: String,
    liked: bool,
    owned: Vec<PlaylistRef>,
    followed: Vec<PlaylistRef>,
}

struct PlaylistRef {
    id: i64,
    name: String,
    owner: String,
}

struct ExternalId {
    service: String,
    external_id: String,
    url: String,
}

/// A tag on the track's detail page: owner + the groups it lives in.
struct TrackTag {
    id: i64,
    name: String,
    owner: String,
    groups: Vec<TrackTagGroup>,
}

struct TrackTagGroup {
    id: i64,
    name: String,
    icon: String,
}

async fn track_page(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Query(flash): Query<Flash>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "track").await else {
        return Redirect::to("/login").into_response();
    };

    let row = sqlx::query(
        "SELECT id, title, artists, album, duration_ms, isrc, service, service_track_id,
                image_url, explicit
           FROM hub_tracks WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten();
    let Some(row) = row else {
        return (
            StatusCode::NOT_FOUND,
            render(&ErrorPage {
                message: "Track nicht gefunden.".to_string(),
            }),
        )
            .into_response();
    };

    // Presence grouped by user, split into own vs. followed playlists.
    let presence = sqlx::query(
        "SELECT p.user_id, u.slug, p.source, p.playlist_id, p.playlist_name, hp.is_owned,
                COALESCE(NULLIF(hp.owner_name, ''), CASE WHEN hp.is_owned = 1 THEN a.display_name END) AS owner_name
           FROM hub_v_track_presence p
           JOIN hub_users u ON u.id = p.user_id
           LEFT JOIN hub_playlists hp ON hp.id = p.playlist_id
           LEFT JOIN hub_service_accounts a ON a.user_id = p.user_id AND a.service = 'spotify'
          WHERE p.track_id = ?1
          ORDER BY u.slug, hp.is_owned DESC, p.playlist_name",
    )
    .bind(id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    let mut order: Vec<i64> = Vec::new();
    let mut groups: std::collections::HashMap<i64, UserGroup> = std::collections::HashMap::new();
    for r in &presence {
        let uid: i64 = r.get("user_id");
        let slug: Option<String> = r.get("slug");
        let source: Option<String> = r.get("source");
        let playlist_id: Option<i64> = r.get("playlist_id");
        let playlist_name: Option<String> = r.get("playlist_name");
        let is_owned: Option<i64> = r.get("is_owned");
        let owner_name: Option<String> = r.get("owner_name");
        let slug = slug.unwrap_or_default();
        let g = groups.entry(uid).or_insert_with(|| {
            order.push(uid);
            UserGroup {
                slug,
                liked: false,
                owned: Vec::new(),
                followed: Vec::new(),
            }
        });
        match source.as_deref() {
            Some("liked") => g.liked = true,
            Some("playlist") => {
                if let (Some(pid), Some(name)) = (playlist_id, playlist_name) {
                    let pref = PlaylistRef {
                        id: pid,
                        name,
                        owner: owner_name.filter(|s| !s.is_empty()).unwrap_or_default(),
                    };
                    if is_owned == Some(1) {
                        g.owned.push(pref);
                    } else {
                        g.followed.push(pref);
                    }
                }
            }
            _ => {}
        }
    }
    let users: Vec<UserGroup> = order
        .into_iter()
        .filter_map(|uid| groups.remove(&uid))
        .collect();

    let duration_ms: Option<i64> = row.get("duration_ms");
    let duration = duration_ms
        .map(|ms| format!("{}:{:02}", ms / 60000, (ms % 60000) / 1000))
        .unwrap_or_else(|| "—".to_string());
    let isrc: String = row.get::<Option<String>, _>("isrc").unwrap_or_default();

    // Audio features (ReccoBeats) for this track.
    let feat = sqlx::query(
        "SELECT bpm, camelot, key_pitch, key_mode, energy, found
           FROM hub_track_features WHERE track_id = ?1",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten();
    let (bpm, music_key, energy, feat_note) = match feat {
        Some(r) if r.get::<i64, _>("found") == 1 => {
            let bpm: Option<f64> = r.get("bpm");
            let camelot: Option<String> = r.get("camelot");
            let pitch: Option<i64> = r.get("key_pitch");
            let mode: Option<i64> = r.get("key_mode");
            let energy: Option<f64> = r.get("energy");
            let bpm = bpm.map(|b| format!("{b:.0}")).unwrap_or_else(|| "—".into());
            let music_key = match (pitch, mode, camelot) {
                (Some(p), Some(m), Some(c)) => {
                    let m = if m == 1 { "maj" } else { "min" };
                    format!("{} {} · {c}", crate::features::key_name(p), m)
                }
                _ => "—".to_string(),
            };
            let energy = energy
                .map(|e| format!("{:.2}", e))
                .unwrap_or_else(|| "—".into());
            (bpm, music_key, energy, String::new())
        }
        Some(_) => (
            "—".into(),
            "—".into(),
            "—".into(),
            "ReccoBeats hat für diesen Track keine Audio-Features.".to_string(),
        ),
        None => (
            "—".into(),
            "—".into(),
            "—".into(),
            "Audio-Features noch nicht abgeglichen (Worker läuft).".to_string(),
        ),
    };

    // External service IDs for this track.
    let external_ids: Vec<ExternalId> = sqlx::query(
        "SELECT service, external_id, url FROM hub_track_external_ids
          WHERE track_id = ?1 ORDER BY service, external_id",
    )
    .bind(id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| {
        let service: String = r.get("service");
        let external_id: String = r.get("external_id");
        let mut url: String = r.get::<Option<String>, _>("url").unwrap_or_default();
        if url.is_empty() && service == "spotify" {
            url = format!("https://open.spotify.com/track/{external_id}");
        }
        ExternalId {
            service,
            external_id,
            url,
        }
    })
    .collect();

    // Resolved tags for this track, with owner and the groups each tag is in.
    let tag_rows = sqlx::query(
        "SELECT t.id AS id, t.name AS name, u.slug AS owner
           FROM hub_track_resolved_tags rt
           JOIN hub_tags t ON t.id = rt.tag_id
           JOIN hub_users u ON u.id = t.owner_user_id
          WHERE rt.track_id = ?1
          ORDER BY u.slug, t.name",
    )
    .bind(id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    let tag_ids: Vec<i64> = tag_rows.iter().map(|r| r.get::<i64, _>("id")).collect();
    let mut groups_by_tag: HashMap<i64, Vec<TrackTagGroup>> = HashMap::new();
    if !tag_ids.is_empty() {
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT gt.tag_id AS tag_id, g.id AS gid, g.name AS gname,
                    COALESCE(g.icon, '') AS gicon
               FROM hub_group_tags gt JOIN hub_tag_groups g ON g.id = gt.group_id
              WHERE gt.tag_id IN (",
        );
        let mut sep = qb.separated(", ");
        for tid in &tag_ids {
            sep.push_bind(*tid);
        }
        qb.push(") ORDER BY g.name COLLATE NOCASE");
        for r in qb.build().fetch_all(&st.pool).await.unwrap_or_default() {
            groups_by_tag
                .entry(r.get::<i64, _>("tag_id"))
                .or_default()
                .push(TrackTagGroup {
                    id: r.get("gid"),
                    name: r.get::<Option<String>, _>("gname").unwrap_or_default(),
                    icon: r.get::<Option<String>, _>("gicon").unwrap_or_default(),
                });
        }
    }
    let tags: Vec<TrackTag> = tag_rows
        .into_iter()
        .map(|r| {
            let id: i64 = r.get("id");
            TrackTag {
                groups: groups_by_tag.remove(&id).unwrap_or_default(),
                id,
                name: r.get::<Option<String>, _>("name").unwrap_or_default(),
                owner: r.get::<Option<String>, _>("owner").unwrap_or_default(),
            }
        })
        .collect();

    // Community genres (Last.fm), if fetched.
    let genres: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT genre FROM hub_track_genres WHERE track_id = ?1 AND genre <> '' ORDER BY genre",
    )
    .bind(id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    // music-api status from the cache — no per-track network call (AGENT.md §10).
    let mut avail_text = String::from("—");
    let mut fetchable = false;
    let mut deezer_id = String::new();
    if !isrc.is_empty() {
        match crate::music_api::cached_state(&st.pool, &isrc).await {
            Some(c) => {
                avail_text = c.label();
                fetchable = !c.ready();
                deezer_id = c.deezer_id.unwrap_or_default();
            }
            None => {
                avail_text = "noch nicht im music-api-Cache".to_string();
                fetchable = true;
            }
        }
    }

    let e = crate::settings::engine(&st.pool).await;
    let rip = crate::scoring::ripeness(&st.pool, id, &e).await;
    let my_tags = crate::tags::list_user_tags(&st.pool, nav.id).await;
    let recommended: Vec<RecTag> = crate::tags::recommended_tags(&st.pool, id, 12)
        .await
        .into_iter()
        .map(|(id, name, owner, score)| RecTag {
            id,
            name,
            owner,
            score,
        })
        .collect();
    let can_stream = st.cfg.music_api_token.is_some() && !isrc.trim().is_empty();
    let me = nav.slug.clone();
    let spotify_id: String = sqlx::query_scalar(
        "SELECT external_id FROM hub_track_external_ids WHERE track_id = ?1 AND service = 'spotify' LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten()
    .unwrap_or_default();

    let page = TrackPage {
        nav,
        id,
        title: row.get::<Option<String>, _>("title").unwrap_or_default(),
        artists: row.get::<Option<String>, _>("artists").unwrap_or_default(),
        album: row.get::<Option<String>, _>("album").unwrap_or_default(),
        duration,
        isrc,
        explicit: row.get::<Option<i64>, _>("explicit").unwrap_or(0) != 0,
        bpm,
        music_key,
        energy,
        feat_note,
        users,
        avail_text,
        deezer_id,
        fetchable,
        external_ids,
        tags,
        genres,
        flash: flash.msg.unwrap_or_default(),
        music_api: st.cfg.music_api_token.is_some(),
        ripeness_total: rip.total.round() as i64,
        ripeness_tag: rip.tag_score.round() as i64,
        ripeness_meta: rip.meta_score.round() as i64,
        ripeness_trak: format!("{:.2}", rip.traktor_score),
        meta_present: rip.meta_present,
        my_tags,
        recommended,
        can_stream,
        me,
        spotify_id,
    };

    match page.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => error_page(&format!("Template-Fehler: {e}")),
    }
}

#[derive(Deserialize)]
struct TrackTagForm {
    name: String,
}

/// Directly tag the track (track view = core element).
async fn track_tag(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<TrackTagForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "track").await else {
        return Redirect::to("/login").into_response();
    };
    if !f.name.trim().is_empty() {
        if let Ok(tag_id) = crate::tags::ensure_tag(&st.pool, nav.id, &f.name).await {
            let _ = crate::tags::tag_track(&st.pool, nav.id, id, tag_id).await;
        }
    }
    Redirect::to(&format!("/track/{id}")).into_response()
}

/// Remove a direct (manual) tag from the track.
async fn track_untag(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<TrackTagForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "track").await else {
        return Redirect::to("/login").into_response();
    };
    if let Some(tag_id) = crate::tags::find_tag_id_by_name(&st.pool, &f.name).await {
        let _ = crate::tags::untag_track(&st.pool, nav.id, id, tag_id).await;
    }
    Redirect::to(&format!("/track/{id}")).into_response()
}

/// A few recently seen tracks for the dashboard.
async fn recent_tracks(st: &AppState) -> Vec<RecentRow> {
    let rows = sqlx::query("SELECT id, artists, title FROM hub_tracks ORDER BY id DESC LIMIT 25")
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    rows.iter()
        .map(|r| RecentRow {
            id: r.get("id"),
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            artists: r.get::<Option<String>, _>("artists").unwrap_or_default(),
        })
        .collect()
}

async fn track_fetch(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if current_user(&st, &headers).await.is_none() {
        return Redirect::to("/login").into_response();
    }
    let isrc: Option<String> = sqlx::query_scalar("SELECT isrc FROM hub_tracks WHERE id = ?1")
        .bind(id)
        .fetch_optional(&st.pool)
        .await
        .ok()
        .flatten();
    let Some(isrc) = isrc.filter(|s| !s.is_empty()) else {
        return Redirect::to(&format!(
            "/track/{id}?msg={}",
            urlencoding::encode("kein ISRC vorhanden")
        ))
        .into_response();
    };
    let msg = match crate::music_api::order(&st.cfg, std::slice::from_ref(&isrc)).await {
        Ok(order) => format!("Bestellt ({order}) — music-api lädt"),
        Err(e) => format!("Fehler: {e}"),
    };
    Redirect::to(&format!("/track/{id}?msg={}", urlencoding::encode(&msg))).into_response()
}

async fn login_form(State(st): State<AppState>) -> Response {
    let open = crate::settings::registration_open(&st.pool).await;
    login_form_html(None, open)
}

async fn signup_form(State(st): State<AppState>) -> Response {
    let open = crate::settings::registration_open(&st.pool).await;
    signup_form_html(None, open)
}

async fn login_submit(State(st): State<AppState>, Form(c): Form<Creds>) -> Response {
    let username = c.username.trim();
    let row = sqlx::query("SELECT id, password_hash FROM hub_users WHERE slug = ?1 COLLATE NOCASE")
        .bind(username)
        .fetch_optional(&st.pool)
        .await
        .ok()
        .flatten();

    let ok = match &row {
        Some(r) => {
            let hash: Option<String> = r.get("password_hash");
            hash.map(|h| bcrypt::verify(&c.password, &h).unwrap_or(false))
                .unwrap_or(false)
        }
        None => false,
    };
    if !ok {
        let open = crate::settings::registration_open(&st.pool).await;
        return login_form_html(Some("Benutzername oder Passwort falsch."), open);
    }

    let uid: i64 = row.unwrap().get("id");
    match create_session(&st.pool, uid).await {
        Ok(tok) => with_cookie(Redirect::to("/").into_response(), session_cookie(&tok)),
        Err(e) => error_page(&format!("Session-Fehler: {e}")),
    }
}

async fn signup_submit(State(st): State<AppState>, Form(c): Form<Creds>) -> Response {
    if !crate::settings::registration_open(&st.pool).await {
        return signup_form_html(Some("Die Registrierung ist geschlossen."), false);
    }
    let username = c.username.trim();
    if username.len() < 2 || c.password.len() < 4 {
        return signup_form_html(
            Some("Benutzername (min. 2) und Passwort (min. 4) sind zu kurz."),
            true,
        );
    }

    let hash = match bcrypt::hash(&c.password, 10) {
        Ok(h) => h,
        Err(e) => return error_page(&format!("Hash-Fehler: {e}")),
    };

    let exists: Option<i64> =
        sqlx::query_scalar("SELECT id FROM hub_users WHERE slug = ?1 COLLATE NOCASE")
            .bind(username)
            .fetch_optional(&st.pool)
            .await
            .ok()
            .flatten();
    if exists.is_some() {
        return signup_form_html(Some("Benutzername ist schon vergeben."), true);
    }

    let uid: i64 = match sqlx::query_scalar(
        "INSERT INTO hub_users (slug, display_name, password_hash, created_at)
         VALUES (?1, ?1, ?2, ?3) RETURNING id",
    )
    .bind(username)
    .bind(&hash)
    .bind(now_iso())
    .fetch_one(&st.pool)
    .await
    {
        Ok(id) => id,
        Err(e) => return error_page(&format!("DB-Fehler: {e}")),
    };

    match create_session(&st.pool, uid).await {
        Ok(tok) => with_cookie(Redirect::to("/").into_response(), session_cookie(&tok)),
        Err(e) => error_page(&format!("Session-Fehler: {e}")),
    }
}

async fn logout() -> Response {
    with_cookie(Redirect::to("/login").into_response(), clear_cookie())
}

async fn connect(
    State(st): State<AppState>,
    Path(service): Path<String>,
    headers: HeaderMap,
) -> Response {
    if service != "spotify" {
        return error_page("Diesen Dienst gibt es (noch) nicht.");
    }
    let Some((uid, _)) = current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let Some(client_id) = st.cfg.spotify_client_id.clone() else {
        return error_page("SPOTIFY_CLIENT_ID ist auf dem Server nicht gesetzt.");
    };

    let (verifier, challenge) = spotify::pkce_pair();
    let state = spotify::random_state();
    st.oauth_states
        .lock()
        .unwrap()
        .insert(state.clone(), (uid, verifier));

    let url = spotify::authorize_url(&client_id, &st.cfg.spotify_redirect_uri, &state, &challenge);
    Redirect::to(&url).into_response()
}

#[derive(Deserialize)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

async fn callback(
    State(st): State<AppState>,
    Path(service): Path<String>,
    Query(p): Query<CallbackParams>,
) -> Response {
    if service != "spotify" {
        return error_page("Diesen Dienst gibt es (noch) nicht.");
    }
    if let Some(e) = p.error {
        return error_page(&format!("Spotify-Fehler: {e}"));
    }
    let (Some(code), Some(state)) = (p.code, p.state) else {
        return error_page("Callback ohne code/state.");
    };
    let entry = st.oauth_states.lock().unwrap().remove(&state);
    let Some((uid, verifier)) = entry else {
        return error_page("Unbekannter oder abgelaufener Login-Versuch.");
    };

    let (Some(client_id), Some(client_secret)) = (
        st.cfg.spotify_client_id.clone(),
        st.cfg.spotify_client_secret.clone(),
    ) else {
        return error_page("Spotify-Credentials fehlen auf dem Server.");
    };

    let tokens = match spotify::exchange_code(
        &client_id,
        &client_secret,
        &st.cfg.spotify_redirect_uri,
        &code,
        &verifier,
    )
    .await
    {
        Ok(t) => t,
        Err(e) => return error_page(&format!("Token-Austausch fehlgeschlagen: {e}")),
    };

    let (remote_id, display_name) =
        match spotify::api_get(&st.cfg.spotify_api_base, &tokens.access_token, "/me").await {
            Ok((200, me)) => (
                me["id"].as_str().map(str::to_string),
                me["display_name"].as_str().map(str::to_string),
            ),
            _ => (None, None),
        };

    if let Err(e) = crate::ingest::store_initial_tokens(
        &st.pool,
        uid,
        &tokens,
        remote_id.as_deref(),
        display_name.as_deref(),
    )
    .await
    {
        return error_page(&format!("Speichern fehlgeschlagen: {e}"));
    }

    Redirect::to("/").into_response()
}

async fn disconnect(
    State(st): State<AppState>,
    Path(service): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some((uid, _)) = current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = service;
    let _ = sqlx::query(
        "UPDATE hub_service_accounts
            SET access_token = NULL, refresh_token = NULL, token_expiry = NULL,
                connected_at = NULL, updated_at = ?2
          WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(uid)
    .bind(now_iso())
    .execute(&st.pool)
    .await;
    Redirect::to("/").into_response()
}
