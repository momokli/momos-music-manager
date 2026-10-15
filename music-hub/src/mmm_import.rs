//! One-off importer: seed a hub user's tag library from a Momo's Music Manager
//! (`library.db`) database — categories (mapped to a tag **type**) and tags.
//! Tags are also linked to a hub playlist of the same name when one exists, so
//! they get their tracks.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::tagtypes::TagType;

/// Open a MMM `library.db` read-only (single connection). Shared by all MMM
/// importers so the connection setup stays in one place.
async fn open_mmm(mmm_db: &str) -> Result<SqlitePool> {
    let url = if mmm_db.starts_with("sqlite:") {
        mmm_db.to_string()
    } else {
        format!("sqlite:{mmm_db}")
    };
    let opts = SqliteConnectOptions::new()
        .filename(url.trim_start_matches("sqlite:"))
        .read_only(true);
    SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .with_context(|| format!("open MMM db {mmm_db}"))
}

/// Map an MMM category name to a hub tag type. Phase categories map to
/// `PHASE_3` as a base; the concrete energy level is applied per tag below.
fn type_for_category(name: &str) -> Option<TagType> {
    match name.trim().to_lowercase().as_str() {
        "mood" | "moods" => Some(TagType::Mood),
        "vibe" | "vibes" => Some(TagType::Vibe),
        "merkmal" | "merkmale" | "attribute" | "attribut" => Some(TagType::Merkmal),
        "genre" | "genres" => Some(TagType::Genre),
        "setlist" | "setlists" => Some(TagType::Setlist),
        "rumpelkiste" | "rumpel" => Some(TagType::Rumpelkiste),
        "phase" | "phase/energy" | "phase / energy" | "energy" => Some(TagType::Phase3),
        _ => None,
    }
}

#[derive(Debug, Default)]
pub struct Summary {
    pub groups: usize,
    pub tags: usize,
    pub linked: usize,
    pub skipped: usize,
    pub parents: usize,
    pub ranked: usize,
}

/// Import categories + tags from `mmm_db` into the hub user `user_slug`.
pub async fn import_tags(hub: &SqlitePool, mmm_db: &str, user_slug: &str) -> Result<Summary> {
    let user_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM hub_users WHERE slug = ?1 COLLATE NOCASE LIMIT 1")
            .bind(user_slug)
            .fetch_optional(hub)
            .await
            .ok()
            .flatten();
    let Some(user_id) = user_id else {
        bail!("hub user '{user_slug}' not found");
    };

    let mmm = open_mmm(mmm_db).await?;

    // Categories -> tag types.
    let cats = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT id, name, COALESCE(icon,'') FROM tag_categories ORDER BY sort_order, id",
    )
    .fetch_all(&mmm)
    .await
    .context("read tag_categories (is this a MMM db?)")?;

    let mut cat_type: HashMap<i64, TagType> = HashMap::new();
    let mut name_to_hub: HashMap<String, i64> = HashMap::new();
    let mut summary = Summary::default();
    for (mmm_cat_id, name, _icon) in cats {
        if let Some(ty) = type_for_category(&name) {
            cat_type.insert(mmm_cat_id, ty);
            summary.groups += 1;
        }
    }

    // Tags.
    let tag_rows =
        sqlx::query_as::<_, (String, i64)>("SELECT name, category_id FROM tags ORDER BY name")
            .fetch_all(&mmm)
            .await
            .context("read tags")?;

    for (name, mmm_cat_id) in tag_rows {
        // Skip names with no alphanumerics (e.g. pure-emoji) — they have no slug.
        if crate::tags::normalize_name(&name).is_empty() {
            summary.skipped += 1;
            continue;
        }
        let tag_id = crate::tags::ensure_tag(hub, user_id, &name).await?;
        name_to_hub.insert(name.trim().to_lowercase(), tag_id);
        summary.tags += 1;
        // The MMM category maps to the tag's hub type.
        if let Some(ty) = cat_type.get(&mmm_cat_id).copied() {
            let _ = crate::tagtypes::set(hub, user_id, tag_id, Some(ty)).await;
        }

        // Link a hub playlist owned by the user with the same name, if any.
        let pid: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM hub_playlists
              WHERE user_id = ?1 AND lower(trim(name)) = lower(trim(?2))
              ORDER BY id LIMIT 1",
        )
        .bind(user_id)
        .bind(&name)
        .fetch_optional(hub)
        .await
        .ok()
        .flatten();
        if let Some(pid) = pid {
            if crate::tags::link_playlist_to_tag(hub, user_id, tag_id, pid)
                .await
                .is_ok()
            {
                summary.linked += 1;
            }
        }
    }

    // Parent / alias tags (`tag_parents`): child tag -> parent tag, by name.
    let parent_rows = sqlx::query_as::<_, (String, String)>(
        "SELECT c.name, p.name FROM tag_parents tp
           JOIN tags c ON c.id = tp.tag_id
           JOIN tags p ON p.id = tp.parent_tag_id",
    )
    .fetch_all(&mmm)
    .await
    .unwrap_or_default();
    for (child, parent) in parent_rows {
        let child_id = name_to_hub.get(&child.trim().to_lowercase()).copied();
        let parent_id = name_to_hub.get(&parent.trim().to_lowercase()).copied();
        if let (Some(c), Some(p)) = (child_id, parent_id) {
            if crate::tags::add_tag_parent(hub, c, p).await.is_ok() {
                summary.parents += 1;
            }
        }
    }

    // Energy levels (`tag_energy_levels`, 0..5): a phase tag's type becomes
    // `PHASE_<level>` (clamped to 1..5, default 3 for phase categories).
    let energy_rows = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT t.name, t.category_id, e.energy_level FROM tag_energy_levels e
           JOIN tags t ON t.id = e.tag_id",
    )
    .fetch_all(&mmm)
    .await
    .unwrap_or_default();
    for (name, mmm_cat_id, level) in energy_rows {
        let tag_id = name_to_hub.get(&name.trim().to_lowercase()).copied();
        let Some(tag_id) = tag_id else { continue };
        let ty = if matches!(cat_type.get(&mmm_cat_id), Some(TagType::Phase3)) {
            let n = level.clamp(1, 5);
            TagType::parse(&format!("PHASE_{n}"))
        } else {
            cat_type.get(&mmm_cat_id).copied()
        };
        if let Some(ty) = ty {
            if crate::tagtypes::set(hub, user_id, tag_id, Some(ty))
                .await
                .is_ok()
            {
                summary.ranked += 1;
            }
        }
    }

    crate::tags::rebuild(hub).await?;
    Ok(summary)
}

