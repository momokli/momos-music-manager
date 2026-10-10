//! One-off importer: seed a hub user's tag library from a Momo's Music Manager
//! (`library.db`) database — categories (with a mapped icon) and tags. Tags are
//! also linked to a hub playlist of the same name when one exists, so they get
//! their tracks.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

/// FontAwesome icon (MMM) → emoji (hub, no FA dependency).
pub fn map_icon(fa: &str) -> String {
    let f = fa.to_lowercase();
    let pick = if f.contains("list") {
        "🎼" // Setlist (fa-list / fa-list-music)
    } else if f.contains("layers") || f.contains("bolt") {
        "⚡" // Phase
    } else if f.contains("heart") {
        "💜" // Mood
    } else if f.contains("rainbow") || f.contains("sparkles") {
        "🌈" // Vibe
    } else if f.contains("hashtag") {
        "#️⃣" // Merkmal
    } else {
        ""
    };
    pick.to_string()
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
    let user_id: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM hub_users WHERE slug = ?1 COLLATE NOCASE LIMIT 1",
    )
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

    // Categories.
    let cats = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT id, name, COALESCE(icon,'') FROM tag_categories ORDER BY sort_order, id",
    )
    .fetch_all(&mmm)
    .await
    .context("read tag_categories (is this a MMM db?)")?;

    let mut cat_map: HashMap<i64, i64> = HashMap::new();
    let mut name_to_hub: HashMap<String, i64> = HashMap::new();
    let mut summary = Summary::default();
    for (mmm_cat_id, name, icon) in cats {
        let hub_id = crate::tags::create_group(hub, user_id, &name, &map_icon(&icon)).await?;
        cat_map.insert(mmm_cat_id, hub_id);
        summary.groups += 1;
    }

    // Tags.
    let tag_rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT name, category_id FROM tags ORDER BY name",
    )
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
        // The MMM category maps to a hub group (many-to-many).
        if let Some(group_id) = cat_map.get(&mmm_cat_id).copied() {
            let _ = crate::tags::add_tag_to_group(hub, user_id, tag_id, group_id).await;
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

    // Energy levels (`tag_energy_levels`, 0..5): mark the category's hub group
    // as ranked and set each tag's rank within it (e.g. Phase: start=1 … peak=5).
    let energy_rows = sqlx::query_as::<_, (String, i64, i64)>(
        "SELECT t.name, t.category_id, e.energy_level FROM tag_energy_levels e
           JOIN tags t ON t.id = e.tag_id",
    )
    .fetch_all(&mmm)
    .await
    .unwrap_or_default();
    let mut ranked_groups: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for (name, mmm_cat_id, level) in energy_rows {
        let tag_id = name_to_hub.get(&name.trim().to_lowercase()).copied();
        let group_id = cat_map.get(&mmm_cat_id).copied();
        if let (Some(tag_id), Some(group_id)) = (tag_id, group_id) {
            sqlx::query("UPDATE hub_group_tags SET rank = ?1 WHERE group_id = ?2 AND tag_id = ?3")
                .bind(level)
                .bind(group_id)
                .bind(tag_id)
                .execute(hub)
                .await?;
            ranked_groups.insert(group_id);
            summary.ranked += 1;
        }
    }
    for gid in ranked_groups {
        sqlx::query("UPDATE hub_tag_groups SET ranked = 1 WHERE id = ?1")
            .bind(gid)
            .execute(hub)
            .await?;
    }

    crate::tags::rebuild(hub).await?;
    Ok(summary)
}
