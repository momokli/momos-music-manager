//! Detection and upgrade-decision logic for "Extended Mix" track versions.
//!
//! Music sources expose multiple versions of the same release under different
//! suffixes — e.g. `Rider (Original Mix)`, `Rider (Radio Edit)`,
//! `Rider (Extended Mix)`. Historically every version was treated as a
//! separate, unrelated track, so there was no way to tell that a longer
//! (Extended Mix) version exists for a shorter one already in the library.
//!
//! This module is the pure, source-agnostic core for issue #29:
//!
//! 1. **Detection** — recognise the variant suffix on a title so versions of
//!    the same release can be grouped ([`base_title`]) and the Extended Mix
//!    identified ([`is_extended_mix`], [`find_extended_mix`]).
//! 2. **Upgrade decision** — decide whether a release should be upgraded to
//!    its Extended Mix without loops or downgrades
//!    ([`upgrade_target_for_release`]).
//!
//! It is intentionally free of any network/DB dependency so the contract can
//! be unit-tested exhaustively. Source adapters (Beatport, Deezer/deemix, …)
//! and the library scan/upgrade actions build on top of these primitives.

/// The known variant suffixes that can appear on a track title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VariantKind {
    ExtendedMix,
    RadioEdit,
    OriginalMix,
    ClubMix,
    Instrumental,
    /// No recognised variant suffix.
    None,
}

impl VariantKind {
    /// Stable machine-readable label (for logs/telemetry/UI).
    pub fn as_str(self) -> &'static str {
        match self {
            VariantKind::ExtendedMix => "extended-mix",
            VariantKind::RadioEdit => "radio-edit",
            VariantKind::OriginalMix => "original-mix",
            VariantKind::ClubMix => "club-mix",
            VariantKind::Instrumental => "instrumental",
            VariantKind::None => "none",
        }
    }
}

/// Variant markers ordered from most to least specific. Order matters for
/// correct `base_title` stripping (e.g. `"extended mix"` must match before
/// `"extended"`, and `"ext mix"` must match before `"extended"`).
const VARIANT_MARKERS: &[(VariantKind, &str)] = &[
    (VariantKind::ExtendedMix, "extended mix"),
    (VariantKind::ExtendedMix, "ext mix"),
    (VariantKind::ExtendedMix, "extended"),
    (VariantKind::RadioEdit, "radio edit"),
    (VariantKind::RadioEdit, "radio version"),
    (VariantKind::RadioEdit, "radio mix"),
    (VariantKind::OriginalMix, "original mix"),
    (VariantKind::OriginalMix, "original"),
    (VariantKind::ClubMix, "club mix"),
    (VariantKind::Instrumental, "instrumental mix"),
    (VariantKind::Instrumental, "instrumental"),
];

/// Normalise a title for suffix matching: lowercase, trim, and strip any
/// trailing closing bracket/paren (plus whitespace) so that both
/// `"Rider (Extended Mix)"` and `"Rider [Extended Mix]"` reduce to a form
/// that ends with the bare `"extended mix"` marker.
fn normalized(title: &str) -> String {
    let mut t = title.trim().to_lowercase();
    loop {
        let next = t
            .trim_end()
            .trim_end_matches(|c: char| c == ')' || c == ']')
            .trim_end()
            .to_string();
        if next == t {
            break;
        }
        t = next;
    }
    t
}

/// Find the first variant marker that `title` ends with (after normalisation).
/// Returns the kind and the matched marker string.
fn match_variant(title: &str) -> Option<(VariantKind, &'static str)> {
    let t = normalized(title);
    if t.is_empty() {
        return None;
    }
    VARIANT_MARKERS
        .iter()
        .find(|(kind, marker)| {
            if !t.ends_with(marker) {
                return false;
            }
            // The bare "extended" marker is only a valid variant when it sits
            // in a parenthesised/bracketed suffix (or is the whole title).
            // Requiring an opening delimiter rejects false positives such as
            // "Mix Extended", which ends in "extended" but is not a variant.
            if *kind == VariantKind::ExtendedMix && *marker == "extended" {
                let before = t.strip_suffix(marker).unwrap_or(&t).trim_end();
                return before.is_empty() || before.ends_with('(') || before.ends_with('[');
            }
            true
        })
        .map(|(kind, marker)| (*kind, *marker))
}

