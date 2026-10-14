//! Data Vault 2.0 model over every musical entity music-api touches.
//!
//! Hubs (business keys), links (relationships) and satellites (descriptive,
//! insert-only, per-source) — hash-keyed and auditable. Multiple sources' truths
//! coexist; consumers pick the value they trust (see the precedence resolver).
//!
//! Design: [`plans/proposed/music-api-data-vault.md`]. The operational queue
//! (`tracks.state`, `orders`) is **not** the vault — the worker writes *loads*
//! into it. Satellites are append-only: a new row is written only when the
//! `hash_diff` over the descriptive columns changes.
//!
//! ```text
//! *_hk = UPPER(HEX(SHA256(normalized_business_key)))
//! *_lk = SHA256(concatenated participating hub keys, stable order)
//! hash_diff = SHA256(descriptive columns)
//! ```

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::{Pool, QueryBuilder, Sqlite, SqliteConnection};

use crate::db;
use crate::models::{MetadataItem, Track, TrackMetadata};

// ── Hashing / normalisation ──────────────────────────────────────────────────

/// `UPPER(HEX(SHA256(bk)))` — the hash key of a normalised business key.
pub fn hash_key(bk: &str) -> String {
    let mut h = Sha256::new();
    h.update(bk.as_bytes());
    hex_upper(h.finalize())
}

/// Link key: hash of the concatenated participating hub keys (stable order).
pub fn link_key(parts: &[&str]) -> String {
    let mut h = Sha256::new();
    for (i, p) in parts.iter().enumerate() {
        if i > 0 {
            h.update([0x1f]); // unit separator — keeps `ab|c` ≠ `a|bc`
        }
        h.update(p.as_bytes());
    }
    hex_upper(h.finalize())
}

/// `hash_diff`: SHA-256 over the satellite's descriptive columns. `None` is a
/// distinct sentinel from the empty string, so "no value" ≠ "empty value".
pub fn hash_diff(cols: &[Option<String>]) -> String {
    let mut h = Sha256::new();
    for c in cols {
        match c {
            Some(v) => h.update(v.as_bytes()),
            None => h.update([0u8]),
        }
        h.update([0x1f]);
    }
    hex_upper(h.finalize())
}

fn hex_upper(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02X}")).collect()
}

/// Normalise an artist name business key: collapse whitespace, uppercase.
pub fn normalize_name(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_uppercase()
}

/// Album business key: normalised title (+ first release date when known).
pub fn normalize_album_key(title: &str, release_date: Option<&str>) -> String {
    let t = normalize_name(title);
    match release_date.map(str::trim).filter(|d| !d.is_empty()) {
        Some(d) => format!("{t}|{d}"),
        None => t,
    }
}

/// UTC ISO-8601 `load_date` with microsecond precision (unique enough to be a
/// satellite PK component without a sequence).
pub fn load_date() -> String {
    let d = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs() as i64;
    let micros = d.subsec_micros();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, dd) = civil_from_days(days);
    let (hh, mm, ss) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    format!("{y:04}-{m:02}-{dd:02}T{hh:02}:{mm:02}:{ss:02}.{micros:06}Z")
}

/// Days since the Unix epoch → (year, month, day). Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn opt_str(v: Option<&str>) -> Option<String> {
    v.map(str::to_string)
}
fn opt_f64(v: Option<f64>) -> Option<String> {
    v.map(|x| format!("{x}"))
}

// ── Schema (additive) ────────────────────────────────────────────────────────

/// Create the vault tables. Additive and idempotent — safe on every boot and on
/// a fresh DB. Called from [`crate::db::init`].
pub async fn init(pool: &Pool<Sqlite>) -> Result<()> {
    let ddl = [
        // Hubs (business keys).
        "CREATE TABLE IF NOT EXISTS hub_track (
            track_hk TEXT PRIMARY KEY, isrc TEXT NOT NULL,
            load_date TEXT NOT NULL, record_source TEXT NOT NULL)",
        "CREATE UNIQUE INDEX IF NOT EXISTS ux_hub_track_bk ON hub_track(isrc)",
        "CREATE TABLE IF NOT EXISTS hub_artist (
            artist_hk TEXT PRIMARY KEY, artist_name TEXT NOT NULL,
            load_date TEXT NOT NULL, record_source TEXT NOT NULL)",
        "CREATE UNIQUE INDEX IF NOT EXISTS ux_hub_artist_bk ON hub_artist(artist_name)",
        "CREATE TABLE IF NOT EXISTS hub_album (
            album_hk TEXT PRIMARY KEY, album_key TEXT NOT NULL,
            load_date TEXT NOT NULL, record_source TEXT NOT NULL)",
        "CREATE UNIQUE INDEX IF NOT EXISTS ux_hub_album_bk ON hub_album(album_key)",
        // Links (relationships).
        "CREATE TABLE IF NOT EXISTS link_track_artist (
            track_artist_lk TEXT PRIMARY KEY, track_hk TEXT NOT NULL, artist_hk TEXT NOT NULL,
            position INTEGER NOT NULL DEFAULT 0, load_date TEXT NOT NULL, record_source TEXT NOT NULL)",
        "CREATE INDEX IF NOT EXISTS ix_lta_track ON link_track_artist(track_hk)",
        "CREATE INDEX IF NOT EXISTS ix_lta_artist ON link_track_artist(artist_hk)",
        "CREATE TABLE IF NOT EXISTS link_track_album (
            track_album_lk TEXT PRIMARY KEY, track_hk TEXT NOT NULL, album_hk TEXT NOT NULL,
            load_date TEXT NOT NULL, record_source TEXT NOT NULL)",
        "CREATE INDEX IF NOT EXISTS ix_lta2_album ON link_track_album(album_hk)",
        // Satellites (descriptive, insert-only).
        "CREATE TABLE IF NOT EXISTS sat_track_meta (
            track_hk TEXT NOT NULL, load_date TEXT NOT NULL, hash_diff TEXT NOT NULL,
            title TEXT, artist TEXT, album TEXT, duration_ms INTEGER, explicit INTEGER,
            image_url TEXT, record_source TEXT NOT NULL, PRIMARY KEY (track_hk, load_date))",
        "CREATE TABLE IF NOT EXISTS sat_track_analysis (
            track_hk TEXT NOT NULL, load_date TEXT NOT NULL, hash_diff TEXT NOT NULL,
            bpm REAL, musical_key TEXT, camelot TEXT, energy REAL,
            analyzer TEXT NOT NULL, record_source TEXT NOT NULL,
            PRIMARY KEY (track_hk, load_date, analyzer))",
        "CREATE TABLE IF NOT EXISTS sat_track_download (
            track_hk TEXT NOT NULL, load_date TEXT NOT NULL, hash_diff TEXT NOT NULL,
            state TEXT, source_format TEXT, formats TEXT, record_source TEXT NOT NULL,
            PRIMARY KEY (track_hk, load_date))",
        "CREATE TABLE IF NOT EXISTS sat_artist_extid (
            artist_hk TEXT NOT NULL, load_date TEXT NOT NULL, hash_diff TEXT NOT NULL,
            service TEXT NOT NULL, external_id TEXT NOT NULL, url TEXT, record_source TEXT NOT NULL,
            PRIMARY KEY (artist_hk, service, load_date))",
        "CREATE TABLE IF NOT EXISTS sat_artist_meta (
            artist_hk TEXT NOT NULL, load_date TEXT NOT NULL, hash_diff TEXT NOT NULL,
            image_url TEXT, followers INTEGER, popularity INTEGER, genres TEXT,
            record_source TEXT NOT NULL, PRIMARY KEY (artist_hk, load_date))",
        "CREATE TABLE IF NOT EXISTS sat_album_meta (
            album_hk TEXT NOT NULL, load_date TEXT NOT NULL, hash_diff TEXT NOT NULL,
            album_type TEXT, release_date TEXT, total_tracks INTEGER, label TEXT, image_url TEXT,
            record_source TEXT NOT NULL, PRIMARY KEY (album_hk, load_date))",
        "CREATE TABLE IF NOT EXISTS sat_album_extid (
            album_hk TEXT NOT NULL, load_date TEXT NOT NULL, hash_diff TEXT NOT NULL,
            service TEXT NOT NULL, external_id TEXT NOT NULL, url TEXT, record_source TEXT NOT NULL,
            PRIMARY KEY (album_hk, service, load_date))",
    ];
    for sql in ddl {
        sqlx::query(sql).execute(pool).await?;
    }
    Ok(())
}

