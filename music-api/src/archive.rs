//! `track_files` — the per-file inventory behind `GET /storage/stats` (#700).
//!
//! One row per physical audio file: what it is (`role`), where it lives
//! (`path`, relative to `DATA_DIR` when inside it), how big it is and which
//! track it belongs to. The catalogue is filled by the `archive-backfill` CLI,
//! never on the request path: `/storage/stats` only aggregates rows.
//!
//! The backfill is **read-only on media files** and idempotent: a second
//! `--apply` run writes no row.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};
use sqlx::{Pool, Sqlite};

use crate::config::Config;
use crate::store::Store;

pub const ROLE_128: &str = "128";
pub const ROLE_320: &str = "320";
pub const ROLE_FLAC: &str = "flac";
pub const ROLE_STEM: &str = "stem";
pub const ROLE_STEM_PART: &str = "stem_part";
/// A file relocated under `_archive/` by the `archive-legacy` CLI (#702).
/// `path` points at the new location; the bytes are never deleted.
pub const ROLE_ARCHIVED: &str = "archived";

pub const ROLES: [&str; 6] = [
    ROLE_128,
    ROLE_320,
    ROLE_FLAC,
    ROLE_STEM,
    ROLE_STEM_PART,
    ROLE_ARCHIVED,
];

/// Stem part suffixes. Mirrors `bin/store_import.rs` (`_vocals`, `_bass`, …).
const STEM_PARTS: &[&str] = &["vocals", "bass", "drums", "instrumental", "other"];

/// Create the additive catalogue tables. Called from [`crate::db::init`] so every
/// fresh database — server and harness alike — has them.
pub async fn init(pool: &Pool<Sqlite>) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS track_files (
            id                INTEGER PRIMARY KEY,
            isrc              TEXT NOT NULL,
            role              TEXT NOT NULL,
            path              TEXT NOT NULL,
            size              INTEGER NOT NULL,
            sha256            TEXT,
            stem_part         TEXT,
            origin            TEXT,
            original_filename TEXT,
            created_at        TEXT NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;

    // The natural key: one row per file slot. Expressed as a unique *index*
    // because SQLite allows expressions in index definitions but not in table
    // constraints. `COALESCE(stem_part, '')` makes `NULL` a real key value.
    sqlx::query(
        "CREATE UNIQUE INDEX IF NOT EXISTS idx_track_files_key
           ON track_files(isrc, role, COALESCE(stem_part, ''));",
    )
    .execute(pool)
    .await?;

    // Files found under the managed roots that could not be attributed to a
    // track. Kept out of `track_files` (which is "one row per *known* file") so
    // `/storage/stats` can report drift without a filesystem scan.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS track_files_unmanaged (
            path   TEXT PRIMARY KEY,
            reason TEXT NOT NULL,
            size   INTEGER NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}

// ── stats ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleStat {
    pub role: String,
    pub files: i64,
    pub bytes: i64,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageStats {
    /// One entry per known role, always in [`ROLES`] order (absent roles are 0).
    pub roles: Vec<RoleStat>,
    pub total_files: i64,
    pub total_bytes: i64,
    /// Tracks with no `track_files` row at all.
    pub tracks_without_files: i64,
    /// Tracks whose demand set (`wanted_formats`) is not covered by rows.
    pub incomplete_tracks: i64,
    /// Files under the managed roots that the backfill could not attribute.
    pub unmanaged_files: i64,
    /// #699 compatibility: `path_*` present but `state <> 'ready'` (DB only).
    pub servable_but_not_ready: i64,
}

/// Aggregate the catalogue. No filesystem access: everything comes from
/// `track_files` / `track_files_unmanaged` (plus one cheap `tracks` count).
pub async fn storage_stats(pool: &Pool<Sqlite>) -> anyhow::Result<StorageStats> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT role, COUNT(*), COALESCE(SUM(size), 0)
           FROM track_files GROUP BY role",
    )
    .fetch_all(pool)
    .await?;

    let mut by_role: HashMap<String, (i64, i64)> = HashMap::new();
    for (role, files, bytes) in rows {
        by_role.insert(role, (files, bytes));
    }
    let mut roles = Vec::with_capacity(ROLES.len());
    let mut total_files = 0i64;
    let mut total_bytes = 0i64;
    for role in ROLES {
        let (files, bytes) = by_role.remove(role).unwrap_or((0, 0));
        total_files += files;
        total_bytes += bytes;
        roles.push(RoleStat {
            role: role.to_string(),
            files,
            bytes,
        });
    }
    // Any role the schema does not know about (future-proofing) is reported as
    // well instead of being silently dropped from the totals.
    for (role, (files, bytes)) in by_role {
        total_files += files;
        total_bytes += bytes;
        roles.push(RoleStat { role, files, bytes });
    }

    let tracks_without_files: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tracks t
          WHERE NOT EXISTS (SELECT 1 FROM track_files f WHERE f.isrc = t.isrc)",
    )
    .fetch_one(pool)
    .await?;

    // A track is incomplete when a demanded tier has no file row. `wanted_formats`
    // is comma-separated (`flac,320,128`); the tier names map 1:1 to roles.
    let incomplete_tracks: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tracks t
          WHERE EXISTS (SELECT 1 FROM track_files f WHERE f.isrc = t.isrc)
            AND (
              (t.wanted_formats LIKE '%flac%' AND NOT EXISTS
                 (SELECT 1 FROM track_files f WHERE f.isrc = t.isrc AND f.role = 'flac'))
              OR (t.wanted_formats LIKE '%320%' AND NOT EXISTS
                 (SELECT 1 FROM track_files f WHERE f.isrc = t.isrc AND f.role = '320'))
              OR (t.wanted_formats LIKE '%128%' AND NOT EXISTS
                 (SELECT 1 FROM track_files f WHERE f.isrc = t.isrc AND f.role = '128'))
            )",
    )
    .fetch_one(pool)
    .await?;

    let unmanaged_files: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM track_files_unmanaged")
            .fetch_one(pool)
            .await?;

    let servable_but_not_ready: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tracks
          WHERE state <> 'ready'
            AND (path_flac IS NOT NULL OR path_320 IS NOT NULL OR path_128 IS NOT NULL)",
    )
    .fetch_one(pool)
    .await?;

    Ok(StorageStats {
        roles,
        total_files,
        total_bytes,
        tracks_without_files,
        incomplete_tracks,
        unmanaged_files,
        servable_but_not_ready,
    })
}