/// Classify the variant suffix on a track title.
///
/// Handles the common real-world shapes: bare (`"Rider Extended Mix"`),
/// parenthesised (`"Rider (Extended Mix)"`), bracketed
/// (`"Rider [Extended Mix]"`), and artist-in-parens
/// (`"Rider (Pavel Khvaleev & Miss Monique Extended Mix)"`). Matching is
/// case-insensitive.
pub fn detect_variant_kind(title: &str) -> VariantKind {
    match_variant(title)
        .map(|(kind, _)| kind)
        .unwrap_or(VariantKind::None)
}

/// Whether `title` is itself the Extended Mix version of a release.
pub fn is_extended_mix(title: &str) -> bool {
    detect_variant_kind(title) == VariantKind::ExtendedMix
}

/// Strip the variant suffix (and its surrounding delimiters) from a title so
/// that all versions of the same release collapse to the same key.
///
/// Example: `"Rider (Extended Mix)"`, `"Rider - Original Mix"` and
/// `"Rider (Radio Edit)"` all normalise to `"rider"`.
pub fn base_title(title: &str) -> String {
    let t = normalized(title);
    match match_variant(title) {
        Some((_, marker)) => t
            .strip_suffix(marker)
            .unwrap_or(&t)
            .trim_end()
            .trim_end_matches(|c: char| {
                c == '(' || c == '[' || c == '-' || c == '\u{2013}' || c == '\u{2014}' || c == ':'
            })
            .trim_end()
            .to_string(),
        None => t,
    }
}

/// Index of the first Extended Mix in a list of version titles, if any.
pub fn find_extended_mix(titles: &[&str]) -> Option<usize> {
    titles.iter().position(|t| is_extended_mix(t))
}

/// Decide whether a release should be upgraded to its Extended Mix.
///
/// `owned` is the list of version titles the library already holds for a
/// release; `available` is the list of titles the source offers. Returns the
/// Extended Mix title to switch to, or `None`.
///
/// The decision implements the issue's no-loop/no-downgrade rule:
///
/// - **Already own an Extended Mix** → `None` (never downgrade or replace an
///   Extended Mix with another version).
/// - **No Extended Mix offered by the source** → `None`.
/// - Otherwise → the Extended Mix title, even if a shorter version is also
///   still present in `owned`.
pub fn upgrade_target_for_release(owned: &[&str], available: &[&str]) -> Option<String> {
    if owned.iter().any(|t| is_extended_mix(t)) {
        return None;
    }
    let idx = find_extended_mix(available)?;
    Some(available[idx].to_string())
}

// ── Library-scan candidate detection ───────────────────────────────────────

/// A single version of a release as observed in the source catalog
/// (`service_tracks`), plus whether the library already owns it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackVersion {
    pub track_id: i64,
    pub title: String,
    pub artist: String,
    pub owned: bool,
}

/// A release whose shorter version is owned but whose Extended Mix is
/// available and not yet owned — i.e. an auto-upgrade candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeCandidate {
    /// Normalised release key (base title) used for grouping.
    pub release_key: String,
    /// Original (display) title of an owned shorter version.
    pub owned_title: String,
    /// Original (display) artist.
    pub artist: String,
    /// The Extended Mix track available at the source.
    pub extended_track_id: i64,
    /// Original (display) title of the Extended Mix.
    pub extended_title: String,
}

