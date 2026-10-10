//! Per-track audio analysis worker: fetch FLAC from `music-api`, compute the
//! EffNet embedding (feature `effnet`) and/or local BPM/key via the analyzer
//! service, and store both. Used by the CLI and the background worker.

use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};

use crate::config::Config;

pub const MODEL: &str = "discogs-effnet";

/// What `analyze_track` managed to produce.
#[derive(Debug, Default, Clone, Copy)]
pub struct Outcome {
    pub embedding: bool,
    pub bpm_key: bool,
}

/// Analyze a single track. Skips work that's already done; returns which parts
/// were produced. Errors if the FLAC can't be fetched (not yet available etc.).
pub async fn analyze_track(pool: &SqlitePool, cfg: &Config, track_id: i64) -> Result<Outcome> {
    let row = sqlx::query("SELECT isrc FROM hub_tracks WHERE id = ?1")
        .bind(track_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let Some(row) = row else {
        return Ok(Outcome::default());
    };
    let isrc = row
        .get::<Option<String>, _>("isrc")
        .unwrap_or_default()
        .trim()
        .to_string();
    if isrc.is_empty() {
        return Ok(Outcome::default());
    }

    let has_emb = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM hub_track_embeddings WHERE track_id = ?1",
    )
    .bind(track_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .is_some();
    let has_an =
        sqlx::query_scalar::<_, i64>("SELECT 1 FROM hub_track_analysis WHERE track_id = ?1")
            .bind(track_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .is_some();

    // Embeddings: in-process (Rust/ort) where the CPU supports it, otherwise via
    // the analyzer service (Essentia), which also does BPM/key.
    let inproc = cfg.effnet_inprocess && crate::audio::available() && cfg.effnet_model.is_some();
    let service = crate::analyzer::enabled(cfg);
    let want_emb_inproc = !has_emb && inproc;
    let want_emb_service = !has_emb && !inproc && service;
    let want_an = !has_an && service;
    if !want_emb_inproc && !want_emb_service && !want_an {
        return Ok(Outcome::default());
    }

    // Fetch the FLAC to a temp file (music-api serves it by ISRC). Use a shared
    // dir when configured so the analyzer service (which may have a private
    // /tmp) can read it.
    let base = cfg
        .analyze_tmp_dir
        .clone()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("mmm-hub-analyze-{track_id}"));
    std::fs::create_dir_all(&dir).context("create temp dir")?;
    let audio_path = dir.join("audio.flac");
    let bytes = crate::music_api::file_bytes(cfg, &isrc, "flac")
        .await
        .with_context(|| format!("fetch flac for isrc {isrc}"))?;
    std::fs::write(&audio_path, &bytes).context("write temp flac")?;

    let mut out = Outcome::default();

    if want_emb_inproc {
        let model = std::path::PathBuf::from(cfg.effnet_model.clone().unwrap());
        let labels = cfg.effnet_labels.clone().map(std::path::PathBuf::from);
        let audio = audio_path.clone();
        let emb = tokio::task::spawn_blocking(move || {
            crate::audio::embed_file(&model, labels.as_deref(), &audio)
        })
        .await
        .context("join embed task")??;
        crate::similar::store(pool, track_id, MODEL, &emb.vec, false).await?;
        out.embedding = true;
    }

    if want_an || want_emb_service {
        let path_str = audio_path.to_string_lossy().to_string();
        if let Some(a) = crate::analyzer::analyze(cfg, &path_str, want_emb_service).await? {
            if want_an {
                store_analysis(pool, track_id, &a).await?;
                out.bpm_key = true;
            }
            if want_emb_service {
                if let Some(emb) = &a.embedding {
                    crate::similar::store(pool, track_id, MODEL, emb, false).await?;
                    out.embedding = true;
                }
            }
        }
    }

    let _ = std::fs::remove_dir_all(&dir);
    Ok(out)
}

/// Upsert local BPM/key analysis.
pub async fn store_analysis(
    pool: &SqlitePool,
    track_id: i64,
    a: &crate::analyzer::Analysis,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO hub_track_analysis (track_id, bpm, key, camelot, source, analyzed_at)
         VALUES (?1, ?2, ?3, ?4, 'essentia', ?5)
         ON CONFLICT(track_id) DO UPDATE SET
             bpm=excluded.bpm, key=excluded.key, camelot=excluded.camelot,
             source=excluded.source, analyzed_at=excluded.analyzed_at",
    )
    .bind(track_id)
    .bind(a.bpm)
    .bind(&a.key)
    .bind(&a.camelot)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

/// How many tracks still need embedding and/or analysis (rough).
pub async fn pending_counts(pool: &SqlitePool) -> (i64, i64) {
    let emb = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM hub_tracks t
           LEFT JOIN hub_track_embeddings e ON e.track_id = t.id
          WHERE t.isrc IS NOT NULL AND trim(t.isrc) <> '' AND e.track_id IS NULL",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    let an = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM hub_tracks t
           LEFT JOIN hub_track_analysis a ON a.track_id = t.id
          WHERE t.isrc IS NOT NULL AND trim(t.isrc) <> '' AND a.track_id IS NULL",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    (emb, an)
}
