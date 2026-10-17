//! Minimal web UI: username/password accounts, sessions, and "Connect Spotify".
//!
//! Uses the public HTTPS redirect (`https://…/api/hub/services/spotify/callback`)
//! instead of a loopback listener, so it works for any logged-in user in a browser.

use askama::Template;
use axum::Router;
use axum::extract::{Form, Path, Query, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::Value;
use sqlx::{QueryBuilder, Row, SqlitePool};

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
        .route("/track/{id}/actions", get(track_actions_frag))
        .route(
            "/track/{id}/actions/playlist",
            get(track_actions_playlist_frag).post(track_actions_playlist_post),
        )
        .route(
            "/track/{id}/actions/tag",
            get(track_actions_tag_frag).post(track_actions_tag_post),
        )
        .route("/api/hub/services/{service}/connect", get(connect))
        .route(
            "/api/hub/services/{service}/fetch-playlists",
            post(fetch_playlists_handler),
        )
        .route("/api/hub/services/{service}/sync-html", post(sync_html))
        .route("/api/hub/playlists/{id}/toggle", post(toggle_playlist))
        .route("/api/hub/playlists/{id}/refetch", post(refetch_playlist))
        .route("/now-playing", get(now_playing_frag))
        .route("/now-playing/queue", get(now_queue_frag))
        .route("/now-playing/queue/add/{id}", post(now_queue_add))
        .route("/now-playing/transport/{cmd}", post(now_transport))
        .route("/now-playing/play/tag/{id}", post(now_play_tag))
        .route("/now-playing/play/backpack/{id}", post(now_play_backpack))
        .route("/now-playing/tag", post(now_tag))
        .route("/now", get(now_page))
        .route("/curate", get(curate_page).post(curate_create))
        .route("/curate/{id}", get(curate_detail))
        .route("/curate/{id}/update", post(curate_update))
        .route("/curate/{id}/push", post(curate_push))
        .route("/curate/{id}/delete", post(curate_delete))
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
    artists_html: String,
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
    s_name: crate::table::SortHead,
    s_owner: crate::table::SortHead,
    s_tracks: crate::table::SortHead,
    s_created: crate::table::SortHead,
    s_last: crate::table::SortHead,
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
    /// Earliest known track `added_at` — Spotify exposes no playlist creation
    /// date, so this is the closest proxy (date only, or "—").
    created: String,
    /// Latest track `added_at` (date only, or "—" when nothing is fetched).
    last_added: String,
    /// When the playlist's items were last fetched (`fetched_at`, date only).
    updated: String,
    /// True when the remote snapshot changed since the last item fetch.
    changed: bool,
}

/// Format a Spotify ISO-8601 timestamp (`YYYY-MM-DDThh:mm:ssZ`) as a plain
/// date. Empty/`None`/non-ISO values yield `—`.
fn fmt_date(ts: Option<String>) -> String {
    match ts {
        Some(s) if s.len() >= 10 => s[..10].to_string(),
        _ => "\u{2014}".to_string(),
    }
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
    /// Recommendation scope (`mine` | `all`); unknown values fall back to `all`.
    #[serde(default)]
    scope: Option<String>,
    /// Engine tag scope (`own` | `collective`); absent = stored setting (#267).
    #[serde(default)]
    tag_scope: Option<String>,
}

/// Server-side playlist filter (issue #163). Unknown values fall back to `all`.
#[derive(Deserialize)]
struct PlaylistFilterQuery {
    filter: Option<String>,
    q: Option<String>,
    owner: Option<String>,
    /// Sort field (whitelisted: `name` | `owner` | `tracks`).
    sort: Option<String>,
    /// Sort direction (`asc`/`desc`).
    dir: Option<String>,
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
    RawQuery(raw): RawQuery,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "playlists").await else {
        return Redirect::to("/login").into_response();
    };
    let filter = normalise_filter(pf.filter.as_deref());
    let q = pf.q.unwrap_or_default().trim().to_string();
    let owner = pf.owner.unwrap_or_default().trim().to_string();
    let allowed: &[(&str, &str)] = &[
        ("name", "name"),
        ("owner", "owner_name"),
        ("tracks", "track_count"),
        ("created", "first_added"),
        ("last", "last_added"),
    ];
    let (sort, dir) = crate::table::resolve_sort(
        pf.sort.as_deref(),
        pf.dir.as_deref(),
        allowed,
        "name",
        "asc",
    );
    let playlists = playlist_rows(&st, nav.id, filter, &q, &owner, &sort, &dir).await;
    let counts = playlist_counts(&st, nav.id).await;
    let owner_options = playlist_owners(&st, nav.id, &owner).await;
    let rawq = raw.as_deref().unwrap_or("");
    let s_name = crate::table::sort_head(rawq, "name", "Playlist", &sort, &dir);
    let s_owner = crate::table::sort_head(rawq, "owner", "Besitzer", &sort, &dir);
    let s_tracks = crate::table::sort_head(rawq, "tracks", "Tracks", &sort, &dir);
    let s_created = crate::table::sort_head(rawq, "created", "Angelegt", &sort, &dir);
    let s_last = crate::table::sort_head(rawq, "last", "Letzte Änderung", &sort, &dir);
    render(&PlaylistsPage {
        nav,
        flash: flash.msg.unwrap_or_default(),
        filter: filter.to_string(),
        q,
        owner_options,
        count: playlists.len(),
        playlists,
        counts,
        s_name,
        s_owner,
        s_tracks,
        s_created,
        s_last,
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
    let snapshot_id: Option<String> = r.get("snapshot_id");
    let last_synced: Option<String> = r.get("last_synced_snapshot");
    // Changed = we have items but the remote snapshot moved since we last fetched.
    let changed = items_available == 1
        && snapshot_id.as_deref().is_some_and(|s| !s.is_empty())
        && snapshot_id != last_synced;

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
        created: fmt_date(r.get::<Option<String>, _>("first_added")),
        last_added: fmt_date(r.get::<Option<String>, _>("last_added")),
        updated: fmt_date(r.get::<Option<String>, _>("fetched_at")),
        changed,
    }
}

