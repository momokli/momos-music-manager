//! Pure planning helpers for the Tonies sync (#501): desired-track list,
//! greedy capacity fit, and chapter diff.
//!
//! Everything here is offline and side-effect free. Vault readiness is injected
//! as a predicate so this module never depends on `music_api` or the network.

use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};

use super::Chapter;
use super::bindings::{SOURCE_BACKPACK, SOURCE_PLAYLIST, SOURCE_TAG};

/// One track the caller wants on the Tonie, in playlist order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesiredTrack {
    pub isrc: Option<String>,
    pub title: String,
    pub duration_ms: i64,
}

/// Why a desired track did not make it into the fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    /// No vault file for this ISRC (does not consume budget).
    NoVault,
    /// The track has no ISRC at all.
    NoIsrc,
    /// An earlier track with the same ISRC already won.
    DuplicateIsrc,
    /// Adding it would exceed the cap; the fit stops here.
    CapReached,
    /// A whole album has a track that is not in the vault yet. Albums are
    /// placed as a unit, so the album is skipped (never partially).
    AlbumNotReady,
}

/// A dropped track plus the reason, for user-facing reporting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkippedTrack {
    pub title: String,
    pub isrc: Option<String>,
    pub reason: SkipReason,
}

/// Result of a greedy fit against a capacity.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FitPlan {
    pub tracks: Vec<DesiredTrack>,
    pub seconds: i64,
    pub skipped: Vec<SkippedTrack>,
}

/// How a planned list relates to the Tonie's current chapters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffKind {
    Unchanged,
    Append,
    Replace,
}

impl Default for DiffKind {
    fn default() -> Self {
        DiffKind::Unchanged
    }
}

/// A plan for reconciling existing chapters with desired tracks.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiffPlan {
    pub kind: DiffKind,
    /// The full target chapter list.
    pub chapters: Vec<Chapter>,
    /// Existing chapters that can be reused verbatim (shared prefix).
    pub reuse: Vec<Chapter>,
    /// Tracks that still need an upload.
    pub upload: Vec<DesiredTrack>,
}

/// Convert milliseconds to seconds, rounding halves up.
pub fn ms_to_seconds(ms: i64) -> i64 {
    if ms >= 0 {
        (ms + 500) / 1000
    } else {
        -((-ms + 500) / 1000)
    }
}

/// Keep the first track per ISRC; later duplicates become `DuplicateIsrc`.
/// Tracks without an ISRC are passed through untouched.
pub fn dedupe_by_isrc(input: Vec<DesiredTrack>) -> (Vec<DesiredTrack>, Vec<SkippedTrack>) {
    let mut seen = std::collections::HashSet::new();
    let mut kept = Vec::with_capacity(input.len());
    let mut skipped = Vec::new();
    for track in input {
        match &track.isrc {
            Some(isrc) => {
                if seen.insert(isrc.clone()) {
                    kept.push(track);
                } else {
                    skipped.push(SkippedTrack {
                        title: track.title,
                        isrc: track.isrc,
                        reason: SkipReason::DuplicateIsrc,
                    });
                }
            }
            None => kept.push(track),
        }
    }
    (kept, skipped)
}

/// Greedily fill `cap_seconds` from `desired`, preserving playlist order.
///
/// * missing ISRC → `NoIsrc`, continue
/// * `!is_ready(isrc)` → `NoVault`, continue (no budget consumed)
/// * track would exceed the cap → `CapReached` for that track and stop
/// * exact fit (`== cap`) is accepted
pub fn greedy_fit(
    desired: &[DesiredTrack],
    cap_seconds: i64,
    is_ready: impl Fn(&str) -> bool,
) -> FitPlan {
    let mut plan = FitPlan::default();
    for track in desired {
        let Some(isrc) = track.isrc.as_deref() else {
            plan.skipped.push(SkippedTrack {
                title: track.title.clone(),
                isrc: None,
                reason: SkipReason::NoIsrc,
            });
            continue;
        };
        if !is_ready(isrc) {
            plan.skipped.push(SkippedTrack {
                title: track.title.clone(),
                isrc: track.isrc.clone(),
                reason: SkipReason::NoVault,
            });
            continue;
        }
        let seconds = ms_to_seconds(track.duration_ms);
        if plan.seconds + seconds > cap_seconds {
            plan.skipped.push(SkippedTrack {
                title: track.title.clone(),
                isrc: track.isrc.clone(),
                reason: SkipReason::CapReached,
            });
            break;
        }
        plan.seconds += seconds;
        plan.tracks.push(track.clone());
    }
    plan
}

