//! Device-token-gated JSON read endpoints for backpacks (issue #367, ADR-081).
//!
//! A paired kellner reads its owner's backpacks and one backpack's resolved
//! track export over `Authorization: Bearer <device token>`:
//!
//! * `GET /api/hub/backpacks` — the owner's backpacks (id / name / is_active).
//! * `GET /api/hub/backpacks/{id}/export` — the resolved tracks + per-track
//!   metadata (ISRC, tags, BPM/key/energy) the device needs to sync locally.
//!
//! Auth mirrors [`crate::traktor_api`]: hash the raw `Bearer` token, resolve the
//! bound device via [`crate::devices::find_bound_by_token_hash`], take its
//! `user_id` and best-effort touch `last_seen_at`. All reads are **owner-scoped**
//! — a foreign backpack is indistinguishable from a missing one (`404`).

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, header::AUTHORIZATION};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::json;
use sqlx::SqlitePool;

use crate::api::AppState;
use crate::backpacks;
use crate::devices;

/// Device-gated backpack read routes.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/hub/backpacks", get(list))
        .route("/api/hub/backpacks/{id}/export", get(export))
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

/// Resolve the `Bearer` device token to its owning `user_id`, or an error
/// response (`401`). Touches `last_seen_at` best-effort (never fails the read).
async fn bound_user(st: &AppState, headers: &HeaderMap) -> Result<i64, Response> {
    let Some(token) = bearer_token(headers) else {
        return Err(err(StatusCode::UNAUTHORIZED, "missing device token"));
    };

    let device = match devices::find_bound_by_token_hash(&st.pool, &devices::hash_token(&token)).await
    {
        Ok(Some(d)) => d,
        Ok(None) => return Err(err(StatusCode::UNAUTHORIZED, "unknown or unbound device token")),
        Err(e) => {
            tracing::warn!(error = %e, "backpack api: device lookup failed");
            return Err(err(StatusCode::INTERNAL_SERVER_ERROR, "device lookup failed"));
        }
    };
    let Some(user_id) = device.user_id else {
        return Err(err(StatusCode::UNAUTHORIZED, "device not bound"));
    };
    if let Err(e) = devices::touch_last_seen(&st.pool, device.id).await {
        tracing::warn!(error = %e, "backpack api: last_seen update failed");
    }
    Ok(user_id)
}

/// `GET /api/hub/backpacks` — the owner's backpacks, in creation order.
async fn list(State(st): State<AppState>, headers: HeaderMap) -> Response {
    let user_id = match bound_user(&st, &headers).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let backpacks = match backpacks::list_backpacks(&st.pool, user_id).await {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "backpack api: list failed");
            return err(StatusCode::INTERNAL_SERVER_ERROR, "list failed");
        }
    };

    let items: Vec<_> = backpacks
        .iter()
        .map(|b| {
            json!({
                "id": b.id,
                "name": b.name,
                "is_active": b.is_active != 0,
            })
        })
        .collect();
    Json(json!({ "backpacks": items })).into_response()
}

/// Per-track metadata read straight from the hub read-models: `isrc`/`title`/
/// `artists` from `hub_tracks`, the chip set from `hub_tags`, and the resolved
/// audio values from `hub_track_audio` (all nullable → `null`).
async fn track_meta(pool: &SqlitePool, track_id: i64) -> sqlx::Result<(String, String, String)> {
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT COALESCE(isrc, '') AS isrc,
                COALESCE(title, '') AS title,
                COALESCE(artists, '') AS artists
           FROM hub_tracks WHERE id = ?1",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.unwrap_or_default())
}

/// The track's tag names, alphabetical (`ORDER BY name`).
async fn track_tags(pool: &SqlitePool, track_id: i64) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT h.name
           FROM hub_track_resolved_tags rt
           JOIN hub_tags h ON h.id = rt.tag_id
          WHERE rt.track_id = ?1
          ORDER BY h.name",
    )
    .bind(track_id)
    .fetch_all(pool)
    .await
}

/// The track's resolved `bpm` / `musical_key` / `energy`, all `None` when the
/// row (or any column) is absent.
async fn track_audio(
    pool: &SqlitePool,
    track_id: i64,
) -> sqlx::Result<(Option<f64>, Option<String>, Option<f64>)> {
    let row: Option<(Option<f64>, Option<String>, Option<f64>)> = sqlx::query_as(
        "SELECT bpm, musical_key, energy FROM hub_track_audio WHERE track_id = ?1",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.unwrap_or((None, None, None)))
}

/// `GET /api/hub/backpacks/{id}/export` — resolved tracks + metadata, in
/// resolve order. An unknown/foreign backpack is a `404`; an empty one yields
/// `tracks: []` (never a `500`).
async fn export(State(st): State<AppState>, Path(id): Path<i64>, headers: HeaderMap) -> Response {
    let user_id = match bound_user(&st, &headers).await {
        Ok(id) => id,
        Err(resp) => return resp,
    };

    let backpack = match backpacks::backpack_owned_by(&st.pool, id, user_id).await {
        Ok(Some(b)) => b,
        Ok(None) => return err(StatusCode::NOT_FOUND, "unknown backpack"),
        Err(e) => {
            tracing::warn!(error = %e, "backpack api: backpack lookup failed");
            return err(StatusCode::INTERNAL_SERVER_ERROR, "backpack lookup failed");
        }
    };

    let resolved = match backpacks::export_tracks(&st.pool, id).await {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!(error = %e, "backpack api: export failed");
            return err(StatusCode::INTERNAL_SERVER_ERROR, "export failed");
        }
    };

    let mut tracks = Vec::with_capacity(resolved.len());
    for t in resolved {
        let (isrc, title, artists) = match track_meta(&st.pool, t.track_id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "backpack api: track metadata failed");
                return err(StatusCode::INTERNAL_SERVER_ERROR, "track metadata failed");
            }
        };
        let tags = match track_tags(&st.pool, t.track_id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "backpack api: track tags failed");
                return err(StatusCode::INTERNAL_SERVER_ERROR, "track tags failed");
            }
        };
        let (bpm, key, energy) = match track_audio(&st.pool, t.track_id).await {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "backpack api: track audio failed");
                return err(StatusCode::INTERNAL_SERVER_ERROR, "track audio failed");
            }
        };

        tracks.push(json!({
            "track_id": t.track_id,
            "isrc": isrc,
            "title": title,
            "artists": artists,
            "tags": tags,
            "bpm": bpm,
            "key": key,
            "energy": energy,
        }));
    }

    Json(json!({
        "backpack": { "id": backpack.id, "name": backpack.name },
        "tracks": tracks,
    }))
    .into_response()
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
