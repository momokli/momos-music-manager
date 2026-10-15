//! Device pairing with the hub (issue #362, ADR-081 §10.2).
//!
//! kellner generates a crypto-random **device token** (its durable credential)
//! and a **pairing token** (the one-time capability embedded in the approval
//! link). It registers both with the hub via
//! `POST {hub_url}/api/hub/devices` and prints the approval URL; a logged-in
//! user opens it and answers "diesen kellner anheuern?".
//!
//! # Binding confirmation
//!
//! The merged hub (#363) ships **no device-token-gated route**, so kellner
//! cannot poll for approval. Confirmation is therefore user-driven: open the
//! URL, let the user approve, then press Enter. **Auto-poll is deferred to
//! #364** and must not be invented here.
//!
//! The raw token is sent as-is (hex, 64 chars); the hub hashes it itself. The
//! token is never logged or printed.

use std::process::Command;

use anyhow::{Context, Result, anyhow};
use rand::RngCore;
use reqwest::header::HeaderValue;
use serde::{Deserialize, Serialize};
use url::Url;

use crate::config::{self, Config, DeviceIdentity};

/// Generate a crypto-random lowercase-hex token (32 bytes → 64 chars).
///
/// Satisfies the hub's `valid_raw_token` rule (hex, `len >= 32`).
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

/// Body of `POST {hub_url}/api/hub/devices` (verbatim hub field names).
#[derive(Clone, Serialize, PartialEq)]
pub struct RegisterRequest {
    pub name: String,
    pub pairing_token: String,
    pub device_token: String,
}

impl std::fmt::Debug for RegisterRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RegisterRequest")
            .field("name", &self.name)
            .field("pairing_token", &"***")
            .field("device_token", &"***")
            .finish()
    }
}

/// Response of `POST {hub_url}/api/hub/devices`.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct RegisterResponse {
    pub device_id: i64,
    pub approval_path: String,
}

/// Abstraction over the hub registration call so the flow is testable offline.
pub trait HubTransport {
    fn register(&self, req: RegisterRequest) -> impl std::future::Future<Output = Result<RegisterResponse>>;
}

/// Real HTTP transport (`reqwest`, rustls, JSON).
pub struct HttpHubTransport {
    hub_url: String,
    client: reqwest::Client,
}

impl HttpHubTransport {
    pub fn new(hub_url: impl Into<String>) -> Self {
        Self {
            hub_url: hub_url.into(),
            client: reqwest::Client::new(),
        }
    }
}

impl HubTransport for HttpHubTransport {
    async fn register(&self, req: RegisterRequest) -> Result<RegisterResponse> {
        let url = format!(
            "{}/api/hub/devices",
            self.hub_url.trim_end_matches('/')
        );
        let resp = self
            .client
            .post(&url)
            .json(&req)
            .send()
            .await
            .context("registering device with the hub (request failed)")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "hub rejected device registration: HTTP {status}: {}",
                body.trim()
            ));
        }
        resp.json::<RegisterResponse>()
            .await
            .context("parsing hub registration response")
    }
}

/// Build the approval URL by joining `hub_url` and the hub-provided path.
pub fn approval_url(hub_url: &str, approval_path: &str) -> Result<Url> {
    let base = Url::parse(hub_url).with_context(|| format!("invalid hub url: {hub_url}"))?;
    base.join(approval_path)
        .with_context(|| format!("invalid approval path: {approval_path}"))
}

/// Best-effort browser open; every failure is swallowed (the URL is printed).
pub fn open_browser(url: &Url) -> bool {
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(url.as_str());
        c
    } else if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", "start", "", url.as_str()]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(url.as_str());
        c
    };
    cmd.spawn().is_ok()
}

/// Wait for the user to confirm they approved the link in the browser.
///
/// `wait` blocks even without a TTY (service/tray precursor). Never polls.
pub fn confirm_enter(wait: bool) -> Result<()> {
    use std::io::{BufRead, IsTerminal, Write};

    let stdin = std::io::stdin();
    if !wait && !stdin.is_terminal() {
        // Non-interactive and not asked to block: do not hang.
        return Ok(());
    }
    print!(
        "Nachdem du im Browser bestätigt hast, drücke Enter … "
    );
    std::io::stdout().flush().ok();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    Ok(())
}

/// `Bearer <token>` header for hub / delegated music-api calls (#364/#365).
#[allow(dead_code)] // used once the device-gated routes land (#364/#365)
pub fn auth_header(token: &str) -> Result<HeaderValue> {
    let value = format!("Bearer {token}");
    HeaderValue::from_str(&value).context("building bearer auth header")
}

/// Options for [`run_pair`], mirroring the CLI flags.
#[derive(Debug, Clone)]
pub struct PairOptions {
    pub hub_url: String,
    pub name: String,
    pub no_open: bool,
    pub print_only: bool,
    pub wait: bool,
    pub force: bool,
    /// `false` in tests: skip browser open + Enter prompt, still register/persist.
    pub interactive: bool,
}

