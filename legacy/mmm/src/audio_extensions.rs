//! Audio file extension enum for type-safe extension validation and matching.
//!
//! This module provides an `AudioExtension` enum that represents all supported
//! audio file formats with case-insensitive parsing and compound extension support.
//!
//! ## Supported Formats
//! - Mp3, Opus, Flac, M4a, StemM4a, Wav, Aac, Ogg, Alac, Webm, Mka, Tta, Wma
//!
//! ## Features
//! - Case-insensitive parsing (`.MP3` matches `AudioExtension::Mp3`)
//! - Compound extension support (`.stem.m4a` matches `StemM4a`, not `M4a`)
//! - Type-safe validation for folder configuration
//! - Serialization/Deserialization support

use serde::{Deserialize, Serialize};
use std::{cmp::Reverse, fmt, str::FromStr};

/// Supported audio file extensions with case-insensitive parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioExtension {
    /// MP3 audio files (.mp3)
    Mp3,
    /// Opus audio files (.opus)
    Opus,
    /// FLAC audio files (.flac)
    Flac,
    /// MPEG-4 Audio files (.m4a)
    M4a,
    /// Stem files (.stem.m4a)
    StemM4a,
    /// Waveform Audio files (.wav)
    Wav,
    /// Advanced Audio Coding files (.aac)
    Aac,
    /// Ogg Vorbis files (.ogg)
    Ogg,
    /// Apple Lossless Audio Codec files (.alac)
    Alac,
    /// WebM audio files (.webm)
    Webm,
    /// Matroska audio files (.mka)
    Mka,
    /// True Audio files (.tta)
    Tta,
    /// Windows Media Audio files (.wma)
    Wma,
}

