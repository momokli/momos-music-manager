//! Client for the BPM/key analyzer service (Essentia, e.g. on the hub host at
//! `http://<host>:8711`). Reuses the hub's contract:
//!
//! ```text
//! POST {base}/analyze  {"path": "<server-local file>", "embed": false}
//!   →  {"bpm": 128.0, "key": "8A", "camelot": "8A", …}
//! ```
//!
//! Analysis is disabled unless `ANALYZER_URL` (fallback `HUB_ANALYZER_URL`) is
//! configured — see [`crate::Config::analyzer_url`].

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::json;

/// BPM + key the analyzer extracted from one file. Any field may be absent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Analysis {
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub camelot: Option<String>,
}

#[derive(Deserialize)]
struct Resp {
    #[serde(default)]
    bpm: Option<f64>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    camelot: Option<String>,
}

impl Analysis {
    /// True when the response carried no usable field (nothing to store).
    pub fn is_empty(&self) -> bool {
        self.bpm.is_none() && self.key.is_none() && self.camelot.is_none()
    }
}

/// Is an analyzer base URL configured (and non-blank)?
pub fn enabled(base: Option<&str>) -> bool {
    base.map(|s| !s.trim().is_empty()).unwrap_or(false)
}

/// Parse the analyzer's JSON body. Pure — unit-tested locally.
pub fn parse_response(body: &str) -> Result<Analysis> {
    let r: Resp = serde_json::from_str(body).context("parse analyzer response")?;
    Ok(Analysis {
        bpm: r.bpm,
        key: r.key,
        camelot: r.camelot,
    })
}

/// `POST {base}/analyze` for one server-local file. `embed:false` — this service
/// only wants BPM/key. Errors are returned for the caller to log and back off;
/// the worker never panics on a flaky analyzer.
pub async fn analyze(http: &reqwest::Client, base: &str, path: &str) -> Result<Analysis> {
    let url = format!("{}/analyze", base.trim_end_matches('/'));
    let resp = http
        .post(&url)
        // Analysis of a full track can take a while; override the shared
        // client's 60s default for this call only.
        .timeout(Duration::from_secs(180))
        .json(&json!({ "path": path, "embed": false }))
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let status = resp.status();
    let body = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("analyzer {url} -> {status}: {body}");
    }
    parse_response(&body).with_context(|| format!("analyzer {url}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_response() {
        let a = parse_response(r#"{"bpm":128.5,"key":"8A","camelot":"8A"}"#).unwrap();
        assert_eq!(a.bpm, Some(128.5));
        assert_eq!(a.key.as_deref(), Some("8A"));
        assert_eq!(a.camelot.as_deref(), Some("8A"));
        assert!(!a.is_empty());
    }

    #[test]
    fn parses_partial_and_tolerates_extra_fields() {
        // The analyzer may add fields (embedding, genres, …) we don't request.
        let a = parse_response(r#"{"bpm":124.0,"embedding":[0.1,0.2],"genres":[]}"#).unwrap();
        assert_eq!(a.bpm, Some(124.0));
        assert_eq!(a.key, None);
        assert_eq!(a.camelot, None);
        assert!(!a.is_empty());
    }

    #[test]
    fn empty_response_is_empty() {
        assert!(parse_response(r#"{}"#).unwrap().is_empty());
    }

    #[test]
    fn rejects_non_json() {
        assert!(parse_response("not json").is_err());
    }

    #[test]
    fn enabled_only_for_non_blank() {
        assert!(enabled(Some("http://127.0.0.1:8711")));
        assert!(!enabled(Some("   ")));
        assert!(!enabled(None));
    }
}
