use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post, put},
};
use serde::{Deserialize, Serialize};
use sqlx::{Row, Sqlite};
use std::sync::Arc;

use crate::AppState;
use crate::api::types::{ApiResponse, ErrorResponse, internal_error};
use crate::db::{
    File, PlaylistSubscription, compute_target_comment, create_tags_from_playlists,
    delete_playlist, get_playlists_without_tags, list_subscriptions, refresh_file_resolved_tags,
    refresh_track_resolved_tags, set_playlist_archive_deleted, subscribe_to_playlist,
    unsubscribe_from_playlist,
};

// ── Types ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistTagInfo {
    pub playlist_name: String,
    pub tag_name: String,
    pub category: String,
    pub prefix: String,
    pub icon: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackFormatInfo {
    pub file_type: String,
    pub local: bool,
    pub backup: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateLocalPlaylistRequest {
    pub name: String,
    #[serde(alias = "fileIds")]
    pub track_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionStatus {
    pub id: i64,
    pub service: String,
    pub playlist_id: String,
    pub service_playlist_id: Option<i64>,
    pub playlist_name: Option<String>,
    pub track_count: i64,
    pub subscribed_at: i64,
    pub last_polled_at: Option<i64>,
    pub poll_interval_secs: i64,
    pub is_active: bool,
}

impl From<PlaylistSubscription> for SubscriptionStatus {
    fn from(s: PlaylistSubscription) -> Self {
        SubscriptionStatus {
            id: s.id,
            service: s.service,
            playlist_id: s.playlist_id,
            service_playlist_id: s.service_playlist_id,
            playlist_name: s.playlist_name,
            track_count: s.track_count,
            subscribed_at: s.subscribed_at,
            last_polled_at: s.last_polled_at,
            poll_interval_secs: s.poll_interval_secs,
            is_active: s.is_active,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: i64,
    pub service: String,
    pub playlist_id: String,
    pub name: String,
    pub description: Option<String>,
    pub track_count: i64,
    pub remote_track_count: i64,
    pub remote_unique_count: i64,
    pub last_fetched_at: Option<i64>,
    pub imported_at: i64,
    pub updated_at: i64,
    pub metadata_json: Option<String>,
    pub tag_name: Option<String>,
    pub archive_deleted: bool,
    pub total_track_count: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistsQuery {
    pub limit: Option<i64>,
    pub offset: Option<i64>,
    pub search: Option<String>,
    pub service: Option<String>,
    pub sort: Option<String>,
    pub order: Option<String>,
    pub page_size: Option<i64>,
    pub categories: Option<String>, // comma-separated category IDs: 1,2,3,4,5
    pub subscribed: Option<bool>,   // true = only subscribed, false = only unsubscribed
    pub stale: Option<bool>,        // true = only playlists where local < remote_unique
    pub archive: Option<String>,    // archived/active/all
    pub untagged: Option<bool>,     // true = only playlists without matching tags
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistWithoutTag {
    pub id: i64,
    pub service: String,
    pub name: String,
    pub playlist_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistsWithoutTagsResponse {
    pub playlists: Vec<PlaylistWithoutTag>,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateTagsFromPlaylistsResponse {
    pub created: usize,
    pub message: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubscribeRequest {
    service: String,
    playlist_id: String,
}

// ── Handlers ──────────────────────────────────────────────────────────────

pub(super) async fn get_playlists_without_tags_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match get_playlists_without_tags(&state.db).await {
        Ok(playlists) => {
            // Convert ServicePlaylist to PlaylistWithoutTag
            let playlists_without_tags: Vec<PlaylistWithoutTag> = playlists
                .into_iter()
                .map(|p| PlaylistWithoutTag {
                    id: p.id,
                    service: p.service,
                    name: p.name,
                    playlist_id: p.playlist_id,
                })
                .collect();

            let count = playlists_without_tags.len();
            let response = PlaylistsWithoutTagsResponse {
                playlists: playlists_without_tags,
                count,
            };

            Json(ApiResponse { data: response }).into_response()
        }
        Err(e) => internal_error(e).into_response(),
    }
}

pub(super) async fn create_tags_from_playlists_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match create_tags_from_playlists(&state.db).await {
        Ok(created) => {
            let response = CreateTagsFromPlaylistsResponse {
                created,
                message: format!("Created {} tags from playlists", created),
            };
            // Auto-recompute embeddings and similarities for new tags
            if created > 0 {
                crate::tasks::start_recompute_embeddings_task(&state.task_manager, &state.db).await;
                // Refresh materialized file_resolved_tags since new tag→playlist links were created
                let _ = refresh_file_resolved_tags(&state.db).await;
                let _ = refresh_track_resolved_tags(&state.db).await;
            }
            Json(ApiResponse { data: response }).into_response()
        }
        Err(e) => internal_error(e).into_response(),
    }
}

async fn create_local_playlist_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateLocalPlaylistRequest>,
) -> impl IntoResponse {
    use uuid::Uuid;

    let pool = &state.db;

    // Validate
    if request.name.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "Playlist name cannot be empty"
            })),
        )
            .into_response();
    }
    if request.track_ids.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "At least one track ID required"
            })),
        )
            .into_response();
    }

    // Create the local playlist
    let playlist_id_str = format!("local-{}", Uuid::new_v4());
    let playlist_result = sqlx::query(
        "INSERT INTO service_playlists (service, playlist_id, name, imported_at, updated_at) VALUES ('local', ?, ?, unixepoch(), unixepoch())"
    )
    .bind(&playlist_id_str)
    .bind(&request.name)
    .execute(pool)
    .await;

    let playlist_id = match playlist_result {
        Ok(r) => r.last_insert_rowid(),
        Err(e) => {
            tracing::error!("Failed to create playlist: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to create playlist"
                })),
            )
                .into_response();
        }
    };

    // Link tracks to playlist
    for track_id in &request.track_ids {
        if let Err(e) = sqlx::query(
            "INSERT OR IGNORE INTO service_playlist_tracks (playlist_id, track_id, added_at) VALUES (?, ?, unixepoch())"
        )
        .bind(playlist_id)
        .bind(track_id)
        .execute(pool)
        .await
        {
            tracing::error!("Failed to add track {} to playlist {}: {}", track_id, playlist_id, e);
        }
    }

    // Refresh materialized tables since new playlist→track links were created
    let _ = refresh_file_resolved_tags(pool).await;
    let _ = refresh_track_resolved_tags(pool).await;

    let response = serde_json::json!({
        "playlistId": playlist_id,
        "trackCount": request.track_ids.len(),
    });

    (StatusCode::OK, Json(ApiResponse { data: response })).into_response()
}

