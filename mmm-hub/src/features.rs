//! Audio features via ReccoBeats (free, no auth) — Spotify's `audio-features`
//! replacement. Bulk lookup by Spotify track id; cached in `hub_track_features`.

use anyhow::{Context, Result};
use serde::Deserialize;
use sqlx::{Row, SqlitePool};

use crate::config::Config;

#[derive(Debug, Clone)]
pub struct Feature {
    /// Spotify track id (from the response `href`).
    pub spotify_id: String,
    pub reccobeats_id: Option<String>,
    pub isrc: Option<String>,
    pub bpm: Option<f64>,
    pub key_pitch: Option<i64>,
    pub key_mode: Option<i64>,
    pub energy: Option<f64>,
    pub danceability: Option<f64>,
    pub valence: Option<f64>,
    pub acousticness: Option<f64>,
    pub instrumentalness: Option<f64>,
    pub liveness: Option<f64>,
    pub loudness: Option<f64>,
    pub speechiness: Option<f64>,
}

#[derive(Deserialize)]
struct Resp {
    #[serde(default)]
    content: Vec<Item>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    id: Option<String>,
    href: Option<String>,
    isrc: Option<String>,
    tempo: Option<f64>,
    key: Option<i64>,
    mode: Option<i64>,
    energy: Option<f64>,
    danceability: Option<f64>,
    valence: Option<f64>,
    acousticness: Option<f64>,
    instrumentalness: Option<f64>,
    liveness: Option<f64>,
    loudness: Option<f64>,
    speechiness: Option<f64>,
}

/// Extract the Spotify track id from an `https://open.spotify.com/track/<id>` href.
fn spotify_id_from_href(href: &str) -> Option<String> {
    href.rsplit('/').next().map(|s| s.to_string())
}

/// `(pitch_class, mode)` -> Camelot wheel notation (e.g. `8B`). `mode`: 1=major, 0=minor.
pub fn camelot(pitch: i64, mode: i64) -> String {
    let p = pitch.rem_euclid(12) as usize;
    let major = mode == 1;
    let table_major = ["8B", "3B", "10B", "5B", "12B", "7B", "2B", "9B", "4B", "11B", "6B", "1B"];
    let table_minor = ["5A", "12A", "7A", "2A", "9A", "4A", "11A", "6A", "1A", "8A", "3A", "10A"];
    (if major { table_major[p] } else { table_minor[p] }).to_string()
}

/// Pitch class -> note name (sharps), for display.
pub fn key_name(pitch: i64) -> &'static str {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    NAMES[pitch.rem_euclid(12) as usize]
}

/// Parse Camelot notation like `8B` into `(number, letter)`.
fn parse_camelot(s: &str) -> Option<(i32, char)> {
    let s = s.trim();
    if s.len() < 2 {
        return None;
    }
    let letter = s.chars().last()?;
    let num: i32 = s[..s.len() - 1].parse().ok()?;
    if !(1..=12).contains(&num) {
        return None;
    }
    Some((num, letter))
}

/// Harmonic compatibility on the Camelot wheel: same key, the relative
/// major/minor (same number, other letter), or a neighbour (+/-1, same letter).
pub fn camelot_compatible(a: &str, b: &str) -> bool {
    let ((an, al), (bn, bl)) = match (parse_camelot(a), parse_camelot(b)) {
        (Some(x), Some(y)) => (x, y),
        _ => return false,
    };
    if an == bn && al != bl {
        return true; // relative major/minor
    }
    let d = (an - bn).rem_euclid(12);
    (d == 0 || d == 1 || d == 11) && al == bl
}

