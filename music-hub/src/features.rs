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

/// Inverse of [`camelot`]: Camelot notation -> `(pitch_class, mode)`
/// (`mode`: 1=major, 0=minor). `None` when the value is not canonical Camelot
/// (`1..=12` + `A`/`B`). Using the inverse tables guarantees
/// `camelot(pitch, mode)` round-trips to the input value.
///
/// Reference: `8A` -> `(9, 0)` (A minor); `8B` -> `(0, 1)` (C major).
pub fn camelot_to_pitch_mode(camelot: &str) -> Option<(i64, i64)> {
    const TABLE_MAJOR: [&str; 12] = [
        "8B", "3B", "10B", "5B", "12B", "7B", "2B", "9B", "4B", "11B", "6B", "1B",
    ];
    const TABLE_MINOR: [&str; 12] = [
        "5A", "12A", "7A", "2A", "9A", "4A", "11A", "6A", "1A", "8A", "3A", "10A",
    ];
    let s = camelot.trim().to_uppercase();
    let letter = s.chars().last()?;
    let (table, mode): (&[&str; 12], i64) = match letter {
        'A' => (&TABLE_MINOR, 0),
        'B' => (&TABLE_MAJOR, 1),
        _ => return None,
    };
    let num: i64 = s[..s.len() - 1].parse().ok()?;
    if !(1..=12).contains(&num) {
        return None;
    }
    let pitch = table.iter().position(|t| *t == s.as_str())? as i64;
    Some((pitch, mode))
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

/// High-priority ReccoBeats pass: process freshly requested tracks that have no
/// features row yet. Hits are stored and the request removed; misses are marked
/// `found = 0` but kept, so the (slow, paid) FreqBlog pass can pick them up.
pub async fn priority_once(pool: &SqlitePool, cfg: &Config, batch: i64) -> Result<usize> {
    let rows = sqlx::query(
        "SELECT r.track_id AS tid, e.external_id AS sid
           FROM hub_feature_requests r
           JOIN hub_track_external_ids e ON e.track_id = r.track_id AND e.service = 'spotify'
          WHERE e.external_id <> ''
            AND NOT EXISTS (SELECT 1 FROM hub_track_features f WHERE f.track_id = r.track_id)
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
        match found.get(sid) {
            Some(f) => {
                store(pool, *tid, Some(f)).await?;
                sqlx::query("DELETE FROM hub_feature_requests WHERE track_id = ?1")
                    .bind(tid)
                    .execute(pool)
                    .await?;
            }
            // ReccoBeats missed: mark tried, keep the request for FreqBlog.
            None => store(pool, *tid, None).await?,
        }
    }
    Ok(pairs.len())
}

/// Slow, paid FreqBlog pass for requests ReccoBeats already missed (`found = 0`).
/// Runs in its own loop because each lookup can take ~20s. Bounded by `batch`
/// and the monthly budget.
pub async fn freqblog_priority_once(pool: &SqlitePool, cfg: &Config, batch: i64) -> Result<usize> {
    if !crate::freqblog::enabled(cfg) {
        return Ok(0);
    }
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT r.track_id FROM hub_feature_requests r
           JOIN hub_track_features f ON f.track_id = r.track_id AND f.found = 0
          LIMIT ?1",
    )
    .bind(batch)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    if ids.is_empty() {
        return Ok(0);
    }

    let (processed, stopped) =
        crate::freqblog::enrich_tracks(pool, cfg, &ids, ids.len()).await?;
    // Only clear requests we actually handled; on a quota wall keep them for later.
    if !stopped {
        for id in &ids {
            sqlx::query("DELETE FROM hub_feature_requests WHERE track_id = ?1")
                .bind(id)
                .execute(pool)
                .await?;
        }
    }
    Ok(processed)
}

