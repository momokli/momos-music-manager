//! Per-device Kellner settings (issue #369, ADR-081 §3/§10.2).
//!
//! Per-device configuration (active backpacks, preferred export format, comment
//! template) is **authoritative in the Hub**, not on the device. The settings
//! live in two additive tables (`hub_device_settings`, `hub_device_backpacks`,
//! migration `052`):
//!
//! * `hub_device_settings` — 1:1 scalar settings per device (`preferred_format`,
//!   `comment_template`).
//! * `hub_device_backpacks` — the N:N set of *active* backpacks for a device,
//!   with a `position` column preserving the order.
//!
//! HTTP surface:
//!
//! ```text
//! GET  /api/hub/devices/{id}/settings   -> get_handler   (JSON, web-session)
//! PUT  /api/hub/devices/{id}/settings   -> put_handler   (JSON, web-session)
//! GET  /kellner                         -> page          (HTML, web-session)
//! POST /kellner/{id}/settings           -> form_submit   (HTML form POST)
//! ```
//!
//! All routes require a web session (`hub_web_sessions`) via
//! [`crate::web::current_user`]; no session redirects to `/login` exactly like
//! `devices.rs`. Ownership goes through [`crate::devices::find_owned`]; a foreign
//! or unknown `device_id` is indistinguishable and yields a `404` (no existence
//! leak). The JSON/HTML shapes never contain `token_hash` or any token material.

use axum::extract::{Form, Path, RawForm, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use askama::Template;
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::SqlitePool;

use crate::api::AppState;

/// The selectable export formats. Mirrors the DB `CHECK` on
/// `hub_device_settings.preferred_format`.
pub const FORMATS: [&str; 4] = ["flac", "mp3", "wav", "m4a"];

/// Longest accepted `comment_template`, measured in Unicode scalar values.
pub const MAX_TEMPLATE_CHARS: usize = 1024;

/// The placeholders a `comment_template` may reference, and the template applied
/// when the stored value is empty (issue #652). Re-exported from the
/// `comment-template` crate (issue #664) so the Hub and kellner share one
/// definition.
pub use comment_template::{ALLOWED_PLACEHOLDERS, DEFAULT_TEMPLATE};

pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/hub/devices/{id}/settings",
            get(get_handler).put(put_handler),
        )
        .route("/kellner", get(page))
        .route("/kellner/{id}/settings", post(form_submit))
        .route("/kellner/{id}/template/preview", post(template_preview))
        .with_state(state)
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ── model ───────────────────────────────────────────────────────────────────

/// One device's Kellner settings, resolved to defaults when no row exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSettings {
    pub device_id: i64,
    pub preferred_format: String,
    pub comment_template: String,
    /// Reference into the comment-template library (#669); `None` = no reference.
    pub comment_template_id: Option<i64>,
    pub active_backpack_ids: Vec<i64>,
    pub updated_at: Option<String>,
}

impl DeviceSettings {
    /// The JSON shape exposed to owners — never contains `token_hash`.
    pub fn to_json(&self) -> Value {
        json!({
            "device_id": self.device_id,
            "preferred_format": self.preferred_format,
            "comment_template": self.comment_template,
            "comment_template_id": self.comment_template_id,
            "active_backpack_ids": self.active_backpack_ids,
            "updated_at": self.updated_at,
        })
    }

    fn defaults(device_id: i64) -> Self {
        DeviceSettings {
            device_id,
            preferred_format: "flac".to_string(),
            comment_template: String::new(),
            comment_template_id: None,
            active_backpack_ids: Vec::new(),
            updated_at: None,
        }
    }
}

/// A partial PUT body (all fields optional): a missing field is left unchanged,
/// a present field fully replaces the stored value (the `active_backpack_ids`
/// list replaces the whole set). This makes the write idempotent and avoids
/// read-modify-write races between two clients.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SettingsBody {
    #[serde(default)]
    pub preferred_format: Option<String>,
    #[serde(default)]
    pub comment_template: Option<String>,
    /// Reference into the comment-template library (#669). `None` (absent) leaves
    /// the stored reference unchanged; a set id must be visible to the caller.
    #[serde(default)]
    pub comment_template_id: Option<i64>,
    /// Explicitly drop the stored `comment_template_id` (#669). Needed because
    /// JSON `null`/absent cannot be told apart in `Option<i64>`; `true` clears
    /// the reference (and must not be combined with a set `comment_template_id`).
    #[serde(default)]
    pub clear_comment_template: bool,
    #[serde(default)]
    pub active_backpack_ids: Option<Vec<i64>>,
}