/// Toggle the archive_deleted flag for a playlist.
/// When enabled, deleted tracks are still considered active for tag resolution.
async fn toggle_playlist_archive_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    let archive = body
        .get("archiveDeleted")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    match set_playlist_archive_deleted(&state.db, id, archive).await {
        Ok(()) => {
            Json(serde_json::json!({"data": {"id": id, "archiveDeleted": archive}})).into_response()
        }
        Err(e) => internal_error(e).into_response(),
    }
}

async fn delete_playlist_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match delete_playlist(&state.db, id).await {
        Ok(true) => Json(ApiResponse {
            data: serde_json::json!({"deleted": true, "id": id}),
        })
        .into_response(),
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Playlist not found".to_string(),
            }),
        )
            .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

// Get paginated playlists from all services
async fn playlists_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PlaylistsQuery>,
) -> impl IntoResponse {
    use sqlx::QueryBuilder;

    // Default values
    let limit = query.page_size.or(query.limit).unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    let search_term = query.search.clone();
    let service_filter = query.service.clone();

    // Build main query with bind parameters (no string interpolation)
    // LEFT JOIN v_tag_playlist with DISTINCT subquery to avoid cartesian product
    // when multiple tags match the same playlist via case-insensitive name matching.
    let mut main_builder = QueryBuilder::new(
        "SELECT sp.*, \n               COUNT(CASE WHEN spt.deleted_at IS NULL THEN 1 END) as track_count, \n               COUNT(spt.track_id) as total_track_count,\n               vtp.tag_name\n         FROM service_playlists sp\n         LEFT JOIN service_playlist_tracks spt ON sp.id = spt.playlist_id\n         LEFT JOIN (SELECT DISTINCT playlist_id, tag_name FROM v_tag_playlist) vtp ON vtp.playlist_id = sp.id",
    );

    let mut count_builder =
        QueryBuilder::new("SELECT COUNT(DISTINCT sp.id) FROM service_playlists sp");

    let mut has_where = false;

    if let Some(ref service) = service_filter {
        let clause = " WHERE sp.service = ";
        main_builder.push(clause);
        main_builder.push_bind(service.clone());
        count_builder.push(clause);
        count_builder.push_bind(service.clone());
        has_where = true;
    }

    if let Some(ref search) = search_term
        && !search.trim().is_empty()
    {
        let clause = if has_where {
            " AND (sp.name LIKE "
        } else {
            " WHERE (sp.name LIKE "
        };
        let search_pat = format!("%{}%", search);
        main_builder.push(clause);
        main_builder.push_bind(search_pat.clone());
        main_builder.push(" OR sp.service LIKE ");
        main_builder.push_bind(search_pat.clone());
        main_builder.push(")");
        if has_where {
            count_builder.push(" AND (sp.name LIKE ");
        } else {
            count_builder.push(" WHERE (sp.name LIKE ");
        }
        count_builder.push_bind(search_pat.clone());
        count_builder.push(" OR sp.service LIKE ");
        count_builder.push_bind(search_pat);
        count_builder.push(")");
        has_where = true;
    }

    // Category filter using v_playlist_tag_category view (category IDs)
    // NOTE: push() treats ? as literal SQL, NOT as a bind placeholder.
    // We push the prefix + paren, then push_bind each value (which emits its own ?),
    // then close the paren. See commit 44ca2b8 for the same fix on PMV filter.
    if let Some(ref cats) = query.categories {
        let cat_ids: Vec<i64> = cats
            .split(',')
            .filter_map(|s| s.trim().parse::<i64>().ok())
            .filter(|id| *id > 0)
            .collect();
        if !cat_ids.is_empty() {
            main_builder
                .push(" LEFT JOIN v_playlist_tag_category vptc ON vptc.playlist_id = sp.id");
            count_builder
                .push(" LEFT JOIN v_playlist_tag_category vptc ON vptc.playlist_id = sp.id");

            let clause = if has_where {
                " AND vptc.category_id IN ("
            } else {
                " WHERE vptc.category_id IN ("
            };
            main_builder.push(clause);
            count_builder.push(clause);
            for (i, id) in cat_ids.iter().enumerate() {
                if i > 0 {
                    main_builder.push(", ");
                    count_builder.push(", ");
                }
                main_builder.push_bind(*id);
                count_builder.push_bind(*id);
            }
            main_builder.push(")");
            count_builder.push(")");
            has_where = true;
        }
    }

    // Subscription filter
    if let Some(subscribed) = query.subscribed {
        let clause = if has_where { " AND " } else { " WHERE " };
        if subscribed {
            let sub = "EXISTS (SELECT 1 FROM playlist_subscriptions ps WHERE ps.service = sp.service AND ps.playlist_id = sp.playlist_id)";
            main_builder.push(format!("{}{}", clause, sub));
            count_builder.push(format!("{}{}", clause, sub));
        } else {
            let sub = "NOT EXISTS (SELECT 1 FROM playlist_subscriptions ps WHERE ps.service = sp.service AND ps.playlist_id = sp.playlist_id)";
            main_builder.push(format!("{}{}", clause, sub));
            count_builder.push(format!("{}{}", clause, sub));
        }
        has_where = true;
    }

    // Stale filter: local track count < remote_unique_count
    if let Some(true) = query.stale {
        let stale_clause = if has_where { " AND " } else { " WHERE " };
        let stale_sub = "(SELECT COUNT(*) FROM service_playlist_tracks spt2 WHERE spt2.playlist_id = sp.id) < sp.remote_unique_count";
        main_builder.push(format!("{}{}", stale_clause, stale_sub));
        count_builder.push(format!("{}{}", stale_clause, stale_sub));
    }

    // Archive filter: archived (archive_deleted = true), active (archive_deleted = false), all
    if let Some(ref archive_filter) = query.archive {
        let clause = if has_where { " AND " } else { " WHERE " };
        match archive_filter.as_str() {
            "archived" => {
                main_builder.push(format!("{}sp.archive_deleted = 1", clause));
                count_builder.push(format!("{}sp.archive_deleted = 1", clause));
            }
            "active" => {
                main_builder.push(format!("{}sp.archive_deleted = 0", clause));
                count_builder.push(format!("{}sp.archive_deleted = 0", clause));
            }
            _ => {} // "all" — no filter
        }
    }

    // Untagged filter: only playlists that have no matching tag via v_tag_playlist
    if let Some(true) = query.untagged {
        let clause = if has_where { " AND " } else { " WHERE " };
        let sub = "NOT EXISTS (SELECT 1 FROM v_tag_playlist vtp WHERE vtp.playlist_id = sp.id)";
        main_builder.push(format!("{}{}", clause, sub));
        count_builder.push(format!("{}{}", clause, sub));
    }

    main_builder.push(" GROUP BY sp.id");

    // Dynamic sort with whitelist + column name mapping
    let sort_col_short = query.sort.as_deref().unwrap_or("name");
    let sort_col = match sort_col_short {
        "track_count" => "track_count",
        "service" => "sp.service",
        "imported_at" => "sp.imported_at",
        "updated_at" => "sp.updated_at",
        _ => "sp.name", // default: sort by name
    };
    let ord = match query.order.as_deref() {
        Some("desc") => "DESC",
        _ => "ASC",
    };
    main_builder.push(format!(" ORDER BY {} {}", sort_col, ord).as_str());

    main_builder.push(" LIMIT ");
    main_builder.push_bind(limit);
    main_builder.push(" OFFSET ");
    main_builder.push_bind(offset);

    // Execute main query
    let playlists = match main_builder
        .build_query_as::<Playlist>()
        .fetch_all(&state.db)
        .await
    {
        Ok(p) => p,
        Err(e) => {
            return internal_error(format!("Failed to fetch playlists: {}", e)).into_response();
        }
    };

    // Execute count query
    let total = match count_builder
        .build_query_scalar::<i64>()
        .fetch_one(&state.db)
        .await
    {
        Ok(t) => t,
        Err(e) => {
            return internal_error(format!("Failed to get total count: {}", e)).into_response();
        }
    };

    // Get deemix status via SQL LEFT JOIN (matches playlist_id in URL)
    let playlist_ids: Vec<String> = playlists.iter().map(|p| p.playlist_id.clone()).collect();
    // Single query with IN clause to find all deemix matches
    let mut deemix_statuses: std::collections::HashMap<String, (Option<String>, Option<i64>)> =
        std::collections::HashMap::new();

    // Build placeholders for IN clause
    let placeholders: Vec<String> = playlist_ids.iter().map(|_| "?".to_string()).collect();
    if !placeholders.is_empty() {
        let sql = format!(
            "SELECT sp.playlist_id, dd.status, dd.id
             FROM service_playlists sp
             LEFT JOIN deemix_downloads dd ON dd.spotify_playlist_url = 'https://open.spotify.com/playlist/' || sp.playlist_id
             WHERE sp.playlist_id IN ({})",
            placeholders.join(",")
        );
        let mut q = sqlx::query(&sql);
        for pid in &playlist_ids {
            q = q.bind(pid);
        }
        if let Ok(rows) = q.fetch_all(&state.db).await {
            for row in rows {
                let pid: String = row.try_get("playlist_id").unwrap_or_default();
                let status: Option<String> = row.try_get("status").ok();
                let dd_id: Option<i64> = row.try_get("id").ok();
                deemix_statuses.insert(pid, (status, dd_id));
            }
        }
    }

    // Fallback: check live deemix queue for playlists not found in local table
    let now = chrono::Utc::now().timestamp();
    if let Some(client) = super::deemix_api::load_deemix_client_from_db(&state.db).await
        && let Ok(remote_queue) = client.get_queue().await
    {
        for p in &playlists {
            if deemix_statuses.contains_key(&p.playlist_id) {
                continue;
            }
            for item in remote_queue.values() {
                if item.id == p.playlist_id {
                    let status = match item.status.as_str() {
                        "completed" | "withErrors" => "completed",
                        "queued" => "queued",
                        "downloading" => "downloading",
                        _ => "queued",
                    };
                    deemix_statuses.insert(p.playlist_id.clone(), (Some(status.to_string()), None));
                    // Backfill into local table for future lookups
                    let url = format!("https://open.spotify.com/playlist/{}", item.id);
                    let _ = sqlx::query(
                        "INSERT OR IGNORE INTO deemix_downloads (spotify_playlist_url, playlist_name, status, track_count_total, track_count_downloaded, created_at, updated_at)
                         VALUES (?, ?, ?, ?, ?, ?, ?)",
                    )
                    .bind(&url)
                    .bind(&item.title)
                    .bind(status)
                    .bind(item.size)
                    .bind(item.downloaded)
                    .bind(now)
                    .bind(now)
                    .execute(&state.db)
                    .await;
                    break;
                }
            }
        }
    }

    // Build enriched playlist objects with deemix status
    let playlists_with_deemix: Vec<serde_json::Value> = playlists
        .iter()
        .map(|p| {
            let (deemix_status, deemix_id) = deemix_statuses
                .get(&p.playlist_id)
                .cloned()
                .unwrap_or((None, None));
            serde_json::json!({
                "id": p.id,
                "service": p.service,
                "playlistId": p.playlist_id,
                "name": p.name,
                "description": p.description,
                "trackCount": p.track_count,
                "localTrackCount": p.track_count,
                "totalTrackCount": p.total_track_count,
                "remoteTrackCount": p.remote_track_count,
                "remoteUniqueCount": p.remote_unique_count,
                "lastFetchedAt": p.last_fetched_at,
                "importedAt": p.imported_at,
                "updatedAt": p.updated_at,
                "metadataJson": p.metadata_json,
                "tagName": p.tag_name,
                "archiveDeleted": p.archive_deleted,
                "deemixStatus": deemix_status,
                "deemixId": deemix_id,
            })
        })
        .collect();

    Json(ApiResponse {
        data: serde_json::json!({
            "playlists": playlists_with_deemix,
            "total": total,
            "limit": limit,
            "offset": offset,
        }),
    })
    .into_response()
}

