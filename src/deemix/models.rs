use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Response from POST /api/loginArl
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixLoginResponse {
    pub status: i64,
    pub arl: String,
    pub user: DeemixUser,
    pub childs: Vec<DeemixUser>,
    #[serde(default)]
    pub current_child: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixUser {
    /// The deemix API has changed over versions — newer versions return
    /// `"user": {}` (empty object) after login. All fields are optional
    /// to tolerate both old and new response shapes.
    #[serde(default)]
    pub id: Option<i64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub picture: Option<String>,
    #[serde(default)]
    pub license_token: Option<String>,
    #[serde(default)]
    pub can_stream_hq: Option<bool>,
    #[serde(default)]
    pub can_stream_lossless: Option<bool>,
    #[serde(default)]
    pub country: Option<String>,
    #[serde(default)]
    pub language: Option<String>,
}

/// Single queue item from GET /api/getQueue (keyed by uuid in response)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixQueueItem {
    #[serde(
        rename = "type",
        default,
        deserialize_with = "deserialize_nullable_string"
    )]
    pub item_type: String,
    /// Playlist or track id — deemix returns strings for playlists but integers
    /// for individual tracks. This custom deserializer normalises both to String.
    #[serde(default, deserialize_with = "deserialize_id_as_string")]
    pub id: String,
    #[serde(default)]
    pub bitrate: i64,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub uuid: String,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub title: String,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub artist: String,
    pub cover: Option<String>,
    #[serde(default)]
    pub explicit: bool,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub downloaded: i64,
    #[serde(default)]
    pub failed: i64,
    #[serde(default)]
    pub progress: i64,
    #[serde(default)]
    pub errors: Vec<DeemixDownloadError>,
    #[serde(default)]
    pub files: Vec<DeemixDownloadedFile>,
    #[serde(
        rename = "__type__",
        default,
        deserialize_with = "deserialize_nullable_string"
    )]
    pub collection_type: String,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub status: String,
    #[serde(rename = "extrasPath")]
    pub extras_path: Option<String>,
}

/// Custom deserializer for the `id` field — deemix returns strings for playlist
/// IDs but raw integers for individual track IDs.
fn deserialize_id_as_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct IdVisitor;
    impl<'de> de::Visitor<'de> for IdVisitor {
        type Value = String;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string or integer")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<String, E> {
            Ok(v)
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_none<E: de::Error>(self) -> Result<String, E> {
            Ok(String::new())
        }

        fn visit_unit<E: de::Error>(self) -> Result<String, E> {
            Ok(String::new())
        }
    }

    deserializer.deserialize_any(IdVisitor)
}

/// Deserializer for String fields that may be `null` in the JSON.
/// Returns empty string for `null`, otherwise the string value.
fn deserialize_nullable_string<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct NullableStringVisitor;
    impl<'de> de::Visitor<'de> for NullableStringVisitor {
        type Value = String;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string or null")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<String, E> {
            Ok(v.to_string())
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<String, E> {
            Ok(v)
        }

        fn visit_none<E: de::Error>(self) -> Result<String, E> {
            Ok(String::new())
        }

        fn visit_unit<E: de::Error>(self) -> Result<String, E> {
            Ok(String::new())
        }
    }

    deserializer.deserialize_any(NullableStringVisitor)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixDownloadError {
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub message: String,
    pub data: Option<DeemixErrorData>,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub stack: String,
    #[serde(
        rename = "type",
        default,
        deserialize_with = "deserialize_nullable_string"
    )]
    pub error_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixErrorData {
    pub id: serde_json::Value,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub title: String,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub artist: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixDownloadedFile {
    pub album_urls: Option<Vec<DeemixAlbumUrl>>,
    pub album_path: Option<String>,
    pub album_filename: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub filename: String,
    pub data: Option<DeemixTrackData>,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixAlbumUrl {
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub url: String,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub ext: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixTrackData {
    pub id: serde_json::Value,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub title: String,
    #[serde(default, deserialize_with = "deserialize_nullable_string")]
    pub artist: String,
}

/// Top-level response from GET /api/getQueue
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixQueueResponse {
    pub queue: HashMap<String, DeemixQueueItem>,
    #[serde(default)]
    pub queue_order: Vec<String>,
}

/// Response from POST /api/addToQueue and POST /api/retryDownload
/// The deemix API returns HTTP 200 even on errors, with result=false and errid set.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixActionResult {
    pub result: bool,
    #[serde(default)]
    pub errid: Option<String>,
}

/// Request body for POST /api/services/deemix/auth
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixAuthRequest {
    pub arl: String,
    pub host: String,
}

/// Request body for POST /api/services/deemix/queue
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixEnqueueRequest {
    pub url: String,
}

/// Combined queue item for the frontend (local DB + remote deemix queue)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct DeemixCombinedQueueItem {
    pub id: Option<i64>,      // local DB id (null for remote-only items)
    pub uuid: Option<String>, // deemix queue UUID (null for local-only items)
    pub spotify_playlist_url: Option<String>,
    pub playlist_name: Option<String>,
    pub status: String,
    pub track_count_total: i64,
    pub track_count_downloaded: i64,
    pub error_message: Option<String>,
    pub created_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub title: Option<String>,  // from deemix queue
    pub artist: Option<String>, // from deemix queue
    pub progress: i64,          // from deemix queue (0-100)
}

/// Audio quality tier, ordered best → worst (`Stem` > `Flac` > `Mp3` > `Wav` >
/// `Other`). This matches the Backpack auto-download priority from
/// [`crate::db::default_format_priorities`] (WAV sources are never preferred).
///
/// `Ord` derives in declaration order, so `Iterator::min()` yields the *best*
/// available quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AudioQuality {
    /// Multi-track stem file (`.stem.m4a` / `.stem`).
    Stem,
    /// Lossless FLAC.
    Flac,
    /// Lossy MP3.
    Mp3,
    /// Uncompressed WAV — a real source, but never the preferred target quality.
    Wav,
    /// Any other / unrecognised format.
    Other,
}

