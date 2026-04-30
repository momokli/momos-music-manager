use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Pool, Row, Sqlite};
use std::sync::Arc;

use crate::db::File;

// ============================================================================
// Camelot Wheel
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CamelotKey {
    pub position: u8, // 1-12
    pub mode: char,   // 'A' or 'B'
}

pub fn parse_camelot_key(s: &str) -> Option<CamelotKey> {
    // Parse "8A", "8B", "12A", "1B", etc.
    // At least 2 chars: number then A/B
    if s.len() < 2 {
        return None;
    }
    let mode = s.chars().last()?;
    if mode != 'A' && mode != 'B' {
        return None;
    }
    let num_str = &s[..s.len() - 1];
    let position: u8 = num_str.parse().ok()?;
    if position < 1 || position > 12 {
        return None;
    }
    Some(CamelotKey { position, mode })
}

/// Check if two keys are compatible given the set of active jump types.
/// jump_types: set of strings like "+1", "-1", "+2", "-2", "+7", "-7", "a_to_b", "same"
pub fn are_keys_compatible(from: CamelotKey, to: CamelotKey, active_jumps: &[String]) -> bool {
    if from == to && active_jumps.contains(&"same".to_string()) {
        return true;
    }

    // Same position, mode change (A↔B / d↔m)
    if from.position == to.position
        && from.mode != to.mode
        && active_jumps.contains(&"a_to_b".to_string())
    {
        return true;
    }

    if from.mode != to.mode {
        return false; // Mode+position change not supported
    }

    let diff = if to.position >= from.position {
        to.position - from.position
    } else {
        to.position + 12 - from.position
    };

    match diff {
        1 => active_jumps.contains(&"+1".to_string()),
        11 => active_jumps.contains(&"-1".to_string()), // -1 ≡ +11 mod 12
        2 => active_jumps.contains(&"+2".to_string()),
        10 => active_jumps.contains(&"-2".to_string()), // -2 ≡ +10 mod 12
        7 => active_jumps.contains(&"+7".to_string()),
        5 => active_jumps.contains(&"-7".to_string()), // -7 ≡ +5 mod 12
        _ => false,
    }
}

