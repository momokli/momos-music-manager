//! Traktor collection.nml parser and DB updater.
//!
//! Parses Traktor's `collection.nml` (XML) and matches entries against
//! the `files` table by absolute path. Updates `play_count`, `last_played`,
//! and optionally `rating` for each match.
//!
//! ## Path format
//! Traktor stores paths in Mac-style colon notation:
//!   `DIR="/:Users/:momo/:Music/:stems/:"`
//!   `FILE="Track Name.stem.m4a"`
//! → absolute: `/Users/momo/Music/stems/Track Name.stem.m4a`
//!
//! ## XML structure
//! ```xml
//! <ENTRY ...>
//!   <LOCATION DIR="/:Users/:momo/:Music/:stems/:" FILE="..."/>
//!   <INFO PLAYCOUNT="3" LAST_PLAYED="2025/12/1" RANKING="102" .../>
//! </ENTRY>
//! ```

use anyhow::{Context, Result};
use roxmltree::Document;
use sqlx::{Pool, Sqlite};
use std::path::{Path, PathBuf};
use tracing::{info, warn};

// ============================================================
// Data types
// ============================================================

/// A single entry parsed from Traktor's collection.nml.
#[derive(Debug, Clone)]
pub struct TraktorEntry {
    /// Mac-style colon path, e.g. `/:Users/:momo/:Music/:stems/:`
    pub dir: String,
    /// File name, e.g. `Track Name.stem.m4a`
    pub file: String,
    /// Number of plays (`PLAYCOUNT` attribute on `<INFO>`)
    pub play_count: Option<i32>,
    /// Last played date in Traktor format (`LAST_PLAYED`), e.g. `"2025/12/1"`
    pub last_played_raw: Option<String>,
    /// Rating (`RANKING` attribute on `<INFO>`, 0-255 scale in Traktor)
    pub rating: Option<i32>,
}

/// Statistics returned after an import run.
#[derive(Debug, Clone, Default)]
pub struct ImportStats {
    /// Total entries parsed from collection.nml
    pub total_entries: usize,
    /// Entries that matched a file in the database
    pub matched: usize,
    /// Entries that had no `PLAYCOUNT` attribute (skipped during update)
    pub no_play_count: usize,
    /// Files whose `play_count` was actually updated
    pub updated_play_count: usize,
    /// Files whose `last_played` was actually updated
    pub updated_last_played: usize,
}

// ============================================================
// Path conversion
// ============================================================

/// Convert a Traktor colon-path + filename to an absolute filesystem path.
///
/// Traktor format: `DIR="/:Users/:momo/:Music/:stems/:"`
/// → `/Users/momo/Music/stems/`
///
/// Traktor uses `:` as path separator (like a Mac resource fork path).
/// Replace `:` with `/` then normalize with PathBuf to collapse
/// multiple slashes into one. E.g. `/:Users/:momo/:Music/:stems/:`
/// → `/Users/momo/Music/stems/`.
fn traktor_path_to_abs(dir: &str, file_name: &str) -> PathBuf {
    // Replace `:` with `/`, then use Path::components() to normalize
    // (collapses `///Users//momo//Music//stems//` → `/Users/momo/Music/stems`)
    let unix_path = dir.replace(':', "/");
    let base: PathBuf = std::path::Path::new(&unix_path).components().collect();
    base.join(file_name)
}

// ============================================================
// Collection.nml discovery
// ============================================================

/// Default base directory for Native Instruments collections.
fn default_ni_base() -> PathBuf {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/Users"));
    home.join("Documents").join("Native Instruments")
}

