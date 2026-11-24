//! Password hashing and web-session invalidation.
//!
//! Shared by the HTTP layer (`web.rs`, `account.rs`) and the CLI
//! (`main.rs: set-password`), so the work factor and the minimum length can
//! never drift apart between the three call sites.
//!
//! Storage format is bcrypt (`$2b$<cost>$<salt><hash>`). bcrypt draws a fresh
//! 16-byte salt on every call and embeds it in the string, so there is no
//! separate salt column — the DB only ever holds `bcrypt(password‖salt)`.
//!
//! Hashing happens in exactly three places, all of which move the *plaintext*
//! in memory only: signup, password change, and an admin reset via the CLI.

/// Work factor for newly written hashes (bcrypt cost).
///
/// 12 costs roughly 200–300 ms on the hub host — imperceptible for a login,
/// expensive for an offline attack. Hashes below this are upgraded on the next
/// successful login ([`rehash_if_stale`]).
pub const BCRYPT_COST: u32 = 12;

/// Minimum length for a *new* password (signup + change + CLI reset).
///
/// Existing hashes are never re-validated: a shorter password set before this
/// rule existed keeps working until it is next changed.
pub const MIN_PASSWORD_LEN: usize = 8;

/// Hash a password with [`BCRYPT_COST`].
pub fn hash_password(password: &str) -> anyhow::Result<String> {
    Ok(bcrypt::hash(password, BCRYPT_COST)?)
}

/// Hash a password with an explicit cost. Tests use a low cost to stay fast;
/// production code must go through [`hash_password`].
pub fn hash_password_with_cost(password: &str, cost: u32) -> anyhow::Result<String> {
    Ok(bcrypt::hash(password, cost)?)
}

/// Verify a password against a stored hash.
///
/// A missing hash (user without a password) never authenticates, and a
/// malformed hash is a failed login rather than an error — the caller only
/// needs a boolean.
pub fn verify_password(password: &str, hash: Option<&str>) -> bool {
    match hash {
        Some(h) => bcrypt::verify(password, h).unwrap_or(false),
        None => false,
    }
}

/// Parse the cost out of a bcrypt string (`$2b$12$…` → `12`).
///
/// Accepts every bcrypt revision (`$2a$`, `$2b$`, `$2y$`), since imported
/// hashes may not have been written by this crate.
pub fn hash_cost(hash: &str) -> Option<u32> {
    let mut parts = hash.split('$');
    parts.next()?; // empty segment before the leading `$`
    parts.next()?; // algorithm revision
    parts.next()?.parse::<u32>().ok()
}

/// Delete every web session of `user_id`, optionally keeping one.
///
/// `keep` is the current session token: a password *change* must not log the
/// user out of the browser they are using, while a *reset* (CLI or magic link)
/// passes `None` and ends everything. Returns the number of rows removed.
pub async fn invalidate_sessions(
    pool: &sqlx::SqlitePool,
    user_id: i64,
    keep: Option<&str>,
) -> sqlx::Result<u64> {
    let res = sqlx::query(
        "DELETE FROM hub_web_sessions
          WHERE user_id = ?1
            AND (?2 IS NULL OR id <> ?2)",
    )
    .bind(user_id)
    .bind(keep)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// Re-hash `password` when its stored hash is weaker than [`BCRYPT_COST`].
///
/// Called right after a successful login — the only moment the plaintext is
/// available. Returns `true` when the stored hash was replaced.
pub async fn rehash_if_stale(
    pool: &sqlx::SqlitePool,
    user_id: i64,
    password: &str,
    hash: &str,
) -> anyhow::Result<bool> {
    if hash_cost(hash).is_some_and(|c| c >= BCRYPT_COST) {
        return Ok(false);
    }
    let fresh = hash_password(password)?;
    sqlx::query("UPDATE hub_users SET password_hash = ?2 WHERE id = ?1")
        .bind(user_id)
        .bind(&fresh)
        .execute(pool)
        .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_salted_and_verify() {
        let a = hash_password_with_cost("correct horse", 4).unwrap();
        let b = hash_password_with_cost("correct horse", 4).unwrap();
        assert_ne!(a, b, "bcrypt must salt every hash separately");
        assert!(verify_password("correct horse", Some(&a)));
        assert!(!verify_password("wrong", Some(&a)));
    }

    #[test]
    fn missing_hash_never_verifies() {
        assert!(!verify_password("anything", None));
        assert!(!verify_password("anything", Some("not-a-hash")));
    }

    #[test]
    fn cost_is_parsed_from_every_revision() {
        assert_eq!(hash_cost("$2b$12$abcdefghijklmnopqrstuv"), Some(12));
        assert_eq!(hash_cost("$2a$10$abcdefghijklmnopqrstuv"), Some(10));
        assert_eq!(hash_cost("$2y$04$abcdefghijklmnopqrstuv"), Some(4));
        assert_eq!(hash_cost("plain"), None);
        assert_eq!(hash_cost(""), None);
    }

    #[test]
    fn stale_threshold_is_the_shared_constant() {
        // Sanity check on the rule `rehash_if_stale` implements, without a DB.
        assert!(hash_cost("$2b$10$x").unwrap() < BCRYPT_COST);
        assert!(hash_cost(&format!("$2b${BCRYPT_COST}$x")).unwrap() >= BCRYPT_COST);
    }
}
