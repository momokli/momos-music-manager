//! Type‑information lookup that powers the new tag‑type subsystem.
//!
//! `TypeSet` represents the per‑owner type definitions from `hub_tag_types`.
//! It replaces the former compile‑time `TagType` enum and `TYPE_NAMES` constant.
//! All callers load one `TypeSet` per `owner_user_id` (once per request) and use it
//! for parsing, labelling, sorting, exporting and scoring.

use anyhow::Result;
use sqlx::SqlitePool;

/// Runtime representation of a tag type taken from `hub_tag_types`.
#[derive(Debug, Clone)]
pub struct TypeInfo {
    pub key: String,      // canonical DB key (e.g., "MOOD")
    pub label: String,    // German display label (e.g., "Mood")
    pub abbrev: String,   // single‑letter export marker (e.g., "M")
    pub icon: String,     // emoji icon (e.g., "💜")
    pub sort_order: i64,  // numeric sort key for exports (lower → earlier)
    pub describes: bool,  // whether this type describes a track (vs. only sorting)
    pub weight: f64,      // scoring weight for similarity (see `scoring.rs`)
}

/// A cached view of all types for a single owner.
#[derive(Debug)]
pub struct TypeSet {
    // Map from type key to its runtime metadata.
    by_key: std::collections::HashMap<String, TypeInfo>,
    // Index from export abbrev to all types that share it (collapse "P" for phases).
    by_abbrev: std::collections::HashMap<String, Vec<TypeInfo>>,
    // Pre‑sorted keys by `sort_order` then `key` (stable output order).
    sorted_keys: Vec<String>,
}

impl TypeSet {
    /// Load the owner's types from `hub_tag_types`, building the index structures.
    pub async fn load(pool: &SqlitePool, owner_user_id: i64) -> Result<Self> {
        // Phase 1: fetch rows from the DB.
        let rows = sqlx::query_as::<
            _,
            (
                i64,      // owner_user_id (ignored, same as parameter)
                String,   // key
                String,   // label
                String,   // abbrev
                String,   // icon
                i64,      // sort_order
                i64,      // describes (stored as INTEGER in SQLite)
                f64,      // weight
            ),
        >(
            "SELECT owner_user_id, key, label, abbrev, icon, sort_order, describes, weight
               FROM hub_tag_types
              WHERE owner_user_id = ?1
              ORDER BY sort_order, key",
        )
        .bind(owner_user_id)
        .fetch_all(pool)
        .await?;

        // Phase 2: build by_key map and by_abbrev index.
        let mut by_key = std::collections::HashMap::new();
        let mut by_abbrev: std::collections::HashMap<String, Vec<TypeInfo>> =
            std::collections::HashMap::new();

        for (owner, key, label, abbrev, icon, sort_order, describes, weight) in rows {
            let info = TypeInfo {
                key,
                label,
                abbrev: abbrev.clone(),
                icon,
                sort_order,
                describes: describes != 0,
                weight,
            };
            by_key.insert(info.key.clone(), info.clone());
            by_abbrev.entry(info.abbrev.clone()).or_default().push(info);
        }

        // Phase 3: build sorted_keys (keys ordered by sort_order then alphabetically).
        let mut sorted_keys: Vec<(i64, String)> = by_key
            .iter()
            .map(|(k, v)| (v.sort_order, k.clone()))
            .collect();
        sorted_keys.sort_by(|a, b| {
            a.0.cmp(&b.0)
                .then_with(|| a.1.to_lowercase().cmp(&b.1.to_lowercase()))
                .then_with(|| a.1.cmp(&b.1))
        });
        let sorted_keys: Vec<String> = sorted_keys.into_iter().map(|(_, k)| k).collect();

        Ok(Self {
            by_key,
            by_abbrev,
            sorted_keys,
        })
    }

    /// Look up a type by its canonical key (DB string). Returns `None` when the
    /// key is unknown (e.g., an untyped tag or a stale migration). The caller must
    /// fall back to a deterministic default in that case.
    pub fn get(&self, key: &str) -> Option<&TypeInfo> {
        self.by_key.get(key)
    }

    /// Resolve the best‑effort metadata for an unknown type key.
    ///
    /// The export and scoring code never want to crash on a bad type string;
    /// instead they rely on this fallback:
    ///   * label = key (raw, trimmed),
    ///   * abbrev = "", sort_order = i64::MAX (pushes to export end),
    ///   * describes = true (safe default),
    ///   * weight = 0.0 (no contribution to scoring),
    ///   * icon = "#️⃣" (neutral emoji).
    pub fn fallback(&self, key: &str) -> TypeInfo {
        let key_trimmed = key.trim();
        TypeInfo {
            key: key_trimmed.to_string(),
            label: key_trimmed.to_string(),
            abbrev: String::new(),
            icon: "#️⃣".to_string(),
            sort_order: i64::MAX,
            describes: true,
            weight: 0.0,
        }
    }

    /// Return the type infos sorted by `sort_order` then `key`.
    pub fn sorted_types(&self) -> impl Iterator<Item = &TypeInfo> {
        self.sorted_keys
            .iter()
            .filter_map(|k| self.by_key.get(k))
    }

    /// Produce the list of distinct export abbrevs, in the order they appear in
    /// the DB (`sort_order`), deduplicated by abbrev.
    ///
    /// Example: `PHASE_1..5` all have abbrev = "P", so the output contains only
    /// one `"P"` in the position of the first phase type.
    pub fn distinct_abbrevs(&self) -> Vec<String> {
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut out: Vec<String> = Vec::new();

        for (abbrev, infos) in &self.by_abbrev {
            if !seen.contains(abbrev) {
                seen.insert(abbrev.clone());
                out.push(abbrev.clone());
            }
        }

        // The export code expects the order to match the canonical `TYPE_NAMES`
        // ordering (lower `sort_order` first) for deterministic parity across
        // back‑package and comment‑preview.  We sort by the `sort_order` of the
        // *first* info for each abbrev.
        out.sort_by_key(|abbrev| {
            self.by_abbrev
                .get(abbrev)
                .and_then(|infos| infos.first().map(|info| info.sort_order))
                .unwrap_or(i64::MAX)
        });

        out
    }
}