/// Find all `collection.nml` files under `~/Documents/Native Instruments/Traktor */`.
pub fn find_all_collections() -> Vec<PathBuf> {
    let base = default_ni_base();
    if !base.exists() {
        warn!("Native Instruments directory not found: {:?}", base);
        return vec![];
    }

    let entries = match std::fs::read_dir(&base) {
        Ok(e) => e,
        Err(err) => {
            warn!("Failed to read {:?}: {}", base, err);
            return vec![];
        }
    };

    let mut collections: Vec<(PathBuf, std::time::SystemTime)> = vec![];

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !dir_name.starts_with("Traktor ") {
            continue;
        }

        let nml_path = path.join("collection.nml");
        if nml_path.exists() {
            // Get modification time for sorting (newest first)
            let mtime = nml_path
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            collections.push((nml_path, mtime));
        }
    }

    // Sort newest first
    collections.sort_by(|a, b| b.1.cmp(&a.1));

    collections.into_iter().map(|(p, _)| p).collect()
}

/// Find the most recent `collection.nml`.
pub fn find_latest_collection() -> Option<PathBuf> {
    find_all_collections().into_iter().next()
}

// ============================================================
// XML parsing
// ============================================================

/// Parse a single `<ENTRY>` node and extract relevant data.
fn parse_entry(node: roxmltree::Node) -> Option<TraktorEntry> {
    // Only process <ENTRY> nodes
    if !node.has_tag_name("ENTRY") {
        return None;
    }

    // Find <LOCATION> child
    let location = node.children().find(|c| c.has_tag_name("LOCATION"))?;
    let dir = location.attribute("DIR")?.to_string();
    let file = location.attribute("FILE")?.to_string();

    // Find <INFO> child
    let info = node.children().find(|c| c.has_tag_name("INFO"))?;

    let play_count = info
        .attribute("PLAYCOUNT")
        .and_then(|v| v.parse::<i32>().ok());

    let last_played_raw = info.attribute("LAST_PLAYED").map(|s| s.to_string());

    let rating = info
        .attribute("RANKING")
        .and_then(|v| v.parse::<i32>().ok());

    Some(TraktorEntry {
        dir,
        file,
        play_count,
        last_played_raw,
        rating,
    })
}

/// Parse a `collection.nml` file and return all entries.
///
/// This is a streaming-friendly parse: we walk the XML tree once
/// and extract every `<ENTRY>` with its `<LOCATION>` and `<INFO>` children.
pub fn parse_collection_nml(path: &Path) -> Result<Vec<TraktorEntry>> {
    let xml_content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read collection.nml: {}", path.display()))?;

    let doc = Document::parse(&xml_content)
        .with_context(|| format!("Failed to parse collection.nml XML: {}", path.display()))?;

    let mut entries: Vec<TraktorEntry> = vec![];

    // Root is <NML>, then <HEAD>, then <COLLECTION>
    let root = doc.root();
    let nml = root.children().find(|c| c.has_tag_name("NML")).or_else(|| {
        // Some versions may not have a wrapper NML element
        root.children().find(|c| c.has_tag_name("COLLECTION"))
    });

    let collection = match nml {
        Some(n) => {
            if n.has_tag_name("NML") {
                n.children().find(|c| c.has_tag_name("COLLECTION"))
            } else {
                Some(n) // already the COLLECTION element
            }
        }
        None => None,
    };

    let collection = match collection {
        Some(c) => c,
        None => {
            anyhow::bail!("Could not find <COLLECTION> element in collection.nml");
        }
    };

    for entry_node in collection.children() {
        if let Some(entry) = parse_entry(entry_node) {
            entries.push(entry);
        }
    }

    info!("Parsed {} entries from {}", entries.len(), path.display());
    Ok(entries)
}

/// Parse a LAST_PLAYED date string from Traktor format to a Unix timestamp.
///
/// Traktor format: `"2025/12/1"` (YYYY/M/D, no leading zeros).
/// Returns seconds since epoch.
fn parse_last_played(date_str: &str) -> Option<i64> {
    // Try Traktor format "YYYY/M/D"
    let parts: Vec<&str> = date_str.split('/').collect();
    if parts.len() == 3 {
        let year = parts[0].parse::<i32>().ok()?;
        let month = parts[1].parse::<u32>().ok()?;
        let day = parts[2].parse::<u32>().ok()?;

        let dt = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
        let datetime = dt.and_hms_opt(0, 0, 0)?;
        return Some(datetime.and_utc().timestamp());
    }

    // Fallback: try standard timestamp parsing
    if let Ok(ts) = date_str.parse::<i64>() {
        return Some(ts);
    }

    None
}