// ── pure helpers ────────────────────────────────────────────────────────────

/// `true` when `s` is one of the four selectable formats (exact, lowercase).
pub fn valid_format(s: &str) -> bool {
    FORMATS.contains(&s)
}

/// Trim `s`, enforce the length cap and validate the placeholders. `""` is
/// valid: it means "use [`DEFAULT_TEMPLATE`]" (resolved on read/export).
pub fn clean_template(s: &str) -> Result<String, String> {
    let t = s.trim();
    if t.chars().count() > MAX_TEMPLATE_CHARS {
        return Err(format!(
            "comment_template too long (max {MAX_TEMPLATE_CHARS} characters)"
        ));
    }
    comment_template::validate(t)?;
    Ok(t.to_string())
}

/// Resolve a stored template to its effective value: an empty (or
/// whitespace-only) template falls back to [`DEFAULT_TEMPLATE`]. The stored
/// value is never rewritten — this is the read-side default.
pub fn resolve_template(stored: &str) -> String {
    let t = stored.trim();
    if t.is_empty() {
        DEFAULT_TEMPLATE.to_string()
    } else {
        t.to_string()
    }
}

/// Static resolution chain (#669): an explicit, visible template body wins; a
/// blank/absent one falls through to the legacy free text and finally to
/// [`DEFAULT_TEMPLATE`]. Kept pure so it can be unit-tested without a DB.
pub fn pick_comment(visible_body: Option<&str>, legacy_free: &str) -> String {
    match visible_body {
        Some(b) if !b.trim().is_empty() => b.trim().to_string(),
        _ => resolve_template(legacy_free),
    }
}

/// The single resolution API (#669) used by the export and the `/kellner`
/// preview. Loads the device settings, checks the referenced template's
/// visibility (`mine ∪ shared ∪ system` via [`comment_templates::get_visible`])
/// and applies [`pick_comment`]. A missing/deleted/invisible reference and any
/// read error fall through to the next stage (best effort — never a `500`).
///
/// Returns `(resolved_body, template_name)`, where the name is [`None`] unless
/// an explicit template was actually used (for logging/display only).
pub async fn resolve_effective_comment_named(
    pool: &SqlitePool,
    device_id: i64,
    user_id: i64,
) -> (String, Option<String>) {
    let settings = match get_for_device(pool, device_id, user_id).await {
        Ok(Some(s)) => s,
        Ok(None) => return (DEFAULT_TEMPLATE.to_string(), None),
        Err(e) => {
            tracing::warn!(error = %e, "resolve comment: settings lookup failed");
            return (DEFAULT_TEMPLATE.to_string(), None);
        }
    };

    let mut body: Option<String> = None;
    let mut name: Option<String> = None;
    if let Some(tid) = settings.comment_template_id {
        match crate::comment_templates::get_visible(pool, user_id, tid).await {
            Ok(Some(t)) => {
                body = Some(t.body);
                name = Some(t.name);
            }
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, "resolve comment: template lookup failed"),
        }
    }

    (pick_comment(body.as_deref(), &settings.comment_template), name)
}

/// Convenience wrapper around [`resolve_effective_comment_named`] returning only
/// the resolved body.
pub async fn resolve_effective_comment(
    pool: &SqlitePool,
    device_id: i64,
    user_id: i64,
) -> String {
    resolve_effective_comment_named(pool, device_id, user_id).await.0
}

// ── queries ─────────────────────────────────────────────────────────────────

