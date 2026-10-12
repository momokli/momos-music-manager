//! Brute-force cosine similarity over `hub_track_embeddings`.
//!
//! 1280 × f32 = 5 KB/track; a linear scan is fine up to ~100k tracks (and the
//! hub's library is far smaller). Swap in `sqlite-vec` if it ever isn't.

use anyhow::Result;
use sqlx::{Row, SqlitePool};

#[derive(Debug, Clone, Copy)]
pub struct Neighbor {
    pub track_id: i64,
    pub score: f32,
}

fn blob_to_f32(b: &[u8]) -> Vec<f32> {
    b.chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

pub fn f32_to_blob(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for &x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn norm(a: &[f32]) -> f32 {
    dot(a, a).sqrt()
}

/// Store (upsert) one track's embedding.
pub async fn store(
    pool: &SqlitePool,
    track_id: i64,
    model: &str,
    embedding: &[f32],
    from_preview: bool,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO hub_track_embeddings (track_id, model, dims, embedding, from_preview, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)
         ON CONFLICT(track_id) DO UPDATE SET
             model=excluded.model, dims=excluded.dims, embedding=excluded.embedding,
             from_preview=excluded.from_preview, created_at=excluded.created_at",
    )
    .bind(track_id)
    .bind(model)
    .bind(embedding.len() as i64)
    .bind(f32_to_blob(embedding))
    .bind(from_preview as i64)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM hub_track_embeddings")
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

/// Nearest neighbours of a seed track by cosine similarity (excludes the seed).
pub async fn neighbors(pool: &SqlitePool, seed_track_id: i64, limit: i64) -> Result<Vec<Neighbor>> {
    let seed: Option<Vec<u8>> =
        sqlx::query_scalar("SELECT embedding FROM hub_track_embeddings WHERE track_id = ?1")
            .bind(seed_track_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    let Some(seed_blob) = seed else {
        return Ok(Vec::new());
    };
    let sv = blob_to_f32(&seed_blob);
    let sn = norm(&sv);
    if sn == 0.0 {
        return Ok(Vec::new());
    }

    let rows = sqlx::query(
        "SELECT track_id, embedding FROM hub_track_embeddings WHERE track_id <> ?1",
    )
    .bind(seed_track_id)
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut out: Vec<Neighbor> = rows
        .iter()
        .filter_map(|r| {
            let v = blob_to_f32(&r.get::<Vec<u8>, _>("embedding"));
            if v.len() != sv.len() {
                return None;
            }
            let denom = sn * norm(&v);
            if denom <= 0.0 {
                return None;
            }
            Some(Neighbor {
                track_id: r.get::<i64, _>("track_id"),
                score: dot(&sv, &v) / denom,
            })
        })
        .collect();
    out.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    out.truncate(limit.max(0) as usize);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_of_identical_is_one() {
        let a = vec![1.0f32, 2.0, 3.0];
        assert!((dot(&a, &a) / (norm(&a) * norm(&a)) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn blob_roundtrip() {
        let v = vec![1.5f32, -2.25, 0.0];
        assert_eq!(blob_to_f32(&f32_to_blob(&v)), v);
    }
}