/// Enqueue a set of candidate tracks for on-demand feature lookup: only those
/// that actually have a Spotify id but no *usable* features yet (`found = 1`),
/// so tracks ReccoBeats already missed can still be retried via FreqBlog.
/// Returns how many were newly queued.
pub async fn enqueue_missing(pool: &SqlitePool, candidate_ids: &[i64]) -> Result<usize> {
    if candidate_ids.is_empty() {
        return Ok(0);
    }
    let now = now_iso();
    let mut queued = 0usize;
    for chunk in candidate_ids.chunks(900) {
        let mut qb = sqlx::QueryBuilder::new(
            "INSERT OR IGNORE INTO hub_feature_requests (track_id, requested_at)
             SELECT t.id, ",
        );
        qb.push_bind(&now);
        qb.push(
            " FROM hub_tracks t
               JOIN hub_track_external_ids e ON e.track_id = t.id
                    AND e.service = 'spotify' AND e.external_id <> ''
              WHERE t.id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(*id);
        }
        qb.push(")");
        qb.push(
            " AND NOT EXISTS (SELECT 1 FROM hub_track_features f\n                             WHERE f.track_id = t.id AND f.found = 1)",
        );
        let res = qb.build().execute(pool).await?;
        queued += res.rows_affected() as usize;
    }
    Ok(queued)
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

// ── music-api metadata backfill (ISRC → BPM/key authority) ──────────────────

/// How long a music-api "queried, no data" marker is trusted before re-checking.
const METADATA_RECHECK_DAYS: i64 = 7;
/// ISRCs sent per `GET /metadata?isrcs=` request.
const METADATA_API_BATCH: usize = 100;

/// Canonicalise a raw `camelot` value from music-api (`8A` / `8a`) to `8A`.
fn canonical_camelot(s: &str) -> Option<String> {
    let s = s.trim().to_uppercase();
    let letter = s.chars().last()?;
    if !matches!(letter, 'A' | 'B') {
        return None;
    }
    let num: i64 = s[..s.len() - 1].parse().ok()?;
    if (1..=12).contains(&num) {
        Some(s)
    } else {
        None
    }
}

/// Record a music-api "queried, no data" marker: only `metadata_checked_at`,
/// never existing values or `source`. Creates a `found = 0` row when absent.
async fn mark_metadata_checked(pool: &SqlitePool, track_id: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO hub_track_features (track_id, found, metadata_checked_at)
         VALUES (?1, 0, ?2)
         ON CONFLICT(track_id) DO UPDATE SET metadata_checked_at = excluded.metadata_checked_at",
    )
    .bind(track_id)
    .bind(now_iso())
    .execute(pool)
    .await?;
    Ok(())
}

/// UPSERT one music-api metadata row into `hub_track_features`. Returns whether
/// the row contributed data (BPM or a parseable key). Existing values are kept
/// via `COALESCE`; a row without usable values only records the check marker.
async fn store_metadata_row(
    pool: &SqlitePool,
    track_id: i64,
    row: &crate::music_api::MetadataRow,
) -> Result<bool> {
    let bpm = row.bpm.filter(|b| *b > 0.0);
    // Prefer a well-formed `camelot`; otherwise derive it from `musicalKey`.
    let camelot = row
        .camelot
        .as_deref()
        .and_then(canonical_camelot)
        .or_else(|| {
            row.musical_key()
                .and_then(crate::music_api::parse_musical_key)
        });
    let (key_pitch, key_mode) = match camelot.as_deref().and_then(camelot_to_pitch_mode) {
        Some((p, m)) => (Some(p), Some(m)),
        None => (None, None),
    };

    if bpm.is_none() && camelot.is_none() {
        // Nothing usable: record the check only (do not change `source`).
        mark_metadata_checked(pool, track_id).await?;
        return Ok(false);
    }

    sqlx::query(
        "INSERT INTO hub_track_features
             (track_id, found, isrc, bpm, key_pitch, key_mode, camelot, source,
              fetched_at, metadata_checked_at)
         VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, 'music-api', ?7, ?7)
         ON CONFLICT(track_id) DO UPDATE SET
             found = 1,
             isrc = COALESCE(excluded.isrc, hub_track_features.isrc),
             bpm = COALESCE(excluded.bpm, hub_track_features.bpm),
             key_pitch = COALESCE(excluded.key_pitch, hub_track_features.key_pitch),
             key_mode = COALESCE(excluded.key_mode, hub_track_features.key_mode),
             camelot = COALESCE(excluded.camelot, hub_track_features.camelot),
             source = 'music-api',
             fetched_at = excluded.fetched_at,
             metadata_checked_at = excluded.metadata_checked_at",
    )
    .bind(track_id)
    .bind(&row.isrc)
    .bind(bpm)
    .bind(key_pitch)
    .bind(key_mode)
    .bind(&camelot)
    .bind(now_iso())
    .execute(pool)
    .await?;
    Ok(true)
}

