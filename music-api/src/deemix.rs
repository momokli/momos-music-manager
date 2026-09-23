//! Minimal deemix client (login / add to queue / read queue).
//!
//! Response bodies are **never** logged: `loginArl` echoes the ARL back, and
//! logging it would write a Deezer session token to disk.

use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct LoginResponse {
    #[serde(default)]
    status: i64,
}

#[derive(Debug, Deserialize)]
struct AddToQueueResponse {
    #[serde(default)]
    result: bool,
    #[serde(default)]
    errid: Option<String>,
    #[serde(default)]
    data: Option<AddData>,
}

#[derive(Debug, Deserialize)]
struct AddData {
    #[serde(default)]
    obj: Vec<QueuedObj>,
}

#[derive(Debug, Deserialize)]
struct QueuedObj {
    #[serde(default)]
    uuid: Option<String>,
}

#[derive(Debug, Deserialize)]
struct QueueResponse {
    #[serde(default)]
    queue: HashMap<String, QueueItem>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct QueueItem {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub status: String,
    /// deemix has moved the item out of the active queue into terminal state.
    #[serde(default)]
    pub progress: Option<f64>,
}

/// Outcome of an `addToQueue` call.
#[derive(Debug, Clone)]
pub enum AddOutcome {
    /// Accepted. `uuid` is the handle to poll (`track_<id>_1`).
    Queued { uuid: Option<String> },
    /// Rejected by deemix (`errid` is e.g. `CantStream`).
    Rejected { errid: Option<String> },
}

/// Authenticate against deemix. Idempotent; call per work cycle.
pub async fn login(http: &reqwest::Client, base: &str, arl: &str) -> anyhow::Result<()> {
    let resp = http
        .post(format!("{base}/api/loginArl"))
        .json(&serde_json::json!({ "arl": arl }))
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("deemix loginArl returned HTTP {}", resp.status());
    }

    // Never log the body — it contains the ARL.
    let body: LoginResponse = resp.json().await?;
    if body.status == 0 {
        anyhow::bail!("deemix rejected the ARL");
    }
    Ok(())
}

/// Queue a source URL at the given bitrate (1 = 128, 3 = 320, 9 = FLAC).
pub async fn add_to_queue(
    http: &reqwest::Client,
    base: &str,
    url: &str,
    bitrate: u8,
) -> anyhow::Result<AddOutcome> {
    let resp = http
        .post(format!("{base}/api/addToQueue"))
        .json(&serde_json::json!({ "url": url, "bitrate": bitrate }))
        .send()
        .await?;

    if !resp.status().is_success() {
        anyhow::bail!("deemix addToQueue returned HTTP {}", resp.status());
    }

    let body: AddToQueueResponse = resp.json().await?;
    if body.result {
        let uuid = body
            .data
            .and_then(|d| d.obj.into_iter().find_map(|o| o.uuid));
        Ok(AddOutcome::Queued { uuid })
    } else {
        Ok(AddOutcome::Rejected { errid: body.errid })
    }
}

/// Read the current deemix queue, keyed by download UUID.
pub async fn queue(
    http: &reqwest::Client,
    base: &str,
) -> anyhow::Result<HashMap<String, QueueItem>> {
    let resp = http.get(format!("{base}/api/getQueue")).send().await?;
    if !resp.status().is_success() {
        anyhow::bail!("deemix getQueue returned HTTP {}", resp.status());
    }
    let body: QueueResponse = resp.json().await?;
    Ok(body.queue)
}

/// Whether a deemix status string means the download is finished.
pub fn is_terminal_status(status: &str) -> bool {
    matches!(
        status.to_lowercase().as_str(),
        "completed" | "witherrors" | "failed" | "skipped"
    )
}

/// Whether a deemed status is a success (file should exist on disk).
pub fn is_success_status(status: &str) -> bool {
    matches!(status.to_lowercase().as_str(), "completed" | "witherrors")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_accepted_add() {
        let json = r#"{"result":true,"data":{"obj":[{"type":"track","id":"3135556","uuid":"track_3135556_1"}]}}"#;
        let r: AddToQueueResponse = serde_json::from_str(json).unwrap();
        assert!(r.result);
        assert_eq!(
            r.data.unwrap().obj.into_iter().next().unwrap().uuid.unwrap(),
            "track_3135556_1"
        );
    }

    #[test]
    fn parses_rejected_add() {
        let json = r#"{"result":false,"errid":"CantStream","data":{"url":["x"],"bitrate":3}}"#;
        let r: AddToQueueResponse = serde_json::from_str(json).unwrap();
        assert!(!r.result);
        assert_eq!(r.errid.as_deref(), Some("CantStream"));
    }

    #[test]
    fn status_helpers() {
        assert!(is_terminal_status("completed"));
        assert!(is_terminal_status("withErrors"));
        assert!(!is_terminal_status("downloading"));
        assert!(is_success_status("completed"));
        assert!(is_success_status("witherrors"));
        assert!(!is_success_status("failed"));
    }
}
