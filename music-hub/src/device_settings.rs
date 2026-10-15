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

use axum::extract::{Path, Query, RawForm, State};
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

pub fn router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/hub/devices/{id}/settings",
            get(get_handler).put(put_handler),
        )
        .route("/kellner", get(page))
        .route("/kellner/{id}/settings", post(form_submit))
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
            "active_backpack_ids": self.active_backpack_ids,
            "updated_at": self.updated_at,
        })
    }

    fn defaults(device_id: i64) -> Self {
        DeviceSettings {
            device_id,
            preferred_format: "flac".to_string(),
            comment_template: String::new(),
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
    #[serde(default)]
    pub active_backpack_ids: Option<Vec<i64>>,
}

// ── pure helpers ────────────────────────────────────────────────────────────

/// `true` when `s` is one of the four selectable formats (exact, lowercase).
pub fn valid_format(s: &str) -> bool {
    FORMATS.contains(&s)
}

/// Trim `s` and enforce the length cap. `""` is valid (v1 default = the
/// alphabetical tag list is applied downstream, not here).
pub fn clean_template(s: &str) -> Result<String, String> {
    let t = s.trim();
    if t.chars().count() > MAX_TEMPLATE_CHARS {
        return Err(format!(
            "comment_template too long (max {MAX_TEMPLATE_CHARS} characters)"
        ));
    }
    Ok(t.to_string())
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
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT preferred_format, comment_template, updated_at
           FROM hub_device_settings WHERE device_id = ?1",
    )
    .bind(device_id)
    .fetch_optional(pool)
    .await?;

    let mut settings = match row {
        Some((fmt, tpl, updated)) => DeviceSettings {
            device_id,
            preferred_format: fmt,
            comment_template: tpl,
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

    let existing: Option<(String, String)> = sqlx::query_as(
        "SELECT preferred_format, comment_template
           FROM hub_device_settings WHERE device_id = ?1",
    )
    .bind(device_id)
    .fetch_optional(&mut *tx)
    .await?;
    let (cur_fmt, cur_tpl) = existing.unwrap_or_else(|| ("flac".to_string(), String::new()));

    let fmt = patch.preferred_format.unwrap_or(cur_fmt);
    let tpl = patch.comment_template.unwrap_or(cur_tpl);

    sqlx::query(
        "INSERT INTO hub_device_settings (device_id, preferred_format, comment_template, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(device_id) DO UPDATE SET
             preferred_format = excluded.preferred_format,
             comment_template = excluded.comment_template,
             updated_at = excluded.updated_at",
    )
    .bind(device_id)
    .bind(&fmt)
    .bind(&tpl)
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
        active_backpack_ids,
    })
}

// ── HTTP: HTML page + form POST ─────────────────────────────────────────────

#[derive(Deserialize, Default)]
struct PageQuery {
    msg: Option<String>,
}

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

/// One device card of the `/kellner` view.
struct DeviceCard {
    id: i64,
    name: String,
    revoked: bool,
    comment_template: String,
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
    let sep = if to.contains('?') { '&' } else { '?' };
    Redirect::to(&format!("{to}{sep}msg={}", urlencoding::encode(&msg))).into_response()
}

/// `GET /kellner` — the Kellner settings page: every device of the caller with
/// editable format / template / active backpacks.
async fn page(
    State(st): State<AppState>,
    Query(q): Query<PageQuery>,
    headers: HeaderMap,
) -> Response {
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
        cards.push(DeviceCard {
            id: d.id,
            name: d.name,
            revoked: d.revoked_at.is_some(),
            comment_template: settings.comment_template,
            formats,
            backpacks,
        });
    }

    render(&KellnerPage {
        nav,
        flash: q.msg.unwrap_or_default(),
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
/// PUT and redirects back to `/kellner?msg=…` on error (or success).
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
    fn to_json_has_exact_keys_and_no_secret() {
        let s = DeviceSettings {
            device_id: 7,
            preferred_format: "mp3".to_string(),
            comment_template: "hi".to_string(),
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
                "device_id",
                "preferred_format",
                "updated_at"
            ]
        );
        assert_eq!(v["device_id"], 7);
        assert_eq!(v["active_backpack_ids"], json!([3, 5]));
        assert!(!v.to_string().contains("token_hash"));
    }

    #[test]
    fn defaults_are_flac_empty_no_backpacks() {
        let d = DeviceSettings::defaults(1);
        assert_eq!(d.preferred_format, "flac");
        assert_eq!(d.comment_template, "");
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