// ── backfill ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnmanagedFile {
    pub path: String,
    pub reason: String,
    pub size: i64,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackfillReport {
    /// `dry-run` or `apply`.
    pub mode: String,
    pub candidates: usize,
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub deleted: usize,
    /// Candidates whose file is no longer on disk (reported, never inserted).
    pub missing_files: Vec<String>,
    /// Files under the managed roots with no attributable track.
    pub unmanaged: Vec<UnmanagedFile>,
    /// Bundles with an ISRC (from column or tag) vs. without.
    pub bundle_with_isrc: usize,
    pub bundle_without_isrc: Vec<String>,
    /// Stem parts with an ISRC (directly or inherited) vs. without.
    pub stem_with_isrc: usize,
    pub stem_without_isrc: Vec<String>,
}

#[derive(Debug, Clone)]
struct Candidate {
    isrc: String,
    role: &'static str,
    abs: PathBuf,
    stem_part: Option<String>,
    origin: String,
    original_filename: Option<String>,
}

impl Candidate {
    fn key(&self) -> String {
        format!(
            "{}\u{1}{}\u{1}{}",
            self.isrc,
            self.role,
            self.stem_part.as_deref().unwrap_or("")
        )
    }
}

/// ISO-8601 UTC (`2026-11-22T02:44:00Z`) from an epoch second, without a date
/// dependency.
pub fn iso8601(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let secs = epoch.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

fn now_iso() -> String {
    iso8601(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
    )
}

fn ext_of(p: &str) -> Option<String> {
    let base = p.rsplit('/').next().unwrap_or(p);
    let (_, e) = base.rsplit_once('.')?;
    Some(e.to_ascii_lowercase())
}


/// Relative-to-`data_dir` path when inside it, absolute otherwise.
fn display_path(abs: &Path, data_dir: &Path) -> String {
    match abs.strip_prefix(data_dir) {
        Ok(rel) => rel.to_string_lossy().replace('\\', "/"),
        Err(_) => abs.to_string_lossy().replace('\\', "/"),
    }
}

fn resolve_abs(p: &str, data_dir: &Path) -> PathBuf {
    let path = Path::new(p);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        data_dir.join(path)
    }
}

/// `Song_vocals.wav` → `("Song", Some("vocals"))`; `Song.stem.m4a` → `("Song", None)`.
fn split_stem_name(file: &str) -> (String, Option<String>) {
    let lower = file.to_ascii_lowercase();
    for part in STEM_PARTS {
        let needle = format!("_{part}");
        if let Some(idx) = lower.find(&needle) {
            let base = &file[..idx];
            return (base.to_string(), Some((*part).to_string()));
        }
    }
    let bundle = lower
        .strip_suffix(".stem.m4a")
        .map(|_| file[..file.len() - ".stem.m4a".len()].to_string());
    if let Some(base) = bundle {
        return (base, None);
    }
    let base = Path::new(file)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(file)
        .to_string();
    (base, None)
}

fn dir_of(original_path: Option<&str>) -> String {
    original_path
        .and_then(|p| p.rsplit_once('/'))
        .map(|(d, _)| d.to_string())
        .unwrap_or_default()
}

/// MP3 bitrate → tier. Anything ≥ 200 kbit/s is `320`, below is `128`.
/// A file ffprobe cannot read falls back to `320` (the store's default copy).
async fn mp3_role(abs: &Path, ffprobe: &str) -> &'static str {
    let out = tokio::process::Command::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=bit_rate",
            "-of",
            "default=nw=1:nk=1",
        ])
        .arg(abs)
        .output()
        .await;
    if let Ok(out) = out {
        if out.status.success() {
            let text = String::from_utf8_lossy(&out.stdout);
            if let Ok(bps) = text.trim().parse::<u64>() {
                return if bps >= 200_000 {
                    ROLE_320
                } else {
                    ROLE_128
                };
            }
        }
    }
    ROLE_320
}

#[derive(sqlx::FromRow)]
struct ObjectRow {
    hash: String,
    size: i64,
    content_type: Option<String>,
    original_path: Option<String>,
    isrc: Option<String>,
    name: Option<String>,
    stem_type: Option<String>,
}

impl ObjectRow {
    fn file_name(&self) -> String {
        self.name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .or_else(|| {
                self.original_path
                    .as_deref()
                    .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
            })
            .unwrap_or_else(|| self.hash.clone())
    }

    fn ext(&self) -> Option<String> {
        ext_of(&self.file_name()).or_else(|| {
            self.original_path
                .as_deref()
                .and_then(ext_of)
        })
    }

    fn is_bundle(&self) -> bool {
        let ct = self.content_type.as_deref().unwrap_or("");
        let ext = self.ext().unwrap_or_default();
        ct == "audio/mp4" || ext == "m4a" || ext == "alac"
    }
}

