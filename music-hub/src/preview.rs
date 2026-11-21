//! Shared comment-template preview (issue #670, slice C4).
//!
//! Renders a template body against the **union of the active backpacks** in
//! export order, limited to the first [`PREVIEW_LIMIT`] tracks, and returns the
//! data the `/kellner/{id}/template/preview` and `/comment-templates/{id}/preview`
//! handlers render into the `_partials/comment_preview.html` fragment.
//!
//! The rendering itself goes through [`comment_template::render`] — the exact
//! same function the kellner write path uses — so the preview is **byte-identical**
//! to the Comment later written into the file (see
//! `kellner/tests/comment_preview_parity.rs`). The preview never truncates a
//! second time: [`comment_template::COMMENT_MAX_CHARS`] is already applied inside
//! `render`.

use std::collections::HashSet;

use anyhow::Result;
use askama::Template;
use sqlx::SqlitePool;

use comment_template::{COMMENT_MAX_CHARS, TrackFacts};

/// How many tracks the preview lists (the counter still reports the full `N`).
pub const PREVIEW_LIMIT: usize = 10;

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
    /// `min(PREVIEW_LIMIT, total)` — the `10` in `10 von N`.
    pub shown: usize,
    /// The deduplicated track count across every active backpack — the `N`.
    pub total: usize,
    /// No active backpack at all → the empty hint instead of an empty list.
    pub empty: bool,
    /// The Comment ceiling (`512`).
    pub max_chars: usize,
}

/// One resolved track of the preview union (deduped by `track_id`).
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

/// Collect the union of the given backpacks' resolved tracks, in backpack order
/// (each backpack's own export order preserved), deduped by `track_id` (the
/// first occurrence wins).
pub async fn collect_tracks(pool: &SqlitePool, backpack_ids: &[i64]) -> Result<Vec<PreviewTrack>> {
    let mut seen: HashSet<i64> = HashSet::new();
    let mut out: Vec<PreviewTrack> = Vec::new();
    for &bp in backpack_ids {
        let resolved = crate::backpacks::resolve_unbounded(pool, bp).await?;
        for t in resolved {
            if !seen.insert(t.track_id) {
                continue;
            }
            let title = track_title(pool, t.track_id).await?;
            let tags = track_tags(pool, t.track_id).await?;
            let (bpm, key, energy) = track_audio(pool, t.track_id).await?;
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
    }
    Ok(out)
}

/// Build the partial data from already-collected tracks. `empty` marks "no active
/// backpack" (the list-less hint case) and is decided by the caller.
pub fn build_from(tracks: &[PreviewTrack], body: &str, empty: bool) -> CommentPreview {
    let total = tracks.len();
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
        empty,
        max_chars: COMMENT_MAX_CHARS,
    }
}

/// Collect the union of `backpack_ids` and render `body` into the preview
/// fragment data. `backpack_ids` empty → the empty (hint) state.
pub async fn comment_preview(
    pool: &SqlitePool,
    backpack_ids: &[i64],
    body: &str,
) -> Result<CommentPreview> {
    let empty = backpack_ids.is_empty();
    let tracks = collect_tracks(pool, backpack_ids).await?;
    Ok(build_from(&tracks, body, empty))
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
    fn build_limits_to_ten_and_counts_all() {
        let tracks: Vec<PreviewTrack> = (0..25)
            .map(|i| t(&format!("T{i}"), &["Techno"], Some(128.0)))
            .collect();
        let p = build_from(&tracks, "{tags} {bpm}", false);
        assert_eq!(p.rows.len(), PREVIEW_LIMIT, "only the first ten rendered");
        assert_eq!(p.shown, 10);
        assert_eq!(p.total, 25);
        assert_eq!(p.rows[0].comment, "Techno BPM-128");
        assert_eq!(p.max_chars, 512);
        assert!(!p.empty);
    }

    #[test]
    fn empty_flag_when_no_backpack() {
        let p = build_from(&[], DEFAULT_BODY, true);
        assert!(p.empty);
        assert_eq!(p.total, 0);
        assert!(p.rows.is_empty());
    }

    #[test]
    fn chars_are_scalar_count_and_over_512_ends_with_marker() {
        // A literal filler body pushes the rendered Comment past 512 chars.
        let body = format!("{{tags}} {}", "x ".repeat(300));
        let p = build_from(&[t("T1", &["Techno"], None)], &body, false);
        assert_eq!(p.rows[0].chars, 512, "capped at the comment ceiling");
        assert!(p.rows[0].comment.ends_with("..."), "{}", p.rows[0].comment);
    }

    const DEFAULT_BODY: &str = "{tags} {bpm} {key} {energy}";
}
