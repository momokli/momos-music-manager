//! Backpack sources (#329): typed model + persistence helpers for attaching a
//! source of one of four kinds (tag / playlist / lens / track) to a backpack.
//!
//! Schema lives in `044_hub_backpacks.sql` (#327). This module hosts the
//! *sources layer* (#329), the *resolver* (#330, [`resolve`] unions the tracks of
//! a backpack's sources, dedups and score-sorts them) and the *active-scope*
//! layer (#331, [`active_backpack`] / [`set_active`] / [`clear_active`] /
//! [`active_scope_track_ids`]). No UI (#328) here.
//!
//! Conventions mirror `curate.rs`: `anyhow` results, `chrono::Utc::now()`
//! RFC3339 timestamps, and every write bumps the parent `hub_backpacks.updated_at`.

use std::collections::{BTreeMap, HashSet};
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

/// Typed view of one `hub_backpacks` row.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Backpack {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub is_active: i64,
    pub created_at: String,
    pub updated_at: String,
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

/// All backpacks owned by `user_id`, ordered by creation (`created_at, id`, a
/// stable total order). Foreign backpacks are never returned; an unknown user
/// yields `Ok(vec![])`.
pub async fn list_backpacks(pool: &SqlitePool, user_id: i64) -> Result<Vec<Backpack>> {
    Ok(sqlx::query_as::<_, Backpack>(
        "SELECT id, user_id, name, is_active, created_at, updated_at
           FROM hub_backpacks
          WHERE user_id = ?1
          ORDER BY created_at, id",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

/// One backpack source as shown in the view: its kind literal, a plain-text
/// label, and its append position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceView {
    pub kind: String,
    pub label: String,
    pub position: i64,
}

/// The backpack's sources enriched with human-readable labels, in append order
/// (`position, id`).
///
/// Label resolution never fails on a missing/unknown reference: tag -> tag name
/// (fallback `#id`), playlist -> playlist name (fallback `#id`), lens ->
/// [`crate::curate::lens_label`], track -> `"title — artists"` (fallback
/// `#id`). An unknown backpack yields `Ok(vec![])`.
pub async fn list_source_views(pool: &SqlitePool, backpack_id: i64) -> Result<Vec<SourceView>> {
    let items = list_sources(pool, backpack_id).await?;
    let mut views = Vec::with_capacity(items.len());
    for item in items {
        let label = match item.source_kind.as_str() {
            "tag" => match item.source_tag_id {
                Some(id) => sqlx::query_scalar::<_, String>("SELECT name FROM hub_tags WHERE id = ?1")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?
                    .unwrap_or_else(|| format!("#{id}")),
                None => "—".to_string(),
            },
            "playlist" => match item.source_playlist_id {
                Some(id) => sqlx::query_scalar::<_, Option<String>>(
                    "SELECT name FROM hub_playlists WHERE id = ?1",
                )
                .bind(id)
                .fetch_optional(pool)
                .await?
                .flatten()
                .unwrap_or_else(|| format!("#{id}")),
                None => "—".to_string(),
            },
            "lens" => crate::curate::lens_label(item.source_lens.as_deref().unwrap_or("")).to_string(),
            "track" => match item.source_track_id {
                Some(id) => {
                    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
                        "SELECT title, artists FROM hub_tracks WHERE id = ?1",
                    )
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
                    match row {
                        Some((Some(title), Some(artists))) if !title.is_empty() => {
                            if artists.is_empty() {
                                title
                            } else {
                                format!("{title} — {artists}")
                            }
                        }
                        Some((Some(title), _)) if !title.is_empty() => title,
                        _ => format!("#{id}"),
                    }
                }
                None => "—".to_string(),
            },
            // The DB CHECK forbids any other kind; fall back defensively.
            other => other.to_string(),
        };
        views.push(SourceView {
            kind: item.source_kind,
            label,
            position: item.position,
        });
    }
    Ok(views)
}

/// Live track count of a backpack, resolved **without** the 1..=500 clamp of
/// [`resolve`] so a large source is counted in full. An empty/unknown backpack
/// yields `0`.
pub async fn resolve_count(pool: &SqlitePool, backpack_id: i64) -> Result<usize> {
    Ok(resolve_unbounded(pool, backpack_id).await?.len())
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

// ── CRUD (#328) ───────────────────────────────────────────────────────────────

/// Create a new backpack owned by `user_id`.
///
/// The name is trimmed; an empty (or whitespace-only) name is an error and
/// writes nothing. Names are unique per user case-insensitively
/// (`idx_hub_backpacks_user_name`), so a clash — including one that differs only
/// by case — is rejected with a user-facing error *before* the insert (never a
/// raw `UNIQUE` constraint failure). A name already used by *another* user is
/// allowed. The new row starts inactive (`is_active = 0`) with
/// `created_at = updated_at = now`. Returns the new backpack id.
pub async fn create_backpack(pool: &SqlitePool, user_id: i64, name: &str) -> Result<i64> {
    let name = name.trim();
    if name.is_empty() {
        bail!("backpack name required");
    }

    let clash = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM hub_backpacks WHERE user_id = ?1 AND name = ?2 COLLATE NOCASE",
    )
    .bind(user_id)
    .bind(name)
    .fetch_optional(pool)
    .await?;
    if clash.is_some() {
        bail!("es gibt schon ein Backpack mit diesem Namen");
    }

    let now = chrono::Utc::now().to_rfc3339();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_backpacks (user_id, name, is_active, created_at, updated_at)
         VALUES (?1, ?2, 0, ?3, ?3)
         RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(&now)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Rename an existing backpack (owner only).
///
/// Trims the name (empty -> error), rejects a case-insensitive name clash with
/// the user's *other* backpacks (`id <> ?`, so renaming to the same name is
/// fine), and bumps `updated_at`. A missing or foreign `backpack_id` is an error
/// and writes nothing (no silent cross-user access).
pub async fn rename_backpack(
    pool: &SqlitePool,
    user_id: i64,
    backpack_id: i64,
    name: &str,
) -> Result<()> {
    let name = name.trim();
    if name.is_empty() {
        bail!("backpack name required");
    }

    let clash = sqlx::query_scalar::<_, i64>(
        "SELECT 1 FROM hub_backpacks
          WHERE user_id = ?1 AND name = ?2 COLLATE NOCASE AND id <> ?3",
    )
    .bind(user_id)
    .bind(name)
    .bind(backpack_id)
    .fetch_optional(pool)
    .await?;
    if clash.is_some() {
        bail!("es gibt schon ein Backpack mit diesem Namen");
    }

    let now = chrono::Utc::now().to_rfc3339();
    let rows = sqlx::query(
        "UPDATE hub_backpacks SET name = ?1, updated_at = ?2
          WHERE id = ?3 AND user_id = ?4",
    )
    .bind(name)
    .bind(&now)
    .bind(backpack_id)
    .bind(user_id)
    .execute(pool)
    .await?
    .rows_affected();
    if rows == 0 {
        bail!("backpack not found or not yours");
    }
    Ok(())
}

/// Delete a backpack (owner only). Returns `true` when a row was removed.
///
/// Items vanish via `ON DELETE CASCADE` — no manual item cleanup. A foreign or
/// missing `backpack_id` deletes nothing and yields `Ok(false)`. Deleting the
/// user's active backpack simply leaves the user with no active backpack (the
/// partial UNIQUE index permits zero active rows).
pub async fn delete_backpack(pool: &SqlitePool, user_id: i64, backpack_id: i64) -> Result<bool> {
    let rows = sqlx::query("DELETE FROM hub_backpacks WHERE id = ?1 AND user_id = ?2")
        .bind(backpack_id)
        .bind(user_id)
        .execute(pool)
        .await?
        .rows_affected();
    Ok(rows > 0)
}

// ── Active scope (#331) ──────────────────────────────────────────────────────

/// The user's active backpack, if any.
///
/// The partial UNIQUE index `idx_hub_backpacks_active_user` guarantees at most
/// one active backpack per user, so this returns `None` or exactly one row.
/// An unknown `user_id` yields `Ok(None)`.
pub async fn active_backpack(pool: &SqlitePool, user_id: i64) -> Result<Option<Backpack>> {
    Ok(sqlx::query_as::<_, Backpack>(
        "SELECT id, user_id, name, is_active, created_at, updated_at
           FROM hub_backpacks
          WHERE user_id = ?1 AND is_active = 1
          LIMIT 1",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?)
}

/// Make `backpack_id` the user's single active backpack (deactivating any other).
///
/// Runs in a transaction: first clears `is_active` on every backpack of the
/// user, then sets the target to `1`. The `updated_at` of every touched row is
/// bumped. A `backpack_id` that does not belong to `user_id` is an error and
/// writes nothing (no silent cross-user access).
pub async fn set_active(pool: &SqlitePool, user_id: i64, backpack_id: i64) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();

    let owned: Option<i64> =
        sqlx::query_scalar("SELECT id FROM hub_backpacks WHERE id = ?1 AND user_id = ?2")
            .bind(backpack_id)
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    let Some(id) = owned else {
        bail!("backpack {backpack_id} not found for user {user_id}");
    };

    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE hub_backpacks SET is_active = 0, updated_at = ?2
          WHERE user_id = ?1 AND is_active = 1",
    )
    .bind(user_id)
    .bind(&now)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE hub_backpacks SET is_active = 1, updated_at = ?2 WHERE id = ?1")
        .bind(id)
        .bind(&now)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Deactivate every backpack of the user, returning the number of rows that
/// were active (and thus changed). An `updated_at` is bumped on those rows.
pub async fn clear_active(pool: &SqlitePool, user_id: i64) -> Result<u64> {
    let now = chrono::Utc::now().to_rfc3339();
    let res = sqlx::query(
        "UPDATE hub_backpacks SET is_active = 0, updated_at = ?2
          WHERE user_id = ?1 AND is_active = 1",
    )
    .bind(user_id)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(res.rows_affected())
}

/// The track-id scope of the user's active backpack.
///
/// `Ok(None)` means **no active backpack** (list views stay unscoped);
/// `Ok(Some(set))` is the backpack's resolved track set, resolved **without**
/// the 500-row clamp of [`resolve`] so a scope filter never truncates.
pub async fn active_scope_track_ids(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<Option<HashSet<i64>>> {
    let Some(bp) = active_backpack(pool, user_id).await? else {
        return Ok(None);
    };
    let tracks = resolve_unbounded(pool, bp.id).await?;
    Ok(Some(tracks.into_iter().map(|t| t.track_id).collect()))
}

/// Scope for the hub list views (#331): `None` = unscoped, `Some` = active
/// backpack's resolved track set.
///
/// This is the single error policy shared by `/tags`, `/albums` and `/artists`:
/// a resolver failure (or any DB hiccup while loading the scope) degrades to
/// **unscoped** and is logged, instead of silently emptying the library. The
/// helper therefore never returns an error — an availability-preserving,
/// fail-open choice documented here so the three views cannot drift apart.
///
/// An active backpack that resolves to **no** tracks is *not* an error: it
/// yields `Some(empty)` and the list views then hide everything (the scope is
/// genuinely empty).
pub async fn active_scope_for_views(pool: &SqlitePool, user_id: i64) -> Option<HashSet<i64>> {
    match active_scope_track_ids(pool, user_id).await {
        Ok(scope) => scope,
        Err(e) => {
            tracing::warn!("active backpack scope load failed for user {user_id}: {e:#}; falling back to unscoped");
            None
        }
    }
}

// ── Resolver (#330) ───────────────────────────────────────────────────────────

/// One resolved track: its hub id, relevance score, and the distinct backpack
/// sources that contributed it (ordered by source position, ascending).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ResolvedTrack {
    pub track_id: i64,
    pub score: i64,
    pub sources: Vec<ResolvedSource>,
}

/// A contributing source of a resolved track. Mirrors [`SourceRef`] (#329) so
/// the UI (#328) can label "why is this track here" without re-querying items.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ResolvedSource {
    Tag { tag_id: i64 },
    Playlist { playlist_id: i64 },
    Lens { lens: String },
    Track { track_id: i64 },
}

/// Canonical kind order for stable `sources` ordering: tag < playlist < lens < track.
fn kind_rank(src: &ResolvedSource) -> u8 {
    match src {
        ResolvedSource::Tag { .. } => 0,
        ResolvedSource::Playlist { .. } => 1,
        ResolvedSource::Lens { .. } => 2,
        ResolvedSource::Track { .. } => 3,
    }
}

/// Resolve a backpack to a scored, deduped, sorted, limited track list.
///
/// Each item is resolved to an ordered set of hub track ids (tag /
/// playlist / lens / literal track), the sets are unioned and deduped by
/// `track_id`, and every track is scored by the number of *distinct* contributing
/// source targets. The result is sorted by `score` descending, then the smallest
/// contributing `position` ascending, then `track_id` ascending (a total order).
///
/// Unknown/empty backpack -> `Ok(vec![])`. `limit` is clamped to `1..=500`
/// (so `limit <= 0` behaves as `1`, mirroring [`crate::curate::resolve_track_ids`]).
/// Never errors on empty/absent source data.
pub async fn resolve(
    pool: &SqlitePool,
    backpack_id: i64,
    limit: i64,
) -> Result<Vec<ResolvedTrack>> {
    resolve_with_limit(pool, backpack_id, limit.clamp(1, 500)).await
}

/// Resolve a backpack's full track set without the 1..=500 clamp of [`resolve`].
///
/// Used by the active-scope filter (#331), which must see every scoped track.
/// Semantics (union / dedup / scoring / ordering) are identical to [`resolve`].
pub async fn resolve_unbounded(pool: &SqlitePool, backpack_id: i64) -> Result<Vec<ResolvedTrack>> {
    resolve_with_limit(pool, backpack_id, i64::MAX).await
}

async fn resolve_with_limit(
    pool: &SqlitePool,
    backpack_id: i64,
    limit: i64,
) -> Result<Vec<ResolvedTrack>> {
    let user_id: Option<i64> =
        sqlx::query_scalar("SELECT user_id FROM hub_backpacks WHERE id = ?1")
            .bind(backpack_id)
            .fetch_optional(pool)
            .await?;
    let Some(user_id) = user_id else {
        return Ok(vec![]);
    };

    let items = list_sources(pool, backpack_id).await?;

    // Resolve each item to (source target, item position, ordered track ids).
    // Source identity is the *target* (kind + id/lens key): a repeated target
    // counts once for scoring.
    let mut contributions: Vec<(ResolvedSource, i64, Vec<i64>)> = Vec::new();
    let mut seen: HashSet<ResolvedSource> = HashSet::new();

    for item in &items {
        let (source, tracks) = match item.source_kind.as_str() {
            "tag" => {
                let Some(tag_id) = item.source_tag_id else {
                    continue;
                };
                (ResolvedSource::Tag { tag_id }, tracks_for_tag(pool, tag_id).await?)
            }
            "playlist" => {
                let Some(playlist_id) = item.source_playlist_id else {
                    continue;
                };
                (
                    ResolvedSource::Playlist { playlist_id },
                    tracks_for_playlist(pool, playlist_id).await?,
                )
            }
            "lens" => {
                let lens = item.source_lens.clone().unwrap_or_default();
                let tracks = tracks_for_lens(pool, user_id, &lens).await?;
                (ResolvedSource::Lens { lens }, tracks)
            }
            "track" => {
                let Some(track_id) = item.source_track_id else {
                    continue;
                };
                (ResolvedSource::Track { track_id }, vec![track_id])
            }
            // The DB CHECK forbids any other kind; skip defensively.
            _ => continue,
        };

        if !seen.insert(source.clone()) {
            continue;
        }
        contributions.push((source, item.position, tracks));
    }

    Ok(fold_sources(&contributions, limit))
}

/// Fold per-source contributions into scored, deduped, ordered, limited tracks.
fn fold_sources(
    contributions: &[(ResolvedSource, i64, Vec<i64>)],
    limit: i64,
) -> Vec<ResolvedTrack> {
    struct Acc {
        score: i64,
        first_position: i64,
        // (position, kind rank, source)
        sources: Vec<(i64, u8, ResolvedSource)>,
    }

    let mut by_track: BTreeMap<i64, Acc> = BTreeMap::new();
    for (source, position, tracks) in contributions {
        let rank = kind_rank(source);
        // A track can only count once per contributing source target, even if
        // that target's query were to yield it more than once.
        let mut counted: HashSet<i64> = HashSet::new();
        for &track_id in tracks {
            if !counted.insert(track_id) {
                continue;
            }
            let acc = by_track.entry(track_id).or_insert_with(|| Acc {
                score: 0,
                first_position: *position,
                sources: Vec::new(),
            });
            acc.score += 1;
            if *position < acc.first_position {
                acc.first_position = *position;
            }
            acc.sources.push((*position, rank, source.clone()));
        }
    }

    let mut ordered: Vec<(i64, Acc)> = by_track.into_iter().collect();
    ordered.sort_by(|(ta, a), (tb, b)| {
        b.score
            .cmp(&a.score)
            .then(a.first_position.cmp(&b.first_position))
            .then(ta.cmp(tb))
    });

    let mut out: Vec<ResolvedTrack> = ordered
        .into_iter()
        .map(|(track_id, mut acc)| {
            acc.sources
                .sort_by(|(pa, ra, _), (pb, rb, _)| pa.cmp(pb).then(ra.cmp(rb)));
            ResolvedTrack {
                track_id,
                score: acc.score,
                sources: acc
                    .sources
                    .into_iter()
                    .map(|(_, _, src)| src)
                    .collect(),
            }
        })
        .collect();

    out.truncate(limit as usize);
    out
}

/// Hub track ids carrying `tag_id` (via `hub_track_resolved_tags`), id-ordered.
async fn tracks_for_tag(pool: &SqlitePool, tag_id: i64) -> Result<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT track_id FROM hub_track_resolved_tags WHERE tag_id = ?1 ORDER BY track_id",
    )
    .bind(tag_id)
    .fetch_all(pool)
    .await?)
}