// ── Hub loaders (idempotent: insert the business key once) ───────────────────

/// Load a track hub by ISRC. Returns its `track_hk`.
pub async fn load_track(
    conn: &mut SqliteConnection,
    isrc: &str,
    record_source: &str,
) -> Result<String> {
    let bk = db::normalize_isrc(isrc);
    let hk = hash_key(&bk);
    sqlx::query(
        "INSERT INTO hub_track (track_hk, isrc, load_date, record_source)
         VALUES (?, ?, ?, ?) ON CONFLICT(track_hk) DO NOTHING",
    )
    .bind(&hk)
    .bind(&bk)
    .bind(load_date())
    .bind(record_source)
    .execute(&mut *conn)
    .await?;
    Ok(hk)
}

/// Load an artist hub by normalised name. `None` for a blank name.
pub async fn load_artist(
    conn: &mut SqliteConnection,
    name: &str,
    record_source: &str,
) -> Result<Option<String>> {
    let bk = normalize_name(name);
    if bk.is_empty() {
        return Ok(None);
    }
    let hk = hash_key(&bk);
    sqlx::query(
        "INSERT INTO hub_artist (artist_hk, artist_name, load_date, record_source)
         VALUES (?, ?, ?, ?) ON CONFLICT(artist_hk) DO NOTHING",
    )
    .bind(&hk)
    .bind(&bk)
    .bind(load_date())
    .bind(record_source)
    .execute(&mut *conn)
    .await?;
    Ok(Some(hk))
}

/// Load an album hub by normalised title (+ release date). `None` for a blank title.
pub async fn load_album(
    conn: &mut SqliteConnection,
    title: &str,
    release_date: Option<&str>,
    record_source: &str,
) -> Result<Option<String>> {
    let bk = normalize_album_key(title, release_date);
    if bk.is_empty() {
        return Ok(None);
    }
    let hk = hash_key(&bk);
    sqlx::query(
        "INSERT INTO hub_album (album_hk, album_key, load_date, record_source)
         VALUES (?, ?, ?, ?) ON CONFLICT(album_hk) DO NOTHING",
    )
    .bind(&hk)
    .bind(&bk)
    .bind(load_date())
    .bind(record_source)
    .execute(&mut *conn)
    .await?;
    Ok(Some(hk))
}

// ── Link loaders (idempotent: the relationship is written once) ──────────────