/// The bulk adapter for ReccoBeats audio features.
#[allow(dead_code)]
pub async fn fetch_batch(cfg: &Config, spotify_ids: &[String]) -> Result<Vec<Feature>> {
    if spotify_ids.is_empty() {
        return Ok(Vec::new());
    }
    let url = format!(
        "{}/audio-features?ids={}",
        cfg.reccobeats_base,
        spotify_ids.join(",")
    );
    let resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let body: Resp = resp.json().await.unwrap_or(Resp { content: vec![] });

    Ok(body
        .content
        .into_iter()
        .filter_map(|i| {
            let spotify_id = i.href.as_deref().and_then(spotify_id_from_href)?;
            Some(Feature {
                spotify_id,
                reccobeats_id: i.id,
                isrc: i.isrc,
                bpm: i.tempo,
                key_pitch: i.key,
                key_mode: i.mode,
                energy: i.energy,
                danceability: i.danceability,
                valence: i.valence,
                acousticness: i.acousticness,
                instrumentalness: i.instrumentalness,
                liveness: i.liveness,
                loudness: i.loudness,
                speechiness: i.speechiness,
            })
        })
        .collect())
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Upsert one track's features. `None` marks the track as tried-but-unavailable
/// (`found = 0`) so we don't look it up again.
pub async fn store(pool: &SqlitePool, track_id: i64, f: Option<&Feature>) -> Result<()> {
    match f {
        None => {
            sqlx::query(
                "INSERT INTO hub_track_features (track_id, found, source, fetched_at)
                 VALUES (?1, 0, 'reccobeats', ?2)
                 ON CONFLICT(track_id) DO UPDATE SET found = 0, fetched_at = excluded.fetched_at",
            )
            .bind(track_id)
            .bind(now_iso())
            .execute(pool)
            .await?;
        }
        Some(f) => {
            let camelot = match (f.key_pitch, f.key_mode) {
                (Some(p), Some(m)) => Some(camelot(p, m)),
                _ => None,
            };
            sqlx::query(
                "INSERT INTO hub_track_features
                     (track_id, found, reccobeats_id, isrc, bpm, key_pitch, key_mode, camelot,
                      energy, danceability, valence, acousticness, instrumentalness, liveness,
                      loudness, speechiness, source, fetched_at)
                 VALUES (?1,1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,'reccobeats',?16)
                 ON CONFLICT(track_id) DO UPDATE SET
                     found=1, reccobeats_id=excluded.reccobeats_id, isrc=excluded.isrc,
                     bpm=excluded.bpm, key_pitch=excluded.key_pitch, key_mode=excluded.key_mode,
                     camelot=excluded.camelot, energy=excluded.energy,
                     danceability=excluded.danceability, valence=excluded.valence,
                     acousticness=excluded.acousticness,
                     instrumentalness=excluded.instrumentalness, liveness=excluded.liveness,
                     loudness=excluded.loudness, speechiness=excluded.speechiness,
                     fetched_at=excluded.fetched_at",
            )
            .bind(track_id)
            .bind(&f.reccobeats_id)
            .bind(&f.isrc)
            .bind(f.bpm)
            .bind(f.key_pitch)
            .bind(f.key_mode)
            .bind(camelot)
            .bind(f.energy)
            .bind(f.danceability)
            .bind(f.valence)
            .bind(f.acousticness)
            .bind(f.instrumentalness)
            .bind(f.liveness)
            .bind(f.loudness)
            .bind(f.speechiness)
            .bind(now_iso())
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

/// One batch: look up up-to-`batch` tracks that have a Spotify id but no
/// features row yet. Returns the number of tracks processed (0 = nothing left).
pub async fn sync_once(pool: &SqlitePool, cfg: &Config, batch: i64) -> Result<usize> {
    let rows = sqlx::query(
        "SELECT t.id AS tid, e.external_id AS sid
           FROM hub_tracks t
           JOIN hub_track_external_ids e ON e.track_id = t.id AND e.service = 'spotify'
           LEFT JOIN hub_track_features f ON f.track_id = t.id
          WHERE f.track_id IS NULL AND e.external_id <> ''
          LIMIT ?1",
    )
    .bind(batch)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    if rows.is_empty() {
        return Ok(0);
    }

    let pairs: Vec<(i64, String)> = rows
        .iter()
        .map(|r| (r.get::<i64, _>("tid"), r.get::<String, _>("sid")))
        .collect();
    let ids: Vec<String> = pairs.iter().map(|(_, s)| s.clone()).collect();

    let feats = fetch_batch(cfg, &ids).await?;
    let found: std::collections::HashMap<String, &Feature> =
        feats.iter().map(|f| (f.spotify_id.clone(), f)).collect();

    for (tid, sid) in &pairs {
        store(pool, *tid, found.get(sid).copied()).await?;
    }
    Ok(pairs.len())
}

#[derive(Debug, Clone)]
pub struct Recommendation {
    pub title: String,
    pub artists: String,
    pub spotify_id: String,
}

#[derive(Deserialize)]
struct RecResp {
    #[serde(default)]
    content: Vec<RecItem>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecItem {
    track_title: Option<String>,
    #[serde(default)]
    artists: Vec<RecArtist>,
    href: Option<String>,
}

#[derive(Deserialize)]
struct RecArtist {
    name: Option<String>,
}

/// ReccoBeats track recommendations from one or more seed Spotify ids (no auth).
pub async fn recommendations(
    cfg: &Config,
    seed_spotify_id: &str,
    size: usize,
) -> Result<Vec<Recommendation>> {
    if seed_spotify_id.is_empty() {
        return Ok(Vec::new());
    }
    let url = format!(
        "{}/track/recommendation?seeds={}&size={}",
        cfg.reccobeats_base, seed_spotify_id, size
    );
    let resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;
    let body: RecResp = resp.json().await.unwrap_or(RecResp { content: vec![] });
    Ok(body
        .content
        .into_iter()
        .map(|i| Recommendation {
            title: i.track_title.unwrap_or_default(),
            artists: i
                .artists
                .iter()
                .filter_map(|a| a.name.clone())
                .collect::<Vec<_>>()
                .join(", "),
            spotify_id: i.href.as_deref().and_then(spotify_id_from_href).unwrap_or_default(),
        })
        .collect())
}

/// Backfill loop for the CLI: run batches until `max_tracks` reached or done.
/// Returns `(processed, exhausted)`.
pub async fn backfill(pool: &SqlitePool, cfg: &Config, max_tracks: usize) -> Result<(usize, bool)> {
    let mut done = 0usize;
    while done < max_tracks {
        let n = sync_once(pool, cfg, 40).await?;
        if n == 0 {
            return Ok((done, true));
        }
        done += n;
    }
    Ok((done, false))
}

#[cfg(test)]
mod tests {
    use super::{camelot, camelot_compatible, key_name, spotify_id_from_href};

    #[test]
    fn camelot_wheel_matches_reference() {
        // Major
        assert_eq!(camelot(0, 1), "8B"); // C major
        assert_eq!(camelot(7, 1), "9B"); // G major
        assert_eq!(camelot(11, 1), "1B"); // B major
        // Minor
        assert_eq!(camelot(0, 0), "5A"); // C minor
        assert_eq!(camelot(9, 0), "8A"); // A minor
        assert_eq!(camelot(11, 0), "10A"); // B minor
    }

    #[test]
    fn key_names() {
        assert_eq!(key_name(0), "C");
        assert_eq!(key_name(7), "G");
        assert_eq!(key_name(11), "B");
    }

    #[test]
    fn camelot_compatibility() {
        assert!(camelot_compatible("8B", "8B")); // same
        assert!(camelot_compatible("8B", "8A")); // relative
        assert!(camelot_compatible("8B", "9B")); // +1
        assert!(camelot_compatible("1B", "12B")); // wrap
        assert!(!camelot_compatible("8B", "10B")); // too far
        assert!(!camelot_compatible("8B", "9A")); // +1 but other letter
        assert!(!camelot_compatible("", "8B"));
    }

    #[test]
    fn href_extracts_spotify_id() {
        assert_eq!(
            spotify_id_from_href("https://open.spotify.com/track/abc123").as_deref(),
            Some("abc123")
        );
    }
}
