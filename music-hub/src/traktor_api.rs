//! Device-token-gated Traktor `collection.nml` ingest (issue #364, ADR-081 §10.5).
//!
//! A paired kellner POSTs its raw `collection.nml` here with
//! `Authorization: Bearer <device token>`; the hub hashes the token, resolves the
//! bound device (see [`crate::devices`]) and reuses [`crate::traktor::import_str`]
//! to land playcount / rating / playlist rows for that device's owner.
//!
//! The hub CLI `import-traktor` remains the operator-side path (ADR-081 §10.5);
//! this route is the only device-facing entry point for the upload.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::json;

use crate::api::AppState;
use crate::devices;

/// Hard cap on the uploaded NML body (16 MiB); larger bodies get a `413` from
/// [`DefaultBodyLimit`] before the handler runs.
pub const MAX_NML_BYTES: usize = 16 * 1024 * 1024;

/// Device-gated routes. Layered with its own (larger) body limit so the raw NML
/// upload is not clipped by the default 2 MiB.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/hub/traktor/import", post(import))
        .layer(DefaultBodyLimit::max(MAX_NML_BYTES))
        .with_state(state)
}

fn err(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({ "error": msg }))).into_response()
}

/// Extract a `Bearer <token>` credential (scheme matched case-insensitively).
fn bearer_token(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(AUTHORIZATION)?.to_str().ok()?.trim();
    let (scheme, token) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = token.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// `POST /api/hub/traktor/import` — raw NML body, device-token gated.
async fn import(State(st): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(token) = bearer_token(&headers) else {
        return err(StatusCode::UNAUTHORIZED, "missing device token");
    };

    let device = match devices::find_bound_by_token_hash(&st.pool, &devices::hash_token(&token)).await
    {
        Ok(Some(d)) => d,
        Ok(None) => return err(StatusCode::UNAUTHORIZED, "unknown or unbound device token"),
        Err(e) => {
            tracing::warn!(error = %e, "traktor import: device lookup failed");
            return err(StatusCode::INTERNAL_SERVER_ERROR, "device lookup failed");
        }
    };
    let Some(user_id) = device.user_id else {
        return err(StatusCode::UNAUTHORIZED, "device not bound");
    };
    // Best-effort last-seen touch (never fails the import).
    if let Err(e) = devices::touch_last_seen(&st.pool, device.id).await {
        tracing::warn!(error = %e, "traktor import: last_seen update failed");
    }

    if body.is_empty() {
        return err(StatusCode::BAD_REQUEST, "empty body");
    }
    let Ok(xml) = std::str::from_utf8(&body) else {
        return err(StatusCode::BAD_REQUEST, "body is not valid UTF-8");
    };
    if !xml.contains("<NML") {
        return err(StatusCode::BAD_REQUEST, "not an NML document");
    }

    match crate::traktor::import_str(&st.pool, user_id, xml).await {
        Ok(stats) => Json(json!({
            "ok": true,
            "stats": {
                "entries": stats.entries,
                "matched": stats.matched,
                "playlists": stats.playlists,
                "with_bpm": stats.with_bpm,
                "with_key": stats.with_key,
            }
        }))
        .into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "traktor import failed");
            err(StatusCode::BAD_REQUEST, "invalid NML")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn bearer_token_accepts_bearer_scheme() {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, HeaderValue::from_static("Bearer abc123"));
        assert_eq!(bearer_token(&h).as_deref(), Some("abc123"));

        h.insert(AUTHORIZATION, HeaderValue::from_static("bearer abc123"));
        assert_eq!(bearer_token(&h).as_deref(), Some("abc123"));
    }

    #[test]
    fn bearer_token_rejects_malformed() {
        let mut h = HeaderMap::new();
        h.insert(AUTHORIZATION, HeaderValue::from_static("basic abc"));
        assert_eq!(bearer_token(&h), None);
        h.insert(AUTHORIZATION, HeaderValue::from_static("Bearer "));
        assert_eq!(bearer_token(&h), None);
        assert_eq!(bearer_token(&HeaderMap::new()), None);
    }
}
