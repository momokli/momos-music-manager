//! Deezer public API: ISRC → track resolution (no auth needed).

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Timeout for the gw-light `deezer.getUserData` probe. Kept short: the tier
/// check must never stall the boot path (it runs in a spawned task).
const ARL_TIER_TIMEOUT: Duration = Duration::from_secs(5);

/// The Deezer account tier behind an ARL, as reported by gw-light's
/// `deezer.getUserData`. FLAC requires `lossless`.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ArlTier {
    pub offer: String,
    pub lossless: bool,
    pub high: bool,
    pub low: bool,
    pub user_id: Option<i64>,
}

impl ArlTier {
    /// Whether this account tier may stream lossless (FLAC).
    pub fn can_stream_flac(&self) -> bool {
        self.lossless
    }
}

/// Parse a gw-light `deezer.getUserData` body into an [`ArlTier`]. Pure and
/// tolerant: missing fields fall back to defaults and never panic. Returns
/// `None` only when the envelope itself carries no `results` object.
pub fn parse_user_data(body: &serde_json::Value) -> Option<ArlTier> {
    let results = body.get("results")?;
    let offer = results
        .get("OFFER_NAME")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let quality = results
        .get("USER")
        .and_then(|u| u.get("OPTIONS"))
        .and_then(|o| o.get("web_sound_quality"));
    let flag = |key: &str| {
        quality
            .and_then(|q| q.get(key))
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    };
    let user_id = results
        .get("USER")
        .and_then(|u| u.get("USER_ID"))
        .and_then(|v| v.as_i64());
    Some(ArlTier {
        offer,
        lossless: flag("lossless"),
        high: flag("high"),
        low: flag("low"),
        user_id,
    })
}

/// Fetch the account tier for `arl` from the Deezer gw-light endpoint.
///
/// A blank/whitespace ARL yields `Ok(None)` without any HTTP call. The request
/// is bounded by [`ARL_TIER_TIMEOUT`]; the response body is parsed but never
/// logged.
pub async fn fetch_arl_tier(
    http: &reqwest::Client,
    gw_base: &str,
    arl: &str,
) -> anyhow::Result<Option<ArlTier>> {
    let arl = arl.trim();
    if arl.is_empty() {
        return Ok(None);
    }
    let url = format!("{gw_base}?method=deezer.getUserData&api_version=1.0&api_token=***");
    let fut = http
        .get(&url)
        .header(reqwest::header::COOKIE, format!("arl={arl}"))
        .header(
            reqwest::header::USER_AGENT,
            "Mozilla/5.0 (compatible; music-api)",
        )
        .send();
    let resp = tokio::time::timeout(ARL_TIER_TIMEOUT, fut)
        .await
        .map_err(|_| anyhow::anyhow!("deezer gw-light getUserData timed out"))??;
    if !resp.status().is_success() {
        anyhow::bail!("deezer gw-light returned HTTP {}", resp.status());
    }
    let body: serde_json::Value = resp.json().await?;
    Ok(parse_user_data(&body))
}

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

// ── Metadata surface for the hub (artist albums, album detail, artist search) ──

/// Upper bound for one metadata page. Deezer documents 100 as the maximum
/// `limit`; a larger request is clamped instead of rejected.
pub const METADATA_MAX_LIMIT: usize = 100;

/// Bound on tracklist pages fetched to backfill a missing ISRC.
const ALBUM_TRACKLIST_MAX_PAGES: usize = 5;

/// Why a Deezer metadata call produced no usable answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MetaError {
    /// Deezer knows no such id/query: an `error` object with code 800, or an
    /// HTTP 404.
    NotFound,
    /// Transport failure, a non-404 HTTP status, a quota/busy error code, or a
    /// payload that does not match the documented shape.
    Upstream(String),
}

impl std::fmt::Display for MetaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetaError::NotFound => f.write_str("deezer: not found"),
            MetaError::Upstream(detail) => write!(f, "deezer: {detail}"),
        }
    }
}

impl std::error::Error for MetaError {}