pub async fn link_track_artist(
    conn: &mut SqliteConnection,
    track_hk: &str,
    artist_hk: &str,
    position: i64,
    record_source: &str,
) -> Result<()> {
    let lk = link_key(&[track_hk, artist_hk]);
    sqlx::query(
        "INSERT INTO link_track_artist
            (track_artist_lk, track_hk, artist_hk, position, load_date, record_source)
         VALUES (?, ?, ?, ?, ?, ?) ON CONFLICT(track_artist_lk) DO NOTHING",
    )
    .bind(&lk)
    .bind(track_hk)
    .bind(artist_hk)
    .bind(position)
    .bind(load_date())
    .bind(record_source)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

pub async fn link_track_album(
    conn: &mut SqliteConnection,
    track_hk: &str,
    album_hk: &str,
    record_source: &str,
) -> Result<()> {
    let lk = link_key(&[track_hk, album_hk]);
    sqlx::query(
        "INSERT INTO link_track_album (track_album_lk, track_hk, album_hk, load_date, record_source)
         VALUES (?, ?, ?, ?, ?) ON CONFLICT(track_album_lk) DO NOTHING",
    )
    .bind(&lk)
    .bind(track_hk)
    .bind(album_hk)
    .bind(load_date())
    .bind(record_source)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

// ── Satellite loaders (insert-only, hash_diff-gated) ─────────────────────────

/// Latest stored `hash_diff` for a satellite keyed by `(hk)` or `(hk, extra)`.
async fn latest_diff(
    conn: &mut SqliteConnection,
    table: &str,
    hk_col: &str,
    hk: &str,
    extra_col: Option<&str>,
    extra: Option<&str>,
) -> Result<Option<String>> {
    let mut sql = format!("SELECT hash_diff FROM {table} WHERE {hk_col} = ?");
    if let Some(col) = extra_col {
        sql.push_str(&format!(" AND {col} = ?"));
    }
    sql.push_str(" ORDER BY load_date DESC LIMIT 1");
    let mut q = sqlx::query_scalar::<_, String>(&sql).bind(hk);
    if let Some(e) = extra {
        q = q.bind(e);
    }
    Ok(q.fetch_optional(&mut *conn).await?)
}

/// Track metadata satellite. Returns `true` when a new row was written.
pub async fn load_track_meta(
    conn: &mut SqliteConnection,
    track_hk: &str,
    title: Option<&str>,
    artist: Option<&str>,
    album: Option<&str>,
    record_source: &str,
) -> Result<bool> {
    let hd = hash_diff(&[opt_str(title), opt_str(artist), opt_str(album)]);
    if latest_diff(conn, "sat_track_meta", "track_hk", track_hk, None, None)
        .await?
        .as_deref()
        == Some(hd.as_str())
    {
        return Ok(false);
    }
    sqlx::query(
        "INSERT OR IGNORE INTO sat_track_meta
            (track_hk, load_date, hash_diff, title, artist, album, record_source)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(track_hk)
    .bind(load_date())
    .bind(&hd)
    .bind(title)
    .bind(artist)
    .bind(album)
    .bind(record_source)
    .execute(&mut *conn)
    .await?;
    Ok(true)
}

/// Track metadata satellite including the duration. Same insert-only,
/// hash_diff-gated contract as [`load_track_meta`]; kept separate so the existing
/// callers' hash (title/artist/album only) is unchanged.
#[allow(clippy::too_many_arguments)]
pub async fn load_track_meta_full(
    conn: &mut SqliteConnection,
    track_hk: &str,
    title: Option<&str>,
    artist: Option<&str>,
    album: Option<&str>,
    duration_ms: Option<i64>,
    record_source: &str,
) -> Result<bool> {
    let hd = hash_diff(&[
        opt_str(title),
        opt_str(artist),
        opt_str(album),
        duration_ms.map(|x| x.to_string()),
    ]);
    if latest_diff(conn, "sat_track_meta", "track_hk", track_hk, None, None)
        .await?
        .as_deref()
        == Some(hd.as_str())
    {
        return Ok(false);
    }
    sqlx::query(
        "INSERT OR IGNORE INTO sat_track_meta
            (track_hk, load_date, hash_diff, title, artist, album, duration_ms, record_source)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(track_hk)
    .bind(load_date())
    .bind(&hd)
    .bind(title)
    .bind(artist)
    .bind(album)
    .bind(duration_ms)
    .bind(record_source)
    .execute(&mut *conn)
    .await?;
    Ok(true)
}
/// point of the vault: "these were the analysis results" stays visible.
#[allow(clippy::too_many_arguments)]
pub async fn load_analysis(
    conn: &mut SqliteConnection,
    track_hk: &str,
    analyzer: &str,
    record_source: &str,
    bpm: Option<f64>,
    musical_key: Option<&str>,
    camelot: Option<&str>,
    energy: Option<f64>,
) -> Result<bool> {
    let hd = hash_diff(&[
        opt_f64(bpm),
        opt_str(musical_key),
        opt_str(camelot),
        opt_f64(energy),
    ]);
    if latest_diff(
        conn,
        "sat_track_analysis",
        "track_hk",
        track_hk,
        Some("analyzer"),
        Some(analyzer),
    )
    .await?
    .as_deref()
        == Some(hd.as_str())
    {
        return Ok(false);
    }
    sqlx::query(
        "INSERT OR IGNORE INTO sat_track_analysis
            (track_hk, load_date, hash_diff, bpm, musical_key, camelot, energy, analyzer, record_source)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(track_hk)
    .bind(load_date())
    .bind(&hd)
    .bind(bpm)
    .bind(musical_key)
    .bind(camelot)
    .bind(energy)
    .bind(analyzer)
    .bind(record_source)
    .execute(&mut *conn)
    .await?;
    Ok(true)
}

/// Download-state satellite — a snapshot of the operational `tracks.state`.
pub async fn load_track_download(
    conn: &mut SqliteConnection,
    track_hk: &str,
    state: &str,
    source_format: Option<&str>,
    formats: &str,
    record_source: &str,
) -> Result<bool> {
    let hd = hash_diff(&[
        opt_str(Some(state)),
        opt_str(source_format),
        opt_str(Some(formats)),
    ]);
    if latest_diff(conn, "sat_track_download", "track_hk", track_hk, None, None)
        .await?
        .as_deref()
        == Some(hd.as_str())
    {
        return Ok(false);
    }
    sqlx::query(
        "INSERT OR IGNORE INTO sat_track_download
            (track_hk, load_date, hash_diff, state, source_format, formats, record_source)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(track_hk)
    .bind(load_date())
    .bind(&hd)
    .bind(state)
    .bind(source_format)
    .bind(formats)
    .bind(record_source)
    .execute(&mut *conn)
    .await?;
    Ok(true)
}

/// Artist external-id satellite (provider ids are satellites, not hubs).
pub async fn load_artist_extid(
    conn: &mut SqliteConnection,
    artist_hk: &str,
    service: &str,
    external_id: &str,
    url: Option<&str>,
    record_source: &str,
) -> Result<bool> {
    let hd = hash_diff(&[opt_str(Some(external_id)), opt_str(url)]);
    if latest_diff(
        conn,
        "sat_artist_extid",
        "artist_hk",
        artist_hk,
        Some("service"),
        Some(service),
    )
    .await?
    .as_deref()
        == Some(hd.as_str())
    {
        return Ok(false);
    }
    sqlx::query(
        "INSERT OR IGNORE INTO sat_artist_extid
            (artist_hk, load_date, hash_diff, service, external_id, url, record_source)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(artist_hk)
    .bind(load_date())
    .bind(&hd)
    .bind(service)
    .bind(external_id)
    .bind(url)
    .bind(record_source)
    .execute(&mut *conn)
    .await?;
    Ok(true)
}

/// Album external-id satellite.
pub async fn load_album_extid(
    conn: &mut SqliteConnection,
    album_hk: &str,
    service: &str,
    external_id: &str,
    url: Option<&str>,
    record_source: &str,
) -> Result<bool> {
    let hd = hash_diff(&[opt_str(Some(external_id)), opt_str(url)]);
    if latest_diff(
        conn,
        "sat_album_extid",
        "album_hk",
        album_hk,
        Some("service"),
        Some(service),
    )
    .await?
    .as_deref()
        == Some(hd.as_str())
    {
        return Ok(false);
    }
    sqlx::query(
        "INSERT OR IGNORE INTO sat_album_extid
            (album_hk, load_date, hash_diff, service, external_id, url, record_source)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(album_hk)
    .bind(load_date())
    .bind(&hd)
    .bind(service)
    .bind(external_id)
    .bind(url)
    .bind(record_source)
    .execute(&mut *conn)
    .await?;
    Ok(true)
}

/// Artist descriptive satellite (image/followers/popularity/genres).
#[allow(clippy::too_many_arguments)]
pub async fn load_artist_meta(
    conn: &mut SqliteConnection,
    artist_hk: &str,
    image_url: Option<&str>,
    followers: Option<i64>,
    popularity: Option<i64>,
    genres: Option<&str>,
    record_source: &str,
) -> Result<bool> {
    let hd = hash_diff(&[
        opt_str(image_url),
        followers.map(|x| x.to_string()),
        popularity.map(|x| x.to_string()),
        opt_str(genres),
    ]);
    if latest_diff(conn, "sat_artist_meta", "artist_hk", artist_hk, None, None)
        .await?
        .as_deref()
        == Some(hd.as_str())
    {
        return Ok(false);
    }
    sqlx::query(
        "INSERT OR IGNORE INTO sat_artist_meta
            (artist_hk, load_date, hash_diff, image_url, followers, popularity, genres, record_source)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(artist_hk)
    .bind(load_date())
    .bind(&hd)
    .bind(image_url)
    .bind(followers)
    .bind(popularity)
    .bind(genres)
    .bind(record_source)
    .execute(&mut *conn)
    .await?;
    Ok(true)
}

/// Album descriptive satellite (type/release/tracks/label/image).
#[allow(clippy::too_many_arguments)]
pub async fn load_album_meta(
    conn: &mut SqliteConnection,
    album_hk: &str,
    album_type: Option<&str>,
    release_date: Option<&str>,
    total_tracks: Option<i64>,
    label: Option<&str>,
    image_url: Option<&str>,
    record_source: &str,
) -> Result<bool> {
    let hd = hash_diff(&[
        opt_str(album_type),
        opt_str(release_date),
        total_tracks.map(|x| x.to_string()),
        opt_str(label),
        opt_str(image_url),
    ]);
    if latest_diff(conn, "sat_album_meta", "album_hk", album_hk, None, None)
        .await?
        .as_deref()
        == Some(hd.as_str())
    {
        return Ok(false);
    }
    sqlx::query(
        "INSERT OR IGNORE INTO sat_album_meta
            (album_hk, load_date, hash_diff, album_type, release_date, total_tracks, label, image_url, record_source)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(album_hk)
    .bind(load_date())
    .bind(&hd)
    .bind(album_type)
    .bind(release_date)
    .bind(total_tracks)
    .bind(label)
    .bind(image_url)
    .bind(record_source)
    .execute(&mut *conn)
    .await?;
    Ok(true)
}

// ── Ingest (composite loads for the pipeline) ────────────────────────────────

/// Ingest a resolved Deezer track: hubs, links, extids and the metadata
/// satellite. Idempotent — re-resolving the same track writes nothing new.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_deezer(
    conn: &mut SqliteConnection,
    isrc: &str,
    deezer_id: &str,
    title: &str,
    artist: &str,
    album: &str,
    artist_id: Option<&str>,
    album_id: Option<&str>,
) -> Result<()> {
    let track_hk = load_track(conn, isrc, "deezer").await?;

    if let Some(artist_hk) = load_artist(conn, artist, "deezer").await? {
        link_track_artist(conn, &track_hk, &artist_hk, 0, "deezer").await?;
        if let Some(id) = artist_id {
            let url = format!("https://www.deezer.com/artist/{id}");
            load_artist_extid(conn, &artist_hk, "deezer", id, Some(&url), "deezer").await?;
        }
    }
    if let Some(album_hk) = load_album(conn, album, None, "deezer").await? {
        link_track_album(conn, &track_hk, &album_hk, "deezer").await?;
        if let Some(id) = album_id {
            let url = format!("https://www.deezer.com/album/{id}");
            load_album_extid(conn, &album_hk, "deezer", id, Some(&url), "deezer").await?;
        }
    }
    // The Deezer track id is descriptive of the track, not a hub.
    let _ = deezer_id;
    load_track_meta(
        conn,
        &track_hk,
        Some(title),
        Some(artist),
        Some(album),
        "deezer",
    )
    .await?;
    Ok(())
}

/// Snapshot the operational download state into the vault.
pub async fn load_download(
    conn: &mut SqliteConnection,
    isrc: &str,
    state: &str,
    source_format: Option<&str>,
    formats: &str,
    record_source: &str,
) -> Result<()> {
    let track_hk = load_track(conn, isrc, record_source).await?;
    load_track_download(
        conn,
        &track_hk,
        state,
        source_format,
        formats,
        record_source,
    )
    .await?;
    Ok(())
}

/// Ingest a batch of hub-pushed metadata items: the analysis satellite (keyed by
/// the item's `source` as the analyzer) plus the metadata satellite.
pub async fn ingest_metadata(pool: &Pool<Sqlite>, items: &[MetadataItem]) -> Result<usize> {
    let mut conn = pool.acquire().await?;
    let mut n = 0usize;
    for it in items {
        let isrc = db::normalize_isrc(&it.isrc);
        if isrc.is_empty() {
            continue;
        }
        let track_hk = load_track(&mut conn, &isrc, "hub").await?;
        let analyzer = it
            .source
            .as_deref()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "hub".to_string());
        if it.bpm.is_some() || it.key.is_some() || it.camelot.is_some() {
            load_analysis(
                &mut conn,
                &track_hk,
                &analyzer,
                "hub",
                it.bpm,
                it.key.as_deref(),
                it.camelot.as_deref(),
                None,
            )
            .await?;
        }
        if it.title.is_some() || it.artist.is_some() || it.album.is_some() {
            load_track_meta(
                &mut conn,
                &track_hk,
                it.title.as_deref(),
                it.artist.as_deref(),
                it.album.as_deref(),
                "hub",
            )
            .await?;
        }
        n += 1;
    }
    Ok(n)
}

/// Ingest one downloaded file's tags into the vault: the track hub (by ISRC),
/// the artist/album hubs, the links, and the metadata satellite (with duration).
/// Returns the `track_hk`, or `None` when there is no ISRC — the vault is keyed
/// by ISRC, so a file without one is a best-effort skip, not an error.
#[allow(clippy::too_many_arguments)]
pub async fn ingest_file_tags(
    conn: &mut SqliteConnection,
    isrc: &str,
    title: Option<&str>,
    artist: Option<&str>,
    album: Option<&str>,
    duration_ms: Option<i64>,
    record_source: &str,
) -> Result<Option<String>> {
    let bk = db::normalize_isrc(isrc);
    if bk.is_empty() {
        return Ok(None);
    }
    let track_hk = load_track(conn, &bk, record_source).await?;

    if let Some(name) = artist.filter(|s| !s.trim().is_empty()) {
        if let Some(artist_hk) = load_artist(conn, name, record_source).await? {
            link_track_artist(conn, &track_hk, &artist_hk, 0, record_source).await?;
        }
    }
    if let Some(title) = album.filter(|s| !s.trim().is_empty()) {
        if let Some(album_hk) = load_album(conn, title, None, record_source).await? {
            link_track_album(conn, &track_hk, &album_hk, record_source).await?;
        }
    }
    load_track_meta_full(
        conn,
        &track_hk,
        title,
        artist,
        album,
        duration_ms,
        record_source,
    )
    .await?;
    Ok(Some(track_hk))
}

// ── Consumer resolution (precedence over the latest per-analyzer rows) ───────

/// The resolved view of one track: the "right" BPM/key picked by precedence over
/// the latest per-analyzer satellite rows.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedTrack {
    pub isrc: String,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub camelot: Option<String>,
    pub energy: Option<f64>,
    /// The analyzer that provided the chosen BPM (fallback: key/camelot/energy).
    pub source: Option<String>,
    /// Latest `load_date` among the contributing rows (ISO-8601 UTC).
    pub updated_at: Option<String>,
}

#[derive(sqlx::FromRow)]
struct AnalysisRow {
    isrc: String,
    analyzer: Option<String>,
    load_date: Option<String>,
    bpm: Option<f64>,
    musical_key: Option<String>,
    camelot: Option<String>,
    energy: Option<f64>,
}

/// Resolve many ISRCs at once. One row per **known** ISRC (a `hub_track` exists),
/// in input order; unknown ISRCs are omitted. Fields are picked per-field by
/// `precedence` (e.g. `traktor,essentia,reccobeats`), falling back to any
/// analyzer that has a value.
pub async fn resolve_tracks(
    pool: &Pool<Sqlite>,
    isrcs: &[String],
    precedence: &[String],
) -> Result<Vec<ResolvedTrack>> {
    if isrcs.is_empty() {
        return Ok(Vec::new());
    }
    let mut qb = QueryBuilder::new(
        "SELECT h.isrc AS isrc, a.analyzer AS analyzer, a.load_date AS load_date,
                a.bpm AS bpm, a.musical_key AS musical_key, a.camelot AS camelot, a.energy AS energy
           FROM hub_track h
           LEFT JOIN sat_track_analysis a ON a.track_hk = h.track_hk
          WHERE h.isrc IN (",
    );
    let mut sep = qb.separated(", ");
    for i in isrcs {
        sep.push_bind(i.clone());
    }
    qb.push(") ORDER BY h.isrc, a.load_date DESC");
    let rows = qb.build_query_as::<AnalysisRow>().fetch_all(pool).await?;

    // Group: isrc → (analyzer → latest row). Rows arrive newest-first.
    let mut by_isrc: BTreeMap<String, BTreeMap<String, AnalysisRow>> = BTreeMap::new();
    for r in rows {
        let entry = by_isrc.entry(r.isrc.clone()).or_default();
        if let Some(analyzer) = r.analyzer.clone() {
            entry.entry(analyzer).or_insert(r);
        }
    }

    let mut out = Vec::new();
    for isrc in isrcs {
        let Some(analyzers) = by_isrc.get(isrc) else {
            continue; // unknown ISRC
        };
        out.push(resolve_one(isrc, analyzers, precedence));
    }
    Ok(out)
}

fn resolve_one(
    isrc: &str,
    analyzers: &BTreeMap<String, AnalysisRow>,
    precedence: &[String],
) -> ResolvedTrack {
    // Precedence order first, then any remaining analyzer (newest first).
    let mut ordered: Vec<&AnalysisRow> = Vec::new();
    for a in precedence {
        if let Some(r) = analyzers.get(a) {
            ordered.push(r);
        }
    }
    let mut rest: Vec<&AnalysisRow> = analyzers
        .values()
        .filter(|r| {
            !precedence
                .iter()
                .any(|p| p == r.analyzer.as_deref().unwrap_or(""))
        })
        .collect();
    rest.sort_by(|a, b| b.load_date.cmp(&a.load_date));
    ordered.extend(rest);

    let bpm = ordered.iter().find(|r| r.bpm.is_some());
    let key = ordered.iter().find(|r| r.musical_key.is_some());
    let camelot = ordered.iter().find(|r| r.camelot.is_some());
    let energy = ordered.iter().find(|r| r.energy.is_some());
    let source = bpm
        .or(key)
        .or(camelot)
        .or(energy)
        .and_then(|r| r.analyzer.clone());
    let updated_at = ordered.iter().filter_map(|r| r.load_date.clone()).max();

    ResolvedTrack {
        isrc: isrc.to_string(),
        bpm: bpm.and_then(|r| r.bpm),
        key: key.and_then(|r| r.musical_key.clone()),
        camelot: camelot.and_then(|r| r.camelot.clone()),
        energy: energy.and_then(|r| r.energy),
        source,
        updated_at,
    }
}

/// Resolve a single ISRC (the fast path). `None` when the ISRC is unknown.
pub async fn resolve_track(
    pool: &Pool<Sqlite>,
    isrc: &str,
    precedence: &[String],
) -> Result<Option<ResolvedTrack>> {
    let key = db::normalize_key(isrc);
    Ok(resolve_tracks(pool, std::slice::from_ref(&key), precedence)
        .await?
        .into_iter()
        .next())
}

// ── Consumer detail view (the atomic track resource) ─────────────────────────
//
// The fully-assembled view of one track: the hub (business key + newest
// metadata), the links (artists/album with their external ids) and *all*
// satellites — transparently, so a consumer can render HUB + LINKS + SATELLITES
// and pick the value it trusts. `resolved` mirrors `/vault/tracks`.

/// `GET /vault/track/{isrc}` payload.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultTrackDetail {
    pub isrc: String,
    pub hub: VaultHub,
    pub resolved: VaultResolved,
    pub links: VaultLinks,
    pub satellites: VaultSatellites,
}

/// `hub_track` + the newest `sat_track_meta`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultHub {
    pub title: Option<String>,
    pub duration_ms: Option<i64>,
    pub explicit: Option<bool>,
}

/// The best BPM/key/key-selector picked by precedence — the same resolution
/// `/vault/tracks` uses (the `isrc`/`updatedAt` envelope fields are dropped here,
/// since the ISRC is the top-level key of the detail view).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultResolved {
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub camelot: Option<String>,
    pub energy: Option<f64>,
    pub source: Option<String>,
}

/// Relationships out of the track hub.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultLinks {
    pub artists: Vec<VaultArtistLink>,
    pub album: Option<VaultAlbumLink>,
}

/// `link_track_artist` + `hub_artist` + the newest `sat_artist_extid` per service.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultArtistLink {
    /// The artist hub's business key — **normalised** (collapsed, uppercased).
    pub name: Option<String>,
    pub external_ids: BTreeMap<String, String>,
}

/// `link_track_album` + `hub_album` + the newest `sat_album_extid` per service.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultAlbumLink {
    /// The title part of the album hub's business key (**normalised**).
    pub title: Option<String>,
    pub release_date: Option<String>,
    pub external_ids: BTreeMap<String, String>,
}

/// Every satellite of the track — nothing resolved away.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultSatellites {
    pub analysis: Vec<VaultAnalysis>,
    pub download: Option<VaultDownload>,
    pub meta: Vec<VaultMeta>,
}

/// One analysis source — the **newest row per analyzer** (`traktor`, `essentia`,
/// `reccobeats`, …), all of them, so a consumer sees every truth.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultAnalysis {
    pub analyzer: String,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub camelot: Option<String>,
    pub energy: Option<f64>,
    pub load_date: String,
}

