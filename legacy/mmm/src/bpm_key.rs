//! BPM//key system playlists — pure derivation, naming and grouping core.
//!
//! For every `(BPM, key)` combination present in the library we materialise a
//! real Spotify playlist. The *derivation* (which files/tracks belong to which
//! bucket) and the *naming* are pure functions, unit-tested here. The database
//! query lives in [`crate::db::bpm_key`] and the Spotify reconcile worker in
//! [`crate::bpm_key::sync`].
//!
//! Grouping is by *canonical* Camelot key (`m`→A, `d`→B) so `12m` and `12A`
//! never split; rendering defaults to the stored Traktor style (`12m`).
//!
//! These playlists are `playlist_kind = 'generated'` system playlists: they must
//! never influence tag matching, the comment write-out, usage / "last touched"
//! scoring or normal playlist polling.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::config::ServiceCredentials;
use crate::digging::parse_camelot_key;

pub mod sync;

/// Traktor-style key rendering (`12m` / `12d`) — the default.
pub const KEY_STYLE_MD: &str = "md";
/// Camelot-style key rendering (`12A` / `12B`).
pub const KEY_STYLE_CAMELOT: &str = "camelot";

/// Default display template → `124bpm // 12m`.
pub const DEFAULT_NAME_TEMPLATE: &str = "{bpm}bpm // {key}";

/// A canonical `(BPM, key)` bucket key. `canonical_key` is Camelot
/// position+mode, e.g. `"12A"` (from `12m`/`12A`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BpmKey {
    pub bpm: i64,
    pub canonical_key: String,
}

impl BpmKey {
    /// Parse a file's raw BPM / musical key.
    ///
    /// Rounds the BPM to the nearest integer and normalises the key to
    /// canonical Camelot (`m`→`A`, `d`→`B`). Returns `None` when either value is
    /// missing, blank, invalid, or the BPM is not finite — such files are
    /// excluded from every bucket.
    pub fn from_file(bpm: Option<f64>, musical_key: Option<&str>) -> Option<Self> {
        let bpm = bpm?;
        if !bpm.is_finite() {
            return None;
        }
        let key = musical_key?.trim();
        if key.is_empty() {
            return None;
        }
        let camelot = parse_camelot_key(key)?;
        Some(Self {
            bpm: bpm.round() as i64,
            canonical_key: format!("{}{}", camelot.position, camelot.mode),
        })
    }

    /// Stable system key for this bucket, e.g. `bpm_key:124:12A`.
    pub fn system_key(&self) -> String {
        system_key(self.bpm, &self.canonical_key)
    }
}

/// Stable combo id, independent of the display template.
pub fn system_key(bpm: i64, canonical_key: &str) -> String {
    format!("bpm_key:{bpm}:{canonical_key}")
}

/// Render a canonical Camelot key in the requested style.
///
/// `md` (default, Traktor) → `12m`/`12d`; `camelot` → `12A`/`12B`.
pub fn render_key(canonical_key: &str, style: &str) -> String {
    if style.eq_ignore_ascii_case(KEY_STYLE_CAMELOT) {
        return canonical_key.to_string();
    }
    if let Some(pos) = canonical_key.strip_suffix('A') {
        format!("{pos}m")
    } else if let Some(pos) = canonical_key.strip_suffix('B') {
        format!("{pos}d")
    } else {
        canonical_key.to_string()
    }
}

/// Evaluate the display template. Supported placeholders: `{bpm}`, `{key}`,
/// `{count}`. `prefix` (default empty) is prepended verbatim.
pub fn format_name(
    template: &str,
    prefix: &str,
    bpm: i64,
    key: &str,
    count: Option<usize>,
) -> String {
    let body = template
        .replace("{bpm}", &bpm.to_string())
        .replace("{key}", key)
        .replace("{count}", &count.map(|c| c.to_string()).unwrap_or_default());
    format!("{prefix}{body}")
}

// ── Grouping ────────────────────────────────────────────────────────────────

/// One `files × service_tracks` link row as returned by the derivation query.
#[derive(Debug, Clone)]
pub struct FileTrackRow {
    pub file_id: i64,
    pub bpm: f64,
    pub musical_key: String,
    pub stem_type: Option<String>,
    pub track_id: i64,
    pub service_id: String,
}