/// One release in an artist's album list.
///
/// `nb_tracks` is always `null`: Deezer's `/artist/{id}/albums` does not carry a
/// track count, and the field stays in the contract so clients keep a stable
/// shape.
///
/// `seconds` is **not** part of Deezer's payload either: it is filled by the
/// handler from the own catalogue (`catalogue_albums.seconds`) after the page is
/// fetched or read from cache. Deezer's album list carries no length, so this
/// stays `None` for albums that are not (yet) in the catalogue.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSummary {
    pub album_id: String,
    pub title: String,
    pub release_date: Option<String>,
    pub record_type: Option<String>,
    #[serde(default)]
    pub cover_url: Option<String>,
    pub nb_tracks: Option<i64>,
    #[serde(default)]
    pub seconds: Option<i64>,
}

/// `GET /deezer/artist/{id}/albums` response.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistAlbumPage {
    pub albums: Vec<AlbumSummary>,
    pub total: i64,
    /// Deezer's own paging link, passed through verbatim (may be absent).
    #[serde(default)]
    pub next: Option<String>,
}

/// One track of an album detail. Tracks without an ISRC are kept (with
/// `isrc: null`) so the caller decides what to do with them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumTrack {
    #[serde(default)]
    pub isrc: Option<String>,
    pub title: String,
    pub duration_ms: i64,
    /// Deezer's 1-based `track_position` (`null` when Deezer omits it).
    pub position: Option<i64>,
}

/// `GET /deezer/album/{id}` response.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDetail {
    pub album_id: String,
    pub title: String,
    pub release_date: Option<String>,
    pub record_type: Option<String>,
    #[serde(default)]
    pub cover_url: Option<String>,
    /// Sum of every track's `duration` (seconds), rounded nowhere.
    pub seconds: i64,
    pub tracks: Vec<AlbumTrack>,
}

/// One hit of an artist search.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtistHit {
    pub artist_id: String,
    pub name: String,
    #[serde(default)]
    pub cover_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawAlbumList {
    #[serde(default)]
    data: Vec<RawAlbum>,
    #[serde(default)]
    total: i64,
    #[serde(default)]
    next: Option<String>,
}

/// Shared shape of a list entry and an `/album/{id}` object. Only the detail
/// response carries `tracks`; the list entry leaves the popular fields aside.
#[derive(Debug, Deserialize)]
struct RawAlbum {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    record_type: Option<String>,
    #[serde(default)]
    cover_big: Option<String>,
    #[serde(default)]
    cover_medium: Option<String>,
    #[serde(default)]
    cover_xl: Option<String>,
    #[serde(default)]
    tracks: Option<RawAlbumTracks>,
}