/// The user's playlists (no pagination) as rows for `playlists.html`.
///
/// `filter` is one of the keys returned by [`normalise_filter`]; the matching
/// `WHERE` fragment is appended before the fuzzy search + whitelisted sort.
async fn playlist_rows(
    st: &AppState,
    user_id: i64,
    filter: &str,
    q: &str,
    owner: &str,
    sort: &str,
    dir: &str,
) -> Vec<PlaylistRow> {
    let allowed: &[(&str, &str)] = &[
        ("name", "name"),
        ("owner", "owner_name"),
        ("tracks", "track_count"),
        ("created", "first_added"),
        ("last", "last_added"),
    ];
    let mut qb: QueryBuilder<sqlx::Sqlite> = QueryBuilder::new(
        "SELECT * FROM (SELECT p.id AS id, p.name AS name, p.is_owned AS is_owned,
                p.track_count AS track_count, p.items_available AS items_available,
                p.enabled_for_fetch AS enabled_for_fetch, p.fetch_error AS fetch_error,
                p.snapshot_id AS snapshot_id, p.last_synced_snapshot AS last_synced_snapshot,
                p.fetched_at AS fetched_at,
                COALESCE(NULLIF(p.owner_name, ''), CASE WHEN p.is_owned = 1 THEN a.display_name END) AS owner_name,
                u.slug AS user_slug,
                (SELECT COUNT(*) FROM hub_playlist_tracks t WHERE t.playlist_id = p.id) AS fetched,
                (SELECT MIN(t.added_at) FROM hub_playlist_tracks t WHERE t.playlist_id = p.id) AS first_added,
                (SELECT MAX(t.added_at) FROM hub_playlist_tracks t WHERE t.playlist_id = p.id) AS last_added,
                (SELECT GROUP_CONCAT(tg.name, ', ') FROM hub_tag_sources s
                   JOIN hub_tags tg ON tg.id = s.tag_id
                  WHERE s.playlist_id = p.id AND tg.owner_user_id = ",
    );
    // `?1` in `filter_clause` refers to this first bind (the current user).
    qb.push_bind(user_id);
    qb.push(
        ") AS my_tags
           FROM hub_playlists p
           JOIN hub_users u ON u.id = p.user_id
           LEFT JOIN hub_service_accounts a ON a.user_id = p.user_id AND a.service = 'spotify'
          WHERE p.user_id = ",
    );
    qb.push_bind(user_id);
    if !owner.is_empty() {
        qb.push(
            " AND COALESCE(NULLIF(p.owner_name, ''), CASE WHEN p.is_owned = 1 THEN a.display_name END) = ",
        )
        .push_bind(owner.to_string());
    }
    qb.push(filter_clause(filter));
    qb.push(") WHERE 1 = 1");
    // Fuzzy full-text over playlist name + owner (order-independent).
    crate::table::push_fuzzy(&mut qb, q, &["name", "owner_name"]);
    crate::table::order_by(&mut qb, sort, dir, allowed, "name");
    qb.push(" LIMIT 5000");

    let rows = qb.build().fetch_all(&st.pool).await.unwrap_or_default();
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
                p.snapshot_id AS snapshot_id, p.last_synced_snapshot AS last_synced_snapshot,
                p.fetched_at AS fetched_at,
                COALESCE(NULLIF(p.owner_name, ''), CASE WHEN p.is_owned = 1 THEN a.display_name END) AS owner_name,
                u.slug AS user_slug,
                (SELECT COUNT(*) FROM hub_playlist_tracks t WHERE t.playlist_id = p.id) AS fetched,
                (SELECT MIN(t.added_at) FROM hub_playlist_tracks t WHERE t.playlist_id = p.id) AS first_added,
                (SELECT MAX(t.added_at) FROM hub_playlist_tracks t WHERE t.playlist_id = p.id) AS last_added,
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

/// Force a single playlist to be re-fetched by the worker (429-safe backend task).
/// Clears the sync marker so the incremental job picks it up again.
async fn refetch_playlist(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some((uid, _)) = current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = sqlx::query(
        "UPDATE hub_playlists
            SET enabled_for_fetch = 1, items_available = 0,
                last_synced_snapshot = NULL, fetch_status = 'queued', fetch_error = NULL
          WHERE id = ?1 AND user_id = ?2",
    )
    .bind(id)
    .bind(uid)
    .execute(&st.pool)
    .await;

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
    album: String,
    album_href: String,
    album_release: String,
    album_external: Vec<ExtId>,
    duration: String,
    isrc: String,
    explicit: bool,
    /// Service the track was ingested from (`spotify` / `soundcloud` / …).
    service: String,
    /// Whether the track has an ISRC (gates the vault section).
    has_isrc: bool,
    /// Whether the live vault read succeeded (gates RESOLVED + SATELLITES).
    has_vault: bool,
    /// The vault's frozen per-track view, rendered transparently.
    vault: VaultView,
    /// Artists for the LINKS section (vault-driven, resolved to hub ids).
    link_artists: Vec<LinkArtist>,
    /// Distinct playlists that hold this track (LINKS: "wer hat's").
    link_playlists: Vec<PlaylistRef>,
    users: Vec<UserGroup>,
    fetchable: bool,
    external_ids: Vec<ExternalId>,
    genres: Vec<String>,
    flash: String,
    // Listening (hub-own).
    listening: Vec<ListenRow>,
    listen_total: i64,
    // Ripeness (issues #214/#215).
    ripeness_total: i64,
    ripeness_tag: i64,
    ripeness_meta: i64,
    ripeness_trak: String,
    meta_present: Vec<(String, bool)>,
    // Core-element additions: direct tagging + player + recommendations.
    recommended: Vec<RecTag>,
    rec_scope: String,
    rec_scope_mine_url: String,
    rec_scope_all_url: String,
    can_stream: bool,
    spotify_id: String,
    tag_clouds: Vec<TagCloudRow>,
    untag_action: String,
    tag_action: String,
    /// htmx swap target for the unified tag input ("" = native POST/reload).
    tag_target: String,
    /// Legacy recommendation scope carried through the tag input (`mine` | `all`).
    rec_scope_param: String,
    /// Engine tag scope (#267) carried through the tag input (`own` | `collective`).
    tag_scope: String,
    tag_scope_own_url: String,
    tag_scope_collective_url: String,
    htmx: bool,
    my_types: Vec<(String, String)>,
    /// Ready download formats (label + the `format` param) — empty when nothing
    /// is ready yet (then `download_hint` carries the passive state, #460).
    download_options: Vec<DownloadOption>,
    /// Passive hint shown instead of buttons when nothing is available yet.
    download_hint: String,
}

/// One ready download format offered on the track page (#460).
struct DownloadOption {
    format: String,
    label: String,
}

struct TagCloudRow {
    type_label: String,
    icon: String,
    chips: Vec<TagChipRow>,
}

struct TagChipRow {
    id: i64,
    name: String,
    mine: bool,
}

fn type_options() -> Vec<(String, String)> {
    crate::tagtypes::options()
        .into_iter()
        .map(|(value, label, _icon)| (value.to_string(), label.to_string()))
        .collect()
}

struct RecTag {
    name: String,
    type_label: String,
    reason: String,
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

/// A tag on the track's detail page: owner + its type.
struct TrackTag {
    id: i64,
    name: String,
    owner: String,
    type_label: String,
    type_icon: String,
}

/// A provider id (service + value + optional link) shown in the LINKS section.
struct ExtId {
    service: String,
    value: String,
    url: String,
}

/// One artist link in the LINKS section: hub page (when known) + provider ids.
struct LinkArtist {
    name: String,
    /// `/artist/{id}` when the hub knows the artist, else empty.
    href: String,
    external: Vec<ExtId>,
}

/// One recent listen (hub-own listening history).
struct ListenRow {
    when: String,
    who: String,
}

/// One analyzer satellite row (per-analyzer analysis result).
struct VaultAnalysisRow {
    analyzer: String,
    bpm: String,
    key: String,
    camelot: String,
    energy: String,
    load_date: String,
}

/// One meta satellite row (per record source).
struct VaultMetaRow {
    record_source: String,
    title: String,
    artist: String,
    album: String,
    duration: String,
    load_date: String,
}

/// Template-facing view of the vault's frozen `/vault/track/{isrc}` response:
/// the RESOLVED best value plus the transparent SATELLITES (analysis / download
/// / meta), each with its source + load date.
#[derive(Default)]
struct VaultView {
    // RESOLVED (precedence winner).
    bpm: String,
    key: String,
    camelot: String,
    energy: String,
    source: String,
    has_resolved: bool,
    // SATELLITES.
    analysis: Vec<VaultAnalysisRow>,
    has_download: bool,
    download_state: String,
    download_format: String,
    download_formats: String,
    download_load_date: String,
    meta: Vec<VaultMetaRow>,
}

/// The `_partials/now_playing.html` widget (issue #293/#294).
#[derive(Template, Default)]
#[template(path = "_partials/now_playing.html")]
struct NowPlaying {
    logged_in: bool,
    connected: bool,
    /// A Spotify error to show instead of the track (`""` = none).
    error: String,
    /// We have a track object at all (drives the "nothing playing" branch).
    playing: bool,
    /// That track is actually playing (not paused) — drives play/pause.
    is_playing: bool,
    has_track: bool,
    hub_track_id: i64,
    track_url: String,
    play_state: String,
    title: String,
    artists_html: String,
    album: String,
    image: String,
    progress_pct: i64,
    progress_disp: String,
    duration_disp: String,
    bpm: String,
    music_key: String,
    tag_clouds: Vec<TagCloudRow>,
}

/// Uniform per-track action group (#318) + its lazily-loaded forms.
#[derive(Template)]
#[template(path = "_partials/track_actions.html")]
struct TrackActions {
    id: i64,
}

#[derive(Template)]
#[template(path = "_partials/track_playlist_picker.html")]
struct TrackPlaylistPicker {
    id: i64,
    curated: Vec<PlaylistMini>,
    spotify: Vec<PlaylistMini>,
    write_ready: bool,
}

struct PlaylistMini {
    id: i64,
    name: String,
    name_lower: String,
}

#[derive(Template)]
#[template(path = "_partials/track_tag_form.html")]
struct TrackTagFormPartial {
    id: i64,
    tag_action: String,
    tag_target: String,
    /// Legacy recommendation scope for the tag input `data-scope`.
    rec_scope_param: String,
    /// Engine tag scope (#267) for the tag input `data-tag-scope`.
    tag_scope: String,
    my_types: Vec<(String, String)>,
}

/// Full-screen "Now Playing" page (`/now`), rendered when the current track has
/// no hub counterpart (or nothing is playing). Known tracks render the real
/// track page instead.
#[derive(Template)]
#[template(path = "now.html")]
struct NowPage {
    nav: crate::ui::Nav,
    flash: String,
    message: String,
    connect_link: bool,
    has_external: bool,
    title: String,
    artists_html: String,
    album: String,
    image: String,
    progress_pct: i64,
    progress_disp: String,
    duration_disp: String,
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
    match build_track_page(
        &st,
        nav,
        id,
        flash.msg.unwrap_or_default(),
        crate::recommend::RecScope::parse(flash.scope.as_deref()),
        flash.scope.is_some(),
        flash.tag_scope.as_deref(),
    )
    .await
    {
        Some(page) => render_track(page),
        None => track_not_found(),
    }
}

/// `2026-01-01T00:00:00+00:00` -> `2026-01-01 00:00` (empty when absent).
fn fmt_ts(ts: Option<String>) -> String {
    match ts {
        Some(s) if s.len() >= 16 => s[..16].replace('T', " "),
        Some(s) => s,
        None => String::new(),
    }
}

/// Best-effort external URL for a provider id (`artist` / `album`).
fn provider_url(kind: &str, service: &str, id: &str) -> String {
    match service {
        "spotify" => format!("https://open.spotify.com/{kind}/{id}"),
        "deezer" => format!("https://www.deezer.com/{kind}/{id}"),
        _ => String::new(),
    }
}

/// Build the full track-detail page model, or `None` when the track is unknown.
/// Shared by the track page and the Now Playing page.
async fn build_track_page(
    st: &AppState,
    nav: crate::ui::Nav,
    id: i64,
    flash: String,
    scope: crate::recommend::RecScope,
    scope_explicit: bool,
    tag_scope_raw: Option<&str>,
) -> Option<TrackPage> {
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
        return None;
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

    let isrc: String = row.get::<Option<String>, _>("isrc").unwrap_or_default();
    let has_isrc = !isrc.trim().is_empty();
    let service: String = row.get::<Option<String>, _>("service").unwrap_or_default();
    let hub_album: String = row.get::<Option<String>, _>("album").unwrap_or_default();

    // ── One live read of the Data Vault (music-api) ──────────────────────────
    // The hub is a pure consumer: HUB identity, RESOLVED best value, LINKS and
    // the transparent SATELLITES all come from this single frozen response — no
    // second cache, no divergent live panel. ISRC-less tracks skip it entirely.
    let vault = if has_isrc {
        crate::music_api::vault_track(&st.cfg, &isrc).await
    } else {
        None
    };
    let has_vault = vault.is_some();

    // HUB identity: duration/explicit come from the hub row, enriched by the
    // vault's own identity block when the hub lacks them.
    let vault_hub = vault.as_ref().map(|v| v.hub.clone()).unwrap_or_default();
    let duration_ms: Option<i64> = row
        .get::<Option<i64>, _>("duration_ms")
        .or(vault_hub.duration_ms);
    let duration = duration_ms
        .map(|ms| format!("{}:{:02}", ms / 60000, (ms % 60000) / 1000))
        .unwrap_or_else(|| "—".to_string());
    let explicit = row.get::<Option<i64>, _>("explicit").unwrap_or(0) != 0
        || vault_hub.explicit.unwrap_or(false);

    // RESOLVED + SATELLITES from the vault.
    let mut view = VaultView::default();
    if let Some(v) = vault.as_ref() {
        view.bpm = v
            .resolved
            .bpm
            .map(|b| format!("{b:.0}"))
            .unwrap_or_else(|| "—".into());
        view.key = v.resolved.key.clone().unwrap_or_else(|| "—".into());
        view.camelot = v.resolved.camelot.clone().unwrap_or_default();
        view.energy = v
            .resolved
            .energy
            .map(|e| format!("{e:.2}"))
            .unwrap_or_else(|| "—".into());
        view.source = v.resolved.source.clone().unwrap_or_default();
        view.has_resolved = v.resolved.bpm.is_some()
            || v.resolved.key.is_some()
            || v.resolved.camelot.is_some()
            || v.resolved.energy.is_some();
        view.analysis = v
            .satellites
            .analysis
            .iter()
            .map(|a| VaultAnalysisRow {
                analyzer: a.analyzer.clone().unwrap_or_default(),
                bpm: a.bpm.map(|b| format!("{b:.0}")).unwrap_or_default(),
                key: a.key.clone().unwrap_or_default(),
                camelot: a.camelot.clone().unwrap_or_default(),
                energy: a.energy.map(|e| format!("{e:.2}")).unwrap_or_default(),
                load_date: fmt_ts(a.load_date.clone()),
            })
            .collect();
        if let Some(d) = v.satellites.download.as_ref() {
            view.has_download = true;
            view.download_state = d.state.clone().unwrap_or_default();
            view.download_format = d.source_format.clone().unwrap_or_default();
            view.download_formats = d.formats.join(", ");
            view.download_load_date = fmt_ts(d.load_date.clone());
        }
        view.meta = v
            .satellites
            .meta
            .iter()
            .map(|m| VaultMetaRow {
                record_source: m.record_source.clone().unwrap_or_default(),
                title: m.title.clone().unwrap_or_default(),
                artist: m.artist.clone().unwrap_or_default(),
                album: m.album.clone().unwrap_or_default(),
                duration: m
                    .duration_ms
                    .map(|ms| format!("{}:{:02}", ms / 60000, (ms % 60000) / 1000))
                    .unwrap_or_default(),
                load_date: fmt_ts(m.load_date.clone()),
            })
            .collect();
    }

    // LINKS: artists (vault-driven, resolved to hub artist pages) + album.
    let hub_artists = crate::artists::artists_for_track(&st.pool, id).await;
    let link_artists: Vec<LinkArtist> = match vault.as_ref() {
        Some(v) if !v.links.artists.is_empty() => {
            let mut out = Vec::with_capacity(v.links.artists.len());
            for a in &v.links.artists {
                let name = a.name.clone().unwrap_or_default();
                let href = match crate::artists::artist_id_by_name(&st.pool, &name).await {
                    Some(aid) => format!("/artist/{aid}"),
                    None => String::new(),
                };
                out.push(LinkArtist {
                    name,
                    href,
                    external: a
                        .external_ids
                        .iter()
                        .map(|(svc, val)| ExtId {
                            service: svc.clone(),
                            value: val.clone(),
                            url: provider_url("artist", svc, val),
                        })
                        .collect(),
                });
            }
            out
        }
        _ => hub_artists
            .iter()
            .map(|(aid, name)| LinkArtist {
                name: name.clone(),
                href: format!("/artist/{aid}"),
                external: Vec::new(),
            })
            .collect(),
    };

    // Album link: prefer the vault's album title, fall back to the hub's.
    let vault_album = vault.as_ref().and_then(|v| v.links.album.clone());
    let album = vault_album
        .as_ref()
        .and_then(|a| a.title.clone())
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(hub_album);
    let album_href = if album.trim().is_empty() {
        String::new()
    } else {
        format!("/album/{}", urlencoding::encode(&album))
    };
    let album_release = vault_album
        .as_ref()
        .and_then(|a| a.release_date.clone())
        .unwrap_or_default();
    let album_external: Vec<ExtId> = vault_album
        .as_ref()
        .map(|a| {
            a.external_ids
                .iter()
                .map(|(svc, val)| ExtId {
                    service: svc.clone(),
                    value: val.clone(),
                    url: provider_url("album", svc, val),
                })
                .collect()
        })
        .unwrap_or_default();

    // Distinct playlists that hold this track (LINKS: "wer hat's").
    let mut link_playlists: Vec<PlaylistRef> = Vec::new();
    let mut seen_pl: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for u in &users {
        for p in u.owned.iter().chain(u.followed.iter()) {
            if seen_pl.insert(p.id) {
                link_playlists.push(PlaylistRef {
                    id: p.id,
                    name: p.name.clone(),
                    owner: p.owner.clone(),
                });
            }
        }
    }

    // Download availability (drives the player's "Jetzt holen" fallback).
    let download_ready = view.has_download && view.download_state == "ready";
    let fetchable = has_isrc && st.cfg.music_api_token.is_some() && !download_ready;
    // Download controls (#460): buttons only for what the vault says is ready,
    // otherwise a passive hint — never a bare link that can 502.
    let (download_options, download_hint) = track_download_options(
        &view,
        download_ready,
        has_isrc,
        st.cfg.music_api_token.is_some(),
    );

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

    // Resolved tags for this track, with owner and type.
    let tag_rows = sqlx::query(
        "SELECT t.id AS id, t.name AS name, u.slug AS owner, COALESCE(t.type,'') AS type
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

    let tags: Vec<TrackTag> = tag_rows
        .into_iter()
        .map(|r| {
            let ty: String = r.get::<Option<String>, _>("type").unwrap_or_default();
            let parsed = crate::tagtypes::TagType::parse(&ty);
            TrackTag {
                id: r.get("id"),
                name: r.get::<Option<String>, _>("name").unwrap_or_default(),
                owner: r.get::<Option<String>, _>("owner").unwrap_or_default(),
                type_label: parsed.map(|t| t.label().to_string()).unwrap_or_default(),
                type_icon: parsed.map(|t| t.icon().to_string()).unwrap_or_default(),
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

    // Listening history for this track (hub-own, not from the vault).
    let listen_total: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_listening_history WHERE track_id = ?1")
            .bind(id)
            .fetch_one(&st.pool)
            .await
            .unwrap_or(0);
    let listening: Vec<ListenRow> = sqlx::query(
        "SELECT h.played_at, u.slug AS who
           FROM hub_listening_history h
           JOIN hub_users u ON u.id = h.user_id
          WHERE h.track_id = ?1
          ORDER BY h.played_at DESC
          LIMIT 8",
    )
    .bind(id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| ListenRow {
        when: fmt_ts(r.get::<Option<String>, _>("played_at")),
        who: r.get::<Option<String>, _>("who").unwrap_or_default(),
    })
    .collect();

    let e = crate::settings::engine(&st.pool).await;
    let requested = crate::scope::requested(&st.pool, nav.id, tag_scope_raw).await;
    let owners = crate::scope::detail_owners(
        &st.pool,
        nav.id,
        scope.is_mine(),
        scope_explicit,
        requested,
    )
    .await;
    let rip = crate::scoring::ripeness(&st.pool, id, &e, &owners).await;
    let recommended: Vec<RecTag> =
        crate::recommend::recommend_tags_scoped(&st.pool, id, 12, &owners)
            .await
            .into_iter()
            .map(|r| RecTag {
                reason: r.why(),
                type_label: r.type_label,
                name: r.name,
            })
            .collect();
    let rec_scope = scope.as_str().to_string();
    let rec_scope_mine_url = format!("/track/{id}?scope=mine");
    let rec_scope_all_url = format!("/track/{id}?scope=all");
    let tag_scope_val = crate::scope::effective(&st.pool, nav.id, tag_scope_raw)
        .await
        .as_str()
        .to_string();
    let mut tag_scope_own_url = format!("/track/{id}?tag_scope=own");
    let mut tag_scope_collective_url = format!("/track/{id}?tag_scope=collective");
    if scope.is_mine() {
        tag_scope_own_url.push_str("&scope=mine");
        tag_scope_collective_url.push_str("&scope=mine");
    }
    let can_stream = st.cfg.music_api_token.is_some() && !isrc.trim().is_empty();
    let me = nav.slug.clone();
    // Bucket the track's tags into clouds by their type.
    let tag_clouds: Vec<TagCloudRow> = {
        let mut map: std::collections::BTreeMap<String, (String, Vec<TagChipRow>)> =
            std::collections::BTreeMap::new();
        for t in &tags {
            let (gname, gicon) = if t.type_label.is_empty() {
                ("Ohne Typ".to_string(), "•".to_string())
            } else {
                (
                    t.type_label.clone(),
                    if t.type_icon.trim().is_empty() {
                        "•".to_string()
                    } else {
                        t.type_icon.clone()
                    },
                )
            };
            let e = map.entry(gname).or_insert_with(|| (gicon, Vec::new()));
            e.1.push(TagChipRow {
                id: t.id,
                name: t.name.clone(),
                mine: t.owner.eq_ignore_ascii_case(&me),
            });
        }
        let mut clouds: Vec<TagCloudRow> = map
            .into_iter()
            .map(|(type_label, (icon, chips))| TagCloudRow {
                type_label,
                icon,
                chips,
            })
            .collect();
        if let Some(pos) = clouds.iter().position(|c| c.type_label == "Ohne Typ") {
            let last = clouds.remove(pos);
            clouds.push(last);
        }
        clouds
    };
    let spotify_id: String = sqlx::query_scalar(
        "SELECT external_id FROM hub_track_external_ids WHERE track_id = ?1 AND service = 'spotify' LIMIT 1",
    )
    .bind(id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten()
    .unwrap_or_default();

    let my_types = type_options();

    // "tagged-complete" flag (#345): auto (all configured families tagged) or
    // manual (user marked it done).
    Some(TrackPage {
        nav,
        id,
        title: row.get::<Option<String>, _>("title").unwrap_or_default(),
        album,
        album_href,
        album_release,
        album_external,
        duration,
        isrc,
        explicit,
        service,
        has_isrc,
        has_vault,
        vault: view,
        link_artists,
        link_playlists,
        users,
        fetchable,
        external_ids,
        genres,
        flash,
        listening,
        listen_total,
        ripeness_total: rip.total.round() as i64,
        ripeness_tag: rip.tag_score.round() as i64,
        ripeness_meta: rip.meta_score.round() as i64,
        ripeness_trak: format!("{:.2}", rip.traktor_score),
        meta_present: rip.meta_present,
        recommended,
        rec_scope: rec_scope.clone(),
        rec_scope_mine_url,
        rec_scope_all_url,
        can_stream,
        spotify_id,
        tag_clouds,
        untag_action: format!("/track/{id}/untag"),
        tag_action: format!("/track/{id}/tag"),
        tag_target: String::new(),
        rec_scope_param: rec_scope.clone(),
        tag_scope: tag_scope_val,
        tag_scope_own_url,
        tag_scope_collective_url,
        htmx: false,
        my_types,
        download_options,
        download_hint,
    })
}

/// Download controls for the track page (#460).
///
/// Ready formats become buttons — coherent labels, in the vault's own vocabulary;
/// anything not ready yields a passive hint instead. We already know the state at
/// render time, so there are never dead buttons (and no manual "holen" — the
/// demand covers the whole library, ADR-068).
fn track_download_options(
    vault: &VaultView,
    ready: bool,
    has_isrc: bool,
    music_api: bool,
) -> (Vec<DownloadOption>, String) {
    if !has_isrc {
        return (Vec::new(), String::new());
    }
    if !music_api {
        return (Vec::new(), "music-api nicht konfiguriert.".to_string());
    }
    if !ready {
        let state = vault.download_state.trim();
        let hint = if state.is_empty() {
            "Noch nicht verfügbar — wird automatisch geholt.".to_string()
        } else {
            format!("Noch nicht verfügbar ({state}) — wird automatisch geholt.")
        };
        return (Vec::new(), hint);
    }
    let mut tokens: Vec<&str> = vault
        .download_formats
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let src = vault.download_format.trim();
    if !src.is_empty() && !tokens.contains(&src) {
        tokens.push(src);
    }
    let mut out: Vec<DownloadOption> = Vec::new();
    for tok in tokens {
        if let Some((format, label)) = download_format(tok) {
            if !out.iter().any(|o| o.format == format) {
                out.push(DownloadOption {
                    format: format.to_string(),
                    label: label.to_string(),
                });
            }
        }
    }
    if out.is_empty() {
        // Ready, but no format token we recognise: the download endpoint falls
        // back across formats anyway, so offer one generic control.
        out.push(DownloadOption {
            format: "flac".to_string(),
            label: "Audio".to_string(),
        });
    }
    (out, String::new())
}

/// Map a vault/download format token to `(hub format param, label)`.
fn download_format(token: &str) -> Option<(&'static str, &'static str)> {
    Some(match token {
        "flac" => ("flac", "FLAC"),
        "mp3" | "320" | "mp3-320" => ("mp3-320", "MP3 320"),
        "128" | "mp3-128" => ("mp3-128", "MP3 128"),
        "wav" => ("wav", "WAV"),
        "m4a" | "aac" => ("m4a", "M4A"),
        _ => return None,
    })
}

fn render_track(page: TrackPage) -> Response {
    match page.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => error_page(&format!("Template-Fehler: {e}")),
    }
}

fn track_not_found() -> Response {
    (
        StatusCode::NOT_FOUND,
        render(&ErrorPage {
            message: "Track nicht gefunden.".to_string(),
        }),
    )
        .into_response()
}

#[derive(Deserialize)]
struct TrackTagForm {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    tag_type: Option<String>,
    /// Existing tag to link (the unified tag input, #449).
    #[serde(default)]
    tag_id: Option<i64>,
    /// Recommendation scope to preserve across the redirect (`mine` | `all`).
    #[serde(default)]
    scope: Option<String>,
    /// Engine tag scope to preserve across the redirect (`own` | `collective`).
    #[serde(default)]
    tag_scope: Option<String>,
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
    if let Some(tag_id) = crate::tags::resolve_tag_for_add(
        &st.pool,
        nav.id,
        f.tag_id,
        Some(&f.name),
        f.tag_type.as_deref(),
    )
    .await
    {
        let _ = crate::tags::tag_track(&st.pool, &st.cfg, nav.id, id, tag_id).await;
    }
    let scope = crate::recommend::RecScope::parse(f.scope.as_deref());
    let tag_scope_q = crate::scope::TagScope::parse(f.tag_scope.as_deref())
        .map(|s| format!("&tag_scope={}", s.as_str()))
        .unwrap_or_default();
    Redirect::to(&format!("/track/{id}?scope={}{tag_scope_q}", scope.as_str())).into_response()
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
        let _ = crate::tags::untag_track(&st.pool, &st.cfg, nav.id, id, tag_id).await;
    }
    let scope = crate::recommend::RecScope::parse(f.scope.as_deref());
    let tag_scope_q = crate::scope::TagScope::parse(f.tag_scope.as_deref())
        .map(|s| format!("&tag_scope={}", s.as_str()))
        .unwrap_or_default();
    Redirect::to(&format!("/track/{id}?scope={}{tag_scope_q}", scope.as_str())).into_response()
}

// ── now playing (issue #293/#294) ───────────────────────────────────────────

/// Build the track's tag clouds (grouped by tag type) for the widget.
async fn now_tag_clouds(st: &AppState, track_id: i64, me: &str) -> Vec<TagCloudRow> {
    let rows = sqlx::query(
        "SELECT t.id AS id, t.name AS name, u.slug AS owner, COALESCE(t.type,'') AS type
           FROM hub_track_resolved_tags rt
           JOIN hub_tags t ON t.id = rt.tag_id
           JOIN hub_users u ON u.id = t.owner_user_id
          WHERE rt.track_id = ?1
          ORDER BY u.slug, t.name",
    )
    .bind(track_id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    let mut map: std::collections::BTreeMap<String, (String, Vec<TagChipRow>)> =
        std::collections::BTreeMap::new();
    for r in rows {
        let ty: String = r.get::<Option<String>, _>("type").unwrap_or_default();
        let owner: String = r.get::<Option<String>, _>("owner").unwrap_or_default();
        let (label, icon) = match crate::tagtypes::TagType::parse(&ty) {
            Some(t) => {
                let i = t.icon().to_string();
                (
                    t.label().to_string(),
                    if i.trim().is_empty() {
                        "•".to_string()
                    } else {
                        i
                    },
                )
            }
            None => ("Ohne Typ".to_string(), "•".to_string()),
        };
        let e = map.entry(label).or_insert_with(|| (icon, Vec::new()));
        e.1.push(TagChipRow {
            id: r.get("id"),
            name: r.get::<Option<String>, _>("name").unwrap_or_default(),
            mine: owner.eq_ignore_ascii_case(me),
        });
    }
    let mut clouds: Vec<TagCloudRow> = map
        .into_iter()
        .map(|(type_label, (icon, chips))| TagCloudRow {
            type_label,
            icon,
            chips,
        })
        .collect();
    if let Some(pos) = clouds.iter().position(|c| c.type_label == "Ohne Typ") {
        let last = clouds.remove(pos);
        clouds.push(last);
    }
    clouds
}

/// Render the now-playing widget for `nav`'s user.
async fn render_now(st: &AppState, nav: &crate::ui::Nav) -> Response {
    if !crate::nowplaying::has_scope(&st.pool, nav.id).await {
        return render(&NowPlaying {
            logged_in: true,
            connected: false,
            ..Default::default()
        });
    }
    match crate::nowplaying::current(&st.pool, &st.cfg, nav.id).await {
        Ok(None) => render(&NowPlaying {
            logged_in: true,
            connected: true,
            ..Default::default()
        }),
        Ok(Some(n)) => {
            let has_track = n.hub_track_id.is_some();
            let (tag_clouds, hub_track_id, track_url, bpm, music_key) = match n.hub_track_id {
                Some(tid) => {
                    let (b, k) = track_bpm_key(st, tid).await;
                    (
                        now_tag_clouds(st, tid, &nav.slug).await,
                        tid,
                        format!("/track/{tid}"),
                        b,
                        k,
                    )
                }
                None => (
                    Vec::new(),
                    0,
                    String::new(),
                    "—".to_string(),
                    "—".to_string(),
                ),
            };
            let artists_html = match n.hub_track_id {
                Some(tid) => {
                    let linked = crate::artists::artists_for_track(&st.pool, tid).await;
                    if linked.is_empty() {
                        crate::artists::escape_html(&n.artists)
                    } else {
                        crate::artists::artist_links_html(&linked)
                    }
                }
                None => crate::artists::escape_html(&n.artists),
            };
            render(&NowPlaying {
                logged_in: true,
                connected: true,
                playing: true,
                is_playing: n.playing,
                has_track,
                hub_track_id,
                track_url,
                play_state: if n.playing { "Läuft" } else { "Pausiert" }.to_string(),
                title: n.title.clone(),
                artists_html,
                album: n.album.clone(),
                image: n.image.clone(),
                progress_pct: n.progress_pct(),
                progress_disp: n.progress_disp(),
                duration_disp: n.duration_disp(),
                bpm,
                music_key,
                tag_clouds,
                error: String::new(),
            })
        }
        Err(e) => render(&NowPlaying {
            logged_in: true,
            connected: true,
            error: format!("Spotify-Fehler: {e}"),
            ..Default::default()
        }),
    }
}

/// htmx fragment: the current Spotify track + inline tagging (#293/#294).
async fn now_playing_frag(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "now").await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    render_now(&st, &nav).await
}

/// The Spotify "Next up" queue widget (`_partials/now_queue.html`), with the
/// per-track append button wired to `POST /now-playing/queue/add/{id}`.
#[derive(Template, Default)]
#[template(path = "_partials/now_queue.html")]
struct NowQueue {
    logged_in: bool,
    connected: bool,
    error: String,
    items: Vec<QueueRow>,
}

#[derive(Default)]
struct QueueRow {
    /// 0 = the track is not in the hub (renders as plain text, no link).
    hub_track_id: i64,
    track_url: String,
    title: String,
    artists: String,
    image: String,
    duration_disp: String,
}

/// htmx fragment: the user's Spotify playback queue.
async fn now_queue_frag(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "now").await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    render_now_queue(&st, &nav).await
}

async fn render_now_queue(st: &AppState, nav: &crate::ui::Nav) -> Response {
    if !crate::nowplaying::has_queue_scope(&st.pool, nav.id).await {
        return render(&NowQueue {
            logged_in: true,
            ..Default::default()
        });
    }
    match crate::nowplaying::queue(&st.pool, &st.cfg, nav.id).await {
        Ok(items) => {
            let rows = items
                .iter()
                .map(|i| QueueRow {
                    hub_track_id: i.hub_track_id.unwrap_or(0),
                    track_url: i
                        .hub_track_id
                        .map(|id| format!("/track/{id}"))
                        .unwrap_or_default(),
                    title: i.title.clone(),
                    artists: i.artists.clone(),
                    image: i.image.clone(),
                    duration_disp: i.duration_disp(),
                })
                .collect();
            render(&NowQueue {
                logged_in: true,
                connected: true,
                error: String::new(),
                items: rows,
            })
        }
        Err(e) => render(&NowQueue {
            logged_in: true,
            connected: true,
            error: format!("Spotify-Fehler: {e}"),
            ..Default::default()
        }),
    }
}

/// Append a hub track to the Spotify queue (htmx). Responds `204` with no body
/// — the shell's queue widget refreshes itself off the `hub:queue-changed`
/// event, so the button works from any page.
async fn now_queue_add(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(track_id): Path<i64>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "now").await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let (msg, kind) = if !crate::nowplaying::has_playback_write_scope(&st.pool, nav.id).await {
        no_write_scope("Warteschlange")
    } else if track_id == 0 {
        ("Kein Track angegeben.".to_string(), "err")
    } else {
        match crate::nowplaying::spotify_uri(&st.pool, track_id).await {
            None => ("Dieser Track hat keine Spotify-ID.".to_string(), "err"),
            Some(uri) => {
                match crate::nowplaying::add_to_queue(&st.pool, &st.cfg, nav.id, &uri).await {
                    Ok(()) => ("In die Spotify-Warteschlange gelegt.".to_string(), "ok"),
                    Err(e) => (format!("{e}"), "err"),
                }
            }
        }
    };

    now_queue_changed_response(&msg, kind)
}

/// Run a player transport command (htmx). `204` + toast; the shell's
/// now-playing widget refreshes itself off the `hub:now-changed` event.
async fn now_transport(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(cmd): Path<String>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "now").await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let (msg, kind) = match crate::nowplaying::Transport::parse(&cmd) {
        None => (format!("Unbekannter Player-Befehl: {cmd}"), "err"),
        Some(t) => {
            if !crate::nowplaying::has_playback_write_scope(&st.pool, nav.id).await {
                no_write_scope("Player-Steuerung")
            } else {
                match crate::nowplaying::transport(&st.pool, &st.cfg, nav.id, t).await {
                    Ok(()) => (t.label().to_string(), "ok"),
                    Err(e) => (format!("{e}"), "err"),
                }
            }
        }
    };

    now_changed_response(&msg, kind)
}

/// Play a whole tag as the current Spotify context (`PUT /me/player/play`).
///
/// The track list comes from the tag's materialized resolution, the uris from
/// `nowplaying::spotify_uris` — i.e. the same source of truth as the per-track
/// queue button.
async fn now_play_tag(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(tag_id): Path<i64>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "now").await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let (msg, kind) = if !crate::nowplaying::has_playback_write_scope(&st.pool, nav.id).await {
        no_write_scope("Abspielen")
    } else {
        let ids = crate::backpacks::tracks_for_tag(&st.pool, tag_id)
            .await
            .unwrap_or_default();
        play_tracks(&st, nav.id, &ids).await
    };
    now_changed_response(&msg, kind)
}

/// Play a whole backpack as the current Spotify context.
async fn now_play_backpack(
    State(st): State<AppState>,
    headers: HeaderMap,
    Path(backpack_id): Path<i64>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "now").await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let (msg, kind) = if !crate::nowplaying::has_playback_write_scope(&st.pool, nav.id).await {
        no_write_scope("Abspielen")
    } else {
        // Owner check first: a backpack someone else owns must not be startable.
        match crate::backpacks::backpack_owned_by(&st.pool, backpack_id, nav.id).await {
            Ok(None) => ("Backpack nicht gefunden.".to_string(), "err"),
            Err(e) => (format!("{e}"), "err"),
            Ok(Some(_)) => {
                let ids: Vec<i64> = crate::backpacks::resolve_unbounded(&st.pool, backpack_id)
                    .await
                    .map(|rs| rs.into_iter().map(|r| r.track_id).collect())
                    .unwrap_or_default();
                play_tracks(&st, nav.id, &ids).await
            }
        }
    };
    now_changed_response(&msg, kind)
}

/// The gate failure shared by the "play a collection" endpoints.
fn no_write_scope(what: &str) -> (String, &'static str) {
    (
        format!("Spotify neu verbinden — {what} braucht den Schreib-Scope."),
        "err",
    )
}

/// Map track ids to Spotify uris and start them. Returns the toast `(msg, kind)`.
async fn play_tracks(st: &AppState, user_id: i64, ids: &[i64]) -> (String, &'static str) {
    match crate::nowplaying::spotify_uris(&st.pool, ids).await {
        Err(e) => (format!("{e}"), "err"),
        Ok(uris) if uris.is_empty() => ("Keine Spotify-Tracks darin.".to_string(), "err"),
        Ok(uris) => {
            let total = uris.len();
            match crate::nowplaying::play_uris(&st.pool, &st.cfg, user_id, &uris).await {
                Ok(n) if n < total => (
                    format!("Die ersten {n} von {total} Tracks laufen (unser Limit)."),
                    "ok",
                ),
                Ok(n) => (format!("{n} Tracks laufen."), "ok"),
                Err(e) => (format!("{e}"), "err"),
            }
        }
    }
}

/// `204` + toast for the playback endpoints (the now widget refreshes itself).
/// `204` + toast + `hub:now-changed` for the player endpoints (the now widget
/// refreshes itself).
fn now_changed_response(msg: &str, kind: &str) -> Response {
    crate::toast::apply_with(
        StatusCode::NO_CONTENT.into_response(),
        msg,
        kind,
        &["hub:now-changed"],
    )
}

/// Same, but refreshing the queue widget instead.
fn now_queue_changed_response(msg: &str, kind: &str) -> Response {
    crate::toast::apply_with(
        StatusCode::NO_CONTENT.into_response(),
        msg,
        kind,
        &["hub:queue-changed"],
    )
}

#[derive(Deserialize)]
struct NowTagForm {
    track_id: i64,
    name: String,
    #[serde(default, rename = "type")]
    tag_type: Option<String>,
}

/// Tag the currently-playing track in place (htmx; returns the refreshed widget).
async fn now_tag(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<NowTagForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "now").await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let (msg, kind) = if f.track_id == 0 {
        ("Track noch nicht im Hub.".to_string(), "err")
    } else if f.name.trim().is_empty() {
        ("Kein Tag-Name angegeben.".to_string(), "err")
    } else {
        match crate::tags::ensure_tag(&st.pool, nav.id, &f.name).await {
            Ok(tag_id) => {
                if let Some(ty) = f
                    .tag_type
                    .as_deref()
                    .and_then(crate::tagtypes::TagType::parse)
                {
                    let _ = crate::tagtypes::set(&st.pool, nav.id, tag_id, Some(ty)).await;
                }
                match crate::tags::tag_track(&st.pool, &st.cfg, nav.id, f.track_id, tag_id).await {
                    Ok(_) => (format!("Tag „{}“ hinzugefügt", f.name.trim()), "ok"),
                    Err(e) => (format!("Fehler: {e}"), "err"),
                }
            }
            Err(e) => (format!("Fehler: {e}"), "err"),
        }
    };

    crate::toast::apply(render_now(&st, &nav).await, &msg, kind)
}

