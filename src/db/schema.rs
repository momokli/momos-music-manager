//! Shared schema-level helpers used across multiple domain modules.

use std::path::Path;

use anyhow::Result;
use sqlx::{Pool, Sqlite};
use std::fs;

use super::types::File;

/// Compute a fast identity hash from mtime + file size.
pub fn calculate_file_hash(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path)?;
    let size = metadata.len();
    let mtime = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    Ok(format!("{}-{}", size, mtime))
}

/// Normalize and validate a folder path.
pub fn normalize_and_validate_folder_path(path: &str) -> Result<String> {
    let stripped = path.trim().trim_end_matches('/').trim_end_matches('\\');
    if stripped.is_empty() {
        return Err(anyhow::anyhow!("Folder path cannot be empty"));
    }
    let canonical = std::fs::canonicalize(stripped)
        .map_err(|e| anyhow::anyhow!("Invalid folder path '{}': {}", stripped, e))?;
    Ok(canonical.to_string_lossy().to_string())
}

/// Determine the file type from its extension.
pub fn file_type_from_path(path: &Path) -> Option<&'static str> {
    path.extension()
        .and_then(|ext| ext.to_str())
        .and_then(|ext| match ext.to_lowercase().as_str() {
            "flac" => Some("flac"),
            "mp3" => Some("mp3"),
            "m4a" => Some("stem.m4a"),
            "wav" => Some("wav"),
            "wma" => Some("wma"),
            "aif" => Some("aif"),
            "aiff" => Some("aiff"),
            "ogg" => Some("ogg"),
            "opus" => Some("opus"),
            "m4p" => Some("m4p"),
            _ => None,
        })
}

/// Known stem types in nuo-stems convention.
pub const STEM_TYPES: &[&str] = &["vocals", "bass", "drums", "instrumental", "other"];

