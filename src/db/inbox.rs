//! Tag roundtrip inbox — files whose stored comment does not match the
//! generated target comment.
//!
//! "Roundtrip" here means: `parse(comment) → generate(parsed) → compare`.
//! Two comments that parse to the same structured content (same tags, PMV
//! chars, source IDs) are treated as equal even if their raw formatting
//! differs (tag order, quoting, case). This prevents false positives that a
//! naive string comparison would produce.
//!
//! The generated target comment is computed with the existing needs-comment
//! logic (`compute_target_comment` / `compute_target_comments_batch`), which
//! derives tags from the file→track→playlist→tag chain. The inbox therefore
//! shows exactly the files that would be (re-)commented / re-tagged when the
//! bulk comment writers run.
//!
//! Full feature set (see `plans/proposed/tag-roundtrip-inbox.md`):
//!
//! * **Similar-tag suggestions** — every NEW tag in an item's diff (a tag
//!   that does not yet exist canonically, from either side of the diff) is
//!   matched against the existing tag vocabulary with case-insensitive
//!   Levenshtein distance ≤ 2. The user sees "I meant THIS existing tag".
//! * **Staging / rename / merge** — the user resolves a new tag via
//!   `tag_inbox` (rename → new spelling, merge → existing tag, dismiss).
//!   Resolving only records a decision; the next comment write consults the
//!   open mappings and writes the canonical (mapped) tag. No auto-apply.
//!
//! Mappings are applied to the target BEFORE the diff is computed, so the
//! inbox always shows the *staged* state: after a merge the target reflects
//! the canonical tag, and once a file is written it drops out of the inbox.

use anyhow::Result;
use serde::Serialize;
use sqlx::{Pool, Row, Sqlite};
use std::collections::{HashMap, HashSet};

use crate::comment::{
    CommentDiff, apply_tag_mappings_to_target, diff_comment_strings, generate_target_comment,
    similar_tags,
};

/// One file in the inbox: file identity + stored comment + generated target
/// comment + the structured roundtrip diff between them.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InboxFileItem {
    pub file_id: i64,
    pub file_path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    /// Stored (canonical DB) comment.
    pub comment: Option<String>,
    /// Generated target comment (what the file *should* carry), after open
    /// tag-inbox mappings have been applied.
    pub target_comment: String,
    /// Roundtrip diff stored → target. `tags_added` = tags the target has but
    /// the stored comment lacks; `tags_removed` = tags only in the stored
    /// comment. `raw_comment_changed` = stored comment was unparseable and
    /// differs from the target string.
    pub diff: CommentDiff,
    /// New tags in this item's diff (not canonically established), each with
    /// fuzzy suggestions of similar EXISTING tags + the open mapping (if any).
    pub new_tags: Vec<NewTagInfo>,
}

/// A new (not-yet-canonical) tag in an inbox item and its fuzzy suggestions.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NewTagInfo {
    /// The tag as it appears in the diff (lowercase).
    pub tag: String,
    /// `true` when the tag comes from the target side (`diff.tags_added`),
    /// `false` when it comes from the stored comment (`diff.tags_removed`).
    pub added: bool,
    /// Similar EXISTING tags (case-insensitive Levenshtein ≤ 2), sorted by
    /// distance then name.
    pub suggestions: Vec<TagSuggestion>,
    /// The open `tag_inbox` mapping for this tag, if the user already
    /// resolved it (rename/merge/dismiss).
    pub mapping: Option<TagInboxMapping>,
}

/// A similar existing tag offered as a click-to-merge target.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TagSuggestion {
    pub tag: String,
    pub distance: usize,
    /// Files currently tagged with this tag (informational).
    pub count: i64,
}

/// A row of the `tag_inbox` staging table (user decision for a new tag).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct TagInboxMapping {
    pub id: i64,
    pub raw_tag: String,
    pub action: String,
    pub target_tag: String,
    pub status: String,
    pub created_at: Option<i64>,
    pub resolved_at: Option<i64>,
    pub file_count: i64,
}

/// All existing tag names — the canonical vocabulary for fuzzy matching.
async fn get_existing_tag_names(pool: &Pool<Sqlite>) -> Result<Vec<String>> {
    let rows = sqlx::query("SELECT name FROM tags ORDER BY name")
        .fetch_all(pool)
        .await?;
    Ok(rows.iter().map(|r| r.get::<String, _>("name")).collect())
}

/// tag_name (lowercase) → number of files currently tagged with it.
/// Single query so suggestion counts are cheap even for large inboxes.
async fn get_tag_file_counts(pool: &Pool<Sqlite>) -> Result<HashMap<String, i64>> {
    let rows = sqlx::query(
        "SELECT tag_name, COUNT(DISTINCT file_id) AS file_count \
         FROM file_resolved_tags GROUP BY tag_name",
    )
    .fetch_all(pool)
    .await?;
    let mut counts = HashMap::new();
    for r in rows {
        counts.insert(
            r.get::<String, _>("tag_name").to_lowercase(),
            r.get::<i64, _>("file_count"),
        );
    }
    Ok(counts)
}