impl AudioExtension {
    /// Returns the string representation of the extension (without leading dot).
    ///
    /// For compound extensions like `.stem.m4a`, returns `"stem.m4a"`.
    pub fn as_str(&self) -> &'static str {
        match self {
            AudioExtension::Mp3 => "mp3",
            AudioExtension::Opus => "opus",
            AudioExtension::Flac => "flac",
            AudioExtension::M4a => "m4a",
            AudioExtension::StemM4a => "stem.m4a",
            AudioExtension::Wav => "wav",
            AudioExtension::Aac => "aac",
            AudioExtension::Ogg => "ogg",
            AudioExtension::Alac => "alac",
            AudioExtension::Webm => "webm",
            AudioExtension::Mka => "mka",
            AudioExtension::Tta => "tta",
            AudioExtension::Wma => "wma",
        }
    }

    /// Returns the enum variant name as a string (for serialization).
    ///
    /// This returns the Rust enum variant name (e.g., "Mp3", "StemM4a").
    pub fn variant_name(&self) -> &'static str {
        match self {
            AudioExtension::Mp3 => "Mp3",
            AudioExtension::Opus => "Opus",
            AudioExtension::Flac => "Flac",
            AudioExtension::M4a => "M4a",
            AudioExtension::StemM4a => "StemM4a",
            AudioExtension::Wav => "Wav",
            AudioExtension::Aac => "Aac",
            AudioExtension::Ogg => "Ogg",
            AudioExtension::Alac => "Alac",
            AudioExtension::Webm => "Webm",
            AudioExtension::Mka => "Mka",
            AudioExtension::Tta => "Tta",
            AudioExtension::Wma => "Wma",
        }
    }

    /// Checks if a file path matches this extension (case-insensitive).
    ///
    /// Handles compound extensions like `.stem.m4a` correctly.
    ///
    /// # Examples
    /// ```
    /// use momos_music_manager::audio_extensions::AudioExtension;
    ///
    /// assert!(AudioExtension::StemM4a.matches_file("track.stem.m4a"));
    /// assert!(AudioExtension::StemM4a.matches_file("track.STEM.M4A")); // case-insensitive
    /// assert!(!AudioExtension::M4a.matches_file("track.stem.m4a")); // full extension required
    /// ```
    pub fn matches_file(&self, file_path: &str) -> bool {
        // Use from_file_path to get the correct extension for this file
        // This handles compound extensions and case-insensitive matching correctly
        // from_file_path no longer calls matches_file, so no recursion
        match Self::from_file_path(file_path) {
            Some(ext) => *self == ext,
            None => false,
        }
    }

    /// Extracts the file extension from a path and tries to match it to an AudioExtension.
    ///
    /// Returns `Some(AudioExtension)` if the extension matches a known audio format,
    /// `None` otherwise.
    ///
    /// Handles compound extensions and case-insensitive matching.
    pub fn from_file_path(file_path: &str) -> Option<Self> {
        let path_lower = file_path.to_lowercase();
        let path = std::path::Path::new(&path_lower);

        // Get file name without path
        let file_name = path.file_name()?.to_string_lossy().to_string();

        // Try all extensions from longest to shortest (for compound extensions)
        // This ensures .stem.m4a is checked before .m4a
        let mut extensions = Vec::new();
        for &ext in ALL_EXTENSIONS.iter() {
            extensions.push(ext);
        }

        // Sort by extension string length descending
        extensions.sort_by_key(|b| Reverse(b.as_str().len()));

        for &ext in extensions.iter() {
            let ext_str = ext.as_str();

            // For compound extensions, check if file ends with .extension
            if ext_str.contains('.') {
                if file_name.ends_with(&format!(".{}", ext_str)) {
                    return Some(ext);
                }
            } else {
                // For simple extensions, get the last extension component
                let path_ext = path.extension()?.to_string_lossy().to_lowercase();
                if path_ext == ext_str {
                    return Some(ext);
                }
            }
        }

        None
    }

    /// Returns all supported audio extensions as a vector.
    #[allow(dead_code)]
    pub fn all() -> Vec<Self> {
        ALL_EXTENSIONS.to_vec()
    }

    /// Parses a comma-separated string of extension names into a vector of AudioExtension.
    ///
    /// Returns an error if any extension name is invalid.
    ///
    /// # Examples
    /// ```
    /// use momos_music_manager::audio_extensions::AudioExtension;
    ///
    /// let exts = AudioExtension::parse_list("Mp3,StemM4a,Wav").unwrap();
    /// assert_eq!(exts.len(), 3);
    /// ```
    pub fn parse_list(list: &str) -> Result<Vec<Self>, String> {
        if list.trim().is_empty() {
            return Ok(Vec::new());
        }

        let mut result = Vec::new();
        for part in list.split(',') {
            let trimmed = part.trim();
            if !trimmed.is_empty() {
                let ext = Self::from_str(trimmed)
                    .map_err(|e| format!("Invalid extension '{}': {}", trimmed, e))?;
                result.push(ext);
            }
        }
        Ok(result)
    }

    /// Converts a vector of AudioExtension to a comma-separated string.
    ///
    /// Uses the variant names (e.g., "Mp3,StemM4a,Wav").
    #[allow(dead_code)]
    pub fn to_string_list(extensions: &[Self]) -> String {
        extensions
            .iter()
            .map(|ext| ext.variant_name())
            .collect::<Vec<_>>()
            .join(",")
    }
}

impl fmt::Display for AudioExtension {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.variant_name())
    }
}

impl FromStr for AudioExtension {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let normalized = s.trim().to_lowercase();
        match normalized.as_str() {
            "mp3" => Ok(AudioExtension::Mp3),
            "opus" => Ok(AudioExtension::Opus),
            "flac" => Ok(AudioExtension::Flac),
            "m4a" => Ok(AudioExtension::M4a),
            "stem.m4a" | "stemm4a" => Ok(AudioExtension::StemM4a),
            "wav" => Ok(AudioExtension::Wav),
            "aac" => Ok(AudioExtension::Aac),
            "ogg" => Ok(AudioExtension::Ogg),
            "alac" => Ok(AudioExtension::Alac),
            "webm" => Ok(AudioExtension::Webm),
            "mka" => Ok(AudioExtension::Mka),
            "tta" => Ok(AudioExtension::Tta),
            "wma" => Ok(AudioExtension::Wma),
            _ => Err(format!("Unknown audio extension: '{}'", s)),
        }
    }
}