/// Parse a WAV filename and link it to its parent stem file.
///
/// Pattern: `{stem_name}_{stem_type}.wav` where stem_type ∈ {vocals,bass,drums,instrumental,other}
/// The stem file is `{stem_name}.stem.m4a` in the parent of the parent directory.
///
/// Returns the stem file_id and stem_type string on success.
pub async fn link_wav_to_stem(
    pool: &Pool<Sqlite>,
    wav_file_id: i64,
    wav_file_path: &str,
) -> Result<Option<(i64, String)>> {
    let path = std::path::Path::new(wav_file_path);
    let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

    // Extract stem_type: text after last '_' before '.wav'
    let stem_name_no_ext = filename.strip_suffix(".wav").unwrap_or(filename);
    let (stem_name, stem_type) = if let Some(last_underscore) = stem_name_no_ext.rfind('_') {
        let candidate = &stem_name_no_ext[last_underscore + 1..];
        if STEM_TYPES.contains(&candidate) {
            (&stem_name_no_ext[..last_underscore], candidate.to_string())
        } else {
            // Unknown suffix — not a stem part WAV
            return Ok(None);
        }
    } else {
        // No underscore — not a stem part WAV
        return Ok(None);
    };

    // The stem file is in the parent of the parent directory
    let parent = path.parent();
    let stems_root = parent.and_then(|p| p.parent());

    let expected_stem_path = if let Some(root) = stems_root {
        format!("{}/{}.stem.m4a", root.display(), stem_name)
    } else {
        return Ok(None);
    };

    // Look up the stem file
    let stem = sqlx::query_as::<_, File>(
        "SELECT * FROM files WHERE file_path = ? AND file_type = 'stem.m4a'",
    )
    .bind(&expected_stem_path)
    .fetch_optional(pool)
    .await?;

    match stem {
        Some(s) => {
            // Link: set source_of and stem_type
            sqlx::query("UPDATE files SET source_of = ?, stem_type = ? WHERE id = ?")
                .bind(s.id)
                .bind(&stem_type)
                .bind(wav_file_id)
                .execute(pool)
                .await?;
            Ok(Some((s.id, stem_type)))
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_file_hash_non_existent() {
        let result = calculate_file_hash(Path::new("/nonexistent/path/file.flac"));
        assert!(result.is_err());
    }

    #[test]
    fn test_file_type_from_path() {
        assert_eq!(file_type_from_path(Path::new("song.flac")), Some("flac"));
        assert_eq!(file_type_from_path(Path::new("song.mp3")), Some("mp3"));
        assert_eq!(
            file_type_from_path(Path::new("song.stem.m4a")),
            Some("stem.m4a")
        );
        assert_eq!(file_type_from_path(Path::new("song.wav")), Some("wav"));
        assert_eq!(file_type_from_path(Path::new("song.txt")), None);
    }

    #[test]
    fn test_normalize_and_validate_folder_path_empty() {
        let result = normalize_and_validate_folder_path("");
        assert!(result.is_err());
    }

    #[test]
    fn test_stem_types_contains_vocals() {
        assert!(STEM_TYPES.contains(&"vocals"));
        assert!(STEM_TYPES.contains(&"bass"));
        assert!(STEM_TYPES.contains(&"drums"));
        assert!(STEM_TYPES.contains(&"instrumental"));
        assert!(STEM_TYPES.contains(&"other"));
        assert_eq!(STEM_TYPES.len(), 5);
    }

    #[test]
    fn test_file_type_from_path_uppercase() {
        assert_eq!(file_type_from_path(Path::new("SONG.FLAC")), Some("flac"));
        assert_eq!(file_type_from_path(Path::new("SONG.MP3")), Some("mp3"));
        assert_eq!(
            file_type_from_path(Path::new("SONG.STEM.M4A")),
            Some("stem.m4a")
        );
        assert_eq!(file_type_from_path(Path::new("SONG.WAV")), Some("wav"));
    }

    #[test]
    fn test_file_type_from_path_edge_cases() {
        // No extension
        assert_eq!(file_type_from_path(Path::new("README")), None);
        assert_eq!(file_type_from_path(Path::new(".gitignore")), None);
        // Unknown extension
        assert_eq!(file_type_from_path(Path::new("song.txt")), None);
        assert_eq!(file_type_from_path(Path::new("song.pdf")), None);
        // Mixed case
        assert_eq!(file_type_from_path(Path::new("Song.Flac")), Some("flac"));
        assert_eq!(file_type_from_path(Path::new("Song.Aiff")), Some("aiff"));
        assert_eq!(file_type_from_path(Path::new("Song.Ogg")), Some("ogg"));
        assert_eq!(file_type_from_path(Path::new("Song.Wma")), Some("wma"));
        assert_eq!(file_type_from_path(Path::new("Song.Aif")), Some("aif"));
        assert_eq!(file_type_from_path(Path::new("Song.M4p")), Some("m4p"));
    }

    #[test]
    fn test_file_type_from_path_stem_m4a() {
        // stem.m4a specifically, not just any .m4a
        assert_eq!(
            file_type_from_path(Path::new("/music/stems/artist - title.stem.m4a")),
            Some("stem.m4a")
        );
        assert_eq!(
            file_type_from_path(Path::new("artist - title.stem.m4a")),
            Some("stem.m4a")
        );
        // Plain .m4a should still work
        assert_eq!(file_type_from_path(Path::new("song.m4a")), Some("stem.m4a"));
    }

    #[test]
    fn test_file_type_from_path_opus_and_ogg() {
        assert_eq!(file_type_from_path(Path::new("song.opus")), Some("opus"));
        assert_eq!(file_type_from_path(Path::new("song.ogg")), Some("ogg"));
    }

    #[test]
    fn test_calculate_file_hash_with_temp_file() {
        let dir = std::env::temp_dir();
        let path = dir.join("test_hash_mmm.tmp");
        std::fs::write(&path, b"hello world").unwrap();

        let hash = calculate_file_hash(&path).unwrap();
        // Should be "size-mtime" format (11 bytes, then a dash, then a timestamp)
        let parts: Vec<&str> = hash.split('-').collect();
        assert_eq!(
            parts.len(),
            2,
            "hash should be size-mtime format, got: {}",
            hash
        );
        assert_eq!(parts[0], "11", "size should be 11 bytes");
        assert!(parts[1].parse::<u64>().is_ok(), "mtime should be numeric");

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn test_normalize_and_validate_folder_path_trailing_slash() {
        let dir = std::env::temp_dir();
        let canonical = std::fs::canonicalize(&dir).unwrap();
        let expected = canonical.to_string_lossy().to_string();

        // With trailing slash
        let input = format!("{}/", dir.display());
        let result = normalize_and_validate_folder_path(&input).unwrap();
        assert_eq!(result, expected);

        // Without trailing slash
        let input = format!("{}", dir.display());
        let result = normalize_and_validate_folder_path(&input).unwrap();
        assert_eq!(result, expected);
    }
}
