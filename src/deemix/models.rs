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
    pub id: i64,
    pub name: String,
    pub picture: String,
    pub license_token: String,
    pub can_stream_hq: bool,
    pub can_stream_lossless: bool,
    pub country: String,
    pub language: String,
}

/// Single queue item from GET /api/getQueue (keyed by uuid in response)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixQueueItem {
    #[serde(rename = "type", default)]
    pub item_type: String,
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub bitrate: i64,
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
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
    #[serde(rename = "__type__", default)]
    pub collection_type: String,
    #[serde(default)]
    pub status: String,
    #[serde(rename = "extrasPath")]
    pub extras_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixDownloadError {
    pub message: String,
    pub data: DeemixErrorData,
    #[serde(default)]
    pub stack: String,
    #[serde(rename = "type")]
    pub error_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixErrorData {
    pub id: serde_json::Value,
    pub title: String,
    pub artist: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixDownloadedFile {
    pub album_urls: Option<Vec<DeemixAlbumUrl>>,
    pub album_path: Option<String>,
    pub album_filename: Option<String>,
    pub filename: String,
    pub data: DeemixTrackData,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixAlbumUrl {
    pub url: String,
    pub ext: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[allow(dead_code)]
pub struct DeemixTrackData {
    pub id: serde_json::Value,
    pub title: String,
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
}
