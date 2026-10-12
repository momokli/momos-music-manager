//! Traktor `collection.nml` ingest (issues #211–#213).
//!
//! Parses a user's Traktor collection: per-track play count / last played /
//! rating plus playlist / collection / session-history membership. NML carries
//! no ISRC, so tracks are matched to hub tracks by (title, artist), falling back
//! to a unique title match.
//!
//! Idempotent: re-importing replaces a user's Traktor rows.

use std::collections::HashMap;

use anyhow::{Context, Result};
use sqlx::SqlitePool;

#[derive(Debug, Default)]
pub struct Stats {
    pub entries: usize,
    pub matched: usize,
    pub playlists: usize,
    pub with_bpm: usize,
    pub with_key: usize,
}

struct Entry {
    title: String,
    artist: String,
    play_count: i64,
    last_played: Option<String>,
    rating: Option<i64>,
    bpm: Option<f64>,
    /// Camelot notation (e.g. "8A"), derived from the Traktor KEY attribute.
    camelot: Option<String>,
}

/// Map a Traktor `KEY` attribute to Camelot notation. Traktor uses the Camelot
/// wheel numbering with `m`=minor(A)/`d`=dur=major(B): "8m" -> "8A", "8d" -> "8B".
fn traktor_key_to_camelot(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.len() < 2 {
        return None;
    }
    let (num, mode) = s.split_at(s.len() - 1);
    let letter = match mode.to_ascii_lowercase().as_str() {
        "m" => 'A',
        "d" => 'B',
        _ => return None,
    };
    let n: i64 = num.parse().ok()?;
    (1..=12).contains(&n).then(|| format!("{n}{letter}"))
}

/// Import a `collection.nml` file from disk for `user_id`.
pub async fn import_nml(pool: &SqlitePool, user_id: i64, path: &str) -> Result<Stats> {
    let xml = std::fs::read_to_string(path).with_context(|| format!("read {path}"))?;
    import_str(pool, user_id, &xml).await
}