// ── Playlists ────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct PlaylistSummary {
    /// hub_playlists rows upserted.
    pub playlists: usize,
    /// Of those, liked-mirror playlists (`is_liked = 1`).
    pub liked: usize,
    /// hub_playlist_tracks rows upserted.
    pub linked: usize,
    /// MMM memberships resolved to a hub track.
    pub matched: usize,
    /// MMM memberships with no matching hub track (left out).
    pub unmatched: usize,
    /// `playlist_kind = 'generated'` playlists skipped.
    pub skipped_playlists: usize,
    /// Archived memberships (`deleted_at IS NOT NULL`) skipped.
    pub skipped_tracks: usize,
}

/// Import service playlists + memberships from `mmm_db` into the hub user
/// `user_slug`. Idempotent via `UNIQUE(user_id, service, playlist_id)` and the
/// `(playlist_id, track_id)` membership PK. Tracks are matched by
/// `(service, service_track_id)`; unmatched memberships are counted, not
/// written.
pub async fn import_playlists(
    hub: &SqlitePool,
    mmm_db: &str,
    user_slug: &str,
) -> Result<PlaylistSummary> {
    let user_id: Option<i64> =
        sqlx::query_scalar("SELECT id FROM hub_users WHERE slug = ?1 COLLATE NOCASE LIMIT 1")
            .bind(user_slug)
            .fetch_optional(hub)
            .await
            .ok()
            .flatten();
    let Some(user_id) = user_id else {
        bail!("hub user '{user_slug}' not found");
    };

    let mmm = open_mmm(mmm_db).await?;

    let playlists = sqlx::query_as::<_, (i64, String, String, String, String, String)>(
        "SELECT id, service, playlist_id, name, COALESCE(description,''), \
                COALESCE(playlist_kind,'curated')
           FROM service_playlists ORDER BY id",
    )
    .fetch_all(&mmm)
    .await
    .context("read service_playlists (is this a MMM db?)")?;

    let now = chrono::Utc::now().to_rfc3339();
    let mut summary = PlaylistSummary::default();

    for (mmm_pl_id, service, playlist_id, name, description, kind) in playlists {
        // Local generated packs are MMM output, not source data.
        if kind == "generated" {
            summary.skipped_playlists += 1;
            continue;
        }
        let is_liked = kind == "liked";

        // Archived memberships are skipped (counted, not written).
        let archived: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM service_playlist_tracks
              WHERE playlist_id = ?1 AND deleted_at IS NOT NULL",
        )
        .bind(mmm_pl_id)
        .fetch_one(&mmm)
        .await
        .unwrap_or(0);
        summary.skipped_tracks += archived as usize;

        // Active memberships, in playlist order, joined to their service track.
        let members = sqlx::query_as::<_, (String, String, Option<i64>)>(
            "SELECT st.service, st.service_id, spt.position
               FROM service_playlist_tracks spt
               JOIN service_tracks st ON st.id = spt.track_id
              WHERE spt.playlist_id = ?1 AND spt.deleted_at IS NULL
              ORDER BY spt.position",
        )
        .bind(mmm_pl_id)
        .fetch_all(&mmm)
        .await
        .unwrap_or_default();

        // Resolve each membership to a hub track by (service, service_track_id).
        let mut resolved: Vec<(i64, Option<i64>)> = Vec::new();
        for (svc, sid, position) in members {
            let hub_track: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM hub_tracks
                  WHERE service = ?1 AND service_track_id = ?2 LIMIT 1",
            )
            .bind(&svc)
            .bind(&sid)
            .fetch_optional(hub)
            .await
            .ok()
            .flatten();
            match hub_track {
                Some(tid) => {
                    summary.matched += 1;
                    resolved.push((tid, position));
                }
                None => summary.unmatched += 1,
            }
        }

        // Upsert the playlist (idempotent on the UNIQUE constraint).
        let items_available = resolved.len() as i64;
        let hub_pl_id: i64 = sqlx::query_scalar(
            "INSERT INTO hub_playlists
                (user_id, service, playlist_id, name, description, is_liked, items_available, fetched_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(user_id, service, playlist_id) DO UPDATE SET
                name            = excluded.name,
                description     = excluded.description,
                is_liked        = excluded.is_liked,
                items_available = excluded.items_available,
                fetched_at      = excluded.fetched_at
             RETURNING id",
        )
        .bind(user_id)
        .bind(&service)
        .bind(&playlist_id)
        .bind(&name)
        .bind(&description)
        .bind(is_liked as i64)
        .bind(items_available)
        .bind(&now)
        .fetch_one(hub)
        .await?;
        summary.playlists += 1;
        if is_liked {
            summary.liked += 1;
        }

        // Upsert memberships on the (playlist_id, track_id) PK.
        for (track_id, position) in resolved {
            sqlx::query(
                "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position, added_at)
                 VALUES (?1, ?2, ?3, NULL)
                 ON CONFLICT(playlist_id, track_id) DO UPDATE SET position = excluded.position",
            )
            .bind(hub_pl_id)
            .bind(track_id)
            .bind(position)
            .execute(hub)
            .await?;
            summary.linked += 1;
        }
    }

    Ok(summary)
}

