//! Album-based Tonie sources (`plans/proposed/tonie-album-queue.md`, M1–M3).
//!
//! ISRC is **edition-bound**: Spotify and Deezer cut the same audio drama
//! differently, so the chapters carry different ISRCs and a playlist binding can
//! never find them (measured 2026-11-18: 34 of the first 40 playlist positions
//! came back as `deezer error 800`, only the title song matched).
//!
//! This module resolves the tonie from Deezer *albums* instead and fits **whole
//! albums only** — never a partial one.

use anyhow::{Context, Result};
use sqlx::{Row, SqlitePool};

use super::bindings::{self, Binding};
use super::sync::{DesiredTrack, SkipReason, SkippedTrack};
use crate::config::Config;
use crate::music_api;

/// How long a cached album detail (tracks + duration) stays valid.
pub const CACHE_TTL_SECS: i64 = 7 * 24 * 3600;
/// Bound on artist pages fetched in one resolve (PAW Patrol has 417 albums).
const MAX_ALBUM_PAGES: u32 = 12;
const PAGE_SIZE: u32 = 100;
/// Bound on album-detail fetches per resolve (each is one music-api call).
const MAX_ALBUM_LOOKUPS: usize = 12;

/// One album that could go on the tonie.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlbumPlan {
    pub album_id: String,
    pub title: String,
    /// Album length in seconds (sum of its tracks).
    pub seconds: i64,
    pub cover_url: Option<String>,
    pub release_date: Option<String>,
    pub tracks: Vec<DesiredTrack>,
}

/// Albums selected for one run, in tonie order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlbumFit {
    pub albums: Vec<AlbumPlan>,
    pub seconds: i64,
    pub skipped: Vec<SkippedTrack>,
}

/// Fill the tonie with **whole albums**, preserving the given order.
///
/// * an album with a track that is not in the vault → `AlbumNotReady`, continue
///   (the next album still gets a chance, otherwise a single missing download
///   would leave the tonie empty)
/// * an album that no longer fits → `CapReached`, **stop**: the running order
///   stays contiguous instead of jumping back and forth
pub fn fit_albums(
    albums: Vec<AlbumPlan>,
    cap_seconds: i64,
    is_ready: impl Fn(&str) -> bool,
) -> AlbumFit {
    let mut fit = AlbumFit::default();
    for album in albums {
        let mut ready = !album.tracks.is_empty();
        for track in &album.tracks {
            match track.isrc.as_deref() {
                Some(isrc) if is_ready(isrc) => {}
                _ => {
                    ready = false;
                    break;
                }
            }
        }
        if !ready {
            fit.skipped.push(SkippedTrack {
                title: album.title,
                isrc: None,
                reason: SkipReason::AlbumNotReady,
            });
            continue;
        }
        if album.seconds <= 0 {
            continue;
        }
        if fit.seconds + album.seconds > cap_seconds {
            fit.skipped.push(SkippedTrack {
                title: album.title,
                isrc: None,
                reason: SkipReason::CapReached,
            });
            break;
        }
        fit.seconds += album.seconds;
        fit.albums.push(album);
    }
    fit
}

/// The albums this binding should place on the tonie, already ordered.
pub async fn resolve(
    pool: &SqlitePool,
    cfg: &Config,
    uid: i64,
    binding: &Binding,
) -> Result<Vec<AlbumPlan>> {
    if binding.album_mode == bindings::ALBUM_MODE_BASKET {
        basket(pool, cfg, binding).await
    } else {
        artist(pool, cfg, uid, binding).await
    }
}

/// Explicit album list: order and content come from the user, `seen` is ignored
/// (an explicit choice always wins over the listened ledger).
async fn basket(pool: &SqlitePool, cfg: &Config, binding: &Binding) -> Result<Vec<AlbumPlan>> {
    let items = bindings::list_album_items(pool, binding.id).await?;
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        out.push(album_plan(pool, cfg, &item.album_id).await?);
    }
    Ok(out)
}

