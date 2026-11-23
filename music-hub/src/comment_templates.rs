//! Comment-template library (#668, slice C2): named bodies the hub stores for
//! the deterministic Comment render/validate semantics of the `comment-template`
//! crate (#664).
//!
//! Schema lives in `066_hub_comment_templates.sql`. Three visibilities exist:
//!
//! * **mine** — `owner_user_id = uid`;
//! * **shared** — a foreign row with `is_shared = 1` (visible, not mutable);
//! * **system** — `owner_user_id IS NULL` (visible to all, read-only except for
//!   admins).
//!
//! Conventions mirror `backpacks.rs`: `anyhow` results, `chrono::Utc::now()`
//! RFC3339 timestamps. Bodies are validated against
//! [`comment_template::validate`] before every write (create / body update).

use anyhow::{Result, bail};
use sqlx::SqlitePool;

/// Typed view of one `hub_comment_templates` row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CommentTemplate {
    pub id: i64,
    /// `None` = system template.
    pub owner_user_id: Option<i64>,
    pub name: String,
    pub body: String,
    pub is_shared: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// A template as shown in the `/comment-templates` view, with its visibility
/// flags resolved for one viewer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateView {
    pub id: i64,
    pub name: String,
    pub body: String,
    /// System template (`owner_user_id IS NULL`).
    pub is_system: bool,
    /// Owned by the viewer.
    pub is_mine: bool,
    /// Foreign and shared with everyone.
    pub is_shared: bool,
    /// Owner slug, or `""` for a system template.
    pub owner_slug: String,
    /// Whether the viewer may mutate (rename / body / share / delete) this row:
    /// own rows always, system rows only for admins.
    pub editable: bool,
}

/// The default body of the `Standard` system template.
pub const DEFAULT_TEMPLATE: &str = "{types} {tags}";

/// Validate a body against the hub placeholder whitelist, mapping the crate
/// error into a user-facing message that names the allowed placeholders.
fn validate_body(body: &str) -> Result<()> {
    comment_template::validate(body).map_err(|e| anyhow::anyhow!(e))
}

/// Whether `row` may be mutated by `uid` (`is_admin` unlocks system rows).
///
/// Own rows are always mutable; a system row only for admins; a foreign row
/// (shared or not) is never mutable by a non-owner.
fn can_mutate(row: &CommentTemplate, uid: i64, is_admin: bool) -> bool {
    match row.owner_user_id {
        Some(owner) => owner == uid,
        None => is_admin,
    }
}