#[derive(Debug, Deserialize)]
struct RawAlbumTracks {
    #[serde(default)]
    data: Vec<RawAlbumTrack>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawAlbumTrack {
    /// Deezer track id — used to backfill the ISRC (see `merge_isrcs`).
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    isrc: Option<String>,
    #[serde(default)]
    title: String,
    #[serde(default)]
    duration: i64,
    #[serde(default)]
    track_position: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct RawArtistSearch {
    #[serde(default)]
    data: Vec<RawArtistHit>,
}

#[derive(Debug, Deserialize)]
struct RawArtistHit {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    name: String,
    #[serde(default)]
    picture_medium: Option<String>,
}

/// Deezer reports "no data" as an `error` object on an HTTP 200 response. Only
/// code 800 means "unknown id"; quota (`4`) and busy (`700`) are upstream
/// failures and must not masquerade as 404.
fn deezer_error(body: &serde_json::Value) -> Option<MetaError> {
    let err = body.get("error")?;
    if err.is_null() {
        return None;
    }
    let code = err.get("code").and_then(serde_json::Value::as_i64);
    Some(match code {
        Some(800) => MetaError::NotFound,
        Some(code) => MetaError::Upstream(format!("deezer error code {code}")),
        None => MetaError::Upstream("deezer error without a code".to_string()),
    })
}

fn malformed(what: &str) -> MetaError {
    MetaError::Upstream(format!("unexpected deezer {what} payload"))
}

/// Trimmed string, `None` when blank.
fn non_blank(raw: Option<String>) -> Option<String> {
    raw.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Cover URL by the contract's preference: `cover_big`, then `cover_medium`,
/// then `cover_xl`.
fn cover_url(album: &RawAlbum) -> Option<String> {
    [
        album.cover_big.as_deref(),
        album.cover_medium.as_deref(),
        album.cover_xl.as_deref(),
    ]
    .into_iter()
    .flatten()
    .map(str::trim)
    .find(|s| !s.is_empty())
    .map(str::to_string)
}

/// Map an `/artist/{id}/albums` payload into the hub response. Pure.
pub fn map_artist_albums(body: &serde_json::Value) -> Result<ArtistAlbumPage, MetaError> {
    if let Some(err) = deezer_error(body) {
        return Err(err);
    }
    let raw: RawAlbumList =
        serde_json::from_value(body.clone()).map_err(|_| malformed("artist albums"))?;

    let albums = raw
        .data
        .iter()
        .map(|album| {
            let album_id = album
                .id
                .filter(|id| *id > 0)
                .map(|id| id.to_string())
                .ok_or_else(|| malformed("album id"))?;
            Ok(AlbumSummary {
                album_id,
                title: album.title.clone(),
                release_date: non_blank(album.release_date.clone()),
                record_type: non_blank(album.record_type.clone()),
                cover_url: cover_url(album),
                nb_tracks: None,
                // Deezer's list has no length; the handler fills this from the
                // own catalogue (Story 3).
                seconds: None,
            })
        })
        .collect::<Result<Vec<_>, MetaError>>()?;

    Ok(ArtistAlbumPage {
        albums,
        total: raw.total,
        next: non_blank(raw.next),
    })
}

/// Map an `/album/{id}` payload into the hub response. Pure. Tracks are sorted
/// by `track_position`; a missing position sorts last.
pub fn map_album_detail(body: &serde_json::Value) -> Result<AlbumDetail, MetaError> {
    let raw = parse_raw_album(body)?;
    let raw_tracks = raw.tracks.as_ref().map(|t| t.data.clone()).unwrap_or_default();
    album_detail_from_raw(&raw, &raw_tracks)
}

/// Parse a `/album/{id}` payload, mapping Deezer's error object / id-less
/// response to [`MetaError::NotFound`].
fn parse_raw_album(body: &serde_json::Value) -> Result<RawAlbum, MetaError> {
    if let Some(err) = deezer_error(body) {
        return Err(err);
    }
    let raw: RawAlbum = serde_json::from_value(body.clone()).map_err(|_| malformed("album"))?;
    if raw.id.filter(|id| *id > 0).is_none() {
        return Err(MetaError::NotFound);
    }
    Ok(raw)
}

/// Build the hub-facing album from already-parsed pieces.
fn album_detail_from_raw(
    raw: &RawAlbum,
    raw_tracks: &[RawAlbumTrack],
) -> Result<AlbumDetail, MetaError> {
    let album_id = raw
        .id
        .filter(|id| *id > 0)
        .map(|id| id.to_string())
        .ok_or(MetaError::NotFound)?;

    let mut tracks: Vec<AlbumTrack> = raw_tracks
        .iter()
        .map(|track| AlbumTrack {
            isrc: non_blank(track.isrc.clone()),
            title: track.title.clone(),
            duration_ms: track.duration.max(0) * 1000,
            position: track.track_position,
        })
        .collect();
    tracks.sort_by_key(|t| t.position.unwrap_or(i64::MAX));

    Ok(AlbumDetail {
        album_id,
        title: raw.title.clone(),
        release_date: non_blank(raw.release_date.clone()),
        record_type: non_blank(raw.record_type.clone()),
        cover_url: cover_url(&raw),
        seconds: raw_tracks.iter().map(|t| t.duration.max(0)).sum(),
        tracks,
    })
}

/// Map a `/search/artist` payload into the hub response. Pure.
pub fn map_search_artists(body: &serde_json::Value) -> Result<Vec<ArtistHit>, MetaError> {
    if let Some(err) = deezer_error(body) {
        return Err(err);
    }
    let raw: RawArtistSearch =
        serde_json::from_value(body.clone()).map_err(|_| malformed("artist search"))?;

    raw.data
        .iter()
        .map(|hit| {
            let artist_id = hit
                .id
                .filter(|id| *id > 0)
                .map(|id| id.to_string())
                .ok_or_else(|| malformed("artist id"))?;
            Ok(ArtistHit {
                artist_id,
                name: hit.name.clone(),
                cover_url: non_blank(hit.picture_medium.clone()),
            })
        })
        .collect()
}

/// One GET against the public Deezer API, mapped onto [`MetaError`]. Nothing
/// from the response is logged here, and error texts never carry the ARL.
async fn get_json(
    http: &reqwest::Client,
    url: &str,
    query: &[(&str, String)],
) -> Result<serde_json::Value, MetaError> {
    let resp = http
        .get(url)
        .query(query)
        .send()
        .await
        .map_err(|e| MetaError::Upstream(format!("request failed: {}", e.without_url())))?;

    let status = resp.status();
    if status == reqwest::StatusCode::NOT_FOUND {
        return Err(MetaError::NotFound);
    }
    if !status.is_success() {
        return Err(MetaError::Upstream(format!(
            "deezer returned HTTP {status}"
        )));
    }

    resp.json()
        .await
        .map_err(|_| MetaError::Upstream("deezer sent an unreadable body".to_string()))
}

/// `GET {base}/artist/{id}/albums?limit=&index=` — the artist's release list.
/// One request; the list endpoint already carries every mapped field.
pub async fn artist_albums(
    http: &reqwest::Client,
    base: &str,
    artist_id: &str,
    limit: usize,
    index: usize,
) -> Result<ArtistAlbumPage, MetaError> {
    let url = format!("{base}/artist/{artist_id}/albums");
    let body = get_json(
        http,
        &url,
        &[
            ("limit", limit.clamp(1, METADATA_MAX_LIMIT).to_string()),
            ("index", index.to_string()),
        ],
    )
    .await?;
    map_artist_albums(&body)
}

/// `GET {base}/album/{id}` — album metadata plus its tracks. One request.
pub async fn album_detail(
    http: &reqwest::Client,
    base: &str,
    album_id: &str,
) -> Result<AlbumDetail, MetaError> {
    let url = format!("{base}/album/{album_id}");
    let body = get_json(http, &url, &[]).await?;
    let raw = parse_raw_album(&body)?;
    let mut raw_tracks = raw.tracks.as_ref().map(|t| t.data.clone()).unwrap_or_default();

    // `/album/{id}` omits the ISRCs of its tracks; only `/album/{id}/tracks`
    // carries them. Without this backfill every track would look like it has no
    // ISRC and the hub would skip the whole album.
    if raw_tracks.iter().any(|t| t.isrc.is_none()) {
        let mut next = format!("{base}/album/{album_id}/tracks");
        for _ in 0..ALBUM_TRACKLIST_MAX_PAGES {
            // Best effort: a failing tracklist must not break the album — the
            // hub handles tracks without ISRC by skipping that album.
            let page = match get_json(http, &next, &[]).await {
                Ok(page) => page,
                Err(_) => break,
            };
            if let Ok(list) = serde_json::from_value::<RawAlbumTracks>(page.clone()) {
                merge_isrcs(&mut raw_tracks, &list.data);
            }
            match page
                .get("next")
                .and_then(|n| n.as_str())
                .filter(|n| !n.is_empty())
            {
                Some(u) => next = u.to_string(),
                None => break,
            }
            if raw_tracks.iter().all(|t| t.isrc.is_some()) {
                break;
            }
        }
    }

    album_detail_from_raw(&raw, &raw_tracks)
}

/// Fill missing ISRCs from the album's tracklist, matching by Deezer track id
/// first and by `track_position` second.
fn merge_isrcs(local: &mut [RawAlbumTrack], incoming: &[RawAlbumTrack]) {
    for track in local.iter_mut() {
        if track.isrc.is_some() {
            continue;
        }
        let found = track
            .id
            .and_then(|id| incoming.iter().find(|i| i.id == Some(id)))
            .or_else(|| {
                track
                    .track_position
                    .and_then(|p| incoming.iter().find(|i| i.track_position == Some(p)))
            });
        if let Some(hit) = found {
            track.isrc = hit.isrc.clone();
        }
    }
}

/// `GET {base}/search/artist?q=&limit=` — artists matching a free-text query.
pub async fn search_artists(
    http: &reqwest::Client,
    base: &str,
    query: &str,
    limit: usize,
) -> Result<Vec<ArtistHit>, MetaError> {
    let url = format!("{base}/search/artist");
    let body = get_json(
        http,
        &url,
        &[
            ("q", query.to_string()),
            ("limit", limit.clamp(1, METADATA_MAX_LIMIT).to_string()),
        ],
    )
    .await?;
    map_search_artists(&body)
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
    fn parses_hifi_tier() {
        let json = serde_json::json!({
            "results": {
                "OFFER_NAME": "Deezer HiFi",
                "USER": {
                    "USER_ID": 42,
                    "OPTIONS": {
                        "web_sound_quality": { "lossless": true, "high": true, "low": true }
                    }
                }
            }
        });
        let tier = parse_user_data(&json).unwrap();
        assert_eq!(tier.offer, "Deezer HiFi");
        assert!(tier.lossless);
        assert!(tier.high);
        assert!(tier.low);
        assert_eq!(tier.user_id, Some(42));
        assert!(tier.can_stream_flac());
    }

    #[test]
    fn parses_free_tier_without_lossless() {
        let json = serde_json::json!({
            "results": {
                "OFFER_NAME": "Deezer Free",
                "USER": {
                    "USER_ID": 7,
                    "OPTIONS": {
                        "web_sound_quality": { "lossless": false, "high": false, "low": true }
                    }
                }
            }
        });
        let tier = parse_user_data(&json).unwrap();
        assert_eq!(tier.offer, "Deezer Free");
        assert!(!tier.can_stream_flac());
    }

    #[test]
    fn parses_missing_fields_with_defaults_without_panic() {
        let json = serde_json::json!({ "results": {} });
        let tier = parse_user_data(&json).unwrap();
        assert_eq!(tier.offer, "unknown");
        assert!(!tier.lossless);
        assert!(!tier.high);
        assert!(!tier.low);
        assert_eq!(tier.user_id, None);
        // No `results` envelope at all → no tier.
        assert!(parse_user_data(&serde_json::json!({})).is_none());
    }

    #[tokio::test]
    async fn empty_arl_skips_network() {
        // Points at a dead URL: the empty ARL must short-circuit before HTTP.
        let http = reqwest::Client::new();
        let tier = fetch_arl_tier(&http, "http://127.0.0.1:1", "   ")
            .await
            .unwrap();
        assert!(tier.is_none());
    }

    // ── Hub metadata mapping (fixtures shaped like the contract) ─────────────

    const ARTIST_ALBUMS_FIXTURE: &str = r#"{
        "data": [{
            "id": 1137915301,
            "title": "Folge 459: Die Robo-Ente (Das Original-Hörspiel zur TV-Serie)",
            "release_date": "2026-11-13",
            "record_type": "album",
            "cover": "https://cdn-images.dzcdn.net/images/cover/aaaa/250x250-000000-80-0-0.jpg",
            "cover_medium": "https://cdn-images.dzcdn.net/images/cover/aaaa/250x250-000000-80-0-0.jpg",
            "cover_big": "https://cdn-images.dzcdn.net/images/cover/bbbb/500x500-000000-80-0-0.jpg",
            "cover_xl": "https://cdn-images.dzcdn.net/images/cover/cccc/1000x1000-000000-80-0-0.jpg",
            "nb_tracks": 12
        }],
        "total": 417,
        "next": "https://api.deezer.com/artist/15167501/albums?limit=50&index=50"
    }"#;

    const ALBUM_FIXTURE: &str = r#"{
        "id": 1137915301,
        "title": "Folge 459: Die Robo-Ente (Das Original-Hörspiel zur TV-Serie)",
        "release_date": "2026-11-13",
        "record_type": "album",
        "cover_big": "https://cdn-images.dzcdn.net/images/cover/bbbb/500x500-000000-80-0-0.jpg",
        "cover_medium": "https://cdn-images.dzcdn.net/images/cover/aaaa/250x250-000000-80-0-0.jpg",
        "tracks": {
            "data": [
                {"id": 2, "title": "Kapitel 02: Die Ente fliegt", "duration": 60, "isrc": "CH7812690466", "track_position": 2},
                {"id": 1, "title": "Kapitel 01: Die Robo-Ente (Folge 459)", "duration": 98, "isrc": "CH7812690465", "track_position": 1},
                {"id": 3, "title": "Kapitel 03: Outro", "duration": 10, "track_position": 3}
            ]
        }
    }"#;

    const ARTIST_SEARCH_FIXTURE: &str = r#"{
        "data": [
            {"id": 15167501, "name": "Paw Patrol", "picture_medium": "https://api.deezer.com/artist/15167501/image"}
        ],
        "total": 1
    }"#;