/// Plan the ISRCs to *order* for a playlist-bound Tonie, honouring the
/// Tonie's current fill level (#624).
///
/// `remaining = max(cap_seconds - present_seconds, 0)` is the time still free
/// on the Tonie. The desired tracks are walked in Tonie order through
/// [`greedy_fit`]; the fit stops at the first overflow (`CapReached`), so no
/// track is ordered that [`super::run::run_sync`] would never place anyway.
///
/// Tracks that are already in the vault (`is_ready`) still consume remaining
/// time — they are on the Tonie and need no download — but are **not** ordered.
/// The result is the missing, fitting ISRCs in Tonie order.
pub fn order_plan_for_tonie(
    desired: &[DesiredTrack],
    cap_seconds: i64,
    present_seconds: i64,
    is_ready: impl Fn(&str) -> bool,
) -> Vec<String> {
    let remaining = (cap_seconds - present_seconds).max(0);
    let (deduped, _) = dedupe_by_isrc(desired.to_vec());
    let fit = greedy_fit(&deduped, remaining, |_| true);
    fit.tracks
        .iter()
        .filter_map(|t| t.isrc.clone().filter(|i| !is_ready(i)))
        .collect()
}

fn chapter_for(track: &DesiredTrack) -> Chapter {
    Chapter {
        title: Some(track.title.clone()),
        seconds: Some(ms_to_seconds(track.duration_ms) as f64),
        ..Default::default()
    }
}

/// How far a chapter length may deviate before it counts as a
/// different chapter. The API reports fractional seconds of the
/// *transcoded* file, which never matches the planned integer length
/// exactly.
const SECONDS_TOLERANCE: f64 = 2.0;

fn same_as(chapter: &Chapter, track: &DesiredTrack) -> bool {
    chapter.title.as_deref() == Some(track.title.as_str())
        && chapter.seconds.is_some_and(|s| {
            (s - ms_to_seconds(track.duration_ms) as f64).abs() <= SECONDS_TOLERANCE
        })
}

/// Compare the Tonie's existing chapters with a planned list.
///
/// Chapters are compared by `title` + `seconds` in order. A shared prefix is
/// reused verbatim so those files never get uploaded again.
pub fn plan_diff(existing: &[Chapter], planned: &[DesiredTrack]) -> DiffPlan {
    let common = existing
        .iter()
        .zip(planned.iter())
        .take_while(|(chapter, track)| same_as(chapter, track))
        .count();

    let kind = if common == existing.len() && common == planned.len() {
        DiffKind::Unchanged
    } else if common == existing.len() {
        DiffKind::Append
    } else {
        DiffKind::Replace
    };

    let reuse: Vec<Chapter> = existing.iter().take(common).cloned().collect();
    let upload: Vec<DesiredTrack> = planned.iter().skip(common).cloned().collect();

    let mut chapters = reuse.clone();
    chapters.extend(upload.iter().map(chapter_for));

    DiffPlan {
        kind,
        chapters,
        reuse,
        upload,
    }
}

