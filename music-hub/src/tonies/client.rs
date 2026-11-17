//! Tonies HTTP client: Keycloak password-grant login, refresh-on-401, the M1
//! read endpoints, and the upload primitives (manual multipart — the
//! `reqwest` `multipart` feature is deliberately **not** enabled, it would pull
//! `mime_guess`).
//!
//! Errors are typed and status-level only: a `401` that cannot be refreshed
//! becomes [`ToniesError::AuthExpired`] (drives the "reconnect" UI); everything
//! else is [`ToniesError::Other`]. No token/password ever appears in a message.

use anyhow::Context;
use chrono::Utc;
use serde_json::Value;

use super::{Chapter, CreativeTonie, Household, Me, Tokens};

/// Default Keycloak auth base; overridable per-config (`TONIES_AUTH_BASE`).
pub const DEFAULT_AUTH_BASE: &str = "https://login.tonies.com/auth";
/// Default REST API base; overridable per-config (`TONIES_API_BASE`).
pub const DEFAULT_API_BASE: &str = "https://api.tonies.com";
/// Keycloak realm used by the Tonies apps.
pub const REALM: &str = "tonies";
/// Public token-endpoint path (within the realm).
pub const TOKEN_PATH: &str = "/protocol/openid-connect/token";
/// Fallback client id for the password grant.
///
/// **Placeholder.** The live login answers `invalid_client` for it (verified
/// 2026-11-17): the real id has to be read off the official app (DevTools →
/// `POST …/protocol/openid-connect/token` → `client_id`) and set via
/// `TONIES_CLIENT_ID` or the `tonies_client_id` setting.
pub const DEFAULT_CLIENT_ID: &str = "tonies-webapp";

/// Typed client error. `AuthExpired` means "the user must reconnect".
#[derive(Debug)]
pub enum ToniesError {
    /// The access token could not be refreshed (refresh token revoked/expired).
    AuthExpired,
    /// Anything else — carries only status/endpoint, never a credential.
    Other(anyhow::Error),
}

impl std::fmt::Display for ToniesError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToniesError::AuthExpired => write!(f, "Tonies-Sitzung abgelaufen"),
            ToniesError::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ToniesError {}

impl From<anyhow::Error> for ToniesError {
    fn from(e: anyhow::Error) -> Self {
        ToniesError::Other(e)
    }
}

pub type TResult<T> = Result<T, ToniesError>;

fn token_url(auth_base: &str) -> String {
    format!(
        "{}/realms/{REALM}{TOKEN_PATH}",
        auth_base.trim_end_matches('/')
    )
}

fn now_epoch() -> i64 {
    Utc::now().timestamp()
}

/// Parse a Keycloak token response into [`Tokens`]. The body is never echoed
/// in an error (it can contain tokens).
async fn parse_tokens(resp: reqwest::Response, what: &str) -> TResult<Tokens> {
    let status = resp.status();
    let body: Value = resp
        .json()
        .await
        .map_err(|_| ToniesError::Other(anyhow::anyhow!("{what}: Antwort nicht lesbar")))?;
    if !status.is_success() {
        return Err(ToniesError::Other(anyhow::anyhow!(
            "{what}: Token-Endpunkt antwortete {status}"
        )));
    }
    let access_token = body["access_token"]
        .as_str()
        .ok_or_else(|| ToniesError::Other(anyhow::anyhow!("{what}: access_token fehlt")))?
        .to_string();
    let refresh_token = body["refresh_token"].as_str().map(str::to_string);
    let expires_in = body["expires_in"].as_i64().unwrap_or(300);
    Ok(Tokens {
        access_token,
        refresh_token,
        expires_at: now_epoch() + expires_in,
    })
}

/// Keycloak password grant. Never logs the password.
pub async fn login(
    auth_base: &str,
    client_id: &str,
    email: &str,
    password: &str,
) -> TResult<Tokens> {
    let client = reqwest::Client::new();
    let resp = client
        .post(token_url(auth_base))
        .form(&[
            ("grant_type", "password"),
            ("client_id", client_id),
            ("username", email),
            ("password", password),
        ])
        .send()
        .await
        .map_err(|_| ToniesError::Other(anyhow::anyhow!("Login: Verbindung fehlgeschlagen")))?;
    parse_tokens(resp, "Login").await
}