/// Full-screen Now Playing page (`/now`): renders the live track's real track
/// view when we have it, otherwise a compact fallback.
async fn now_page(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "now").await else {
        return Redirect::to("/login").into_response();
    };
    if !crate::nowplaying::has_scope(&st.pool, nav.id).await {
        return render(&NowPage {
            nav,
            flash: String::new(),
            message: "Spotify ist ohne Now-Playing-Berechtigung verbunden.".to_string(),
            connect_link: true,
            has_external: false,
            title: String::new(),
            artists_html: String::new(),
            album: String::new(),
            image: String::new(),
            progress_pct: 0,
            progress_disp: String::new(),
            duration_disp: String::new(),
        });
    }
    match crate::nowplaying::current(&st.pool, &st.cfg, nav.id).await {
        Ok(Some(n)) => match n.hub_track_id {
            Some(id) => {
                match build_track_page(
                    &st,
                    nav.clone(),
                    id,
                    "Jetzt läuft in Spotify".to_string(),
                    crate::recommend::RecScope::All,
                    false,
                    None,
                )
                .await
                {
                    Some(page) => render_track(page),
                    None => render(&now_external(nav, &n, true)),
                }
            }
            None => render(&now_external(nav, &n, false)),
        },
        Ok(None) => render(&NowPage {
            nav,
            flash: String::new(),
            message: "Gerade läuft nichts in Spotify.".to_string(),
            connect_link: false,
            has_external: false,
            title: String::new(),
            artists_html: String::new(),
            album: String::new(),
            image: String::new(),
            progress_pct: 0,
            progress_disp: String::new(),
            duration_disp: String::new(),
        }),
        Err(e) => render(&NowPage {
            nav,
            flash: String::new(),
            message: format!("Spotify-Fehler: {e}"),
            connect_link: false,
            has_external: false,
            title: String::new(),
            artists_html: String::new(),
            album: String::new(),
            image: String::new(),
            progress_pct: 0,
            progress_disp: String::new(),
            duration_disp: String::new(),
        }),
    }
}

