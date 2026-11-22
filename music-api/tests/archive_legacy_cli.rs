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

const ISRC: &str = "DECLI0000007";

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

/// A short real MP3. `false` when ffmpeg (libmp3lame) is unavailable.
fn make_mp3(path: &Path) -> bool {
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
            "libmp3lame",
        ])
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

async fn seed_track_128(pool: &sqlx::SqlitePool, isrc: &str, mp3_128: &str) {
    let ts = db::now();
    sqlx::query(
        "INSERT INTO tracks
            (isrc, source, state, path_128, created_at, updated_at, wanted_formats)
         VALUES (?, 'deezer', 'ready', ?, ?, ?, 'mp3_128')",
    )
    .bind(isrc)
    .bind(mp3_128)
    .bind(ts)
    .bind(ts)
    .execute(pool)
    .await
    .unwrap();
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
        ISRC,
        "CLI702.flac",
    )
    .await;
    seed_track(&h.state.pool, ISRC, &delivery.display().to_string()).await;
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
    // Dry-run reports the plan under `planned`; `moved` stays 0.
    assert_eq!(flac["planned"], 1, "{report}");
    assert_eq!(flac["moved"], 0, "{report}");
    assert_eq!(report["totalPlanned"], 1, "{report}");
    assert_eq!(report["totalMoved"], 0, "{report}");
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

    assert!(!candidate.exists(), "original path is empty after rename");
    assert!(delivery.exists(), "delivery FLAC must stay put");

    let (role, path): (String, String) =
        sqlx::query_as("SELECT role, path FROM track_files WHERE isrc=? AND role='archived'")
            .bind(ISRC)
            .fetch_one(&h.state.pool)
            .await
            .unwrap();
    assert_eq!(role, "archived");
    assert!(
        path.starts_with("_archive/flac-copies/DECLI0000007-") && path.ends_with(".flac"),
        "unexpected archived path: {path}"
    );
    let archived_path = data_dir.join(&path);
    assert!(archived_path.exists(), "file relocated under _archive/");

    // Nothing deleted: file count under _archive == totalMoved.
    assert_eq!(
        count_files(&data_dir.join("_archive")),
        total_moved as usize,
        "archive file count must equal totalMoved"
    );

    // Blocker 3: the moved object's `store_objects` row is gone.
    let store_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM store_objects")
        .fetch_one(&h.state.pool)
        .await
        .unwrap();
    assert_eq!(store_rows, 0, "store_objects row removed after the move");

    // ---- 3. second --apply is a no-op. ----
    let (code, stdout, stderr) = run_cli(&data_dir, &store_root, "--apply");
    assert_eq!(code, 0, "second apply exit code; stderr={stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("second apply report must be JSON: {e}\nstdout={stdout}"));
    assert_eq!(
        report["totalMoved"], 0,
        "second apply moves nothing: {report}"
    );
    assert_eq!(report["classes"]["flacCopies"]["moved"], 0, "{report}");
    assert!(archived_path.exists(), "relocated file still present");
    assert_eq!(
        count_files(&data_dir.join("_archive")),
        1,
        "no extra archive files on the no-op run"
    );
}