    #[test]
    fn maps_artist_albums_into_the_contract_shape() {
        let body: serde_json::Value = serde_json::from_str(ARTIST_ALBUMS_FIXTURE).unwrap();
        let page = map_artist_albums(&body).unwrap();

        assert_eq!(page.total, 417);
        assert_eq!(
            page.next.as_deref(),
            Some("https://api.deezer.com/artist/15167501/albums?limit=50&index=50")
        );
        assert_eq!(page.albums.len(), 1);

        let album = &page.albums[0];
        assert_eq!(album.album_id, "1137915301");
        assert_eq!(
            album.title,
            "Folge 459: Die Robo-Ente (Das Original-Hörspiel zur TV-Serie)"
        );
        assert_eq!(album.release_date.as_deref(), Some("2026-11-13"));
        assert_eq!(album.record_type.as_deref(), Some("album"));
        // cover_big wins over cover_medium/cover_xl.
        assert_eq!(
            album.cover_url.as_deref(),
            Some("https://cdn-images.dzcdn.net/images/cover/bbbb/500x500-000000-80-0-0.jpg")
        );
        // Never taken from Deezer's payload, even though this fixture has it.
        assert_eq!(album.nb_tracks, None);

        // The serialized wire form uses camelCase and keeps `nbTracks`.
        let wire = serde_json::to_value(&page).unwrap();
        let item = &wire["albums"][0];
        assert_eq!(item["albumId"], "1137915301");
        assert_eq!(item["releaseDate"], "2026-11-13");
        assert_eq!(item["recordType"], "album");
        assert!(item["coverUrl"].is_string());
        assert!(item["nbTracks"].is_null());
        assert!(wire["next"].is_string());
    }