/// Build the fallback `NowPage` for a track that has no hub counterpart.
fn now_external(nav: crate::ui::Nav, n: &crate::nowplaying::Now, _known: bool) -> NowPage {
    NowPage {
        nav,
        flash: String::new(),
        message: "Dieser Track ist noch nicht im Hub — er erscheint nach dem nächsten Sync."
            .to_string(),
        connect_link: false,
        has_external: true,
        title: n.title.clone(),
        artists_html: crate::artists::escape_html(&n.artists),
        album: n.album.clone(),
        image: n.image.clone(),
        progress_pct: n.progress_pct(),
        progress_disp: n.progress_disp(),
        duration_disp: n.duration_disp(),
    }
}

// ── uniform track actions (#318) ────────────────────────────────

/// Add an `HX-Trigger` toast header to a response.
fn with_toast(resp: Response, msg: &str, kind: &str) -> Response {
    crate::toast::apply(resp, msg, kind)
}

/// Displayed BPM + key for a hub track.
///
/// Reads the **live vault** (`resolved`) — the very same source the track page
/// renders — so the two can never disagree (#466). The `hub_track_audio`
/// read-model is deliberately **not** used here: it lags behind, which is why the
/// now-playing widget used to show „—" while the track page showed a BPM.
async fn track_bpm_key(st: &AppState, track_id: i64) -> (String, String) {
    let isrc: Option<String> = sqlx::query_scalar(
        "SELECT isrc FROM hub_tracks WHERE id = ?1 AND isrc IS NOT NULL AND TRIM(isrc) <> ''",
    )
    .bind(track_id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten();
    let resolved = match isrc {
        Some(isrc) => crate::music_api::resolved_audio(&st.cfg, &isrc).await,
        None => None,
    };
    match resolved {
        Some(r) => (
            r.bpm
                .map(|b| format!("{b:.0}"))
                .unwrap_or_else(|| "—".into()),
            r.key.or(r.camelot).unwrap_or_else(|| "—".into()),
        ),
        None => ("—".into(), "—".into()),
    }
}

async fn track_actions_frag(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if current_user(&st, &headers).await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    render(&TrackActions { id })
}

async fn track_actions_playlist_frag(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "track").await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let curated = crate::curate::list_for_user(&st.pool, nav.id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|c| mini(c.id, c.name))
        .collect();
    // The user's own Spotify playlists — adding to these writes to Spotify.
    let spotify = sqlx::query(
        "SELECT id, name FROM hub_playlists
          WHERE user_id = ?1 AND service = 'spotify' AND is_owned = 1 AND name IS NOT NULL
          ORDER BY name COLLATE NOCASE LIMIT 500",
    )
    .bind(nav.id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|r| {
        let name: String = r.get::<Option<String>, _>("name").unwrap_or_default();
        mini(r.get("id"), name)
    })
    .collect();
    let write_ready = crate::curate::has_write_scope(&st.pool, nav.id).await;
    render(&TrackPlaylistPicker {
        id,
        curated,
        spotify,
        write_ready,
    })
}