/// Run the `archive-backfill` command.
///
/// `apply == false` computes the same plan but performs no DB write.
pub async fn backfill(
    pool: &Pool<Sqlite>,
    config: &Config,
    store: &Store,
    apply: bool,
) -> anyhow::Result<BackfillReport> {
    let data_dir = config.data_dir.clone();
    let mut report = BackfillReport {
        mode: if apply { "apply" } else { "dry-run" }.to_string(),
        ..Default::default()
    };

    // 1. Existing rows keyed like `Candidate::key`.
    let existing: Vec<(String, String, Option<String>, String, i64)> = sqlx::query_as(
        "SELECT isrc, role, stem_part, path, size FROM track_files",
    )
    .fetch_all(pool)
    .await?;
    let mut existing_map: HashMap<String, (String, i64)> = HashMap::new();
    for (isrc, role, stem_part, path, size) in existing {
        existing_map.insert(
            format!("{}\u{1}{}\u{1}{}", isrc, role, stem_part.unwrap_or_default()),
            (path, size),
        );
    }

    let mut candidates: BTreeMap<String, Candidate> = BTreeMap::new();
    let mut missing_files: Vec<String> = Vec::new();

    // 2. Output formats from `tracks.path_*`.
    let track_rows: Vec<(String, Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT isrc, path_flac, path_320, path_128 FROM tracks")
            .fetch_all(pool)
            .await?;
    for (isrc, flac, p320, p128) in track_rows {
        for (role, raw) in [
            (ROLE_FLAC, flac),
            (ROLE_320, p320),
            (ROLE_128, p128),
        ] {
            let Some(raw) = raw.filter(|p| !p.trim().is_empty()) else {
                continue;
            };
            let abs = resolve_abs(&raw, &data_dir);
            if !abs.is_file() {
                missing_files.push(display_path(&abs, &data_dir));
                continue;
            }
            let c = Candidate {
                isrc: isrc.clone(),
                role,
                abs,
                stem_part: None,
                origin: format!("tracks.path_{role}"),
                original_filename: None,
            };
            candidates.insert(c.key(), c);
        }
    }

    // 3. Object store.
    let objects: Vec<ObjectRow> = sqlx::query_as(
        "SELECT hash, size, content_type, original_path, isrc, name, stem_type
           FROM store_objects",
    )
    .fetch_all(pool)
    .await?;

    // Bundles first: their ISRC is the authority stem parts inherit from.
    // `(folder, base_lower)` plus a per-folder fallback.
    let mut bundle_by_base: HashMap<(String, String), String> = HashMap::new();
    let mut bundles_by_dir: HashMap<String, Vec<String>> = HashMap::new();
    let mut bundle_isrc: HashMap<String, Option<String>> = HashMap::new();

    for o in &objects {
        if !o.is_bundle() {
            continue;
        }
        let abs = store.path_for(&o.hash);
        let name = o.file_name();
        let (base, _) = split_stem_name(&name);
        let isrc = o
            .isrc
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| crate::tags::read_isrc(&abs));
        bundle_isrc.insert(o.hash.clone(), isrc.clone());
        if let Some(isrc) = isrc {
            bundle_by_base.insert((dir_of(o.original_path.as_deref()), base.to_lowercase()), isrc.clone());
            bundles_by_dir
                .entry(dir_of(o.original_path.as_deref()))
                .or_default()
                .push(isrc);
        }
    }

    for o in &objects {
        let abs = store.path_for(&o.hash);
        if !abs.is_file() {
            missing_files.push(display_path(&abs, &data_dir));
            continue;
        }
        let name = o.file_name();
        let ext = o.ext().unwrap_or_default();
        let ct = o.content_type.as_deref().unwrap_or("");

        let (role, stem_part): (&'static str, Option<String>) = if o.is_bundle() {
            (ROLE_STEM, None)
        } else if ext == "wav" || ext == "aiff" || ext == "aif" || ct.contains("wav") {
            let (_base, part) = split_stem_name(&name);
            let part = o
                .stem_type
                .clone()
                .filter(|s| !s.trim().is_empty())
                .or(part)
                .unwrap_or_else(|| "other".to_string());
            (ROLE_STEM_PART, Some(part))
        } else if ext == "flac" || ext == "alac" {
            (ROLE_FLAC, None)
        } else if ext == "mp3" {
            (mp3_role(&abs, &config.ffprobe).await, None)
        } else {
            report.unmanaged.push(UnmanagedFile {
                path: display_path(&abs, &data_dir),
                reason: format!("unclassified object ({name})"),
                size: o.size,
            });
            continue;
        };

        let mut isrc = o
            .isrc
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| crate::tags::read_isrc(&abs));

        if isrc.is_none() && role == ROLE_STEM_PART {
            let (base, _) = split_stem_name(&name);
            let dir = dir_of(o.original_path.as_deref());
            isrc = bundle_by_base
                .get(&(dir.clone(), base.to_lowercase()))
                .cloned()
                .or_else(|| {
                    // Fallback: exactly one bundle in the same original folder.
                    let v = bundles_by_dir.get(&dir)?;
                    let mut unique: Vec<&String> = v.iter().collect();
                    unique.sort();
                    unique.dedup();
                    if unique.len() == 1 {
                        Some(unique[0].clone())
                    } else {
                        None
                    }
                });
        }

        let Some(isrc) = isrc else {
            if role == ROLE_STEM {
                report.bundle_without_isrc.push(display_path(&abs, &data_dir));
            } else if role == ROLE_STEM_PART {
                report.stem_without_isrc.push(display_path(&abs, &data_dir));
            }
            report.unmanaged.push(UnmanagedFile {
                path: display_path(&abs, &data_dir),
                reason: "no ISRC (tag + catalogue both empty)".to_string(),
                size: o.size,
            });
            continue;
        };

        match role {
            ROLE_STEM => report.bundle_with_isrc += 1,
            ROLE_STEM_PART => report.stem_with_isrc += 1,
            _ => {}
        }

        let c = Candidate {
            isrc,
            role,
            abs,
            stem_part,
            origin: format!("store_objects:{ct}"),
            original_filename: Some(name),
        };
        candidates.insert(c.key(), c);
    }

    report.candidates = candidates.len();

    // 4. Plan against the existing rows.
    let mut seen_keys: Vec<String> = Vec::new();
    let mut now = now_iso();
    for (key, c) in &candidates {
        seen_keys.push(key.clone());
        let size = std::fs::metadata(&c.abs).map(|m| m.len() as i64).unwrap_or(0);
        let rel = display_path(&c.abs, &data_dir);

        match existing_map.get(key) {
            Some((old_path, old_size)) if old_path == &rel && *old_size == size => {
                report.unchanged += 1;
                continue;
            }
            Some(_) => {
                report.updated += 1;
                if apply {
                    sqlx::query(
                        "UPDATE track_files
                            SET path = ?, size = ?, origin = ?, original_filename = ?
                          WHERE isrc = ? AND role = ?
                            AND COALESCE(stem_part, '') = COALESCE(?, '')",
                    )
                    .bind(&rel)
                    .bind(size)
                    .bind(&c.origin)
                    .bind(&c.original_filename)
                    .bind(&c.isrc)
                    .bind(c.role)
                    .bind(&c.stem_part)
                    .execute(pool)
                    .await?;
                }
            }
            None => {
                report.inserted += 1;
                if apply {
                    let n = sqlx::query(
                        "INSERT INTO track_files
                            (isrc, role, path, size, sha256, stem_part, origin,
                             original_filename, created_at)
                         VALUES (?, ?, ?, ?, NULL, ?, ?, ?, ?)
                         ON CONFLICT DO NOTHING",
                    )
                    .bind(&c.isrc)
                    .bind(c.role)
                    .bind(&rel)
                    .bind(size)
                    .bind(&c.stem_part)
                    .bind(&c.origin)
                    .bind(&c.original_filename)
                    .bind(&now)
                    .execute(pool)
                    .await?
                    .rows_affected();
                    if n == 0 {
                        // Lost a race with another writer — the row exists.
                        report.inserted -= 1;
                        report.unchanged += 1;
                    } else {
                        now = now_iso();
                    }
                }
            }
        }
    }

    // 5. Rows whose file vanished are dropped (a stale catalogue is worse than
    //    a missing one). Only rows that map to a managed absolute path.
    let mut stale: Vec<(String, String, Option<String>)> = Vec::new();
    for (key, _) in existing_map.iter() {
        if seen_keys.contains(key) {
            continue;
        }
        let mut parts = key.split('\u{1}');
        let isrc = parts.next().unwrap_or("").to_string();
        let role = parts.next().unwrap_or("").to_string();
        let stem_part = parts.next().filter(|s| !s.is_empty()).map(str::to_string);
        stale.push((isrc, role, stem_part));
    }
    report.deleted = stale.len();
    if apply {
        for (isrc, role, stem_part) in &stale {
            sqlx::query(
                "DELETE FROM track_files
                  WHERE isrc = ? AND role = ?
                    AND COALESCE(stem_part, '') = COALESCE(?, '')",
            )
            .bind(isrc)
            .bind(role)
            .bind(stem_part)
            .execute(pool)
            .await?;
        }
    }

    // 6. Filesystem sweep: anything under the managed roots that the plan did
    //    not account for.
    let mut known: HashMap<String, ()> = HashMap::new();
    for c in candidates.values() {
        known.insert(display_path(&c.abs, &data_dir), ());
    }

    for dir in [config.flac_dir(), config.mp3_320_dir(), config.mp3_128_dir()] {
        collect_unmanaged(&dir, &data_dir, &known, &mut report.unmanaged);
    }
    collect_objects_unmanaged(
        &store.root,
        &data_dir,
        &objects.iter().map(|o| o.hash.clone()).collect(),
        &mut report.unmanaged,
    );
    // Files reported as missing must not also appear as unmanaged.
    report.unmanaged.sort_by(|a, b| a.path.cmp(&b.path));
    report.unmanaged.dedup_by(|a, b| a.path == b.path);

    if apply {
        let current: std::collections::HashSet<String> =
            report.unmanaged.iter().map(|u| u.path.clone()).collect();
        let old: Vec<(String, String, i64)> =
            sqlx::query_as("SELECT path, reason, size FROM track_files_unmanaged")
                .fetch_all(pool)
                .await?;
        for (path, _, _) in old {
            if !current.contains(&path) {
                sqlx::query("DELETE FROM track_files_unmanaged WHERE path = ?")
                    .bind(&path)
                    .execute(pool)
                    .await?;
            }
        }
        for u in &report.unmanaged {
            sqlx::query(
                "INSERT INTO track_files_unmanaged (path, reason, size) VALUES (?, ?, ?)
                 ON CONFLICT(path) DO UPDATE SET reason = excluded.reason, size = excluded.size",
            )
            .bind(&u.path)
            .bind(&u.reason)
            .bind(u.size)
            .execute(pool)
            .await?;
        }
    }

    report.missing_files = missing_files;
    report.missing_files.sort();
    report.missing_files.dedup();
    Ok(report)
}

