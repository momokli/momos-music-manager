use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Pool, Row, Sqlite};

use crate::db::File;

// ============================================================================
// Camelot Wheel
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub struct CamelotKey {
    pub position: u8, // 1-12
    pub mode: char,   // 'A' or 'B'
}

#[allow(dead_code)]
pub fn parse_camelot_key(s: &str) -> Option<CamelotKey> {
    // Parse "8A", "8B", "12A", "1B", etc.
    // At least 2 chars: number then A/B
    if s.len() < 2 {
        return None;
    }
    let mode = s.chars().last()?.to_ascii_uppercase();
    let mode = match mode {
        'A' | 'M' => 'A', // 'm' = minor = 'A' on Camelot wheel
        'B' | 'D' => 'B', // 'd' = major = 'B' on Camelot wheel
        _ => return None,
    };
    let num_str = &s[..s.len() - 1];
    let position: u8 = num_str.parse().ok()?;
    if !(1..=12).contains(&position) {
        return None;
    }
    Some(CamelotKey { position, mode })
}

/// Check if two keys are compatible given the set of active jump types.
/// jump_types: set of strings like "+1", "-1", "+2", "-2", "+7", "-7", "a_to_b", "same"
#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

#[allow(dead_code)]
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

// ============================================================================
// Multi-Seed Suggestion Engine
// ============================================================================

/// Request to find tracks similar to a set of seed files.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingSuggestRequest {
    pub seed_file_ids: Option<Vec<i64>>,
    pub seed_tag: Option<String>,
    pub bpm_range: Option<f64>,
    pub camelot_jumps: Option<Vec<String>>,
    pub limit: Option<i64>,
    pub dedup_by_isrc: Option<bool>,
    /// Boost tracks that are well-tagged across many categories (Phase, Mood, Vibe, Merkmal).
    pub prefer_tag_richness: Option<bool>,
}

/// A resolved seed file with its tags.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingSeed {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub genre: Option<String>,
    pub isrc: Option<String>,
    pub file_path: String,
    pub file_type: String,
    pub play_count: i32,
    pub last_played: Option<i64>,
    pub duration_ms: Option<i64>,
    pub tags: Vec<DiggingTag>,
    pub excluded_as_outlier: bool,
}

/// A tag on a file, with category context.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingTag {
    pub id: i64,
    pub name: String,
    pub category_name: String,
    pub prefix: String,
}

/// A scored suggestion for a candidate file.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingSuggestion {
    pub file_id: i64,
    pub title: String,
    pub artist: String,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub genre: Option<String>,
    pub isrc: Option<String>,
    pub file_path: String,
    pub file_type: String,
    pub play_count: i32,
    pub last_played: Option<i64>,
    pub duration_ms: Option<i64>,
    pub matching_seed_id: i64,
    pub camelot_compatibility: String,
    pub bpm_diff: Option<f64>,
    pub shared_tags: Vec<String>,
    /// All tags on this file, not just shared ones.
    pub all_tags: Vec<DiggingTag>,
    /// Computed energy level (average of Phase tag energy levels).
    pub energy_level: Option<f64>,
    pub score_breakdown: ScoreBreakdown,
    pub score: f64,
}

/// Breakdown of how the suggestion score was computed.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreBreakdown {
    pub play_count_score: f64,
    pub recency_score: f64,
    pub bpm_score: f64,
    pub camelot_bonus: f64,
    pub tag_match_bonus: f64,
    /// Bonus for tracks with tags across many distinct categories.
    pub tag_richness_bonus: f64,
    /// Bonus for tracks sharing categories with the matched seed.
    pub category_overlap_bonus: f64,
    /// Score penalty for energy mismatch with target (ladder mode).
    #[serde(default)]
    pub energy_match_score: f64,
}

/// Response from the multi-seed suggestion engine.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingSuggestResponse {
    pub seeds: Vec<DiggingSeed>,
    pub bpm_min: f64,
    pub bpm_max: f64,
    pub suggestions: Vec<DiggingSuggestion>,
    pub candidates_considered: usize,
}

/// When deduplicating by ISRC, prefer formats that play in browsers.
/// stem.m4a > mp3 > flac > wav > aiff > other
fn audio_format_preference(file_type: &str) -> u8 {
    match file_type.to_lowercase().as_str() {
        "stem.m4a" | "m4a" => 0,
        "mp3" | "mpeg" => 1,
        "flac" => 2,
        "wav" | "wave" => 3,
        "aif" | "aiff" => 4,
        _ => 5,
    }
}

/// Load resolved tags (with category name + prefix) for a file from `file_resolved_tags`.
async fn load_file_tags(pool: &Pool<Sqlite>, file_id: i64) -> Result<Vec<DiggingTag>> {
    let rows = sqlx::query(
        r#"SELECT DISTINCT tag_id, tag_name, category_name, prefix
           FROM file_resolved_tags
           WHERE file_id = ?
           ORDER BY tag_name"#,
    )
    .bind(file_id)
    .fetch_all(pool)
    .await?;

    let tags = rows
        .iter()
        .map(|row| DiggingTag {
            id: row.get("tag_id"),
            name: row.get("tag_name"),
            category_name: row.get("category_name"),
            prefix: row.get("prefix"),
        })
        .collect();

    Ok(tags)
}

/// Compute a file's energy level by averaging the energy levels of its Phase tags.
/// Returns None if the file has no Phase tags with defined energy levels.
pub async fn compute_track_energy(pool: &Pool<Sqlite>, file_id: i64) -> Result<Option<f64>> {
    // Use file_resolved_tags to pick up parent-resolved Phase tags
    let row = sqlx::query(
        r#"SELECT AVG(CAST(tel.energy_level AS REAL)) as avg_energy
           FROM (
               SELECT DISTINCT frt.tag_id
               FROM file_resolved_tags frt
               JOIN tag_energy_levels tel ON tel.tag_id = frt.tag_id
               WHERE frt.file_id = ?
           ) distinct_tags
           JOIN tag_energy_levels tel ON tel.tag_id = distinct_tags.tag_id"#,
    )
    .bind(file_id)
    .fetch_one(pool)
    .await?;

    let avg: Option<f64> = row.try_get("avg_energy").ok().flatten();
    Ok(avg)
}

/// Deduplicate suggestions by ISRC, keeping the lowest-scoring entry per ISRC.
/// On score tie, prefer formats that play in browsers via `audio_format_preference`.
/// NULL ISRC values are treated as unique (never deduplicated).
fn dedup_suggestions(suggestions: Vec<DiggingSuggestion>) -> Vec<DiggingSuggestion> {
    let mut seen: std::collections::HashMap<Option<String>, DiggingSuggestion> =
        std::collections::HashMap::new();

    for s in suggestions {
        if s.isrc.is_none() {
            // NULL ISRC: always unique, always keep
            seen.insert(Some(format!("__null_{}", s.file_id)), s);
            continue;
        }

        let key = s.isrc.clone();
        match seen.get(&key) {
            None => {
                seen.insert(key, s);
            }
            Some(existing) => {
                // Keep the one with lower score; on tie, prefer browser-playable format
                let keep = if s.score < existing.score {
                    true
                } else if (s.score - existing.score).abs() < f64::EPSILON {
                    audio_format_preference(&s.file_type)
                        < audio_format_preference(&existing.file_type)
                } else {
                    false
                };
                if keep {
                    seen.insert(key, s);
                }
            }
        }
    }

    seen.into_values().collect()
}

