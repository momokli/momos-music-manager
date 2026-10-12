//! FreqBlog audio-features adapter (paid, ISRC/name lookup) with a strict
//! monthly quota guard.
//!
//! FreqBlog is Spotify's `audio-features` replacement (BPM, key + Camelot,
//! energy, valence, danceability, mood, genre). Its **free tier is 1,000
//! requests/month** — so this module never exceeds a configurable cap
//! (`freqblog_monthly_cap`, default 950) and only ever looks up tracks that
//! ReccoBeats (and the free sources) already missed. Callers must configure
//! `FREQBlog_API_KEY`; without it the adapter is a no-op.
//!
//! Auth is the `X-Api-Key` header (not Bearer). Billing notes we rely on:
//! `200`/`202` cost 1 request, a `404` (no match) is free, `429` is a quota /
//! rate wall. The counter we keep is therefore an *upper bound* on the real
//! usage, which keeps us safely under the free tier.

use anyhow::{Context, Result};
use serde::Deserialize;
use sqlx::{Row, SqlitePool};
use std::time::Duration;

use crate::config::Config;

/// Provider key used in `hub_api_usage`.
pub const PROVIDER: &str = "freqblog";

/// A parsed audio-feature row (the subset that maps onto `hub_track_features`).
#[derive(Debug, Clone, Default)]
pub struct Feature {
    pub isrc: Option<String>,
    pub bpm: Option<f64>,
    pub key_pitch: Option<i64>,
    pub key_mode: Option<i64>,
    pub camelot: Option<String>,
    pub energy: Option<f64>,
    pub danceability: Option<f64>,
    pub valence: Option<f64>,
    pub acousticness: Option<f64>,
    pub loudness: Option<f64>,
}

#[derive(Deserialize)]
struct Resp {
    #[serde(default)]
    isrc: Option<String>,
    #[serde(default)]
    bpm: Option<f64>,
    #[serde(default)]
    key_int: Option<i64>,
    #[serde(default)]
    mode: Option<i64>,
    #[serde(default)]
    camelot: Option<String>,
    #[serde(default)]
    energy: Option<f64>,
    #[serde(default)]
    danceability: Option<f64>,
    #[serde(default)]
    valence: Option<f64>,
    #[serde(default)]
    acousticness: Option<f64>,
    #[serde(default)]
    loudness_db: Option<f64>,
}

impl Resp {
    fn into_feature(self) -> Feature {
        let camelot = self.camelot.or_else(|| {
            // FreqBlog always sends `camelot`; derive it as a last resort.
            match (self.key_int, self.mode) {
                (Some(p), Some(m)) => Some(crate::features::camelot(p, m)),
                _ => None,
            }
        });
        Feature {
            isrc: self.isrc,
            bpm: self.bpm,
            key_pitch: self.key_int,
            key_mode: self.mode,
            camelot,
            energy: self.energy,
            danceability: self.danceability,
            valence: self.valence,
            acousticness: self.acousticness,
            loudness: self.loudness_db,
        }
    }
}

/// Outcome of a single lookup, so the caller can update the quota counter.
pub struct Lookup {
    pub feature: Option<Feature>,
    /// Quota requests this call is billed as (0 or 1).
    pub charged: i64,
    /// A hard stop (quota/rate wall, auth error): abort the whole backfill.
    pub stop: bool,
}

/// Is a FreqBlog API key configured?
pub fn enabled(cfg: &Config) -> bool {
    cfg.freqblog_api_key
        .as_deref()
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false)
}

/// Current month as `YYYY-MM` (UTC).
pub fn period_utc() -> String {
    chrono::Utc::now().format("%Y-%m").to_string()
}

/// Requests already spent this month for a provider.
pub async fn used(pool: &SqlitePool, provider: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "SELECT used FROM hub_api_usage WHERE provider = ?1 AND period = ?2",
    )
    .bind(provider)
    .bind(period_utc())
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .unwrap_or(0)
}