async fn playlist_detail_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    // Query playlist by id with track count
    let row = sqlx::query(
        "SELECT sp.*, COUNT(spt.track_id) as track_count
         FROM service_playlists sp
         LEFT JOIN service_playlist_tracks spt ON spt.playlist_id = sp.id
         WHERE sp.id = ?
         GROUP BY sp.id",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await;

    match row {
        Ok(Some(row)) => {
            let playlist = serde_json::json!({
                "id": row.try_get::<i64, _>("id").unwrap_or(0),
                "service": row.try_get::<String, _>("service").unwrap_or_default(),
                "playlistId": row.try_get::<String, _>("playlist_id").unwrap_or_default(),
                "name": row.try_get::<String, _>("name").unwrap_or_default(),
                "description": row.try_get::<Option<String>, _>("description").unwrap_or(None),
                "trackCount": row.try_get::<i64, _>("track_count").unwrap_or(0),
                "importedAt": row.try_get::<i64, _>("imported_at").unwrap_or(0),
                "updatedAt": row.try_get::<i64, _>("updated_at").unwrap_or(0),
            });
            Json(ApiResponse { data: playlist }).into_response()
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse {
                data: serde_json::json!({"error": "Playlist not found"}),
            }),
        )
            .into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}

async fn playlist_tracks_handler(
    State(_state): State<Arc<AppState>>,
    Path(playlist_id): Path<i64>,
) -> impl IntoResponse {
    // TODO: Implement fetching tracks for playlist — query service_playlist_tracks for given playlist_id
    Json(ApiResponse {
        data: format!(
            "Playlist tracks endpoint not implemented for playlist_id: {}",
            playlist_id
        ),
    })
    .into_response()
}