/// All supported audio extensions as a static array.
pub const ALL_EXTENSIONS: &[AudioExtension] = &[
    AudioExtension::Mp3,
    AudioExtension::Opus,
    AudioExtension::Flac,
    AudioExtension::M4a,
    AudioExtension::StemM4a,
    AudioExtension::Wav,
    AudioExtension::Aac,
    AudioExtension::Ogg,
    AudioExtension::Alac,
    AudioExtension::Webm,
    AudioExtension::Mka,
    AudioExtension::Tta,
    AudioExtension::Wma,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_extension() {
        assert_eq!("mp3".parse(), Ok(AudioExtension::Mp3));
        assert_eq!("MP3".parse(), Ok(AudioExtension::Mp3));
        assert_eq!(" Mp3 ".parse(), Ok(AudioExtension::Mp3));
        assert_eq!("stem.m4a".parse(), Ok(AudioExtension::StemM4a));
        assert_eq!("STEM.M4A".parse(), Ok(AudioExtension::StemM4a));
        assert_eq!("stemm4a".parse(), Ok(AudioExtension::StemM4a));
        assert!("unknown".parse::<AudioExtension>().is_err());
    }

    #[test]
    fn test_matches_file() {
        assert!(AudioExtension::Mp3.matches_file("song.mp3"));
        assert!(AudioExtension::Mp3.matches_file("song.MP3"));
        assert!(AudioExtension::Mp3.matches_file("/path/to/song.mp3"));
        assert!(!AudioExtension::Mp3.matches_file("song.wav"));

        assert!(AudioExtension::StemM4a.matches_file("track.stem.m4a"));
        assert!(AudioExtension::StemM4a.matches_file("track.STEM.M4A"));
        assert!(!AudioExtension::M4a.matches_file("track.stem.m4a"));
        assert!(AudioExtension::M4a.matches_file("track.m4a"));
    }

    #[test]
    fn test_from_file_path() {
        assert_eq!(
            AudioExtension::from_file_path("song.mp3"),
            Some(AudioExtension::Mp3)
        );
        assert_eq!(
            AudioExtension::from_file_path("song.MP3"),
            Some(AudioExtension::Mp3)
        );
        assert_eq!(
            AudioExtension::from_file_path("track.stem.m4a"),
            Some(AudioExtension::StemM4a)
        );
        assert_eq!(
            AudioExtension::from_file_path("track.STEM.M4A"),
            Some(AudioExtension::StemM4a)
        );
        assert_eq!(
            AudioExtension::from_file_path("track.m4a"),
            Some(AudioExtension::M4a)
        );
        assert_eq!(
            AudioExtension::from_file_path("song.wav"),
            Some(AudioExtension::Wav)
        );
        assert_eq!(AudioExtension::from_file_path("song.txt"), None);
    }

    #[test]
    fn test_parse_list() {
        let exts = AudioExtension::parse_list("Mp3, StemM4a , Wav").unwrap();
        assert_eq!(exts.len(), 3);
        assert!(exts.contains(&AudioExtension::Mp3));
        assert!(exts.contains(&AudioExtension::StemM4a));
        assert!(exts.contains(&AudioExtension::Wav));

        let empty = AudioExtension::parse_list("").unwrap();
        assert!(empty.is_empty());

        let empty_trimmed = AudioExtension::parse_list("   ").unwrap();
        assert!(empty_trimmed.is_empty());

        assert!(AudioExtension::parse_list("Mp3,Unknown,Wav").is_err());
    }

    #[test]
    fn test_to_string_list() {
        let exts = vec![
            AudioExtension::Mp3,
            AudioExtension::StemM4a,
            AudioExtension::Wav,
        ];
        assert_eq!(AudioExtension::to_string_list(&exts), "Mp3,StemM4a,Wav");
    }

    #[test]
    fn test_all_extensions() {
        let all = AudioExtension::all();
        assert!(all.contains(&AudioExtension::Mp3));
        assert!(all.contains(&AudioExtension::StemM4a));
        assert!(all.contains(&AudioExtension::Webm));
        assert_eq!(all.len(), ALL_EXTENSIONS.len());
    }
}
