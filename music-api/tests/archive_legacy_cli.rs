//! #702 — end-to-end coverage of the **built binary**.
//!
//! These tests invoke `CARGO_BIN_EXE_music-api archive-legacy …` as a real
//! subprocess against a temporary `DATA_DIR` seeded with real ffmpeg fixtures.
//! They complement `archive_legacy.rs`, which calls the library function
//! directly and therefore never exercises `main.rs`/`Config::from_env`.

mod common;

use common::*;
use music_api::db;
use std::path::{Path, PathBuf};
use std::process::Command;

fn hash_n(n: u32) -> String {
    format!("{:064x}", n)
}

/// A short real FLAC. `false` when ffmpeg is unavailable.
fn make_flac(path: &Path) -> bool {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=0.5",
            "-c:a",
            "flac",
        ])
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn seed_track(pool: &sqlx::SqlitePool, isrc: &str, flac: &str) {
    let ts = db::now();
    sqlx::query(
        "INSERT INTO tracks
            (isrc, source, state, path_flac, created_at, updated_at, wanted_formats)
         VALUES (?, 'deezer', 'ready', ?, ?, ?, 'flac')",
    )
    .bind(isrc)
    .bind(flac)
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
    original_path: &str,
    isrc: &str,
    name: &str,
) {
    sqlx::query(
        "INSERT INTO store_objects
            (hash, size, content_type, original_path, isrc, name, created_at)
         VALUES (?, ?, 'audio/flac', ?, ?, ?, ?)",
    )
    .bind(hash)
    .bind(size)
    .bind(original_path)
    .bind(isrc)
    .bind(name)
    .bind(db::now())
    .execute(pool)
    .await
    .unwrap();
}

fn count_files(root: &Path) -> usize {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .count()
}

/// Run the real binary with the harness's `DATA_DIR`/`STORE_ROOT`.
fn run_cli(data_dir: &Path, store_root: &Path, mode: &str) -> (i32, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_music-api"))
        .arg("archive-legacy")
        .arg(mode)
        .env("MUSIC_API_TOKEN", TOKEN)
        .env("DATA_DIR", data_dir)
        .env("STORE_ROOT", store_root)
        .env("FFMPEG", "ffmpeg")
        .env("RUST_LOG", "error")
        // Avoid sqlite temp-file stalls on slow/CI disks.
        .env("TMPDIR", "/dev/shm/mmm702")
        .output()
        .expect("failed to spawn music-api binary");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Delivery FLAC + a byte-identical copy registered as a legacy store object.
async fn setup(h: &Harness) -> Option<(PathBuf, PathBuf)> {
    let cfg = &h.state.config;
    let delivery = cfg.flac_dir().join("CLI702.flac");
    if !make_flac(&delivery) {
        eprintln!("skipping: ffmpeg not available");
        return None;
    }
    let candidate = h.state.store.path_for(&hash_n(0x77));
    std::fs::create_dir_all(candidate.parent().unwrap()).unwrap();
    std::fs::copy(&delivery, &candidate).unwrap();
    seed_object(
        &h.state.pool,
        &hash_n(0x77),
        std::fs::metadata(&candidate).unwrap().len() as i64,
        "legacy/CLI702.flac",
        "CLI702",
        "CLI702.flac",
    )
    .await;
    seed_track(&h.state.pool, "CLI702", &delivery.display().to_string()).await;
    Some((delivery, candidate))
}

#[tokio::test]
async fn cli_dry_run_apply_idempotent() {
    let h = harness().await;
    let Some((delivery, candidate)) = setup(&h).await else {
        return;
    };
    let data_dir = h.state.config.data_dir.clone();
    let store_root = h.state.config.store_root.clone();

    // ---- 1. --dry-run: report only, nothing moved, no archived row. ----
    let (code, stdout, stderr) = run_cli(&data_dir, &store_root, "--dry-run");
    assert_eq!(code, 0, "dry-run exit code; stderr={stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("dry-run report must be JSON: {e}\nstdout={stdout}"));
    assert_eq!(report["mode"], "dry-run");
    assert_eq!(report["aborted"], false);
    let flac = &report["classes"]["flacCopies"];
    assert_eq!(flac["count"], 1, "{report}");
    assert_eq!(flac["moved"], 1, "{report}");
    assert!(delivery.exists(), "delivery untouched");
    assert!(candidate.exists(), "dry-run must not move the candidate");
    assert!(
        !data_dir.join("_archive").exists(),
        "dry-run must not create the archive tree"
    );
    let archived: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM track_files WHERE role='archived'")
            .fetch_one(&h.state.pool)
            .await
            .unwrap();
    assert_eq!(archived, 0, "dry-run must not write an archived row");

    // ---- 2. --apply: real move, DB row, nothing deleted. ----
    let (code, stdout, stderr) = run_cli(&data_dir, &store_root, "--apply");
    assert_eq!(code, 0, "apply exit code; stderr={stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("apply report must be JSON: {e}\nstdout={stdout}"));
    assert_eq!(report["mode"], "apply");
    assert_eq!(report["aborted"], false);
    let total_moved = report["totalMoved"].as_u64().expect("totalMoved");
    assert_eq!(total_moved, 1, "{report}");

    let archived_path = data_dir.join("_archive/flac-copies/CLI702.flac");
    assert!(archived_path.exists(), "file relocated under _archive/");
    assert!(!candidate.exists(), "original path is empty after rename");
    assert!(delivery.exists(), "delivery FLAC must stay put");

    // Nothing deleted: file count under _archive == totalMoved.
    assert_eq!(
        count_files(&data_dir.join("_archive")),
        total_moved as usize,
        "archive file count must equal totalMoved"
    );

    let (role, path): (String, String) = sqlx::query_as(
        "SELECT role, path FROM track_files WHERE isrc='CLI702' AND role='archived'",
    )
    .fetch_one(&h.state.pool)
    .await
    .unwrap();
    assert_eq!(role, "archived");
    assert_eq!(path, "_archive/flac-copies/CLI702.flac");

    // ---- 3. second --apply is a no-op. ----
    let (code, stdout, stderr) = run_cli(&data_dir, &store_root, "--apply");
    assert_eq!(code, 0, "second apply exit code; stderr={stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("second apply report must be JSON: {e}\nstdout={stdout}"));
    assert_eq!(report["totalMoved"], 0, "second apply moves nothing: {report}");
    assert_eq!(report["classes"]["flacCopies"]["moved"], 0, "{report}");
    assert!(archived_path.exists(), "relocated file still present");
    assert_eq!(
        count_files(&data_dir.join("_archive")),
        1,
        "no extra archive files on the no-op run"
    );
}

/// The CLI must reject an ambiguous mode without --dry-run/--apply.
#[tokio::test]
async fn cli_requires_exactly_one_mode_flag() {
    let h = harness().await;
    let data_dir = h.state.config.data_dir.clone();
    let store_root = h.state.config.store_root.clone();

    let out = Command::new(env!("CARGO_BIN_EXE_music-api"))
        .arg("archive-legacy")
        .env("MUSIC_API_TOKEN", TOKEN)
        .env("DATA_DIR", &data_dir)
        .env("STORE_ROOT", &store_root)
        .env("RUST_LOG", "error")
        .env("TMPDIR", "/dev/shm/mmm702")
        .output()
        .expect("failed to spawn music-api binary");

    assert!(!out.status.success(), "missing mode flag must fail");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("usage:") || stderr.contains("archive-legacy"),
        "usage hint expected, got: {stderr}"
    );
}