//! SoundCloud public scraping via the api-v2 endpoints.
//!
//! yt-dlp's `--flat-playlist` mode yields no metadata for SoundCloud sets in
//! some environments (and only id/title/url when it does work). The SoundCloud
//! web player embeds a public `client_id` in `window.__sc_hydration`
//! (`apiClient`) that authorises the api-v2 endpoints, which return full track
//! objects (duration, genre, artwork, uploader). We scrape that id — plus the
//! owner's user id — from the profile page, then call the API.
//!
//! Verified working (2026-10) against `soundcloud.com/momokli/{sets,likes}`:
//! `GET /resolve`, `GET /users/{id}/playlists`, `GET /users/{id}/likes`.

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// Default api-v2 base. Overridable so integration tests can point at a mock.
pub const DEFAULT_API_BASE: &str = "https://api-v2.soundcloud.com";

/// A browser-ish UA — SoundCloud only serves the hydration payload to clients
/// that look like a real browser.
const UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
    AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// What kind of SoundCloud URL we were handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UrlKind {
    /// `…/{user}/likes`
    Likes,
    /// `…/{user}/sets`
    AllPlaylists,
    /// `…/{user}/sets/{name}` (or any other single playlist URL)
    SinglePlaylist,
}

/// Classify a SoundCloud URL by its path.
pub fn classify(url: &str) -> UrlKind {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let trimmed = path.trim_end_matches('/');
    if trimmed.ends_with("/likes") {
        UrlKind::Likes
    } else if trimmed.ends_with("/sets") {
        UrlKind::AllPlaylists
    } else {
        UrlKind::SinglePlaylist
    }
}

/// A ready-to-use api-v2 client bound to one profile's `client_id` + user id.
pub struct Client {
    http: reqwest::Client,
    api_base: String,
    client_id: String,
    user_id: i64,
}

impl Client {
    /// Fetch `profile_url`, scrape the public `client_id` + owner user id from
    /// the embedded hydration payload, and return a ready API client.
    pub async fn connect(api_base: &str, profile_url: &str) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(UA)
            .build()
            .context("build reqwest client")?;

        let html = http
            .get(profile_url)
            .send()
            .await
            .with_context(|| format!("GET {profile_url}"))?
            .error_for_status()
            .with_context(|| format!("GET {profile_url} returned an error status"))?
            .text()
            .await
            .context("read SoundCloud profile body")?;

        let hydration = extract_hydration(&html).context("no __sc_hydration payload found")?;

        // client_id: prefer the hydration `apiClient`, then scan the HTML, then
        // the JS bundles (the id usually lives in the bundles, not the HTML).
        let mut client_id = hydration_client_id(&hydration).or_else(|| find_client_id(&html));
        if client_id.is_none() {
            for src in extract_script_srcs(&html).into_iter().take(12) {
                if let Ok(resp) = http.get(&src).send().await {
                    if let Ok(body) = resp.text().await {
                        if let Some(id) = find_client_id(&body) {
                            client_id = Some(id);
                            break;
                        }
                    }
                }
            }
        }
        let client_id =
            client_id.context("could not find a SoundCloud client_id (hydration + JS bundles)")?;

        let user_id = hydration_user_id(&hydration)
            .context("could not find the SoundCloud user id in the hydration payload")?;

        Ok(Self {
            http,
            api_base: api_base.trim_end_matches('/').to_string(),
            client_id,
            user_id,
        })
    }

    pub fn user_id(&self) -> i64 {
        self.user_id
    }

    /// Resolve a single playlist URL to its full object (with `tracks`).
    pub async fn resolve(&self, url: &str) -> Result<Value> {
        let endpoint = format!(
            "{}/resolve?url={}&client_id={}",
            self.api_base,
            urlencoding::encode(url),
            self.client_id
        );
        self.get_json(&endpoint).await
    }

    /// All playlists (sets) owned by the user, paginated.
    pub async fn user_playlists(&self) -> Result<Vec<Value>> {
        let first = format!(
            "{}/users/{}/playlists?client_id={}&limit=50",
            self.api_base, self.user_id, self.client_id
        );
        self.collect(&first).await
    }

    /// All liked tracks, paginated.
    pub async fn user_likes(&self) -> Result<Vec<Value>> {
        let first = format!(
            "{}/users/{}/likes?client_id={}&limit=50",
            self.api_base, self.user_id, self.client_id
        );
        self.collect(&first).await
    }

    /// Follow `next_href` until exhausted, collecting `collection` entries.
    async fn collect(&self, first: &str) -> Result<Vec<Value>> {
        let mut out = Vec::new();
        let mut next = Some(first.to_string());
        while let Some(url) = next {
            let page = self.get_json(&url).await?;
            if let Some(arr) = page["collection"].as_array() {
                out.extend(arr.iter().cloned());
            }
            next = page["next_href"]
                .as_str()
                .map(|href| self.with_client_id(href));
        }
        Ok(out)
    }

    /// The api-v2 `next_href` omits `client_id`; re-attach it.
    fn with_client_id(&self, url: &str) -> String {
        if url.contains("client_id=") {
            url.to_string()
        } else if url.contains('?') {
            format!("{url}&client_id={}", self.client_id)
        } else {
            format!("{url}?client_id={}", self.client_id)
        }
    }

    async fn get_json(&self, url: &str) -> Result<Value> {
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        let body = resp.text().await.context("read SoundCloud API body")?;
        if !status.is_success() {
            bail!(
                "SoundCloud API {url} returned {status}: {}",
                truncate(&body)
            );
        }
        serde_json::from_str(&body).with_context(|| format!("parse SoundCloud API JSON from {url}"))
    }
}