// ============================================================
// DB matching & updating
// ============================================================

/// Convert a Traktor `RANKING` value (0–255) to our rating scale (0–5).
///
/// Traktor uses 0–255 where:
/// - 0 = unrated
/// - 1–51 = 1 star (approx)
/// - 52–102 = 2 stars
/// - 103–153 = 3 stars
/// - 154–204 = 4 stars
/// - 205–255 = 5 stars
fn traktor_ranking_to_rating(ranking: i32) -> i32 {
    if ranking <= 0 {
        0
    } else if ranking <= 51 {
        1
    } else if ranking <= 102 {
        2
    } else if ranking <= 153 {
        3
    } else if ranking <= 204 {
        4
    } else {
        5
    }
}

/// Match parsed Traktor entries against the database and update stats.
///
/// Returns `ImportStats` with counts of what happened.
pub async fn import_play_stats(db: &Pool<Sqlite>, entries: &[TraktorEntry]) -> Result<ImportStats> {
    let mut stats = ImportStats {
        total_entries: entries.len(),
        ..Default::default()
    };

    if entries.is_empty() {
        info!("No entries to import");
        return Ok(stats);
    }

    // Pre-collect all file paths from the DB for matching
    // We build a HashMap<absolute_path, file_id> for O(1) lookups
    let rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, file_path FROM files")
        .fetch_all(db)
        .await
        .context("Failed to query files table")?;

    if rows.is_empty() {
        info!("No files in database — nothing to match");
        return Ok(stats);
    }

    // Build a lookup map: lowercased canonical path → (file_id, original_path)
    let path_map: std::collections::HashMap<String, (i64, String)> = rows
        .into_iter()
        .map(|(id, path)| {
            let normalized = normalize_path(&path);
            (normalized, (id, path))
        })
        .collect();

    // Prepare batch updates
    let mut updated_play_count = 0usize;
    let mut updated_last_played = 0usize;
    let mut matched = 0usize;
    let mut no_play_count = 0usize;

    // Collect updates to avoid holding a tx open during iteration
    type UpdateRow = (Option<i32>, Option<i64>, i64); // (play_count, last_played, file_id)
    let mut updates: Vec<UpdateRow> = vec![];

    for entry in entries {
        let abs_path = traktor_path_to_abs(&entry.dir, &entry.file);
        let abs_str = abs_path.to_string_lossy();
        let normalized = normalize_path(&abs_str);

        if let Some((file_id, _original_path)) = path_map.get(&normalized) {
            matched += 1;

            let play_count = entry.play_count;
            let last_played_ts = entry.last_played_raw.as_deref().and_then(parse_last_played);

            if play_count.is_none() {
                no_play_count += 1;
            } else {
                updated_play_count += 1;
            }
            if last_played_ts.is_some() {
                updated_last_played += 1;
            }

            updates.push((play_count, last_played_ts, *file_id));
        }
    }

    stats.matched = matched;
    stats.no_play_count = no_play_count;

    // Execute batch updates
    if !updates.is_empty() {
        let batch_size = 100;
        for chunk in updates.chunks(batch_size) {
            let now = chrono::Utc::now().timestamp();
            for (play_count, last_played_ts, file_id) in chunk {
                sqlx::query(
                    "UPDATE files SET play_count = COALESCE(?, play_count), last_played = COALESCE(?, last_played), updated_at = ? WHERE id = ?",
                )
                .bind(*play_count)
                .bind(*last_played_ts)
                .bind(now)
                .bind(file_id)
                .execute(db)
                .await
                .with_context(|| format!("Failed to update file #{}", file_id))?;
            }
        }
    }

    stats.updated_play_count = updated_play_count;
    stats.updated_last_played = updated_last_played;

    info!(
        "Traktor import complete: {} entries, {} matched, {} play counts, {} last played dates",
        stats.total_entries, stats.matched, stats.updated_play_count, stats.updated_last_played,
    );

    Ok(stats)
}

