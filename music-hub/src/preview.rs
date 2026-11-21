//! Shared comment-template preview (issue #670, slice C4; source switched in #678, slice C6).
//!
//! Renders a template body against the caller's **top tagged tracks**, ordered
//! by relevance (see [`top_tagged_tracks`]), limited to the first
//! [`PREVIEW_LIMIT`] tracks, and returns the data the
//! `/kellner/{id}/template/preview` and `/comment-templates/{id}/preview`
//! handlers render into the `_partials/comment_preview.html` fragment.
//!
//! A track counts as *tagged* for a user when it carries at least one resolved
//! tag (`hub_track_resolved_tags`) whose owning tag (`hub_tags.owner_user_id`)
//! belongs to that user. The source is the **hub DB only** — no network, no
//! `MUSIC_API_TOKEN`, no backpacks.
//!
//! The rendering itself goes through [`comment_template::render`] — the exact
//! same function the kellner write path uses — so the preview is **byte-identical**
//! to the Comment later written into the file (see
//! `kellner/tests/comment_preview_parity.rs`). The preview never truncates a
//! second time: [`comment_template::COMMENT_MAX_CHARS`] is already applied inside
//! `render`.

use anyhow::Result;
use askama::Template;
use sqlx::SqlitePool;

use comment_template::{COMMENT_MAX_CHARS, TrackFacts};

/// How many tracks the preview lists (the counter still reports the full `N`).
pub const PREVIEW_LIMIT: usize = 3;

/// One preview row: the track title, the rendered Comment and its length.
pub struct PreviewRow {
    pub title: String,
    pub comment: String,
    /// `rendered.chars().count()`, i.e. the `n` in `n/512`.
    pub chars: usize,
}

/// The rendered `_partials/comment_preview.html` fragment data.
#[derive(Template)]
#[template(path = "_partials/comment_preview.html")]
pub struct CommentPreview {
    pub rows: Vec<PreviewRow>,
    /// `min(PREVIEW_LIMIT, total)` — the `3` in `3 von N`.
    pub shown: usize,
    /// The number of the user's tagged tracks (without limit) — the `N`.
    pub total: usize,
    /// No tagged track at all → the empty hint instead of an empty list.
    pub empty: bool,
    /// The Comment ceiling (`512`).
    pub max_chars: usize,
}

/// One resolved track of the preview (deduped by `track_id`).
pub struct PreviewTrack {
    pub title: String,
    pub facts: TrackFacts,
}

/// The track title from `hub_tracks` (`""` when the row/column is absent).
async fn track_title(pool: &SqlitePool, track_id: i64) -> sqlx::Result<String> {
    let row: Option<(String,)> =
        sqlx::query_as("SELECT COALESCE(title, '') FROM hub_tracks WHERE id = ?1")
            .bind(track_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|(t,)| t).unwrap_or_default())
}

/// The track's tag names, alphabetical (`ORDER BY h.name`); mirrors
/// `backpack_api.rs:122`.
async fn track_tags(pool: &SqlitePool, track_id: i64) -> sqlx::Result<Vec<String>> {
    sqlx::query_scalar(
        "SELECT h.name
           FROM hub_track_resolved_tags rt
           JOIN hub_tags h ON h.id = rt.tag_id
          WHERE rt.track_id = ?1
          ORDER BY h.name",
    )
    .bind(track_id)
    .fetch_all(pool)
    .await
}

/// The track's resolved `bpm` / `musical_key` / `energy`; mirrors
/// `backpack_api.rs:137`. All `None` when the row (or any column) is absent.
async fn track_audio(
    pool: &SqlitePool,
    track_id: i64,
) -> sqlx::Result<(Option<f64>, Option<String>, Option<f64>)> {
    let row: Option<(Option<f64>, Option<String>, Option<f64>)> =
        sqlx::query_as("SELECT bpm, musical_key, energy FROM hub_track_audio WHERE track_id = ?1")
            .bind(track_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.unwrap_or((None, None, None)))
}

/// `N` — the number of the user's tagged tracks (a track counts once however
/// many of the user's tags it carries).
pub async fn count_tagged_tracks(pool: &SqlitePool, user_id: i64) -> Result<usize> {
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT rt.track_id)
           FROM hub_track_resolved_tags rt
           JOIN hub_tags h ON h.id = rt.tag_id
          WHERE h.owner_user_id = ?1",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await?;
    Ok(count.max(0) as usize)
}