async fn add_track_to_playlist_handler(
    State(_state): State<Arc<AppState>>,
    Path(playlist_id): Path<i64>,
) -> impl IntoResponse {
    // TODO: Implement adding track to playlist — insert into service_playlist_tracks
    Json(ApiResponse {
        data: format!(
            "Add track to playlist endpoint not implemented for playlist_id: {}",
            playlist_id
        ),
    })
    .into_response()
}

/// GET /api/playlists/subscriptions
async fn subscriptions_list_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let subscriptions = match list_subscriptions(&state.db).await {
        Ok(subs) => subs,
        Err(e) => {
            return internal_error(format!("Failed to list subscriptions: {}", e)).into_response();
        }
    };

    let statuses: Vec<SubscriptionStatus> = subscriptions
        .into_iter()
        .map(SubscriptionStatus::from)
        .collect();

    Json(ApiResponse { data: statuses }).into_response()
}

/// POST /api/playlists/subscriptions
async fn subscribe_handler(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SubscribeRequest>,
) -> impl IntoResponse {
    match subscribe_to_playlist(
        &state.db,
        &body.service,
        &body.playlist_id,
        None,
    )
    .await
    {
        Ok(id) => Json(ApiResponse {
            data: serde_json::json!({"id": id, "service": body.service, "playlistId": body.playlist_id}),
        })
        .into_response(),
        Err(e) => internal_error(format!("Failed to subscribe: {}", e)).into_response(),
    }
}