/// Main multi-seed suggestion engine.
///
/// Given seed files (by tag name or direct file IDs), find similar tracks from the
/// local library using Camelot harmonic mixing + BPM proximity, scored and ranked.
pub async fn get_multi_seed_suggestions(
    pool: &Pool<Sqlite>,
    req: &DiggingSuggestRequest,
) -> Result<DiggingSuggestResponse> {
    // ---- Defaults ----
    let bpm_range = req.bpm_range.unwrap_or(8.0).clamp(1.0, 30.0);
    let limit = (req.limit.unwrap_or(20) as usize).clamp(1, 50);
    let dedup = req.dedup_by_isrc.unwrap_or(true);
    let prefer_tag_richness = req.prefer_tag_richness.unwrap_or(false);

    let active_jumps: Vec<String> = if let Some(ref jumps) = req.camelot_jumps {
        if jumps.is_empty() {
            vec![
                "+1".to_string(),
                "-1".to_string(),
                "+2".to_string(),
                "-2".to_string(),
                "+7".to_string(),
                "-7".to_string(),
                "a_to_b".to_string(),
                "same".to_string(),
            ]
        } else {
            jumps.clone()
        }
    } else {
        vec![
            "+1".to_string(),
            "-1".to_string(),
            "+2".to_string(),
            "-2".to_string(),
            "+7".to_string(),
            "-7".to_string(),
            "a_to_b".to_string(),
            "same".to_string(),
        ]
    };

    // ---- Step 1: Resolve seed file IDs ----
    let seed_ids: Vec<i64> = if let Some(ref tag) = req.seed_tag {
        sqlx::query_scalar::<_, i64>(
            "SELECT DISTINCT file_id FROM file_resolved_tags WHERE LOWER(tag_name) = LOWER(?)",
        )
        .bind(tag)
        .fetch_all(pool)
        .await?
    } else if let Some(ref ids) = req.seed_file_ids {
        ids.clone()
    } else {
        anyhow::bail!("Either seed_file_ids or seed_tag must be provided");
    };

    if seed_ids.is_empty() {
        anyhow::bail!("No seed files found");
    }

    // Load seed files (using json_each to avoid dynamic bind count)
    let seed_ids_json = serde_json::to_string(&seed_ids)?;
    let seed_files: Vec<File> = sqlx::query_as::<_, File>(
        "SELECT * FROM files WHERE id IN (SELECT value FROM json_each(?))",
    )
    .bind(&seed_ids_json)
    .fetch_all(pool)
    .await?;

    if seed_files.is_empty() {
        anyhow::bail!("No seed files found in database");
    }

    // Load tags for each seed file
    let mut seeds: Vec<DiggingSeed> = Vec::with_capacity(seed_files.len());
    for file in seed_files {
        let tags = load_file_tags(pool, file.id).await?;
        seeds.push(DiggingSeed {
            id: file.id,
            title: file.title.unwrap_or_default(),
            artist: file.artist.unwrap_or_default(),
            bpm: file.bpm,
            musical_key: file.musical_key.clone(),
            genre: file.genre.clone(),
            isrc: file.isrc.clone(),
            file_path: file.file_path,
            file_type: file.file_type,
            play_count: file.play_count,
            last_played: file.last_played,
            duration_ms: file.duration_ms,
            tags,
            excluded_as_outlier: false,
        });
    }

    // ---- Step 2: Outlier detection ----
    let bpms: Vec<f64> = seeds.iter().filter_map(|s| s.bpm).collect();
    let median_bpm = if bpms.is_empty() {
        0.0
    } else {
        let mut sorted = bpms.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mid = sorted.len() / 2;
        if sorted.len() % 2 == 0 {
            (sorted[mid - 1] + sorted[mid]) / 2.0
        } else {
            sorted[mid]
        }
    };

    // Valid seeds: not an outlier, have both BPM and musical_key
    let valid_seed_ids: Vec<i64> = seeds
        .iter()
        .filter(|s| {
            s.bpm.is_some_and(|bpm| (bpm - median_bpm).abs() <= 15.0) && s.musical_key.is_some()
        })
        .map(|s| s.id)
        .collect();

    // Mark outliers in the seeds vec
    for seed in &mut seeds {
        if !valid_seed_ids.contains(&seed.id) {
            seed.excluded_as_outlier = true;
        }
    }

    if valid_seed_ids.is_empty() {
        anyhow::bail!("No valid seeds with both BPM and musical_key available");
    }

    // ---- Step 3: BPM range ----
    let valid_bpms: Vec<f64> = seeds
        .iter()
        .filter(|s| valid_seed_ids.contains(&s.id))
        .filter_map(|s| s.bpm)
        .collect();

    let bpm_min_seed = valid_bpms.iter().cloned().fold(f64::MAX, f64::min);
    let bpm_max_seed = valid_bpms.iter().cloned().fold(f64::MIN, f64::max);
    let bpm_query_min = bpm_min_seed - bpm_range;
    let bpm_query_max = bpm_max_seed + bpm_range;

    // ---- Step 4: Candidate query ----
    let candidate_pool_size = limit * 5;

    let candidates: Vec<File> = sqlx::query_as::<_, File>(
        "SELECT * FROM files \
         WHERE id NOT IN (SELECT value FROM json_each(?)) \
           AND bpm IS NOT NULL \
           AND musical_key IS NOT NULL \
           AND bpm >= ? AND bpm <= ? \
         ORDER BY play_count ASC, COALESCE(last_played, 0) ASC \
         LIMIT ?",
    )
    .bind(&seed_ids_json)
    .bind(bpm_query_min)
    .bind(bpm_query_max)
    .bind(candidate_pool_size as i64)
    .fetch_all(pool)
    .await?;

    let candidates_considered = candidates.len();

    // ---- Step 5: Camelot filtering + scoring ----
    let mut suggestions: Vec<DiggingSuggestion> = Vec::with_capacity(candidates.len());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    for candidate in candidates {
        let candidate_key = candidate.musical_key.as_deref().and_then(parse_camelot_key);
        let candidate_tags = load_file_tags(pool, candidate.id).await?;
        let candidate_tag_names: std::collections::HashSet<&str> =
            candidate_tags.iter().map(|t| t.name.as_str()).collect();

        let mut best_seed_id: Option<i64> = None;
        let mut best_compat: Option<String> = None;
        let mut best_bpm_diff: Option<f64> = None;
        let mut best_shared: Vec<String> = Vec::new();
        let mut best_score = f64::MAX;
        let mut best_breakdown: Option<ScoreBreakdown> = None;

        for valid_id in &valid_seed_ids {
            let seed = seeds.iter().find(|s| s.id == *valid_id).unwrap();
            let seed_key = seed.musical_key.as_deref().and_then(parse_camelot_key);
            let seed_bpm = seed.bpm.unwrap_or(120.0);

            // Check Camelot compatibility
            let compats = match (seed_key, candidate_key) {
                (Some(sk), Some(ck)) => {
                    if are_keys_compatible(sk, ck, &active_jumps) {
                        if sk == ck {
                            Some("perfect")
                        } else if sk.position == ck.position && sk.mode != ck.mode {
                            Some("good")
                        } else {
                            Some("ok")
                        }
                    } else {
                        None
                    }
                }
                _ => None,
            };

            if let Some(compat_str) = compats {
                let bpm_diff = candidate.bpm.map(|b| (b - seed_bpm).abs());

                // Score components
                let play_count_score = (candidate.play_count as f64).min(100.0) * 2.0;

                let recency_score = if let Some(lp) = candidate.last_played {
                    let days_since = (now - lp) / 86400;
                    (1000.0 - (days_since as f64).min(1000.0)) * 0.5
                } else {
                    -50.0
                };

                let bpm_score = bpm_diff.map(|d| d * 1.5).unwrap_or(0.0);

                let camelot_bonus = match compat_str {
                    "perfect" => -30.0,
                    "good" => -15.0,
                    _ => 0.0,
                };

                // Shared tags: intersect seed tags with candidate tags
                let seed_tag_names: std::collections::HashSet<&str> =
                    seed.tags.iter().map(|t| t.name.as_str()).collect();
                let shared: Vec<String> = seed_tag_names
                    .intersection(&candidate_tag_names)
                    .map(|s| s.to_string())
                    .collect();
                let tag_match_bonus = -(shared.len() as f64) * 5.0;

                // Tag richness & category overlap bonuses
                let (tag_richness_bonus, category_overlap_bonus) = if prefer_tag_richness {
                    // 1. Tag richness: how many distinct categories does this candidate have?
                    let candidate_categories: std::collections::HashSet<&str> = candidate_tags
                        .iter()
                        .map(|t| t.category_name.as_str())
                        .collect();
                    let richness = -(candidate_categories.len() as f64) * 8.0;

                    // 2. Category overlap: how many categories are shared with this seed?
                    let seed_categories: std::collections::HashSet<&str> =
                        seed.tags.iter().map(|t| t.category_name.as_str()).collect();
                    let shared = seed_categories.intersection(&candidate_categories).count();
                    let overlap = -(shared as f64) * 5.0;

                    (richness, overlap)
                } else {
                    (0.0, 0.0)
                };

                let total_score = play_count_score
                    + recency_score
                    + bpm_score
                    + camelot_bonus
                    + tag_match_bonus
                    + tag_richness_bonus
                    + category_overlap_bonus;

                // Determine if this seed is a better match than the current best
                let is_better = match best_seed_id {
                    None => true,
                    Some(_) => {
                        let compat_priority = |c: &str| match c {
                            "perfect" => 0,
                            "good" => 1,
                            _ => 2,
                        };
                        let curr = compat_priority(compat_str);
                        let best = compat_priority(best_compat.as_deref().unwrap_or("ok"));
                        if curr < best {
                            true
                        } else if curr > best {
                            false
                        } else {
                            // Same priority: smaller BPM diff wins
                            let cur_diff = bpm_diff.unwrap_or(f64::MAX);
                            let bes_diff = best_bpm_diff.unwrap_or(f64::MAX);
                            cur_diff < bes_diff
                        }
                    }
                };

                if is_better {
                    best_seed_id = Some(seed.id);
                    best_compat = Some(compat_str.to_string());
                    best_bpm_diff = bpm_diff;
                    best_shared = shared;
                    best_score = total_score;
                    best_breakdown = Some(ScoreBreakdown {
                        play_count_score,
                        recency_score,
                        bpm_score,
                        camelot_bonus,
                        tag_match_bonus,
                        tag_richness_bonus,
                        category_overlap_bonus,
                        energy_match_score: 0.0,
                    });
                }
            }
        }

        if let (Some(seed_id), Some(compat), Some(breakdown)) =
            (best_seed_id, best_compat, best_breakdown)
        {
            // Compute energy for this candidate
            let candidate_energy = compute_track_energy(pool, candidate.id)
                .await
                .unwrap_or(None);

            suggestions.push(DiggingSuggestion {
                file_id: candidate.id,
                title: candidate.title.unwrap_or_default(),
                artist: candidate.artist.unwrap_or_default(),
                bpm: candidate.bpm,
                musical_key: candidate.musical_key.clone(),
                genre: candidate.genre.clone(),
                isrc: candidate.isrc.clone(),
                file_path: candidate.file_path,
                file_type: candidate.file_type,
                play_count: candidate.play_count,
                last_played: candidate.last_played,
                duration_ms: candidate.duration_ms,
                matching_seed_id: seed_id,
                camelot_compatibility: compat,
                bpm_diff: best_bpm_diff,
                shared_tags: best_shared,
                all_tags: candidate_tags.clone(),
                energy_level: candidate_energy,
                score_breakdown: breakdown,
                score: best_score,
            });
        }
    }

    // ---- Step 6: ISRC dedup ----
    if dedup {
        suggestions = dedup_suggestions(suggestions);
    }

    // ---- Step 7: Sort + limit ----
    suggestions.sort_by(|a, b| {
        a.score
            .partial_cmp(&b.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    suggestions.truncate(limit);

    Ok(DiggingSuggestResponse {
        seeds,
        bpm_min: bpm_query_min,
        bpm_max: bpm_query_max,
        suggestions,
        candidates_considered,
    })
}

// ============================================================================
// Unified Search
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingSearchQuery {
    pub q: String,
    pub file_limit: Option<i64>,
    pub tag_limit: Option<i64>,
    pub energy_min: Option<f64>,
    pub energy_max: Option<f64>,
    /// Comma-separated tag names for OR filtering. Track matches if it has ANY of these tags.
    pub tags: Option<String>,
    /// Minimum BPM filter.
    pub bpm_min: Option<f64>,
    /// Maximum BPM filter.
    pub bpm_max: Option<f64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingSearchResponse {
    pub tags: Vec<SearchTagResult>,
    pub files: Vec<SearchFileResult>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchTagResult {
    pub id: i64,
    pub name: String,
    pub category_name: String,
    pub prefix: String,
    pub file_count: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchFileResult {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub file_type: String,
    pub genre: Option<String>,
    pub isrc: Option<String>,
    pub duration_ms: Option<i64>,
    pub play_count: i32,
    pub last_played: Option<i64>,
    pub energy_level: Option<f64>,
    pub tags: Vec<DiggingTag>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingTracksQuery {
    pub q: Option<String>,
    pub tags: Option<String>,
    pub energy_levels: Option<String>,
    pub key_list: Option<String>,
    pub key_range: Option<String>,
    pub bpm_min: Option<f64>,
    pub bpm_max: Option<f64>,
    pub page: Option<i64>,
    pub page_size: Option<i64>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
    pub pmv_categories: Option<String>,
    pub pmv_aggregate: Option<String>,
}

/// Internal row for the tracks query (maps SQL columns directly).
#[derive(Debug, FromRow)]
#[allow(dead_code)]
struct TrackDiggingRow {
    id: i64,
    service: String,
    service_id: String,
    title: String,
    artist: String,
    isrc: Option<String>,
    duration_ms: Option<i64>,
    genre: Option<String>,
    bpm: Option<f64>,
    musical_key: Option<String>,
    play_count: i32,
    rating: i32,
    last_played: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingTrackFile {
    pub id: i64,
    pub file_type: String,
    pub location: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingTrackResult {
    pub id: i64,
    pub service: String,
    pub title: String,
    pub artist: String,
    pub isrc: Option<String>,
    pub duration_ms: Option<i64>,
    pub genre: Option<String>,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub energy_level: Option<f64>,
    pub tags: Vec<DiggingTag>,
    pub files: Vec<DiggingTrackFile>,
    pub playlists: Vec<String>,
    pub file_match_count: i64,
    /// Max play count from linked files (via v_file_track_link)
    pub play_count: i32,
    /// Max rating from linked files, 0-100 scale (divide by 20 for stars)
    pub rating: i32,
    /// Most recent last_played from linked files (epoch seconds)
    pub last_played: Option<i64>,
    /// Number of distinct tag categories this track has
    pub tag_category_count: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingTracksResponse {
    pub tracks: Vec<DiggingTrackResult>,
    pub total: i64,
    pub page: i64,
    pub page_size: i64,
}

/// Batch-load tags for multiple track IDs in a single query.
/// Returns a map from track_id -> Vec<DiggingTag>.
async fn fetch_tags_for_tracks_batch(
    pool: &Pool<Sqlite>,
    track_ids: &[i64],
) -> Result<std::collections::HashMap<i64, Vec<DiggingTag>>> {
    if track_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let placeholders: Vec<String> = track_ids.iter().map(|_| "?".to_string()).collect();
    let sql = format!(
        r#"SELECT spt.track_id, t.id, t.name, tc.name as category_name, tc.prefix
           FROM service_playlist_tracks spt
           JOIN service_playlists sp ON sp.id = spt.playlist_id
           JOIN tags t ON LOWER(TRIM(t.name)) = LOWER(TRIM(sp.name))
           JOIN tag_categories tc ON tc.id = t.category_id
           WHERE spt.track_id IN ({})
             AND (sp.archive_deleted = 1 OR spt.deleted_at IS NULL)
           ORDER BY t.name"#,
        placeholders.join(",")
    );

    let mut q = sqlx::query(&sql);
    for id in track_ids {
        q = q.bind(id);
    }

    let rows = q.fetch_all(pool).await?;

    let mut result: std::collections::HashMap<i64, Vec<DiggingTag>> =
        std::collections::HashMap::new();
    for row in rows {
        let track_id: i64 = row.get("track_id");
        let tag = DiggingTag {
            id: row.get("id"),
            name: row.get("name"),
            category_name: row.get("category_name"),
            prefix: row.get("prefix"),
        };
        result.entry(track_id).or_default().push(tag);
    }

    Ok(result)
}

/// Search tracks with optional filters for BPM, key (with Camelot expansion),
/// energy levels, tags, and text search. Paginated.
pub async fn search_digging_tracks(
    pool: &Pool<Sqlite>,
    query: &DiggingTracksQuery,
) -> Result<DiggingTracksResponse> {
    let page = query.page.unwrap_or(0).max(0);
    let page_size = query.page_size.unwrap_or(20).clamp(1, 100);
    let has_text = query
        .q
        .as_ref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    let search_pattern = has_text
        .then(|| format!("%{}%", query.q.as_ref().unwrap().trim()))
        .unwrap_or_default();

    // ── Parse filter tags ──
    let filter_tags: Vec<String> = query
        .tags
        .as_ref()
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_lowercase())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let has_tag_filter = !filter_tags.is_empty();

    // ── Parse BPM ──
    let has_bpm = query.bpm_min.is_some() || query.bpm_max.is_some();
    let bpm_min = query.bpm_min.unwrap_or(0.0);
    let bpm_max = query.bpm_max.unwrap_or(999.0);

    // ── Parse energy levels ──
    // Each energy level E (1-5) maps to [E - 0.5, E + 0.5]
    let energy_levels: Vec<u8> = query
        .energy_levels
        .as_ref()
        .map(|s| {
            s.split(',')
                .filter_map(|e| e.trim().parse::<u8>().ok())
                .filter(|&e| (1..=5).contains(&e))
                .collect()
        })
        .unwrap_or_default();
    let has_energy_filter = !energy_levels.is_empty();

    // ── Parse key_list + key_range ──
    let key_list: Vec<String> = query
        .key_list
        .as_ref()
        .map(|s| {
            s.split(',')
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let key_range: Vec<String> = query
        .key_range
        .as_ref()
        .map(|s| {
            s.split(',')
                .map(|j| j.trim().to_string())
                .filter(|j| !j.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let has_key_filter = !key_list.is_empty() && !key_range.is_empty();

    // Expand key_list using key_range jumps
    let expanded_keys: std::collections::BTreeSet<String> = if has_key_filter {
        key_list
            .iter()
            .filter_map(|k| {
                let base = parse_camelot_key(k)?;
                let all_keys: Vec<String> = (1..=12)
                    .flat_map(|pos| [format!("{}m", pos), format!("{}d", pos)])
                    .collect();
                Some(
                    all_keys
                        .iter()
                        .filter_map(|k2| {
                            let target = parse_camelot_key(k2)?;
                            if are_keys_compatible(base, target, &key_range) {
                                Some(k2.clone())
                            } else {
                                None
                            }
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .flatten()
            .collect()
    } else {
        std::collections::BTreeSet::new()
    };

    let expanded_key_str: Vec<String> = expanded_keys.iter().map(|k| k.to_lowercase()).collect();

    // ── Parse PMV filters ──
    let pmv_categories: Vec<String> = query
        .pmv_categories
        .as_ref()
        .map(|s| {
            s.split(',')
                .map(|c| c.trim().to_lowercase())
                .filter(|c| !c.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let has_pmv_categories = !pmv_categories.is_empty();

    let pmv_aggregate: Option<String> = query
        .pmv_aggregate
        .as_ref()
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let has_pmv_aggregate = pmv_aggregate.is_some();
    let _has_pmv_filter = has_pmv_categories || has_pmv_aggregate;

    // ── Build FROM + JOIN + WHERE clauses ──
    // Use a direct JOIN to v_file_track_link + files instead of correlated EXISTS
    // subqueries, which are extremely slow (2000x slower) on SQLite views.

    let from_clause = String::from(
        "FROM service_tracks st
         JOIN v_file_track_link vft ON vft.track_id = st.id
         JOIN files f ON f.id = vft.file_id AND f.bpm IS NOT NULL AND f.musical_key IS NOT NULL",
    );

    let mut where_parts: Vec<String> = Vec::new();

    if has_text {
        where_parts.push("(st.title LIKE ? OR st.artist LIKE ?)".to_string());
    }

    if has_bpm {
        where_parts.push("f.bpm >= ?".to_string());
        where_parts.push("f.bpm <= ?".to_string());
    }

    if has_tag_filter {
        let placeholders: Vec<String> =
            filter_tags.iter().map(|_| "LOWER(?)".to_string()).collect();
        where_parts.push(format!(
            "EXISTS (SELECT 1 FROM file_resolved_tags frt3 WHERE frt3.file_id = f.id AND LOWER(frt3.tag_name) IN ({}))",
            placeholders.join(",")
        ));
    }

    if !expanded_key_str.is_empty() {
        let key_placeholders: Vec<String> = expanded_key_str
            .iter()
            .map(|_| "LOWER(?)".to_string())
            .collect();
        where_parts.push(format!(
            "LOWER(f.musical_key) IN ({})",
            key_placeholders.join(",")
        ));
    }

    if has_energy_filter {
        let mut energy_clauses: Vec<String> = Vec::new();
        for level in &energy_levels {
            let low = (*level as f64) - 0.5;
            let high = (*level as f64) + 0.5;
            energy_clauses.push(format!(
                "EXISTS (SELECT 1 FROM service_playlist_tracks spt_en
                         JOIN service_playlists sp_en ON sp_en.id = spt_en.playlist_id
                         JOIN tags t_en ON LOWER(TRIM(t_en.name)) = LOWER(TRIM(sp_en.name))
                         JOIN tag_energy_levels tel_en ON tel_en.tag_id = t_en.id
                         JOIN v_file_track_link vft_en ON vft_en.track_id = spt_en.track_id
                         WHERE vft_en.file_id = f.id
                           AND (sp_en.archive_deleted = 1 OR spt_en.deleted_at IS NULL)
                         GROUP BY vft_en.file_id
                         HAVING AVG(CAST(tel_en.energy_level AS REAL)) BETWEEN {} AND {})",
                low, high
            ));
        }
        where_parts.push(format!("({})", energy_clauses.join(" OR ")));
    }

    // ── PMV categories filter (OR logic via EXISTS subquery) ──
    if has_pmv_categories {
        let pmv_placeholders: Vec<String> = pmv_categories
            .iter()
            .map(|_| "LOWER(?)".to_string())
            .collect();
        where_parts.push(format!(
            "EXISTS (SELECT 1 FROM track_resolved_tags trt_pmv WHERE trt_pmv.track_id = st.id AND LOWER(trt_pmv.prefix) IN ({}))",
            pmv_placeholders.join(",")
        ));
    }

    // ── PMV aggregate filter ──
    if has_pmv_aggregate {
        match pmv_aggregate.as_deref() {
            Some("full") => {
                // Track must have tags in all three PMV categories
                for prefix in ["p", "m", "v"] {
                    where_parts.push(format!(
                        "EXISTS (SELECT 1 FROM track_resolved_tags trt_pa WHERE trt_pa.track_id = st.id AND LOWER(trt_pa.prefix) = '{}')",
                        prefix
                    ));
                }
            }
            Some("partial") => {
                // Track must have at least one PMV category tag
                where_parts.push(
                    "EXISTS (SELECT 1 FROM track_resolved_tags trt_pp WHERE trt_pp.track_id = st.id AND LOWER(trt_pp.prefix) IN ('p','m','v'))"
                        .to_string(),
                );
            }
            Some("none") => {
                // Track must have NO PMV tags
                where_parts.push(
                    "NOT EXISTS (SELECT 1 FROM track_resolved_tags trt_pn WHERE trt_pn.track_id = st.id AND LOWER(trt_pn.prefix) IN ('p','m','v'))"
                        .to_string(),
                );
            }
            _ => {}
        }
    }

    let where_clause = if where_parts.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", where_parts.join(" AND "))
    };

    // ── Count query (uses the base JOIN, no scalar subqueries needed) ──
    let count_sql = format!(
        "SELECT COUNT(DISTINCT st.id) {} {}",
        from_clause, where_clause
    );

    let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);

    if has_text {
        count_q = count_q.bind(&search_pattern).bind(&search_pattern);
    }
    if has_bpm {
        count_q = count_q.bind(bpm_min).bind(bpm_max);
    }
    for tag in &filter_tags {
        count_q = count_q.bind(tag);
    }
    for key in &expanded_key_str {
        count_q = count_q.bind(key);
    }
    // energy filter has no bind parameters (inlined literals)
    for prefix in &pmv_categories {
        count_q = count_q.bind(prefix);
    }
    // pmv aggregate filter has no bind parameters (inlined literals)

    let total: i64 = count_q.fetch_one(pool).await?;

    // ── Data query ──
    // Use GROUP BY with aggregate functions instead of DISTINCT + scalar subqueries,
    // which is much faster in SQLite (avoids N subquery evaluations per row).
    let offset = page * page_size;

    let data_sql = format!(
        r#"SELECT st.id, st.service, st.service_id, st.title, st.artist,
                   st.isrc, st.duration_ms,
                   MIN(f.genre) as genre,
                   MIN(f.bpm) as bpm,
                   MIN(f.musical_key) as musical_key,
                   COALESCE(MAX(f.play_count), 0) as play_count,
                   COALESCE(MAX(f.rating), 0) as rating,
                   MAX(f.last_played) as last_played
            {} {}
            GROUP BY st.id
            ORDER BY st.id
            LIMIT ? OFFSET ?"#,
        from_clause, where_clause
    );

    let mut data_q = sqlx::query_as::<_, TrackDiggingRow>(&data_sql);

    if has_text {
        data_q = data_q.bind(&search_pattern).bind(&search_pattern);
    }
    if has_bpm {
        data_q = data_q.bind(bpm_min).bind(bpm_max);
    }
    for tag in &filter_tags {
        data_q = data_q.bind(tag);
    }
    for key in &expanded_key_str {
        data_q = data_q.bind(key);
    }
    for prefix in &pmv_categories {
        data_q = data_q.bind(prefix);
    }
    data_q = data_q.bind(page_size).bind(offset);

    let rows: Vec<TrackDiggingRow> = data_q.fetch_all(pool).await?;

    // ── Batch-load tags for all track IDs (N+1 → 1 query) ──
    let track_ids: Vec<i64> = rows.iter().map(|r| r.id).collect();
    let all_tags = fetch_tags_for_tracks_batch(pool, &track_ids).await?;

    // ── Load auxiliary data per track ──
    // Tags are loaded from the materialized file_resolved_tags / track_resolved_tags
    // tables above. No need for slow view-based lookups.
    let mut results = Vec::with_capacity(rows.len());
    for row in rows {
        // Look up tags from the batch-loaded HashMap (no per-row query)
        let tags = all_tags.get(&row.id).cloned().unwrap_or_default();

        // Compute tag_category_count: distinct categories across this track's tags
        let tag_category_count = tags
            .iter()
            .map(|t| t.category_name.as_str())
            .collect::<std::collections::HashSet<&str>>()
            .len();

        // Compute file_match_count: how many of the filter tags this track has
        let tag_names: std::collections::HashSet<String> =
            tags.iter().map(|t| t.name.to_lowercase()).collect();
        let file_match_count = filter_tags
            .iter()
            .filter(|ft| tag_names.contains(ft.as_str()))
            .count() as i64;

        // Compute energy level directly from track's playlists (avoids file-based energy query)
        let energy_level: Option<f64> = {
            sqlx::query_scalar::<_, Option<f64>>(
                r#"SELECT AVG(CAST(energy_level AS REAL))
                   FROM (
                       SELECT DISTINCT t2.id as tag_id, tel2.energy_level
                       FROM service_playlist_tracks spt2
                       JOIN service_playlists sp2 ON sp2.id = spt2.playlist_id
                       JOIN tags t2 ON LOWER(TRIM(t2.name)) = LOWER(TRIM(sp2.name))
                       JOIN tag_energy_levels tel2 ON tel2.tag_id = t2.id
                       WHERE spt2.track_id = ?
                         AND (sp2.archive_deleted = 1 OR spt2.deleted_at IS NULL)
                   )"#,
            )
            .bind(row.id)
            .fetch_one(pool)
            .await?
        };

        // Load files
        let files: Vec<DiggingTrackFile> = {
            let file_rows = sqlx::query(
                r#"SELECT f.id, f.file_type,
                          CASE WHEN fl_local.id IS NOT NULL THEN 'local' ELSE 'backup' END as location
                   FROM files f
                   JOIN v_file_track_link vftl4 ON vftl4.file_id = f.id
                   LEFT JOIN file_locations fl_local ON fl_local.file_id = f.id
                       AND fl_local.location_type = 'local'
                   WHERE vftl4.track_id = ?
                   ORDER BY f.file_path"#,
            )
            .bind(row.id)
            .fetch_all(pool)
            .await?;

            file_rows
                .iter()
                .map(|r| DiggingTrackFile {
                    id: r.get("id"),
                    file_type: r.get("file_type"),
                    location: r.get("location"),
                })
                .collect()
        };

        // Load playlists (already uses direct table joins — no view chain)
        let playlists: Vec<String> = {
            let pl_rows: Vec<(String,)> = sqlx::query_as(
                r#"SELECT DISTINCT sp.name
                   FROM service_playlist_tracks spt
                   JOIN service_playlists sp ON sp.id = spt.playlist_id
                   WHERE spt.track_id = ?
                   ORDER BY sp.name"#,
            )
            .bind(row.id)
            .fetch_all(pool)
            .await?;

            pl_rows.into_iter().map(|(name,)| name).collect()
        };

        results.push(DiggingTrackResult {
            id: row.id,
            service: row.service,
            title: row.title,
            artist: row.artist,
            isrc: row.isrc,
            duration_ms: row.duration_ms,
            genre: row.genre,
            bpm: row.bpm,
            musical_key: row.musical_key,
            energy_level,
            tags,
            files,
            playlists,
            file_match_count,
            play_count: row.play_count,
            rating: row.rating,
            last_played: row.last_played,
            tag_category_count,
        });
    }

    // ── Relevance sorting (if tag filter is active) ──
    if has_tag_filter {
        let e_levels = &energy_levels;
        results.sort_by(|a, b| {
            // Primary: file_match_count descending
            let match_cmp = b.file_match_count.cmp(&a.file_match_count);
            if match_cmp != std::cmp::Ordering::Equal {
                return match_cmp;
            }

            // Secondary: energy proximity to nearest target energy midpoint
            if has_energy_filter {
                // Compute midpoints of each energy level range
                let midpoints: Vec<f64> = e_levels.iter().map(|&e| e as f64).collect();

                let a_prox = a.energy_level.map(|ae| {
                    midpoints
                        .iter()
                        .map(|&m| (ae - m).abs())
                        .fold(f64::MAX, f64::min)
                });
                let b_prox = b.energy_level.map(|be| {
                    midpoints
                        .iter()
                        .map(|&m| (be - m).abs())
                        .fold(f64::MAX, f64::min)
                });

                let energy_cmp = match (a_prox, b_prox) {
                    (Some(ap), Some(bp)) => {
                        ap.partial_cmp(&bp).unwrap_or(std::cmp::Ordering::Equal)
                    }
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, None) => std::cmp::Ordering::Equal,
                };
                if energy_cmp != std::cmp::Ordering::Equal {
                    return energy_cmp;
                }
            }

            // Tertiary: BPM proximity to BPM midpoint
            let bpm_mid = (bpm_min + bpm_max) / 2.0;
            let a_bpm_diff = a.bpm.map(|b| (b - bpm_mid).abs());
            let b_bpm_diff = b.bpm.map(|b| (b - bpm_mid).abs());
            match (a_bpm_diff, b_bpm_diff) {
                (Some(ad), Some(bd)) => ad.partial_cmp(&bd).unwrap_or(std::cmp::Ordering::Equal),
                _ => std::cmp::Ordering::Equal,
            }
        });
    }

    // ---- Server-side sorting ----
    let sort_by = query.sort_by.as_deref().unwrap_or("relevance");
    let sort_asc = query
        .sort_order
        .as_deref()
        .map(|o| o == "asc")
        .unwrap_or(false);

    // When no filters are active and no text search, default to rating desc, playCount desc
    let is_default = query
        .q
        .as_deref()
        .map(|q| q.trim().is_empty())
        .unwrap_or(true)
        && !has_tag_filter
        && !has_energy_filter
        && !has_bpm
        && !has_key_filter;
    let effective_sort = if is_default { "rating" } else { sort_by };
    let effective_order = if is_default { false } else { sort_asc };

    if sort_by != "relevance" || is_default {
        results.sort_by(|a, b| {
            let cmp = match effective_sort {
                "playCount" => a.play_count.cmp(&b.play_count),
                "rating" => a.rating.cmp(&b.rating),
                "bpm" => a
                    .bpm
                    .unwrap_or(0.0)
                    .partial_cmp(&b.bpm.unwrap_or(0.0))
                    .unwrap_or(std::cmp::Ordering::Equal),
                "energy" => a
                    .energy_level
                    .unwrap_or(-1.0)
                    .partial_cmp(&b.energy_level.unwrap_or(-1.0))
                    .unwrap_or(std::cmp::Ordering::Equal),
                "lastPlayed" => a.last_played.unwrap_or(0).cmp(&b.last_played.unwrap_or(0)),
                "tagCount" => a.tag_category_count.cmp(&b.tag_category_count),
                _ => std::cmp::Ordering::Equal,
            };
            if effective_order { cmp } else { cmp.reverse() }
        });
    }

    Ok(DiggingTracksResponse {
        tracks: results,
        total,
        page,
        page_size,
    })
}

pub async fn search_tracks_and_files(
    pool: &Pool<Sqlite>,
    query: &DiggingSearchQuery,
) -> Result<DiggingSearchResponse> {
    let file_limit = query.file_limit.unwrap_or(20).clamp(1, 50);
    let tag_limit = query.tag_limit.unwrap_or(10).clamp(1, 50);
    let search_pattern = format!("%{}%", query.q);
    let has_text_search = !query.q.trim().is_empty();

    // Parse tag filter
    let filter_tags: Vec<String> = query
        .tags
        .as_ref()
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_lowercase())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let has_tag_filter = !filter_tags.is_empty();

    // BPM filter
    let has_bpm_filter = query.bpm_min.is_some() || query.bpm_max.is_some();
    let bpm_min = query.bpm_min.unwrap_or(0.0);
    let bpm_max = query.bpm_max.unwrap_or(999.0);

    // ── Tags query (unchanged) ──
    let tags: Vec<SearchTagResult> = sqlx::query_as::<_, (i64, String, String, String, i64)>(
        r#"SELECT t.id, t.name, tc.name as category_name, tc.prefix,
                   COUNT(DISTINCT frt.file_id) as file_count
                    FROM tags t
                    JOIN tag_categories tc ON tc.id = t.category_id
                    LEFT JOIN file_resolved_tags frt ON frt.tag_id = t.id
            WHERE t.name LIKE ?
            GROUP BY t.id
            ORDER BY file_count DESC
            LIMIT ?"#,
    )
    .bind(&search_pattern)
    .bind(tag_limit)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(
        |(id, name, category_name, prefix, file_count)| SearchTagResult {
            id,
            name,
            category_name,
            prefix,
            file_count,
        },
    )
    .collect();

    // ── Files query ──
    // Build SQL dynamically to support optional filters
    let mut sql = String::from("SELECT f.* FROM files f WHERE 1=1");

    if has_text_search {
        sql.push_str(
            " AND (f.title LIKE ? OR f.artist LIKE ? OR f.album LIKE ? OR f.genre LIKE ? OR f.comment LIKE ?)",
        );
    }

    if has_bpm_filter {
        sql.push_str(" AND f.bpm >= ? AND f.bpm <= ?");
    }

    if has_tag_filter {
        let placeholders: Vec<String> =
            filter_tags.iter().map(|_| "LOWER(?)".to_string()).collect();
        sql.push_str(&format!(
            " AND EXISTS (SELECT 1 FROM file_resolved_tags frt WHERE frt.file_id = f.id AND LOWER(frt.tag_name) IN ({}))",
            placeholders.join(",")
        ));
    }

    sql.push_str(" AND f.bpm IS NOT NULL AND f.musical_key IS NOT NULL");
    sql.push_str(" ORDER BY f.play_count ASC LIMIT ?");

    let mut q = sqlx::query_as::<_, File>(&sql);

    if has_text_search {
        q = q
            .bind(&search_pattern)
            .bind(&search_pattern)
            .bind(&search_pattern)
            .bind(&search_pattern)
            .bind(&search_pattern);
    }
    if has_bpm_filter {
        q = q.bind(bpm_min).bind(bpm_max);
    }
    for tag in &filter_tags {
        q = q.bind(tag);
    }
    q = q.bind(file_limit);

    let files: Vec<File> = q.fetch_all(pool).await?;

    // ── Load tags + energy for each file ──
    let mut file_results = Vec::with_capacity(files.len());
    for f in files {
        let tags = load_file_tags(pool, f.id).await?;
        let energy = compute_track_energy(pool, f.id).await.unwrap_or(None);
        file_results.push(SearchFileResult {
            id: f.id,
            title: f.title.unwrap_or_default(),
            artist: f.artist.unwrap_or_default(),
            bpm: f.bpm,
            musical_key: f.musical_key,
            file_type: f.file_type,
            genre: f.genre,
            isrc: f.isrc,
            duration_ms: f.duration_ms,
            play_count: f.play_count,
            last_played: f.last_played,
            energy_level: energy,
            tags,
        });
    }

    // ── Energy filter (client-side, after loading tags + energy) ──
    if query.energy_min.is_some() || query.energy_max.is_some() {
        let e_min = query.energy_min.unwrap_or(0.0);
        let e_max = query.energy_max.unwrap_or(5.0);
        file_results.retain(|fr| match fr.energy_level {
            Some(e) => e >= e_min && e <= e_max,
            None => false,
        });
    }

    // ── Relevance sorting ──
    if has_tag_filter {
        let e_target = query
            .energy_min
            .map(|m| (m + query.energy_max.unwrap_or(m)) / 2.0);
        file_results.sort_by(|a, b| {
            let a_tag_names: std::collections::HashSet<&str> =
                a.tags.iter().map(|t| t.name.as_str()).collect();
            let b_tag_names: std::collections::HashSet<&str> =
                b.tags.iter().map(|t| t.name.as_str()).collect();

            let a_matches = filter_tags
                .iter()
                .filter(|ft| a_tag_names.contains(ft.as_str()))
                .count();
            let b_matches = filter_tags
                .iter()
                .filter(|ft| b_tag_names.contains(ft.as_str()))
                .count();

            // Sort by match count descending
            let match_cmp = b_matches.cmp(&a_matches);
            if match_cmp != std::cmp::Ordering::Equal {
                return match_cmp;
            }

            // Then by energy proximity to the target energy midpoint
            match (e_target, a.energy_level, b.energy_level) {
                (Some(t), Some(ae), Some(be)) => (ae - t)
                    .abs()
                    .partial_cmp(&(be - t).abs())
                    .unwrap_or(std::cmp::Ordering::Equal),
                (Some(_), None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(_), Some(_), None) => std::cmp::Ordering::Less,
                _ => std::cmp::Ordering::Equal,
            }
        });
    }

    Ok(DiggingSearchResponse {
        tags,
        files: file_results,
    })
}

// ============================================================================
// Ladder Suggest
// ============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LadderSuggestRequest {
    pub previous_track_id: i64,
    pub target_energy: Option<f64>,
    pub bpm_range: Option<f64>,
    pub key_jumps: Option<Vec<String>>,
    pub exclude_file_ids: Option<Vec<i64>>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LadderPreviousTrack {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub energy_level: Option<f64>,
    pub tags: Vec<DiggingTag>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LadderSuggestResponse {
    pub previous_track: LadderPreviousTrack,
    pub suggestions: Vec<DiggingSuggestion>,
    pub candidates_considered: usize,
}

pub async fn get_ladder_suggestions(
    pool: &Pool<Sqlite>,
    req: &LadderSuggestRequest,
) -> Result<LadderSuggestResponse> {
    use std::collections::HashSet;

    let bpm_range = req.bpm_range.unwrap_or(5.0).clamp(1.0, 20.0);
    let limit = (req.limit.unwrap_or(10) as usize).clamp(1, 50);
    let candidate_pool = limit * 5;

    let active_jumps: Vec<String> = req.key_jumps.clone().unwrap_or_else(|| {
        vec![
            "+1".to_string(),
            "-1".to_string(),
            "same".to_string(),
            "a_to_b".to_string(),
        ]
    });

    let prev_file = sqlx::query_as::<_, File>("SELECT * FROM files WHERE id = ?")
        .bind(req.previous_track_id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Previous track not found: id={}", req.previous_track_id))?;

    let prev_bpm = prev_file.bpm.unwrap_or(120.0);
    let bpm_min = prev_bpm - bpm_range;
    let bpm_max = prev_bpm + bpm_range;
    let prev_key = prev_file.musical_key.as_deref().and_then(parse_camelot_key);
    let prev_energy = compute_track_energy(pool, prev_file.id)
        .await
        .unwrap_or(None);
    let prev_tags = load_file_tags(pool, prev_file.id).await?;

    let prev_track_info = LadderPreviousTrack {
        id: prev_file.id,
        title: prev_file.title.unwrap_or_default(),
        artist: prev_file.artist.unwrap_or_default(),
        bpm: prev_file.bpm,
        musical_key: prev_file.musical_key.clone(),
        energy_level: prev_energy,
        tags: prev_tags.clone(),
    };

    let mut excluded = req.exclude_file_ids.clone().unwrap_or_default();
    excluded.push(prev_file.id);
    let exclude_json = serde_json::to_string(&excluded)?;

    let candidates: Vec<File> = sqlx::query_as::<_, File>(
        "SELECT * FROM files \
         WHERE id NOT IN (SELECT value FROM json_each(?)) \
           AND bpm IS NOT NULL \
           AND musical_key IS NOT NULL \
           AND bpm >= ? AND bpm <= ? \
         ORDER BY play_count ASC, COALESCE(last_played, 0) ASC \
         LIMIT ?",
    )
    .bind(&exclude_json)
    .bind(bpm_min)
    .bind(bpm_max)
    .bind(candidate_pool as i64)
    .fetch_all(pool)
    .await?;

    let candidates_considered = candidates.len();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let mut suggestions: Vec<DiggingSuggestion> = Vec::with_capacity(candidates.len());

    for candidate in candidates {
        let candidate_key = candidate.musical_key.as_deref().and_then(parse_camelot_key);
        let candidate_tags = load_file_tags(pool, candidate.id).await?;
        let candidate_energy = compute_track_energy(pool, candidate.id)
            .await
            .unwrap_or(None);

        let compat = match (prev_key, candidate_key) {
            (Some(pk), Some(ck)) => {
                if are_keys_compatible(pk, ck, &active_jumps) {
                    if pk == ck {
                        Some("perfect")
                    } else if pk.position == ck.position {
                        Some("good")
                    } else {
                        Some("ok")
                    }
                } else {
                    None
                }
            }
            _ => None,
        };

        if let Some(compat_str) = compat {
            let bpm_diff = candidate.bpm.map(|b| (b - prev_bpm).abs());

            let play_count_score = (candidate.play_count as f64).min(100.0) * 2.0;
            let recency_score = if let Some(lp) = candidate.last_played {
                let days_since = (now - lp) / 86400;
                (1000.0 - (days_since as f64).min(1000.0)) * 0.5
            } else {
                -50.0
            };
            let bpm_score = bpm_diff.map(|d| d * 1.5).unwrap_or(0.0);
            let camelot_bonus = match compat_str {
                "perfect" => -30.0,
                "good" => -15.0,
                _ => 0.0,
            };

            let energy_match_score = match (candidate_energy, req.target_energy) {
                (Some(ce), Some(te)) => (ce - te).abs() * 10.0,
                (None, Some(_)) => 20.0,
                (_, None) => 0.0,
            };

            let prev_tag_names: HashSet<&str> = prev_tags.iter().map(|t| t.name.as_str()).collect();
            let cand_tag_names: HashSet<&str> =
                candidate_tags.iter().map(|t| t.name.as_str()).collect();
            let shared: Vec<String> = prev_tag_names
                .intersection(&cand_tag_names)
                .map(|s| s.to_string())
                .collect();
            let tag_match_bonus = -(shared.len() as f64) * 5.0;

            let candidate_categories: HashSet<&str> = candidate_tags
                .iter()
                .map(|t| t.category_name.as_str())
                .collect();
            let tag_richness_bonus = -(candidate_categories.len() as f64) * 3.0;

            let total_score = play_count_score
                + recency_score
                + bpm_score
                + camelot_bonus
                + energy_match_score
                + tag_match_bonus
                + tag_richness_bonus;

            suggestions.push(DiggingSuggestion {
                file_id: candidate.id,
                title: candidate.title.unwrap_or_default(),
                artist: candidate.artist.unwrap_or_default(),
                bpm: candidate.bpm,
                musical_key: candidate.musical_key.clone(),
                genre: candidate.genre.clone(),
                isrc: candidate.isrc.clone(),
                file_path: candidate.file_path,
                file_type: candidate.file_type,
                play_count: candidate.play_count,
                last_played: candidate.last_played,
                duration_ms: candidate.duration_ms,
                matching_seed_id: prev_file.id,
                camelot_compatibility: compat_str.to_string(),
                bpm_diff,
                shared_tags: shared,
                all_tags: candidate_tags,
                energy_level: candidate_energy,
                score_breakdown: ScoreBreakdown {
                    play_count_score,
                    recency_score,
                    bpm_score,
                    camelot_bonus,
                    tag_match_bonus,
                    tag_richness_bonus,
                    category_overlap_bonus: 0.0,
                    energy_match_score,
                },
                score: total_score,
            });
        }
    }

    suggestions.sort_by(|a, b| {
        a.score
            .partial_cmp(&b.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    suggestions.truncate(limit);

    Ok(LadderSuggestResponse {
        previous_track: prev_track_info,
        suggestions,
        candidates_considered,
    })
}