/// The top-`limit` tagged tracks of the user, in rank order:
/// 1. `tag_count DESC` (number of the user's own resolved tags on the track),
/// 2. `last_tagged_at DESC` (NULLs last),
/// 3. `id ASC` (stable tiebreak).
/// Facts (tags/bpm/key/energy) are joined per track afterwards.
pub async fn top_tagged_tracks(
    pool: &SqlitePool,
    user_id: i64,
    limit: usize,
) -> Result<Vec<PreviewTrack>> {
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT rt.track_id
           FROM hub_track_resolved_tags rt
           JOIN hub_tags h ON h.id = rt.tag_id
           JOIN hub_tracks t ON t.id = rt.track_id
          WHERE h.owner_user_id = ?1
          GROUP BY rt.track_id
          ORDER BY COUNT(h.id) DESC,
                   (t.last_tagged_at IS NULL) ASC,
                   t.last_tagged_at DESC,
                   rt.track_id ASC
          LIMIT ?2",
    )
    .bind(user_id)
    .bind(limit as i64)
    .fetch_all(pool)
    .await?;

    let mut out: Vec<PreviewTrack> = Vec::with_capacity(ids.len());
    for track_id in ids {
        let title = track_title(pool, track_id).await?;
        let tags = track_tags(pool, track_id).await?;
        let (bpm, key, energy) = track_audio(pool, track_id).await?;
        out.push(PreviewTrack {
            title,
            facts: TrackFacts {
                tags,
                bpm,
                key,
                energy,
            },
        });
    }
    Ok(out)
}

/// Build the partial data from already-collected tracks. `total` is the full
/// tagged-track count (independent of `tracks.len()`); `empty` is derived as
/// `total == 0`.
pub fn build_from(tracks: &[PreviewTrack], total: usize, body: &str) -> CommentPreview {
    let rows: Vec<PreviewRow> = tracks
        .iter()
        .take(PREVIEW_LIMIT)
        .map(|t| {
            let comment = comment_template::render(body, &t.facts);
            PreviewRow {
                title: t.title.clone(),
                chars: comment.chars().count(),
                comment,
            }
        })
        .collect();
    CommentPreview {
        rows,
        shown: total.min(PREVIEW_LIMIT),
        total,
        empty: total == 0,
        max_chars: COMMENT_MAX_CHARS,
    }
}

/// Load the user's top tagged tracks and render `body` into the preview
/// fragment data. No tagged track → the empty (hint) state.
pub async fn comment_preview(
    pool: &SqlitePool,
    user_id: i64,
    body: &str,
) -> Result<CommentPreview> {
    let total = count_tagged_tracks(pool, user_id).await?;
    let tracks = top_tagged_tracks(pool, user_id, PREVIEW_LIMIT).await?;
    Ok(build_from(&tracks, total, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(title: &str, tags: &[&str], bpm: Option<f64>) -> PreviewTrack {
        PreviewTrack {
            title: title.to_string(),
            facts: TrackFacts {
                tags: tags.iter().map(|s| s.to_string()).collect(),
                bpm,
                key: None,
                energy: None,
            },
        }
    }

    #[test]
    fn build_limits_to_three_and_counts_total_independently() {
        let tracks: Vec<PreviewTrack> = (0..25)
            .map(|i| t(&format!("T{i}"), &["Techno"], Some(128.0)))
            .collect();
        let p = build_from(&tracks, 25, "{tags} {bpm}");
        assert_eq!(p.rows.len(), PREVIEW_LIMIT, "only the first three rendered");
        assert_eq!(p.shown, 3);
        assert_eq!(p.total, 25);
        assert_eq!(p.rows[0].comment, "Techno BPM-128");
        assert_eq!(p.max_chars, 512);
        assert!(!p.empty);
    }

    #[test]
    fn total_may_exceed_track_len() {
        let tracks = vec![t("T1", &["Techno"], None)];
        let p = build_from(&tracks, 17, "{tags}");
        assert_eq!(p.rows.len(), 1, "only the tracks handed in are rendered");
        assert_eq!(p.shown, 3, "shown is driven by total, not tracks.len()");
        assert_eq!(p.total, 17);
        assert!(!p.empty);
    }

    #[test]
    fn empty_flag_when_total_is_zero() {
        let p = build_from(&[], 0, DEFAULT_BODY);
        assert!(p.empty);
        assert_eq!(p.shown, 0);
        assert_eq!(p.total, 0);
        assert!(p.rows.is_empty());
    }

    #[test]
    fn chars_are_scalar_count_and_over_512_ends_with_marker() {
        // A literal filler body pushes the rendered Comment past 512 chars.
        let body = format!("{{tags}} {}", "x ".repeat(300));
        let p = build_from(&[t("T1", &["Techno"], None)], 1, &body);
        assert_eq!(p.rows[0].chars, 512, "capped at the comment ceiling");
        assert!(p.rows[0].comment.ends_with("..."), "{}", p.rows[0].comment);
    }

    const DEFAULT_BODY: &str = "{tags} {bpm} {key} {energy}";
}