impl AudioQuality {
    /// Detect the quality tier from a file extension (leading dot optional).
    pub fn from_extension(ext: &str) -> Self {
        let e = ext.trim_start_matches('.').to_lowercase();
        match e.as_str() {
            "stem.m4a" | "stem" => AudioQuality::Stem,
            "flac" => AudioQuality::Flac,
            "mp3" => AudioQuality::Mp3,
            "wav" => AudioQuality::Wav,
            _ => AudioQuality::Other,
        }
    }

    /// Detect the quality tier from a full file name (suffix match).
    pub fn from_filename(name: &str) -> Self {
        let lower = name.to_lowercase();
        if lower.ends_with(".stem.m4a") || lower.ends_with(".stem") {
            AudioQuality::Stem
        } else if lower.ends_with(".flac") {
            AudioQuality::Flac
        } else if lower.ends_with(".mp3") {
            AudioQuality::Mp3
        } else if lower.ends_with(".wav") {
            AudioQuality::Wav
        } else {
            AudioQuality::Other
        }
    }

    /// Best (highest-priority) quality among the given extensions.
    pub fn best_from_extensions<'a>(exts: impl Iterator<Item = &'a str>) -> Option<Self> {
        exts.map(AudioQuality::from_extension).min()
    }
}

/// Progress snapshot for a single queued download (Status-/Fortschritts-Polling).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct DeemixDownloadProgress {
    /// deemix queue UUID.
    pub uuid: String,
    /// Raw deemix status (`inQueue`, `downloading`, `completed`, `failed`, `withErrors`).
    pub status: String,
    /// Download progress percentage (0–100).
    pub progress: i64,
    /// Number of tracks downloaded so far.
    pub downloaded: i64,
    /// Total number of tracks.
    pub total: i64,
    /// Whether the item reached a terminal state (`completed` or `withErrors`).
    pub finished: bool,
    /// Whether the item has any recorded errors.
    pub has_errors: bool,
}

/// Result of verifying a completed deemix download (Download-Verifikation).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct DownloadVerification {
    /// deemix reported the item as `completed`.
    pub completed: bool,
    /// Number of files deemix reported as downloaded.
    pub file_count: usize,
    /// Best available quality among the reported files (`stem` > `flac` > `mp3`).
    pub best_quality: Option<AudioQuality>,
    /// Whether the download satisfies the target: completed + at least one file.
    pub verified: bool,
}