/// A materialisable bucket: canonical `(BPM, key)` plus its backing tracks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub bpm: i64,
    pub canonical_key: String,
    pub file_count: i64,
    pub track_count: i64,
    /// De-duplicated `spotify:track:…` URIs, sorted.
    pub uris: Vec<String>,
}

impl Group {
    pub fn system_key(&self) -> String {
        system_key(self.bpm, &self.canonical_key)
    }
}

/// Group link rows into canonical buckets.
///
/// Derivation is **per Spotify track**: a track's bucket is decided by one
/// *representative file* (prefer a non-stem file — `stem_type IS NULL` — then
/// the lowest `files.id`). A `128.0` flac plus a `128.5` stem that share a
/// track therefore collapse into a single bucket. `file_count` counts every
/// distinct local file backing the bucket's tracks; `track_count` the distinct
/// tracks. URIs are de-duplicated.
pub fn group_rows(rows: &[FileTrackRow]) -> Vec<Group> {
    // 1. Representative file per track (prefer non-stem, then lowest file id).
    struct Rep {
        bucket: BpmKey,
        stem_rank: u8,
        file_id: i64,
    }
    let mut reps: HashMap<i64, Rep> = HashMap::new();
    for row in rows {
        let Some(bucket) = BpmKey::from_file(Some(row.bpm), Some(&row.musical_key)) else {
            continue;
        };
        let stem_rank = u8::from(row.stem_type.is_some());
        let dominates = reps
            .get(&row.track_id)
            .map(|cur| (stem_rank, row.file_id) < (cur.stem_rank, cur.file_id))
            .unwrap_or(true);
        if dominates {
            reps.insert(
                row.track_id,
                Rep {
                    bucket,
                    stem_rank,
                    file_id: row.file_id,
                },
            );
        }
    }

    // 2. Accumulate tracks/files/URIs per bucket via the representative's bucket.
    let mut buckets: BTreeMap<(i64, String), (BTreeSet<i64>, BTreeSet<i64>, BTreeSet<String>)> =
        BTreeMap::new();
    for row in rows {
        let Some(rep) = reps.get(&row.track_id) else {
            continue;
        };
        let entry = buckets
            .entry((rep.bucket.bpm, rep.bucket.canonical_key.clone()))
            .or_default();
        entry.0.insert(row.track_id);
        entry.1.insert(row.file_id);
        entry.2.insert(format!("spotify:track:{}", row.service_id));
    }

    let mut out: Vec<Group> = buckets
        .into_iter()
        .map(|((bpm, canonical_key), (tracks, files, uris))| Group {
            bpm,
            canonical_key,
            file_count: files.len() as i64,
            track_count: tracks.len() as i64,
            uris: uris.into_iter().collect(),
        })
        .collect();

    // Deterministic, human-friendly order: BPM, then Camelot position, then mode.
    out.sort_by(|a, b| {
        a.bpm.cmp(&b.bpm).then_with(|| {
            let ka = parse_camelot_key(&a.canonical_key).map(|k| (k.position, k.mode));
            let kb = parse_camelot_key(&b.canonical_key).map(|k| (k.position, k.mode));
            ka.cmp(&kb)
        })
    });
    out
}

// ── Settings ────────────────────────────────────────────────────────────────

/// Persisted feature settings (settings-KV, namespace `bpmkey.`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BpmKeySettings {
    /// Auto-enqueue a sync after scans/imports and on the schedule.
    pub enabled: bool,
    pub name_template: String,
    pub name_prefix: String,
    pub min_tracks: i64,
    pub public: bool,
    pub key_style: String,
    pub strict: bool,
    pub schedule_enabled: bool,
    pub schedule_interval_secs: i64,
}

impl Default for BpmKeySettings {
    fn default() -> Self {
        Self {
            enabled: false,
            name_template: DEFAULT_NAME_TEMPLATE.to_string(),
            name_prefix: String::new(),
            min_tracks: 1,
            public: false,
            key_style: KEY_STYLE_MD.to_string(),
            strict: false,
            schedule_enabled: false,
            schedule_interval_secs: 3600,
        }
    }
}

