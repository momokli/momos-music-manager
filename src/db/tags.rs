//! Tag-related database queries — CRUD, categories, parents, curation, embeddings, backpack.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Pool, Row, Sqlite};

use super::types::*;

// ── Exported types from tag domain ───────────────────────────────────────

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceConnections {
    pub spotify: bool,
    pub soundcloud: bool,
    pub youtube: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, FromRow)]
pub struct CurationTag {
    pub id: i64,
    pub name: String,
    pub file_count: i64,
    pub parent_count: i64,
    pub category_id: i64,
    pub category: String,
    pub category_icon: String,
    pub parents_json: String,
}

// ── Tag CRUD ─────────────────────────────────────────────────────────────

pub async fn get_tag_categories(pool: &Pool<Sqlite>) -> Result<Vec<TagCategory>> {
    let categories =
        sqlx::query_as::<_, TagCategory>("SELECT * FROM v_tag_categories ORDER BY sort_order")
            .fetch_all(pool)
            .await?;
    Ok(categories)
}

/// Get a single tag category by ID
pub async fn get_tag_category_by_id(
    pool: &Pool<Sqlite>,
    category_id: i64,
) -> Result<Option<TagCategory>> {
    let category = sqlx::query_as::<_, TagCategory>("SELECT * FROM v_tag_categories WHERE id = ?")
        .bind(category_id)
        .fetch_optional(pool)
        .await?;
    Ok(category)
}

pub async fn get_default_tag_category(pool: &Pool<Sqlite>) -> Result<Option<TagCategory>> {
    let category =
        sqlx::query_as::<_, TagCategory>("SELECT * FROM v_tag_categories WHERE is_default = TRUE")
            .fetch_optional(pool)
            .await?;
    Ok(category)
}

pub async fn get_tags(pool: &Pool<Sqlite>) -> Result<Vec<Tag>> {
    let tags = sqlx::query_as::<_, Tag>("SELECT * FROM tags ORDER BY name")
        .fetch_all(pool)
        .await?;
    Ok(tags)
}

pub async fn get_tag_by_name(pool: &Pool<Sqlite>, name: &str) -> Result<Option<Tag>> {
    let tag = sqlx::query_as::<_, Tag>("SELECT * FROM tags WHERE name = ? COLLATE NOCASE")
        .bind(name)
        .fetch_optional(pool)
        .await?;
    Ok(tag)
}

pub async fn create_tag(pool: &Pool<Sqlite>, name: &str, category_id: i64) -> Result<Tag> {
    let tag = sqlx::query_as::<_, Tag>(
        r#"
        INSERT INTO tags (name, category_id, created_at)
        VALUES (?, ?, ?)
        RETURNING *
        "#,
    )
    .bind(name)
    .bind(category_id)
    .bind(chrono::Utc::now().timestamp())
    .fetch_one(pool)
    .await?;
    Ok(tag)
}

pub async fn get_tag_by_id(pool: &Pool<Sqlite>, tag_id: i64) -> Result<Option<Tag>> {
    let tag = sqlx::query_as::<_, Tag>("SELECT * FROM tags WHERE id = ?")
        .bind(tag_id)
        .fetch_optional(pool)
        .await?;
    Ok(tag)
}