async fn load_backpack_ids(pool: &SqlitePool, device_id: i64) -> sqlx::Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT backpack_id FROM hub_device_backpacks
          WHERE device_id = ?1 ORDER BY position, backpack_id",
    )
    .bind(device_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// The settings for a device owned by `user_id`, or `None` when the device is
/// foreign/unknown (both indistinguishable). A device without a settings row
/// resolves to the defaults — no auto-insert.
pub async fn get_for_device(
    pool: &SqlitePool,
    device_id: i64,
    user_id: i64,
) -> sqlx::Result<Option<DeviceSettings>> {
    if crate::devices::find_owned(pool, device_id, user_id)
        .await?
        .is_none()
    {
        return Ok(None);
    }
    let row: Option<(String, String, Option<i64>, String)> = sqlx::query_as(
        "SELECT preferred_format, comment_template, comment_template_id, updated_at
           FROM hub_device_settings WHERE device_id = ?1",
    )
    .bind(device_id)
    .fetch_optional(pool)
    .await?;

    let mut settings = match row {
        Some((fmt, tpl, tpl_id, updated)) => DeviceSettings {
            device_id,
            preferred_format: fmt,
            comment_template: tpl,
            comment_template_id: tpl_id,
            active_backpack_ids: Vec::new(),
            updated_at: Some(updated),
        },
        None => DeviceSettings::defaults(device_id),
    };
    settings.active_backpack_ids = load_backpack_ids(pool, device_id).await?;
    Ok(Some(settings))
}

/// Apply a partial patch to a device owned by `user_id`, in a single
/// transaction: upsert the scalar row, then replace the whole active-backpack
/// set (delete + re-insert in order). Returns the stored settings, or `None`
/// when the device is foreign/unknown.
///
/// Validation (`valid_format` / `clean_template` / backpack ownership) happens
/// in the HTTP handler; this function assumes an already-cleaned patch.
pub async fn upsert(
    pool: &SqlitePool,
    device_id: i64,
    user_id: i64,
    patch: SettingsBody,
) -> sqlx::Result<Option<DeviceSettings>> {
    if crate::devices::find_owned(pool, device_id, user_id)
        .await?
        .is_none()
    {
        return Ok(None);
    }

    let now = now_iso();
    let mut tx = pool.begin().await?;

    let existing: Option<(String, String, Option<i64>)> = sqlx::query_as(
        "SELECT preferred_format, comment_template, comment_template_id
           FROM hub_device_settings WHERE device_id = ?1",
    )
    .bind(device_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (cur_fmt, cur_tpl, cur_ref) = existing
        .unwrap_or_else(|| ("flac".to_string(), String::new(), None));

    let fmt = patch.preferred_format.unwrap_or(cur_fmt);
    let tpl = patch.comment_template.unwrap_or(cur_tpl);
    // `clear_comment_template` wins: an explicit drop sets the reference to NULL.
    // Otherwise COALESCE semantics: an absent reference leaves the stored one
    // unchanged (a set id replaces it).
    let tpl_ref = if patch.clear_comment_template {
        None
    } else {
        patch.comment_template_id.or(cur_ref)
    };

    sqlx::query(
        "INSERT INTO hub_device_settings
             (device_id, preferred_format, comment_template, comment_template_id, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(device_id) DO UPDATE SET
             preferred_format = excluded.preferred_format,
             comment_template = excluded.comment_template,
             comment_template_id = excluded.comment_template_id,
             updated_at = excluded.updated_at",
    )
    .bind(device_id)
    .bind(&fmt)
    .bind(&tpl)
    .bind(tpl_ref)
    .bind(&now)
    .execute(&mut *tx)
    .await?;

    if let Some(ids) = patch.active_backpack_ids {
        sqlx::query("DELETE FROM hub_device_backpacks WHERE device_id = ?1")
            .bind(device_id)
            .execute(&mut *tx)
            .await?;
        for (pos, bid) in ids.iter().enumerate() {
            sqlx::query(
                "INSERT INTO hub_device_backpacks (device_id, backpack_id, position, added_at)
                 VALUES (?1, ?2, ?3, ?4)",
            )
            .bind(device_id)
            .bind(bid)
            .bind(pos as i64)
            .bind(&now)
            .execute(&mut *tx)
            .await?;
        }
    }

    tx.commit().await?;

    let mut settings = DeviceSettings {
        device_id,
        preferred_format: fmt,
        comment_template: tpl,
        comment_template_id: tpl_ref,
        active_backpack_ids: Vec::new(),
        updated_at: Some(now),
    };
    settings.active_backpack_ids = load_backpack_ids(pool, device_id).await?;
    Ok(Some(settings))
}

// ── HTTP: JSON API ──────────────────────────────────────────────────────────

fn err(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({ "error": msg }))).into_response()
}

/// `GET /api/hub/devices/{id}/settings` — the caller's device settings (or
/// defaults). Web-session gated; foreign/unknown device → `404`.
async fn get_handler(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some((user_id, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    match get_for_device(&st.pool, id, user_id).await {
        Ok(Some(s)) => Json(s.to_json()).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "unknown device"),
        Err(e) => {
            tracing::warn!(error = %e, "load device settings failed");
            err(StatusCode::INTERNAL_SERVER_ERROR, "load failed")
        }
    }
}

/// `PUT /api/hub/devices/{id}/settings` — partial update. Validates the format,
/// the template length and the ownership of every referenced backpack (foreign
/// backpack → `400`, nothing written). Foreign/unknown device → `404`.
async fn put_handler(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Json(body): Json<SettingsBody>,
) -> Response {
    let Some((user_id, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };

    let patch = match validate_body(&st, user_id, body).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match upsert(&st.pool, id, user_id, patch).await {
        Ok(Some(s)) => Json(s.to_json()).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "unknown device"),
        Err(e) => {
            tracing::warn!(error = %e, "update device settings failed");
            err(StatusCode::INTERNAL_SERVER_ERROR, "update failed")
        }
    }
}

/// Validate a partial patch; on failure return the ready `400`/`500` response.
async fn validate_body(
    st: &AppState,
    user_id: i64,
    body: SettingsBody,
) -> Result<SettingsBody, Response> {
    if let Some(f) = body.preferred_format.as_deref() {
        if !valid_format(f) {
            return Err(err(
                StatusCode::BAD_REQUEST,
                "invalid preferred_format (expected flac|mp3|wav|m4a)",
            ));
        }
    }
    let comment_template = match body.comment_template.as_deref() {
        Some(t) => match clean_template(t) {
            Ok(c) => Some(c),
            Err(e) => return Err(err(StatusCode::BAD_REQUEST, &e)),
        },
        None => None,
    };

    // `clear` and a set id are contradictory: reject instead of guessing.
    if body.clear_comment_template && body.comment_template_id.is_some() {
        return Err(err(
            StatusCode::BAD_REQUEST,
            "clear_comment_template conflicts with comment_template_id",
        ));
    }

    // A set reference must be visible to the caller (mine ∪ shared ∪ system); a
    // foreign private template is rejected exactly like an unknown one. A pure
    // clear (`Some` absent) skips the lookup.
    if let Some(tid) = body.comment_template_id {
        match crate::comment_templates::get_visible(&st.pool, user_id, tid).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "unknown or invisible comment_template_id",
                ));
            }
            Err(e) => {
                tracing::warn!(error = %e, "comment template visibility check failed");
                return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "lookup failed"));
            }
        }
    }
    let active_backpack_ids = match &body.active_backpack_ids {
        Some(ids) => {
            for bid in ids {
                match crate::backpacks::backpack_owned_by(&st.pool, *bid, user_id).await {
                    Ok(Some(_)) => {}
                    Ok(None) => {
                        return Err(err(
                            StatusCode::BAD_REQUEST,
                            "unknown or foreign backpack in active_backpack_ids",
                        ));
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "backpack ownership check failed");
                        return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "lookup failed"));
                    }
                }
            }
            Some(ids.clone())
        }
        None => None,
    };

    Ok(SettingsBody {
        preferred_format: body.preferred_format,
        comment_template,
        comment_template_id: body.comment_template_id,
        clear_comment_template: body.clear_comment_template,
        active_backpack_ids,
    })
}

