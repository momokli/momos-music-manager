//! #702 — `archive-legacy`: relocate proven duplicates under `_archive/`.
//!
//! Real audio is generated with ffmpeg (skipped with a notice when ffmpeg is
//! unavailable). Nothing is ever deleted or copied — only renamed.

mod common;

use common::*;
use music_api::{archive, db};
use std::path::Path;

fn write_file(path: &Path, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn hash_n(n: u32) -> String {
    format!("{:064x}", n)
}

/// A short real FLAC. `false` when ffmpeg is unavailable.
fn make_flac(path: &Path) -> bool {
    make_audio(&["-c:a", "flac"], path)
}

/// A short real MP3. `false` when ffmpeg is unavailable.
fn make_mp3(path: &Path) -> bool {
    make_audio(&["-c:a", "libmp3lame", "-b:a", "128k"], path)
}

/// A short real AAC/m4a bundle. `false` when ffmpeg is unavailable.
fn make_m4a(path: &Path) -> bool {
    make_audio(&["-c:a", "aac", "-f", "mp4"], path)
}

fn make_audio(codec: &[&str], path: &Path) -> bool {
    make_audio_freq("440", codec, path)
}

fn make_audio_freq(freq: &str, codec: &[&str], path: &Path) -> bool {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let mut cmd = std::process::Command::new("ffmpeg");
    cmd.args(["-y", "-f", "lavfi", "-i", &format!("sine=frequency={freq}:duration=0.5")])
        .args(codec)
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

async fn seed_track(pool: &sqlx::SqlitePool, isrc: &str, flac: Option<&str>, p128: Option<&str>) {
    let ts = db::now();
    sqlx::query(
        "INSERT INTO tracks
            (isrc, source, state, path_flac, path_128, created_at, updated_at, wanted_formats)
         VALUES (?, 'deezer', 'ready', ?, ?, ?, ?, 'flac,128')",
    )
    .bind(isrc)
    .bind(flac)
    .bind(p128)
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
    content_type: &str,
    original_path: &str,
    isrc: &str,
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

async fn seed_track_file(pool: &sqlx::SqlitePool, isrc: &str, role: &str, path: &str) {
    sqlx::query(
        "INSERT INTO track_files (isrc, role, path, size, created_at)
         VALUES (?, ?, ?, 0, ?)",
    )
    .bind(isrc)
    .bind(role)
    .bind(path)
    .bind(db::now())
    .execute(pool)
    .await
    .unwrap();
}

fn count_archive_files(root: &Path) -> usize {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .count()
}

/// Class A candidate: a store-object FLAC that is a byte-for-byte copy of the
/// delivery FLAC (so PCM MD5 matches without metadata games).
async fn setup_flac_copy(h: &Harness) -> (std::path::PathBuf, std::path::PathBuf, String) {
    let cfg = &h.state.config;
    let delivery = cfg.flac_dir().join("ARCHA1.flac");
    assert!(make_flac(&delivery), "ffmpeg required");
    let hash = hash_n(0xA1);
    let candidate = h.state.store.path_for(&hash);
    std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
    std::fs::copy(&delivery, &candidate).unwrap();
    seed_object(
        &h.state.pool,
        &hash,
        std::fs::metadata(&candidate).unwrap().len() as i64,
        "audio/flac",
        "legacy/ARCHA1.flac",
        "ARCHA1",
        "ARCHA1.flac",
    )
    .await;
    seed_track(
        &h.state.pool,
        "ARCHA1",
        Some(&delivery.display().to_string()),
        None,
    )
    .await;
    (delivery, candidate, hash)
}

#[tokio::test]
async fn dry_run_moves_nothing() {
    let h = harness().await;
    if !make_flac(&h.state.config.data_dir.join("probe.flac")) {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    let (delivery, candidate, _hash) = setup_flac_copy(&h).await;

    let report = archive::archive_legacy(&h.state.pool, &h.state.config, &h.state.store, false)
        .await
        .unwrap();
    assert_eq!(report.mode, "dry-run");
    assert!(!report.aborted);
    assert_eq!(report.classes.flac_copies.count, 1, "{report:?}");
    assert_eq!(report.classes.flac_copies.moved, 1, "{report:?}");

    assert!(delivery.exists());
    assert!(candidate.exists(), "dry-run must not move the source");
    assert!(!h.state.config.data_dir.join("_archive").exists());
    let archived: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM track_files WHERE role='archived'")
        .fetch_one(&h.state.pool)
        .await
        .unwrap();
    assert_eq!(archived, 0);
}

#[tokio::test]
async fn class_a_moves_flac_copy_and_is_idempotent() {
    let h = harness().await;
    let (delivery, candidate, _hash) = setup_flac_copy(&h).await;

    let first = archive::archive_legacy(&h.state.pool, &h.state.config, &h.state.store, true)
        .await
        .unwrap();
    assert_eq!(first.total_moved, 1, "{first:?}");
    let archived_path = h.state.config.data_dir.join("_archive/flac-copies/ARCHA1.flac");
    assert!(archived_path.exists(), "copy relocated to _archive/flac-copies");
    assert!(delivery.exists(), "delivery stays put");
    assert!(!candidate.exists(), "source only renamed");

    let row: (String, String) = sqlx::query_as(
        "SELECT role, path FROM track_files WHERE role='archived' AND isrc='ARCHA1'",
    )
    .fetch_one(&h.state.pool)
    .await
    .unwrap();
    assert_eq!(row.0, "archived");
    assert_eq!(row.1, "_archive/flac-copies/ARCHA1.flac");

    // Second apply: nothing left to move.
    let second = archive::archive_legacy(&h.state.pool, &h.state.config, &h.state.store, true)
        .await
        .unwrap();
    assert_eq!(second.total_moved, 0, "{second:?}");
    assert_eq!(second.classes.flac_copies.moved, 0);
}

#[tokio::test]
async fn class_b_duplicate_bundle_with_named_twin() {
    let h = harness().await;
    let cfg = &h.state.config;
    let twin = cfg.data_dir.join("stems/ARCHB1.stem.m4a");
    assert!(make_m4a(&twin), "ffmpeg required");
    seed_track_file(&h.state.pool, "ARCHB1", archive::ROLE_STEM, &twin.display().to_string()).await;

    let hash = hash_n(0xB1);
    let candidate = h.state.store.path_for(&hash);
    std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
    std::fs::copy(&twin, &candidate).unwrap();
    seed_object(
        &h.state.pool,
        &hash,
        std::fs::metadata(&candidate).unwrap().len() as i64,
        "audio/mp4",
        "anon/song.stem.m4a",
        "ARCHB1",
        "song.stem.m4a",
    )
    .await;

    let report = archive::archive_legacy(&h.state.pool, &cfg, &h.state.store, true)
        .await
        .unwrap();
    assert!(!report.aborted, "{report:?}");
    assert_eq!(report.classes.duplicate_bundles.moved, 1, "{report:?}");
    assert!(cfg.data_dir.join("_archive/duplicate-bundles").join(&hash).exists());
    assert!(twin.exists(), "named twin untouched");
    assert!(!candidate.exists());
}

#[tokio::test]
async fn class_c_relocates_orphan_bundle() {
    let h = harness().await;
    let cfg = &h.state.config;
    let hash = hash_n(0xC1);
    let candidate = h.state.store.path_for(&hash);
    assert!(make_m4a(&candidate), "ffmpeg required");
    seed_object(
        &h.state.pool,
        &hash,
        std::fs::metadata(&candidate).unwrap().len() as i64,
        "audio/mp4",
        "anon/orphan.stem.m4a",
        "ARCHC1",
        "orphan.stem.m4a",
    )
    .await;

    let report = archive::archive_legacy(&h.state.pool, &cfg, &h.state.store, true)
        .await
        .unwrap();
    assert!(!report.aborted, "{report:?}");
    assert_eq!(report.classes.relocated_bundles.moved, 1, "{report:?}");
    let canonical = cfg.data_dir.join("tracks/ARCHC1/ARCHC1.stem.m4a");
    assert!(canonical.exists(), "orphan moved into its track folder");
    assert!(!candidate.exists());
}

#[tokio::test]
async fn class_d_mp3_identical_moves_not_identical_stays() {
    let h = harness().await;
    let cfg = &h.state.config;
    let reference = cfg.mp3_128_dir().join("ARCHD1.mp3");
    assert!(make_mp3(&reference), "ffmpeg required");
    seed_track(&h.state.pool, "ARCHD1", None, Some(&reference.display().to_string())).await;

    // Identical copy (byte-for-byte) → relocated.
    let h_dup = hash_n(0xD1);
    let dup = h.state.store.path_for(&h_dup);
    std::fs::create_dir_all(dup.parent().unwrap()).unwrap();
    std::fs::copy(&reference, &dup).unwrap();
    seed_object(
        &h.state.pool,
        &h_dup,
        std::fs::metadata(&dup).unwrap().len() as i64,
        "audio/mpeg",
        "legacy/ARCHD1-copy.mp3",
        "ARCHD1",
        "ARCHD1-copy.mp3",
    )
    .await;

    // Different audio → stays + reported.
    let h_other = hash_n(0xD2);
    let other = h.state.store.path_for(&h_other);
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    // ffmpeg cannot infer a muxer for the extensionless store path, so encode
    // elsewhere and copy the bytes in.
    let tmp_other = cfg.data_dir.join("other-tmp.mp3");
    assert!(
        make_audio_freq("880", &["-c:a", "libmp3lame", "-b:a", "128k"], &tmp_other),
        "ffmpeg must produce the differing MP3"
    );
    std::fs::create_dir_all(other.parent().unwrap()).unwrap();
    std::fs::copy(&tmp_other, &other).unwrap();
    seed_object(
        &h.state.pool,
        &h_other,
        std::fs::metadata(&other).unwrap().len() as i64,
        "audio/mpeg",
        "legacy/ARCHD1-other.mp3",
        "ARCHD1",
        "ARCHD1-other.mp3",
    )
    .await;

    let report = archive::archive_legacy(&h.state.pool, &cfg, &h.state.store, true)
        .await
        .unwrap();
    assert!(!report.aborted, "{report:?}");
    assert_eq!(report.classes.mp3_copies.moved, 1, "{report:?}");
    assert!(
        cfg.data_dir.join("_archive/mp3-copies/ARCHD1-copy.mp3").exists(),
        "identical MP3 relocated"
    );
    assert!(reference.exists(), "128 reference untouched");
    assert!(other.exists(), "non-identical MP3 stays");
    assert_eq!(report.classes.mp3_copies.not_identical.len(), 1, "{report:?}");
}

#[tokio::test]
async fn aborts_when_target_is_occupied() {
    let h = harness().await;
    let (delivery, candidate, _hash) = setup_flac_copy(&h).await;
    let occupied = h.state.config.data_dir.join("_archive/flac-copies/ARCHA1.flac");
    write_file(&occupied, b"already here");

    let report = archive::archive_legacy(&h.state.pool, &h.state.config, &h.state.store, true)
        .await
        .unwrap();
    assert!(report.aborted, "{report:?}");
    assert!(report.aborted_reason.is_some());
    assert_eq!(report.total_moved, 0);
    assert!(delivery.exists());
    assert!(candidate.exists(), "nothing moved on abort");
}

#[tokio::test]
async fn archived_role_visible_in_storage_stats() {
    let h = harness().await;
    let (_delivery, _candidate, _hash) = setup_flac_copy(&h).await;
    archive::archive_legacy(&h.state.pool, &h.state.config, &h.state.store, true)
        .await
        .unwrap();

    let stats = archive::storage_stats(&h.state.pool).await.unwrap();
    let archived = stats
        .roles
        .iter()
        .find(|r| r.role == archive::ROLE_ARCHIVED)
        .expect("archived role present");
    assert_eq!(archived.files, 1);
}

#[tokio::test]
async fn nothing_is_deleted() {
    let h = harness().await;
    setup_flac_copy(&h).await;
    let report = archive::archive_legacy(&h.state.pool, &h.state.config, &h.state.store, true)
        .await
        .unwrap();
    let count = count_archive_files(&h.state.config.data_dir.join("_archive"));
    assert_eq!(count, report.total_moved);
}

/// Blocker #702: a candidate taken from `track_files.role='flac'` must have its
/// origin row rewritten **in place** to `archived`, never duplicated — otherwise
/// the old `flac` row would dangle on the relocated path and `storage_stats`
/// (purely row-based) would count the file twice.
#[tokio::test]
async fn class_a_track_file_candidate_rewrites_origin_row_in_place() {
    let h = harness().await;
    let cfg = &h.state.config;
    let delivery = cfg.flac_dir().join("ARCHI1.flac");
    if !make_flac(&delivery) {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    let legacy = cfg.data_dir.join("downloads/ARCHI1.flac");
    std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
    std::fs::copy(&delivery, &legacy).unwrap();
    seed_track(
        &h.state.pool,
        "ARCHI1",
        Some(&delivery.display().to_string()),
        None,
    )
    .await;
    seed_track_file(
        &h.state.pool,
        "ARCHI1",
        archive::ROLE_FLAC,
        &legacy.display().to_string(),
    )
    .await;

    let id_before: i64 =
        sqlx::query_scalar("SELECT id FROM track_files WHERE isrc='ARCHI1' AND role='flac'")
            .fetch_one(&h.state.pool)
            .await
            .unwrap();

    let report = archive::archive_legacy(&h.state.pool, cfg, &h.state.store, true)
        .await
        .unwrap();
    assert!(!report.aborted, "{report:?}");
    assert_eq!(report.classes.flac_copies.moved, 1, "{report:?}");

    // Exactly one row remains, with the original id, now `archived`.
    let rows: Vec<(i64, String, String)> =
        sqlx::query_as("SELECT id, role, path FROM track_files WHERE isrc='ARCHI1'")
            .fetch_all(&h.state.pool)
            .await
            .unwrap();
    assert_eq!(rows.len(), 1, "no duplicate inserted: {rows:?}");
    assert_eq!(rows[0].0, id_before, "row rewritten in place");
    assert_eq!(rows[0].1, "archived");
    assert_eq!(rows[0].2, "_archive/flac-copies/ARCHI1.flac");

    // No row points at the relocated-away path any longer.
    let dangling: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM track_files WHERE path = ?")
        .bind(legacy.display().to_string())
        .fetch_one(&h.state.pool)
        .await
        .unwrap();
    assert_eq!(dangling, 0, "no dangling rows at the removed path");

    // Row-based stats count the single file exactly once.
    let stats = archive::storage_stats(&h.state.pool).await.unwrap();
    let archived = stats
        .roles
        .iter()
        .find(|r| r.role == archive::ROLE_ARCHIVED)
        .expect("archived role present");
    assert_eq!(archived.files, 1);
    assert_eq!(stats.total_files, 1, "no double count: {stats:?}");
}

/// Two planned moves that resolve to the same destination must abort the whole
/// run before anything is touched.
#[tokio::test]
async fn aborts_when_two_moves_share_a_target() {
    let h = harness().await;
    let cfg = &h.state.config;
    let delivery = cfg.flac_dir().join("ARCHT1.flac");
    if !make_flac(&delivery) {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    seed_track(
        &h.state.pool,
        "ARCHT1",
        Some(&delivery.display().to_string()),
        None,
    )
    .await;

    // Two distinct copies of the same delivery → both plan `_archive/flac-copies/ARCHT1.flac`.
    let mut candidates = Vec::new();
    for (hash, name) in [(hash_n(0xE1), "ARCHT1-a.flac"), (hash_n(0xE2), "ARCHT1-b.flac")] {
        let cand = h.state.store.path_for(&hash);
        std::fs::create_dir_all(cand.parent().unwrap()).unwrap();
        std::fs::copy(&delivery, &cand).unwrap();
        seed_object(
            &h.state.pool,
            &hash,
            std::fs::metadata(&cand).unwrap().len() as i64,
            "audio/flac",
            &format!("legacy/{name}"),
            "ARCHT1",
            name,
        )
        .await;
        candidates.push(cand);
    }

    let report = archive::archive_legacy(&h.state.pool, cfg, &h.state.store, true)
        .await
        .unwrap();
    assert!(report.aborted, "{report:?}");
    assert_eq!(report.total_moved, 0, "nothing moved on abort");
    let reason = report.aborted_reason.as_deref().unwrap_or("");
    assert!(
        reason.contains("two moves"),
        "reason should name the duplicate target: {reason:?}"
    );
    for cand in &candidates {
        assert!(cand.exists(), "source untouched on abort");
    }
    assert!(
        !cfg.data_dir.join("_archive").exists(),
        "no archive tree created on abort"
    );
}
