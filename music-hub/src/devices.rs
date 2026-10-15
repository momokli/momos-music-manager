//! Hub-side per-device token registry (issue #363, ADR-081 §10.2).
//!
//! A "kellner" (device) pairs with a user account through a two-token model:
//!
//! * **Device token** — durable credential, generated *on the device*. The hub
//!   only ever stores its SHA-256. It becomes the `Bearer` credential for the
//!   device-gated routes in #364/#365 (it is persisted in `token_hash` after
//!   binding).
//! * **Pairing token** — one-time capability, also generated on the device and
//!   embedded in the approval URL (`/kellner/anheuern?token=…`). While the row
//!   is *pending* the hub stores only its hash in `token_hash`; on approval the
//!   pairing hash is rotated to the device-token hash, so a later-leaked link
//!   can no longer re-bind (ADR a re-hijack guarantee). Unused pending rows
//!   expire after a TTL.
//!
//! Registration is unauthenticated (bootstrap); list/revoke and the approval
//! page require a web session (`hub_web_sessions`), like the other `/api/hub/*`
//! routes. Tokens are never echoed back except the raw pairing token when the
//! client supplied it (needed to build the approval link locally).

use askama::Template;
use axum::extract::{Form, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{Value, json};
use sqlx::{Row, SqlitePool};

use crate::api::AppState;

/// Pending rows live for this many seconds before they can no longer bind.
pub const PENDING_TTL_SECS: i64 = 15 * 60;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/hub/devices", post(register).get(list))
        .route("/api/hub/devices/{id}", axum::routing::delete(revoke_handler))
        .route(
            "/kellner/anheuern",
            get(approval_page).post(approval_submit),
        )
        .with_state(state)
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ── token helpers ───────────────────────────────────────────────────────────

/// SHA-256 of `s` as lowercase hex. The hub stores/compares only hashes.
pub fn hash_token(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    let out = hasher.finalize();
    let mut hex = String::with_capacity(out.len() * 2);
    for b in out {
        hex.push_str(&format!("{b:02x}"));
    }
    hex
}

/// Constant-time string comparison (avoids leaking hash prefixes by timing).
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn is_hex(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// A raw token generated on the device: hex, at least 16 bytes (32 chars).
fn valid_raw_token(s: &str) -> bool {
    is_hex(s) && s.len() >= 32
}

/// A client-supplied SHA-256 hex digest (32 bytes → 64 chars).
fn valid_hash(s: &str) -> bool {
    is_hex(s) && s.len() == 64
}

// ── model ───────────────────────────────────────────────────────────────────

/// A `hub_devices` row, token material excluded.
#[derive(Debug, Clone)]
pub struct Device {
    pub id: i64,
    pub user_id: Option<i64>,
    pub name: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
    pub paired_at: Option<String>,
    pub expires_at: Option<String>,
    pub revoked_at: Option<String>,
}

impl Device {
    fn from_row(r: &sqlx::sqlite::SqliteRow) -> Self {
        Device {
            id: r.get("id"),
            user_id: r.get("user_id"),
            name: r.get("name"),
            created_at: r.get("created_at"),
            last_seen_at: r.get("last_seen_at"),
            paired_at: r.get("paired_at"),
            expires_at: r.get("expires_at"),
            revoked_at: r.get("revoked_at"),
        }
    }

    /// The JSON shape exposed to owners — never contains `token_hash`.
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id,
            "name": self.name,
            "created_at": self.created_at,
            "last_seen_at": self.last_seen_at,
            "paired_at": self.paired_at,
            "revoked_at": self.revoked_at,
        })
    }
}

const DEVICE_COLS: &str =
    "id, user_id, name, created_at, last_seen_at, paired_at, expires_at, revoked_at";

// ── queries ─────────────────────────────────────────────────────────────────

