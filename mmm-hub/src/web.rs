//! Minimal web UI: username/password accounts, sessions, and "Connect Spotify".
//!
//! Uses the public HTTPS redirect (`https://…/api/hub/services/spotify/callback`)
//! instead of a loopback listener, so it works for any logged-in user in a browser.

use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::Router;
use serde::Deserialize;
use serde_json::Value;
use sqlx::{Row, SqlitePool};

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
        .route("/sql", get(sql_page).post(sql_run))
        .route("/api/hub/services/{service}/connect", get(connect))
        .route("/api/hub/services/{service}/fetch-playlists", post(fetch_playlists_handler))
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

fn page(body: &str) -> String {
    format!(
        r#"<!doctype html><html lang="de"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>MMM Hub</title>
<style>
 body{{font:15px/1.5 -apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif;margin:0;background:#0f1115;color:#e6e6e6}}
 .wrap{{max-width:640px;margin:8vh auto;padding:0 20px}}
 .card{{background:#171a21;border:1px solid #262b36;border-radius:14px;padding:28px}}
 h1{{margin:0 0 4px;font-size:22px}} h2{{font-size:16px;margin:24px 0 8px;color:#9aa4b2}}
 .muted{{color:#9aa4b2}} a{{color:#7cc4ff}}
 input{{width:100%;box-sizing:border-box;padding:11px 13px;margin:6px 0 14px;border-radius:9px;border:1px solid #333a47;background:#0f1115;color:#e6e6e6}}
 button{{cursor:pointer;border:0;border-radius:9px;padding:11px 16px;font-weight:600;font-size:15px}}
 .primary{{background:#1db954;color:#05240f}} .danger{{background:#3a1f22;color:#ff9a9a}}
 .badge{{display:inline-block;padding:3px 10px;border-radius:999px;font-size:13px}}
 .ok{{background:#123c22;color:#5fe08a}} .no{{background:#3a1f22;color:#ff9a9a}}
 .row{{display:flex;align-items:center;justify-content:space-between;gap:16px;margin-top:18px}}
 form{{margin:0}} .err{{background:#3a1f22;color:#ff9a9a;padding:10px 12px;border-radius:9px;margin-bottom:14px}}
 .flash{{background:#123c22;color:#5fe08a;padding:10px 12px;border-radius:9px;margin-bottom:14px}}
 table{{width:100%;border-collapse:collapse;margin-top:12px;font-size:14px}}
 th,td{{text-align:left;padding:8px;border-bottom:1px solid #262b36;vertical-align:middle}}
 th{{color:#9aa4b2;font-weight:600}}
 td form button{{padding:6px 11px;font-size:13px}}
 .st-ok{{color:#5fe08a}} .st-no{{color:#ff9a9a}} .st-pend{{color:#ffd479}}
 textarea{{width:100%;box-sizing:border-box;font:13px/1.55 ui-monospace,SFMono-Regular,Menlo,monospace;background:#0f1115;color:#e6e6e6;border:1px solid #333a47;border-radius:9px;padding:11px 13px;margin:10px 0 12px;resize:vertical}}
 select{{background:#0f1115;color:#e6e6e6;border:1px solid #333a47;border-radius:9px;padding:8px 10px;font-size:14px}}
 table td{{font-variant-numeric:tabular-nums}}
 .sqlwrap{{max-width:1100px}}
 a.back{{color:#9aa4b2;text-decoration:none;font-size:14px}}
</style></head><body><div class="wrap"><div class="card">{body}</div></div></body></html>"#
    )
}

fn login_page(err: Option<&str>) -> String {
    let e = err.map(|e| format!("<div class=\"err\">{}</div>", esc(e))).unwrap_or_default();
    page(&format!(
        r#"<h1>MMM Hub</h1><p class="muted">Login</p>{e}
<form method="post" action="/login">
 <input name="username" placeholder="Benutzername" autofocus required>
 <input name="password" type="password" placeholder="Passwort" required>
 <button class="primary" type="submit">Einloggen</button>
</form>
<p class="muted">Noch keinen Account? <a href="/signup">Registrieren</a></p>"#
    ))
}

fn signup_page(err: Option<&str>) -> String {
    let e = err.map(|e| format!("<div class=\"err\">{}</div>", esc(e))).unwrap_or_default();
    page(&format!(
        r#"<h1>MMM Hub</h1><p class="muted">Account anlegen</p>{e}
<form method="post" action="/signup">
 <input name="username" placeholder="Benutzername" autofocus required>
 <input name="password" type="password" placeholder="Passwort" required>
 <button class="primary" type="submit">Registrieren</button>
</form>
<p class="muted">Schon dabei? <a href="/login">Einloggen</a></p>"#
    ))
}

fn error_page(msg: &str) -> Response {
    Html(page(&format!(
        "<h1>Fehler</h1><p>{}</p><p><a href=\"/\">Zurück</a></p>",
        esc(msg)
    )))
    .into_response()
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

async fn index(
    State(st): State<AppState>,
    headers: HeaderMap,
    Query(flash): Query<Flash>,
) -> Response {
    let Some((uid, slug)) = current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let status = spotify_status(&st, uid).await;
    let connected = matches!(status, Some((true, _)));

    let (badge, action) = match status {
        Some((true, name)) => (
            format!(
                "<span class=\"badge ok\">verbunden{}</span>",
                name.map(|n| format!(" als {}", esc(&n))).unwrap_or_default()
            ),
            "<span style=\"display:inline-flex;gap:8px\">\
             <form method=\"post\" action=\"/api/hub/services/spotify/fetch-playlists\"><button class=\"primary\">Playlists holen</button></form>\
             <form method=\"post\" action=\"/api/hub/services/spotify/disconnect\"><button class=\"danger\">Trennen</button></form>\
             </span>"
                .to_string(),
        ),
        _ => (
            "<span class=\"badge no\">nicht verbunden</span>".to_string(),
            "<form method=\"get\" action=\"/api/hub/services/spotify/connect\"><button class=\"primary\">Spotify verbinden</button></form>".to_string(),
        ),
    };

    let flash_html = flash
        .msg
        .map(|m| format!("<div class=\"flash\">{}</div>", esc(&m)))
        .unwrap_or_default();
    let playlists_html = playlists_table(&st, uid).await;
    let likes_html = if connected {
        likes_info(&st, uid).await
    } else {
        String::new()
    };

    Html(page(&format!(
        r#"{flash_html}<div class="row"><h1>Hallo, {}</h1>
<form method="post" action="/logout"><button class="danger">Logout</button></form></div>
<h2>Spotify</h2>
<div class="row"><div>Status: {badge}</div>{action}</div>
{likes_html}
{playlists_html}
<h2>Daten</h2>
<p class="muted">JSON-API: <a href="/api/hub/users">users</a> ·
<a href="/api/hub/playlists">playlists</a> ·
<a href="/api/hub/overlap">overlap</a> ·
<a href="/sql">SQL-Konsole →</a></p>"#,
        esc(&slug)
    )))
    .into_response()
}

/// Liked-tracks count + sync status for the dashboard.
async fn likes_info(st: &AppState, user_id: i64) -> String {
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

    let value = if let Some(e) = err {
        format!("<span class=\"st-no\">Fehler: {}</span>", esc(&e))
    } else if count > 0 {
        format!("<span class=\"st-ok\">{count}</span>")
    } else if status.as_deref() == Some("done") {
        "<span class=\"muted\">0</span>".to_string()
    } else {
        "<span class=\"st-pend\">lädt…</span>".to_string()
    };
    format!("<div class=\"row\"><div>Liked-Tracks: {value}</div></div>")
}

/// Table of the user's playlists (no pagination) with per-row fetch toggles.
async fn playlists_table(st: &AppState, user_id: i64) -> String {
    let rows = sqlx::query(
        "SELECT p.id, p.name, p.is_owned, p.track_count, p.items_available,
                p.enabled_for_fetch, p.fetch_error,
                (SELECT COUNT(*) FROM hub_playlist_tracks t WHERE t.playlist_id = p.id) AS fetched
           FROM hub_playlists p
          WHERE p.user_id = ?1
          ORDER BY p.is_owned DESC, p.name COLLATE NOCASE",
    )
    .bind(user_id)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    if rows.is_empty() {
        return "<h2>Playlists</h2><p class=\"muted\">Noch keine — oben \"Playlists holen\".</p>"
            .to_string();
    }

    let owned_count = rows.iter().filter(|r| r.get::<i64, _>("is_owned") == 1).count();

    let mut body = String::from(
        "<h2>Playlists</h2><div class=\"row\"><span class=\"muted\">Fetch läuft im Hintergrund</span>\
         <span style=\"display:inline-flex;gap:8px\">\
         <form method=\"post\" action=\"/api/hub/playlists/enable-all\"><button class=\"primary\">Alle aktivieren\
         </button></form>\
         <form method=\"post\" action=\"/api/hub/playlists/disable-all\"><button class=\"danger\">Alle stoppen</button></form>\
         </span></div>",
    );
    body.push_str("<table><tr><th>Playlist</th><th>Typ</th><th>Tracks</th><th>Status</th><th></th></tr>");

    for r in &rows {
        let id: i64 = r.get("id");
        let name: String = r.get::<Option<String>, _>("name").unwrap_or_default();
        let owned: i64 = r.get("is_owned");
        let track_count: Option<i64> = r.get("track_count");
        let items_available: i64 = r.get("items_available");
        let enabled: i64 = r.get("enabled_for_fetch");
        let err: Option<String> = r.get("fetch_error");
        let fetched: i64 = r.get("fetched");

        let typ = if owned == 1 {
            "<span class=\"badge ok\">eigen</span>"
        } else {
            "<span class=\"badge no\">gefolgt</span>"
        };
        let tracks = format!(
            "{} / {}",
            fetched,
            track_count.map(|n| n.to_string()).unwrap_or_else(|| "?".into())
        );
        let status = if let Some(e) = err {
            format!("<span class=\"st-no\">Fehler: {}</span>", esc(&e))
        } else if items_available == 1 {
            "<span class=\"st-ok\">✓ geholt</span>".to_string()
        } else if enabled == 1 {
            "<span class=\"st-pend\">wartet…</span>".to_string()
        } else {
            "<span class=\"muted\">—</span>".to_string()
        };
        let action = if enabled == 1 {
            format!("<form method=\"post\" action=\"/api/hub/playlists/{id}/toggle\"><button class=\"danger\">Stop</button></form>")
        } else {
            format!("<form method=\"post\" action=\"/api/hub/playlists/{id}/toggle\"><button class=\"primary\">Fetch</button></form>")
        };

        body.push_str(&format!(
            "<tr><td>{}</td><td>{typ}</td><td>{tracks}</td><td>{status}</td><td>{action}</td></tr>",
            esc(&name)
        ));
    }
    body.push_str("</table>");
    let _ = owned_count;
    body
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
            Redirect::to(&format!("/?msg={}", urlencoding::encode(&msg))).into_response()
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
    Redirect::to("/").into_response()
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
    Redirect::to("/").into_response()
}

async fn disable_all(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some((uid, _)) = current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    let _ = sqlx::query("UPDATE hub_playlists SET enabled_for_fetch = 0 WHERE user_id = ?1")
        .bind(uid)
        .execute(&st.pool)
        .await;
    Redirect::to("/").into_response()
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
    if current_user(&st, &headers).await.is_none() {
        return Redirect::to("/login").into_response();
    }
    Html(page(&sql_page_html(DEFAULT_SQL, None, None))).into_response()
}

async fn sql_run(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<SqlForm>,
) -> Response {
    if current_user(&st, &headers).await.is_none() {
        return Redirect::to("/login").into_response();
    }
    let sql = form.sql.unwrap_or_default();
    match crate::api::run_readonly_query(&st.ro_pool, &sql).await {
        Ok((columns, rows)) => {
            Html(page(&sql_page_html(&sql, Some((columns, rows)), None))).into_response()
        }
        Err(e) => Html(page(&sql_page_html(&sql, None, Some(e.to_string())))).into_response(),
    }
}

fn sql_page_html(
    sql: &str,
    results: Option<(Vec<String>, Vec<Value>)>,
    error: Option<String>,
) -> String {
    let presets_json: Vec<Value> = PRESETS
        .iter()
        .map(|(_, q)| Value::String(q.to_string()))
        .collect();
    let presets_json = serde_json::to_string(&presets_json).unwrap_or_else(|_| "[]".to_string());
    let opts: String = PRESETS
        .iter()
        .enumerate()
        .map(|(i, (name, _))| format!("<option value=\"{i}\">{}</option>", esc(name)))
        .collect();
    let err_html = error
        .map(|e| format!("<div class=\"err\">{}</div>", esc(&e)))
        .unwrap_or_default();
    let results_html = match results {
        Some((cols, rows)) => render_table(&cols, &rows),
        None => String::new(),
    };

    format!(
        r#"<div class="row"><div class="sqlwrap"><h1>SQL-Konsole</h1></div><a class="back" href="/">← Dashboard</a></div>
<p class="muted">Read-only (<code>SELECT</code>/<code>WITH</code>) auf der geteilten <code>hub.db</code>.</p>
{err_html}
<form method="post" action="/sql">
  <div class="row"><select id="preset" onchange="setPreset(this.value)"><option value="">Vorlage…</option>{opts}</select>
  <button class="primary" type="submit">Ausführen</button></div>
  <textarea id="sql" name="sql" rows="6" spellcheck="false">{sql_text}</textarea>
</form>
{results_html}
<script>
const PRESETS = {presets_json};
function setPreset(i){{ if (i === "") return; document.getElementById("sql").value = PRESETS[+i]; }}
</script>"#,
        err_html = err_html,
        opts = opts,
        sql_text = esc(sql),
        results_html = results_html,
        presets_json = presets_json,
    )
}

fn render_table(columns: &[String], rows: &[Value]) -> String {
    let cap = 2000usize;
    let shown = rows.len().min(cap);
    let note = if rows.len() > cap {
        format!(" (erste {cap} angezeigt)")
    } else {
        String::new()
    };
    let mut out = format!("<p class=\"muted\">{} Zeile(n){note}</p><table>", rows.len());
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

/// `Some((connected, display_name))` when the user has a linked account.
async fn spotify_status(st: &AppState, user_id: i64) -> Option<(bool, Option<String>)> {
    let row = sqlx::query(
        "SELECT display_name, access_token FROM hub_service_accounts
          WHERE user_id = ?1 AND service = 'spotify'",
    )
    .bind(user_id)
    .fetch_optional(&st.pool)
    .await
    .ok()
    .flatten()?;
    let display: Option<String> = row.get("display_name");
    let access: Option<String> = row.get("access_token");
    Some((access.map(|a| !a.is_empty()).unwrap_or(false), display))
}

async fn login_form() -> Response {
    Html(login_page(None)).into_response()
}

async fn signup_form() -> Response {
    Html(signup_page(None)).into_response()
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
        return Html(login_page(Some("Benutzername oder Passwort falsch."))).into_response();
    }

    let uid: i64 = row.unwrap().get("id");
    match create_session(&st.pool, uid).await {
        Ok(tok) => with_cookie(Redirect::to("/").into_response(), session_cookie(&tok)),
        Err(e) => error_page(&format!("Session-Fehler: {e}")),
    }
}

async fn signup_submit(State(st): State<AppState>, Form(c): Form<Creds>) -> Response {
    let username = c.username.trim();
    if username.len() < 2 || c.password.len() < 4 {
        return Html(signup_page(Some("Benutzername (min. 2) und Passwort (min. 4) sind zu kurz.")))
            .into_response();
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
        return Html(signup_page(Some("Benutzername ist schon vergeben."))).into_response();
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

    let (remote_id, display_name) = match spotify::api_get(&tokens.access_token, "/me").await {
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