/// Orchestrate a pairing run. Never prints or logs the token.
pub async fn run_pair(
    cfg: &mut Config,
    transport: &impl HubTransport,
    opts: &PairOptions,
) -> Result<()> {
    if let Some(existing) = &cfg.device {
        if !opts.force && !opts.print_only {
            return Err(anyhow!(
                "this device is already paired (device_id {}); pass --force to re-pair",
                existing.device_id
            ));
        }
    }

    let pairing_token = generate_token();
    let device_token = generate_token();
    let request = RegisterRequest {
        name: opts.name.clone(),
        pairing_token: pairing_token.clone(),
        device_token: device_token.clone(),
    };

    let response = transport
        .register(request)
        .await
        .context("registering the device with the hub")?;
    let url = approval_url(&opts.hub_url, &response.approval_path)?;

    println!("kellner: registered as {}", opts.name);
    println!("device_id: {}", response.device_id);
    println!("Approve this kellner by opening:");
    println!("  {url}");

    if opts.print_only {
        // Dry run (scripts/CI): register + print only, do not bind or persist.
        return Ok(());
    }

    if opts.interactive {
        if !opts.no_open && !open_browser(&url) {
            eprintln!("could not open a browser automatically — please open the URL above");
        }
        // Binding confirmation is user-driven: the hub exposes no device-gated
        // route yet, so we cannot poll for approval. Auto-poll → #364.
        confirm_enter(opts.wait)?;
    }

    cfg.hub_url = Some(opts.hub_url.clone());
    cfg.device = Some(DeviceIdentity {
        device_id: response.device_id,
        device_name: opts.name.clone(),
        device_token,
        paired_at: now_iso(),
    });
    config::save(cfg).context("persisting the device identity")?;
    println!("kellner: paired and stored (device_id {}).", response.device_id);
    Ok(())
}

fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct FakeTransport {
        recorded: Mutex<Option<RegisterRequest>>,
        response: RegisterResponse,
    }

    impl FakeTransport {
        fn new(response: RegisterResponse) -> Self {
            Self {
                recorded: Mutex::new(None),
                response,
            }
        }

        fn recorded(&self) -> RegisterRequest {
            self.recorded.lock().unwrap().clone().expect("not called")
        }
    }

    impl HubTransport for FakeTransport {
        async fn register(&self, req: RegisterRequest) -> Result<RegisterResponse> {
            *self.recorded.lock().unwrap() = Some(req);
            Ok(self.response.clone())
        }
    }

    fn opts(name: &str) -> PairOptions {
        PairOptions {
            hub_url: "https://hub.zukkafabrik.de".into(),
            name: name.into(),
            no_open: true,
            print_only: false,
            wait: false,
            force: false,
            interactive: false,
        }
    }

    #[test]
    fn generate_token_is_64_hex() {
        let t = generate_token();
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(t.chars().all(|c| !c.is_ascii_uppercase()));
        assert!(t.len() >= 32);
    }

    #[test]
    fn generate_token_is_unique() {
        assert_ne!(generate_token(), generate_token());
    }

    #[test]
    fn register_request_json_shape() {
        let req = RegisterRequest {
            name: "host".into(),
            pairing_token: "aa".into(),
            device_token: "bb".into(),
        };
        let v = serde_json::to_value(&req).unwrap();
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 3);
        assert_eq!(v["name"], "host");
        assert_eq!(v["pairing_token"], "aa");
        assert_eq!(v["device_token"], "bb");
    }

    #[test]
    fn register_response_parses() {
        let json = r#"{"device_id": 7, "approval_path": "/kellner/anheuern?token=abc"}"#;
        let resp: RegisterResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.device_id, 7);
        assert_eq!(resp.approval_path, "/kellner/anheuern?token=abc");
    }

    #[tokio::test]
    async fn run_pair_uses_fake_transport() {
        let _guard = config::test_support::env_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kellner/config.toml");
        unsafe {
            std::env::set_var("KELLNER_CONFIG", &path);
        }

        let fake = FakeTransport::new(RegisterResponse {
            device_id: 123,
            approval_path: "/kellner/anheuern?token=xyz".into(),
        });
        let mut cfg = Config::default();
        let o = opts("my-mac");
        run_pair(&mut cfg, &fake, &o).await.unwrap();

        let recorded = fake.recorded();
        assert_eq!(recorded.name, "my-mac");
        assert_eq!(recorded.pairing_token.len(), 64);
        assert_eq!(recorded.device_token.len(), 64);

        let loaded = config::load().unwrap();
        let device = loaded.device.expect("identity persisted");
        assert_eq!(device.device_id, 123);
        assert_eq!(device.device_name, "my-mac");
        assert_eq!(device.device_token, recorded.device_token);
    }

    #[test]
    fn approval_url_joins_path() {
        let url = approval_url(
            "https://hub.zukkafabrik.de",
            "/kellner/anheuern?token=x",
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://hub.zukkafabrik.de/kellner/anheuern?token=x"
        );
    }

    #[test]
    fn auth_header_is_bearer() {
        let h = auth_header("abc123").unwrap();
        assert_eq!(h.to_str().unwrap(), "Bearer abc123");
    }

    #[tokio::test]
    async fn print_only_does_not_persist() {
        let _guard = config::test_support::env_lock();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kellner/config.toml");
        unsafe {
            std::env::set_var("KELLNER_CONFIG", &path);
        }

        let fake = FakeTransport::new(RegisterResponse {
            device_id: 9,
            approval_path: "/kellner/anheuern?token=z".into(),
        });
        let mut cfg = Config::default();
        let mut o = opts("dry-run");
        o.print_only = true;
        run_pair(&mut cfg, &fake, &o).await.unwrap();

        assert!(cfg.device.is_none());
        assert!(!path.exists(), "print-only must not write a config file");
    }
}