/// Delete pending rows whose TTL has elapsed (never touches bound/revoked rows).
pub async fn sweep_expired(pool: &SqlitePool) -> sqlx::Result<u64> {
    let res = sqlx::query(
        "DELETE FROM hub_devices
          WHERE user_id IS NULL AND revoked_at IS NULL
            AND expires_at IS NOT NULL AND expires_at <= ?1",
    )
    .bind(now_iso())
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Create a fresh pending row, or **re-pair** the single live pending row for
/// this `name` (one live pending row per name avoids pending-row spam from the
/// unauthenticated register route). Expired rows are swept first.
pub async fn create_pending(
    pool: &SqlitePool,
    name: &str,
    pairing_hash: &str,
    device_hash: &str,
) -> sqlx::Result<i64> {
    sweep_expired(pool).await?;
    let now = now_iso();
    let expires = (chrono::Utc::now() + chrono::Duration::seconds(PENDING_TTL_SECS)).to_rfc3339();

    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM hub_devices
          WHERE name = ?1 AND user_id IS NULL AND revoked_at IS NULL
            AND expires_at IS NOT NULL AND expires_at > ?2
          LIMIT 1",
    )
    .bind(name)
    .bind(&now)
    .fetch_optional(pool)
    .await?;

    if let Some(id) = existing {
        sqlx::query(
            "UPDATE hub_devices
                SET token_hash = ?1, pending_token_hash = ?2, created_at = ?3, expires_at = ?4
              WHERE id = ?5",
        )
        .bind(pairing_hash)
        .bind(device_hash)
        .bind(&now)
        .bind(&expires)
        .bind(id)
        .execute(pool)
        .await?;
        return Ok(id);
    }

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_devices (user_id, name, token_hash, pending_token_hash, created_at, expires_at)
         VALUES (NULL, ?1, ?2, ?3, ?4, ?5) RETURNING id",
    )
    .bind(name)
    .bind(pairing_hash)
    .bind(device_hash)
    .bind(&now)
    .bind(&expires)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Look up an unbindable-until-now pending row by its **pairing** hash.
pub async fn find_pending_by_pairing_hash(
    pool: &SqlitePool,
    pairing_hash: &str,
) -> sqlx::Result<Option<Device>> {
    let row = sqlx::query(&format!(
        "SELECT {DEVICE_COLS} FROM hub_devices
          WHERE token_hash = ?1 AND user_id IS NULL AND revoked_at IS NULL
            AND expires_at IS NOT NULL AND expires_at > ?2
          LIMIT 1"
    ))
    .bind(pairing_hash)
    .bind(now_iso())
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(Device::from_row))
}