fn collect_unmanaged(
    dir: &Path,
    data_dir: &Path,
    known: &HashMap<String, ()>,
    out: &mut Vec<UnmanagedFile>,
) {
    let walker = walkdir::WalkDir::new(dir).into_iter().filter_map(Result::ok);
    for entry in walker {
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = display_path(entry.path(), data_dir);
        if known.contains_key(&rel) {
            continue;
        }
        out.push(UnmanagedFile {
            path: rel,
            reason: "no track_files row (format dir)".to_string(),
            size: entry.metadata().map(|m| m.len() as i64).unwrap_or(0),
        });
    }
}

fn collect_objects_unmanaged(
    root: &Path,
    data_dir: &Path,
    known_hashes: &std::collections::HashSet<String>,
    out: &mut Vec<UnmanagedFile>,
) {
    let tmp = root.join("tmp");
    let walker = walkdir::WalkDir::new(root).into_iter().filter_map(Result::ok);
    for entry in walker {
        if !entry.file_type().is_file() {
            continue;
        }
        if entry.path().starts_with(&tmp) {
            continue;
        }
        let hash = entry
            .path()
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        if known_hashes.contains(&hash) {
            continue;
        }
        out.push(UnmanagedFile {
            path: display_path(entry.path(), data_dir),
            reason: "object file without store_objects row".to_string(),
            size: entry.metadata().map(|m| m.len() as i64).unwrap_or(0),
        });
    }
}

