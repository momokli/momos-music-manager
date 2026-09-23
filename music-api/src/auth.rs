//! Bearer-token auth. Defence in depth: Caddy already checks the token at the
//! edge, but a misconfigured route must not expose the API, so every protected
//! handler checks it again.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::AppState;

pub async fn require_bearer(
    State(state): State<Arc<AppState>>,
    req: Request,
    next: Next,
) -> Response {
    let provided = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    match provided {
        Some(token) if ct_eq(token.as_bytes(), state.config.token.as_bytes()) => next.run(req).await,
        _ => (StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
    }
}

/// Length-checked, branch-free byte comparison to avoid leaking the token
/// through response timing.
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::ct_eq;

    #[test]
    fn compares() {
        assert!(ct_eq(b"secret", b"secret"));
        assert!(!ct_eq(b"secret", b"secreT"));
        assert!(!ct_eq(b"secret", b"secret-longer"));
        assert!(!ct_eq(b"", b"x"));
        assert!(ct_eq(b"", b""));
    }
}
