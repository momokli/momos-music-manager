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

use anyhow::Result;
use serde::Serialize;
use sqlx::{Pool, Row, Sqlite};

use crate::comment::{CommentDiff, diff_comment_strings, generate_target_comment};

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
    /// Generated target comment (what the file *should* carry).
    pub target_comment: String,
    /// Roundtrip diff stored → target. `tags_added` = tags the target has but
    /// the stored comment lacks; `tags_removed` = tags only in the stored
    /// comment. `raw_comment_changed` = stored comment was unparseable and
    /// differs from the target string.
    pub diff: CommentDiff,
}

/// Internal: load every file's stored comment + source IDs, compute the
/// target comment for each (batch), and return (file_id, comment, target,
/// roundtrip diff) for every file that has a non-empty diff.
async fn diff_all_files(
    pool: &Pool<Sqlite>,
) -> Result<Vec<(i64, Option<String>, String, CommentDiff)>> {
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

    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let fid: i64 = row.get("id");
        let comment: Option<String> = row.get("comment");
        let target = targets.get(&fid).cloned().unwrap_or_default();
        let diff = diff_comment_strings(comment.as_deref(), Some(&target));
        if !diff.is_empty() {
            out.push((fid, comment, target, diff));
        }
    }

    Ok(out)
}

/// Fetch the inbox: files whose stored comment ≠ generated target comment
/// (roundtrip-compared). Sorted by file id, paginated.
///
/// Pagination happens AFTER the roundtrip filter so pages are stable
/// (a page of N always contains N files that actually need an update).
pub async fn get_inbox_files(
    pool: &Pool<Sqlite>,
    limit: i64,
    offset: i64,
) -> Result<Vec<InboxFileItem>> {
    let mut matched = diff_all_files(pool).await?;
    matched.sort_by_key(|(fid, _, _, _)| *fid);

    let start = (offset.max(0) as usize).min(matched.len());
    let end = (start + limit.max(0) as usize).min(matched.len());

    let mut result = Vec::with_capacity(end - start);
    for (fid, comment, target, diff) in matched.into_iter().skip(start).take(end - start) {
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