/// Import from an in-memory NML string (used by tests).
pub async fn import_str(pool: &SqlitePool, user_id: i64, xml: &str) -> Result<Stats> {
    let doc = roxmltree::Document::parse(xml).context("parse NML")?;
    let now = chrono::Utc::now().to_rfc3339();
    let run_id = crate::history::start_run(pool, user_id, "traktor")
        .await
        .ok();

    // ── collection entries ──────────────────────────────────────────────────
    let mut entries: Vec<Entry> = Vec::new();
    let mut by_file: HashMap<String, usize> = HashMap::new();
    if let Some(coll) = doc.descendants().find(|n| n.has_tag_name("COLLECTION")) {
        for e in coll.children().filter(|n| n.has_tag_name("ENTRY")) {
            let title = text_of(&e, "TITLE");
            if title.is_empty() {
                continue;
            }
            let artist = text_of(&e, "ARTIST");
            let info = e.children().find(|n| n.has_tag_name("INFO"));
            let (play_count, last_played, rating) = match info {
                Some(i) => (
                    i.attribute("PLAYCOUNT")
                        .and_then(|v| v.parse::<i64>().ok())
                        .unwrap_or(0),
                    i.attribute("LAST_PLAY").map(str::to_string),
                    parse_rating(i.attribute("RATING")),
                ),
                None => (0, None, None),
            };
            let file = e
                .children()
                .find(|n| n.has_tag_name("LOCATION"))
                .and_then(|l| l.attribute("FILE"))
                .unwrap_or("")
                .to_string();
            // BPM from <TEMPO BPM="...">, key from <INFO KEY="8m">.
            let bpm = e
                .children()
                .find(|n| n.has_tag_name("TEMPO"))
                .and_then(|t| t.attribute("BPM"))
                .and_then(|v| v.parse::<f64>().ok())
                .filter(|b| *b > 0.0);
            let camelot = info
                .and_then(|i| i.attribute("KEY"))
                .and_then(traktor_key_to_camelot);
            if !file.is_empty() {
                by_file.insert(file.clone(), entries.len());
            }
            entries.push(Entry {
                title,
                artist,
                play_count,
                last_played,
                rating,
                bpm,
                camelot,
            });
        }
    }
    let stats_entries = entries.len();
    let with_bpm = entries.iter().filter(|e| e.bpm.is_some()).count();
    let with_key = entries.iter().filter(|e| e.camelot.is_some()).count();

    // ── resolve + upsert per-track meta ─────────────────────────────────────
    let mut track_of: Vec<Option<i64>> = Vec::with_capacity(entries.len());
    let mut matched = 0usize;
    for en in &entries {
        let id = resolve_track(pool, &en.title, &en.artist).await;
        if id.is_some() {
            matched += 1;
        }
        track_of.push(id);
    }
    // Clear the previous import for this user, then re-insert (idempotent).
    sqlx::query("DELETE FROM hub_traktor_playlist_tracks WHERE user_id = ?1")
        .bind(user_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM hub_traktor_playlists WHERE user_id = ?1")
        .bind(user_id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM hub_traktor_tracks WHERE user_id = ?1")
        .bind(user_id)
        .execute(pool)
        .await?;

    for (en, id) in entries.iter().zip(&track_of) {
        let Some(track_id) = id else { continue };
        sqlx::query(
            "INSERT INTO hub_traktor_tracks
                 (user_id, track_id, play_count, last_played, rating, bpm, camelot, imported_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(user_id)
        .bind(track_id)
        .bind(en.play_count)
        .bind(&en.last_played)
        .bind(en.rating)
        .bind(en.bpm)
        .bind(&en.camelot)
        .bind(&now)
        .execute(pool)
        .await?;
    }

    // ── the COLLECTION pseudo-playlist (all matched collection tracks) ───────
    let mut next_id: i64 = 1;
    upsert_playlist(pool, user_id, next_id, "Collection", "collection", &now).await?;
    for id in track_of.iter().flatten() {
        insert_membership(pool, user_id, next_id, *id).await?;
    }
    next_id += 1;

    // ── Traktor playlist / session nodes ────────────────────────────────────
    let mut playlist_count = 1usize; // count the collection node
    for node in doc.descendants().filter(|n| n.has_tag_name("NODE")) {
        if node.attribute("TYPE") != Some("PLAYLIST") {
            continue;
        }
        let name = node.attribute("NAME").unwrap_or("").trim().to_string();
        if name.is_empty() {
            continue;
        }
        let node_type = if is_session_name(&name) {
            "session"
        } else {
            "playlist"
        };
        upsert_playlist(pool, user_id, next_id, &name, node_type, &now).await?;
        if let Some(pl) = node.children().find(|n| n.has_tag_name("PLAYLIST")) {
            for pk in pl.descendants().filter(|n| n.has_tag_name("PRIMARYKEY")) {
                let key = pk.attribute("KEY").unwrap_or("");
                let file = key.rsplit('/').next().unwrap_or(key);
                if let Some(&idx) = by_file.get(file) {
                    if let Some(track_id) = track_of[idx] {
                        insert_membership(pool, user_id, next_id, track_id).await?;
                    }
                }
            }
        }
        next_id += 1;
        playlist_count += 1;
    }

    let stats = Stats {
        entries: stats_entries,
        matched,
        playlists: playlist_count,
        with_bpm,
        with_key,
    };
    if let Some(rid) = run_id {
        let js = serde_json::json!({
            "entries": stats.entries,
            "matched": stats.matched,
            "playlists": stats.playlists,
            "with_bpm": stats.with_bpm,
            "with_key": stats.with_key,
        });
        let _ = crate::history::finish_run(pool, rid, "ok", &js).await;
    }
    Ok(stats)
}

fn text_of(node: &roxmltree::Node, tag: &str) -> String {
    node.children()
        .find(|n| n.has_tag_name(tag))
        .and_then(|n| n.text())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Traktor stores rating as 0..255 in 51-steps; map to 1..5 (0 => none).
fn parse_rating(raw: Option<&str>) -> Option<i64> {
    let v: f64 = raw?.parse().ok()?;
    if v <= 0.0 {
        return None;
    }
    Some(((v / 51.0).round() as i64).clamp(1, 5))
}

fn is_session_name(name: &str) -> bool {
    let n = name.to_lowercase();
    n.contains("history") || n.contains("session")
}

async fn resolve_track(pool: &SqlitePool, title: &str, artist: &str) -> Option<i64> {
    if !artist.is_empty() {
        let by_both = sqlx::query_scalar::<_, i64>(
            "SELECT id FROM hub_tracks
              WHERE lower(title) = lower(?1) AND lower(artists) = lower(?2) LIMIT 1",
        )
        .bind(title)
        .bind(artist)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        if by_both.is_some() {
            return by_both;
        }
    }
    sqlx::query_scalar::<_, i64>("SELECT id FROM hub_tracks WHERE lower(title) = lower(?1) LIMIT 1")
        .bind(title)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

async fn upsert_playlist(
    pool: &SqlitePool,
    user_id: i64,
    id: i64,
    name: &str,
    node_type: &str,
    now: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO hub_traktor_playlists (id, user_id, name, node_type, imported_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(user_id, id) DO UPDATE SET name = excluded.name,
             node_type = excluded.node_type, imported_at = excluded.imported_at",
    )
    .bind(id)
    .bind(user_id)
    .bind(name)
    .bind(node_type)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

async fn insert_membership(
    pool: &SqlitePool,
    user_id: i64,
    playlist_id: i64,
    track_id: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT OR IGNORE INTO hub_traktor_playlist_tracks (user_id, playlist_id, track_id)
         VALUES (?1, ?2, ?3)",
    )
    .bind(user_id)
    .bind(playlist_id)
    .bind(track_id)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::traktor_key_to_camelot;

    #[test]
    fn traktor_key_maps_to_camelot() {
        assert_eq!(traktor_key_to_camelot("8m").as_deref(), Some("8A"));
        assert_eq!(traktor_key_to_camelot("8d").as_deref(), Some("8B"));
        assert_eq!(traktor_key_to_camelot("12M").as_deref(), Some("12A"));
        assert_eq!(traktor_key_to_camelot("1d").as_deref(), Some("1B"));
        assert_eq!(traktor_key_to_camelot(""), None);
        assert_eq!(traktor_key_to_camelot("9"), None);
        assert_eq!(traktor_key_to_camelot("0m"), None);
        assert_eq!(traktor_key_to_camelot("13m"), None);
        assert_eq!(traktor_key_to_camelot("xm"), None);
    }
}