// ── HTTP: HTML page + form POST ─────────────────────────────────────────────

/// One format option in the `<select>`, with its selection state.
struct FormatOption {
    value: &'static str,
    selected: bool,
}

/// One backpack option in the multi-select, with its selection state.
struct BackpackOption {
    id: i64,
    name: String,
    selected: bool,
}

/// One comment-template option in the `<select>`, with its selection state.
struct TemplateOption {
    id: i64,
    name: String,
    selected: bool,
}

/// One device card of the `/kellner` view.
struct DeviceCard {
    id: i64,
    name: String,
    revoked: bool,
    /// The legacy inline template (shown only while non-empty, `#669`).
    legacy_comment: String,
    /// The effective comment after the resolution chain (preview, `#669`).
    resolved_comment: String,
    /// The pre-rendered `_partials/comment_preview.html` fragment (`#670`).
    preview_html: String,
    /// Selected template id, or `0` when no template is referenced (`#669`).
    selected_template_id: i64,
    comment_templates: Vec<TemplateOption>,
    formats: Vec<FormatOption>,
    backpacks: Vec<BackpackOption>,
}

#[derive(Template)]
#[template(path = "kellner_settings.html")]
struct KellnerPage {
    nav: crate::ui::Nav,
    flash: String,
    cards: Vec<DeviceCard>,
}

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

