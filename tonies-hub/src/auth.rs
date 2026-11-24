//! Sessions (bcrypt-backed login) and the `tonies_session` cookie.

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use sqlx::SqlitePool;

pub const SESSION_COOKIE: &str = "tonies_session";
pub const SESSION_TTL_SECS: i64 = 60 * 60 * 24 * 30;
pub const BCRYPT_COST: u32 = 10;

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Extract a cookie value from the request headers.
pub fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
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

/// Resolve the session cookie to `(user_id, slug)`, ignoring expired sessions.
pub async fn current_user(pool: &SqlitePool, headers: &HeaderMap) -> Option<(i64, String)> {
    let token = cookie_value(headers, SESSION_COOKIE)?;
    sqlx::query_as::<_, (i64, String)>(
        "SELECT u.id, u.slug
           FROM sessions s
           JOIN users u ON u.id = s.user_id
          WHERE s.id = ?1 AND s.expires_at > ?2",
    )
    .bind(&token)
    .bind(now_iso())
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
}

/// 32-character alphanumeric session token.
fn random_token() -> String {
    use rand::Rng;
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut rng = rand::thread_rng();
    (0..32)
        .map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char)
        .collect()
}

/// Create a session row and return its token.
pub async fn create_session(pool: &SqlitePool, user_id: i64) -> anyhow::Result<String> {
    let token = random_token();
    let expires =
        (chrono::Utc::now() + chrono::Duration::seconds(SESSION_TTL_SECS)).to_rfc3339();
    sqlx::query(
        "INSERT INTO sessions (id, user_id, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&token)
    .bind(user_id)
    .bind(now_iso())
    .bind(expires)
    .execute(pool)
    .await?;
    Ok(token)
}

/// `Set-Cookie` value for a valid session.
pub fn session_cookie(token: &str, secure: bool) -> String {
    let mut cookie = format!(
        "{SESSION_COOKIE}={token}; HttpOnly; Path=/; Max-Age={SESSION_TTL_SECS}; SameSite=Lax"
    );
    if secure {
        cookie.push_str("; Secure");
    }
    cookie
}

/// `Set-Cookie` value that deletes the session cookie.
pub fn clear_cookie() -> String {
    format!("{SESSION_COOKIE}=; HttpOnly; Path=/; Max-Age=0; SameSite=Lax")
}

/// Attach a `Set-Cookie` header to a response.
pub fn with_cookie(mut resp: Response, cookie: String) -> Response {
    if let Ok(v) = HeaderValue::from_str(&cookie) {
        resp.headers_mut().insert(header::SET_COOKIE, v);
    }
    resp
}

/// Require a logged-in user: either `(user_id, slug)` or a 303 to `/login`.
pub async fn require_user(
    pool: &SqlitePool,
    headers: &HeaderMap,
) -> Result<(i64, String), Response> {
    match current_user(pool, headers).await {
        Some(user) => Ok(user),
        None => Err(Redirect::to("/login").into_response()),
    }
}

/// Hash a password with bcrypt (cost 10).
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    Ok(bcrypt::hash(password, BCRYPT_COST)?)
}

/// Verify a password against a bcrypt hash. Malformed hashes fail closed.
pub fn verify_password(password: &str, hash: &str) -> bool {
    bcrypt::verify(password, hash).unwrap_or(false)
}

pub fn unauthorized_page(msg: &str) -> Response {
    (StatusCode::UNAUTHORIZED, msg.to_string()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_32_alnum_chars() {
        let t = random_token();
        assert_eq!(t.len(), 32);
        assert!(t.chars().all(|c| c.is_ascii_alphanumeric()));
        assert_ne!(t, random_token());
    }

    #[test]
    fn bcrypt_roundtrip() {
        let hash = hash_password("geheim").unwrap();
        assert!(hash.starts_with("$2"));
        assert!(verify_password("geheim", &hash));
        assert!(!verify_password("falsch", &hash));
        assert!(!verify_password("geheim", "not-a-hash"));
    }

    #[test]
    fn cookie_format() {
        let c = session_cookie("abc", false);
        assert!(c.starts_with("tonies_session=abc;"));
        assert!(c.contains("HttpOnly"));
        assert!(c.contains("Path=/"));
        assert!(c.contains("Max-Age=2592000"));
        assert!(c.contains("SameSite=Lax"));
        assert!(!c.contains("Secure"));

        assert!(session_cookie("abc", true).contains("Secure"));
        assert!(clear_cookie().contains("Max-Age=0"));
    }
}