    #[test]
    fn artist_albums_cover_falls_back_medium_then_xl() {
        let only_medium: serde_json::Value = serde_json::json!({
            "data": [{
                "id": 7,
                "title": "Etazhi",
                "cover_medium": "https://cdn/250.jpg",
                "cover_xl": "https://cdn/1000.jpg"
            }],
            "total": 1
        });
        let page = map_artist_albums(&only_medium).unwrap();
        assert_eq!(
            page.albums[0].cover_url.as_deref(),
            Some("https://cdn/250.jpg")
        );
        // No `next` in the payload → stays `None` (serialized as null).
        assert!(page.next.is_none());

        let only_xl: serde_json::Value = serde_json::json!({
            "data": [{"id": 7, "title": "Etazhi", "cover_xl": "https://cdn/1000.jpg"}],
            "total": 1
        });
        let page = map_artist_albums(&only_xl).unwrap();
        assert_eq!(
            page.albums[0].cover_url.as_deref(),
            Some("https://cdn/1000.jpg")
        );

        let no_cover: serde_json::Value = serde_json::json!({
            "data": [{"id": 7, "title": "Etazhi", "cover_big": ""}],
            "total": 1
        });
        let page = map_artist_albums(&no_cover).unwrap();
        assert_eq!(page.albums[0].cover_url, None);
    }

