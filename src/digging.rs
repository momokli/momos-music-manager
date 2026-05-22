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
#[derive(Debug, Serialize)]
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

/// Load resolved tags (with category name + prefix) for a file from `v_file_tags`.
async fn load_file_tags(pool: &Pool<Sqlite>, file_id: i64) -> Result<Vec<DiggingTag>> {
    let rows = sqlx::query(
        r#"SELECT DISTINCT tag_id, tag_name, category_name, prefix
           FROM v_file_tags
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
            "SELECT DISTINCT file_id FROM v_file_tags WHERE LOWER(tag_name) = LOWER(?)",
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
                    });
                }
            }
        }

        if let (Some(seed_id), Some(compat), Some(breakdown)) =
            (best_seed_id, best_compat, best_breakdown)
        {
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