/// Add `n` to this month's counter for a provider.
pub async fn add_usage(pool: &SqlitePool, provider: &str, n: i64) -> Result<()> {
    if n <= 0 {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO hub_api_usage (provider, period, used, updated_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(provider, period) DO UPDATE SET
             used = used + excluded.used, updated_at = excluded.updated_at",
    )
    .bind(provider)
    .bind(period_utc())
    .bind(n)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// Remaining budget for this month (`cap - used`, floored at 0).
pub async fn remaining(pool: &SqlitePool, cfg: &Config) -> i64 {
    (cfg.freqblog_monthly_cap - used(pool, PROVIDER).await).max(0)
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("mmm-hub/0.9 (+https://hub.zukkafabrik.de)")
        .build()
        .context("build reqwest client")
}

/// Look up one track by ISRC (preferred) or by artist + title.
///
/// Adds `wait=20` so a track FreqBlog has to analyse on demand (`202`) is
/// usually returned inline (`200`) in the same request — one quota unit, one
/// round-trip.
pub async fn lookup(
    cfg: &Config,
    isrc: Option<&str>,
    artist: &str,
    title: &str,
) -> Result<Lookup> {
    let Some(key) = cfg.freqblog_api_key.as_deref().map(str::trim).filter(|k| !k.is_empty()) else {
        return Ok(Lookup { feature: None, charged: 0, stop: true });
    };
    let isrc = isrc.map(str::trim).filter(|s| !s.is_empty());

    let mut params: Vec<(&str, String)> = vec![("wait", "20".to_string())];
    match isrc {
        Some(code) => params.push(("isrc", code.to_string())),
        None => {
            params.push(("track", title.trim().to_string()));
            if !artist.trim().is_empty() {
                params.push(("artist", artist.trim().to_string()));
            }
        }
    }

    let url = format!("{}/lookup", cfg.freqblog_base.trim_end_matches('/'));
    let resp = client()?
        .get(&url)
        .header("X-Api-Key", key)
        .query(&params)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?;

    let status = resp.status().as_u16();
    match status {
        200 => {
            let body: Resp = resp.json().await.unwrap_or_else(|_| Resp {
                isrc: None,
                bpm: None,
                key_int: None,
                mode: None,
                camelot: None,
                energy: None,
                danceability: None,
                valence: None,
                acousticness: None,
                loudness_db: None,
            });
            Ok(Lookup { feature: Some(body.into_feature()), charged: 1, stop: false })
        }
        // 202 = queued for on-demand analysis; billed, retry later.
        202 => Ok(Lookup { feature: None, charged: 1, stop: false }),
        // 404 = no match anywhere (free). Mark as tried so we don't re-query.
        404 => Ok(Lookup { feature: None, charged: 0, stop: false }),
        // 429 = quota or concurrency wall; 401/403 = auth/tier. Stop.
        _ => Ok(Lookup { feature: None, charged: 0, stop: true }),
    }
}

/// Enrich one track by id if it still lacks features. Returns `None` when the
/// track doesn't exist or already has features (`found = 1`) — those cost
/// nothing. Otherwise performs the lookup, records quota and stores the result.
async fn enrich_one(pool: &SqlitePool, cfg: &Config, track_id: i64) -> Result<Option<Lookup>> {
    let row = sqlx::query(
        "SELECT t.isrc AS isrc, t.title AS title, t.artists AS artists,
                (f.track_id IS NOT NULL AND f.found = 1) AS have
           FROM hub_tracks t
           LEFT JOIN hub_track_features f ON f.track_id = t.id
          WHERE t.id = ?1",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let Some(r) = row else {
        return Ok(None);
    };
    if r.get::<i64, _>("have") == 1 {
        return Ok(None);
    }
    let isrc = r.get::<Option<String>, _>("isrc").unwrap_or_default();
    let title = r.get::<Option<String>, _>("title").unwrap_or_default();
    let artists = r.get::<Option<String>, _>("artists").unwrap_or_default();
    let isrc = isrc.trim();

    let outcome = lookup(cfg, (!isrc.is_empty()).then_some(isrc), &artists, &title).await?;
    add_usage(pool, PROVIDER, outcome.charged).await?;
    if let Some(f) = &outcome.feature {
        store_found(pool, track_id, f).await?;
    } else if !outcome.stop {
        // 404 (no match anywhere) — record as tried so we don't spend budget again.
        store_missing(pool, track_id).await?;
    }
    Ok(Some(outcome))
}

/// Enrich an explicit set of tracks (manual digging), bounded by `limit` and the
/// remaining monthly budget. Tracks that already have features are skipped free
/// of charge. Returns `(billed_tracks, stopped)`.
pub async fn enrich_tracks(
    pool: &SqlitePool,
    cfg: &Config,
    ids: &[i64],
    limit: usize,
) -> Result<(usize, bool)> {
    if !enabled(cfg) {
        return Ok((0, true));
    }
    let mut processed = 0usize;
    for &id in ids {
        if processed >= limit {
            break;
        }
        if remaining(pool, cfg).await <= 0 {
            return Ok((processed, true));
        }
        match enrich_one(pool, cfg, id).await? {
            None => continue,
            Some(o) => {
                processed += 1;
                if o.stop {
                    return Ok((processed, true));
                }
            }
        }
    }
    Ok((processed, false))
}

/// Automatic backfill batch over the library, bounded by the monthly budget.
/// Kept for one-off/manual use — the app does **not** schedule it; FreqBlog is
/// reserved for manual digging sessions. Returns `(processed, exhausted)`.
pub async fn backfill(pool: &SqlitePool, cfg: &Config, limit: usize) -> Result<(usize, bool)> {
    if !enabled(cfg) {
        return Ok((0, true));
    }
    let budget = remaining(pool, cfg).await;
    if budget <= 0 {
        return Ok((0, true));
    }
    let take = (limit as i64).min(budget);

    // Tracks with an ISRC and no usable features yet: either never tried, or
    // tried by ReccoBeats and missed (`found = 0`).
    let rows = sqlx::query(
        "SELECT t.id AS tid
           FROM hub_tracks t
           LEFT JOIN hub_track_features f ON f.track_id = t.id
          WHERE t.isrc IS NOT NULL AND trim(t.isrc) <> ''
            AND (f.track_id IS NULL OR f.found = 0)
          ORDER BY t.id
          LIMIT ?1",
    )
    .bind(take)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let ids: Vec<i64> = rows.iter().map(|r| r.get::<i64, _>("tid")).collect();
    if ids.is_empty() {
        return Ok((0, true));
    }
    let (processed, stopped) = enrich_tracks(pool, cfg, &ids, take as usize).await?;
    Ok((processed, stopped || processed == 0))
}

async fn store_found(pool: &SqlitePool, track_id: i64, f: &Feature) -> Result<()> {
    sqlx::query(
        "INSERT INTO hub_track_features
             (track_id, found, isrc, bpm, key_pitch, key_mode, camelot, energy, danceability,
              valence, acousticness, loudness, source, fetched_at)
         VALUES (?1,1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,'freqblog',?12)
         ON CONFLICT(track_id) DO UPDATE SET
             found=1, isrc=COALESCE(excluded.isrc, hub_track_features.isrc),
             bpm=excluded.bpm, key_pitch=excluded.key_pitch, key_mode=excluded.key_mode,
             camelot=excluded.camelot, energy=excluded.energy,
             danceability=excluded.danceability, valence=excluded.valence,
             acousticness=excluded.acousticness, loudness=excluded.loudness,
             source='freqblog', fetched_at=excluded.fetched_at",
    )
    .bind(track_id)
    .bind(&f.isrc)
    .bind(f.bpm)
    .bind(f.key_pitch)
    .bind(f.key_mode)
    .bind(&f.camelot)
    .bind(f.energy)
    .bind(f.danceability)
    .bind(f.valence)
    .bind(f.acousticness)
    .bind(f.loudness)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

async fn store_missing(pool: &SqlitePool, track_id: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO hub_track_features (track_id, found, source, fetched_at)
         VALUES (?1, 0, 'freqblog', ?2)
         ON CONFLICT(track_id) DO UPDATE SET found=0, source='freqblog', fetched_at=excluded.fetched_at",
    )
    .bind(track_id)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lookup_response() {
        let json = r#"{
            "track_name":"Blinding Lights","artist_name":"The Weeknd",
            "isrc":"USUMV2403154","bpm":85.39,"key":"F-Minor","key_int":5,"mode":0,
            "camelot":"4A","energy":1.0,"danceability":0.7036,"valence":0.4224,
            "acousticness":0.03,"loudness_db":-10.96,"mood":"tense","genre":"synthwave"
        }"#;
        let f = serde_json::from_str::<Resp>(json).unwrap().into_feature();
        assert_eq!(f.bpm, Some(85.39));
        assert_eq!(f.key_pitch, Some(5));
        assert_eq!(f.key_mode, Some(0));
        assert_eq!(f.camelot.as_deref(), Some("4A"));
        assert_eq!(f.isrc.as_deref(), Some("USUMV2403154"));
    }

    #[test]
    fn derives_camelot_when_missing() {
        let r = Resp {
            isrc: None,
            bpm: None,
            key_int: Some(0),
            mode: Some(1),
            camelot: None,
            energy: None,
            danceability: None,
            valence: None,
            acousticness: None,
            loudness_db: None,
        };
        // C major -> 8B on the Camelot wheel.
        assert_eq!(r.into_feature().camelot.as_deref(), Some("8B"));
    }

    #[test]
    fn disabled_without_key() {
        let cfg = Config::for_test("sqlite::memory:");
        assert!(!enabled(&cfg));
        assert_eq!(cfg.freqblog_monthly_cap, 950);
    }

    #[tokio::test]
    async fn quota_counter_and_remaining() {
        let dir = tempfile::tempdir().unwrap();
        let url = format!("sqlite:{}", dir.path().join("t.db").display());
        let pool = crate::db::connect(&url).await.unwrap();
        let mut cfg = Config::for_test(&url);
        cfg.freqblog_api_key = Some("k".into());

        assert_eq!(remaining(&pool, &cfg).await, 950);
        add_usage(&pool, PROVIDER, 3).await.unwrap();
        assert_eq!(used(&pool, PROVIDER).await, 3);
        assert_eq!(remaining(&pool, &cfg).await, 947);

        // Cap is configurable; remaining floors at 0.
        cfg.freqblog_monthly_cap = 2;
        assert_eq!(remaining(&pool, &cfg).await, 0);
    }
}