impl BpmKeySettings {
    /// Display key (e.g. `12m`) for a canonical key (`12A`) under this settings'
    /// `key_style`.
    pub fn display_key(&self, canonical_key: &str) -> String {
        render_key(canonical_key, &self.key_style)
    }

    /// Playlist name for a bucket under this settings' template + prefix.
    pub fn name_for(&self, bpm: i64, canonical_key: &str, count: Option<usize>) -> String {
        format_name(
            &self.name_template,
            &self.name_prefix,
            bpm,
            &self.display_key(canonical_key),
            count,
        )
    }

    /// Clamp/validate values that come from an untrusted (partial) request body.
    pub fn sanitize(&mut self) {
        let t = self.name_template.trim();
        if t.is_empty() {
            self.name_template = DEFAULT_NAME_TEMPLATE.to_string();
        }
        if self.min_tracks < 1 {
            self.min_tracks = 1;
        }
        if self.schedule_interval_secs < 60 {
            self.schedule_interval_secs = 60;
        }
        if !self.key_style.eq_ignore_ascii_case(KEY_STYLE_CAMELOT) {
            self.key_style = KEY_STYLE_MD.to_string();
        } else {
            self.key_style = KEY_STYLE_CAMELOT.to_string();
        }
    }
}

// ── Process-wide config for auto-triggered syncs ─────────────────────────────
//
// The auto-trigger runs inside background scan/import workers that (by design of
// the surrounding modules) are not handed the `ServiceCredentials`. `lib.rs`
// installs a clone of the live config here at router-build time — which happens
// in both the binary and every integration test.

static INSTALLED_CONFIG: OnceLock<ServiceCredentials> = OnceLock::new();

/// Install the process-wide config used by auto-triggered syncs. Idempotent —
/// the first install wins.
pub fn install_config(config: ServiceCredentials) {
    let _ = INSTALLED_CONFIG.set(config);
}