impl DeemixQueueItem {
    /// Verify a queue item's download result (B.1.4).
    ///
    /// The audio quality is derived from each reported file's `filename`
    /// (e.g. `Track.stem.m4a`, `Track.flac`, `Track.mp3`). `album_urls` are album
    /// artwork and are intentionally ignored for quality detection.
    pub fn verify_download(&self) -> DownloadVerification {
        let completed = self.status.eq_ignore_ascii_case("completed");
        let file_count = self.files.len();
        let best_quality = self
            .files
            .iter()
            .filter(|f| !f.filename.is_empty())
            .map(|f| AudioQuality::from_filename(&f.filename))
            .min();
        let verified = completed && file_count > 0;
        DownloadVerification {
            completed,
            file_count,
            best_quality,
            verified,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deemix_action_result_success() {
        let json = r#"{"result": true}"#;
        let result: DeemixActionResult = serde_json::from_str(json).unwrap();
        assert!(result.result);
        assert_eq!(result.errid, None);
    }

    #[test]
    fn test_deemix_action_result_failure() {
        let json = r#"{"result": false, "errid": "QUEUE_FULL"}"#;
        let result: DeemixActionResult = serde_json::from_str(json).unwrap();
        assert!(!result.result);
        assert_eq!(result.errid, Some("QUEUE_FULL".to_string()));
    }

    #[test]
    fn test_deemix_queue_response_empty() {
        let json = r#"{"queue": {}, "queue_order": []}"#;
        let result: DeemixQueueResponse = serde_json::from_str(json).unwrap();
        assert!(result.queue.is_empty());
        assert!(result.queue_order.is_empty());
    }

    #[test]
    fn test_deemix_queue_response_with_items() {
        let json = r#"{
            "queue": {
                "abc-123": {
                    "type": "spotify",
                    "id": "37i9dQZEVXcJZyENOWUFo7",
                    "bitrate": 320,
                    "uuid": "abc-123",
                    "title": "Test Playlist",
                    "artist": "Various Artists",
                    "cover": null,
                    "explicit": false,
                    "size": 50,
                    "downloaded": 30,
                    "failed": 0,
                    "progress": 60,
                    "errors": [],
                    "files": [],
                    "__type__": "playlist",
                    "status": "downloading"
                }
            },
            "queue_order": ["abc-123"]
        }"#;
        let result: DeemixQueueResponse = serde_json::from_str(json).unwrap();
        assert_eq!(result.queue.len(), 1);
        assert_eq!(result.queue_order.len(), 1);

        let item = result.queue.get("abc-123").unwrap();
        assert_eq!(item.title, "Test Playlist");
        assert_eq!(item.status, "downloading");
        assert_eq!(item.progress, 60);
        assert_eq!(item.downloaded, 30);
        assert_eq!(item.size, 50);
    }

    #[test]
    fn test_deemix_enqueue_request() {
        let req = DeemixEnqueueRequest {
            url: "https://open.spotify.com/playlist/abc".to_string(),
        };
        assert_eq!(req.url, "https://open.spotify.com/playlist/abc");
    }

    #[test]
    fn test_deemix_auth_request() {
        let req = DeemixAuthRequest {
            arl: "arl_token_here".to_string(),
            host: "http://localhost:6596".to_string(),
        };
        assert_eq!(req.arl, "arl_token_here");
        assert_eq!(req.host, "http://localhost:6596");
    }

    #[test]
    fn test_deemix_combined_queue_item() {
        let item = DeemixCombinedQueueItem {
            id: Some(42),
            uuid: Some("uuid-abc".to_string()),
            spotify_playlist_url: Some("https://open.spotify.com/playlist/xyz".to_string()),
            playlist_name: Some("Deep House".to_string()),
            status: "downloading".to_string(),
            track_count_total: 50,
            track_count_downloaded: 20,
            error_message: None,
            created_at: Some(1000000),
            updated_at: Some(1000100),
            title: Some("Deep House".to_string()),
            artist: Some("Various".to_string()),
            progress: 40,
        };

        assert_eq!(item.id, Some(42));
        assert_eq!(item.status, "downloading");
        assert_eq!(item.progress, 40);
        assert_eq!(item.track_count_total, 50);
        assert_eq!(item.track_count_downloaded, 20);
    }

    #[test]
    fn test_deemix_queue_item_defaults() {
        // Test that serde defaults work for missing fields
        let json = r#"{"type": "spotify", "id": "abc", "uuid": "def"}"#;
        let item: DeemixQueueItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.item_type, "spotify");
        assert_eq!(item.id, "abc");
        assert_eq!(item.uuid, "def");
        assert_eq!(item.bitrate, 0);
        assert_eq!(item.size, 0);
        assert_eq!(item.downloaded, 0);
        assert_eq!(item.progress, 0);
        assert!(item.errors.is_empty());
        assert!(item.files.is_empty());
        assert_eq!(item.status, "");
    }

    #[test]
    fn test_audio_quality_ordering_best_first() {
        // Derived Ord orders Stem < Flac < Mp3 < Wav < Other, so min() = best.
        assert!(AudioQuality::Stem < AudioQuality::Flac);
        assert!(AudioQuality::Flac < AudioQuality::Mp3);
        assert!(AudioQuality::Mp3 < AudioQuality::Wav);
        assert!(AudioQuality::Wav < AudioQuality::Other);
    }

