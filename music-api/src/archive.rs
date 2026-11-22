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

use sqlx::{Pool, Sqlite};

use crate::config::Config;
use crate::store::Store;

pub const ROLE_128: &str = "128";
pub const ROLE_320: &str = "320";
pub const ROLE_FLAC: &str = "flac";
pub const ROLE_STEM: &str = "stem";
pub const ROLE_STEM_PART: &str = "stem_part";

pub const ROLES: [&str; 5] = [
    ROLE_128,
    ROLE_320,
    ROLE_FLAC,
    ROLE_STEM,
    ROLE_STEM_PART,
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