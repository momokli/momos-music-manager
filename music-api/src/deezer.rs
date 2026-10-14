//! Deezer public API: ISRC → track resolution (no auth, no deemix needed).

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct DeezerArtist {
    #[serde(default)]
    pub id: Option<i64>,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct DeezerAlbum {
    #[serde(default)]
    pub id: Option<i64>,
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
        /// Deezer artist id (for the vault's artist extid satellite).
        artist_id: Option<String>,
        /// Deezer album id (for the vault's album extid satellite).
        album_id: Option<String>,
    },
    /// Deezer has no such ISRC (or it is not streamable for any account).
    Absent { reason: String },
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
        });
    }
    if body.id == 0 {
        return Ok(Lookup::Absent {
            reason: "no deezer match".to_string(),
        });
    }
    if !body.readable {
        return Ok(Lookup::Absent {
            reason: "not streamable on deezer".to_string(),
        });
    }

    let (artist, artist_id) = match body.artist {
        Some(a) => (a.name, a.id.map(|i| i.to_string())),
        None => (String::new(), None),
    };
    let (album, album_id) = match body.album {
        Some(a) => (a.title, a.id.map(|i| i.to_string())),
        None => (String::new(), None),
    };

    Ok(Lookup::Found {
        deezer_id: body.id.to_string(),
        title: body.title,
        artist,
        album,
        artist_id,
        album_id,
    })
}

/// Build the deemix source URL for a resolved Deezer track.
pub fn deemix_track_url(deezer_id: &str) -> String {
    format!("https://www.deezer.com/track/{deezer_id}")
}

/// Normalise a caller-supplied artist reference into the canonical Deezer artist
/// URL deemix accepts. A bare numeric id becomes
/// `https://www.deezer.com/artist/<id>`; a `deezer.com/artist/<id>` URL is
/// canonicalised; any other URL is passed through unchanged. `None` when the
/// input is neither a numeric id nor a URL.
pub fn normalize_artist_url(input: &str) -> Option<String> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if s.chars().all(|c| c.is_ascii_digit()) {
        return Some(format!("https://www.deezer.com/artist/{s}"));
    }
    if s.contains("://") {
        // Only a Deezer artist URL is canonicalised; other URLs pass through.
        if s.contains("deezer.com") {
            if let Some(id) = deezer_artist_id(s) {
                return Some(format!("https://www.deezer.com/artist/{id}"));
            }
        }
        return Some(s.to_string());
    }
    None
}

/// Extract the numeric id from a `deezer.com/artist/<id>` URL.
fn deezer_artist_id(url: &str) -> Option<String> {
    let rest = url.split("/artist/").nth(1)?;
    let id: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    (!id.is_empty()).then_some(id)
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

    #[test]
    fn normalises_artist_references() {
        assert_eq!(
            normalize_artist_url("12345").as_deref(),
            Some("https://www.deezer.com/artist/12345")
        );
        assert_eq!(
            normalize_artist_url("https://www.deezer.com/artist/12345").as_deref(),
            Some("https://www.deezer.com/artist/12345")
        );
        assert_eq!(
            normalize_artist_url("https://www.deezer.com/de/artist/12345?utm=x").as_deref(),
            Some("https://www.deezer.com/artist/12345")
        );
        // A non-Deezer URL is passed through.
        assert_eq!(
            normalize_artist_url("https://example.com/artist/9").as_deref(),
            Some("https://example.com/artist/9")
        );
        assert_eq!(normalize_artist_url("not an artist"), None);
        assert_eq!(normalize_artist_url("  "), None);
    }
}