    #[test]
    fn test_audio_quality_from_extension() {
        assert_eq!(AudioQuality::from_extension("stem.m4a"), AudioQuality::Stem);
        assert_eq!(AudioQuality::from_extension("flac"), AudioQuality::Flac);
        assert_eq!(AudioQuality::from_extension("mp3"), AudioQuality::Mp3);
        assert_eq!(AudioQuality::from_extension("wav"), AudioQuality::Wav);
        assert_eq!(AudioQuality::from_extension("ogg"), AudioQuality::Other);
        // leading dot tolerated
        assert_eq!(AudioQuality::from_extension(".flac"), AudioQuality::Flac);
        // case-insensitive
        assert_eq!(AudioQuality::from_extension("FLAC"), AudioQuality::Flac);
    }

    #[test]
    fn test_audio_quality_from_filename() {
        assert_eq!(
            AudioQuality::from_filename("Track.stem.m4a"),
            AudioQuality::Stem
        );
        assert_eq!(AudioQuality::from_filename("Track.flac"), AudioQuality::Flac);
        assert_eq!(AudioQuality::from_filename("Track.mp3"), AudioQuality::Mp3);
        assert_eq!(AudioQuality::from_filename("Track.wav"), AudioQuality::Wav);
        assert_eq!(AudioQuality::from_filename("Track.ogg"), AudioQuality::Other);
        // case-insensitive
        assert_eq!(AudioQuality::from_filename("TRACK.FLAC"), AudioQuality::Flac);
    }

    #[test]
    fn test_best_from_extensions_picks_stem_over_flac_mp3() {
        let best = AudioQuality::best_from_extensions(["mp3", "flac", "stem.m4a"].into_iter());
        assert_eq!(best, Some(AudioQuality::Stem));
    }

    #[test]
    fn test_best_from_extensions_empty() {
        let best = AudioQuality::best_from_extensions(std::iter::empty());
        assert_eq!(best, None);
    }

    fn file(name: &str) -> DeemixDownloadedFile {
        DeemixDownloadedFile {
            album_urls: None,
            album_path: None,
            album_filename: None,
            filename: name.to_string(),
            data: None,
            path: format!("/music/{name}"),
        }
    }

    #[test]
    fn test_verify_download_completed_flac() {
        let mut item = DeemixQueueItem {
            item_type: "spotify".into(),
            id: "abc".into(),
            bitrate: 0,
            uuid: "u".into(),
            title: "T".into(),
            artist: "A".into(),
            cover: None,
            explicit: false,
            size: 1,
            downloaded: 1,
            failed: 0,
            progress: 100,
            errors: vec![],
            files: vec![file("Track.flac")],
            collection_type: "playlist".into(),
            status: "completed".into(),
            extras_path: None,
        };
        let v = item.verify_download();
        assert!(v.completed);
        assert_eq!(v.file_count, 1);
        assert_eq!(v.best_quality, Some(AudioQuality::Flac));
        assert!(v.verified);

        // same shape but still downloading → not verified
        item.status = "downloading".into();
        let v2 = item.verify_download();
        assert!(!v2.completed);
        assert!(!v2.verified);
    }

    #[test]
    fn test_verify_download_best_quality_is_stem() {
        let item = DeemixQueueItem {
            item_type: "spotify".into(),
            id: "abc".into(),
            bitrate: 0,
            uuid: "u".into(),
            title: "T".into(),
            artist: "A".into(),
            cover: None,
            explicit: false,
            size: 3,
            downloaded: 3,
            failed: 0,
            progress: 100,
            errors: vec![],
            files: vec![
                file("Track.mp3"),
                file("Track.flac"),
                file("Track.stem.m4a"),
            ],
            collection_type: "playlist".into(),
            status: "completed".into(),
            extras_path: None,
        };
        let v = item.verify_download();
        assert!(v.verified);
        assert_eq!(v.best_quality, Some(AudioQuality::Stem));
    }

    #[test]
    fn test_verify_download_ignores_album_artwork_and_empty_files() {
        let item = DeemixQueueItem {
            item_type: "spotify".into(),
            id: "abc".into(),
            bitrate: 0,
            uuid: "u".into(),
            title: "T".into(),
            artist: "A".into(),
            cover: None,
            explicit: false,
            size: 0,
            downloaded: 0,
            failed: 0,
            progress: 0,
            errors: vec![],
            files: vec![],
            collection_type: "playlist".into(),
            status: "completed".into(),
            extras_path: None,
        };
        let v = item.verify_download();
        assert!(v.completed);
        assert_eq!(v.file_count, 0);
        assert_eq!(v.best_quality, None);
        assert!(!v.verified);
    }
}
