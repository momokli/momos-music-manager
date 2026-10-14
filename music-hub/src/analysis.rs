//! `hub_track_audio` — the hub's flat read-model of resolved audio values.
//!
//! The hub **never analyses** and owns **no vault**. music-api is the Data Vault
//! of music: it resolves the "right" BPM/key per track by source precedence and
//! exposes it on the frozen consumer surface
//! `GET /vault/tracks?isrcs=a,b,c` →
//! `{"tracks":[{"isrc","bpm","key","camelot","energy","source","updatedAt"}]}`.
//!
//! [`refresh_from_vault`] pulls that resolved best value (batched, bounded) and
//! UPSERTs it into the flat read-model `hub_track_audio` — one row per track, no
//! per-source rows, no hash keys (that is the vault's job). The UI reads only
//! this read-model via [`get`] / [`get_many`].

use std::collections::HashMap;

use anyhow::Result;
use sqlx::{Row, SqlitePool};

use crate::config::Config;

/// How long a "queried, no data" row is trusted before re-checking (the vault
/// may fill it later, e.g. once the music-api analysis worker has run).
const RECHECK_DAYS: i64 = 7;
const CHUNK: usize = 400;
const VAULT_BATCH: usize = 100;

/// The resolved best audio value for one track (as resolved by the vault).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackAudio {
    pub bpm: Option<f64>,
    /// Musical key (e.g. `A-Minor` / `11m`), if the vault resolved one.
    pub key: Option<String>,
    /// Camelot notation (e.g. `8A`), if the vault resolved one.
    pub camelot: Option<String>,
    pub energy: Option<f64>,
    /// Source the vault resolved the value from (e.g. `traktor`, `essentia`).
    pub source: Option<String>,
}

impl TrackAudio {
    pub fn has_data(&self) -> bool {
        self.bpm.is_some() || self.key.is_some() || self.camelot.is_some()
    }
}

fn in_clause(n: usize) -> String {
    std::iter::repeat("?").take(n).collect::<Vec<_>>().join(",")
}