/// Exchange a refresh token for a fresh access token.
pub async fn refresh(auth_base: &str, client_id: &str, refresh_token: &str) -> TResult<Tokens> {
    let client = reqwest::Client::new();
    let resp = client
        .post(token_url(auth_base))
        .form(&[
            ("grant_type", "refresh_token"),
            ("client_id", client_id),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .map_err(|_| ToniesError::Other(anyhow::anyhow!("Refresh: Verbindung fehlgeschlagen")))?;
    parse_tokens(resp, "Refresh").await
}

/// Presigned-S3 upload ticket from `POST /v2/file`.
#[derive(Debug, Clone, Default)]
pub struct UploadTicket {
    pub file_id: String,
    pub url: String,
    /// Ordered form fields for the multipart PUT (sent before the file part).
    pub fields: Vec<(String, String)>,
}

/// Stateful client bound to the configured bases and the current tokens.
#[derive(Debug, Clone)]
pub struct ToniesClient {
    http: reqwest::Client,
    auth_base: String,
    api_base: String,
    /// Keycloak client for the token exchange; `DEFAULT_CLIENT_ID` unless the
    /// configured one was passed in.
    client_id: String,
    access_token: Option<String>,
    expires_at: i64,
    refresh_token: Option<String>,
}

impl ToniesClient {
    pub fn new(auth_base: &str, api_base: &str) -> Self {
        Self::with_client_id(auth_base, api_base, DEFAULT_CLIENT_ID)
    }

    /// Like [`Self::new`], but with an explicit Keycloak client id.
    pub fn with_client_id(auth_base: &str, api_base: &str, client_id: &str) -> Self {
        ToniesClient {
            http: reqwest::Client::new(),
            auth_base: auth_base.trim_end_matches('/').to_string(),
            api_base: api_base.trim_end_matches('/').to_string(),
            client_id: client_id.trim().to_string(),
            access_token: None,
            expires_at: 0,
            refresh_token: None,
        }
    }

    pub fn with_tokens(auth_base: &str, api_base: &str, tokens: Tokens) -> Self {
        let mut c = Self::new(auth_base, api_base);
        c.set_tokens(tokens);
        c
    }

    /// [`Self::with_tokens`] with an explicit Keycloak client id.
    pub fn with_tokens_and_client(
        auth_base: &str,
        api_base: &str,
        tokens: Tokens,
        client_id: &str,
    ) -> Self {
        let mut c = Self::with_client_id(auth_base, api_base, client_id);
        c.set_tokens(tokens);
        c
    }

    pub fn set_tokens(&mut self, tokens: Tokens) {
        self.access_token = Some(tokens.access_token);
        if tokens.refresh_token.is_some() {
            self.refresh_token = tokens.refresh_token;
        }
        self.expires_at = tokens.expires_at;
    }

    pub fn access_token(&self) -> Option<&str> {
        self.access_token.as_deref()
    }

    pub fn refresh_token(&self) -> Option<&str> {
        self.refresh_token.as_deref()
    }

    pub fn expires_at(&self) -> i64 {
        self.expires_at
    }

    /// Refresh once using the stored refresh token; map any failure to
    /// `AuthExpired` so the UI can offer "reconnect".
    async fn refresh_now(&mut self) -> TResult<()> {
        let rt = self
            .refresh_token
            .clone()
            .ok_or(ToniesError::AuthExpired)?;
        match refresh(&self.auth_base, &self.client_id, &rt).await {
            Ok(tokens) => {
                self.set_tokens(tokens);
                Ok(())
            }
            Err(_) => Err(ToniesError::AuthExpired),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.api_base, path)
    }

    /// Authenticated request with a single refresh-on-401 retry.
    async fn authed(
        &mut self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> TResult<Value> {
        let url = self.url(path);
        let resp = self.send(method.clone(), &url, body.clone()).await?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            self.refresh_now().await?;
            let resp = self.send(method, &url, body).await?;
            return Self::json(resp, path).await;
        }
        Self::json(resp, path).await
    }

    async fn send(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<Value>,
    ) -> TResult<reqwest::Response> {
        let mut req = self.http.request(method, url);
        if let Some(tok) = self.access_token.as_deref() {
            req = req.bearer_auth(tok);
        }
        if let Some(b) = body {
            req = req.json(&b);
        }
        req.send()
            .await
            .map_err(|_| ToniesError::Other(anyhow::anyhow!("Tonies-API nicht erreichbar")))
    }

    /// Decode a response to JSON. Only the status + path appear in an error.
    async fn json(resp: reqwest::Response, path: &str) -> TResult<Value> {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(ToniesError::Other(anyhow::anyhow!(
                "Tonies-API {path} antwortete {status}"
            )));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text)
            .with_context(|| format!("Tonies-API {path}: Antwort nicht lesbar"))
            .map_err(ToniesError::Other)
    }

    // ── read endpoints ──────────────────────────────────────────────────────

    pub async fn get_me(&mut self) -> TResult<Me> {
        let v = self.authed(reqwest::Method::GET, "/v2/me", None).await?;
        Ok(serde_json::from_value(v).unwrap_or_default())
    }

    pub async fn get_households(&mut self) -> TResult<Vec<Household>> {
        let v = self
            .authed(reqwest::Method::GET, "/v2/households", None)
            .await?;
        Ok(parse_list(&v, "households"))
    }

    pub async fn get_creativetonies(&mut self, household: &str) -> TResult<Vec<CreativeTonie>> {
        let path = format!("/v2/households/{household}/creativetonies");
        let v = self.authed(reqwest::Method::GET, &path, None).await?;
        Ok(parse_list(&v, "creativetonies"))
    }

    pub async fn get_creativetonie(
        &mut self,
        household: &str,
        ctid: &str,
    ) -> TResult<CreativeTonie> {
        let path = format!("/v2/households/{household}/creativetonies/{ctid}");
        let v = self.authed(reqwest::Method::GET, &path, None).await?;
        Ok(serde_json::from_value(v).unwrap_or_default())
    }

    // ── upload primitives (M2-prepared, client-side + tested in M1) ─────────

    /// `POST /v2/file` → presigned S3 ticket.
    pub async fn request_upload(&mut self) -> TResult<UploadTicket> {
        let v = self
            .authed(reqwest::Method::POST, "/v2/file", Some(serde_json::json!({})))
            .await?;
        Ok(parse_ticket(&v))
    }

    /// Multipart `PUT` to the presigned S3 URL. Fields come first, the file
    /// part last — built manually (no reqwest multipart feature).
    pub async fn put_multipart(
        &self,
        ticket: &UploadTicket,
        filename: &str,
        content_type: &str,
        bytes: &[u8],
    ) -> TResult<()> {
        let boundary = format!("----hubtonies{}", Utc::now().timestamp_nanos_opt().unwrap_or(0));
        let body = build_multipart(&boundary, &ticket.fields, filename, content_type, bytes);
        let resp = self
            .http
            .put(&ticket.url)
            .header(
                reqwest::header::CONTENT_TYPE,
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(reqwest::Body::from(body))
            .send()
            .await
            .map_err(|_| ToniesError::Other(anyhow::anyhow!("S3-Upload nicht erreichbar")))?;
        if !resp.status().is_success() {
            return Err(ToniesError::Other(anyhow::anyhow!(
                "S3-Upload antwortete {}",
                resp.status()
            )));
        }
        Ok(())
    }

    /// `POST …/creativetonies/{ctid}/chapters`.
    pub async fn add_chapter(
        &mut self,
        household: &str,
        ctid: &str,
        title: &str,
        file: &str,
    ) -> TResult<Value> {
        let path = format!("/v2/households/{household}/creativetonies/{ctid}/chapters");
        let body = serde_json::json!({ "title": title, "file": file });
        self.authed(reqwest::Method::POST, &path, Some(body)).await
    }

    /// `PATCH …/creativetonies/{ctid}` with the full chapter list (an empty
    /// list deletes everything).
    pub async fn patch_chapters(
        &mut self,
        household: &str,
        ctid: &str,
        chapters: &[Chapter],
    ) -> TResult<Value> {
        let path = format!("/v2/households/{household}/creativetonies/{ctid}");
        let body = serde_json::json!({ "chapters": chapters });
        self.authed(reqwest::Method::PATCH, &path, Some(body)).await
    }
}

/// Accept either a bare JSON array or `{"<key>": [...]}` / `{"items": [...]}`.
fn parse_list<T: serde::de::DeserializeOwned>(v: &Value, key: &str) -> Vec<T> {
    let arr = if let Some(a) = v.as_array() {
        a.clone()
    } else {
        ["items", key]
            .iter()
            .find_map(|k| v.get(*k).and_then(Value::as_array).cloned())
            .unwrap_or_default()
    };
    arr.into_iter()
        .filter_map(|item| serde_json::from_value(item).ok())
        .collect()
}

fn parse_ticket(v: &Value) -> UploadTicket {
    let file_id = v
        .get("fileId")
        .or_else(|| v.get("file_id"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let url = v
        .get("url")
        .or_else(|| v.get("presignedUrl"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut fields = Vec::new();
    if let Some(obj) = v.get("fields").and_then(Value::as_object) {
        for (k, val) in obj {
            fields.push((k.clone(), val.as_str().unwrap_or_default().to_string()));
        }
    }
    UploadTicket {
        file_id,
        url,
        fields,
    }
}

/// Build the multipart body: form fields first, the file part last.
fn build_multipart(
    boundary: &str,
    fields: &[(String, String)],
    filename: &str,
    content_type: &str,
    bytes: &[u8],
) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body.extend_from_slice(
        format!(
            "Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n\
             Content-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multipart_fields_come_before_file() {
        let fields = vec![
            ("key".to_string(), "abc".to_string()),
            ("policy".to_string(), "pol".to_string()),
        ];
        let body = build_multipart("BND", &fields, "a.mp3", "audio/mpeg", b"DATA");
        let s = String::from_utf8_lossy(&body).to_string();
        let key_pos = s.find("name=\"key\"").unwrap();
        let policy_pos = s.find("name=\"policy\"").unwrap();
        let file_pos = s.find("name=\"file\"").unwrap();
        assert!(key_pos < policy_pos, "fields keep their order");
        assert!(policy_pos < file_pos, "file part must be last");
        assert!(s.contains("Content-Type: audio/mpeg"));
        assert!(s.contains("DATA"));
        assert!(s.trim_end().ends_with("--BND--"));
    }

    #[test]
    fn parse_ticket_reads_file_id_and_fields() {
        let v = serde_json::json!({
            "fileId": "f1",
            "url": "http://s3/put",
            "fields": { "key": "k", "policy": "p" }
        });
        let t = parse_ticket(&v);
        assert_eq!(t.file_id, "f1");
        assert_eq!(t.url, "http://s3/put");
        assert_eq!(t.fields.len(), 2);
    }
}