/// Normalize a file path for case-insensitive matching:
/// - Lowercase
/// - Remove trailing slash (if any)
fn normalize_path(path: &str) -> String {
    let p = path.trim().to_lowercase();
    p.trim_end_matches('/').to_string()
}

// ============================================================
// High-level API
// ============================================================

/// Run a full Traktor import: find collection.nml → parse → match → update.
///
/// Returns the stats and the path that was used.
pub async fn run_import(
    db: &Pool<Sqlite>,
    custom_path: Option<&Path>,
) -> Result<(ImportStats, PathBuf)> {
    let nml_path = resolve_collection_path(custom_path)?;
    let entries = parse_collection_nml(&nml_path)?;
    let stats = import_play_stats(db, &entries).await?;
    Ok((stats, nml_path))
}

/// Resolve the path to a collection.nml file.
///
/// If `custom_path` is provided, uses that directly (and validates it exists).
/// Otherwise, auto-detects the newest `collection.nml` under
/// `~/Documents/Native Instruments/Traktor */`.
pub fn resolve_collection_path(custom_path: Option<&Path>) -> Result<PathBuf> {
    match custom_path {
        Some(p) => {
            if !p.exists() {
                anyhow::bail!("collection.nml not found at: {}", p.display());
            }
            Ok(p.to_path_buf())
        }
        None => match find_latest_collection() {
            Some(p) => {
                info!("Auto-detected collection.nml: {}", p.display());
                Ok(p)
            }
            None => {
                // Try default paths as last resort
                let home = std::env::var("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| PathBuf::from("/Users"));
                let candidates = [
                    "Documents/Native Instruments/Traktor 4/collection.nml",
                    "Documents/Native Instruments/Traktor 3/collection.nml",
                    "Documents/Native Instruments/Traktor 2/collection.nml",
                ];
                let found = candidates.iter().find_map(|rel| {
                    let p = home.join(rel);
                    if p.exists() { Some(p) } else { None }
                });
                match found {
                    Some(p) => Ok(p),
                    None => anyhow::bail!(
                        "No collection.nml found. Searched ~/Documents/Native Instruments/Traktor */collection.nml"
                    ),
                }
            }
        },
    }
}

/// Get the path and last modification time of the collection.nml file.
///
/// Useful for checking if the collection has changed since last import.
pub fn get_collection_status(
    custom_path: Option<&Path>,
) -> Result<(PathBuf, std::time::SystemTime)> {
    let path = resolve_collection_path(custom_path)?;
    let mtime = path
        .metadata()
        .and_then(|m| m.modified())
        .map_err(|e| anyhow::anyhow!("Failed to read modification time: {}", e))?;
    Ok((path, mtime))
}

// ============================================================
// Tests
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_traktor_path_conversion() {
        let dir = "/:Users/:momo/:Music/:stems/:";
        let file = "Track Name.stem.m4a";
        let result = traktor_path_to_abs(dir, file);
        assert_eq!(
            result.to_string_lossy(),
            "/Users/momo/Music/stems/Track Name.stem.m4a"
        );
    }

    #[test]
    fn test_traktor_path_with_spaces() {
        let dir = "/:Users/:momo/:Music/:playlists/:Space House/:";
        let file = "Some Track.mp3";
        let result = traktor_path_to_abs(dir, file);
        assert_eq!(
            result.to_string_lossy(),
            "/Users/momo/Music/playlists/Space House/Some Track.mp3"
        );
    }

    #[test]
    fn test_parse_last_played_traktor() {
        // 2025-12-01 00:00:00 UTC
        let ts = parse_last_played("2025/12/1");
        assert!(
            ts.is_some(),
            "parse_last_played returned None for 2025/12/1"
        );
    }

    #[test]
    fn test_parse_last_played_single_digits() {
        // 2025-05-03 00:00:00 UTC
        let ts = parse_last_played("2025/5/3");
        assert!(ts.is_some(), "parse_last_played returned None for 2025/5/3");
    }

    #[test]
    fn test_parse_collection_nml_basic() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<NML VERSION="20">
<HEAD COMPANY="www.native-instruments.com" PROGRAM="Traktor Pro 4"></HEAD>
<COLLECTION ENTRIES="2">
<ENTRY MODIFIED_DATE="2026/3/1" MODIFIED_TIME="58284" LOCK="1"
      AUDIO_ID="abc123" TITLE="Test Track" ARTIST="Test Artist">
  <LOCATION DIR="/:Users/:test/:Music/:" FILE="test.mp3" VOLUME="Macintosh HD" VOLUMEID="Macintosh HD"></LOCATION>
  <ALBUM TITLE="Test Album"></ALBUM>
  <INFO BITRATE="320000" KEY="8m" PLAYCOUNT="5" PLAYTIME="240" IMPORT_DATE="2025/1/1" LAST_PLAYED="2025/6/15" RANKING="180" FLAGS="12" FILESIZE="10000"></INFO>
  <TEMPO BPM="124.000000" BPM_QUALITY="100.000000"></TEMPO>
</ENTRY>
<ENTRY MODIFIED_DATE="2026/3/1" MODIFIED_TIME="58285"
      AUDIO_ID="def456" TITLE="No Playcount" ARTIST="Silent Artist">
  <LOCATION DIR="/:Users/:test/:Music/:flacs/:" FILE="silent.flac" VOLUME="Macintosh HD" VOLUMEID="Macintosh HD"></LOCATION>
  <ALBUM TITLE="Silent Album"></ALBUM>
  <INFO BITRATE="900000" KEY="1m" PLAYTIME="300" IMPORT_DATE="2025/2/1" FLAGS="12" FILESIZE="20000"></INFO>
  <TEMPO BPM="128.000000" BPM_QUALITY="100.000000"></TEMPO>
</ENTRY>
</COLLECTION>
</NML>"#;

        // Write to temp file
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(xml.as_bytes()).unwrap();
        let path = tmpfile.into_temp_path();

        let entries = parse_collection_nml(&path).unwrap();
        assert_eq!(entries.len(), 2);

        // First entry
        assert_eq!(entries[0].dir, "/:Users/:test/:Music/:");
        assert_eq!(entries[0].file, "test.mp3");
        assert_eq!(entries[0].play_count, Some(5));
        assert_eq!(entries[0].last_played_raw, Some("2025/6/15".to_string()));
        assert_eq!(entries[0].rating, Some(180));

        // Second entry (no playcount)
        assert_eq!(entries[1].play_count, None);
        assert!(entries[1].last_played_raw.is_none());
        assert_eq!(entries[1].rating, None);
    }

    #[test]
    fn test_parse_empty_collection() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<NML VERSION="20">