    #[test]
    fn maps_album_detail_sums_seconds_and_sorts_by_position() {
        let body: serde_json::Value = serde_json::from_str(ALBUM_FIXTURE).unwrap();
        let album = map_album_detail(&body).unwrap();

        assert_eq!(album.album_id, "1137915301");
        assert_eq!(album.record_type.as_deref(), Some("album"));
        assert_eq!(album.release_date.as_deref(), Some("2026-11-13"));
        // 60 + 98 + 10 seconds.
        assert_eq!(album.seconds, 168);

        let positions: Vec<Option<i64>> = album.tracks.iter().map(|t| t.position).collect();
        assert_eq!(positions, vec![Some(1), Some(2), Some(3)]);
        assert_eq!(
            album.tracks[0].title,
            "Kapitel 01: Die Robo-Ente (Folge 459)"
        );
        assert_eq!(album.tracks[0].duration_ms, 98_000);
        assert_eq!(album.tracks[0].isrc.as_deref(), Some("CH7812690465"));
        // A track without an ISRC is kept, not filtered.
        assert_eq!(album.tracks[2].isrc, None);

        let wire = serde_json::to_value(&album).unwrap();
        assert_eq!(wire["albumId"], "1137915301");
        assert_eq!(wire["seconds"], 168);
        assert_eq!(wire["tracks"][0]["durationMs"], 98_000);
        assert_eq!(wire["tracks"][0]["position"], 1);
        assert!(wire["tracks"][2]["isrc"].is_null());
    }