/// The newest `sat_track_download` snapshot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultDownload {
    pub state: Option<String>,
    pub source_format: Option<String>,
    pub formats: Vec<String>,
    pub load_date: String,
}

/// One metadata source — the newest `sat_track_meta` row **per `record_source`**.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultMeta {
    pub record_source: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub load_date: String,
}

#[derive(sqlx::FromRow)]
struct HubMetaRow {
    title: Option<String>,
    duration_ms: Option<i64>,
    explicit: Option<i64>,
}

#[derive(sqlx::FromRow)]
struct AnalysisSatRow {
    analyzer: String,
    bpm: Option<f64>,
    musical_key: Option<String>,
    camelot: Option<String>,
    energy: Option<f64>,
    load_date: String,
}

#[derive(sqlx::FromRow)]
struct DownloadSatRow {
    state: Option<String>,
    source_format: Option<String>,
    formats: Option<String>,
    load_date: String,
}

#[derive(sqlx::FromRow)]
struct MetaSatRow {
    record_source: String,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    duration_ms: Option<i64>,
    load_date: String,
}

#[derive(sqlx::FromRow)]
struct ArtistLinkRow {
    artist_hk: String,
    artist_name: String,
}

#[derive(sqlx::FromRow)]
struct ExtIdRow {
    service: String,
    external_id: String,
}