/// Group the source's versions of every release and report auto-upgrade
/// candidates: a shorter version is owned, an Extended Mix is available but
/// not owned, and no Extended Mix is already owned.
///
/// Implements the issue's no-loop/no-downgrade rule at the library scale.
/// Versions of the same release are grouped by [`base_title`] plus the
/// lower-cased artist, so `"Rider"`, `"Rider (Radio Edit)"` and
/// `"Rider (Extended Mix)"` all collapse to the same group.
pub fn find_upgrade_candidates(versions: &[TrackVersion]) -> Vec<UpgradeCandidate> {
    use std::collections::BTreeMap;

    // Group by (base_title, lower-cased artist).
    let mut groups: BTreeMap<(String, String), Vec<&TrackVersion>> = BTreeMap::new();
    for v in versions {
        let key = (base_title(&v.title), v.artist.trim().to_lowercase());
        groups.entry(key).or_default().push(v);
    }

    let mut candidates = Vec::new();
    for ((base, artist_norm), members) in groups {
        let extended: Vec<&TrackVersion> = members
            .iter()
            .copied()
            .filter(|v| is_extended_mix(&v.title))
            .collect();
        if extended.is_empty() {
            continue;
        }
        // Already own an Extended Mix → never downgrade/replace.
        if extended.iter().any(|v| v.owned) {
            continue;
        }
        // Do we own a shorter version?
        let owned_short: Vec<&TrackVersion> = members
            .iter()
            .copied()
            .filter(|v| v.owned && !is_extended_mix(&v.title))
            .collect();
        if owned_short.is_empty() {
            continue;
        }
        // Deterministic Extended Mix target (lowest track_id).
        let target = extended.iter().copied().min_by_key(|v| v.track_id).unwrap();
        let owned_title = owned_short
            .iter()
            .map(|v| v.title.clone())
            .min()
            .unwrap_or_default();
        let artist = owned_short
            .iter()
            .map(|v| v.artist.clone())
            .min()
            .unwrap_or_else(|| artist_norm.clone());
        candidates.push(UpgradeCandidate {
            release_key: base,
            owned_title,
            artist,
            extended_track_id: target.track_id,
            extended_title: target.title.clone(),
        });
    }
    candidates.sort_by(|a, b| a.release_key.cmp(&b.release_key));
    candidates
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── detect_variant_kind ─────────────────────────────────────────────

    #[test]
    fn detects_bare_extended_mix() {
        assert_eq!(detect_variant_kind("Rider Extended Mix"), VariantKind::ExtendedMix);
        assert_eq!(detect_variant_kind("Rider extended mix"), VariantKind::ExtendedMix);
    }

    #[test]
    fn detects_parenthesised_and_bracketed() {
        assert_eq!(detect_variant_kind("Rider (Extended Mix)"), VariantKind::ExtendedMix);
        assert_eq!(detect_variant_kind("Rider [Extended Mix]"), VariantKind::ExtendedMix);
        assert_eq!(detect_variant_kind("Rider - Extended Mix"), VariantKind::ExtendedMix);
    }

    #[test]
    fn detects_artist_in_parens_before_extended_mix() {
        assert_eq!(
            detect_variant_kind("Rider (Pavel Khvaleev & Miss Monique Extended Mix)"),
            VariantKind::ExtendedMix
        );
    }

    #[test]
    fn detects_ext_mix_and_bare_extended() {
        assert_eq!(detect_variant_kind("Rider (Ext Mix)"), VariantKind::ExtendedMix);
        assert_eq!(detect_variant_kind("Rider (Extended)"), VariantKind::ExtendedMix);
    }

    #[test]
    fn detects_other_variant_kinds() {
        assert_eq!(detect_variant_kind("Rider (Radio Edit)"), VariantKind::RadioEdit);
        assert_eq!(detect_variant_kind("Rider (Original Mix)"), VariantKind::OriginalMix);
        assert_eq!(detect_variant_kind("Rider (Club Mix)"), VariantKind::ClubMix);
        assert_eq!(detect_variant_kind("Rider (Instrumental)"), VariantKind::Instrumental);
    }

    #[test]
    fn non_variant_titles_are_none() {
        assert_eq!(detect_variant_kind("Rider"), VariantKind::None);
        assert_eq!(detect_variant_kind(""), VariantKind::None);
        assert_eq!(detect_variant_kind("Mix Extended"), VariantKind::None);
    }

    // ── is_extended_mix ─────────────────────────────────────────────────

    #[test]
    fn is_extended_mix_true_and_false() {
        assert!(is_extended_mix("Rider (Extended Mix)"));
        assert!(is_extended_mix("Rider Extended Mix"));
        assert!(!is_extended_mix("Rider (Original Mix)"));
        assert!(!is_extended_mix("Rider (Radio Edit)"));
        assert!(!is_extended_mix("Rider"));
    }

    // ── base_title grouping ─────────────────────────────────────────────

    #[test]
    fn base_title_collapses_versions_of_same_release() {
        let a = base_title("Rider (Extended Mix)");
        let b = base_title("Rider - Original Mix");
        let c = base_title("Rider (Radio Edit)");
        let d = base_title("Rider");
        assert_eq!(a, "rider");
        assert_eq!(b, "rider");
        assert_eq!(c, "rider");
        assert_eq!(d, "rider");
    }

    #[test]
    fn base_title_strips_delimiters() {
        assert_eq!(base_title("Rider - Extended Mix"), "rider");
        assert_eq!(base_title("Rider (Ext Mix)"), "rider");
        assert_eq!(base_title("Rider [Extended Mix]"), "rider");
    }

    #[test]
    fn base_title_leaves_unmatched_titles_untouched() {
        assert_eq!(base_title("Rider"), "rider");
        assert_eq!(base_title("  Rider  "), "rider");
    }

    // ── find_extended_mix ───────────────────────────────────────────────

    #[test]
    fn finds_extended_mix_in_list() {
        let titles = ["Rider (Original Mix)", "Rider (Radio Edit)", "Rider (Extended Mix)"];
        assert_eq!(find_extended_mix(&titles), Some(2));
    }

    #[test]
    fn no_extended_mix_in_list() {
        let titles = ["Rider (Original Mix)", "Rider (Radio Edit)"];
        assert_eq!(find_extended_mix(&titles), None);
        assert_eq!(find_extended_mix(&[]), None);
    }

    // ── upgrade_target_for_release ──────────────────────────────────────

    #[test]
    fn upgrades_shorter_version_to_extended_mix() {
        let owned = ["Rider (Original Mix)"];
        let available = ["Rider (Original Mix)", "Rider (Extended Mix)"];
        assert_eq!(
            upgrade_target_for_release(&owned, &available),
            Some("Rider (Extended Mix)".to_string())
        );
    }

    #[test]
    fn no_upgrade_when_already_own_extended_mix() {
        // Already own the Extended Mix → never downgrade/replace.
        let owned = ["Rider (Extended Mix)"];
        let available = ["Rider (Original Mix)", "Rider (Extended Mix)"];
        assert_eq!(upgrade_target_for_release(&owned, &available), None);
    }

    #[test]
    fn no_upgrade_when_source_has_no_extended_mix() {
        let owned = ["Rider (Original Mix)"];
        let available = ["Rider (Original Mix)", "Rider (Radio Edit)"];
        assert_eq!(upgrade_target_for_release(&owned, &available), None);
    }

    #[test]
    fn no_upgrade_when_owned_contains_any_extended_mix() {
        // Even if the library holds several versions, owning *any* Extended
        // Mix is enough to suppress the upgrade (no loops).
        let owned = ["Rider (Original Mix)", "Rider (Extended Mix)"];
        let available = ["Rider (Extended Mix)"];
        assert_eq!(upgrade_target_for_release(&owned, &available), None);
    }

    // ── find_upgrade_candidates ─────────────────────────────────────────

    fn ver(id: i64, title: &str, artist: &str, owned: bool) -> TrackVersion {
        TrackVersion {
            track_id: id,
            title: title.to_string(),
            artist: artist.to_string(),
            owned,
        }
    }

    #[test]
    fn finds_candidate_when_shorter_owned_and_extended_available() {
        let versions = vec![
            ver(1, "Rider (Radio Edit)", "Pavel Khvaleev", true),
            ver(2, "Rider (Extended Mix)", "Pavel Khvaleev", false),
        ];
        let candidates = find_upgrade_candidates(&versions);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].release_key, "rider");
        assert_eq!(candidates[0].owned_title, "Rider (Radio Edit)");
        assert_eq!(candidates[0].extended_track_id, 2);
        assert_eq!(candidates[0].extended_title, "Rider (Extended Mix)");
    }

    #[test]
    fn candidate_groups_across_variant_suffixes() {
        // Plain "Rider" and "Rider (Extended Mix)" belong to the same release.
        let versions = vec![
            ver(1, "Rider", "Pavel Khvaleev", true),
            ver(2, "Rider (Extended Mix)", "Pavel Khvaleev", false),
        ];
        let candidates = find_upgrade_candidates(&versions);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].owned_title, "Rider");
    }

    #[test]
    fn no_candidate_when_extended_already_owned() {
        let versions = vec![
            ver(1, "Rider (Radio Edit)", "Pavel Khvaleev", true),
            ver(2, "Rider (Extended Mix)", "Pavel Khvaleev", true),
        ];
        assert!(find_upgrade_candidates(&versions).is_empty());
    }

    #[test]
    fn no_candidate_when_nothing_shorter_owned() {
        // Extended Mix exists but we own no shorter version.
        let versions = vec![ver(2, "Rider (Extended Mix)", "Pavel Khvaleev", false)];
        assert!(find_upgrade_candidates(&versions).is_empty());
    }

    #[test]
    fn no_candidate_when_no_extended_mix_available() {
        let versions = vec![
            ver(1, "Rider (Radio Edit)", "Pavel Khvaleev", true),
            ver(2, "Rider (Original Mix)", "Pavel Khvaleev", false),
        ];
        assert!(find_upgrade_candidates(&versions).is_empty());
    }

    #[test]
    fn candidates_are_grouped_by_base_title_and_artist() {
        let versions = vec![
            ver(1, "Rider (Radio Edit)", "Pavel Khvaleev", true),
            ver(2, "Rider (Extended Mix)", "Pavel Khvaleev", false),
            ver(3, "Rider (Extended Mix)", "Different Artist", false),
        ];
        // Only the same-artist group yields a candidate.
        let candidates = find_upgrade_candidates(&versions);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].artist, "Pavel Khvaleev");
    }
}