/// Hub track ids in a playlist, in playlist order.
async fn tracks_for_playlist(pool: &SqlitePool, playlist_id: i64) -> Result<Vec<i64>> {
    Ok(sqlx::query_scalar(
        "SELECT track_id FROM hub_playlist_tracks WHERE playlist_id = ?1 ORDER BY position",
    )
    .bind(playlist_id)
    .fetch_all(pool)
    .await?)
}

/// Resolve a smart lens to hub track ids using the same rules as
/// [`crate::curate::resolve_track_ids`], scoped to the backpack's `user_id`.
/// Unknown/empty lens keys fall back to `newest` (never errors).
async fn tracks_for_lens(pool: &SqlitePool, user_id: i64, lens: &str) -> Result<Vec<i64>> {
    let ids = match lens {
        "liked" => {
            sqlx::query_scalar(
                "SELECT track_id FROM hub_liked_tracks
                  WHERE user_id = ?1 ORDER BY liked_at DESC",
            )
            .bind(user_id)
            .fetch_all(pool)
            .await?
        }
        "unheard" => {
            sqlx::query_scalar(
                "SELECT t.id
                   FROM hub_tracks t
                   LEFT JOIN hub_v_track_traktor v ON v.track_id = t.id
                  ORDER BY COALESCE(v.play_count, 0) ASC, t.first_seen_at DESC",
            )
            .fetch_all(pool)
            .await?
        }
        // "newest" and any unknown/empty key fall back to newest-in-hub.
        _ => {
            sqlx::query_scalar("SELECT id FROM hub_tracks ORDER BY first_seen_at DESC, id DESC")
                .fetch_all(pool)
                .await?
        }
    };
    Ok(ids)
}