/// Assemble the atomic detail view of one track. `None` when the ISRC is unknown
/// to the vault (no `hub_track` row) — the caller turns that into a `404`.
pub async fn track_detail(
    pool: &Pool<Sqlite>,
    isrc: &str,
    precedence: &[String],
) -> Result<Option<VaultTrackDetail>> {
    let isrc = db::normalize_isrc(isrc);
    let track_hk: Option<String> =
        sqlx::query_scalar("SELECT track_hk FROM hub_track WHERE isrc = ?")
            .bind(&isrc)
            .fetch_optional(pool)
            .await?;
    let Some(track_hk) = track_hk else {
        return Ok(None);
    };

    // hub ← the newest `sat_track_meta`.
    let hub_row = sqlx::query_as::<_, HubMetaRow>(
        "SELECT title, duration_ms, explicit FROM sat_track_meta
          WHERE track_hk = ? ORDER BY load_date DESC LIMIT 1",
    )
    .bind(&track_hk)
    .fetch_optional(pool)
    .await?;
    let hub = VaultHub {
        title: hub_row.as_ref().and_then(|r| r.title.clone()),
        duration_ms: hub_row.as_ref().and_then(|r| r.duration_ms),
        explicit: hub_row.as_ref().and_then(|r| r.explicit).map(|e| e != 0),
    };

    // resolved ← the same precedence resolver as `/vault/tracks`.
    let resolved = resolve_tracks(pool, std::slice::from_ref(&isrc), precedence)
        .await?
        .into_iter()
        .next()
        .map(|r| VaultResolved {
            bpm: r.bpm,
            key: r.key,
            camelot: r.camelot,
            energy: r.energy,
            source: r.source,
        })
        .unwrap_or(VaultResolved {
            bpm: None,
            key: None,
            camelot: None,
            energy: None,
            source: None,
        });

    // satellites.analysis ← the newest row per analyzer (all analyzers).
    let analysis = sqlx::query_as::<_, AnalysisSatRow>(
        "SELECT analyzer, bpm, musical_key, camelot, energy, load_date
           FROM sat_track_analysis a
          WHERE track_hk = ?
            AND load_date = (SELECT MAX(load_date) FROM sat_track_analysis b
                              WHERE b.track_hk = a.track_hk AND b.analyzer = a.analyzer)
          ORDER BY analyzer",
    )
    .bind(&track_hk)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| VaultAnalysis {
        analyzer: r.analyzer,
        bpm: r.bpm,
        key: r.musical_key,
        camelot: r.camelot,
        energy: r.energy,
        load_date: r.load_date,
    })
    .collect();

    // satellites.download ← the newest snapshot.
    let download = sqlx::query_as::<_, DownloadSatRow>(
        "SELECT state, source_format, formats, load_date FROM sat_track_download
          WHERE track_hk = ? ORDER BY load_date DESC LIMIT 1",
    )
    .bind(&track_hk)
    .fetch_optional(pool)
    .await?
    .map(|r| VaultDownload {
        state: r.state,
        source_format: r.source_format,
        formats: r.formats.as_deref().map(split_formats).unwrap_or_default(),
        load_date: r.load_date,
    });

    // satellites.meta ← the newest row per record_source (all sources).
    let meta = sqlx::query_as::<_, MetaSatRow>(
        "SELECT record_source, title, artist, album, duration_ms, load_date
           FROM sat_track_meta a
          WHERE track_hk = ?
            AND load_date = (SELECT MAX(load_date) FROM sat_track_meta b
                              WHERE b.track_hk = a.track_hk AND b.record_source = a.record_source)
          ORDER BY record_source",
    )
    .bind(&track_hk)
    .fetch_all(pool)
    .await?
    .into_iter()
    .map(|r| VaultMeta {
        record_source: r.record_source,
        title: r.title,
        artist: r.artist,
        album: r.album,
        duration_ms: r.duration_ms,
        load_date: r.load_date,
    })
    .collect();

    // links.artists ← link_track_artist + hub_artist + newest `sat_artist_extid`.
    let artist_rows = sqlx::query_as::<_, ArtistLinkRow>(
        "SELECT ha.artist_hk, ha.artist_name
           FROM link_track_artist l
           JOIN hub_artist ha ON ha.artist_hk = l.artist_hk
          WHERE l.track_hk = ?
          ORDER BY l.position, ha.artist_name",
    )
    .bind(&track_hk)
    .fetch_all(pool)
    .await?;
    let mut artists = Vec::with_capacity(artist_rows.len());
    for a in artist_rows {
        let extids = sqlx::query_as::<_, ExtIdRow>(
            "SELECT service, external_id FROM sat_artist_extid x
              WHERE artist_hk = ?
                AND load_date = (SELECT MAX(load_date) FROM sat_artist_extid y
                                  WHERE y.artist_hk = x.artist_hk AND y.service = x.service)
              ORDER BY service",
        )
        .bind(&a.artist_hk)
        .fetch_all(pool)
        .await?;
        artists.push(VaultArtistLink {
            name: Some(a.artist_name),
            external_ids: extids
                .into_iter()
                .map(|e| (e.service, e.external_id))
                .collect(),
        });
    }

    // links.album ← link_track_album + hub_album + newest `sat_album_extid`.
    let album = match sqlx::query_scalar::<_, String>(
        "SELECT album_hk FROM link_track_album WHERE track_hk = ? ORDER BY load_date DESC LIMIT 1",
    )
    .bind(&track_hk)
    .fetch_optional(pool)
    .await?
    {
        Some(album_hk) => Some(album_link(pool, &album_hk).await?),
        None => None,
    };

    Ok(Some(VaultTrackDetail {
        isrc,
        hub,
        resolved,
        links: VaultLinks { artists, album },
        satellites: VaultSatellites {
            analysis,
            download,
            meta,
        },
    }))
}

