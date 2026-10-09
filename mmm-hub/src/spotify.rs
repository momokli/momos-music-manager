//! Minimal Spotify Web API client for the walking skeleton.
//!
//! Deliberately uses raw `reqwest` rather than `rspotify`: we need the
//! post-February-2026 endpoints (`/playlists/{id}/items`) and a loopback
//! token grab, and don't want a client library that still targets `/tracks`.

use anyhow::{Context, Result, bail};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use serde_json::Value;
use sha2::{Digest, Sha256};

pub const SCOPES: &str =
    "user-read-private user-library-read playlist-read-private playlist-read-collaborative";

const AUTHORIZE_URL: &str = "https://accounts.spotify.com/authorize";
const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
/// Default Spotify Web API base; overridable per-config (`SPOTIFY_API_BASE`)
/// so integration tests can point the client at a local mock server.
pub const DEFAULT_API_BASE: &str = "https://api.spotify.com/v1";

#[derive(Debug, Clone)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix epoch seconds at which `access_token` expires.
    pub expires_at: i64,
}

fn now_epoch() -> i64 {
    chrono::Utc::now().timestamp()
}

fn random_b64(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    URL_SAFE_NO_PAD.encode(buf)
}

pub fn random_state() -> String {
    random_b64(16)
}

/// A 256-bit opaque token (session ids, etc.).
pub fn random_token() -> String {
    random_b64(32)
}

/// Returns `(verifier, challenge)` for PKCE S256.
pub fn pkce_pair() -> (String, String) {
    let verifier = random_b64(64);
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

pub fn authorize_url(client_id: &str, redirect_uri: &str, state: &str, challenge: &str) -> String {
    format!(
        "{AUTHORIZE_URL}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}&code_challenge={}&code_challenge_method=S256",
        urlencoding::encode(client_id),
        urlencoding::encode(redirect_uri),
        urlencoding::encode(SCOPES),
        urlencoding::encode(state),
        urlencoding::encode(challenge),
    )
}

async fn token_request(form: &[(&str, &str)]) -> Result<Tokens> {
    let client = reqwest::Client::new();
    let resp = client
        .post(TOKEN_URL)
        .form(form)
        .send()
        .await
        .context("POST accounts.spotify.com/api/token")?;

    let status = resp.status();
    let body: Value = resp.json().await.context("decode token response")?;
    if !status.is_success() {
        bail!("Spotify token endpoint returned {status}: {body}");
    }

    let access_token = body["access_token"]
        .as_str()
        .context("token response missing access_token")?
        .to_string();
    let expires_in = body["expires_in"].as_i64().unwrap_or(3600);
    Ok(Tokens {
        access_token,
        refresh_token: body["refresh_token"].as_str().map(str::to_string),
        expires_at: now_epoch() + expires_in,
    })
}

pub async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    redirect_uri: &str,
    code: &str,
    verifier: &str,
) -> Result<Tokens> {
    token_request(&[
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code_verifier", verifier),
    ])
    .await
}

pub async fn refresh(client_id: &str, client_secret: &str, refresh_token: &str) -> Result<Tokens> {
    token_request(&[
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("client_id", client_id),
        ("client_secret", client_secret),
    ])
    .await
}

/// GET a Spotify API URL. Returns the HTTP status and parsed JSON body.
pub async fn get_json(access_token: &str, url: &str) -> Result<(u16, Value)> {
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .bearer_auth(access_token)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    Ok((status, body))
}

pub async fn api_get(base: &str, access_token: &str, path: &str) -> Result<(u16, Value)> {
    get_json(access_token, &api_url(base, path)).await
}

pub fn api_url(base: &str, path: &str) -> String {
    format!("{base}{path}")
}

/// One response page with the bits the rate-limit handler needs.
pub struct Page {
    pub status: u16,
    pub retry_after: Option<u64>,
    pub quota_exceeded: bool,
    pub body: Value,
}

pub async fn get_page(access_token: &str, url: &str) -> Result<Page> {
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .bearer_auth(access_token)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let status = resp.status().as_u16();
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<u64>().ok());
    let body: Value = resp.json().await.unwrap_or(Value::Null);
    let quota_exceeded = body["error"]["reason"].as_str() == Some("QUOTA_EXCEEDED");
    Ok(Page {
        status,
        retry_after,
        quota_exceeded,
        body,
    })
}

/// Follow Spotify's `next` links and collect all `items`.
pub async fn get_all_items(base: &str, access_token: &str, first_path: &str) -> Result<Vec<Value>> {
    let mut url = api_url(base, first_path);
    let mut out = Vec::new();
    loop {
        let (status, body) = get_json(access_token, &url).await?;
        if status != 200 {
            bail!("Spotify returned {status} for {url}: {body}");
        }
        if let Some(items) = body["items"].as_array() {
            out.extend(items.iter().cloned());
        }
        match body["next"].as_str() {
            Some(next) if !next.is_empty() => url = next.to_string(),
            _ => break,
        }
    }
    Ok(out)
}