fn flash_redirect(to: &str, msg: String) -> Response {
    crate::flash::redirect(to, &msg, "ok")
}

/// `GET /kellner` — the Kellner settings page: every device of the caller with
/// editable format / template / active backpacks.
async fn page(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "kellner").await else {
        return Redirect::to("/login").into_response();
    };

    let devices = match crate::devices::list_for_user(&st.pool, nav.id).await {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "kellner settings: list devices failed");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Geräte konnten nicht geladen werden",
            )
                .into_response();
        }
    };
    let all_backpacks = crate::backpacks::list_backpacks(&st.pool, nav.id)
        .await
        .unwrap_or_default();
    // Templates visible to the caller (system ∪ own ∪ shared), loaded once.
    let visible_templates = crate::comment_templates::list_visible(&st.pool, nav.id, nav.is_admin)
        .await
        .unwrap_or_default();

    let mut cards = Vec::with_capacity(devices.len());
    for d in devices {
        let settings = get_for_device(&st.pool, d.id, nav.id)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| DeviceSettings::defaults(d.id));
        let backpacks = all_backpacks
            .iter()
            .map(|bp| BackpackOption {
                id: bp.id,
                name: bp.name.clone(),
                selected: settings.active_backpack_ids.contains(&bp.id),
            })
            .collect();
        let formats = FORMATS
            .iter()
            .map(|f| FormatOption {
                value: f,
                selected: *f == settings.preferred_format,
            })
            .collect();
        let comment_templates = visible_templates
            .iter()
            .map(|t| TemplateOption {
                id: t.id,
                name: t.name.clone(),
                selected: Some(t.id) == settings.comment_template_id,
            })
            .collect();
        let resolved_comment = resolve_effective_comment(&st.pool, d.id, nav.id).await;
        let preview_html = render_preview_fragment(&st.pool, nav.id, &resolved_comment).await;
        cards.push(DeviceCard {
            id: d.id,
            name: d.name,
            revoked: d.revoked_at.is_some(),
            legacy_comment: settings.comment_template,
            resolved_comment,
            preview_html,
            selected_template_id: settings.comment_template_id.unwrap_or(0),
            comment_templates,
            formats,
            backpacks,
        });
    }

    render(&KellnerPage {
        nav,
        flash: crate::flash::from_headers(&headers),
        cards,
    })
}

/// Parse an `application/x-www-form-urlencoded` body into `(key, value)` pairs,
/// preserving repeats (a multi-select posts the same key once per selection).
fn parse_form(body: &str) -> Vec<(String, String)> {
    body.split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (decode_form(k), decode_form(v))
        })
        .collect()
}

fn decode_form(s: &str) -> String {
    let plus_decoded = s.replace('+', " ");
    urlencoding::decode(&plus_decoded)
        .map(|c| c.into_owned())
        .unwrap_or(plus_decoded)
}