/// Build the album link: the title/release date from the hub's business key
/// (`TITLE|DATE`), the release date preferred from the newest `sat_album_meta`,
/// and the newest `sat_album_extid` per service.
async fn album_link(pool: &Pool<Sqlite>, album_hk: &str) -> Result<VaultAlbumLink> {
    let album_key: Option<String> =
        sqlx::query_scalar("SELECT album_key FROM hub_album WHERE album_hk = ?")
            .bind(album_hk)
            .fetch_optional(pool)
            .await?;
    let (title, key_date) = match album_key.as_deref() {
        Some(k) => match k.split_once('|') {
            Some((t, d)) => (Some(t.to_string()), Some(d.to_string())),
            None => (Some(k.to_string()), None),
        },
        None => (None, None),
    };
    let sat_date: Option<String> = sqlx::query_scalar(
        "SELECT release_date FROM sat_album_meta
          WHERE album_hk = ? AND release_date IS NOT NULL
          ORDER BY load_date DESC LIMIT 1",
    )
    .bind(album_hk)
    .fetch_optional(pool)
    .await?;
    let extids = sqlx::query_as::<_, ExtIdRow>(
        "SELECT service, external_id FROM sat_album_extid x
          WHERE album_hk = ?
            AND load_date = (SELECT MAX(load_date) FROM sat_album_extid y
                              WHERE y.album_hk = x.album_hk AND y.service = x.service)
          ORDER BY service",
    )
    .bind(album_hk)
    .fetch_all(pool)
    .await?;
    Ok(VaultAlbumLink {
        title,
        release_date: sat_date.or(key_date),
        external_ids: extids
            .into_iter()
            .map(|e| (e.service, e.external_id))
            .collect(),
    })
}

