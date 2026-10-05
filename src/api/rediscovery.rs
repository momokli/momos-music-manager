//! Rediscovery candidates — the resurfacing queue.
//!
//! `GET /api/rediscovery/candidates` answers "what have I not touched in a long
//! time", with the *reason* for every row attached (the reason is server
//! knowledge, not a frontend concern). The primary signal is
//! `last_touched_at` from `v_track_forgotten_facts` (curated **and** liked
//! playlists), never `playlist_count` alone.
//!
//! Filtering, sorting, `total` and pagination all run server-side over one
//! batched candidate load; there is no client-side filtering after pagination.

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use rand::SeedableRng;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, ErrorResponse, internal_error};
use crate::db::rediscovery::{self, AudioFilter, FileFacts, TrackFacts};

const SORTS: &[&str] = &["oldest-touched", "forgotten", "random", "bpm", "artist"];
const MAX_LIMIT: i64 = 200;

// ── Query ────────────────────────────────────────────────────────────────

fn default_touched_before_days() -> i64 {
    365
}
fn default_true() -> bool {
    true
}
fn default_exclude_pushed_days() -> Option<i64> {
    Some(180)
}
fn default_limit() -> i64 {
    50
}
fn default_offset() -> i64 {
    0
}
fn default_sort() -> String {
    "oldest-touched".to_string()
}

/// Query parameters. All fields optional; defaults follow Issue #81.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RediscoveryCandidatesQuery {
    /// PRIMARY: `last_touched_at` older than `now - N` days.
    #[serde(default = "default_touched_before_days")]
    pub touched_before_days: i64,
    /// Secondary: `playlist_count <= N` (curated only; `null` = off).
    #[serde(default)]
    pub max_playlists: Option<i64>,
    #[serde(default)]
    pub liked_only: bool,
    #[serde(default = "default_true")]
    pub exclude_backpack: bool,
    /// Exclude tracks pushed within the last N days (ledger 033). `0` disables.
    #[serde(default = "default_exclude_pushed_days")]
    pub exclude_pushed_since_days: Option<i64>,
    #[serde(default)]
    pub play_count_max: Option<i64>,
    #[serde(default)]
    pub not_played_since_days: Option<i64>,
    #[serde(default)]
    pub require_bpm: bool,
    #[serde(default)]
    pub require_key: bool,
    #[serde(default)]
    pub bpm_min: Option<f64>,
    #[serde(default)]
    pub bpm_max: Option<f64>,
    #[serde(default)]
    pub keys: Vec<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(default = "default_offset")]
    pub offset: i64,
    #[serde(default)]
    pub seed: Option<u64>,
    #[serde(default = "default_sort")]
    pub sort: String,
}

// ── Response ─────────────────────────────────────────────────────────────

/// One candidate row. `reasons` explains *why* it surfaced, derived from the
/// facets that were actually applied (plus the always-on `last-touched` signal).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidateRow {
    pub track_id: i64,
    pub spotify_id: Option<String>,
    pub title: String,
    pub artist: String,
    pub playlist_count: i64,
    pub liked_at: Option<i64>,
    pub last_added_at: Option<i64>,
    pub touched_years_ago: f64,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub genre: Option<String>,
    pub in_backpack: bool,
    pub owned: bool,
    pub reasons: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CandidatesResponse {
    pub candidates: Vec<CandidateRow>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

// ── Router ───────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new().route("/api/rediscovery/candidates", get(candidates_handler))
}

// ── Handler ──────────────────────────────────────────────────────────────