/// Artist subscription: newest (or oldest) albums first, listened ones skipped.
async fn artist(
    pool: &SqlitePool,
    cfg: &Config,
    uid: i64,
    binding: &Binding,
) -> Result<Vec<AlbumPlan>> {
    let artist_id = binding
        .source_id
        .map(|id| id.to_string())
        .context("Kein Deezer-Artist verknüpft")?;

    let mut summaries = Vec::new();
    let mut index = 0u32;
    for _ in 0..MAX_ALBUM_PAGES {
        let page = music_api::deezer_artist_albums(cfg, &artist_id, PAGE_SIZE, index).await?;
        let got = page.albums.len() as u32;
        summaries.extend(page.albums);
        if got < PAGE_SIZE {
            break;
        }
        index += PAGE_SIZE;
    }

    // Only real albums: singles/compilations are noise on a tonie.
    summaries.retain(|a| {
        a.record_type
            .as_deref()
            .map(|r| r == "album")
            .unwrap_or(true)
    });
    // Newest first; a missing release date sorts last.
    summaries.sort_by(|a, b| {
        b.release_date
            .clone()
            .unwrap_or_default()
            .cmp(&a.release_date.clone().unwrap_or_default())
            .then_with(|| b.album_id.cmp(&a.album_id))
    });
    if binding.album_mode == bindings::ALBUM_MODE_OLDEST {
        summaries.reverse();
    }

    let seen = seen_ids(pool, uid).await?;
    let mut out = Vec::new();
    let mut seconds = 0i64;
    let mut looked = 0usize;
    for summary in summaries {
        if seconds >= binding.cap_seconds || looked >= MAX_ALBUM_LOOKUPS {
            break;
        }
        if seen.contains(&summary.album_id) {
            continue;
        }
        looked += 1;
        let plan = album_plan(pool, cfg, &summary.album_id).await?;
        seconds += plan.seconds;
        out.push(plan);
    }
    Ok(out)
}

/// Album detail from the cache, falling back to one music-api call.
async fn album_plan(pool: &SqlitePool, cfg: &Config, album_id: &str) -> Result<AlbumPlan> {
    let cutoff = (chrono::Utc::now() - chrono::Duration::seconds(CACHE_TTL_SECS)).to_rfc3339();
    let row = sqlx::query(
        "SELECT title, cover_url, release_date, seconds, tracks_json
           FROM hub_deezer_albums
          WHERE album_id = ?1 AND fetched_at >= ?2",
    )
    .bind(album_id)
    .bind(&cutoff)
    .fetch_optional(pool)
    .await
    .context("Album-Cache konnte nicht gelesen werden")?;

    if let Some(r) = row
        && let Ok(tracks) =
            serde_json::from_str::<Vec<CachedTrack>>(&r.get::<String, _>("tracks_json"))
    {
        return Ok(AlbumPlan {
            album_id: album_id.to_string(),
            title: r.get::<Option<String>, _>("title").unwrap_or_default(),
            seconds: r.get::<Option<i64>, _>("seconds").unwrap_or(0),
            cover_url: r.get("cover_url"),
            release_date: r.get("release_date"),
            tracks: tracks.into_iter().map(|t| t.into_desired()).collect(),
        });
    }

    let detail = music_api::deezer_album(cfg, album_id).await?;
    let tracks: Vec<CachedTrack> = detail
        .tracks
        .iter()
        .map(|t| CachedTrack {
            isrc: t.isrc.clone(),
            title: t.title.clone(),
            duration_ms: t.duration_ms,
        })
        .collect();
    let seconds = detail
        .seconds
        .unwrap_or_else(|| tracks.iter().map(|t| t.duration_ms / 1000).sum());
    let tracks_json = serde_json::to_string(&tracks)?;

    sqlx::query(
        "INSERT INTO hub_deezer_albums
             (album_id, artist_id, title, release_date, record_type, cover_url, seconds,
              tracks_json, fetched_at)
         VALUES (?1, NULL, ?2, ?3, NULL, ?4, ?5, ?6, ?7)
         ON CONFLICT(album_id) DO UPDATE SET
             title = excluded.title,
             release_date = excluded.release_date,
             cover_url = excluded.cover_url,
             seconds = excluded.seconds,
             tracks_json = excluded.tracks_json,
             fetched_at = excluded.fetched_at",
    )
    .bind(album_id)
    .bind(&detail.title)
    .bind(&detail.release_date)
    .bind(&detail.cover_url)
    .bind(seconds)
    .bind(&tracks_json)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(pool)
    .await
    .context("Album-Cache konnte nicht geschrieben werden")?;

    Ok(AlbumPlan {
        album_id: album_id.to_string(),
        title: detail.title,
        seconds,
        cover_url: detail.cover_url,
        release_date: detail.release_date,
        tracks: tracks.into_iter().map(|t| t.into_desired()).collect(),
    })
}