/// Load the desired track list for a playlist (read-only).
///
/// `order_by` decides which tracks the greedy fit sees first:
/// `playlist` keeps Spotify's `position` order, `newest`/`newest_chrono`
/// order by `added_at` descending so the freshest tracks fill the Tonie.
/// NULL title/duration degrade to an empty title and zero duration.
pub async fn desired_from_playlist(
    pool: &SqlitePool,
    playlist_id: i64,
    order_by: &str,
) -> Result<Vec<DesiredTrack>> {
    let order_sql = match order_by {
        super::bindings::ORDER_NEWEST | super::bindings::ORDER_NEWEST_CHRONO => {
            "ORDER BY IFNULL(hpt.added_at, '') DESC, hpt.position"
        }
        _ => "ORDER BY hpt.position",
    };
    let sql = format!(
        "SELECT t.isrc AS isrc, t.title AS title, t.duration_ms AS duration_ms
           FROM hub_playlist_tracks hpt
           JOIN hub_tracks t ON t.id = hpt.track_id
          WHERE hpt.playlist_id = ?1
          {order_sql}"
    );
    let rows = sqlx::query(&sql)
    .bind(playlist_id)
    .fetch_all(pool)
    .await
    .context("Playlist-Tracks konnten nicht gelesen werden")?;

    Ok(rows
        .into_iter()
        .map(|row| DesiredTrack {
            isrc: row.get::<Option<String>, _>("isrc"),
            title: row
                .get::<Option<String>, _>("title")
                .unwrap_or_default(),
            duration_ms: row.get::<Option<i64>, _>("duration_ms").unwrap_or(0),
        })
        .collect())
}

/// Resolve the desired track list for a binding source (#504).
///
/// Dispatches on `source_kind`:
/// * `playlist` → [`desired_from_playlist`] (per-binding `order_by`),
/// * `tag` → [`desired_from_tag`] (hub track id order),
/// * `backpack` → [`desired_from_backpack`] (resolver order).
///
/// A missing `source_id` ("no source linked") and an unknown kind are hard
/// errors — an *existing but empty* source yields an empty `Vec` instead; the
/// caller turns that emptiness into a clear error state (no silent empty sync).
///
/// Ownership (#504): migration 060 drops the FK on `source_id`, and the
/// `hub_playlists`/`hub_backpacks`/`hub_tags` primary keys are plain INTEGER
/// (no AUTOINCREMENT), so a deleted-then-recreated source can reuse an id. A
/// binding from user A could therefore point at a source now owned by user B.
/// We verify ownership before reading: a source that is not (or no longer)
/// owned by `uid` resolves to an empty `Vec` — the caller turns that into an
/// error state and **never** reads another user's tracks.
pub async fn desired_for_source(
    pool: &SqlitePool,
    uid: i64,
    source_kind: &str,
    source_id: Option<i64>,
    order_by: &str,
) -> Result<Vec<DesiredTrack>> {
    let Some(source_id) = source_id else {
        anyhow::bail!("Keine Quelle verknüpft");
    };
    if !matches!(source_kind, SOURCE_PLAYLIST | SOURCE_TAG | SOURCE_BACKPACK) {
        anyhow::bail!("Unbekannte Quellenart: {source_kind}");
    }
    if !source_owned_by(pool, uid, source_kind, source_id).await? {
        return Ok(Vec::new());
    }
    match source_kind {
        SOURCE_PLAYLIST => desired_from_playlist(pool, source_id, order_by).await,
        SOURCE_TAG => desired_from_tag(pool, source_id).await,
        SOURCE_BACKPACK => desired_from_backpack(pool, source_id).await,
        // Unreachable: validated above.
        other => anyhow::bail!("Unbekannte Quellenart: {other}"),
    }
}

/// Whether `source_id` of `kind` is owned by `uid` (#504).
///
/// Each table names its user column differently: `hub_playlists.user_id`,
/// `hub_backpacks.user_id`, `hub_tags.owner_user_id`. A missing row is simply
/// not owned (returns `false`).
async fn source_owned_by(
    pool: &SqlitePool,
    uid: i64,
    kind: &str,
    id: i64,
) -> Result<bool> {
    let found: Option<i64> = match kind {
        SOURCE_PLAYLIST => {
            sqlx::query_scalar("SELECT 1 FROM hub_playlists WHERE id = ?1 AND user_id = ?2")
                .bind(id)
                .bind(uid)
                .fetch_optional(pool)
                .await?
        }
        SOURCE_BACKPACK => {
            sqlx::query_scalar("SELECT 1 FROM hub_backpacks WHERE id = ?1 AND user_id = ?2")
                .bind(id)
                .bind(uid)
                .fetch_optional(pool)
                .await?
        }
        SOURCE_TAG => {
            sqlx::query_scalar("SELECT 1 FROM hub_tags WHERE id = ?1 AND owner_user_id = ?2")
                .bind(id)
                .bind(uid)
                .fetch_optional(pool)
                .await?
        }
        _ => return Ok(false),
    };
    Ok(found.is_some())
}

