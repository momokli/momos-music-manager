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