async fn candidates_handler(
    State(state): State<Arc<AppState>>,
    Query(q): Query<RediscoveryCandidatesQuery>,
) -> Response {
    // ── Validation ──────────────────────────────────────────────────────
    if !SORTS.contains(&q.sort.as_str()) {
        return bad_request(format!(
            "Unknown sort '{}'. Valid: {}",
            q.sort,
            SORTS.join(", ")
        ));
    }
    if let (Some(min), Some(max)) = (q.bpm_min, q.bpm_max) {
        if min > max {
            return bad_request(format!(
                "bpmMin ({min}) must not be greater than bpmMax ({max})"
            ));
        }
    }
    let limit = q.limit.clamp(1, MAX_LIMIT);
    let offset = q.offset.max(0);
    let now = chrono::Utc::now().timestamp();

    // ── Batched loads (no N+1, no pro-row view access) ──────────────────
    let all = match rediscovery::list_forgotten_facts(&state.db).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("Failed to load forgotten facts: {}", e);
            return internal_error(e).into_response();
        }
    };

    let backpack: HashSet<i64> = match crate::backpack::get_backpack_track_ids(&state.db).await {
        Ok(v) => v.into_iter().collect(),
        Err(e) => return internal_error(e).into_response(),
    };

    let pushed: HashSet<i64> = match q.exclude_pushed_since_days {
        Some(days) => match rediscovery::pushed_track_ids_since(&state.db, days, now).await {
            Ok(v) => v,
            Err(e) => return internal_error(e).into_response(),
        },
        None => HashSet::new(),
    };

    let audio_filter = AudioFilter {
        bpm_min: q.bpm_min,
        bpm_max: q.bpm_max,
        require_bpm: q.require_bpm,
        require_key: q.require_key,
        keys: q.keys.clone(),
        genres: q.genres.clone(),
        play_count_max: q.play_count_max,
        not_played_before: q.not_played_since_days.map(|d| now - d * 86_400),
    };
    let audio_active = audio_filter.bpm_min.is_some()
        || audio_filter.bpm_max.is_some()
        || audio_filter.require_bpm
        || audio_filter.require_key
        || !audio_filter.keys.is_empty()
        || !audio_filter.genres.is_empty()
        || audio_filter.play_count_max.is_some()
        || audio_filter.not_played_before.is_some();
    let audio: Option<HashSet<i64>> = if audio_active {
        match rediscovery::tracks_matching_audio(&state.db, &audio_filter).await {
            Ok(set) => Some(set),
            Err(e) => return internal_error(e).into_response(),
        }
    } else {
        None
    };

    // ── Server-side facet filtering ─────────────────────────────────────
    let touched_cutoff = now - q.touched_before_days * 86_400;
    let mut survivors: Vec<&TrackFacts> = Vec::new();
    for fact in &all {
        // PRIMARY facet: last_touched_at is required and must be older.
        let Some(last_touched) = fact.last_touched_at else {
            continue;
        };
        if last_touched >= touched_cutoff {
            continue;
        }
        if let Some(max) = q.max_playlists {
            if fact.playlist_count > max {
                continue;
            }
        }
        if q.liked_only && !fact.liked {
            continue;
        }
        if q.exclude_backpack && backpack.contains(&fact.track_id) {
            continue;
        }
        if q.exclude_pushed_since_days.is_some() && pushed.contains(&fact.track_id) {
            continue;
        }
        if let Some(set) = &audio {
            if !set.contains(&fact.track_id) {
                continue;
            }
        }
        survivors.push(fact);
    }

    // ── Build, sort, paginate ───────────────────────────────────────────
    let ids: Vec<i64> = survivors.iter().map(|f| f.track_id).collect();
    let files = match rediscovery::load_file_facts(&state.db, &ids).await {
        Ok(v) => v,
        Err(e) => return internal_error(e).into_response(),
    };
    let owned = match rediscovery::owned_track_ids(&state.db, &ids).await {
        Ok(v) => v,
        Err(e) => return internal_error(e).into_response(),
    };
    let push_dates = match rediscovery::last_push_dates(&state.db, &ids).await {
        Ok(v) => v,
        Err(e) => return internal_error(e).into_response(),
    };

    let mut rows: Vec<CandidateRow> = survivors
        .iter()
        .map(|fact| {
            build_row(
                fact,
                files.get(&fact.track_id),
                owned.contains(&fact.track_id),
                &backpack,
                &q,
                now,
                &push_dates,
            )
        })
        .collect();

    sort_rows(&mut rows, &q.sort, q.seed);

    let total = rows.len() as i64;
    let start = (offset as usize).min(rows.len());
    let end = (start + limit as usize).min(rows.len());
    let mut page: Vec<CandidateRow> = rows[start..end].to_vec();

    // ── Attach service-track metadata for the page only ─────────────────
    if !page.is_empty() {
        let page_ids: Vec<i64> = page.iter().map(|r| r.track_id).collect();
        let meta = match rediscovery::track_metadata(&state.db, &page_ids).await {
            Ok(v) => v,
            Err(e) => return internal_error(e).into_response(),
        };
        for row in page.iter_mut() {
            if let Some((service_id, title, artist)) = meta.get(&row.track_id) {
                row.spotify_id = service_id.clone();
                row.title = title.clone();
                row.artist = artist.clone();
            }
        }
    }

    (
        StatusCode::OK,
        Json(ApiResponse {
            data: CandidatesResponse {
                candidates: page,
                total,
                limit,
                offset,
            },
        }),
    )
        .into_response()
}

// ── Row assembly ─────────────────────────────────────────────────────────