/// Read the read-model row for one track (`None` when absent).
pub async fn get(pool: &SqlitePool, track_id: i64) -> Option<TrackAudio> {
    let row = sqlx::query(
        "SELECT bpm, musical_key, camelot, energy, source FROM hub_track_audio WHERE track_id = ?1",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()?;
    Some(TrackAudio {
        bpm: row.get("bpm"),
        key: row.get("musical_key"),
        camelot: row.get("camelot"),
        energy: row.get("energy"),
        source: row.get("source"),
    })
}

/// Batched [`get`] over many tracks.
pub async fn get_many(pool: &SqlitePool, ids: &[i64]) -> HashMap<i64, TrackAudio> {
    let mut out = HashMap::new();
    for chunk in ids.chunks(CHUNK) {
        let sql = format!(
            "SELECT track_id, bpm, musical_key, camelot, energy, source \
               FROM hub_track_audio WHERE track_id IN ({})",
            in_clause(chunk.len())
        );
        let mut q = sqlx::query(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        for r in q.fetch_all(pool).await.unwrap_or_default() {
            out.insert(
                r.get("track_id"),
                TrackAudio {
                    bpm: r.get("bpm"),
                    key: r.get("musical_key"),
                    camelot: r.get("camelot"),
                    energy: r.get("energy"),
                    source: r.get("source"),
                },
            );
        }
    }
    out
}

/// Upsert the resolved best value for one track. Non-null incoming fields win;
/// nulls keep the existing value (so a later "no data" read never wipes real
/// data).
pub async fn upsert(
    pool: &SqlitePool,
    track_id: i64,
    bpm: Option<f64>,
    key: Option<&str>,
    camelot: Option<&str>,
    energy: Option<f64>,
    source: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO hub_track_audio
             (track_id, bpm, musical_key, camelot, energy, source, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(track_id) DO UPDATE SET
             bpm         = COALESCE(excluded.bpm, hub_track_audio.bpm),
             musical_key = COALESCE(excluded.musical_key, hub_track_audio.musical_key),
             camelot     = COALESCE(excluded.camelot, hub_track_audio.camelot),
             energy      = COALESCE(excluded.energy, hub_track_audio.energy),
             source      = COALESCE(excluded.source, hub_track_audio.source),
             updated_at  = excluded.updated_at",
    )
    .bind(track_id)
    .bind(bpm)
    .bind(key)
    .bind(camelot)
    .bind(energy)
    .bind(source)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// Pull the vault's resolved best BPM/key for up to `limit` tracks with an ISRC
/// that lack a (fresh) read-model row, and UPSERT into `hub_track_audio`.
/// Batched + bounded; a transient failure skips the batch (never poisons the
/// read-model with "no data" rows). Returns the number of rows written.
pub async fn refresh_from_vault(pool: &SqlitePool, cfg: &Config, limit: usize) -> Result<usize> {
    if cfg.music_api_token.is_none() || limit == 0 {
        return Ok(0);
    }
    let ttl = (chrono::Utc::now() - chrono::Duration::days(RECHECK_DAYS)).to_rfc3339();
    let rows = sqlx::query(
        "SELECT t.id AS id, t.isrc AS isrc
           FROM hub_tracks t
           LEFT JOIN hub_track_audio a ON a.track_id = t.id
          WHERE t.isrc IS NOT NULL AND TRIM(t.isrc) <> ''
            AND (a.track_id IS NULL
                 OR (a.bpm IS NULL AND a.musical_key IS NULL AND a.camelot IS NULL
                     AND (a.updated_at IS NULL OR a.updated_at < ?1)))
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

    // isrc (upper) -> track id, plus the ordered ISRC list to query.
    let mut by_isrc: HashMap<String, i64> = HashMap::new();
    let mut isrcs: Vec<String> = Vec::new();
    for r in &rows {
        let isrc: String = r.get("isrc");
        let id: i64 = r.get("id");
        by_isrc.insert(isrc.to_uppercase(), id);
        isrcs.push(isrc);
    }

    let mut written = 0usize;
    for chunk in isrcs.chunks(VAULT_BATCH) {
        // A transient failure must not poison the read-model with "no data" rows.
        let Some(items) = crate::music_api::vault_tracks(cfg, chunk).await else {
            continue;
        };
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for it in items {
            let isrc = it.get("isrc").and_then(|v| v.as_str()).unwrap_or("");
            let Some(&tid) = by_isrc.get(&isrc.to_uppercase()) else {
                continue;
            };
            seen.insert(isrc.to_uppercase());
            let bpm = it.get("bpm").and_then(|v| v.as_f64());
            let key = it.get("key").and_then(|v| v.as_str());
            let camelot = it.get("camelot").and_then(|v| v.as_str());
            let energy = it.get("energy").and_then(|v| v.as_f64());
            let source = it.get("source").and_then(|v| v.as_str());
            upsert(pool, tid, bpm, key, camelot, energy, source).await?;
            written += 1;
        }
        // Unknown ISRCs: record a "queried, no data" row so we don't re-query
        // every pass (re-checked after the TTL).
        for isrc in chunk {
            if seen.contains(&isrc.to_uppercase()) {
                continue;
            }
            if let Some(&tid) = by_isrc.get(&isrc.to_uppercase()) {
                upsert(pool, tid, None, None, None, None, None).await?;
            }
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_clause_builds_placeholders() {
        assert_eq!(in_clause(1), "?");
        assert_eq!(in_clause(3), "?,?,?");
    }

    #[test]
    fn has_data_requires_a_value() {
        assert!(!TrackAudio::default().has_data());
        assert!(
            TrackAudio {
                bpm: Some(120.0),
                ..Default::default()
            }
            .has_data()
        );
        assert!(
            TrackAudio {
                key: Some("11m".into()),
                ..Default::default()
            }
            .has_data()
        );
    }
}
