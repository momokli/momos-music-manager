//! Deezer public API: ISRC → track resolution (no auth, no deemix needed).

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Deserialize)]
pub struct DeezerArtist {
    /// Deezer sometimes omits `name` (e.g. for some compilations); default it.
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct DeezerAlbum {
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Deserialize)]
struct DeezerError {
    #[serde(default)]
    message: String,
    #[serde(default)]
    code: i64,
}

/// Raw `/track/isrc:<isrc>` payload. Deezer reports "not found" as an `error`
/// object with HTTP 200, so absence is a field, not a status code.
#[derive(Debug, Deserialize)]
struct DeezerTrackResponse {
    #[serde(default)]
    id: i64,
    #[serde(default)]
    readable: bool,
    #[serde(default)]
    title: String,
    #[serde(default)]
    artist: Option<DeezerArtist>,
    #[serde(default)]
    album: Option<DeezerAlbum>,
    #[serde(default)]
    error: Option<DeezerError>,
}

/// Outcome of an ISRC lookup.
#[derive(Debug, Clone)]
pub enum Lookup {
    /// Deezer knows the track and it is streamable.
    Found {
        deezer_id: String,
        title: String,
        artist: String,
        album: String,
    },
    /// Deezer has no such ISRC (or it is not streamable for any account).
    /// `title`/`artist` may still be present (Deezer knows the track but it is
    /// not streamable) so a fallback provider can search for it.
    Absent {
        reason: String,
        title: Option<String>,
        artist: Option<String>,
    },
}

/// Resolve a single ISRC against Deezer's public API.
pub async fn lookup_isrc(http: &reqwest::Client, base: &str, isrc: &str) -> anyhow::Result<Lookup> {
    let url = format!("{base}/track/isrc:{isrc}");
    let resp = http.get(&url).send().await?;

    if !resp.status().is_success() {
        // Deezer's public API uses 200 for "not found", so anything else is a
        // real transport/rate-limit problem we should retry, not an absence.
        anyhow::bail!("deezer returned HTTP {} for {}", resp.status(), isrc);
    }

    let body: DeezerTrackResponse = resp.json().await?;

    if let Some(err) = body.error {
        return Ok(Lookup::Absent {
            reason: format!("deezer error {}: {}", err.code, err.message),
            title: None,
            artist: None,
        });
    }
    if body.id == 0 {
        return Ok(Lookup::Absent {
            reason: "no deezer match".to_string(),
            title: None,
            artist: None,
        });
    }
    if !body.readable {
        // Deezer knows the track but it is not streamable — keep the metadata
        // so the fallback provider can search for it.
        return Ok(Lookup::Absent {
            reason: "not streamable on deezer".to_string(),
            title: Some(body.title),
            artist: body.artist.map(|a| a.name),
        });
    }

    Ok(Lookup::Found {
        deezer_id: body.id.to_string(),
        title: body.title,
        artist: body.artist.map(|a| a.name).unwrap_or_default(),
        album: body.album.map(|a| a.title).unwrap_or_default(),
    })
}

/// Build the deemix source URL for a resolved Deezer track.
pub fn deemix_track_url(deezer_id: &str) -> String {
    format!("https://www.deezer.com/track/{deezer_id}")
}

/// The account tier behind an ARL, as reported by Deezer's web API.
#[derive(Debug, Clone)]
pub struct ArlTier {
    /// e.g. `Deezer Free`, `Deezer Premium`, `Deezer HiFi`.
    pub offer: String,
    /// Whether the account may stream lossless (FLAC).
    pub lossless: bool,
}

impl ArlTier {
    /// True when the account can stream FLAC/HQ.
    pub fn can_stream_flac(&self) -> bool {
        self.lossless
    }
}

/// Query Deezer's web API for the tier behind an ARL. The ARL is sent as a
/// cookie; the response is never logged (it echoes account data).
///
/// Returns `None` when the ARL is empty or the response cannot be parsed.
pub async fn check_arl_tier(http: &reqwest::Client, arl: &str) -> Option<ArlTier> {
    if arl.trim().is_empty() {
        return None;
    }
    let resp = http
        .get(
            "https://www.deezer.com/ajax/gw-light.php\
             ?method=deezer.getUserData&api_version=1.0&api_token=null",
        )
        .header("Cookie", format!("arl={arl}"))
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36",
        )
        .send()
        .await
        .ok()?;
    let body: Value = resp.json().await.ok()?;
    let results = &body["results"];
    let offer = results["OFFER_NAME"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let lossless = results["USER"]["OPTIONS"]["web_sound_quality"]["lossless"]
        .as_bool()
        .unwrap_or(false);
    Some(ArlTier { offer, lossless })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_found_payload() {
        let json = r#"{"id":836932812,"readable":true,"title":"Sudno","isrc":"AEA0D1846146",
                       "artist":{"id":1,"name":"Molchat Doma"},"album":{"id":2,"title":"Etazhi"}}"#;
        let r: DeezerTrackResponse = serde_json::from_str(json).unwrap();
        assert!(r.error.is_none());
        assert_eq!(r.id, 836932812);
        assert_eq!(r.artist.unwrap().name, "Molchat Doma");
    }

    #[test]
    fn parses_absent_payload() {
        let json = r#"{"error":{"type":"DataException","message":"no data","code":800}}"#;
        let r: DeezerTrackResponse = serde_json::from_str(json).unwrap();
        let err = r.error.unwrap();
        assert_eq!(err.code, 800);
    }

    #[test]
    fn builds_track_url() {
        assert_eq!(
            deemix_track_url("3135556"),
            "https://www.deezer.com/track/3135556"
        );
    }
}