fn build_row(
    fact: &TrackFacts,
    files: Option<&FileFacts>,
    owned: bool,
    backpack: &HashSet<i64>,
    q: &RediscoveryCandidatesQuery,
    now: i64,
    push_dates: &HashMap<i64, i64>,
) -> CandidateRow {
    let mut reasons: Vec<String> = Vec::new();

    // 1. last-touched — the always-applied primary signal.
    if let Some(last) = fact.last_touched_at {
        reasons.push(format!("last-touched-{}", fmt_date(last)));
    }

    // 2. playlist breadth.
    if fact.playlist_count <= 1 {
        reasons.push("only-in-1-playlist".to_string());
    } else {
        reasons.push(format!("in-{}-playlists", fact.playlist_count));
    }

    // 3. push history — only meaningful while the facet is active.
    if q.exclude_pushed_since_days.is_some() {
        match push_dates.get(&fact.track_id) {
            Some(ts) => reasons.push(format!("pushed-{}", fmt_date(*ts))),
            None => reasons.push("never-pushed".to_string()),
        }
    }

    // 4. liked.
    if fact.liked {
        reasons.push("liked".to_string());
    }

    // 5. play-count cap.
    if q.play_count_max.is_some() {
        reasons.push("low-play-count".to_string());
    }

    // 6. not-played-since.
    if let Some(days) = q.not_played_since_days {
        reasons.push(format!("not-played-since-{}", fmt_date(now - days * 86_400)));
    }

    // 7. bpm range.
    if q.bpm_min.is_some() || q.bpm_max.is_some() {
        reasons.push("bpm-in-range".to_string());
    }

    // 8. key / genre set matches (report the actual matched value).
    if !q.keys.is_empty() {
        if let Some(key) = files.and_then(|f| f.musical_key.as_deref()) {
            reasons.push(format!("key-match:{key}"));
        }
    }
    if !q.genres.is_empty() {
        if let Some(genre) = files.and_then(|f| f.genre.as_deref()) {
            reasons.push(format!("genre-match:{genre}"));
        }
    }

    // 9. backpack exclusion.
    if q.exclude_backpack && !backpack.contains(&fact.track_id) {
        reasons.push("not-in-backpack".to_string());
    }

    CandidateRow {
        track_id: fact.track_id,
        spotify_id: None,
        title: String::new(),
        artist: String::new(),
        playlist_count: fact.playlist_count,
        liked_at: fact.liked_at,
        last_added_at: fact.last_touched_at,
        touched_years_ago: fact
            .last_touched_at
            .map(|t| (now - t) as f64 / 365.25)
            .unwrap_or(0.0),
        bpm: files.and_then(|f| f.bpm),
        musical_key: files.and_then(|f| f.musical_key.clone()),
        genre: files.and_then(|f| f.genre.clone()),
        in_backpack: backpack.contains(&fact.track_id),
        owned,
        reasons,
    }
}

fn fmt_date(unix_seconds: i64) -> String {
    chrono::DateTime::from_timestamp(unix_seconds, 0)
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

fn sort_rows(rows: &mut [CandidateRow], sort: &str, seed: Option<u64>) {
    // Every deterministic sort gets `track_id` as a final tiebreaker so that
    // pagination across equal keys is stable.
    match sort {
        "forgotten" => rows.sort_by(|a, b| {
            a.last_added_at
                .cmp(&b.last_added_at)
                .then(a.playlist_count.cmp(&b.playlist_count))
                .then(a.track_id.cmp(&b.track_id))
        }),
        "bpm" => rows.sort_by(|a, b| {
            match (a.bpm, b.bpm) {
                (Some(x), Some(y)) => x.partial_cmp(&y).unwrap_or(std::cmp::Ordering::Equal),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
            .then(a.track_id.cmp(&b.track_id))
        }),
        "artist" => rows.sort_by(|a, b| {
            a.artist
                .cmp(&b.artist)
                .then(a.title.cmp(&b.title))
                .then(a.track_id.cmp(&b.track_id))
        }),
        "random" => match seed {
            Some(seed) => {
                let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
                rows.shuffle(&mut rng);
            }
            None => {
                let mut rng = rand::thread_rng();
                rows.shuffle(&mut rng);
            }
        },
        // "oldest-touched" and any fallback (validated above).
        _ => rows.sort_by(|a, b| {
            a.last_added_at
                .cmp(&b.last_added_at)
                .then(a.track_id.cmp(&b.track_id))
        }),
    }
}

fn bad_request(message: String) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse { error: message }),
    )
        .into_response()
}