// ── archive-legacy (#702) ────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkipEntry {
    pub path: String,
    pub reason: String,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassReport {
    pub count: usize,
    pub bytes: u64,
    pub moved: usize,
    pub remaining: usize,
    pub not_identical: Vec<String>,
    pub skipped: Vec<SkipEntry>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyClasses {
    pub flac_copies: ClassReport,
    pub duplicate_bundles: ClassReport,
    pub relocated_bundles: ClassReport,
    pub mp3_copies: ClassReport,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveLegacyReport {
    /// `dry-run` or `apply`.
    pub mode: String,
    pub aborted: bool,
    pub aborted_reason: Option<String>,
    pub classes: LegacyClasses,
    pub total_moved: usize,
    pub total_bytes_moved: u64,
    /// Union of every class's skips (path + reason).
    pub skipped: Vec<SkipEntry>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LegacyClass {
    FlacCopy,
    DuplicateBundle,
    RelocatedBundle,
    Mp3Copy,
}

impl LegacyClass {
    fn label(self) -> &'static str {
        match self {
            LegacyClass::FlacCopy => "flac-copy",
            LegacyClass::DuplicateBundle => "duplicate-bundle",
            LegacyClass::RelocatedBundle => "relocated-bundle",
            LegacyClass::Mp3Copy => "mp3-copy",
        }
    }
}

#[derive(Debug, Clone)]
struct MovePlan {
    class: LegacyClass,
    isrc: String,
    src: PathBuf,
    dst: PathBuf,
    size: u64,
    /// `track_files.stem_part` discriminator (the unique key is
    /// `(isrc, role, COALESCE(stem_part,''))`, so each archived file needs one).
    stem_part: String,
    /// The existing catalogue row (`role`, `stem_part`) this move rewrites, if
    /// the candidate came from `track_files`. `None` → insert a new row.
    origin_row: Option<(String, Option<String>)>,
}

fn hex_str(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn sha256_file(p: &Path) -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open(p).ok()?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Some(hex_str(&hasher.finalize()))
}

fn basename(p: &Path) -> String {
    p.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string()
}

/// PCM-MD5 memo keyed by `(absolute path, size)` — a file is decoded at most once.
struct Md5Cache<'a> {
    ffmpeg: &'a str,
    map: HashMap<(String, u64), Option<String>>,
}

impl Md5Cache<'_> {
    async fn get(&mut self, p: &Path) -> Option<String> {
        let size = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
        let key = (p.to_string_lossy().to_string(), size);
        if let Some(v) = self.map.get(&key) {
            return v.clone();
        }
        let v = crate::transcode::pcm_md5(self.ffmpeg, p).await;
        self.map.insert(key, v.clone());
        v
    }
}

/// Reserve a unique `(isrc, 'archived', stem_part)` discriminator.
///
/// Uniqueness is enforced both among the planned moves (`used`) and against
/// rows already present in `track_files` (`existing`, keyed by
/// `(isrc, COALESCE(stem_part,''))`) so an in-place rewrite can never collide
/// with an existing `archived` row under the unique index.
fn unique_disc(
    used: &mut HashMap<(String, String), usize>,
    existing: &std::collections::HashSet<(String, String)>,
    isrc: &str,
    base: &str,
) -> String {
    let key = (isrc.to_string(), base.to_string());
    let n = used.entry(key).or_insert(0);
    loop {
        *n += 1;
        let cand = if *n == 1 {
            base.to_string()
        } else {
            format!("{base}-{n}")
        };
        if !existing.contains(&(isrc.to_string(), cand.clone())) {
            return cand;
        }
    }
}