// ── Export (#333) ─────────────────────────────────────────────────────────────

/// Format-agnostic export view of one resolved backpack track.
///
/// Carries the hub track id plus the metadata the m3u8 / NML / ZIP builders
/// need. `score` is kept so a caller can order or annotate without re-resolving.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportTrack {
    pub track_id: i64,
    pub title: String,
    pub artists: String,
    pub score: i64,
}

/// The backpack row when it belongs to `user_id`, else `None`.
///
/// Basis for the export owner check: a foreign backpack is indistinguishable
/// from a missing one (both `None`), so the handler yields a 404 without a
/// cross-user leak.
pub async fn backpack_owned_by(
    pool: &SqlitePool,
    backpack_id: i64,
    user_id: i64,
) -> Result<Option<Backpack>> {
    Ok(sqlx::query_as::<_, Backpack>(
        "SELECT id, user_id, name, is_active, created_at, updated_at
           FROM hub_backpacks
          WHERE id = ?1 AND user_id = ?2",
    )
    .bind(backpack_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?)
}

/// Load a backpack's resolved tracks as [`ExportTrack`] rows.
///
/// Source is [`resolve_unbounded`] (no 1..=500 clamp), so the output order is
/// exactly the resolve order (score desc -> first-position -> track_id). The
/// `title` / `artists` come from `hub_tracks` via `COALESCE(..., '')`: a missing
/// row or NULL column degrades to empty strings and never errors. An
/// unknown/empty backpack yields `Ok(vec![])`.
pub async fn export_tracks(pool: &SqlitePool, backpack_id: i64) -> Result<Vec<ExportTrack>> {
    let resolved = resolve_unbounded(pool, backpack_id).await?;
    let mut out = Vec::with_capacity(resolved.len());
    for rt in resolved {
        let meta: Option<(String, String)> = sqlx::query_as(
            "SELECT COALESCE(title, '') AS title, COALESCE(artists, '') AS artists
               FROM hub_tracks WHERE id = ?1",
        )
        .bind(rt.track_id)
        .fetch_optional(pool)
        .await?;
        let (title, artists) = meta.unwrap_or_default();
        out.push(ExportTrack {
            track_id: rt.track_id,
            title,
            artists,
            score: rt.score,
        });
    }
    Ok(out)
}

/// Export a backpack as an m3u8 text playlist.
///
/// `#EXTM3U` first, then an optional `#PLAYLIST:{name}` header (only when the
/// trimmed name is non-empty), then per track exactly two lines: an
/// `#EXTINF:-1,{artists} - {title}` info line and a `/track/{id}/stream` URL.
/// Line endings are `\n` with a trailing newline; the order is stable (input
/// order, i.e. resolve order) and no time/random values are written. An empty
/// list yields just the header (`#EXTM3U` [+ `#PLAYLIST`]) and never panics.
///
/// Caveat: `/track/{id}/stream` is session-bound, so the playlist is meant for
/// authenticated hub consumers (e.g. VLC with a session cookie), not open
/// third-party contexts.
pub fn m3u8_playlist(name: &str, tracks: &[ExportTrack]) -> String {
    let mut s = String::new();
    s.push_str("#EXTM3U\n");
    let name = name.trim();
    if !name.is_empty() {
        s.push_str("#PLAYLIST:");
        s.push_str(name);
        s.push('\n');
    }
    for t in tracks {
        s.push_str("#EXTINF:-1,");
        s.push_str(&t.artists);
        s.push_str(" - ");
        s.push_str(&t.title);
        s.push('\n');
        s.push_str(&format!("/track/{}/stream\n", t.track_id));
    }
    s
}

/// XML-escape the five markup-significant characters (`& < > " '`).
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Export a backpack as a Traktor NML playlist document.
///
/// Mirrors the node structure [`crate::traktor::import_str`] understands: a
/// `<COLLECTION>` with one `<ENTRY>` per track (`LOCATION/FILE`, `TITLE`,
/// `ARTIST`) and a `<PLAYLISTS>` tree with `NODE TYPE="FOLDER"` wrapping a
/// `NODE TYPE="PLAYLIST"` holding `PLAYLIST/ENTRY/PRIMARYKEY` references. Both
/// the `FILE` basename and the `PRIMARYKEY KEY` basename are
/// `{artists} - {title}.{ext}` (so import's basename mapping stays consistent).
/// `title` / `artists` / `name` are XML-escaped. An empty list yields a valid
/// NML with `ENTRIES="0"`.
pub fn traktor_nml(name: &str, tracks: &[ExportTrack], ext: &str) -> String {
    let ext = ext.trim_start_matches('.');
    let n = tracks.len();
    let mut s = String::new();
    s.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"no\"?>\n");
    s.push_str("<NML VERSION=\"19\">\n");
    s.push_str("<HEAD COMPANY=\"music-hub\" PRODUCT=\"music-hub\" PRODUCTVERSION=\"19\"></HEAD>\n");
    s.push_str("<MUSICFOLDERS></MUSICFOLDERS>\n");
    s.push_str(&format!("<COLLECTION ENTRIES=\"{n}\">\n"));
    for t in tracks {
        let file = format!("{} - {}.{}", t.artists, t.title, ext);
        s.push_str("<ENTRY>\n");
        s.push_str(&format!(
            "<LOCATION DIR=\"/:hub:/\" FILE=\"{}\"></LOCATION>\n",
            xml_escape(&file)
        ));
        s.push_str("<INFO></INFO>\n");
        s.push_str(&format!("<TITLE>{}</TITLE>\n", xml_escape(&t.title)));
        s.push_str(&format!("<ARTIST>{}</ARTIST>\n", xml_escape(&t.artists)));
        s.push_str("</ENTRY>\n");
    }
    s.push_str("</COLLECTION>\n");
    s.push_str("<PLAYLISTS>\n");
    s.push_str("<NODE TYPE=\"FOLDER\" NAME=\"Hub\">\n");
    s.push_str(&format!(
        "<NODE TYPE=\"PLAYLIST\" NAME=\"{}\">\n",
        xml_escape(name)
    ));
    s.push_str(&format!("<PLAYLIST ENTRIES=\"{n}\">\n"));
    for t in tracks {
        let key = format!("{} - {}.{}", t.artists, t.title, ext);
        s.push_str("<ENTRY>\n");
        s.push_str(&format!(
            "<PRIMARYKEY TYPE=\"TRACK\" KEY=\"{}\"></PRIMARYKEY>\n",
            xml_escape(&key)
        ));
        s.push_str("</ENTRY>\n");
    }
    s.push_str("</PLAYLIST>\n");
    s.push_str("</NODE>\n");
    s.push_str("</NODE>\n");
    s.push_str("</PLAYLISTS>\n");
    s.push_str("</NML>\n");
    s
}