fn first<'a>(fields: &'a [(String, String)], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// `POST /kellner/{id}/settings` — the plain HTML form. Validates like the JSON
/// PUT and redirects back to `/kellner` (with a flash cookie) on error or success.
async fn form_submit(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    RawForm(raw): RawForm,
) -> Response {
    let Some((user_id, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };

    let body = String::from_utf8_lossy(&raw).into_owned();
    let fields = parse_form(&body);

    let fmt = first(&fields, "preferred_format").unwrap_or("");
    if !valid_format(fmt) {
        return flash_redirect("/kellner", "Fehler: ungültiges Ausgabeformat.".to_string());
    }

    let raw_tpl = first(&fields, "comment_template").unwrap_or("");
    let tpl = match clean_template(raw_tpl) {
        Ok(t) => t,
        Err(e) => return flash_redirect("/kellner", format!("Fehler: {e}")),
    };

    // Template reference (`#669`): empty = keep current (COALESCE semantics),
    // `"0"` = explicit drop (Legacy/Standard), otherwise the id must be visible.
    let mut clear = false;
    let raw_tid = first(&fields, "comment_template_id").unwrap_or("").trim();
    let tpl_id = if raw_tid.is_empty() {
        None
    } else if raw_tid == "0" {
        clear = true;
        None
    } else {
        match raw_tid.parse::<i64>() {
            Ok(n) => Some(n),
            Err(_) => {
                return flash_redirect("/kellner", "Fehler: ungültige Vorlagen-ID.".to_string())
            }
        }
    };
    if let Some(tid) = tpl_id {
        match crate::comment_templates::get_visible(&st.pool, user_id, tid).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return flash_redirect(
                    "/kellner",
                    "Fehler: unbekannte oder fremde Vorlage.".to_string(),
                )
            }
            Err(e) => {
                tracing::warn!(error = %e, "kellner form: template visibility check failed");
                return flash_redirect(
                    "/kellner",
                    "Fehler: Vorlagen-Prüfung fehlgeschlagen.".to_string(),
                );
            }
        }
    }

    let mut ids: Vec<i64> = Vec::new();
    let mut bad_id = false;
    for (k, v) in &fields {
        if k == "active_backpack_ids" {
            match v.parse::<i64>() {
                Ok(n) => ids.push(n),
                Err(_) => bad_id = true,
            }
        }
    }
    if bad_id {
        return flash_redirect("/kellner", "Fehler: ungültige Backpack-ID.".to_string());
    }
    for bid in &ids {
        match crate::backpacks::backpack_owned_by(&st.pool, *bid, user_id).await {
            Ok(Some(_)) => {}
            Ok(None) => {
                return flash_redirect(
                    "/kellner",
                    "Fehler: unbekanntes oder fremdes Backpack.".to_string(),
                );
            }
            Err(e) => {
                tracing::warn!(error = %e, "kellner form: backpack ownership check failed");
                return flash_redirect("/kellner", "Fehler: Backpack-Prüfung fehlgeschlagen.".to_string());
            }
        }
    }

    let patch = SettingsBody {
        preferred_format: Some(fmt.to_string()),
        comment_template: Some(tpl),
        comment_template_id: tpl_id,
        clear_comment_template: clear,
        active_backpack_ids: Some(ids),
    };

    match upsert(&st.pool, id, user_id, patch).await {
        Ok(Some(_)) => Redirect::to("/kellner").into_response(),
        Ok(None) => (StatusCode::NOT_FOUND, "Unbekanntes Gerät").into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "kellner form: upsert failed");
            flash_redirect("/kellner", "Fehler: Speichern fehlgeschlagen.".to_string())
        }
    }
}

// ── HTTP: comment-template preview (#670) ───────────────────────────────────

/// Render the `_partials/comment_preview.html` fragment for the `/kellner` card
/// (`#670`). Advisory: a resolve failure yields the empty hint, never a 500.
async fn render_preview_fragment(pool: &SqlitePool, user_id: i64, body: &str) -> String {
    match crate::preview::comment_preview(pool, user_id, body).await {
        Ok(p) => p.render().unwrap_or_default(),
        Err(e) => {
            tracing::warn!(error = %e, "comment preview failed");
            String::new()
        }
    }
}