pub async fn update_tag(
    pool: &Pool<Sqlite>,
    tag_id: i64,
    name: Option<&str>,
    category_id: Option<i64>,
) -> Result<Tag> {
    let mut updates = Vec::new();
    let mut params: Vec<String> = Vec::new();

    if let Some(name) = name {
        updates.push("name = ?");
        params.push(name.to_string());
    }

    if let Some(category_id) = category_id {
        updates.push("category_id = ?");
        params.push(category_id.to_string());
    }

    if updates.is_empty() {
        // No updates, return existing tag
        return get_tag_by_id(pool, tag_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Tag not found"));
    }

    let query_str = format!(
        "UPDATE tags SET {} WHERE id = ? RETURNING *",
        updates.join(", ")
    );

    let mut query = sqlx::query_as::<_, Tag>(&query_str);

    // Bind parameters in order
    for param in params {
        query = query.bind(param);
    }

    query = query.bind(tag_id);

    let tag = query.fetch_one(pool).await?;
    Ok(tag)
}

pub async fn delete_tag(pool: &Pool<Sqlite>, tag_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM tags WHERE id = ?")
        .bind(tag_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ── Tag Categories ───────────────────────────────────────────────────────

pub async fn create_tag_category(
    pool: &Pool<Sqlite>,
    name: &str,
    prefix: &str,
    icon: &str,
    is_default: bool,
    sort_order: i64,
) -> Result<TagCategory> {
    let now = chrono::Utc::now().timestamp();
    let result = sqlx::query(
        r#"
        INSERT INTO tag_categories (name, prefix, icon, is_default, sort_order, created_at)
        VALUES (?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(name)
    .bind(prefix)
    .bind(icon)
    .bind(is_default)
    .bind(sort_order)
    .bind(now)
    .execute(pool)
    .await?;

    let new_id = result.last_insert_rowid();
    get_tag_category_by_id(pool, new_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Failed to retrieve created tag category"))
}

pub async fn update_tag_category_metadata(
    pool: &Pool<Sqlite>,
    category_id: i64,
    name: Option<&str>,
    prefix: Option<&str>,
    icon: Option<&str>,
    is_default: Option<bool>,
    sort_order: Option<i64>,
) -> Result<TagCategory> {
    let mut updates = Vec::new();
    let mut params: Vec<String> = Vec::new();

    if let Some(name) = name {
        updates.push("name = ?");
        params.push(name.to_string());
    }
    if let Some(prefix) = prefix {
        updates.push("prefix = ?");
        params.push(prefix.to_string());
    }
    if let Some(icon) = icon {
        updates.push("icon = ?");
        params.push(icon.to_string());
    }
    if let Some(is_default) = is_default {
        updates.push("is_default = ?");
        params.push(if is_default {
            "1".to_string()
        } else {
            "0".to_string()
        });
    }
    if let Some(sort_order) = sort_order {
        updates.push("sort_order = ?");
        params.push(sort_order.to_string());
    }

    if updates.is_empty() {
        // No updates, return existing category
        return get_tag_category_by_id(pool, category_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Tag category not found"));
    }

    let query_str = format!(
        "UPDATE tag_categories SET {} WHERE id = ?",
        updates.join(", ")
    );

    let mut db_query = sqlx::query(&query_str);
    for param in params {
        db_query = db_query.bind(param);
    }
    db_query = db_query.bind(category_id);

    db_query.execute(pool).await?;

    get_tag_category_by_id(pool, category_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Tag category not found"))
}

pub async fn delete_tag_category(pool: &Pool<Sqlite>, category_id: i64) -> Result<()> {
    // Check if category is in use
    let count: Option<i64> = sqlx::query_scalar("SELECT COUNT(*) FROM tags WHERE category_id = ?")
        .bind(category_id)
        .fetch_one(pool)
        .await?;

    let count_val: i64 = count.unwrap_or_default();

    if count_val > 0 {
        return Err(anyhow::anyhow!(
            "Cannot delete category that is in use by tags"
        ));
    }

    sqlx::query("DELETE FROM tag_categories WHERE id = ?")
        .bind(category_id)
        .execute(pool)
        .await?;

    Ok(())
}

// ── Tag Parents/Children ────────────────────────────────────────────────

/// Get all parent tags for a given tag
pub async fn get_tag_parents(pool: &Pool<Sqlite>, tag_id: i64) -> Result<Vec<Tag>> {
    let parents = sqlx::query_as::<_, Tag>(
        r#"
        SELECT t.id, t.name, t.category_id, t.sort_order, t.created_at, t.backpack
        FROM tag_parents tp
        JOIN tags t ON t.id = tp.parent_tag_id
        WHERE tp.tag_id = ?
        ORDER BY t.name
        "#,
    )
    .bind(tag_id)
    .fetch_all(pool)
    .await?;
    Ok(parents)
}

/// Get all tags that use this tag as a parent (reverse lookup)
pub async fn get_tag_children(pool: &Pool<Sqlite>, parent_tag_id: i64) -> Result<Vec<Tag>> {
    let children = sqlx::query_as::<_, Tag>(
        r#"
        SELECT t.id, t.name, t.category_id, t.sort_order, t.created_at, t.backpack
        FROM tag_parents tp
        JOIN tags t ON t.id = tp.tag_id
        WHERE tp.parent_tag_id = ?
        ORDER BY t.name
        "#,
    )
    .bind(parent_tag_id)
    .fetch_all(pool)
    .await?;
    Ok(children)
}

/// Set parent tags for a tag (replaces all existing parents).
/// Only tags in the Setlist category can have parents.
/// Returns the new list of parent tags.
pub async fn set_tag_parents(
    pool: &Pool<Sqlite>,
    tag_id: i64,
    parent_tag_ids: &[i64],
) -> Result<Vec<Tag>> {
    // Validate: the tag must be in the Setlist category
    let category_row = sqlx::query(
        "SELECT tc.name FROM tags t JOIN tag_categories tc ON tc.id = t.category_id WHERE t.id = ?",
    )
    .bind(tag_id)
    .fetch_optional(pool)
    .await?;

    match category_row {
        Some(row) => {
            let cat_name: String = row.try_get("name")?;
            if cat_name != "Setlist" {
                return Err(anyhow::anyhow!(
                    "Only Setlist tags can have parent tags. Tag is in category: {}",
                    cat_name
                ));
            }
        }
        None => return Err(anyhow::anyhow!("Tag with id {} not found", tag_id)),
    }

    // Validate: no self-reference
    if parent_tag_ids.contains(&tag_id) {
        return Err(anyhow::anyhow!("A tag cannot be its own parent"));
    }

    // Validate: all parent tags exist
    for &parent_id in parent_tag_ids {
        let exists: bool = sqlx::query_scalar("SELECT COUNT(*) > 0 FROM tags WHERE id = ?")
            .bind(parent_id)
            .fetch_one(pool)
            .await?;
        if !exists {
            return Err(anyhow::anyhow!(
                "Parent tag with id {} not found",
                parent_id
            ));
        }
    }

    // Validate: parent tags must not be Setlist (only P/M/V/E categories)
    // Setlist parents create indirection without resolution — just another
    // long name that would itself need parents.
    if !parent_tag_ids.is_empty() {
        let placeholders: Vec<String> = parent_tag_ids.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "SELECT t.name FROM tags t JOIN tag_categories tc ON tc.id = t.category_id WHERE t.id IN ({}) AND tc.name = 'Setlist' LIMIT 1",
            placeholders.join(",")
        );
        let mut q = sqlx::query_scalar::<_, String>(&sql);
        for id in parent_tag_ids {
            q = q.bind(id);
        }
        if let Ok(Some(name)) = q.fetch_optional(pool).await {
            return Err(anyhow::anyhow!(
                "Parent tag '{}' is a Setlist tag. Parent tags must be from Phase, Mood, Vibe, or Merkmal categories, not Setlist.",
                name
            ));
        }
    }

    // Delete existing parents and insert new ones in a transaction
    let mut tx = pool.begin().await?;

    sqlx::query("DELETE FROM tag_parents WHERE tag_id = ?")
        .bind(tag_id)
        .execute(&mut *tx)
        .await?;

    for &parent_id in parent_tag_ids {
        sqlx::query("INSERT OR IGNORE INTO tag_parents (tag_id, parent_tag_id) VALUES (?, ?)")
            .bind(tag_id)
            .bind(parent_id)
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;

    // Return the new parent tags
    get_tag_parents(pool, tag_id).await
}

// ── Tag Service Connections ─────────────────────────────────────────────

pub async fn get_tag_service_connections(
    pool: &Pool<Sqlite>,
    tag_name: &str,
) -> Result<ServiceConnections> {
    let services = sqlx::query_scalar::<_, String>(
        r#"SELECT DISTINCT vtp.service FROM v_tag_playlist vtp WHERE LOWER(TRIM(vtp.tag_name)) = LOWER(TRIM(?))"#
    )
    .bind(tag_name)
    .fetch_all(pool)
    .await?;

    let spotify = services.iter().any(|s| s == "spotify");
    let soundcloud = services.iter().any(|s| s == "soundcloud");
    let youtube = services.iter().any(|s| s == "youtube");

    Ok(ServiceConnections {
        spotify,
        soundcloud,
        youtube,
    })
}

// ── Curation Queue ──────────────────────────────────────────────────────

/// Get the curation queue: all Setlist tags with file counts, parent counts,
/// and full parent tag details as a JSON string.
pub async fn get_curation_queue(
    pool: &Pool<Sqlite>,
    search: Option<&str>,
    sort: Option<&str>,
    order: Option<&str>,
    has_parents: Option<&str>,
    limit: Option<i64>,
) -> Result<Vec<CurationTag>> {
    let limit = limit.unwrap_or(200);
    let search_pattern = search.and_then(|s| {
        if s.is_empty() {
            None
        } else {
            Some(format!("%{}%", s))
        }
    });

    let mut sql = String::from(
        r#"
        SELECT
            t.id,
            t.name,
            COALESCE(vfc.file_count, 0) AS file_count,
            COALESCE(tp_count.parent_count, 0) AS parent_count,
            tc.id AS category_id,
            tc.name AS category,
            tc.icon AS category_icon,
            COALESCE(pj.parents_json, '[]') AS parents_json
        FROM tags t
        JOIN tag_categories tc ON tc.id = t.category_id
        LEFT JOIN v_tag_file_counts vfc ON vfc.tag_id = t.id
        LEFT JOIN (
            SELECT tag_id, COUNT(*) AS parent_count
            FROM tag_parents
            GROUP BY tag_id
        ) tp_count ON tp_count.tag_id = t.id
        LEFT JOIN (
            SELECT tp.tag_id, json_group_array(json_object(
                'id', pt.id,
                'name', pt.name,
                'category', ptc.name,
                'categoryIcon', ptc.icon
            )) AS parents_json
            FROM tag_parents tp
            JOIN tags pt ON pt.id = tp.parent_tag_id
            JOIN tag_categories ptc ON ptc.id = pt.category_id
            GROUP BY tp.tag_id
        ) pj ON pj.tag_id = t.id
        WHERE tc.name = 'Setlist'
        "#,
    );

    if search_pattern.is_some() {
        sql.push_str(" AND t.name LIKE ?");
    }

    if let Some(has_p) = has_parents {
        match has_p {
            "yes" => sql.push_str(" AND tp_count.parent_count > 0"),
            "no" => {
                sql.push_str(" AND (tp_count.parent_count = 0 OR tp_count.parent_count IS NULL)")
            }
            _ => {} // "any" or anything else → no filter
        }
    }

    // Sort: name | length | files | parents; default length DESC
    let sort_col = match sort {
        Some("name") => "t.name",
        Some("files") => "file_count",
        Some("parents") => "parent_count",
        _ => "LENGTH(t.name)", // "length" or default
    };
    let ord = match order {
        Some("asc") => "ASC",
        _ => "DESC", // default desc
    };
    sql.push_str(&format!(" ORDER BY {} {}", sort_col, ord));
    sql.push_str(" LIMIT ?");

    let mut q = sqlx::query_as::<_, CurationTag>(&sql);
    if let Some(ref pattern) = search_pattern {
        q = q.bind(pattern);
    }
    q = q.bind(limit);
    q.fetch_all(pool).await.map_err(|e| anyhow::anyhow!("{e}"))
}

// ── Tag Embeddings ──────────────────────────────────────────────────────

/// Get a single tag embedding from the cache
pub async fn get_tag_embedding(pool: &Pool<Sqlite>, tag_id: i64) -> Result<Option<Vec<u8>>> {
    let row: Option<(Vec<u8>,)> =
        sqlx::query_as("SELECT embedding FROM tag_embeddings WHERE tag_id = ?")
            .bind(tag_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|r| r.0))
}

/// Upsert (insert or replace) a tag embedding
pub async fn upsert_tag_embedding(
    pool: &Pool<Sqlite>,
    tag_id: i64,
    embedding_blob: &[u8],
    model_version: &str,
) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        r#"
        INSERT INTO tag_embeddings (tag_id, embedding, model_version, updated_at)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(tag_id) DO UPDATE SET
            embedding = excluded.embedding,
            model_version = excluded.model_version,
            updated_at = excluded.updated_at
        "#,
    )
    .bind(tag_id)
    .bind(embedding_blob)
    .bind(model_version)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get all tag embeddings for a given category
pub async fn get_embeddings_by_category(
    pool: &Pool<Sqlite>,
    category_id: i64,
) -> Result<Vec<(i64, Vec<u8>)>> {
    let rows = sqlx::query_as::<_, (i64, Vec<u8>)>(
        r#"
        SELECT te.tag_id, te.embedding
        FROM tag_embeddings te
        JOIN tags t ON t.id = te.tag_id
        WHERE t.category_id = ?
        "#,
    )
    .bind(category_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Get ALL tag embeddings (tag_id → embedding blob).
/// Returns all tags with their embedding, including tag name.
pub async fn get_all_embeddings(
    pool: &Pool<Sqlite>,
) -> Result<Vec<(i64, String, Option<Vec<u8>>)>> {
    let rows = sqlx::query_as::<_, (i64, String, Option<Vec<u8>>)>(
        r#"
        SELECT t.id, t.name, te.embedding
        FROM tags t
        LEFT JOIN tag_embeddings te ON te.tag_id = t.id
        ORDER BY t.name
        "#,
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Delete all tag embeddings (used before recompute)
pub async fn clear_all_embeddings(pool: &Pool<Sqlite>) -> Result<()> {
    sqlx::query("DELETE FROM tag_embeddings")
        .execute(pool)
        .await?;
    Ok(())
}

/// Reset reviewed_at to NULL for all tags (unreview all)
pub async fn reset_all_reviewed_at(pool: &Pool<Sqlite>) -> Result<usize> {
    let result = sqlx::query("UPDATE tags SET reviewed_at = NULL")
        .execute(pool)
        .await?;
    Ok(result.rows_affected() as usize)
}

// ── Bulk Tag Operations ─────────────────────────────────────────────────

/// Bulk-update category_id + reviewed_at for multiple tags in a single transaction.
/// Returns the number of tags updated.
pub async fn bulk_categorize_tags(
    pool: &Pool<Sqlite>,
    tag_ids: &[i64],
    category_id: i64,
) -> Result<u64> {
    if tag_ids.is_empty() {
        return Ok(0);
    }
    let now = chrono::Utc::now().timestamp();
    let mut tx = pool.begin().await?;
    let mut count: u64 = 0;
    for &tag_id in tag_ids {
        let rows = sqlx::query("UPDATE tags SET category_id = ?, reviewed_at = ? WHERE id = ?")
            .bind(category_id)
            .bind(now)
            .bind(tag_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        count += rows;
    }
    tx.commit().await?;
    Ok(count)
}

/// Set category_id and reviewed_at for a tag.
/// Returns the updated Tag.
pub async fn categorize_tag(pool: &Pool<Sqlite>, tag_id: i64, category_id: i64) -> Result<Tag> {
    let now = chrono::Utc::now().timestamp();
    let tag = sqlx::query_as::<_, Tag>(
        r#"
        UPDATE tags
        SET category_id = ?, reviewed_at = ?
        WHERE id = ?
        RETURNING *
        "#,
    )
    .bind(category_id)
    .bind(now)
    .bind(tag_id)
    .fetch_one(pool)
    .await?;
    Ok(tag)
}

/// Check bulk tag names against DB.
/// Returns for each name: whether it exists, its current category_id (if any), and its current category name.
pub async fn bulk_check_tags(
    pool: &Pool<Sqlite>,
    names: &[String],
) -> Result<Vec<(String, Option<i64>, Option<String>)>> {
    let mut results = Vec::new();
    for name in names {
        let tag = sqlx::query_as::<_, Tag>(
            "SELECT t.* FROM tags t
             WHERE t.name = ? COLLATE NOCASE",
        )
        .bind(name)
        .fetch_optional(pool)
        .await?;

        match tag {
            Some(t) => {
                let cat_name: Option<String> = if t.category_id > 0 {
                    let name: Option<String> =
                        sqlx::query_scalar("SELECT name FROM tag_categories WHERE id = ?")
                            .bind(t.category_id)
                            .fetch_optional(pool)
                            .await?
                            .flatten();
                    name
                } else {
                    None
                };
                results.push((name.clone(), Some(t.category_id), cat_name));
            }
            None => {
                results.push((name.clone(), None, None));
            }
        }
    }
    Ok(results)
}

/// Bulk create tags (all assign category + mark reviewed).
/// Returns created tags with their ids.
pub async fn bulk_create_tags(pool: &Pool<Sqlite>, entries: &[(String, i64)]) -> Result<Vec<Tag>> {
    let now = chrono::Utc::now().timestamp();
    let mut created = Vec::new();
    for (name, category_id) in entries {
        let tag = sqlx::query_as::<_, Tag>(
            r#"
            INSERT INTO tags (name, category_id, created_at, reviewed_at)
            VALUES (?, ?, ?, ?)
            RETURNING *
            "#,
        )
        .bind(name)
        .bind(category_id)
        .bind(now)
        .bind(now)
        .fetch_one(pool)
        .await?;
        created.push(tag);
    }
    Ok(created)
}

/// Bulk update tags: change category + mark reviewed.
/// Returns updated tags.
pub async fn bulk_update_tags(pool: &Pool<Sqlite>, entries: &[(String, i64)]) -> Result<Vec<Tag>> {
    let now = chrono::Utc::now().timestamp();
    let mut updated = Vec::new();
    for (name, category_id) in entries {
        let tag = sqlx::query_as::<_, Tag>(
            r#"
            UPDATE tags
            SET category_id = ?, reviewed_at = ?
            WHERE name = ? COLLATE NOCASE
            RETURNING *
            "#,
        )
        .bind(category_id)
        .bind(now)
        .bind(name)
        .fetch_one(pool)
        .await?;
        updated.push(tag);
    }
    Ok(updated)
}

/// Mark existing tags as reviewed (no category change).
pub async fn bulk_review_tags(pool: &Pool<Sqlite>, names: &[String]) -> Result<usize> {
    let now = chrono::Utc::now().timestamp();
    let mut count = 0;
    for name in names {
        let result = sqlx::query(
            "UPDATE tags SET reviewed_at = ? WHERE name = ? COLLATE NOCASE AND reviewed_at IS NULL",
        )
        .bind(now)
        .bind(name)
        .execute(pool)
        .await?;
        count += result.rows_affected() as usize;
    }
    Ok(count)
}

// ── Unreviewed Tags ─────────────────────────────────────────────────────

pub async fn get_unreviewed_tags(pool: &Pool<Sqlite>) -> Result<Vec<Tag>> {
    let tags =
        sqlx::query_as::<_, Tag>("SELECT * FROM tags WHERE reviewed_at IS NULL ORDER BY name")
            .fetch_all(pool)
            .await?;
    Ok(tags)
}

/// Get counts of reviewed and unreviewed tags
pub async fn get_tag_review_counts(pool: &Pool<Sqlite>) -> Result<(usize, usize)> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags")
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    let unreviewed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags WHERE reviewed_at IS NULL")
        .fetch_one(pool)
        .await
        .unwrap_or(0);

    Ok((total as usize - unreviewed as usize, unreviewed as usize))
}

// ── Backpack ────────────────────────────────────────────────────────────

pub async fn set_tag_backpack(pool: &Pool<Sqlite>, tag_id: i64, backpack: bool) -> Result<()> {
    sqlx::query("UPDATE tags SET backpack = ? WHERE id = ?")
        .bind(backpack)
        .bind(tag_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Get all backpack tags
pub async fn get_backpack_tags(pool: &Pool<Sqlite>) -> Result<Vec<Tag>> {
    let tags = sqlx::query_as::<_, Tag>("SELECT * FROM tags WHERE backpack = 1")
        .fetch_all(pool)
        .await?;
    Ok(tags)
}

/// Find the "backpack" tag (a Setlist tag named "backpack")
pub async fn get_backpack_tag(pool: &Pool<Sqlite>) -> Result<Option<Tag>> {
    let tag = sqlx::query_as::<_, Tag>(
        "SELECT t.* FROM tags t JOIN tag_categories tc ON t.category_id = tc.id WHERE LOWER(t.name) = 'backpack' AND tc.name = 'Setlist'"
    )
    .fetch_optional(pool)
    .await?;
    Ok(tag)
}

/// Ensure the "backpack" tag exists, create it if missing, then return it
pub async fn ensure_backpack_tag(pool: &Pool<Sqlite>) -> Result<Tag> {
    if let Some(tag) = get_backpack_tag(pool).await? {
        return Ok(tag);
    }
    // Find Setlist category
    let cat_id: i64 = sqlx::query_scalar("SELECT id FROM tag_categories WHERE name = 'Setlist'")
        .fetch_one(pool)
        .await?;
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "INSERT INTO tags (name, category_id, created_at, backpack) VALUES ('backpack', ?, ?, 1)",
    )
    .bind(cat_id)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(get_backpack_tag(pool).await?.unwrap())
}

/// Check if a file has ANY backpack tag
pub async fn is_in_backpack(pool: &Pool<Sqlite>, file_id: i64) -> Result<bool> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT frt.tag_id) FROM file_resolved_tags frt
         JOIN tags t ON t.id = frt.tag_id
         WHERE frt.file_id = ? AND t.backpack = 1",
    )
    .bind(file_id)
    .fetch_one(pool)
    .await?;
    Ok(count > 0)
}
