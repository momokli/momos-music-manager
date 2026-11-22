//! #700 — `track_files` catalogue, `archive-backfill` and `/storage/stats`.

mod common;

use common::*;
use music_api::{archive, db};

fn write_file(path: &std::path::Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

async fn seed_track(pool: &sqlx::SqlitePool, isrc: &str, path128: Option<&str>) {
    let ts = db::now();
    sqlx::query(
        "INSERT INTO tracks
            (isrc, source, state, path_128, created_at, updated_at, wanted_formats)
         VALUES (?, 'deezer', 'ready', ?, ?, ?, '128')",
    )
    .bind(isrc)
    .bind(path128)
    .bind(ts)
    .bind(ts)
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_object(
    pool: &sqlx::SqlitePool,
    hash: &str,
    size: i64,
    content_type: Option<&str>,
    original_path: &str,
    isrc: Option<&str>,
    name: &str,
) {
    sqlx::query(
        "INSERT INTO store_objects
            (hash, size, content_type, original_path, isrc, name, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(hash)
    .bind(size)
    .bind(content_type)
    .bind(original_path)
    .bind(isrc)
    .bind(name)
    .bind(db::now())
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn backfill_catalogues_files_and_is_idempotent() {
    let h = harness().await;
    let cfg = &h.state.config;

    // One 128 output file referenced by a track row.
    let p128 = cfg.mp3_128_dir().join("Artist - Song (ISRC1).mp3");
    write_file(&p128, b"ID3\x04128");
    seed_track(&h.state.pool, "ISRC1", Some(&p128.display().to_string())).await;

    // A bundle (m4a with ISRC) plus its stem WAV in the same original folder.
    let bundle_hash = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let stem_hash = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let bundle_path = h.state.store.path_for(bundle_hash);
    let stem_path = h.state.store.path_for(stem_hash);
    write_file(&bundle_path, b"m4a-bundle");
    write_file(&stem_path, b"wav-vocals");
    seed_object(
        &h.state.pool,
        bundle_hash,
        10,
        Some("audio/mp4"),
        "Album/Song.stem.m4a",
        Some("ISRC1"),
        "Song.stem.m4a",
    )
    .await;
    seed_object(
        &h.state.pool,
        stem_hash,
        10,
        Some("audio/wav"),
        "Album/Song_vocals.wav",
        None,
        "Song_vocals.wav",
    )
    .await;

    let first = archive::backfill(&h.state.pool, cfg, &h.state.store, true)
        .await
        .unwrap();
    assert_eq!(first.mode, "apply");
    assert_eq!(first.candidates, 3, "{first:?}");
    assert_eq!(first.inserted, 3, "{first:?}");
    assert_eq!(first.updated, 0);
    assert_eq!(first.deleted, 0);
    assert_eq!(first.bundle_with_isrc, 1);
    assert_eq!(first.stem_with_isrc, 1, "stem must inherit the bundle ISRC");
    assert!(first.unmanaged.is_empty(), "{:?}", first.unmanaged);

    // Second apply is a no-op.
    let second = archive::backfill(&h.state.pool, cfg, &h.state.store, true)
        .await
        .unwrap();
    assert_eq!(second.inserted, 0, "{second:?}");
    assert_eq!(second.updated, 0, "{second:?}");
    assert_eq!(second.deleted, 0, "{second:?}");
    assert_eq!(second.unchanged, 3, "{second:?}");

    let rows: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT isrc, role, stem_part FROM track_files ORDER BY role")
            .fetch_all(&h.state.pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 3);
    assert!(rows.iter().any(|(i, r, sp)| i == "ISRC1"
        && r == archive::ROLE_STEM_PART
        && sp.as_deref() == Some("vocals")));
}

#[tokio::test]
async fn dry_run_writes_no_rows() {
    let h = harness().await;
    let cfg = &h.state.config;
    let p = cfg.mp3_320_dir().join("a.mp3");
    write_file(&p, b"abc");
    seed_track(&h.state.pool, "ISRC2", None).await;
    sqlx::query("UPDATE tracks SET path_320 = ? WHERE isrc = 'ISRC2'")
        .bind(p.display().to_string())
        .execute(&h.state.pool)
        .await
        .unwrap();

    let report = archive::backfill(&h.state.pool, cfg, &h.state.store, false)
        .await
        .unwrap();
    assert_eq!(report.mode, "dry-run");
    assert_eq!(report.inserted, 1);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM track_files")
        .fetch_one(&h.state.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "dry-run must not write");
}

#[tokio::test]
async fn unmanaged_files_are_reported() {
    let h = harness().await;
    let cfg = &h.state.config;
    let stray = cfg.mp3_128_dir().join("nobody-knows.mp3");
    write_file(&stray, b"xyz");

    let report = archive::backfill(&h.state.pool, cfg, &h.state.store, true)
        .await
        .unwrap();
    assert_eq!(report.unmanaged.len(), 1, "{:?}", report.unmanaged);
    assert_eq!(report.unmanaged[0].path, "128/nobody-knows.mp3");

    let stats = archive::storage_stats(&h.state.pool).await.unwrap();
    assert_eq!(stats.unmanaged_files, 1);
}

#[tokio::test]
async fn stats_aggregate_roles_from_catalogue_only() {
    let h = harness().await;
    let ts = "2026-11-22T00:00:00Z";
    for (isrc, role, size) in [
        ("A", archive::ROLE_128, 100i64),
        ("A", archive::ROLE_320, 300),
        ("B", archive::ROLE_FLAC, 1000),
    ] {
        sqlx::query(
            "INSERT INTO track_files (isrc, role, path, size, created_at)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(isrc)
        .bind(role)
        .bind(format!("x/{isrc}-{role}"))
        .bind(size)
        .bind(ts)
        .execute(&h.state.pool)
        .await
        .unwrap();
    }

    let stats = archive::storage_stats(&h.state.pool).await.unwrap();
    assert_eq!(stats.total_files, 3);
    assert_eq!(stats.total_bytes, 1400);
    let get = |r: &str| {
        stats
            .roles
            .iter()
            .find(|s| s.role == r)
            .unwrap_or_else(|| panic!("role {r} missing"))
            .clone()
    };
    assert_eq!(get(archive::ROLE_128).bytes, 100);
    assert_eq!(get(archive::ROLE_320).files, 1);
    assert_eq!(get(archive::ROLE_STEM).files, 0);
}