/// Backfill BPM/key into `hub_track_features` from music-api's ISRC authority
/// (`GET /metadata?isrcs=`). Selection is bounded (tracks with an ISRC and no
/// usable BPM/key, re-checked after [`METADATA_RECHECK_DAYS`]) and chunked; a
/// transient failure skips the chunk (`continue`) so a "no data" marker never
/// poisons a row. Returns the number of rows that contributed data.
pub async fn backfill_from_music_api(
    pool: &SqlitePool,
    cfg: &Config,
    limit: usize,
) -> Result<usize> {
    if cfg.music_api_token.is_none() || limit == 0 {
        return Ok(0);
    }
    let ttl = (chrono::Utc::now() - chrono::Duration::days(METADATA_RECHECK_DAYS)).to_rfc3339();
    let rows = sqlx::query(
        "SELECT t.id AS id, t.isrc AS isrc
           FROM hub_tracks t
           LEFT JOIN hub_track_features f ON f.track_id = t.id
          WHERE t.isrc IS NOT NULL AND TRIM(t.isrc) <> ''
            AND (f.track_id IS NULL
                 OR (f.bpm IS NULL AND f.camelot IS NULL
                     AND (f.metadata_checked_at IS NULL OR f.metadata_checked_at < ?1)))
          ORDER BY t.id
          LIMIT ?2",
    )
    .bind(&ttl)
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(0);
    }

    let mut by_isrc: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
    let mut isrcs: Vec<String> = Vec::new();
    for r in &rows {
        let isrc: String = r.get("isrc");
        by_isrc.insert(isrc.to_uppercase(), r.get("id"));
        isrcs.push(isrc);
    }

    let mut written = 0usize;
    for chunk in isrcs.chunks(METADATA_API_BATCH) {
        // A transient failure must not poison rows with a "no data" marker.
        let Some((items, _missing)) = crate::music_api::metadata_range(cfg, chunk).await else {
            continue;
        };
        let hit: std::collections::HashMap<String, &crate::music_api::MetadataRow> = items
            .iter()
            .map(|m| (m.isrc.to_uppercase(), m))
            .collect();
        for isrc in chunk {
            let key = isrc.to_uppercase();
            let Some(&tid) = by_isrc.get(&key) else {
                continue;
            };
            match hit.get(&key) {
                Some(row) => {
                    if store_metadata_row(pool, tid, row).await? {
                        written += 1;
                    }
                }
                // In `missing` (or not returned): mark the check, keep data.
                None => mark_metadata_checked(pool, tid).await?,
            }
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::{camelot, camelot_compatible, camelot_to_pitch_mode, key_name, spotify_id_from_href};

    #[test]
    fn camelot_inverse_matches_reference() {
        assert_eq!(camelot_to_pitch_mode("8A"), Some((9, 0))); // A minor
        assert_eq!(camelot_to_pitch_mode("8B"), Some((0, 1))); // C major
        assert_eq!(camelot_to_pitch_mode("8a"), Some((9, 0))); // case-insensitive
        assert_eq!(camelot_to_pitch_mode("13A"), None);
        assert_eq!(camelot_to_pitch_mode("8"), None);
        assert_eq!(camelot_to_pitch_mode(""), None);
    }

    #[test]
    fn camelot_inverse_round_trips_all_24_keys() {
        for pitch in 0..12 {
            for mode in [0, 1] {
                let c = camelot(pitch, mode);
                let (p, m) = camelot_to_pitch_mode(&c).expect("invertible");
                assert_eq!((p, m), (pitch, mode), "round-trip for {c}");
                assert_eq!(camelot(p, m), c);
            }
        }
    }

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
