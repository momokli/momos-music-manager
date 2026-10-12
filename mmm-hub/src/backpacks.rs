//! Backpack sources (#329): typed model + persistence helpers for attaching a
//! source of one of four kinds (tag / playlist / lens / track) to a backpack.
//!
//! Schema lives in `044_hub_backpacks.sql` (#327). This module is the *sources
//! layer* only: no resolution/union/dedup (that is the resolver, #330), no UI
//! (#328) and no active-scope logic (#331).
//!
//! Conventions mirror `curate.rs`: `anyhow` results, `chrono::Utc::now()`
//! RFC3339 timestamps, and every write bumps the parent `hub_backpacks.updated_at`.

use std::str::FromStr;

use anyhow::{Result, bail};
use sqlx::SqlitePool;

/// The four source kinds, matching the DB CHECK on `hub_backpack_items.source_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Tag,
    Playlist,
    Lens,
    Track,
}

impl SourceKind {
    /// The exact DB literal for this kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            SourceKind::Tag => "tag",
            SourceKind::Playlist => "playlist",
            SourceKind::Lens => "lens",
            SourceKind::Track => "track",
        }
    }
}

impl FromStr for SourceKind {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "tag" => Ok(SourceKind::Tag),
            "playlist" => Ok(SourceKind::Playlist),
            "lens" => Ok(SourceKind::Lens),
            "track" => Ok(SourceKind::Track),
            other => bail!("unknown source kind: {other}"),
        }
    }
}

/// Typed view of one `hub_backpack_items` row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BackpackItem {
    pub id: i64,
    pub backpack_id: i64,
    pub source_kind: String,
    pub source_tag_id: Option<i64>,
    pub source_playlist_id: Option<i64>,
    pub source_lens: Option<String>,
    pub source_track_id: Option<i64>,
    pub position: i64,
    pub added_at: String,
}

/// A source to attach to a backpack (exactly one variant).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceRef {
    Tag(i64),
    Playlist(i64),
    Lens(String),
    Track(i64),
}

impl SourceRef {
    pub fn kind(&self) -> SourceKind {
        match self {
            SourceRef::Tag(_) => SourceKind::Tag,
            SourceRef::Playlist(_) => SourceKind::Playlist,
            SourceRef::Lens(_) => SourceKind::Lens,
            SourceRef::Track(_) => SourceKind::Track,
        }
    }
}

/// Attach a source to a backpack, appending it (position = MAX+1) and bumping
/// the backpack's `updated_at`. Returns the new item id.
///
/// A lens key must exist in [`crate::curate::LENSES`]; otherwise this returns an
/// error and writes nothing.
pub async fn add_source(pool: &SqlitePool, backpack_id: i64, src: SourceRef) -> Result<i64> {
    let now = chrono::Utc::now().to_rfc3339();
    let kind = src.kind().as_str();

    // App-level lens validation (DB CHECK allows any lens text on purpose).
    let (tag_id, playlist_id, lens, track_id): (Option<i64>, Option<i64>, Option<String>, Option<i64>) =
        match &src {
            SourceRef::Tag(id) => (Some(*id), None, None, None),
            SourceRef::Playlist(id) => (None, Some(*id), None, None),
            SourceRef::Lens(key) => {
                if !crate::curate::LENSES.iter().any(|(k, _)| k == key) {
                    bail!("unknown lens key: {key}");
                }
                (None, None, Some(key.clone()), None)
            }
            SourceRef::Track(id) => (None, None, None, Some(*id)),
        };

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_backpack_items
            (backpack_id, source_kind, source_tag_id, source_playlist_id, source_lens, source_track_id, position, added_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6,
                 (SELECT COALESCE(MAX(position), 0) + 1 FROM hub_backpack_items WHERE backpack_id = ?1),
                 ?7)
         RETURNING id",
    )
    .bind(backpack_id)
    .bind(kind)
    .bind(tag_id)
    .bind(playlist_id)
    .bind(lens)
    .bind(track_id)
    .bind(&now)
    .fetch_one(pool)
    .await?;

    sqlx::query("UPDATE hub_backpacks SET updated_at = ?2 WHERE id = ?1")
        .bind(backpack_id)
        .bind(&now)
        .execute(pool)
        .await?;

    Ok(id)
}

/// All items of a backpack, ordered by append order (`position, id`).
/// An unknown/empty backpack yields an empty `Vec`.
pub async fn list_sources(pool: &SqlitePool, backpack_id: i64) -> Result<Vec<BackpackItem>> {
    Ok(sqlx::query_as::<_, BackpackItem>(
        "SELECT id, backpack_id, source_kind, source_tag_id, source_playlist_id,
                source_lens, source_track_id, position, added_at
           FROM hub_backpack_items
          WHERE backpack_id = ?1
          ORDER BY position, id",
    )
    .bind(backpack_id)
    .fetch_all(pool)
    .await?)
}

/// Remove one item by id. Returns `true` when a row was deleted. Bumps the
/// owning backpack's `updated_at` (a no-op bump when nothing was deleted).
pub async fn remove_source(pool: &SqlitePool, item_id: i64) -> Result<bool> {
    let now = chrono::Utc::now().to_rfc3339();
    let backpack_id: Option<i64> = sqlx::query_scalar(
        "DELETE FROM hub_backpack_items WHERE id = ?1 RETURNING backpack_id",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;

    if let Some(bp) = backpack_id {
        sqlx::query("UPDATE hub_backpacks SET updated_at = ?2 WHERE id = ?1")
            .bind(bp)
            .bind(&now)
            .execute(pool)
            .await?;
    }

    Ok(backpack_id.is_some())
}

/// Delete all items of a backpack. Returns the number of rows removed and bumps
/// the backpack's `updated_at`.
pub async fn clear_sources(pool: &SqlitePool, backpack_id: i64) -> Result<u64> {
    let now = chrono::Utc::now().to_rfc3339();
    let res = sqlx::query("DELETE FROM hub_backpack_items WHERE backpack_id = ?1")
        .bind(backpack_id)
        .execute(pool)
        .await?;
    sqlx::query("UPDATE hub_backpacks SET updated_at = ?2 WHERE id = ?1")
        .bind(backpack_id)
        .bind(&now)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}