// ── Settings ──────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct SettingsSummary {
    pub imported: usize,
    pub skipped_operational: usize,
    pub skipped_unknown: usize,
}

/// MMM settings key → `hub_settings` key (whitelist). Only non-operational,
/// non-secret keys are migrated; every imported key gets a `mmm.` prefix so it
/// cannot collide with hub-native settings.
const SETTINGS_MAP: &[(&str, &str)] = &[
    ("backpack.sync_enabled", "mmm.backpack_sync_enabled"),
    (
        "spotify.liked_sync_interval_secs",
        "mmm.liked_sync_interval_secs",
    ),
    ("bpmkey.enabled", "mmm.bpmkey.enabled"),
    ("bpmkey.name_template", "mmm.bpmkey.name_template"),
    ("bpmkey.name_prefix", "mmm.bpmkey.name_prefix"),
    ("bpmkey.min_tracks", "mmm.bpmkey.min_tracks"),
    ("bpmkey.public", "mmm.bpmkey.public"),
    ("bpmkey.key_style", "mmm.bpmkey.key_style"),
    ("bpmkey.strict", "mmm.bpmkey.strict"),
    ("bpmkey.schedule_enabled", "mmm.bpmkey.schedule_enabled"),
    (
        "bpmkey.schedule_interval_secs",
        "mmm.bpmkey.schedule_interval_secs",
    ),
];

/// Operational keys are desktop/runtime state, not user preferences.
fn is_operational(key: &str) -> bool {
    key.starts_with("autoupdate.") || key.starts_with("telemetry.") || key.contains("last_")
}

/// Resolve the whitelist mapping without writing anything (used by `--dry-run`).
/// Returns the mapped `(hub_key, value)` rows plus the skip counters.
pub async fn plan_settings(mmm_db: &str) -> Result<(Vec<(String, String)>, SettingsSummary)> {
    let mmm = open_mmm(mmm_db).await?;
    let rows = sqlx::query_as::<_, (String, String)>("SELECT key, value FROM settings ORDER BY key")
        .fetch_all(&mmm)
        .await
        .context("read settings (is this a MMM db?)")?;

    let mut out = Vec::new();
    let mut summary = SettingsSummary::default();
    for (key, value) in rows {
        if let Some((_, hub_key)) = SETTINGS_MAP.iter().find(|(m, _)| *m == key) {
            out.push(((*hub_key).to_string(), value));
            summary.imported += 1;
        } else if is_operational(&key) {
            summary.skipped_operational += 1;
        } else {
            summary.skipped_unknown += 1;
        }
    }
    Ok((out, summary))
}

/// Import non-operational settings from `mmm_db` into `hub_settings`
/// (idempotent upsert). Secrets/tokens are never copied — only whitelisted keys
/// are considered, and MMM keeps tokens in `service_config`, not here.
pub async fn import_settings(hub: &SqlitePool, mmm_db: &str) -> Result<SettingsSummary> {
    let (rows, summary) = plan_settings(mmm_db).await?;
    let now = chrono::Utc::now().to_rfc3339();
    for (key, value) in rows {
        sqlx::query(
            "INSERT INTO hub_settings (key, value, updated_at)
             VALUES (?1, ?2, ?3)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(&key)
        .bind(&value)
        .bind(&now)
        .execute(hub)
        .await?;
    }
    Ok(summary)
}