#[cfg(test)]
mod export_tests {
    use super::*;

    fn tr(id: i64, title: &str, artists: &str, score: i64) -> ExportTrack {
        ExportTrack {
            track_id: id,
            title: title.to_string(),
            artists: artists.to_string(),
            score,
        }
    }

    #[test]
    fn m3u8_empty_has_only_header() {
        let s = m3u8_playlist("Reise", &[]);
        assert_eq!(s, "#EXTM3U\n#PLAYLIST:Reise\n");
    }

    #[test]
    fn m3u8_empty_name_omits_playlist_header() {
        let s = m3u8_playlist("   ", &[]);
        assert_eq!(s, "#EXTM3U\n");
    }

    #[test]
    fn m3u8_preserves_order_and_fields() {
        let tracks = vec![tr(7, "Alpha", "Aaa", 2), tr(3, "Beta", "Bbb", 1)];
        let s = m3u8_playlist("Mix", &tracks);
        assert!(s.starts_with("#EXTM3U\n#PLAYLIST:Mix\n"));
        assert!(s.contains("#EXTINF:-1,Aaa - Alpha\n/track/7/stream\n"));
        assert!(s.contains("#EXTINF:-1,Bbb - Beta\n/track/3/stream\n"));
        // order: 7 before 3
        let p7 = s.find("/track/7/stream").unwrap();
        let p3 = s.find("/track/3/stream").unwrap();
        assert!(p7 < p3);
        assert!(s.ends_with("\n"));
    }

