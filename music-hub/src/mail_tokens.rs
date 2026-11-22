//! Single-use e-mail tokens for password resets and invitations.
//!
//! Backed by `hub_email_tokens` (migration `068`). Only the SHA-256 of the token
//! is stored: a database leak yields no usable link. Redemption is a single
//! `UPDATE … RETURNING` so two concurrent requests cannot both win — the second
//! one sees `used_at IS NULL` already false.
//!
//! Tokens are bound to an address, and for `reset` also to a user id. A reset
//! token therefore cannot be redirected to another account by editing a form.

use chrono::{Duration, Utc};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

/// Password reset: short-lived, because the request is urgent.
pub const RESET_TTL_MINUTES: i64 = 30;
/// Invitation: may sit in an inbox over a weekend.
pub const REGISTER_TTL_DAYS: i64 = 7;

pub const PURPOSE_RESET: &str = "reset";
pub const PURPOSE_REGISTER: &str = "register";

/// A successfully redeemed token.
#[derive(Debug, Clone)]
pub struct Redeemed {
    /// `None` for an invitation to an address without an account yet.
    pub user_id: Option<i64>,
    pub email: String,
}

/// SHA-256 of the plaintext token, lower-case hex.
pub fn hash_token(plaintext: &str) -> String {
    let digest = Sha256::digest(plaintext.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// Mint a token and store only its hash. Returns the plaintext (the only moment
/// it exists outside the recipient's inbox).
pub async fn issue(
    pool: &SqlitePool,
    user_id: Option<i64>,
    email: &str,
    purpose: &str,
    ttl: Duration,
    created_by: Option<i64>,
) -> anyhow::Result<String> {
    let plaintext = crate::spotify::random_token();
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO hub_email_tokens
             (user_id, email, purpose, token_hash, expires_at, created_by, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(user_id)
    .bind(email)
    .bind(purpose)
    .bind(hash_token(&plaintext))
    .bind((now + ttl).to_rfc3339())
    .bind(created_by)
    .bind(now.to_rfc3339())
    .execute(pool)
    .await?;
    Ok(plaintext)
}

/// Check a token without consuming it (used to render the form).
pub async fn is_usable(
    pool: &SqlitePool,
    plaintext: &str,
    purpose: &str,
) -> anyhow::Result<bool> {
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM hub_email_tokens
          WHERE token_hash = ?1 AND purpose = ?2 AND used_at IS NULL AND expires_at > ?3",
    )
    .bind(hash_token(plaintext))
    .bind(purpose)
    .bind(Utc::now().to_rfc3339())
    .fetch_optional(pool)
    .await?;
    Ok(found.is_some())
}

/// Consume a token exactly once. `None` means unknown, expired, already used or
/// minted for a different purpose — all indistinguishable by design.
pub async fn redeem(
    pool: &SqlitePool,
    plaintext: &str,
    purpose: &str,
) -> anyhow::Result<Option<Redeemed>> {
    let now = Utc::now().to_rfc3339();
    let row = sqlx::query_as::<_, (Option<i64>, String)>(
        "UPDATE hub_email_tokens
            SET used_at = ?3
          WHERE token_hash = ?1
            AND purpose = ?2
            AND used_at IS NULL
            AND expires_at > ?3
          RETURNING user_id, email",
    )
    .bind(hash_token(plaintext))
    .bind(purpose)
    .bind(now)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(user_id, email)| Redeemed { user_id, email }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_stable_hex_of_fixed_length() {
        let a = hash_token("abc");
        assert_eq!(a, hash_token("abc"));
        assert_ne!(a, hash_token("abd"));
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn plaintext_is_not_recoverable_from_the_hash() {
        let token = crate::spotify::random_token();
        assert!(!hash_token(&token).contains(&token));
    }
}