/// DELETE /api/playlists/subscriptions/{id}
async fn unsubscribe_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> impl IntoResponse {
    match unsubscribe_from_playlist(&state.db, id).await {
        Ok(()) => Json(ApiResponse {
            data: serde_json::json!({"unsubscribed": true}),
        })
        .into_response(),
        Err(e) => internal_error(format!("Failed to unsubscribe: {}", e)).into_response(),
    }
}

/// GET /api/playlists/comment-diff-stats
/// For each subscribed playlist, count of linked files needing comment updates
async fn playlist_comment_diff_stats_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    use std::collections::HashMap;

    // 1. Get all subscribed playlists
    let subscriptions = match list_subscriptions(&state.db).await {
        Ok(subs) => subs,
        Err(e) => {
            return internal_error(format!("Failed to list subscriptions: {}", e)).into_response();
        }
    };

    if subscriptions.is_empty() {
        return Json(ApiResponse {
            data: serde_json::json!({ "playlists": [], "total": 0 }),
        })
        .into_response();
    }

    // 2. Get all files
    let files = sqlx::query_as::<_, File>("SELECT * FROM files")
        .fetch_all(&state.db)
        .await;

    let files = match files {
        Ok(f) => f,
        Err(e) => {
            return internal_error(format!("Failed to fetch files: {}", e)).into_response();
        }
    };

    // 3. Build map: playlist_id -> count
    let mut playlist_counts: HashMap<i64, i64> = HashMap::new();

    for file in &files {
        let needs_update = match compute_target_comment(&state.db, file.id).await {
            Ok(target) => file.comment.as_deref().unwrap_or("") != target,
            Err(_) => false,
        };

        if !needs_update {
            continue;
        }

        let playlist_rows = sqlx::query_as::<_, (i64,)>(
            r#"SELECT DISTINCT sp.id
             FROM service_playlists sp
             JOIN service_playlist_tracks spt ON spt.playlist_id = sp.id
             JOIN v_file_track_link v ON v.track_id = spt.track_id
             WHERE v.file_id = ?"#,
        )
        .bind(file.id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

        for (playlist_id,) in playlist_rows {
            *playlist_counts.entry(playlist_id).or_insert(0) += 1i64;
        }
    }

    // 4. Build response: only subscribed playlists, with names
    let sub_map: HashMap<i64, &PlaylistSubscription> = subscriptions
        .iter()
        .filter_map(|s| s.service_playlist_id.map(|id| (id, s)))
        .collect();

    let mut result: Vec<serde_json::Value> = Vec::new();
    for (playlist_id, count) in &playlist_counts {
        if let Some(sub) = sub_map.get(playlist_id) {
            result.push(serde_json::json!({"subscriptionId": sub.id, "playlistId": sub.playlist_id, "playlistName": sub.playlist_name, "service": sub.service, "filesNeedingUpdate": count}));
        }
    }

    // Sort by count descending
    result.sort_by(|a, b| {
        let a_count = a["filesNeedingUpdate"].as_i64().unwrap_or(0);
        let b_count = b["filesNeedingUpdate"].as_i64().unwrap_or(0);
        b_count.cmp(&a_count)
    });

    Json(ApiResponse {
        data: serde_json::json!({"playlists": result, "total": result.len()}),
    })
    .into_response()
}

// ── Router ────────────────────────────────────────────────────────────────

pub(super) fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/playlists", get(playlists_handler))
        .route("/api/playlists/local", post(create_local_playlist_handler))
        .route(
            "/api/playlists/subscriptions",
            get(subscriptions_list_handler),
        )
        .route("/api/playlists/subscriptions", post(subscribe_handler))
        .route(
            "/api/playlists/subscriptions/{id}",
            delete(unsubscribe_handler),
        )
        .route(
            "/api/playlists/comment-diff-stats",
            get(playlist_comment_diff_stats_handler),
        )
        .route(
            "/api/playlists/without-tags",
            get(get_playlists_without_tags_handler),
        )
        .route(
            "/api/playlists/create-tags",
            post(create_tags_from_playlists_handler),
        )
        .route(
            "/api/playlists/{id}",
            get(playlist_detail_handler).delete(delete_playlist_handler),
        )
        .route(
            "/api/playlists/{id}/tracks",
            get(playlist_tracks_handler).post(add_track_to_playlist_handler),
        )
        .route(
            "/api/playlists/{id}/archive",
            put(toggle_playlist_archive_handler),
        )
}