/// Scenario 3: two PCM-identical FLAC copies of the same ISRC must both be
/// relocated under unique names — no global abort — and exit 0.
#[tokio::test]
async fn cli_two_identical_flac_copies_same_isrc_both_moved() {
    let h = harness().await;
    let cfg = &h.state.config;
    let delivery = cfg.flac_dir().join("DUP702.flac");
    if !make_flac(&delivery) {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    // Two byte-identical legacy copies under two distinct object hashes.
    for n in [0xA1u32, 0xB2u32] {
        let cand = h.state.store.path_for(&hash_n(n));
        std::fs::create_dir_all(cand.parent().unwrap()).unwrap();
        std::fs::copy(&delivery, &cand).unwrap();
        seed_object(
            &h.state.pool,
            &hash_n(n),
            std::fs::metadata(&cand).unwrap().len() as i64,
            "legacy/DUP702.flac",
            ISRC,
            "DUP702.flac",
        )
        .await;
    }
    seed_track(&h.state.pool, ISRC, &delivery.display().to_string()).await;

    let data_dir = cfg.data_dir.clone();
    let store_root = cfg.store_root.clone();

    let (code, stdout, stderr) = run_cli(&data_dir, &store_root, "--apply");
    assert_eq!(
        code, 0,
        "apply must not abort on identical ISRC copies; stderr={stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("report must be JSON: {e}\nstdout={stdout}"));
    assert_eq!(report["aborted"], false, "{report}");
    assert_eq!(report["classes"]["flacCopies"]["planned"], 2, "{report}");
    assert_eq!(report["classes"]["flacCopies"]["moved"], 2, "{report}");
    assert_eq!(report["totalMoved"], 2, "{report}");

    // Exactly two archived files, with distinct destination names.
    let files: Vec<String> = walkdir::WalkDir::new(data_dir.join("_archive"))
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files.len(), 2, "two archived files expected: {files:?}");
    let unique: std::collections::HashSet<&String> = files.iter().collect();
    assert_eq!(
        unique.len(),
        2,
        "destination names must be unique: {files:?}"
    );
}

/// Scenario 4: a genuine `rename` failure during `--apply` flips `aborted`
/// and the process must exit non-zero (2).
#[tokio::test]
async fn cli_apply_aborts_on_rename_failure() {
    use std::os::unix::fs::PermissionsExt;

    let h = harness().await;
    let Some((_delivery, candidate)) = setup(&h).await else {
        return;
    };
    let data_dir = h.state.config.data_dir.clone();
    let store_root = h.state.config.store_root.clone();

    // Pre-create the target directory read-only: `create_dir_all(parent)` still
    // succeeds, but `rename` into it fails with EACCES — a real I/O abort.
    let target_dir = data_dir.join("_archive/flac-copies");
    std::fs::create_dir_all(&target_dir).unwrap();
    std::fs::set_permissions(&target_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    let (code, stdout, stderr) = run_cli(&data_dir, &store_root, "--apply");

    // Restore write permission so the tempdir can be cleaned up.
    let _ = std::fs::set_permissions(&target_dir, std::fs::Permissions::from_mode(0o755));

    assert_eq!(
        code, 2,
        "aborted apply must exit with code 2; code={code} stderr={stderr}"
    );
    let report: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("aborted report must be JSON: {e}\nstdout={stdout}"));
    assert_eq!(report["aborted"], true, "{report}");
    assert!(
        report["abortedReason"].is_string(),
        "abortedReason required: {report}"
    );
    assert_eq!(report["totalMoved"], 0, "nothing moved on abort: {report}");
    assert!(candidate.exists(), "source must not be consumed on abort");
}

/// Scenario 5: hostile `store_objects` values (`isrc="../../escape"`,
/// `name="../x.mp3"`) must never place a file outside `data_dir`.
#[tokio::test]
async fn cli_path_traversal_isrc_and_name_are_defused() {
    let h = harness().await;
    let cfg = &h.state.config;

    // (a) Hostile ISRC on a FLAC copy → skipped as `invalid_isrc`.
    let evil = h.state.store.path_for(&hash_n(0xE1));
    std::fs::create_dir_all(evil.parent().unwrap()).unwrap();
    if !make_flac(&evil) {
        eprintln!("skipping: ffmpeg not available");
        return;
    }
    seed_object(
        &h.state.pool,
        &hash_n(0xE1),
        std::fs::metadata(&evil).unwrap().len() as i64,
        "legacy/evil.flac",
        "../../escape",
        "evil.flac",
    )
    .await;

    // (b) Valid ISRC but hostile `name` on an MP3 copy identical to the 128
    // reference → must land inside `data_dir` under a sanitised basename.
    let ref_mp3 = cfg.mp3_128_dir().join("REF702.mp3");
    if !make_mp3(&ref_mp3) {
        eprintln!("skipping: ffmpeg mp3 encoder not available");
        return;
    }
    let cand = h.state.store.path_for(&hash_n(0xE2));
    std::fs::create_dir_all(cand.parent().unwrap()).unwrap();
    std::fs::copy(&ref_mp3, &cand).unwrap();
    seed_object(
        &h.state.pool,
        &hash_n(0xE2),
        std::fs::metadata(&cand).unwrap().len() as i64,
        "legacy/x.mp3",
        ISRC,
        "../x.mp3",
    )
    .await;
    seed_track_128(&h.state.pool, ISRC, &ref_mp3.display().to_string()).await;

    let data_dir = cfg.data_dir.clone();
    let store_root = cfg.store_root.clone();

    let (code, stdout, stderr) = run_cli(&data_dir, &store_root, "--apply");
    assert_eq!(code, 0, "apply exit code; stderr={stderr}");
    let report: serde_json::Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("report must be JSON: {e}\nstdout={stdout}"));

    // Hostile ISRC object skipped, never planned.
    let skipped: Vec<serde_json::Value> = report["skipped"].as_array().cloned().unwrap_or_default();
    assert!(
        skipped.iter().any(|s| s["reason"] == "invalid_isrc"),
        "hostile ISRC must be skipped as invalid_isrc: {report}"
    );
    // Only the (name-hostile) mp3 copy moved.
    assert_eq!(report["totalMoved"], 1, "{report}");

    // The relocated file sits inside data_dir with a sanitised basename.
    let dst = data_dir
        .join("_archive/mp3-copies")
        .join(ISRC)
        .join("x.mp3");
    assert!(dst.exists(), "expected sanitised destination: {dst:?}");
    assert!(
        dst.starts_with(&data_dir),
        "destination must stay inside data_dir"
    );

    // Nothing escaped the data dir (no `../x.mp3`, no `../../escape`).
    let outside_parent = data_dir.parent().unwrap();
    assert!(
        !outside_parent.join("x.mp3").exists(),
        "file escaped data_dir via the name"
    );
    assert!(
        !outside_parent.join("escape").exists(),
        "directory escaped data_dir via the ISRC"
    );
    assert!(
        !data_dir.join("_archive/x.mp3").exists(),
        "name traversal leaked"
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