fn mini(id: i64, name: String) -> PlaylistMini {
    let name_lower = name.to_lowercase();
    PlaylistMini {
        id,
        name,
        name_lower,
    }
}

async fn track_actions_tag_frag(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if crate::ui::nav(&st, &headers, "track").await.is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    render(&TrackTagFormPartial {
        id,
        tag_action: format!("/track/{id}/actions/tag"),
        tag_target: "closest .hub-ta".to_string(),
        rec_scope_param: String::new(),
        tag_scope: String::new(),
        my_types: type_options(),
    })
}

#[derive(Deserialize)]
struct AddToPlaylistForm {
    #[serde(default)]
    playlist_id: Option<i64>,
    #[serde(default)]
    sp_playlist_id: Option<i64>,
    #[serde(default)]
    new_name: Option<String>,
}

async fn track_actions_playlist_post(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<AddToPlaylistForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "track").await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let (msg, kind) = if let Some(name) = f
        .new_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match crate::curate::create_manual(&st.pool, nav.id, name).await {
            Ok(pid) => match crate::curate::add_item(&st.pool, pid, id).await {
                Ok(_) => (format!("In neue Playlist „{name}“ gelegt"), "ok"),
                Err(e) => (format!("Fehler: {e}"), "err"),
            },
            Err(e) => (format!("Fehler: {e}"), "err"),
        }
    } else if let Some(pid) = f.playlist_id {
        let owner = crate::curate::get(&st.pool, pid)
            .await
            .ok()
            .flatten()
            .map(|c| c.user_id);
        if owner == Some(nav.id) {
            match crate::curate::add_item(&st.pool, pid, id).await {
                Ok(_) => ("Zur Playlist hinzugefügt".to_string(), "ok"),
                Err(e) => (format!("Fehler: {e}"), "err"),
            }
        } else {
            ("Playlist nicht gefunden".to_string(), "err")
        }
    } else if let Some(sp) = f.sp_playlist_id {
        match add_to_spotify_playlist(&st, nav.id, sp, id).await {
            Ok(()) => ("Zu Spotify hinzugefügt".to_string(), "ok"),
            Err(e) => (format!("Fehler: {e}"), "err"),
        }
    } else {
        ("Keine Playlist angegeben".to_string(), "err")
    };
    with_toast(render(&TrackActions { id }), &msg, kind)
}