/// The htmx preview form: the selected template id (`"0"` = none/legacy) and,
/// when shown, the legacy free text. Both optional — a bare request falls back
/// to `DEFAULT_TEMPLATE`.
#[derive(Debug, Deserialize, Default)]
struct PreviewForm {
    #[serde(default)]
    comment_template_id: Option<String>,
    #[serde(default)]
    comment_template: Option<String>,
}

/// `POST /kellner/{id}/template/preview` (`#670`) — htmx fragment. Web-session
/// gated; foreign/unknown device → `404`. The rendered body comes from the
/// **just-submitted** form value (selected template → legacy free text →
/// [`DEFAULT_TEMPLATE`]), never the stored selection, so the preview tracks the
/// form. Track source: the session user's top tagged tracks (`#678`).
async fn template_preview(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(f): Form<PreviewForm>,
) -> Response {
    let Some((user_id, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };

    // 404 scoping: foreign/unknown device is rejected. The settings value
    // itself is no longer needed — the track source is the session user.
    match get_for_device(&st.pool, id, user_id).await {
        Ok(Some(_)) => {}
        Ok(None) => return (StatusCode::NOT_FOUND, "Unbekanntes Gerät").into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "template preview: settings lookup failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Vorschau fehlgeschlagen").into_response();
        }
    }

    // Body: the just-selected template wins; an empty/`"0"` id falls back to the
    // legacy free text of the form and finally to DEFAULT_TEMPLATE.
    let raw_tid = f
        .comment_template_id
        .as_deref()
        .unwrap_or("")
        .trim()
        .to_string();
    let body = if !raw_tid.is_empty() && raw_tid != "0" {
        let tid = match raw_tid.parse::<i64>() {
            Ok(t) => t,
            Err(_) => return (StatusCode::NOT_FOUND, "Vorlage nicht gefunden.").into_response(),
        };
        match crate::comment_templates::get_visible(&st.pool, user_id, tid).await {
            Ok(Some(t)) => t.body,
            Ok(None) => {
                return (StatusCode::NOT_FOUND, "Vorlage nicht gefunden.").into_response()
            }
            Err(e) => {
                tracing::warn!(error = %e, "template preview: template lookup failed");
                return (StatusCode::INTERNAL_SERVER_ERROR, "Vorschau fehlgeschlagen")
                    .into_response();
            }
        }
    } else {
        pick_comment(None, f.comment_template.as_deref().unwrap_or(""))
    };

    match crate::preview::comment_preview(&st.pool, user_id, &body).await {
        Ok(p) => render(&p),
        Err(e) => {
            tracing::warn!(error = %e, "template preview: render failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "Vorschau fehlgeschlagen").into_response()
        }
    }
}