/// The installed config, if any.
pub fn installed_config() -> Option<ServiceCredentials> {
    INSTALLED_CONFIG.get().cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        file_id: i64,
        bpm: f64,
        key: &str,
        stem: Option<&str>,
        track_id: i64,
        sid: &str,
    ) -> FileTrackRow {
        FileTrackRow {
            file_id,
            bpm,
            musical_key: key.to_string(),
            stem_type: stem.map(|s| s.to_string()),
            track_id,
            service_id: sid.to_string(),
        }
    }

    #[test]
    fn from_file_rounds_bpm_to_nearest_integer() {
        assert_eq!(
            BpmKey::from_file(Some(128.4), Some("12m")).unwrap().bpm,
            128
        );
        assert_eq!(
            BpmKey::from_file(Some(128.5), Some("12m")).unwrap().bpm,
            129
        );
        assert_eq!(
            BpmKey::from_file(Some(124.0), Some("12m")).unwrap().bpm,
            124
        );
    }

    #[test]
    fn from_file_normalises_keys_to_canonical_camelot() {
        assert_eq!(
            BpmKey::from_file(Some(124.0), Some("12m"))
                .unwrap()
                .canonical_key,
            "12A"
        );
        assert_eq!(
            BpmKey::from_file(Some(124.0), Some("12A"))
                .unwrap()
                .canonical_key,
            "12A"
        );
        assert_eq!(
            BpmKey::from_file(Some(124.0), Some("4d"))
                .unwrap()
                .canonical_key,
            "4B"
        );
        assert_eq!(
            BpmKey::from_file(Some(124.0), Some("4B"))
                .unwrap()
                .canonical_key,
            "4B"
        );
    }

    #[test]
    fn from_file_rejects_missing_blank_and_invalid() {
        assert!(BpmKey::from_file(None, Some("12m")).is_none());
        assert!(BpmKey::from_file(Some(124.0), None).is_none());
        assert!(BpmKey::from_file(Some(124.0), Some("   ")).is_none());
        assert!(BpmKey::from_file(Some(124.0), Some("Am")).is_none());
        assert!(BpmKey::from_file(Some(124.0), Some("13m")).is_none());
        assert!(BpmKey::from_file(Some(f64::NAN), Some("12m")).is_none());
    }

    #[test]
    fn system_key_format() {
        assert_eq!(system_key(124, "12A"), "bpm_key:124:12A");
        assert_eq!(
            BpmKey::from_file(Some(124.0), Some("12m"))
                .unwrap()
                .system_key(),
            "bpm_key:124:12A"
        );
    }

    #[test]
    fn render_key_styles() {
        assert_eq!(render_key("12A", KEY_STYLE_MD), "12m");
        assert_eq!(render_key("12B", KEY_STYLE_MD), "12d");
        assert_eq!(render_key("12A", KEY_STYLE_CAMELOT), "12A");
        assert_eq!(render_key("4B", "garbage"), "4d"); // falls back to md
    }

    #[test]
    fn template_rendering() {
        assert_eq!(
            format_name(DEFAULT_NAME_TEMPLATE, "", 124, "12m", None),
            "124bpm // 12m"
        );
        assert_eq!(
            format_name("{bpm}bpm // {key} ({count})", "", 124, "12m", Some(37)),
            "124bpm // 12m (37)"
        );
        assert_eq!(
            format_name(DEFAULT_NAME_TEMPLATE, "⌁ ", 124, "12m", None),
            "⌁ 124bpm // 12m"
        );
    }

    #[test]
    fn settings_name_for_applies_template_prefix_and_style() {
        let s = BpmKeySettings {
            name_prefix: "⌁ ".to_string(),
            key_style: KEY_STYLE_CAMELOT.to_string(),
            ..Default::default()
        };
        assert_eq!(s.name_for(124, "12A", None), "⌁ 124bpm // 12A");
    }

    #[test]
    fn group_rows_collapses_m_and_camelot_variants() {
        // Two files, two tracks, same bucket via 12m and 12A.
        let rows = vec![
            row(1, 124.0, "12m", None, 10, "aaa"),
            row(2, 124.0, "12A", None, 11, "bbb"),
        ];
        let groups = group_rows(&rows);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].bpm, 124);
        assert_eq!(groups[0].canonical_key, "12A");
        assert_eq!(groups[0].track_count, 2);
        assert_eq!(groups[0].file_count, 2);
        assert_eq!(
            groups[0].uris,
            vec!["spotify:track:aaa", "spotify:track:bbb"]
        );
    }

    #[test]
    fn group_rows_variant_pair_yields_single_bucket() {
        // 128.0 flac (non-stem) + 128.5 stem, both linked to the SAME track.
        let rows = vec![
            row(1, 128.0, "12m", None, 10, "aaa"),
            row(2, 128.5, "12m", Some("drums"), 10, "aaa"),
        ];
        let groups = group_rows(&rows);
        assert_eq!(groups.len(), 1, "the stem must not create a second bucket");
        assert_eq!(groups[0].bpm, 128, "flac is the representative file");
        assert_eq!(groups[0].track_count, 1);
        assert_eq!(groups[0].file_count, 2);
        assert_eq!(groups[0].uris, vec!["spotify:track:aaa"], "URIs de-duped");
    }

    #[test]
    fn group_rows_ignores_files_without_bpm_or_key() {
        let rows = vec![
            row(1, 124.0, "", None, 10, "aaa"), // blank key
            row(2, 124.0, "not-a-key", None, 10, "aaa"),
        ];
        assert!(group_rows(&rows).is_empty());
    }

    #[test]
    fn group_rows_sorted_by_bpm_then_key() {
        let rows = vec![
            row(1, 130.0, "4d", None, 12, "ccc"),
            row(2, 124.0, "12m", None, 10, "aaa"),
            row(3, 124.0, "4m", None, 11, "bbb"),
        ];
        let groups = group_rows(&rows);
        let keys: Vec<(i64, &str)> = groups
            .iter()
            .map(|g| (g.bpm, g.canonical_key.as_str()))
            .collect();
        assert_eq!(keys, vec![(124, "4A"), (124, "12A"), (130, "4B")]);
    }
}
