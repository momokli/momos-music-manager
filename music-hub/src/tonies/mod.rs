//! Tonies integration (#499): AES-256-GCM crypto helpers, tolerant yet typed
//! API models, and the per-user encrypted account store.
//!
//! M1 scope: connection + read-only view. The HTTP client lives in
//! [`client`]. No secret ever touches `tracing`, an `anyhow` message, or a
//! template — only typed, status-level errors.

pub mod autosync;
pub mod bindings;
pub mod client;
pub mod run;
pub mod sync;

pub use autosync::{Debounce, due_bindings, is_due, run_due};
pub use bindings::{
    Binding, SOURCE_BACKPACK, SOURCE_PLAYLIST, SOURCE_TAG, delete_binding, get_binding,
    list_bindings, source_kind_label, upsert_binding, valid_format, valid_mode, valid_source_kind,
};
pub use run::{
    Claim, RunOutcome, SyncOptions, SyncRow, Transcoding, claim_run, execute_binding, finish_run,
    list_syncs, poll_transcoding, readiness, run_sync, summary,
};
pub use sync::{
    DesiredTrack, DiffKind, DiffPlan, FitPlan, SkipReason, SkippedTrack, dedupe_by_isrc,
    desired_for_source, desired_from_backpack, desired_from_playlist, desired_from_tag, greedy_fit,
    ms_to_seconds, plan_diff,
};

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use ring::aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

/// Total capacity of a creative Tonie, in seconds (90 min).
pub const TOTAL_SECONDS: i64 = 5400;

// ── crypto ──────────────────────────────────────────────────────────────────

/// Derive a 256-bit AEAD key from the configured secret string. The raw key is
/// never stored; a SHA-256 digest gives a uniform 32 bytes for any input.
fn derive_key(secret: &str) -> [u8; 32] {
    let digest = Sha256::digest(secret.as_bytes());
    let mut key = [0u8; 32];
    key.copy_from_slice(&digest);
    key
}

/// Encrypt `plaintext` with AES-256-GCM. Returns
/// `base64(nonce(12 B) || ciphertext || tag)`.
pub fn encrypt_secret(secret: &str, plaintext: &str) -> Result<String> {
    let key = LessSafeKey::new(
        UnboundKey::new(&AES_256_GCM, &derive_key(secret))
            .map_err(|_| anyhow::anyhow!("ungueltiger Schluessel"))?,
    );
    let mut nonce_bytes = [0u8; NONCE_LEN];
    SystemRandom::new()
        .fill(&mut nonce_bytes)
        .map_err(|_| anyhow::anyhow!("Nonce konnte nicht erzeugt werden"))?;
    let nonce = Nonce::assume_unique_for_key(nonce_bytes);

    let mut in_out = plaintext.as_bytes().to_vec();
    key.seal_in_place_append_tag(nonce, Aad::empty(), &mut in_out)
        .map_err(|_| anyhow::anyhow!("Verschluesselung fehlgeschlagen"))?;

    let mut blob = nonce_bytes.to_vec();
    blob.extend_from_slice(&in_out);
    Ok(B64.encode(blob))
}

/// Decrypt a blob produced by [`encrypt_secret`]. A wrong key or tampered bytes
/// yield `Err` — never a panic and never the plaintext in the message.
pub fn decrypt_secret(secret: &str, blob: &str) -> Result<String> {
    let raw = B64
        .decode(blob.trim())
        .context("gespeichertes Geheimnis ist kein gueltiges Base64")?;
    if raw.len() < NONCE_LEN + 16 {
        bail!("gespeichertes Geheimnis ist zu kurz"); // 16 = GCM tag
    }
    let (nonce_bytes, ciphertext) = raw.split_at(NONCE_LEN);
    let mut nonce_arr = [0u8; NONCE_LEN];
    nonce_arr.copy_from_slice(nonce_bytes);
    let nonce = Nonce::assume_unique_for_key(nonce_arr);

    let key = LessSafeKey::new(
        UnboundKey::new(&AES_256_GCM, &derive_key(secret))
            .map_err(|_| anyhow::anyhow!("ungueltiger Schluessel"))?,
    );
    let mut in_out = ciphertext.to_vec();
    let plain = key
        .open_in_place(nonce, Aad::empty(), &mut in_out)
        .map_err(|_| anyhow::anyhow!("Entschluesselung fehlgeschlagen (falscher Schluessel?)"))?;
    String::from_utf8(plain.to_vec()).context("Geheimnis ist kein gueltiges UTF-8")
}

// ── models (tolerant: every field optional) ─────────────────────────────────

/// OAuth/Keycloak token response.
#[derive(Debug, Clone, Default)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix epoch seconds at which `access_token` expires.
    pub expires_at: i64,
}

/// The signed-in Tonies user (`GET /v2/me`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Me {
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub household_id: Option<String>,
}