/// Desired tracks of a tag: its resolved hub tracks (`hub_track_resolved_tags`),
/// id-ordered, mapped through [`tracks_to_desired`].
pub async fn desired_from_tag(pool: &SqlitePool, tag_id: i64) -> Result<Vec<DesiredTrack>> {
    let ids = crate::backpacks::tracks_for_tag(pool, tag_id).await?;
    tracks_to_desired(pool, &ids).await
}

/// Desired tracks of a backpack: every resolved track (unbounded — the
/// 1..=500 clamp of [`crate::backpacks::resolve`] must not truncate the source),
/// in resolver order, mapped through [`tracks_to_desired`].
pub async fn desired_from_backpack(
    pool: &SqlitePool,
    backpack_id: i64,
) -> Result<Vec<DesiredTrack>> {
    let resolved = crate::backpacks::resolve_unbounded(pool, backpack_id).await?;
    let ids: Vec<i64> = resolved.into_iter().map(|r| r.track_id).collect();
    tracks_to_desired(pool, &ids).await
}

/// Map hub track ids to [`DesiredTrack`]s, preserving the given order and
/// degrading NULL title/duration exactly like [`desired_from_playlist`] (empty
/// title, zero duration). Ids without a `hub_tracks` row are skipped, matching
/// the inner JOIN of [`desired_from_playlist`].
async fn tracks_to_desired(pool: &SqlitePool, ids: &[i64]) -> Result<Vec<DesiredTrack>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut by_id: std::collections::HashMap<i64, DesiredTrack> = std::collections::HashMap::new();
    // Chunk so the `IN (...)` placeholder list stays well below SQLite's
    // parameter limit even for a large backpack.
    for chunk in ids.chunks(500) {
        let placeholders = (1..=chunk.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, isrc, title, duration_ms FROM hub_tracks WHERE id IN ({placeholders})"
        );
        let mut q = sqlx::query(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        let rows = q
            .fetch_all(pool)
            .await
            .context("Quell-Tracks konnten nicht gelesen werden")?;
        for row in rows {
            by_id.insert(
                row.get::<i64, _>("id"),
                DesiredTrack {
                    isrc: row.get::<Option<String>, _>("isrc"),
                    title: row.get::<Option<String>, _>("title").unwrap_or_default(),
                    duration_ms: row.get::<Option<i64>, _>("duration_ms").unwrap_or(0),
                },
            );
        }
    }
    Ok(ids.iter().filter_map(|id| by_id.get(id).cloned()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn track(isrc: Option<&str>, title: &str, seconds: i64) -> DesiredTrack {
        DesiredTrack {
            isrc: isrc.map(str::to_string),
            title: title.to_string(),
            duration_ms: seconds * 1000,
        }
    }

    fn ready<'a>(isrcs: &'a [&'a str]) -> impl Fn(&str) -> bool + 'a {
        let set: HashSet<String> = isrcs.iter().map(|s| s.to_string()).collect();
        move |isrc: &str| set.contains(isrc)
    }

    fn chapter(title: &str, seconds: i64) -> Chapter {
        Chapter {
            title: Some(title.to_string()),
            seconds: Some(seconds as f64),
            ..Default::default()
        }
    }

    #[test]
    fn ms_rounds_half_up() {
        assert_eq!(ms_to_seconds(0), 0);
        assert_eq!(ms_to_seconds(499), 0);
        assert_eq!(ms_to_seconds(500), 1);
        assert_eq!(ms_to_seconds(1499), 1);
        assert_eq!(ms_to_seconds(1500), 2);
        assert_eq!(ms_to_seconds(-1500), -2);
    }

    #[test]
    fn exact_cap_fits_everything() {
        let desired = vec![
            track(Some("a"), "A", 1800),
            track(Some("b"), "B", 1800),
            track(Some("c"), "C", 1800),
        ];
        let plan = greedy_fit(&desired, 5400, ready(&["a", "b", "c"]));
        assert_eq!(plan.tracks.len(), 3);
        assert_eq!(plan.seconds, 5400);
        assert!(plan.skipped.is_empty());
    }

    #[test]
    fn too_long_first_track_stops_search() {
        let desired = vec![
            track(Some("big"), "Big", 6000),
            track(Some("small"), "Small", 60),
        ];
        let plan = greedy_fit(&desired, 5400, ready(&["big", "small"]));
        assert!(plan.tracks.is_empty());
        assert_eq!(plan.seconds, 0);
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].reason, SkipReason::CapReached);
        // No later track was considered.
        assert_eq!(plan.skipped[0].title, "Big");
    }

    #[test]
    fn empty_source_is_empty_plan() {
        let plan = greedy_fit(&[], 5400, |_: &str| true);
        assert!(plan.tracks.is_empty());
        assert!(plan.skipped.is_empty());
        assert_eq!(plan.seconds, 0);
    }

    #[test]
    fn duplicate_isrc_keeps_first() {
        let input = vec![
            track(Some("dup"), "First", 100),
            track(Some("dup"), "Second", 100),
            track(Some("other"), "Other", 100),
        ];
        let (kept, skipped) = dedupe_by_isrc(input);
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].title, "First");
        assert_eq!(skipped.len(), 1);
        assert_eq!(skipped[0].title, "Second");
        assert_eq!(skipped[0].reason, SkipReason::DuplicateIsrc);
    }

    #[test]
    fn dedupe_leaves_missing_isrc_alone() {
        let input = vec![track(None, "One", 10), track(None, "Two", 10)];
        let (kept, skipped) = dedupe_by_isrc(input);
        assert_eq!(kept.len(), 2);
        assert!(skipped.is_empty());
    }

    #[test]
    fn missing_vault_spends_no_budget() {
        let desired = vec![
            track(Some("missing"), "Missing", 3000),
            track(Some("ok"), "OK", 1800),
        ];
        let plan = greedy_fit(&desired, 1800, ready(&["ok"]));
        assert_eq!(plan.tracks.len(), 1);
        assert_eq!(plan.tracks[0].title, "OK");
        assert_eq!(plan.seconds, 1800);
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].reason, SkipReason::NoVault);
    }

    #[test]
    fn missing_isrc_is_skipped() {
        let desired = vec![track(None, "NoIsrc", 100), track(Some("a"), "A", 100)];
        let plan = greedy_fit(&desired, 5400, ready(&["a"]));
        assert_eq!(plan.tracks.len(), 1);
        assert_eq!(plan.skipped[0].reason, SkipReason::NoIsrc);
    }

    #[test]
    fn order_plan_keeps_only_missing_fitting_isrcs_in_order() {
        // 5400 cap, 5300 present → 100 s remaining: only the 60 s track fits.
        let desired = vec![
            track(Some("a"), "A", 60),
            track(Some("b"), "B", 200),
        ];
        let plan = order_plan_for_tonie(&desired, 5400, 5300, |_| false);
        assert_eq!(plan, vec!["a"]);
    }

    #[test]
    fn order_plan_ready_track_spends_time_but_is_not_ordered() {
        // A (60 s, ready) consumes 60 s, leaving 40 s; B (30 s, missing) fits.
        let desired = vec![
            track(Some("a"), "A", 60),
            track(Some("b"), "B", 30),
            track(Some("c"), "C", 30),
        ];
        let plan = order_plan_for_tonie(&desired, 5400, 5300, ready(&["a"]));
        // remaining 100; a(60 ready) + b(30) = 90; c(30) would be 120 > 100 → break.
        assert_eq!(plan, vec!["b"]);
    }

    #[test]
    fn order_plan_overflow_stops() {
        let desired = vec![
            track(Some("a"), "A", 60),
            track(Some("b"), "B", 200),
            track(Some("c"), "C", 10),
        ];
        let plan = order_plan_for_tonie(&desired, 5400, 5300, |_| false);
        // a fits (60), b overflows (260 > 100) → break; c is never considered.
        assert_eq!(plan, vec!["a"]);
    }

    #[test]
    fn order_plan_dedupe_first_wins() {
        let desired = vec![
            track(Some("dup"), "First", 10),
            track(Some("dup"), "Second", 10),
        ];
        let plan = order_plan_for_tonie(&desired, 5400, 0, |_| false);
        assert_eq!(plan, vec!["dup"]);
    }

    #[test]
    fn order_plan_exact_fit_is_ordered() {
        let desired = vec![track(Some("a"), "A", 100)];
        let plan = order_plan_for_tonie(&desired, 5400, 5300, |_| false);
        assert_eq!(plan, vec!["a"]);
    }

    #[test]
    fn order_plan_no_isrc_is_not_ordered() {
        let desired = vec![track(None, "NoIsrc", 10), track(Some("a"), "A", 10)];
        let plan = order_plan_for_tonie(&desired, 5400, 0, |_| false);
        assert_eq!(plan, vec!["a"]);
    }

    #[test]
    fn order_plan_empty_source_is_empty() {
        let plan = order_plan_for_tonie(&[], 5400, 0, |_| false);
        assert!(plan.is_empty());
    }

    #[test]
    fn order_plan_present_at_cap_is_empty() {
        let desired = vec![track(Some("a"), "A", 10)];
        let plan = order_plan_for_tonie(&desired, 5400, 5400, |_| false);
        assert!(plan.is_empty());
    }

    #[test]
    fn diff_unchanged() {
        let existing = vec![chapter("A", 100), chapter("B", 200)];
        let planned = vec![track(Some("a"), "A", 100), track(Some("b"), "B", 200)];
        let plan = plan_diff(&existing, &planned);
        assert_eq!(plan.kind, DiffKind::Unchanged);
        assert!(plan.upload.is_empty());
        assert_eq!(plan.reuse, existing);
        assert_eq!(plan.chapters, existing);
    }

    #[test]
    fn diff_append_uploads_only_tail() {
        let existing = vec![chapter("A", 100)];
        let planned = vec![track(Some("a"), "A", 100), track(Some("b"), "B", 200)];
        let plan = plan_diff(&existing, &planned);
        assert_eq!(plan.kind, DiffKind::Append);
        assert_eq!(plan.reuse, existing);
        assert_eq!(plan.upload.len(), 1);
        assert_eq!(plan.upload[0].title, "B");
        assert_eq!(plan.chapters.len(), 2);
        assert_eq!(plan.chapters[1].title.as_deref(), Some("B"));
    }

    #[test]
    fn diff_replace_reuses_prefix_and_uploads_difference() {
        let existing = vec![chapter("A", 100), chapter("Old", 500)];
        let planned = vec![
            track(Some("a"), "A", 100),
            track(Some("b"), "B", 200),
            track(Some("c"), "C", 300),
        ];
        let plan = plan_diff(&existing, &planned);
        assert_eq!(plan.kind, DiffKind::Replace);
        assert_eq!(plan.reuse.len(), 1);
        assert_eq!(plan.reuse[0].title.as_deref(), Some("A"));
        assert_eq!(plan.upload.len(), 2);
        assert_eq!(plan.upload[0].title, "B");
        assert_eq!(plan.upload[1].title, "C");
        assert_eq!(plan.chapters.len(), 3);
        assert_eq!(plan.chapters[0], chapter("A", 100));
    }

    #[test]
    fn diff_replace_on_divergent_first_chapter() {
        let existing = vec![chapter("X", 999)];
        let planned = vec![track(Some("a"), "A", 100)];
        let plan = plan_diff(&existing, &planned);
        assert_eq!(plan.kind, DiffKind::Replace);
        assert!(plan.reuse.is_empty());
        assert_eq!(plan.upload.len(), 1);
        assert_eq!(plan.chapters.len(), 1);
        assert_eq!(plan.chapters[0].title.as_deref(), Some("A"));
    }
}