/// Bind a pending device to `user_id`: rotate the pairing hash to the staged
/// device-token hash and clear the TTL. Returns `true` when a pending row was
/// bound (single-use — a second attempt finds nothing).
pub async fn bind_device(pool: &SqlitePool, id: i64, user_id: i64) -> sqlx::Result<bool> {
    let now = now_iso();
    let res = sqlx::query(
        "UPDATE hub_devices
            SET user_id = ?1, paired_at = ?2, token_hash = pending_token_hash,
                pending_token_hash = NULL, expires_at = NULL
          WHERE id = ?3 AND user_id IS NULL AND revoked_at IS NULL
            AND expires_at IS NOT NULL AND expires_at > ?2
            AND pending_token_hash IS NOT NULL",
    )
    .bind(user_id)
    .bind(&now)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Look up a **bound** device by its device-token hash (issue #364): the row
/// must be bound to a user, paired, and not revoked. Unknown / pending / revoked
/// tokens all yield `None` (no existence leak).
///
/// This is the authentication primitive for the device-gated routes (#364+):
/// a raw `Bearer` device token is hashed with [`hash_token`] and looked up here.
pub async fn find_bound_by_token_hash(
    pool: &SqlitePool,
    token_hash: &str,
) -> sqlx::Result<Option<Device>> {
    let row = sqlx::query(&format!(
        "SELECT {DEVICE_COLS} FROM hub_devices
          WHERE token_hash = ?1 AND user_id IS NOT NULL
            AND revoked_at IS NULL AND paired_at IS NOT NULL
          LIMIT 1"
    ))
    .bind(token_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(Device::from_row))
}

/// Record that a bound device just used a device-gated route (#364).
/// Best-effort: the caller logs but never fails on an error here.
pub async fn touch_last_seen(pool: &SqlitePool, id: i64) -> sqlx::Result<()> {
    sqlx::query("UPDATE hub_devices SET last_seen_at = ?1 WHERE id = ?2")
        .bind(now_iso())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// All devices owned by `user_id` (revoked ones included, so a client can show
/// history); never exposes token hashes.
pub async fn list_for_user(pool: &SqlitePool, user_id: i64) -> sqlx::Result<Vec<Device>> {
    let rows = sqlx::query(&format!(
        "SELECT {DEVICE_COLS} FROM hub_devices WHERE user_id = ?1 ORDER BY id"
    ))
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(Device::from_row).collect())
}

/// Revoke a device owned by `user_id`. Idempotent: a second call leaves the
/// original `revoked_at` untouched. Returns `false` when the id is unknown or
/// owned by another user (no existence leak).
pub async fn revoke(pool: &SqlitePool, id: i64, user_id: i64) -> sqlx::Result<bool> {
    let owned: Option<i64> =
        sqlx::query_scalar("SELECT id FROM hub_devices WHERE id = ?1 AND user_id = ?2")
            .bind(id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    if owned.is_none() {
        return Ok(false);
    }
    sqlx::query("UPDATE hub_devices SET revoked_at = ?1 WHERE id = ?2 AND revoked_at IS NULL")
        .bind(now_iso())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(true)
}

// ── HTTP: register / list / revoke ──────────────────────────────────────────

fn err(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({ "error": msg }))).into_response()
}

#[derive(Deserialize)]
struct RegisterBody {
    name: String,
    /// Raw pairing token generated on the device (echoed back in the link).
    pairing_token: Option<String>,
    /// Raw device token generated on the device (stored hashed, never echoed).
    device_token: Option<String>,
    /// Pre-hashed alternatives (hex SHA-256) for clients that hash locally.
    pairing_token_hash: Option<String>,
    device_token_hash: Option<String>,
}

/// `POST /api/hub/devices` — bootstrap register / re-pair (no session). Creates
/// or reuses a pending row and returns `{ device_id, approval_path }`.
async fn register(State(st): State<AppState>, Json(body): Json<RegisterBody>) -> Response {
    let name = body.name.trim().to_string();
    if name.is_empty() || name.len() > 128 {
        return err(StatusCode::BAD_REQUEST, "name required");
    }

    let (pairing_hash, echo) = if let Some(raw) = body.pairing_token.as_deref() {
        if !valid_raw_token(raw) {
            return err(StatusCode::BAD_REQUEST, "invalid pairing_token");
        }
        (hash_token(raw), Some(raw.to_string()))
    } else if let Some(h) = body.pairing_token_hash.as_deref() {
        if !valid_hash(h) {
            return err(StatusCode::BAD_REQUEST, "invalid pairing_token_hash");
        }
        (h.to_ascii_lowercase(), None)
    } else {
        return err(StatusCode::BAD_REQUEST, "pairing_token required");
    };

    let device_hash = if let Some(raw) = body.device_token.as_deref() {
        if !valid_raw_token(raw) {
            return err(StatusCode::BAD_REQUEST, "invalid device_token");
        }
        hash_token(raw)
    } else if let Some(h) = body.device_token_hash.as_deref() {
        if !valid_hash(h) {
            return err(StatusCode::BAD_REQUEST, "invalid device_token_hash");
        }
        h.to_ascii_lowercase()
    } else {
        return err(StatusCode::BAD_REQUEST, "device_token required");
    };

    let id = match create_pending(&st.pool, &name, &pairing_hash, &device_hash).await {
        Ok(id) => id,
        Err(e) => {
            tracing::warn!(error = %e, "register device failed");
            return err(StatusCode::INTERNAL_SERVER_ERROR, "register failed");
        }
    };

    let approval_path = match echo {
        Some(t) => format!("/kellner/anheuern?token={t}"),
        None => "/kellner/anheuern?token={token}".to_string(),
    };
    Json(json!({ "device_id": id, "approval_path": approval_path })).into_response()
}

/// `GET /api/hub/devices` — the caller's devices (web-session gated).
async fn list(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let Some((user_id, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    match list_for_user(&st.pool, user_id).await {
        Ok(devices) => {
            let arr: Vec<Value> = devices.iter().map(Device::to_json).collect();
            Json(Value::Array(arr)).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "list devices failed");
            err(StatusCode::INTERNAL_SERVER_ERROR, "list failed")
        }
    }
}

/// `DELETE /api/hub/devices/{id}` — revoke an owned device (web-session gated).
async fn revoke_handler(
    State(st): State<AppState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some((user_id, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    match revoke(&st.pool, id, user_id).await {
        Ok(true) => Json(json!({ "ok": true })).into_response(),
        Ok(false) => err(StatusCode::NOT_FOUND, "unknown device"),
        Err(e) => {
            tracing::warn!(error = %e, "revoke device failed");
            err(StatusCode::INTERNAL_SERVER_ERROR, "revoke failed")
        }
    }
}

// ── HTTP: approval page ─────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ApprovalQuery {
    token: Option<String>,
}

#[derive(Deserialize)]
struct ApprovalForm {
    token: String,
    decision: Option<String>,
}

#[derive(Template)]
#[template(path = "kellner_anheuern.html")]
struct ApprovalPage {
    nav: crate::ui::Nav,
    flash: String,
    done: bool,
    device_name: String,
    token: String,
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

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Friendly error page for an invalid / expired / already-bound token.
fn token_error_page(status: StatusCode, msg: &str) -> Response {
    let html = format!(
        "<!doctype html><html lang=\"de\"><head><meta charset=\"utf-8\">\
         <title>Anheuern — Spelunke</title></head><body>\
         <main><h1>Anheuern nicht möglich</h1><p>{}</p>\
         <p><a href=\"/\">← Zur Startseite</a></p></main></body></html>",
        esc(msg)
    );
    (status, Html(html)).into_response()
}

/// Resolve `token` to a pending device, or a friendly 404/410 response.
async fn pending_or_error(st: &AppState, token: &str) -> Result<Device, Response> {
    if !valid_raw_token(token) {
        return Err(token_error_page(
            StatusCode::NOT_FOUND,
            "Dieser Anheuer-Link ist ungültig.",
        ));
    }
    match find_pending_by_pairing_hash(&st.pool, &hash_token(token)).await {
        Ok(Some(d)) => Ok(d),
        Ok(None) => Err(token_error_page(
            StatusCode::GONE,
            "Dieser Anheuer-Link ist abgelaufen oder bereits verwendet worden.",
        )),
        Err(e) => {
            tracing::warn!(error = %e, "approval lookup failed");
            Err(err(StatusCode::INTERNAL_SERVER_ERROR, "lookup failed"))
        }
    }
}

/// `GET /kellner/anheuern?token=…` — the "diesen kellner anheuern?" page.
async fn approval_page(
    State(st): State<AppState>,
    Query(q): Query<ApprovalQuery>,
    headers: HeaderMap,
) -> Response {
    let token = q.token.unwrap_or_default();
    if crate::web::current_user(&st, &headers).await.is_none() {
        let target = format!("/kellner/anheuern?token={token}");
        let next = urlencoding::encode(&target);
        return Redirect::to(&format!("/login?next={next}")).into_response();
    }
    let nav = match crate::ui::nav(&st, &headers, "dashboard").await {
        Some(n) => n,
        None => return Redirect::to("/login").into_response(),
    };
    let device = match pending_or_error(&st, &token).await {
        Ok(d) => d,
        Err(resp) => return resp,
    };
    render(&ApprovalPage {
        nav,
        flash: String::new(),
        done: false,
        device_name: device.name,
        token,
    })
}

/// `POST /kellner/anheuern` — bind ("Ja") or decline ("Nein").
async fn approval_submit(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(form): Form<ApprovalForm>,
) -> Response {
    if crate::web::current_user(&st, &headers).await.is_none() {
        let target = format!("/kellner/anheuern?token={}", form.token);
        let next = urlencoding::encode(&target);
        return Redirect::to(&format!("/login?next={next}")).into_response();
    }
    let nav = match crate::ui::nav(&st, &headers, "dashboard").await {
        Some(n) => n,
        None => return Redirect::to("/login").into_response(),
    };
    let device = match pending_or_error(&st, &form.token).await {
        Ok(d) => d,
        Err(resp) => return resp,
    };

    if form.decision.as_deref() != Some("ja") {
        return Redirect::to("/").into_response();
    }

    let Some((user_id, _slug)) = crate::web::current_user(&st, &headers).await else {
        return Redirect::to("/login").into_response();
    };
    match bind_device(&st.pool, device.id, user_id).await {
        Ok(true) => render(&ApprovalPage {
            nav,
            flash: String::new(),
            done: true,
            device_name: device.name,
            token: String::new(),
        }),
        Ok(false) => token_error_page(
            StatusCode::GONE,
            "Dieses Gerät wurde bereits angeheuert.",
        ),
        Err(e) => {
            tracing::warn!(error = %e, "bind device failed");
            err(StatusCode::INTERNAL_SERVER_ERROR, "bind failed")
        }
    }
}

// ── unit tests (pure logic) ─────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_token_is_sha256_hex() {
        let h = hash_token("abc");
        assert_eq!(
            h,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(h.len(), 64);
    }

    #[test]
    fn constant_time_eq_matches_values() {
        assert!(constant_time_eq("deadbeef", "deadbeef"));
        assert!(!constant_time_eq("deadbeef", "deadbeee"));
        assert!(!constant_time_eq("dead", "deadbeef"));
        assert!(constant_time_eq("", ""));
    }

    #[test]
    fn token_and_hash_validation() {
        assert!(valid_raw_token(&"a".repeat(64)));
        assert!(!valid_raw_token(&"a".repeat(16))); // too short
        assert!(!valid_raw_token("not-hex-zzzz"));
        assert!(valid_hash(&hash_token("x")));
        assert!(!valid_hash("abc"));
    }
}