/// Build the `new_tags` list for one diff: every tag on either side of the
/// diff (deduplicated case-insensitively), annotated with fuzzy suggestions
/// and the user's open mapping (if any).
fn build_new_tags(
    diff: &CommentDiff,
    tag_names: &[String],
    counts: &HashMap<String, i64>,
    mappings: &[TagInboxMapping],
) -> Vec<NewTagInfo> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<NewTagInfo> = Vec::new();

    for (tag, added) in diff
        .tags_added
        .iter()
        .map(|t| (t.clone(), true))
        .chain(diff.tags_removed.iter().map(|t| (t.clone(), false)))
    {
        if !seen.insert(tag.to_lowercase()) {
            continue;
        }
        let suggestions = similar_tags(&tag, tag_names, 2)
            .into_iter()
            .map(|(name, distance)| TagSuggestion {
                count: counts.get(&name.to_lowercase()).copied().unwrap_or(0),
                tag: name,
                distance,
            })
            .collect();
        let mapping = mappings
            .iter()
            .find(|m| m.raw_tag.eq_ignore_ascii_case(&tag))
            .cloned();
        out.push(NewTagInfo {
            tag,
            added,
            suggestions,
            mapping,
        });
    }

    out.sort_by(|a, b| a.tag.cmp(&b.tag));
    out
}

/// Internal: load every file's stored comment + source IDs, compute the
/// target comment for each (batch), apply open tag-inbox mappings (staging),
/// and return (file_id, comment, target, roundtrip diff) for every file that
/// has a non-empty diff.
async fn diff_all_files(
    pool: &Pool<Sqlite>,
) -> Result<Vec<(i64, Option<String>, String, CommentDiff, Vec<NewTagInfo>)>> {
    let rows = sqlx::query(
        "SELECT id, comment, spotify_id, soundcloud_id, youtube_id FROM files",
    )
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let file_ids: Vec<i64> = rows.iter().map(|r| r.get::<i64, _>("id")).collect();

    // Batch-compute target comments for files that HAVE resolved tags.
    let mut targets = crate::db::files::compute_target_comments_batch(pool, &file_ids).await?;

    // Files without resolved tags are skipped by the batch helper. Their
    // target is still well-defined: `[___]` + source IDs (from the file row).
    for row in &rows {
        let fid: i64 = row.get("id");
        if targets.contains_key(&fid) {
            continue;
        }
        let sp: Option<String> = row.get("spotify_id");
        let sc: Option<String> = row.get("soundcloud_id");
        let yt: Option<String> = row.get("youtube_id");
        targets.insert(
            fid,
            generate_target_comment('_', '_', '_', &[], sp.as_deref(), sc.as_deref(), yt.as_deref()),
        );
    }

    // Full feature set: existing vocabulary + file counts + open mappings.
    let tag_names = get_existing_tag_names(pool).await?;
    let counts = get_tag_file_counts(pool).await?;
    let mappings = get_open_tag_mappings(pool).await?;
    let mapping_map: HashMap<String, String> = mappings
        .iter()
        .filter(|m| m.status == "open" && (m.action == "rename" || m.action == "merge"))
        .map(|m| (m.raw_tag.to_lowercase(), m.target_tag.to_lowercase()))
        .collect();

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let fid: i64 = row.get("id");
        let comment: Option<String> = row.get("comment");
        let mut target = targets.get(&fid).cloned().unwrap_or_default();
        // Staging: the target shown (and later written) uses canonical spellings.
        target = apply_tag_mappings_to_target(&target, comment.as_deref(), &mapping_map);
        let diff = diff_comment_strings(comment.as_deref(), Some(&target));
        if !diff.is_empty() {
            let new_tags = build_new_tags(&diff, &tag_names, &counts, &mappings);
            out.push((fid, comment, target, diff, new_tags));
        }
    }

    Ok(out)
}

/// Fetch the inbox: files whose stored comment ≠ generated target comment
/// (roundtrip-compared, after open tag-inbox mappings). Sorted by file id,
/// paginated.
///
/// Pagination happens AFTER the roundtrip filter so pages are stable
/// (a page of N always contains N files that actually need an update).
pub async fn get_inbox_files(
    pool: &Pool<Sqlite>,
    limit: i64,
    offset: i64,
) -> Result<Vec<InboxFileItem>> {
    let mut matched = diff_all_files(pool).await?;
    matched.sort_by_key(|(fid, _, _, _, _)| *fid);

    let start = (offset.max(0) as usize).min(matched.len());
    let end = (start + limit.max(0) as usize).min(matched.len());

    let mut result = Vec::with_capacity(end - start);
    for (fid, comment, target, diff, new_tags) in matched.into_iter().skip(start).take(end - start) {
        let row = sqlx::query("SELECT file_path, title, artist FROM files WHERE id = ?")
            .bind(fid)
            .fetch_one(pool)
            .await?;
        result.push(InboxFileItem {
            file_id: fid,
            file_path: row.get("file_path"),
            title: row.get("title"),
            artist: row.get("artist"),
            comment,
            target_comment: target,
            diff,
            new_tags,
        });
    }

    Ok(result)
}