// ── unit tests (pure logic + JSON shape) ────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_format_accepts_only_known_lowercase() {
        for f in FORMATS {
            assert!(valid_format(f), "{f} should be valid");
        }
        assert!(!valid_format("ogg"));
        assert!(!valid_format(""));
        assert!(!valid_format("FLAC"), "case-sensitive: uppercase rejected");
        assert!(!valid_format("wav "));
    }

    #[test]
    fn clean_template_trims_and_caps() {
        assert_eq!(clean_template("  hi  ").unwrap(), "hi");
        assert_eq!(clean_template("").unwrap(), "");
        assert_eq!(clean_template("   ").unwrap(), "");
        let long = "a".repeat(MAX_TEMPLATE_CHARS);
        assert_eq!(clean_template(&long).unwrap().len(), MAX_TEMPLATE_CHARS);
        let too_long = "a".repeat(MAX_TEMPLATE_CHARS + 1);
        assert!(clean_template(&too_long).is_err());
        // Whitespace is trimmed *before* the length check.
        let padded = format!("  {}  ", "a".repeat(MAX_TEMPLATE_CHARS + 1));
        assert!(clean_template(&padded).is_err());
    }

    #[test]
    fn clean_template_accepts_allowed_placeholders() {
        for t in [
            DEFAULT_TEMPLATE,
            "{tags}",
            "{bpm} {key} {energy}",
            "tags: {tags}",
            "",
            "   ",
        ] {
            assert!(clean_template(t).is_ok(), "{t:?} should be accepted");
        }
    }

    #[test]
    fn clean_template_rejects_unknown_placeholder() {
        let err = clean_template("{title}").unwrap_err();
        assert!(err.contains("{title}"), "names the token: {err}");
        assert!(err.contains("{tags}"), "lists allowed: {err}");
        assert!(err.contains("{energy}"), "lists allowed: {err}");
        assert!(clean_template("{tags} {artist}").is_err());
    }

    #[test]
    fn clean_template_accepts_double_brace_edge_cases() {
        // `{{` is an escaped literal, never an unknown placeholder.
        for t in ["{{", "{{tags}}", "a {{ b", "{{{tags}}}", "}", "{tags", "no braces"] {
            assert!(clean_template(t).is_ok(), "{t:?} should be accepted");
        }
        // A `{name}` after an escaped pair is still validated.
        assert!(clean_template("{{ {nope}").is_err());
    }

    #[test]
    fn resolve_template_falls_back_to_default() {
        assert_eq!(resolve_template(""), DEFAULT_TEMPLATE);
        assert_eq!(resolve_template("   "), DEFAULT_TEMPLATE);
        assert_eq!(resolve_template("{tags}"), "{tags}");
        assert_eq!(resolve_template("  {tags}  "), "{tags}");
    }

    #[test]
    fn pick_comment_matrix() {
        // A non-blank visible template body wins.
        assert_eq!(pick_comment(Some("{key} / {bpm}"), "legacy"), "{key} / {bpm}");
        assert_eq!(pick_comment(Some("{tags}"), ""), "{tags}");
        assert_eq!(pick_comment(Some("  {tags}  "), "legacy"), "{tags}");
        // Absent / blank template -> legacy free text.
        assert_eq!(pick_comment(None, "legacy"), "legacy");
        assert_eq!(pick_comment(Some(""), "legacy"), "legacy");
        assert_eq!(pick_comment(Some("   "), "legacy"), "legacy");
        // Both empty -> the default template.
        assert_eq!(pick_comment(None, ""), DEFAULT_TEMPLATE);
        assert_eq!(pick_comment(None, "   "), DEFAULT_TEMPLATE);
        assert_eq!(pick_comment(Some("   "), ""), DEFAULT_TEMPLATE);
    }

    #[test]
    fn to_json_has_exact_keys_and_no_secret() {
        let s = DeviceSettings {
            device_id: 7,
            preferred_format: "mp3".to_string(),
            comment_template: "hi".to_string(),
            comment_template_id: Some(42),
            active_backpack_ids: vec![3, 5],
            updated_at: Some("2026-01-01T00:00:00+00:00".to_string()),
        };
        let v = s.to_json();
        let obj = v.as_object().expect("object");
        let mut keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            vec![
                "active_backpack_ids",
                "comment_template",
                "comment_template_id",
                "device_id",
                "preferred_format",
                "updated_at"
            ]
        );
        assert_eq!(v["device_id"], 7);
        assert_eq!(v["comment_template_id"], 42);
        assert_eq!(v["active_backpack_ids"], json!([3, 5]));
        assert!(!v.to_string().contains("token_hash"));
    }

    #[test]
    fn defaults_are_flac_empty_no_backpacks() {
        let d = DeviceSettings::defaults(1);
        assert_eq!(d.preferred_format, "flac");
        assert_eq!(d.comment_template, "");
        assert!(d.comment_template_id.is_none());
        assert!(d.active_backpack_ids.is_empty());
        assert!(d.updated_at.is_none());
    }

    #[test]
    fn parse_form_keeps_repeats_and_decodes() {
        let f = parse_form("preferred_format=mp3&active_backpack_ids=3&active_backpack_ids=5&comment_template=a+b%26c");
        assert_eq!(first(&f, "preferred_format"), Some("mp3"));
        let ids: Vec<&str> = f
            .iter()
            .filter(|(k, _)| k == "active_backpack_ids")
            .map(|(_, v)| v.as_str())
            .collect();
        assert_eq!(ids, vec!["3", "5"]);
        assert_eq!(first(&f, "comment_template"), Some("a b&c"));
    }
}