/// Insert a fresh `archived` row for a move that has no catalogue origin.
/// `ON CONFLICT DO NOTHING` keeps a stray pre-existing discriminator harmless.
async fn insert_archived_row(
    pool: &Pool<Sqlite>,
    m: &MovePlan,
    dst_rel: &str,
    origin: &str,
    now: &str,
) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO track_files
            (isrc, role, path, size, sha256, stem_part, origin,
             original_filename, created_at)
         VALUES (?, 'archived', ?, ?, NULL, ?, ?, ?, ?)
         ON CONFLICT DO NOTHING",
    )
    .bind(&m.isrc)
    .bind(dst_rel)
    .bind(m.size as i64)
    .bind(&m.stem_part)
    .bind(origin)
    .bind(basename(&m.dst))
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Run `archive-legacy`.
///
/// Relocates legacy duplicates under `_archive/` (and orphan bundles into their
/// track folder) — **never deletes, never copies**: only `std::fs::rename`
/// inside the data dataset, and only for bytes proven identical by PCM MD5.
/// `apply == false` builds the same plan and reports it without touching a file
/// or writing a row.
pub async fn archive_legacy(
    pool: &Pool<Sqlite>,
    config: &Config,
    store: &Store,
    apply: bool,
) -> anyhow::Result<ArchiveLegacyReport> {
    let data_dir = config.data_dir.clone();
    let archive_root = data_dir.join("_archive");
    let flac_copies_dir = archive_root.join("flac-copies");
    let dup_dir = archive_root.join("duplicate-bundles");
    let mp3_copies_dir = archive_root.join("mp3-copies");
    let tracks_dir = data_dir.join("tracks");

    let mut report = ArchiveLegacyReport {
        mode: if apply { "apply" } else { "dry-run" }.to_string(),
        ..Default::default()
    };
    let mut cache = Md5Cache {
        ffmpeg: &config.ffmpeg,
        map: HashMap::new(),
    };
    let mut used_disc: HashMap<(String, String), usize> = HashMap::new();
    let mut moves: Vec<MovePlan> = Vec::new();

    // ── Load the catalogue ─────────────────────────────────────────────────
    let tracks: Vec<(String, Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT isrc, path_flac, path_320, path_128 FROM tracks")
            .fetch_all(pool)
            .await?;
    let mut delivery_flac: HashMap<String, PathBuf> = HashMap::new();
    let mut ref128: HashMap<String, PathBuf> = HashMap::new();
    for (isrc, flac, _p320, p128) in &tracks {
        if let Some(p) = flac.as_deref().filter(|s| !s.trim().is_empty()) {
            delivery_flac.insert(isrc.clone(), resolve_abs(p, &data_dir));
        }
        if let Some(p) = p128.as_deref().filter(|s| !s.trim().is_empty()) {
            ref128.insert(isrc.clone(), resolve_abs(p, &data_dir));
        }
    }

    let tf: Vec<(String, String, String, Option<String>)> =
        sqlx::query_as("SELECT isrc, role, path, stem_part FROM track_files")
            .fetch_all(pool)
            .await?;

    // Existing archived discriminators per ISRC — consulted when reserving a
    // new `stem_part` so an in-place rewrite cannot violate the unique index.
    let existing_archived: std::collections::HashSet<(String, String)> =
        sqlx::query_as::<_, (String, String)>(
            "SELECT isrc, COALESCE(stem_part, '') FROM track_files WHERE role = 'archived'",
        )
        .fetch_all(pool)
        .await?
        .into_iter()
        .collect();

    // Legit (never-touched) MP3 and FLAC delivery paths per ISRC.
    let mut legit_mp3: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (isrc, _role, path, _sp) in &tf {
        if let Some(p) = ref128.get(isrc) {
            legit_mp3.insert(p.to_string_lossy().to_string());
        }
        legit_mp3.insert(resolve_abs(path, &data_dir).to_string_lossy().to_string());
    }
    for (_isrc, _flac, p320, p128) in &tracks {
        for p in [p320, p128] {
            if let Some(p) = p.as_deref().filter(|s| !s.trim().is_empty()) {
                legit_mp3.insert(resolve_abs(p, &data_dir).to_string_lossy().to_string());
            }
        }
    }

    // Named stem bundles per ISRC (S1-attributed) — the "named twin".
    let mut named_stem: HashMap<String, PathBuf> = HashMap::new();
    let mut tf128: HashMap<String, PathBuf> = HashMap::new();
    for (isrc, role, path, _sp) in &tf {
        if role == ROLE_STEM {
            named_stem.entry(isrc.clone()).or_insert_with(|| resolve_abs(path, &data_dir));
        } else if role == ROLE_128 {
            tf128.entry(isrc.clone()).or_insert_with(|| resolve_abs(path, &data_dir));
        }
    }

    let objects: Vec<ObjectRow> = sqlx::query_as(
        "SELECT hash, size, content_type, original_path, isrc, name, stem_type
           FROM store_objects",
    )
    .fetch_all(pool)
    .await?;
    let unmanaged: Vec<(String, String, i64)> =
        sqlx::query_as("SELECT path, reason, size FROM track_files_unmanaged")
            .fetch_all(pool)
            .await?;

    // ── Class A: FLAC copies identical to the delivery FLAC ────────────────
    let mut seen_a: std::collections::HashSet<String> = std::collections::HashSet::new();
    // `origin_row` carries the existing `track_files` row (`role`, `stem_part`)
    // when the candidate is catalogue-backed, so `apply` rewrites it in place
    // instead of inserting a duplicate row (which would leave the old row
    // dangling on the relocated path).
    let mut cand_a: Vec<(PathBuf, String, Option<(String, Option<String>)>)> = Vec::new();
    for (isrc, role, path, sp) in &tf {
        if role != ROLE_FLAC {
            continue;
        }
        let abs = resolve_abs(path, &data_dir);
        if delivery_flac.get(isrc).map(|d| d == &abs).unwrap_or(false) {
            continue;
        }
        if seen_a.insert(abs.to_string_lossy().to_string()) {
            cand_a.push((abs, isrc.clone(), Some((role.clone(), sp.clone()))));
        }
    }
    for o in &objects {
        if o.ext().as_deref() != Some("flac") {
            continue;
        }
        let abs = store.path_for(&o.hash);
        let isrc = o
            .isrc
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| crate::tags::read_isrc(&abs));
        let Some(isrc) = isrc else { continue };
        if delivery_flac.get(&isrc).map(|d| d == &abs).unwrap_or(false) {
            continue;
        }
        if seen_a.insert(abs.to_string_lossy().to_string()) {
            cand_a.push((abs, isrc, None));
        }
    }
    for (abs, isrc, origin) in cand_a {
        if !abs.is_file() {
            report.classes.flac_copies.skipped.push(SkipEntry {
                path: display_path(&abs, &data_dir),
                reason: "source missing".to_string(),
            });
            continue;
        }
        let size = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
        report.classes.flac_copies.count += 1;
        report.classes.flac_copies.bytes += size;
        let rel = display_path(&abs, &data_dir);
        let Some(delivery) = delivery_flac.get(&isrc) else {
            report.classes.flac_copies.skipped.push(SkipEntry {
                path: rel,
                reason: "no delivery flac for ISRC".to_string(),
            });
            continue;
        };
        if !delivery.is_file() {
            report.classes.flac_copies.skipped.push(SkipEntry {
                path: rel,
                reason: "delivery flac missing".to_string(),
            });
            continue;
        }
        let cand_md5 = cache.get(&abs).await;
        let del_md5 = cache.get(delivery).await;
        match (cand_md5, del_md5) {
            (Some(a), Some(d)) if a == d => {
                let dst = flac_copies_dir.join(format!("{isrc}.flac"));
                let base = format!("flac-copy:{}", basename(&abs));
                let stem_part = unique_disc(&mut used_disc, &existing_archived, &isrc, &base);
                moves.push(MovePlan {
                    class: LegacyClass::FlacCopy,
                    isrc,
                    src: abs,
                    dst,
                    size,
                    stem_part,
                    origin_row: origin,
                });
            }
            (None, _) => report.classes.flac_copies.skipped.push(SkipEntry {
                path: rel,
                reason: "candidate unreadable (pcm md5)".to_string(),
            }),
            (_, None) => report.classes.flac_copies.skipped.push(SkipEntry {
                path: rel,
                reason: "delivery unreadable (pcm md5)".to_string(),
            }),
            _ => report.classes.flac_copies.not_identical.push(rel),
        }
    }

    // ── Classes B & C: anonymous bundles ──────────────────────────────────
    // Object hash by absolute path (for `duplicate-bundles/<hash>`).
    let mut obj_hash: HashMap<String, String> = HashMap::new();
    let mut obj_name: HashMap<String, String> = HashMap::new();
    let mut cand_bundle: Vec<(PathBuf, String)> = Vec::new();
    for o in &objects {
        let abs = store.path_for(&o.hash);
        obj_hash.insert(abs.to_string_lossy().to_string(), o.hash.clone());
        obj_name.insert(abs.to_string_lossy().to_string(), o.file_name());
        if !o.is_bundle() {
            continue;
        }
        let isrc = o
            .isrc
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| crate::tags::read_isrc(&abs));
        let Some(isrc) = isrc else { continue };
        cand_bundle.push((abs, isrc));
    }
    for (path, _reason, _size) in &unmanaged {
        let abs = resolve_abs(path, &data_dir);
        if !matches!(ext_of(path).as_deref(), Some("m4a") | Some("alac")) {
            continue;
        }
        let isrc = crate::tags::read_isrc(&abs);
        let Some(isrc) = isrc else {
            report.classes.relocated_bundles.skipped.push(SkipEntry {
                path: display_path(&abs, &data_dir),
                reason: "no_isrc".to_string(),
            });
            report.classes.relocated_bundles.count += 1;
            continue;
        };
        cand_bundle.push((abs, isrc));
    }

    let mut seen_b: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (abs, isrc) in cand_bundle {
        let canonical = tracks_dir.join(&isrc).join(format!("{isrc}.stem.m4a"));
        if abs == canonical || !seen_b.insert(abs.to_string_lossy().to_string()) {
            continue;
        }
        if !abs.is_file() {
            report.classes.relocated_bundles.skipped.push(SkipEntry {
                path: display_path(&abs, &data_dir),
                reason: "source missing".to_string(),
            });
            continue;
        }
        let size = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
        let rel = display_path(&abs, &data_dir);
        // A named twin: an S1-attributed stem row, else a file already at the
        // canonical path.
        let twin = named_stem
            .get(&isrc)
            .cloned()
            .or_else(|| canonical.is_file().then(|| canonical.clone()));
        match twin {
            Some(twin) if twin != abs => {
                report.classes.duplicate_bundles.count += 1;
                report.classes.duplicate_bundles.bytes += size;
                let cand_md5 = cache.get(&abs).await;
                let twin_md5 = cache.get(&twin).await;
                match (cand_md5, twin_md5) {
                    (Some(a), Some(t)) if a == t => {
                        let hash = obj_hash
                            .get(&abs.to_string_lossy().to_string())
                            .cloned()
                            .or_else(|| sha256_file(&abs))
                            .unwrap_or_else(|| basename(&abs));
                        let dst = dup_dir.join(&hash);
                        let base = format!("duplicate-bundle:{hash}");
                        let stem_part = unique_disc(&mut used_disc, &existing_archived, &isrc, &base);
                        moves.push(MovePlan {
                            class: LegacyClass::DuplicateBundle,
                            isrc,
                            src: abs,
                            dst,
                            size,
                            stem_part,
                            origin_row: None,
                        });
                    }
                    (None, _) => report.classes.duplicate_bundles.skipped.push(SkipEntry {
                        path: rel,
                        reason: "candidate unreadable (pcm md5)".to_string(),
                    }),
                    (_, None) => report.classes.duplicate_bundles.skipped.push(SkipEntry {
                        path: rel,
                        reason: "twin unreadable (pcm md5)".to_string(),
                    }),
                    _ => report.classes.duplicate_bundles.not_identical.push(rel),
                }
            }
            _ => {
                // Class C: no twin → relocate into the track folder.
                report.classes.relocated_bundles.count += 1;
                report.classes.relocated_bundles.bytes += size;
                if canonical.exists() {
                    report.classes.relocated_bundles.skipped.push(SkipEntry {
                        path: rel,
                        reason: "target occupied".to_string(),
                    });
                    continue;
                }
                let base = format!("relocated-bundle:{isrc}");
                let stem_part = unique_disc(&mut used_disc, &existing_archived, &isrc, &base);
                moves.push(MovePlan {
                    class: LegacyClass::RelocatedBundle,
                    isrc,
                    src: abs,
                    dst: canonical,
                    size,
                    stem_part,
                    origin_row: None,
                });
            }
        }
    }

    // ── Class D: MP3 copies identical to the 128 reference ────────────────
    let mut cand_mp3: Vec<(PathBuf, String)> = Vec::new();
    let mut seen_d: std::collections::HashSet<String> = std::collections::HashSet::new();
    for o in &objects {
        if o.ext().as_deref() != Some("mp3") {
            continue;
        }
        let abs = store.path_for(&o.hash);
        if legit_mp3.contains(&abs.to_string_lossy().to_string()) {
            continue;
        }
        let isrc = o
            .isrc
            .clone()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| crate::tags::read_isrc(&abs));
        let Some(isrc) = isrc else { continue };
        if seen_d.insert(abs.to_string_lossy().to_string()) {
            cand_mp3.push((abs, isrc));
        }
    }
    for (path, _reason, _size) in &unmanaged {
        if ext_of(path).as_deref() != Some("mp3") {
            continue;
        }
        let abs = resolve_abs(path, &data_dir);
        if legit_mp3.contains(&abs.to_string_lossy().to_string()) {
            continue;
        }
        let Some(isrc) = crate::tags::read_isrc(&abs) else { continue };
        if seen_d.insert(abs.to_string_lossy().to_string()) {
            cand_mp3.push((abs, isrc));
        }
    }
    for (abs, isrc) in cand_mp3 {
        if !abs.is_file() {
            report.classes.mp3_copies.skipped.push(SkipEntry {
                path: display_path(&abs, &data_dir),
                reason: "source missing".to_string(),
            });
            continue;
        }
        let size = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
        report.classes.mp3_copies.count += 1;
        report.classes.mp3_copies.bytes += size;
        let rel = display_path(&abs, &data_dir);
        let reference = ref128.get(&isrc).cloned().or_else(|| tf128.get(&isrc).cloned());
        let Some(reference) = reference else {
            report.classes.mp3_copies.skipped.push(SkipEntry {
                path: rel,
                reason: "no 128 reference".to_string(),
            });
            continue;
        };
        if !reference.is_file() {
            report.classes.mp3_copies.skipped.push(SkipEntry {
                path: rel,
                reason: "128 reference missing".to_string(),
            });
            continue;
        }
        let cand_md5 = cache.get(&abs).await;
        let ref_md5 = cache.get(&reference).await;
        match (cand_md5, ref_md5) {
            (Some(a), Some(r)) if a == r => {
                let name = obj_name
                    .get(&abs.to_string_lossy().to_string())
                    .cloned()
                    .unwrap_or_else(|| basename(&abs));
                let name = if name.to_ascii_lowercase().ends_with(".mp3") {
                    name
                } else {
                    format!("{name}.mp3")
                };
                let dst = mp3_copies_dir.join(&name);
                let base = format!("mp3-copy:{name}");
                let stem_part = unique_disc(&mut used_disc, &existing_archived, &isrc, &base);
                moves.push(MovePlan {
                    class: LegacyClass::Mp3Copy,
                    isrc,
                    src: abs,
                    dst,
                    size,
                    stem_part,
                    origin_row: None,
                });
            }
            (None, _) => report.classes.mp3_copies.skipped.push(SkipEntry {
                path: rel,
                reason: "candidate unreadable (pcm md5)".to_string(),
            }),
            (_, None) => report.classes.mp3_copies.skipped.push(SkipEntry {
                path: rel,
                reason: "reference unreadable (pcm md5)".to_string(),
            }),
            _ => report.classes.mp3_copies.not_identical.push(rel),
        }
    }

    // ── Pre-flight: a target must be free and unique ───────────────────────
    let mut seen_dst: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut conflict: Option<String> = None;
    for m in &moves {
        if m.dst.exists() {
            conflict = Some(format!("target occupied: {}", display_path(&m.dst, &data_dir)));
            break;
        }
        if !seen_dst.insert(m.dst.to_string_lossy().to_string()) {
            conflict = Some(format!("two moves to {}", display_path(&m.dst, &data_dir)));
            break;
        }
    }
    if let Some(reason) = conflict {
        report.aborted = true;
        report.aborted_reason = Some(reason);
        return Ok(report);
    }

    // Planned counts are visible in both modes; `apply` performs them below.
    report.total_moved = moves.len();
    report.total_bytes_moved = moves.iter().map(|m| m.size).sum();
    for m in &moves {
        let cr = match m.class {
            LegacyClass::FlacCopy => &mut report.classes.flac_copies,
            LegacyClass::DuplicateBundle => &mut report.classes.duplicate_bundles,
            LegacyClass::RelocatedBundle => &mut report.classes.relocated_bundles,
            LegacyClass::Mp3Copy => &mut report.classes.mp3_copies,
        };
        cr.moved += 1;
    }

    if apply {
        std::fs::create_dir_all(&archive_root)?;
        let now = now_iso();
        let mut moved_now: usize = 0;
        for m in &moves {
            if let Some(parent) = m.dst.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if let Err(e) = std::fs::rename(&m.src, &m.dst) {
                report.errors.push(format!(
                    "rename {}: {e}",
                    display_path(&m.src, &data_dir)
                ));
                report.aborted = true;
                report.aborted_reason = Some(format!(
                    "rename failed: {}",
                    display_path(&m.src, &data_dir)
                ));
                break;
            }
            moved_now += 1;
            let dst_rel = display_path(&m.dst, &data_dir);
            let origin = format!("{}:{}", m.class.label(), display_path(&m.src, &data_dir));
            if let Some((role, sp)) = &m.origin_row {
                let updated = sqlx::query(
                    "UPDATE track_files
                        SET role = 'archived', path = ?, size = ?, stem_part = ?,
                            origin = ?, original_filename = ?
                      WHERE isrc = ? AND role = ?
                        AND COALESCE(stem_part, '') = COALESCE(?, '')",
                )
                .bind(&dst_rel)
                .bind(m.size as i64)
                .bind(&m.stem_part)
                .bind(&origin)
                .bind(basename(&m.dst))
                .bind(&m.isrc)
                .bind(role)
                .bind(sp)
                .execute(pool)
                .await?
                .rows_affected();
                // The origin row should always exist (it came from `tf`); if it
                // did not, still record the relocated file instead of leaving a
                // dangling filesystem entry without a catalogue row.
                if updated == 0 {
                    insert_archived_row(pool, m, &dst_rel, &origin, &now).await?;
                }
            } else {
                insert_archived_row(pool, m, &dst_rel, &origin, &now).await?;
            }
        }
        report.total_moved = moved_now;
        report.total_bytes_moved = moves.iter().take(moved_now).map(|m| m.size).sum();
    }

    // ── Derived counters + aggregate skips ────────────────────────────────
    let classes = &mut report.classes;
    for cr in [
        &mut classes.flac_copies,
        &mut classes.duplicate_bundles,
        &mut classes.relocated_bundles,
        &mut classes.mp3_copies,
    ] {
        cr.remaining = cr
            .count
            .saturating_sub(cr.moved)
            .saturating_sub(cr.not_identical.len());
    }
    report.skipped = [
        &classes.flac_copies,
        &classes.duplicate_bundles,
        &classes.relocated_bundles,
        &classes.mp3_copies,
    ]
    .iter()
    .flat_map(|cr| cr.skipped.iter().cloned())
    .collect();

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_formats_known_instants() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_700_000_000), "2023-11-14T22:13:20Z");
    }

    #[test]
    fn stem_names_split_by_suffix() {
        assert_eq!(
            split_stem_name("Artist - Song_vocals.wav"),
            ("Artist - Song".to_string(), Some("vocals".to_string()))
        );
        assert_eq!(
            split_stem_name("Artist - Song.stem.m4a"),
            ("Artist - Song".to_string(), None)
        );
    }

    #[test]
    fn display_path_prefers_data_dir_relative() {
        let dd = Path::new("/data");
        assert_eq!(
            display_path(Path::new("/data/128/a.mp3"), dd),
            "128/a.mp3"
        );
        assert_eq!(display_path(Path::new("/other/a.mp3"), dd), "/other/a.mp3");
    }
}