/// Split the stored comma-joined `formats` column into a list (empty when blank).
fn split_formats(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

// ── Backfill (off the boot path, batched, transactional, idempotent) ─────────

#[derive(Debug, Default, Clone, Copy)]
pub struct BackfillStats {
    pub tracks: usize,
    pub analyses: usize,
    pub batches: usize,
}

/// Backfill the vault from the operational tables (`tracks` + `track_metadata`).
///
/// **Never** called on the boot path — only from the `backfill-vault` CLI. Runs
/// in bounded batches, one transaction per batch, and is idempotent (hub upserts
/// + hash_diff-gated satellites), so it is safe to re-run.
pub async fn backfill(pool: &Pool<Sqlite>, batch: i64) -> Result<BackfillStats> {
    let batch = batch.max(1);
    let mut stats = BackfillStats::default();

    // 1) tracks → hub_track + links + sat_track_meta + sat_track_download.
    let mut last = String::new();
    loop {
        let rows =
            sqlx::query_as::<_, Track>("SELECT * FROM tracks WHERE isrc > ? ORDER BY isrc LIMIT ?")
                .bind(&last)
                .bind(batch)
                .fetch_all(pool)
                .await?;
        if rows.is_empty() {
            break;
        }
        let mut tx = pool.begin().await?;
        for t in &rows {
            // Composite (`<source>:<id>`) keys are not ISRCs — not a vault track bk.
            if t.isrc.contains(':') {
                continue;
            }
            let track_hk = load_track(&mut tx, &t.isrc, "backfill").await?;
            if let Some(artist) = t.artist.as_deref().filter(|s| !s.trim().is_empty()) {
                if let Some(artist_hk) = load_artist(&mut tx, artist, "backfill").await? {
                    link_track_artist(&mut tx, &track_hk, &artist_hk, 0, "backfill").await?;
                }
            }
            if let Some(album) = t.album.as_deref().filter(|s| !s.trim().is_empty()) {
                if let Some(album_hk) = load_album(&mut tx, album, None, "backfill").await? {
                    link_track_album(&mut tx, &track_hk, &album_hk, "backfill").await?;
                }
            }
            load_track_meta(
                &mut tx,
                &track_hk,
                t.title.as_deref(),
                t.artist.as_deref(),
                t.album.as_deref(),
                "backfill",
            )
            .await?;
            let formats = t.formats().join(",");
            load_track_download(
                &mut tx,
                &track_hk,
                &t.state,
                t.source_format.as_deref(),
                &formats,
                "backfill",
            )
            .await?;
            stats.tracks += 1;
        }
        tx.commit().await?;
        last = rows.last().expect("non-empty batch").isrc.clone();
        stats.batches += 1;
    }

    // 2) track_metadata → sat_track_analysis (analyzer = the stored source).
    let mut last = String::new();
    loop {
        let rows = sqlx::query_as::<_, TrackMetadata>(
            "SELECT * FROM track_metadata WHERE isrc > ? ORDER BY isrc LIMIT ?",
        )
        .bind(&last)
        .bind(batch)
        .fetch_all(pool)
        .await?;
        if rows.is_empty() {
            break;
        }
        let mut tx = pool.begin().await?;
        for m in &rows {
            if m.isrc.contains(':') {
                continue;
            }
            let track_hk = load_track(&mut tx, &m.isrc, "backfill").await?;
            let analyzer = m
                .source
                .as_deref()
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "hub".to_string());
            if m.bpm.is_some() || m.musical_key.is_some() || m.camelot.is_some() {
                load_analysis(
                    &mut tx,
                    &track_hk,
                    &analyzer,
                    "backfill",
                    m.bpm,
                    m.musical_key.as_deref(),
                    m.camelot.as_deref(),
                    None,
                )
                .await?;
            }
            stats.analyses += 1;
        }
        tx.commit().await?;
        last = rows.last().expect("non-empty batch").isrc.clone();
        stats.batches += 1;
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_key_is_upper_hex_sha256() {
        // SHA256("") = e3b0c442...  (well-known vector).
        assert_eq!(
            hash_key(""),
            "E3B0C44298FC1C149AFBF4C8996FB92427AE41E4649B934CA495991B7852B855"
        );
        assert_eq!(hash_key("ABC").len(), 64);
        assert!(hash_key("abc").chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn link_key_is_order_and_boundary_sensitive() {
        let a = link_key(&["A", "B"]);
        let b = link_key(&["B", "A"]);
        assert_ne!(a, b, "order matters");
        // Separator prevents `ab|c` colliding with `a|bc`.
        assert_ne!(link_key(&["ab", "c"]), link_key(&["a", "bc"]));
    }

    #[test]
    fn hash_diff_distinguishes_none_from_empty() {
        assert_ne!(hash_diff(&[None]), hash_diff(&[Some(String::new())]));
        assert_eq!(
            hash_diff(&[Some("x".into()), None]),
            hash_diff(&[Some("x".into()), None])
        );
        assert_ne!(
            hash_diff(&[Some("x".into())]),
            hash_diff(&[Some("x".into()), None])
        );
    }

    #[test]
    fn names_and_album_keys_normalise() {
        assert_eq!(normalize_name("  Molchat   Doma "), "MOLCHAT DOMA");
        assert_eq!(normalize_name(""), "");
        assert_eq!(normalize_album_key("Etazhi", None), "ETAZHI");
        assert_eq!(
            normalize_album_key("Etazhi", Some("2018-09-07")),
            "ETAZHI|2018-09-07"
        );
        assert_eq!(normalize_album_key("Etazhi", Some("  ")), "ETAZHI");
    }

    #[test]
    fn load_date_is_iso_utc() {
        let d = load_date();
        assert!(d.ends_with('Z'), "{d}");
        assert_eq!(d.len(), 27, "YYYY-MM-DDTHH:MM:SS.ffffffZ: {d}");
        assert_eq!(&d[4..5], "-");
        assert_eq!(&d[10..11], "T");
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1)); // 2024-01-01
    }
}