/// The Spotify track id for a hub track (external id, else the service id).
async fn track_spotify_id(st: &AppState, track_id: i64) -> Option<String> {
    let ext: Option<String> = sqlx::query_scalar(
        "SELECT external_id FROM hub_track_external_ids
          WHERE track_id = ?1 AND service = 'spotify' LIMIT 1",
    )
    .bind(track_id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten();
    if let Some(e) = ext {
        if !e.is_empty() {
            return Some(e);
        }
    }
    sqlx::query_scalar(
        "SELECT service_track_id FROM hub_tracks
          WHERE id = ?1 AND service = 'spotify' LIMIT 1",
    )
    .bind(track_id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten()
}

/// Add a track to one of the user's own Spotify playlists (writes to Spotify).
async fn add_to_spotify_playlist(
    st: &AppState,
    user_id: i64,
    local_playlist_id: i64,
    track_id: i64,
) -> anyhow::Result<()> {
    use anyhow::{Context, bail};
    let row = sqlx::query(
        "SELECT user_id, playlist_id, is_owned FROM hub_playlists
          WHERE id = ?1 AND service = 'spotify'",
    )
    .bind(local_playlist_id)
    .fetch_optional(&st.pool)
    .await?
    .context("Playlist nicht gefunden")?;
    let owner: i64 = row.get("user_id");
    let remote: String = row.get("playlist_id");
    let is_owned: i64 = row.get("is_owned");
    if owner != user_id || is_owned == 0 {
        bail!("Playlist gehört dir nicht");
    }
    let spotify_id = track_spotify_id(st, track_id)
        .await
        .context("Track hat keine Spotify-ID")?;
    let token = crate::ingest::access_token(&st.pool, &st.cfg, user_id).await?;
    let url = crate::spotify::api_url(
        &st.cfg.spotify_api_base,
        &format!("/playlists/{remote}/items"),
    );
    let (status, resp) = crate::spotify::post_json(
        &token,
        &url,
        &serde_json::json!({ "uris": [format!("spotify:track:{spotify_id}")] }),
    )
    .await?;
    if !(200..300).contains(&status) {
        bail!(
            "Spotify -> {status}: {}",
            resp["error"]["message"].as_str().unwrap_or("Fehler")
        );
    }
    Ok(())
}

#[derive(Deserialize)]
struct AddTagForm {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    tag_type: Option<String>,
    /// Existing tag to link (the unified tag input, #449).
    #[serde(default)]
    tag_id: Option<i64>,
}

async fn track_actions_tag_post(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<AddTagForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "track").await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let (msg, kind) = match crate::tags::resolve_tag_for_add(
        &st.pool,
        nav.id,
        f.tag_id,
        Some(&f.name),
        f.tag_type.as_deref(),
    )
    .await
    {
        Some(tag_id) => match crate::tags::tag_track(&st.pool, &st.cfg, nav.id, id, tag_id).await {
            Ok(_) => ("Tag hinzugefügt".to_string(), "ok"),
            Err(e) => (format!("Fehler: {e}"), "err"),
        },
        None => ("Kein Tag angegeben".to_string(), "err"),
    };
    with_toast(render(&TrackActions { id }), &msg, kind)
}

// ── curated playlists (#316) ────────────────────────────────────────────────

/// A `<select>` option with a precomputed `selected` flag.
struct Opt {
    value: String,
    label: String,
    selected: bool,
}

struct CurateRow {
    id: i64,
    name: String,
    mode_label: String,
    source: String,
    limit: i64,
    daily: bool,
    spotify: bool,
    last_push: String,
    status: String,
    status_class: String,
}

struct CurateTrackRow {
    id: i64,
    title: String,
    artists_html: String,
    spotify: bool,
}

#[derive(Template)]
#[template(path = "curate.html")]
struct CuratePage {
    nav: crate::ui::Nav,
    flash: String,
    rows: Vec<CurateRow>,
    modes: Vec<Opt>,
    tags: Vec<Opt>,
    lenses: Vec<Opt>,
    spotify_ready: bool,
    default_limit: i64,
}

#[derive(Template)]
#[template(path = "curate_detail.html")]
struct CurateDetailPage {
    nav: crate::ui::Nav,
    flash: String,
    id: i64,
    name: String,
    description: String,
    is_public: bool,
    daily: bool,
    fill_limit: i64,
    modes: Vec<Opt>,
    tags: Vec<Opt>,
    lenses: Vec<Opt>,
    spotify: bool,
    spotify_playlist_id: String,
    last_push: String,
    status: String,
    status_class: String,
    preview: Vec<CurateTrackRow>,
    preview_total: i64,
    preview_pushable: i64,
}

fn mode_label(mode: &str) -> &'static str {
    match mode {
        "tag" => "Tag",
        "lens" => "Linse",
        _ => "Manuell",
    }
}