/// A Tonies household (`GET /v2/households`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Household {
    #[serde(default, alias = "householdId")]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

/// A creative Tonie (`GET …/creativetonies[/{ctid}]`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreativeTonie {
    #[serde(default, alias = "ctid")]
    pub id: Option<String>,
    #[serde(default, alias = "name")]
    pub title: Option<String>,
    /// Seconds already occupied on the Tonie.
    #[serde(default)]
    pub seconds_present: Option<i64>,
    /// Seconds still free (capacity minus present).
    #[serde(default)]
    pub seconds_remaining: Option<i64>,
    #[serde(default)]
    pub chapters: Vec<Chapter>,
}

/// One chapter of a creative Tonie. Serialisable so it can be reused verbatim
/// in a `PATCH …/creativetonies/{ctid}` body.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chapter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// Chapter length in seconds, when known. Additive and tolerant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seconds: Option<i64>,
    /// `true` while the Tonies backend is still transcoding this chapter.
    /// Additive and tolerant; `None` means the API did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcoding: Option<bool>,
    /// Backend chapter kind (the API's `type` field), when present.
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub chapter_type: Option<String>,
}

/// Capacity snapshot for a creative Tonie, in seconds.
#[derive(Debug, Clone, Copy, Default)]
pub struct Capacity {
    pub present: i64,
    pub remaining: i64,
}

impl Capacity {
    /// Derive a capacity from the API's `secondsPresent`/`secondsRemaining`,
    /// falling back to the 90-minute total when the API omits a field.
    pub fn from_tonie(t: &CreativeTonie) -> Capacity {
        let present = t.seconds_present.unwrap_or(0).max(0);
        let remaining = t
            .seconds_remaining
            .unwrap_or_else(|| (TOTAL_SECONDS - present).max(0))
            .max(0);
        Capacity { present, remaining }
    }

    pub fn total(&self) -> i64 {
        self.present + self.remaining
    }

    pub fn minutes_present(&self) -> i64 {
        self.present / 60
    }

    pub fn minutes_total(&self) -> i64 {
        self.total() / 60
    }

    /// 0–100 fill for a capacity bar.
    pub fn percent(&self) -> i64 {
        let total = self.total();
        if total <= 0 {
            0
        } else {
            (self.present * 100 / total).clamp(0, 100)
        }
    }
}

// ── account store ───────────────────────────────────────────────────────────

