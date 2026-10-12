//! Track genres via Last.fm community tags (`track.getTopTags`). Disabled
//! unless `LASTFM_API_KEY` is set. A `''` genre row marks "checked, no tags".

use anyhow::{Context, Result};
use serde::Deserialize;
use sqlx::{Row, SqlitePool};

use crate::config::Config;

#[derive(Deserialize)]
struct Resp {
    #[serde(default)]
    toptags: Option<TopTags>,
}

#[derive(Deserialize)]
struct TopTags {
    #[serde(default)]
    tag: Vec<Tag>,
}

#[derive(Deserialize)]
struct Tag {
    #[serde(default)]
    name: String,
}

/// Tags that describe the library, not the genre — filtered out.
fn is_noise(tag: &str) -> bool {
    matches!(
        tag.to_lowercase().as_str(),
        "seen live"
            | "favorites"
            | "favourites"
            | "favorite"
            | "favourite"
            | "favorites songs"
            | "love"
            | "awesome"
            | "cool"
    )
}

pub async fn top_tags(cfg: &Config, artist: &str, track: &str) -> Result<Vec<String>> {
    let Some(key) = cfg.lastfm_api_key.as_deref() else {
        return Ok(Vec::new());
    };
    if artist.is_empty() || track.is_empty() {
        return Ok(Vec::new());
    }
    let url = format!(
        "https://ws.audioscrobbler.com/2.0/?method=track.gettoptags&artist={}&track={}&api_key={}&format=json&limit=8",
        urlencoding::encode(artist),
        urlencoding::encode(track),
        urlencoding::encode(key),
    );
    let resp = reqwest::Client::new()
        .get(&url)
        .send()
        .await
        .context("GET last.fm toptags")?;
    let body: Resp = resp.json().await.unwrap_or(Resp { toptags: None });
    Ok(body
        .toptags
        .map(|t| t.tag)
        .unwrap_or_default()
        .into_iter()
        .filter(|t| !t.name.is_empty() && !is_noise(&t.name))
        .take(5)
        .map(|t| t.name)
        .collect())
}

/// Replace a track's genres. Empty `genres` writes the `''` "checked, none" marker.
pub async fn store(pool: &SqlitePool, track_id: i64, genres: &[String]) -> Result<()> {
    sqlx::query("DELETE FROM hub_track_genres WHERE track_id = ?1 AND source = 'lastfm'")
        .bind(track_id)
        .execute(pool)
        .await?;
    if genres.is_empty() {
        sqlx::query(
            "INSERT INTO hub_track_genres (track_id, genre, source) VALUES (?1, '', 'lastfm')
             ON CONFLICT(track_id, source, genre) DO NOTHING",
        )
        .bind(track_id)
        .execute(pool)
        .await?;
    } else {
        for g in genres {
            sqlx::query(
                "INSERT INTO hub_track_genres (track_id, genre, source) VALUES (?1, ?2, 'lastfm')
                 ON CONFLICT(track_id, source, genre) DO NOTHING",
            )
            .bind(track_id)
            .bind(g)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

/// One batch of tracks (with artist+title) not yet tagged. Returns processed count.
pub async fn sync_once(pool: &SqlitePool, cfg: &Config, batch: i64) -> Result<usize> {
    if cfg.lastfm_api_key.is_none() {
        return Ok(0);
    }
    let rows = sqlx::query(
        "SELECT t.id AS tid, t.artists AS artists, t.title AS title
           FROM hub_tracks t
          WHERE t.artists IS NOT NULL AND t.artists <> '' AND t.title IS NOT NULL AND t.title <> ''
            AND NOT EXISTS (
                SELECT 1 FROM hub_track_genres g
                 WHERE g.track_id = t.id AND g.source = 'lastfm')
          LIMIT ?1",
    )
    .bind(batch)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    if rows.is_empty() {
        return Ok(0);
    }

    let n = rows.len();
    for r in &rows {
        let tid: i64 = r.get("tid");
        let artists: String = r.get("artists");
        let title: String = r.get("title");
        // Last.fm: keep it gentle.
        let tags = top_tags(cfg, &artists, &title).await.unwrap_or_default();
        store(pool, tid, &tags).await?;
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    Ok(n)
}

pub async fn backfill(pool: &SqlitePool, cfg: &Config, max: usize) -> Result<(usize, bool)> {
    let mut done = 0usize;
    while done < max {
        let n = sync_once(pool, cfg, 40).await?;
        if n == 0 {
            return Ok((done, true));
        }
        done += n;
    }
    Ok((done, false))
}

#[cfg(test)]
mod tests {
    use super::is_noise;

    #[test]
    fn noise_filter() {
        assert!(is_noise("seen live"));
        assert!(is_noise("Favourites"));
        assert!(!is_noise("techno"));
        assert!(!is_noise("deep house"));
    }
}