fn mode_opts(selected: &str) -> Vec<Opt> {
    ["manual", "tag", "lens"]
        .iter()
        .map(|m| Opt {
            value: m.to_string(),
            label: mode_label(m).to_string(),
            selected: *m == selected,
        })
        .collect()
}

fn lens_opts(selected: Option<&str>) -> Vec<Opt> {
    crate::curate::LENSES
        .iter()
        .map(|(k, l)| Opt {
            value: k.to_string(),
            label: l.to_string(),
            selected: selected == Some(*k),
        })
        .collect()
}

fn tag_opts(tags: &[(i64, String)], selected: Option<i64>) -> Vec<Opt> {
    tags.iter()
        .map(|(id, name)| Opt {
            value: id.to_string(),
            label: name.clone(),
            selected: selected == Some(*id),
        })
        .collect()
}

fn fmt_date_opt(ts: Option<&str>) -> String {
    match ts {
        Some(s) if s.len() >= 16 => s[..16].replace('T', " "),
        Some(s) if s.len() >= 10 => s[..10].to_string(),
        _ => "—".to_string(),
    }
}

fn status_class(status: Option<&str>) -> &'static str {
    match status {
        Some("ok") => "hub-badge hub-badge-ok",
        Some("error") => "hub-badge hub-badge-no",
        _ => "hub-badge",
    }
}

#[derive(Deserialize)]
struct CurateForm {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    fill_mode: Option<String>,
    #[serde(default)]
    fill_tag_id: Option<i64>,
    #[serde(default)]
    fill_lens: Option<String>,
    #[serde(default)]
    fill_limit: Option<i64>,
    #[serde(default)]
    is_public: Option<String>,
    #[serde(default)]
    daily: Option<String>,
}

struct ParsedCurate {
    name: String,
    description: String,
    is_public: i64,
    daily: i64,
    fill_mode: String,
    fill_tag_id: Option<i64>,
    fill_lens: Option<String>,
    fill_limit: i64,
}

fn parse_curate(f: &CurateForm) -> ParsedCurate {
    let mode = match f.fill_mode.as_deref() {
        Some("tag") => "tag",
        Some("lens") => "lens",
        _ => "manual",
    }
    .to_string();
    ParsedCurate {
        name: f.name.trim().to_string(),
        description: f.description.trim().to_string(),
        is_public: if f.is_public.is_some() { 1 } else { 0 },
        daily: if f.daily.is_some() { 1 } else { 0 },
        fill_tag_id: if mode == "tag" { f.fill_tag_id } else { None },
        fill_lens: if mode == "lens" {
            f.fill_lens.clone().filter(|s| !s.is_empty())
        } else {
            None
        },
        fill_mode: mode,
        fill_limit: f.fill_limit.unwrap_or(50).clamp(1, 500),
    }
}

