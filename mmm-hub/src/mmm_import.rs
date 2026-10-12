//! One-off importer: seed a hub user's tag library from a Momo's Music Manager
//! (`library.db`) database — categories (mapped to a tag **type**) and tags.
//! Tags are also linked to a hub playlist of the same name when one exists, so
//! they get their tracks.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

use crate::tagtypes::TagType;

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

    let url = if mmm_db.starts_with("sqlite:") {
        mmm_db.to_string()
    } else {
        format!("sqlite:{mmm_db}")
    };
    let opts = SqliteConnectOptions::new()
        .filename(url.trim_start_matches("sqlite:"))
        .read_only(true);
    let mmm = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .with_context(|| format!("open MMM db {mmm_db}"))?;

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