/// A decrypted view of one `hub_tonies_accounts` row (no secret inside).
#[derive(Debug, Clone, Default)]
pub struct Account {
    pub user_id: i64,
    pub household_id: Option<String>,
    pub email: Option<String>,
    pub last_login_at: Option<String>,
    pub last_error: Option<String>,
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ── client construction (pool/cfg-based, for web + worker) ──────────────────

/// Why building a Tonies client for a user failed (credential-free).
#[derive(Debug, Clone)]
pub struct ClientError {
    /// Categorised, credential-free message.
    pub message: String,
    /// The stored token could not be read → offer "reconnect".
    pub needs_reconnect: bool,
}

/// Build a Tonies client for `uid` from the stored, still-encrypted refresh
/// token. The token is decrypted in memory only — it is never logged, rendered,
/// or put into an error message. Pool/cfg-based so both the web handler and the
/// worker auto-sync can share it.
pub async fn client_for(
    pool: &SqlitePool,
    cfg: &crate::config::Config,
    uid: i64,
) -> Result<client::ToniesClient, ClientError> {
    let key = cfg.tonies_key().map_err(|e| ClientError {
        message: format!("{e}"),
        needs_reconnect: false,
    })?;
    let secret_enc = load_secret_enc(pool, uid)
        .await
        .map_err(|e| {
            tracing::warn!("tonies secret load failed for user {uid}: {e:#}");
            ClientError {
                message: "Tonies-Verbindung konnte nicht geladen werden".to_string(),
                needs_reconnect: false,
            }
        })?
        .unwrap_or_default();
    let refresh_token = decrypt_secret(key, &secret_enc).map_err(|_| ClientError {
        message: "Tonies-Sitzung konnte nicht gelesen werden — bitte neu verbinden.".to_string(),
        needs_reconnect: true,
    })?;
    let tokens = Tokens {
        access_token: String::new(),
        refresh_token: Some(refresh_token),
        expires_at: 0,
    };
    Ok(client::ToniesClient::with_tokens(
        &cfg.tonies_auth_base,
        &cfg.tonies_api_base,
        tokens,
    ))
}

/// Resolve the household id: prefer the configured/first household, else the id
/// carried by `/v2/me`.
pub async fn resolve_household(client: &mut client::ToniesClient) -> Option<String> {
    if let Ok(hs) = client.get_households().await {
        if let Some(id) = hs.into_iter().find_map(|h| h.id) {
            return Some(id);
        }
    }
    client.get_me().await.ok().and_then(|m| m.household_id)
}

/// Upsert the account, encrypting `refresh_token` at rest and clearing any
/// previous `last_error`. The plaintext token never reaches the DB.
pub async fn save_account(
    pool: &SqlitePool,
    user_id: i64,
    household_id: Option<&str>,
    email: Option<&str>,
    refresh_token: &str,
    secret_key: &str,
) -> Result<()> {
    let secret_enc = encrypt_secret(secret_key, refresh_token)?;
    let ts = now_iso();
    sqlx::query(
        "INSERT INTO hub_tonies_accounts
             (user_id, household_id, tonie_email, secret_enc, last_login_at, last_error, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?5, ?5)
         ON CONFLICT(user_id) DO UPDATE SET
             household_id  = excluded.household_id,
             tonie_email   = excluded.tonie_email,
             secret_enc    = excluded.secret_enc,
             last_login_at = excluded.last_login_at,
             last_error    = NULL,
             updated_at    = excluded.updated_at",
    )
    .bind(user_id)
    .bind(household_id)
    .bind(email)
    .bind(&secret_enc)
    .bind(&ts)
    .execute(pool)
    .await
    .context("Tonies-Account konnte nicht gespeichert werden")?;
    Ok(())
}

/// Load a user's account row, if connected. Returns `None` when absent.
pub async fn load_account(pool: &SqlitePool, user_id: i64) -> Result<Option<Account>> {
    let row = sqlx::query(
        "SELECT household_id, tonie_email, last_login_at, last_error
           FROM hub_tonies_accounts WHERE user_id = ?1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .context("Tonies-Account konnte nicht gelesen werden")?;
    Ok(row.map(|r| Account {
        user_id,
        household_id: r.get("household_id"),
        email: r.get("tonie_email"),
        last_login_at: r.get("last_login_at"),
        last_error: r.get("last_error"),
    }))
}

/// The stored, still-encrypted secret for a user (for the page handler to
/// decrypt). `None` when not connected.
pub async fn load_secret_enc(pool: &SqlitePool, user_id: i64) -> Result<Option<String>> {
    let row = sqlx::query_scalar::<_, String>(
        "SELECT secret_enc FROM hub_tonies_accounts WHERE user_id = ?1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .context("Tonies-Geheimnis konnte nicht gelesen werden")?;
    Ok(row)
}

/// Drop the connection entirely.
pub async fn clear_account(pool: &SqlitePool, user_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM hub_tonies_accounts WHERE user_id = ?1")
        .bind(user_id)
        .execute(pool)
        .await
        .context("Tonies-Account konnte nicht entfernt werden")?;
    Ok(())
}

/// Record a categorised error message for the user (never a credential).
pub async fn mark_error(pool: &SqlitePool, user_id: i64, message: &str) -> Result<()> {
    sqlx::query(
        "UPDATE hub_tonies_accounts SET last_error = ?2, updated_at = ?3 WHERE user_id = ?1",
    )
    .bind(user_id)
    .bind(message)
    .bind(now_iso())
    .execute(pool)
    .await
    .context("Tonies-Fehler konnte nicht gespeichert werden")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crypto_roundtrip() {
        let secret = "host-secret";
        let plain = "refresh-token-abc.def-ghi";
        let blob = encrypt_secret(secret, plain).unwrap();
        assert_ne!(blob, plain, "ciphertext must differ from plaintext");
        assert!(!blob.contains(plain));
        assert_eq!(decrypt_secret(secret, &blob).unwrap(), plain);
    }

    #[test]
    fn wrong_key_is_err_without_plaintext() {
        let plain = "top-secret-refresh";
        let blob = encrypt_secret("key-a", plain).unwrap();
        let err = decrypt_secret("key-b", &blob).unwrap_err();
        let msg = format!("{err:#}");
        assert!(!msg.contains(plain), "error must not leak the plaintext");
        assert!(!msg.contains(&blob), "error must not echo the blob");
    }

    #[test]
    fn tampered_blob_is_err() {
        let plain = "refresh-token";
        let blob = encrypt_secret("k", plain).unwrap();
        let mut raw = B64.decode(&blob).unwrap();
        let last = raw.len() - 1;
        raw[last] ^= 0x01; // flip a ciphertext/tag bit
        let tampered = B64.encode(&raw);
        assert!(decrypt_secret("k", &tampered).is_err());
    }

    #[test]
    fn capacity_minutes_from_seconds() {
        let t = CreativeTonie {
            seconds_present: Some(3780),
            seconds_remaining: Some(1620),
            ..Default::default()
        };
        let c = Capacity::from_tonie(&t);
        assert_eq!(c.minutes_present(), 63);
        assert_eq!(c.minutes_total(), 90);
        assert_eq!(c.percent(), 70);
    }
}