    #[test]
    fn merge_isrcs_matches_by_id_then_by_position() {
        let mut local = vec![
            RawAlbumTrack {
                id: Some(10),
                isrc: None,
                title: "a".into(),
                duration: 1,
                track_position: Some(1),
            },
            RawAlbumTrack {
                id: Some(11),
                isrc: None,
                title: "b".into(),
                duration: 1,
                track_position: Some(2),
            },
            RawAlbumTrack {
                id: Some(12),
                isrc: Some("KEEP".into()),
                title: "c".into(),
                duration: 1,
                track_position: Some(3),
            },
        ];
        let incoming = vec![
            RawAlbumTrack {
                id: Some(11),
                isrc: Some("BY_ID".into()),
                title: "b".into(),
                duration: 1,
                track_position: Some(2),
            },
            RawAlbumTrack {
                id: Some(99),
                isrc: Some("BY_POS".into()),
                title: "a".into(),
                duration: 1,
                track_position: Some(1),
            },
        ];

        merge_isrcs(&mut local, &incoming);

        assert_eq!(local[0].isrc.as_deref(), Some("BY_POS"));
        assert_eq!(local[1].isrc.as_deref(), Some("BY_ID"));
        assert_eq!(local[2].isrc.as_deref(), Some("KEEP"));
    }

    #[test]
    fn album_detail_without_tracks_is_empty_not_an_error() {
        let body: serde_json::Value = serde_json::json!({"id": 5, "title": "Solo"});
        let album = map_album_detail(&body).unwrap();
        assert_eq!(album.seconds, 0);
        assert!(album.tracks.is_empty());
    }

    #[test]
    fn deezer_no_data_is_not_found() {
        let body: serde_json::Value = serde_json::from_str(
            r#"{"error":{"type":"DataException","message":"no data","code":800}}"#,
        )
        .unwrap();
        assert_eq!(map_album_detail(&body), Err(MetaError::NotFound));
        assert_eq!(map_artist_albums(&body), Err(MetaError::NotFound));
        assert_eq!(map_search_artists(&body), Err(MetaError::NotFound));
        // An album object without a usable id means the same thing.
        assert_eq!(
            map_album_detail(&serde_json::json!({"id": 0})),
            Err(MetaError::NotFound)
        );
    }

    #[test]
    fn deezer_quota_error_is_upstream_not_not_found() {
        let body: serde_json::Value = serde_json::from_str(
            r#"{"error":{"type":"Exception","message":"Quota limit exceeded","code":4}}"#,
        )
        .unwrap();
        match map_album_detail(&body) {
            Err(MetaError::Upstream(msg)) => assert!(msg.contains("code 4"), "msg was {msg}"),
            other => panic!("expected upstream error, got {other:?}"),
        }
    }

    #[test]
    fn maps_artist_search_into_the_contract_shape() {
        let body: serde_json::Value = serde_json::from_str(ARTIST_SEARCH_FIXTURE).unwrap();
        let artists = map_search_artists(&body).unwrap();
        assert_eq!(artists.len(), 1);
        assert_eq!(artists[0].artist_id, "15167501");
        assert_eq!(artists[0].name, "Paw Patrol");
        assert_eq!(
            artists[0].cover_url.as_deref(),
            Some("https://api.deezer.com/artist/15167501/image")
        );

        let wire = serde_json::to_value(&artists).unwrap();
        assert_eq!(wire[0]["artistId"], "15167501");
        assert!(wire[0]["coverUrl"].is_string());

        // An empty result set is `{ "artists": [] }`, not an error.
        let empty = map_search_artists(&serde_json::json!({"data": []})).unwrap();
        assert!(empty.is_empty());
    }
}