/// One track of a cached album.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CachedTrack {
    isrc: Option<String>,
    title: String,
    duration_ms: i64,
}

impl CachedTrack {
    fn into_desired(self) -> DesiredTrack {
        DesiredTrack {
            isrc: self.isrc,
            title: self.title,
            duration_ms: self.duration_ms,
        }
    }
}

/// Album ids this user has already had on a tonie.
pub async fn seen_ids(pool: &SqlitePool, uid: i64) -> Result<std::collections::HashSet<String>> {
    let rows: Vec<String> =
        sqlx::query_scalar("SELECT album_id FROM hub_tonie_albums_seen WHERE user_id = ?1")
            .bind(uid)
            .fetch_all(pool)
            .await
            .context("Tonie-Album-Ledger konnte nicht gelesen werden")?;
    Ok(rows.into_iter().collect())
}

/// Mark the albums of a finished run as listened.
pub async fn mark_seen(pool: &SqlitePool, uid: i64, albums: &[AlbumPlan]) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    for album in albums {
        sqlx::query(
            "INSERT INTO hub_tonie_albums_seen
                 (user_id, album_id, album_title, first_used_at, last_used_at)
             VALUES (?1, ?2, ?3, ?4, ?4)
             ON CONFLICT(user_id, album_id) DO UPDATE SET
                 album_title = excluded.album_title,
                 last_used_at = excluded.last_used_at",
        )
        .bind(uid)
        .bind(&album.album_id)
        .bind(&album.title)
        .bind(&now)
        .execute(pool)
        .await
        .context("Tonie-Album-Ledger konnte nicht geschrieben werden")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(id: &str, seconds: i64, isrcs: &[&str]) -> AlbumPlan {
        AlbumPlan {
            album_id: id.to_string(),
            title: id.to_string(),
            seconds,
            tracks: isrcs
                .iter()
                .map(|i| DesiredTrack {
                    isrc: Some(i.to_string()),
                    title: format!("{id}-{i}"),
                    duration_ms: 1000,
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn whole_albums_only() {
        // 20 min album, 10 min left -> skipped, never cut.
        let fit = fit_albums(vec![album("a", 1200, &["x"])], 600, |_| true);
        assert!(fit.albums.is_empty());
        assert_eq!(fit.skipped.len(), 1);
        assert_eq!(fit.skipped[0].reason, SkipReason::CapReached);
        assert_eq!(fit.seconds, 0);
    }

    #[test]
    fn fills_whole_albums_in_order() {
        let fit = fit_albums(
            vec![
                album("a", 1200, &["a1", "a2"]),
                album("b", 1200, &["b1"]),
                album("c", 1200, &["c1"]),
            ],
            3000,
            |_| true,
        );
        let ids: Vec<&str> = fit.albums.iter().map(|a| a.album_id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b"]);
        assert_eq!(fit.seconds, 2400);
        // The third album no longer fits and stops the fit.
        assert_eq!(fit.skipped.len(), 1);
        assert_eq!(fit.skipped[0].title, "c");
    }

    #[test]
    fn not_ready_album_is_skipped_but_others_get_a_chance() {
        let ready = |isrc: &str| isrc != "a1";
        let fit = fit_albums(
            vec![album("a", 600, &["a1"]), album("b", 600, &["b1"])],
            5400,
            ready,
        );
        let ids: Vec<&str> = fit.albums.iter().map(|a| a.album_id.as_str()).collect();
        assert_eq!(ids, vec!["b"]);
        assert_eq!(fit.skipped.len(), 1);
        assert_eq!(fit.skipped[0].reason, SkipReason::AlbumNotReady);
    }

    #[test]
    fn album_without_tracks_is_not_ready() {
        let fit = fit_albums(vec![album("a", 600, &[])], 5400, |_| true);
        assert!(fit.albums.is_empty());
        assert_eq!(fit.skipped[0].reason, SkipReason::AlbumNotReady);
    }
}