// ============================================================================
// API Types
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingSeedQuery {
    pub limit: Option<i64>,
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    pub key: Option<String>,
    pub search: Option<String>,
    pub genre: Option<String>,
    // Sorting
    pub sort_by: Option<String>, // "play_count", "last_played", "bpm", "title", "rating", "random"
    pub sort_order: Option<String>, // "asc" or "desc"
    // Play count filter
    pub play_count_max: Option<i32>,
    pub play_count_min: Option<i32>,
    // Last played filter (unix timestamp, files with last_played < this or NULL)
    pub last_played_before: Option<i64>,
    pub last_played_after: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionRequest {
    pub seed_track_id: i64,
    pub active_jumps: Vec<String>, // ["+1", "-1", "+2", "a_to_b", "same", ...]
    pub bpm_range: Option<f64>,    // default 8.0 (±bpm_range)
    pub limit: Option<i64>,        // default 20
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoredTrack {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub genre: Option<String>,
    pub play_count: i32,
    pub last_played: Option<i64>,
    pub duration_ms: Option<i64>,
    pub camelot_compatibility: String, // "perfect" (exact match), "good" (A↔B), "ok" (any other)
    pub bpm_diff: Option<f64>,
    pub score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveChainRequest {
    pub tag_name: String,      // e.g., "digging-01"
    pub track_ids: Vec<i64>,   // File IDs in chain order
    pub comment_updates: bool, // whether to write tag into file comments
}

// ============================================================================
// Tag Energy Level
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct TagEnergyLevel {
    pub tag_id: i64,
    pub energy_level: i32,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagWithEnergy {
    pub tag_id: i64,
    pub tag_name: String,
    pub category_name: String,
    pub energy_level: Option<i32>,
    pub sort_order: i64,
}

// ============================================================================
// Suggestion Engine
// ============================================================================

pub async fn get_seeds(pool: &Pool<Sqlite>, query: &DiggingSeedQuery) -> Result<Vec<File>> {
    let limit = query.limit.unwrap_or(20).min(50);

    // Build SQL with ? bind parameters instead of string interpolation.
    // Each filter uses (? IS NULL OR condition) — when the bound value is
    // NULL (i.e. the user didn't supply that filter), the IS NULL check
    // makes the whole clause a no-op, so we can always bind all parameters
    // without conditional branches.
    let sort_column = match query.sort_by.as_deref().unwrap_or("play_count") {
        "last_played" => "COALESCE(last_played, 0)",
        "bpm" => "COALESCE(bpm, 0)",
        "title" => "COALESCE(title, '')",
        "rating" => "rating",
        "random" => "RANDOM()",
        _ => "play_count",
    };
    let order = match query.sort_order.as_deref().unwrap_or("asc") {
        "desc" => "DESC",
        _ => "ASC",
    };

    let sql = format!(
        "SELECT * FROM files WHERE 1=1 \
         AND (? IS NULL OR bpm >= ?) \
         AND (? IS NULL OR bpm <= ?) \
         AND (? IS NULL OR musical_key = ?) \
         AND (? IS NULL OR (title LIKE ('%' || ? || '%') OR artist LIKE ('%' || ? || '%') OR file_path LIKE ('%' || ? || '%'))) \
         AND (? IS NULL OR genre LIKE ('%' || ? || '%')) \
         AND (? IS NULL OR play_count <= ?) \
         AND (? IS NULL OR play_count >= ?) \
         AND (? IS NULL OR last_played IS NULL OR last_played < ?) \
         AND (? IS NULL OR (last_played IS NOT NULL AND last_played > ?)) \
         ORDER BY {} {} LIMIT ?",
        sort_column, order,
    );

    let files = sqlx::query_as::<_, File>(&sql)
        .bind(query.bpm_min)
        .bind(query.bpm_min) // bpm >=
        .bind(query.bpm_max)
        .bind(query.bpm_max) // bpm <=
        .bind(&query.key)
        .bind(&query.key) // musical_key =
        .bind(&query.search)
        .bind(&query.search) // search guard + title
        .bind(&query.search)
        .bind(&query.search) // artist + file_path
        .bind(&query.genre)
        .bind(&query.genre) // genre LIKE
        .bind(query.play_count_max)
        .bind(query.play_count_max) // play_count <=
        .bind(query.play_count_min)
        .bind(query.play_count_min) // play_count >=
        .bind(query.last_played_before)
        .bind(query.last_played_before) // last_played <
        .bind(query.last_played_after)
        .bind(query.last_played_after) // last_played >
        .bind(limit)
        .fetch_all(pool)
        .await?;

    Ok(files)
}

pub async fn get_suggestions(
    pool: &Pool<Sqlite>,
    req: &SuggestionRequest,
) -> Result<Vec<ScoredTrack>> {
    let limit = req.limit.unwrap_or(20).min(50);
    let bpm_range = req.bpm_range.unwrap_or(8.0);

    // Fetch seed track
    let seed = sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = ?")
        .bind(req.seed_track_id)
        .fetch_one(pool)
        .await?;

    let seed_bpm = seed.bpm.unwrap_or(120.0);
    let seed_key = seed.musical_key.as_deref().and_then(parse_camelot_key);

    let bpm_min = seed_bpm - bpm_range;
    let bpm_max = seed_bpm + bpm_range;

    // Query files in BPM range (excluding seed track)
    let files = sqlx::query_as::<_, File>(
        "SELECT * FROM files WHERE id != ? AND bpm >= ? AND bpm <= ? ORDER BY play_count ASC, COALESCE(last_played, 0) ASC LIMIT ?"
    )
    .bind(req.seed_track_id)
    .bind(bpm_min)
    .bind(bpm_max)
    .bind(limit * 3)  // Fetch extra for scoring
    .fetch_all(pool)
    .await?;

    let mut scored: Vec<ScoredTrack> = Vec::new();

    for file in files {
        let file_key = file.musical_key.as_deref().and_then(parse_camelot_key);
        let bpm_diff = file.bpm.map(|b| (b - seed_bpm).abs());

        // Camelot compatibility
        let camelot_compatibility = match (seed_key, file_key) {
            (Some(s), Some(f)) => {
                if are_keys_compatible(s, f, &req.active_jumps) {
                    if s == f {
                        "perfect"
                    } else if s.position == f.position && s.mode != f.mode {
                        "good"
                    } else {
                        "ok"
                    }
                } else {
                    continue; // Skip incompatible tracks
                }
            }
            _ => "unknown",
        };

        // Score: lower is better
        let mut score = 0.0;

        // Play count (lower = better, max 100 plays)
        score += (file.play_count as f64).min(100.0) * 2.0;

        // Last played (older = better, recent = worse)
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        if let Some(lp) = file.last_played {
            let days_since = (now - lp) / 86400;
            score += (1000.0 - (days_since as f64).min(1000.0)) * 0.5;
        } else {
            // Never played = best
            score -= 50.0;
        }

        // BPM difference (closer = better)
        if let Some(diff) = bpm_diff {
            score += diff * 1.5;
        }

        // Camelot match bonus
        match camelot_compatibility {
            "perfect" => score -= 30.0,
            "good" => score -= 15.0,
            _ => {}
        }

        scored.push(ScoredTrack {
            id: file.id,
            title: file.title.clone().unwrap_or_default(),
            artist: file.artist.clone().unwrap_or_default(),
            bpm: file.bpm,
            musical_key: file.musical_key.clone(),
            genre: file.genre.clone(),
            play_count: file.play_count,
            last_played: file.last_played,
            duration_ms: file.duration_ms,
            camelot_compatibility: camelot_compatibility.to_string(),
            bpm_diff,
            score,
        });
    }

    // Sort by score ascending, take top N
    scored.sort_by(|a, b| {
        a.score
            .partial_cmp(&b.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(limit as usize);

    Ok(scored)
}

pub async fn save_chain(
    pool: &Pool<Sqlite>,
    task_manager: &crate::tasks::TaskManager,
    req: &SaveChainRequest,
) -> Result<String> {
    // 1. Create or find the tag
    // Check if tag exists
    let existing = sqlx::query_scalar::<_, i64>("SELECT id FROM tags WHERE name = ?")
        .bind(&req.tag_name)
        .fetch_optional(pool)
        .await?;

    let tag_id = match existing {
        Some(id) => id,
        None => {
            // Find the Setlist category (is_default = TRUE)
            let cat = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM tag_categories WHERE is_default = TRUE LIMIT 1",
            )
            .fetch_one(pool)
            .await?;

            // Create tag
            sqlx::query("INSERT INTO tags (name, category_id) VALUES (?, ?)")
                .bind(&req.tag_name)
                .bind(cat)
                .execute(pool)
                .await?;

            sqlx::query_scalar::<_, i64>("SELECT id FROM tags WHERE name = ?")
                .bind(&req.tag_name)
                .fetch_one(pool)
                .await?
        }
    };

    // 2. Write comments if requested (pass all file IDs at once)
    if req.comment_updates {
        crate::tasks::start_write_comment_task(task_manager, pool, req.track_ids.clone()).await;
    }

    Ok(format!(
        "Chain saved as tag '{}' (id={})",
        req.tag_name, tag_id
    ))
}

// ============================================================================
// Tag Energy Level functions
// ============================================================================

pub async fn get_tag_energy_levels(pool: &Pool<Sqlite>) -> Result<Vec<TagWithEnergy>> {
    let rows = sqlx::query(
        r#"SELECT t.id as tag_id, t.name as tag_name, tc.name as category_name, tel.energy_level, t.sort_order
           FROM tags t
           JOIN tag_categories tc ON tc.id = t.category_id
           LEFT JOIN tag_energy_levels tel ON tel.tag_id = t.id
           WHERE tc.prefix = 'P'
           ORDER BY t.sort_order, t.name"#,
    )
    .fetch_all(pool)
    .await?;

    let result = rows
        .iter()
        .map(|row| TagWithEnergy {
            tag_id: row.get("tag_id"),
            tag_name: row.get("tag_name"),
            category_name: row.get("category_name"),
            energy_level: row.get("energy_level"),
            sort_order: row.get("sort_order"),
        })
        .collect();

    Ok(result)
}

pub async fn set_tag_energy_level(
    pool: &Pool<Sqlite>,
    tag_id: i64,
    energy_level: i32,
) -> Result<()> {
    sqlx::query(
        r#"INSERT INTO tag_energy_levels (tag_id, energy_level, created_at)
           VALUES (?, ?, unixepoch())
           ON CONFLICT(tag_id) DO UPDATE SET
               energy_level = excluded.energy_level"#,
    )
    .bind(tag_id)
    .bind(energy_level)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn delete_tag_energy_level(pool: &Pool<Sqlite>, tag_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM tag_energy_levels WHERE tag_id = ?")
        .bind(tag_id)
        .execute(pool)
        .await?;

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagReorderItem {
    pub tag_id: i64,
    pub energy_level: i32,
    pub sort_order: i64,
}

pub async fn reorder_tags_batch(pool: &Pool<Sqlite>, items: &[TagReorderItem]) -> Result<()> {
    let mut tx = pool.begin().await?;

    for item in items {
        // Update sort_order on tags table
        sqlx::query("UPDATE tags SET sort_order = ? WHERE id = ?")
            .bind(item.sort_order)
            .bind(item.tag_id)
            .execute(&mut *tx)
            .await?;

        // Upsert energy_level in tag_energy_levels table
        sqlx::query(
            r#"INSERT INTO tag_energy_levels (tag_id, energy_level, created_at)
               VALUES (?, ?, unixepoch())
               ON CONFLICT(tag_id) DO UPDATE SET
                   energy_level = excluded.energy_level"#,
        )
        .bind(item.tag_id)
        .bind(item.energy_level)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}