    #[test]
    fn m3u8_single_track_has_two_lines() {
        let tracks = vec![tr(1, "Solo", "One", 5)];
        let s = m3u8_playlist("P", &tracks);
        assert_eq!(
            s,
            "#EXTM3U\n#PLAYLIST:P\n#EXTINF:-1,One - Solo\n/track/1/stream\n"
        );
    }

    #[test]
    fn nml_parses_and_has_playlist_node() {
        let tracks = vec![tr(1, "Alpha", "Aaa", 2), tr(2, "Beta", "Bbb", 1)];
        let s = traktor_nml("Mix", &tracks, "flac");
        let doc = roxmltree::Document::parse(&s).expect("valid NML");
        assert!(doc.descendants().any(|n| n.has_tag_name("COLLECTION")));
        let nodes: Vec<_> = doc
            .descendants()
            .filter(|n| n.has_tag_name("NODE") && n.attribute("TYPE") == Some("PLAYLIST"))
            .collect();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].attribute("NAME"), Some("Mix"));
        let pks: Vec<_> = doc
            .descendants()
            .filter(|n| n.has_tag_name("PRIMARYKEY"))
            .map(|n| n.attribute("KEY").unwrap().to_string())
            .collect();
        assert_eq!(pks, vec!["Aaa - Alpha.flac", "Bbb - Beta.flac"]);
    }

    #[test]
    fn nml_escapes_special_chars() {
        let tracks = vec![tr(1, "A&B <C>", "D\"e'f", 1)];
        let s = traktor_nml("Mix & More", &tracks, "mp3");
        let doc = roxmltree::Document::parse(&s).expect("valid NML after escaping");
        let coll = doc
            .descendants()
            .find(|n| n.has_tag_name("COLLECTION"))
            .unwrap();
        let entry = coll.children().find(|n| n.has_tag_name("ENTRY")).unwrap();
        let title = entry
            .children()
            .find(|n| n.has_tag_name("TITLE"))
            .unwrap()
            .text()
            .unwrap();
        assert_eq!(title, "A&B <C>");
        let loc = entry
            .children()
            .find(|n| n.has_tag_name("LOCATION"))
            .unwrap();
        let file = loc.attribute("FILE").unwrap();
        let pk = doc
            .descendants()
            .find(|n| n.has_tag_name("PRIMARYKEY"))
            .unwrap()
            .attribute("KEY")
            .unwrap();
        assert_eq!(file, pk, "KEY basename == FILE basename");
        assert_eq!(file, "D\"e'f - A&B <C>.mp3");
    }

    #[test]
    fn nml_empty_is_valid() {
        let s = traktor_nml("Empty", &[], "flac");
        let doc = roxmltree::Document::parse(&s).expect("valid empty NML");
        let coll = doc
            .descendants()
            .find(|n| n.has_tag_name("COLLECTION"))
            .unwrap();
        assert_eq!(coll.attribute("ENTRIES"), Some("0"));
        assert_eq!(
            doc.descendants()
                .filter(|n| n.has_tag_name("ENTRY"))
                .count(),
            0
        );
    }
}