<HEAD COMPANY="www.native-instruments.com" PROGRAM="Traktor Pro 4"></HEAD>
<COLLECTION ENTRIES="0">
</COLLECTION>
</NML>"#;

        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        tmpfile.write_all(xml.as_bytes()).unwrap();
        let path = tmpfile.into_temp_path();

        let entries = parse_collection_nml(&path).unwrap();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_traktor_ranking_to_rating() {
        assert_eq!(traktor_ranking_to_rating(0), 0);
        assert_eq!(traktor_ranking_to_rating(25), 1);
        assert_eq!(traktor_ranking_to_rating(75), 2);
        assert_eq!(traktor_ranking_to_rating(128), 3);
        assert_eq!(traktor_ranking_to_rating(180), 4);
        assert_eq!(traktor_ranking_to_rating(255), 5);
    }

    #[test]
    fn test_normalize_path() {
        assert_eq!(
            normalize_path("/Users/momo/Music/stems/"),
            "/Users/momo/Music/stems".to_lowercase()
        );
        assert_eq!(
            normalize_path("/USERS/MOMO/Music/Stems/"),
            "/users/momo/music/stems".to_lowercase()
        );
        assert_eq!(
            normalize_path("/Users/momo/Music/stems"),
            "/Users/momo/Music/stems".to_lowercase()
        );
    }
}