/// Count files whose stored comment ≠ generated target comment
/// (roundtrip-compared). Used for the nav badge / inbox header.
pub async fn get_inbox_count(pool: &Pool<Sqlite>) -> Result<i64> {
    let matched = diff_all_files(pool).await?;
    Ok(matched.len() as i64)
}

// ============================================================================
// Tag-inbox staging (rename / merge / dismiss decisions)
// ============================================================================

/// True when the `tag_inbox` staging table exists (migration 023 applied).
/// Guards the write path for DBs that predate the migration (e.g. partial
/// unit-test schemas) — mappings are simply not applied then.
pub async fn tag_inbox_table_exists(pool: &Pool<Sqlite>) -> bool {
    sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'tag_inbox'",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0)
        > 0
}

/// All `tag_inbox` rows with status `open`, ordered by raw tag.
pub async fn get_open_tag_mappings(pool: &Pool<Sqlite>) -> Result<Vec<TagInboxMapping>> {
    if !tag_inbox_table_exists(pool).await {
        return Ok(Vec::new());
    }
    let rows = sqlx::query_as::<_, TagInboxMapping>(
        "SELECT * FROM tag_inbox WHERE status = 'open' ORDER BY raw_tag",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// raw_tag (lowercase) → target_tag (lowercase) for open rename/merge
/// mappings. This is the map the write path consults when generating targets.
pub async fn load_tag_inbox_mapping_map(pool: &Pool<Sqlite>) -> Result<HashMap<String, String>> {
    if !tag_inbox_table_exists(pool).await {
        return Ok(HashMap::new());
    }
    let rows = sqlx::query(
        "SELECT raw_tag, target_tag FROM tag_inbox \
         WHERE status = 'open' AND action IN ('rename', 'merge')",
    )
    .fetch_all(pool)
    .await?;
    let mut map = HashMap::new();
    for r in rows {
        map.insert(
            r.get::<String, _>("raw_tag").to_lowercase(),
            r.get::<String, _>("target_tag").to_lowercase(),
        );
    }
    Ok(map)
}

/// Record (or update) the user's decision for a new tag. Normalizes both
/// sides to lowercase (comments store tags lowercased). `file_count` is
/// updated to the number of files currently carrying `raw_tag` in their
/// stored comment (informational for the UI).
pub async fn upsert_tag_inbox_mapping(
    pool: &Pool<Sqlite>,
    raw_tag: &str,
    action: &str,
    target_tag: &str,
) -> Result<TagInboxMapping> {
    let raw = raw_tag.trim().to_lowercase();
    let target = target_tag.trim().to_lowercase();
    if raw.is_empty() {
        anyhow::bail!("raw_tag must not be empty");
    }
    if target.is_empty() {
        anyhow::bail!("target_tag must not be empty");
    }

    // Files currently carrying the raw tag in their stored comment
    // (informational for the UI). Token match via the comment parser.
    let file_count = count_files_with_tag(pool, &raw).await?;

    let row = sqlx::query_as::<_, TagInboxMapping>(
        r#"
        INSERT INTO tag_inbox (raw_tag, action, target_tag, status, created_at, file_count)
        VALUES (?, ?, ?, 'open', unixepoch(), ?)
        ON CONFLICT(raw_tag) DO UPDATE SET
            action = excluded.action,
            target_tag = excluded.target_tag,
            status = 'open',
            resolved_at = NULL,
            file_count = excluded.file_count
        RETURNING *
        "#,
    )
    .bind(&raw)
    .bind(action)
    .bind(&target)
    .bind(file_count)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Count files whose STORED comment contains `tag` as a parsed tag.
async fn count_files_with_tag(pool: &Pool<Sqlite>, tag: &str) -> Result<i64> {
    // Load all stored comments once and count in Rust — the comment parser is
    // the single source of truth for what counts as a tag.
    let rows = sqlx::query("SELECT id, comment FROM files")
        .fetch_all(pool)
        .await?;
    let mut count = 0i64;
    for r in rows {
        let comment: Option<String> = r.get("comment");
        let Some(c) = comment else { continue };
        let Some(parsed) = crate::comment::parse_comment(&c) else { continue };
        if parsed.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            count += 1;
        }
    }
    Ok(count)
}

/// Mark a mapping as written/applied (after a write-comment run picked it up).
pub async fn mark_tag_inbox_mapping_applied(pool: &Pool<Sqlite>, raw_tag: &str) -> Result<()> {
    sqlx::query(
        "UPDATE tag_inbox SET status = 'applied', resolved_at = unixepoch() \
         WHERE raw_tag = ? COLLATE NOCASE AND status = 'open'",
    )
    .bind(raw_tag)
    .execute(pool)
    .await?;
    Ok(())
}