fn truncate(s: &str) -> String {
    s.chars().take(200).collect()
}

/// Extract the JSON array assigned to `window.__sc_hydration`.
fn extract_hydration(html: &str) -> Result<Vec<Value>> {
    let marker = "window.__sc_hydration";
    let start = html.find(marker).context("marker not found")?;
    let rest = &html[start..];
    let eq = rest.find('=').context("no '=' after marker")?;
    let rest = rest[eq + 1..].trim_start();
    let open = rest.find('[').context("no '[' after marker")?;
    let json = balanced_slice(&rest[open..], '[', ']').context("unbalanced hydration array")?;
    serde_json::from_str(json).context("parse hydration JSON")
}

/// Return the substring starting at `s[0]` up to and including the matching
/// close bracket, respecting JSON strings and escapes.
fn balanced_slice(s: &str, open: char, close: char) -> Option<&str> {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    for (i, &b) in s.as_bytes().iter().enumerate() {
        let c = b as char;
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        if c == '"' {
            in_str = true;
        } else if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(&s[..=i]);
            }
        }
    }
    None
}

fn hydration_client_id(hydration: &[Value]) -> Option<String> {
    hydration.iter().find_map(|e| {
        (e["hydratable"].as_str() == Some("apiClient"))
            .then(|| e["data"]["id"].as_str().map(str::to_string))
            .flatten()
    })
}

fn hydration_user_id(hydration: &[Value]) -> Option<i64> {
    hydration.iter().find_map(|e| {
        (e["hydratable"].as_str() == Some("user"))
            .then(|| e["data"]["id"].as_i64())
            .flatten()
    })
}

/// Find a `client_id`-style token in arbitrary text (`client_id:"…"`,
/// `client_id":"…"`, `client_id=…`). Requires ≥16 alphanumerics to avoid noise.
fn find_client_id(text: &str) -> Option<String> {
    let needle = "client_id";
    let mut idx = 0;
    while let Some(pos) = text[idx..].find(needle) {
        let abs = idx + pos;
        let rest = &text[abs + needle.len()..];
        let rest = rest.trim_start_matches([':', '=', '"', '\'', ' ']);
        let id: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if id.len() >= 16 {
            return Some(id);
        }
        idx = abs + needle.len();
    }
    None
}

/// Collect `src="…"` URLs of `<script>` tags that point at JS bundles.
fn extract_script_srcs(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(pos) = rest.find("<script") {
        rest = &rest[pos..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..end];
        if let Some(src) = attr(tag, "src") {
            if src.starts_with("http") && src.ends_with(".js") {
                out.push(src);
            }
        }
        rest = &rest[end..];
    }
    out
}

fn attr(tag: &str, name: &str) -> Option<String> {
    let pat = format!("{name}=\"");
    let pos = tag.find(&pat)?;
    let rest = &tag[pos + pat.len()..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_detects_url_kinds() {
        assert_eq!(
            classify("https://soundcloud.com/momokli/likes"),
            UrlKind::Likes
        );
        assert_eq!(
            classify("https://soundcloud.com/momokli/likes/"),
            UrlKind::Likes
        );
        assert_eq!(
            classify("https://soundcloud.com/momokli/sets"),
            UrlKind::AllPlaylists
        );
        assert_eq!(
            classify("https://soundcloud.com/momokli/sets/discover"),
            UrlKind::SinglePlaylist
        );
        assert_eq!(
            classify("https://soundcloud.com/momokli/sets/discover?si=abc"),
            UrlKind::SinglePlaylist
        );
    }

    #[test]
    fn extract_hydration_reads_nested_json() {
        let html = r#"<html><script>window.__sc_hydration = [
            {"hydratable":"apiClient","data":{"id":"abc123def456ghi789"}},
            {"hydratable":"user","data":{"id":35227519,"permalink":"momokli"}}
        ];</script></html>"#;
        let h = extract_hydration(html).unwrap();
        assert_eq!(
            hydration_client_id(&h).as_deref(),
            Some("abc123def456ghi789")
        );
        assert_eq!(hydration_user_id(&h), Some(35227519));
    }

    #[test]
    fn balanced_slice_ignores_brackets_in_strings() {
        let s = r#"[{"a":"]"},{"b":"["}]"#;
        assert_eq!(balanced_slice(s, '[', ']').unwrap(), s);
    }

    #[test]
    fn find_client_id_matches_common_shapes() {
        assert_eq!(
            find_client_id(r#"x client_id:"vI5BsvpTIlavDLl7RDbbcFAPg8kls8Bg" y"#).as_deref(),
            Some("vI5BsvpTIlavDLl7RDbbcFAPg8kls8Bg")
        );
        assert_eq!(
            find_client_id(r#"client_id":"vI5BsvpTIlavDLl7RDbbcFAPg8kls8Bg""#).as_deref(),
            Some("vI5BsvpTIlavDLl7RDbbcFAPg8kls8Bg")
        );
        assert_eq!(find_client_id("client_id: short"), None);
    }
}