async fn curate_page(
    State(st): State<AppState>,
    Query(flash): Query<Flash>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "curate").await else {
        return Redirect::to("/login").into_response();
    };
    let cps = crate::curate::list_for_user(&st.pool, nav.id)
        .await
        .unwrap_or_default();

    // Tag names for display.
    let tag_names: std::collections::HashMap<i64, String> =
        crate::tags::list_user_tags(&st.pool, nav.id)
            .await
            .into_iter()
            .collect();

    let rows: Vec<CurateRow> = cps
        .iter()
        .map(|cp| CurateRow {
            id: cp.id,
            name: cp.name.clone(),
            mode_label: mode_label(&cp.fill_mode).to_string(),
            source: match cp.fill_mode.as_str() {
                "tag" => cp
                    .fill_tag_id
                    .and_then(|t| tag_names.get(&t).cloned())
                    .unwrap_or_else(|| "(kein Tag)".to_string()),
                "lens" => {
                    crate::curate::lens_label(cp.fill_lens.as_deref().unwrap_or("")).to_string()
                }
                _ => "—".to_string(),
            },
            limit: cp.fill_limit,
            daily: cp.daily(),
            spotify: cp.pushed_to_spotify(),
            last_push: fmt_date_opt(cp.last_push_at.as_deref()),
            status: cp
                .last_push_status
                .clone()
                .unwrap_or_else(|| "—".to_string()),
            status_class: status_class(cp.last_push_status.as_deref()).to_string(),
        })
        .collect();

    let tags = crate::tags::list_user_tags(&st.pool, nav.id).await;
    let spotify_ready = crate::curate::has_write_scope(&st.pool, nav.id).await;
    render(&CuratePage {
        nav,
        flash: flash.msg.unwrap_or_default(),
        rows,
        modes: mode_opts("manual"),
        tags: tag_opts(&tags, None),
        lenses: lens_opts(None),
        spotify_ready,
        default_limit: 50,
    })
}

async fn curate_create(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<CurateForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "curate").await else {
        return Redirect::to("/login").into_response();
    };
    let p = parse_curate(&f);
    if p.name.is_empty() {
        return Redirect::to("/curate").into_response();
    }
    let now = now_iso();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_curated_playlists
            (user_id, name, description, is_public, fill_mode, fill_tag_id, fill_lens,
             fill_limit, daily, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
         RETURNING id",
    )
    .bind(nav.id)
    .bind(&p.name)
    .bind(&p.description)
    .bind(p.is_public)
    .bind(&p.fill_mode)
    .bind(p.fill_tag_id)
    .bind(&p.fill_lens)
    .bind(p.fill_limit)
    .bind(p.daily)
    .bind(&now)
    .fetch_one(&st.pool)
    .await
    .unwrap_or(0);
    Redirect::to(&format!("/curate/{id}")).into_response()
}

async fn curate_update(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<CurateForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "curate").await else {
        return Redirect::to("/login").into_response();
    };
    let p = parse_curate(&f);
    let now = now_iso();
    let _ = sqlx::query(
        "UPDATE hub_curated_playlists
            SET name = ?3, description = ?4, is_public = ?5, fill_mode = ?6,
                fill_tag_id = ?7, fill_lens = ?8, fill_limit = ?9, daily = ?10,
                updated_at = ?11
          WHERE id = ?1 AND user_id = ?2",
    )
    .bind(id)
    .bind(nav.id)
    .bind(&p.name)
    .bind(&p.description)
    .bind(p.is_public)
    .bind(&p.fill_mode)
    .bind(p.fill_tag_id)
    .bind(&p.fill_lens)
    .bind(p.fill_limit)
    .bind(p.daily)
    .bind(&now)
    .execute(&st.pool)
    .await;
    Redirect::to(&format!("/curate/{id}")).into_response()
}

async fn curate_detail(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Query(flash): Query<Flash>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "curate").await else {
        return Redirect::to("/login").into_response();
    };
    let Some(cp) = crate::curate::get(&st.pool, id).await.ok().flatten() else {
        return track_not_found();
    };
    if cp.user_id != nav.id {
        return track_not_found();
    }
    let ids = crate::curate::resolve_track_ids(&st.pool, &cp)
        .await
        .unwrap_or_default();
    let total = ids.len() as i64;

    // Title/artists + Spotify availability for the preview (capped at 200).
    let mut preview = Vec::new();
    let mut pushable = 0i64;
    for chunk in ids.chunks(200) {
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT t.id, t.title, t.artists,
                    EXISTS(SELECT 1 FROM hub_track_external_ids e
                            WHERE e.track_id = t.id AND e.service = 'spotify') AS sp
               FROM hub_tracks t WHERE t.id IN (",
        );
        let mut sep = qb.separated(", ");
        for tid in chunk {
            sep.push_bind(*tid);
        }
        sep.push_unseparated(") ");
        let order: std::collections::HashMap<i64, usize> =
            chunk.iter().enumerate().map(|(i, t)| (*t, i)).collect();
        let mut got: Vec<CurateTrackRow> = qb
            .build()
            .fetch_all(&st.pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| {
                let sp: i64 = r.try_get("sp").unwrap_or(0);
                if sp != 0 {
                    pushable += 1;
                }
                CurateTrackRow {
                    id: r.try_get("id").unwrap_or(0),
                    title: r
                        .try_get::<Option<String>, _>("title")
                        .unwrap_or_default()
                        .unwrap_or_default(),
                    artists_html: String::new(),
                    spotify: sp != 0,
                }
            })
            .collect();
        got.sort_by_key(|r| order.get(&r.id).copied().unwrap_or(usize::MAX));
        preview.extend(got);
    }
    let preview_ids: Vec<i64> = preview.iter().map(|r| r.id).collect();
    let mut preview_html = crate::artists::artists_html_for_tracks(&st.pool, &preview_ids).await;
    for r in preview.iter_mut() {
        r.artists_html = preview_html.remove(&r.id).unwrap_or_default();
    }

    let tags = crate::tags::list_user_tags(&st.pool, nav.id).await;
    render(&CurateDetailPage {
        nav,
        flash: flash.msg.unwrap_or_default(),
        id: cp.id,
        name: cp.name.clone(),
        description: cp.description.clone(),
        is_public: cp.is_public(),
        daily: cp.daily(),
        fill_limit: cp.fill_limit,
        modes: mode_opts(&cp.fill_mode),
        tags: tag_opts(&tags, cp.fill_tag_id),
        lenses: lens_opts(cp.fill_lens.as_deref()),
        spotify: cp.pushed_to_spotify(),
        spotify_playlist_id: cp.spotify_playlist_id.clone().unwrap_or_default(),
        last_push: fmt_date_opt(cp.last_push_at.as_deref()),
        status: cp
            .last_push_status
            .clone()
            .unwrap_or_else(|| "—".to_string()),
        status_class: status_class(cp.last_push_status.as_deref()).to_string(),
        preview,
        preview_total: total,
        preview_pushable: pushable,
    })
}

async fn curate_push(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "curate").await else {
        return Redirect::to("/login").into_response();
    };
    let Some(cp) = crate::curate::get(&st.pool, id).await.ok().flatten() else {
        return track_not_found();
    };
    if cp.user_id != nav.id {
        return track_not_found();
    }
    let msg = match crate::curate::push(&st.pool, &st.cfg, &cp).await {
        Ok(r) => format!("Zu Spotify gepusht: {} Tracks", r.pushed),
        Err(e) => {
            let _ = sqlx::query(
                "UPDATE hub_curated_playlists SET last_push_status='error', last_push_error=?2 WHERE id=?1",
            )
            .bind(cp.id)
            .bind(e.to_string())
            .execute(&st.pool)
            .await;
            format!("Push-Fehler: {e}")
        }
    };
    Redirect::to(&format!("/curate/{id}?msg={}", urlencoding::encode(&msg))).into_response()
}

async fn curate_delete(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "curate").await else {
        return Redirect::to("/login").into_response();
    };
    let _ = sqlx::query("DELETE FROM hub_curated_playlists WHERE id = ?1 AND user_id = ?2")
        .bind(id)
        .bind(nav.id)
        .execute(&st.pool)
        .await;
    Redirect::to("/curate").into_response()
}

/// A few recently seen tracks for the dashboard.
async fn recent_tracks(st: &AppState) -> Vec<RecentRow> {
    let rows = sqlx::query("SELECT id, title FROM hub_tracks ORDER BY id DESC LIMIT 25")
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default();
    let ids: Vec<i64> = rows.iter().map(|r| r.get::<i64, _>("id")).collect();
    let mut html = crate::artists::artists_html_for_tracks(&st.pool, &ids).await;
    rows.iter()
        .map(|r| {
            let id = r.get::<i64, _>("id");
            RecentRow {
                id,
                title: r.get::<Option<String>, _>("title").unwrap_or_default(),
                artists_html: html.remove(&id).unwrap_or_default(),
            }
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