/// Raw row lookup by id, regardless of visibility. `None` when absent.
pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<CommentTemplate>> {
    Ok(sqlx::query_as::<_, CommentTemplate>(
        "SELECT id, owner_user_id, name, body, is_shared, created_at, updated_at
           FROM hub_comment_templates
          WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// The row when it is visible to `uid` (mine ∪ shared ∪ system), else `None`.
///
/// A foreign private template is indistinguishable from a missing one, so the
/// handler answers `404` without a cross-user leak.
pub async fn get_visible(pool: &SqlitePool, uid: i64, id: i64) -> Result<Option<CommentTemplate>> {
    Ok(sqlx::query_as::<_, CommentTemplate>(
        "SELECT id, owner_user_id, name, body, is_shared, created_at, updated_at
           FROM hub_comment_templates
          WHERE id = ?1
            AND (owner_user_id = ?2 OR is_shared = 1 OR owner_user_id IS NULL)",
    )
    .bind(id)
    .bind(uid)
    .fetch_optional(pool)
    .await?)
}

/// Whether `uid` may mutate the template `id`.
///
/// `None` = no such row (handler `404`); `Some(false)` = visible but read-only
/// for this viewer (handler `403`); `Some(true)` = mutable.
pub async fn mutable_by(
    pool: &SqlitePool,
    uid: i64,
    is_admin: bool,
    id: i64,
) -> Result<Option<bool>> {
    match get(pool, id).await? {
        None => Ok(None),
        Some(row) => Ok(Some(can_mutate(&row, uid, is_admin))),
    }
}

/// All templates visible to `uid`, grouped for the UI: system first, then the
/// viewer's own, then foreign shared ones, each group name-sorted
/// (case-insensitive).
pub async fn list_visible(
    pool: &SqlitePool,
    uid: i64,
    is_admin: bool,
) -> Result<Vec<TemplateView>> {
    let rows = sqlx::query_as::<_, (i64, Option<i64>, String, String, i64, String)>(
        "SELECT t.id, t.owner_user_id, t.name, t.body, t.is_shared,
                COALESCE(u.slug, '') AS owner_slug
           FROM hub_comment_templates t
           LEFT JOIN hub_users u ON u.id = t.owner_user_id
          WHERE t.owner_user_id = ?1 OR t.is_shared = 1 OR t.owner_user_id IS NULL
          ORDER BY CASE
                     WHEN t.owner_user_id IS NULL THEN 0
                     WHEN t.owner_user_id = ?1 THEN 1
                     ELSE 2
                   END,
                   t.name COLLATE NOCASE,
                   t.id",
    )
    .bind(uid)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(id, owner_user_id, name, body, is_shared, owner_slug)| {
            let is_system = owner_user_id.is_none();
            let is_mine = owner_user_id == Some(uid);
            let is_shared = is_shared != 0;
            let editable = is_mine || (is_system && is_admin);
            TemplateView {
                id,
                name,
                body,
                is_system,
                is_mine,
                is_shared,
                owner_slug,
                editable,
            }
        })
        .collect())
}

/// Create a template owned by `owner_user_id` (never a system row). The name is
/// trimmed (empty rejected); a case-insensitive clash with the owner's other
/// names is rejected before the insert. The body is validated against the
/// placeholder whitelist. New rows start unshared. Returns the new id.
pub async fn create(
    pool: &SqlitePool,
    owner_user_id: i64,
    name: &str,
    body: &str,
    is_shared: bool,
) -> Result<i64> {
    let name = name.trim();
    if name.is_empty() {
        bail!("name required");
    }
    validate_body(body)?;

    let clash = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM hub_comment_templates
          WHERE owner_user_id = ?1 AND name = ?2 COLLATE NOCASE",
    )
    .bind(owner_user_id)
    .bind(name)
    .fetch_optional(pool)
    .await?;
    if clash.is_some() {
        bail!("es gibt schon eine Vorlage mit diesem Namen");
    }

    let now = chrono::Utc::now().to_rfc3339();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_comment_templates
            (owner_user_id, name, body, is_shared, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?5)
         RETURNING id",
    )
    .bind(owner_user_id)
    .bind(name)
    .bind(body)
    .bind(i64::from(is_shared))
    .bind(&now)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Rename a template. Requires mutability (`uid` owns it, or `is_admin` on a
/// system row). Trims the name (empty rejected), rejects a case-insensitive
/// clash within the same owner scope, and bumps `updated_at`.
pub async fn rename(
    pool: &SqlitePool,
    uid: i64,
    is_admin: bool,
    id: i64,
    name: &str,
) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("name required");
    }
    let row = get(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Vorlage nicht gefunden"))?;
    if !can_mutate(&row, uid, is_admin) {
        bail!("keine Berechtigung");
    }

    let clash = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM hub_comment_templates
          WHERE owner_user_id IS ?1 AND name = ?2 COLLATE NOCASE AND id <> ?3",
    )
    .bind(row.owner_user_id)
    .bind(name)
    .bind(id)
    .fetch_optional(pool)
    .await?;
    if clash.is_some() {
        bail!("es gibt schon eine Vorlage mit diesem Namen");
    }

    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("UPDATE hub_comment_templates SET name = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(name)
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Replace a template's body. Requires mutability. The new body is validated
/// against the placeholder whitelist before the write.
pub async fn update_body(
    pool: &SqlitePool,
    uid: i64,
    is_admin: bool,
    id: i64,
    body: &str,
) -> Result<()> {
    let row = get(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Vorlage nicht gefunden"))?;
    if !can_mutate(&row, uid, is_admin) {
        bail!("keine Berechtigung");
    }
    validate_body(body)?;

    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("UPDATE hub_comment_templates SET body = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(body)
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Set (or clear) the `is_shared` flag. Requires mutability.
pub async fn set_shared(
    pool: &SqlitePool,
    uid: i64,
    is_admin: bool,
    id: i64,
    is_shared: bool,
) -> Result<()> {
    let row = get(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Vorlage nicht gefunden"))?;
    if !can_mutate(&row, uid, is_admin) {
        bail!("keine Berechtigung");
    }

    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query("UPDATE hub_comment_templates SET is_shared = ?1, updated_at = ?2 WHERE id = ?3")
        .bind(i64::from(is_shared))
        .bind(&now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Copy a visible template into a new row owned by `uid` (unshared, always
/// mutable). The copy name is derived from the source name and made unique
/// within the caller's own templates (`name`, then `name (Kopie)`,
/// `name (Kopie 2)`, …). Returns the new id.
pub async fn duplicate(pool: &SqlitePool, uid: i64, id: i64) -> Result<i64> {
    let row = get(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Vorlage nicht gefunden"))?;

    let base = row.name.trim();
    let mut candidate = base.to_string();
    let mut n = 0u32;
    loop {
        let clash = sqlx::query_scalar::<_, i64>(
            "SELECT 1 FROM hub_comment_templates
              WHERE owner_user_id = ?1 AND name = ?2 COLLATE NOCASE",
        )
        .bind(uid)
        .bind(&candidate)
        .fetch_optional(pool)
        .await?;
        if clash.is_none() {
            break;
        }
        n += 1;
        candidate = if n == 1 {
            format!("{base} (Kopie)")
        } else {
            format!("{base} (Kopie {n})")
        };
    }

    create(pool, uid, &candidate, &row.body, false).await
}

/// Delete a template. Requires mutability (`uid` owns it, or `is_admin` on a
/// system row). Returns `true` when a row was removed.
pub async fn delete(pool: &SqlitePool, uid: i64, is_admin: bool, id: i64) -> Result<bool> {
    let row = get(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Vorlage nicht gefunden"))?;
    if !can_mutate(&row, uid, is_admin) {
        bail!("keine Berechtigung");
    }
    let rows = sqlx::query("DELETE FROM hub_comment_templates WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(rows > 0)
}
