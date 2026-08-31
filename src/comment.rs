#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Represents a parsed comment with structured data
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedComment {
    /// Phase indicator: 'P' if present, '_' if missing
    pub phase: char,
    /// Mood indicator: 'M' if present, '_' if missing
    pub mood: char,
    /// Vibe indicator: 'V' if present, '_' if missing
    pub vibe: char,
    /// Set of tags (excluding source ID)
    pub tags: HashSet<String>,
    /// Source IDs in format "sp:xxx", "sc:xxx", "yt:xxx" (multiple allowed)
    pub source_ids: Vec<String>,
}

impl ParsedComment {
    /// Create a new empty parsed comment
    pub fn empty() -> Self {
        Self {
            phase: '_',
            mood: '_',
            vibe: '_',
            tags: HashSet::new(),
            source_ids: Vec::new(),
        }
    }
}

/// Parse a comment string into structured data
/// Format: [{phase_char}{mood_char}{vibe_char}] {tags} {source_id}
/// Example: "[PMV] house sunny sp:123456789"
pub fn parse_comment(comment: &str) -> Option<ParsedComment> {
    if comment.trim().is_empty() {
        return Some(ParsedComment::empty());
    }

    // Try to extract PMV indicators from bracket format
    let trimmed = comment.trim();
    if let Some(bracket_end) = trimmed.find(']')
        && trimmed.starts_with('[')
        && bracket_end >= 4
    {
        let pmv_str = &trimmed[1..bracket_end];
        if pmv_str.len() == 3 {
            let phase = pmv_str.chars().next()?;
            let mood = pmv_str.chars().nth(1)?;
            let vibe = pmv_str.chars().nth(2)?;

            // Validate PMV characters
            if !is_valid_pmv_char(phase) || !is_valid_pmv_char(mood) || !is_valid_pmv_char(vibe) {
                return None;
            }

            let after_bracket = &trimmed[bracket_end + 1..].trim();
            return parse_tags_and_source_id(after_bracket, phase, mood, vibe);
        }
    }

    // If no bracket format, try to parse as tags only
    parse_tags_and_source_id(trimmed, '_', '_', '_')
}

/// Quote-aware tokenizer: splits on whitespace but respects double-quoted strings.
/// Quoted tokens have their quotes stripped and escape sequences (`\"`, `\\`) resolved.
/// Example: `hello "zu späßen" world` → ["hello", "zu späßen", "world"]
fn split_quoted_tokens(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = input.chars();

    while let Some(c) = chars.next() {
        match c {
            '"' if !in_quotes => {
                in_quotes = true;
            }
            '"' if in_quotes => {
                in_quotes = false;
            }
            '\\' if in_quotes => match chars.next() {
                Some(next) => current.push(next),
                None => current.push(c),
            },
            _ if c.is_whitespace() && !in_quotes => {
                if !current.is_empty() {
                    tokens.push(current.clone());
                    current.clear();
                }
            }
            _ => {
                current.push(c);
            }
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }
    tokens
}

/// Parse the tags and source ID part of a comment
fn parse_tags_and_source_id(
    content: &str,
    phase: char,
    mood: char,
    vibe: char,
) -> Option<ParsedComment> {
    let parts = split_quoted_tokens(content);
    if parts.is_empty() {
        return Some(ParsedComment {
            phase,
            mood,
            vibe,
            tags: HashSet::new(),
            source_ids: Vec::new(),
        });
    }

    // Collect tags and source IDs
    let mut tags = HashSet::new();
    let mut source_ids = Vec::new();

    for part in &parts {
        if is_source_id(part) {
            source_ids.push(part.clone());
        } else {
            tags.insert(part.to_lowercase());
        }
    }

    // If we found source IDs but the last part isn't a source ID, we might have misparsed
    // In that case, treat everything as tags
    if !source_ids.is_empty() && !is_source_id(&parts[parts.len() - 1]) {
        // Reset and treat all as tags
        tags.clear();
        source_ids.clear();
        for part in &parts {
            tags.insert(part.to_lowercase());
        }
    }

    Some(ParsedComment {
        phase,
        mood,
        vibe,
        tags,
        source_ids,
    })
}

/// Quote a tag name if it contains spaces or quotes.
/// Tags with spaces are wrapped in `"..."`.
/// Existing double quotes inside a tag are escaped as `\"`.
/// Example: `zu späßen aufgelegt` → `"zu späßen aufgelegt"`
/// Example: `hello` → `hello`
/// Example: `hello "world"` → `"hello \"world\""`
fn quote_tag(tag: &str) -> String {
    if tag.contains(' ') || tag.contains('"') {
        let escaped = tag.replace('\\', "\\\\").replace('"', "\\\"");
        format!("\"{}\"", escaped)
    } else {
        tag.to_string()
    }
}

/// Generate a comment string from structured data
pub fn generate_comment(parsed: &ParsedComment) -> String {
    let pmv_str = format!("{}{}{}", parsed.phase, parsed.mood, parsed.vibe);

    let mut parts = Vec::new();
    let mut tags: Vec<String> = parsed.tags.iter().cloned().collect();
    tags.sort();
    for tag in tags {
        parts.push(quote_tag(&tag));
    }

    // Add all source IDs
    for source_id in &parsed.source_ids {
        parts.push(source_id.clone());
    }

    if pmv_str == "___" && parts.is_empty() {
        String::new()
    } else {
        format!("[{}] {}", pmv_str, parts.join(" "))
    }
}

/// Generate a comment from PMV indicators and tags
/// If multiple source IDs are provided, they will be included in the comment
pub fn generate_from_parts(
    phase: char,
    mood: char,
    vibe: char,
    tags: &[String],
    source_ids: &[String],
) -> String {
    let parsed = ParsedComment {
        phase: validate_pmv_char(phase),
        mood: validate_pmv_char(mood),
        vibe: validate_pmv_char(vibe),
        tags: tags.iter().map(|s| s.to_lowercase()).collect(),
        source_ids: source_ids.iter().map(|s| s.to_string()).collect(),
    };

    generate_comment(&parsed)
}

/// Generate a comment from PMV indicators and tags (backward compatibility)
pub fn generate_from_parts_single(
    phase: char,
    mood: char,
    vibe: char,
    tags: &[String],
    source_id: Option<&str>,
) -> String {
    let source_ids: Vec<String> = source_id.map(|s| s.to_string()).into_iter().collect();
    generate_from_parts(phase, mood, vibe, tags, &source_ids)
}

/// Extract tags from a comment string (for backward compatibility)
pub fn extract_tags_from_comment(comment: &str) -> HashSet<String> {
    match parse_comment(comment) {
        Some(parsed) => parsed.tags,
        None => HashSet::new(),
    }
}

/// Extract PMV indicators from a comment string
pub fn extract_pmv_from_comment(comment: &str) -> (char, char, char) {
    match parse_comment(comment) {
        Some(parsed) => (parsed.phase, parsed.mood, parsed.vibe),
        None => ('_', '_', '_'),
    }
}

/// Extract source ID from a comment string
pub fn extract_source_id_from_comment(comment: &str) -> Option<String> {
    parse_comment(comment).and_then(|p| p.source_ids.first().cloned())
}

/// Check if a string is a valid source ID
pub fn is_source_id(s: &str) -> bool {
    if s.len() < 4 {
        return false;
    }

    let prefix = &s[0..3];
    matches!(prefix, "sp:" | "sc:" | "yt:")
}

/// Get service type from source ID
pub fn get_service_from_source_id(source_id: &str) -> Option<&str> {
    if !is_source_id(source_id) {
        return None;
    }

    match &source_id[0..3] {
        "sp:" => Some("spotify"),
        "sc:" => Some("soundcloud"),
        "yt:" => Some("youtube"),
        _ => None,
    }
}

/// Get the ID part from source ID (without prefix)
pub fn get_id_from_source_id(source_id: &str) -> Option<&str> {
    if !is_source_id(source_id) || source_id.len() <= 3 {
        return None;
    }

    Some(&source_id[3..])
}

/// Check if a character is valid for PMV indicators
fn is_valid_pmv_char(c: char) -> bool {
    c == 'P' || c == 'M' || c == 'V' || c == '_'
}

/// Validate and correct a PMV character
fn validate_pmv_char(c: char) -> char {
    if is_valid_pmv_char(c) { c } else { '_' }
}

/// Create a source ID string for a service
pub fn create_source_id(service: &str, id: &str) -> String {
    let prefix = match service.to_lowercase().as_str() {
        "spotify" => "sp:",
        "soundcloud" => "sc:",
        "youtube" => "yt:",
        _ => "unk:",
    };

    format!("{}{}", prefix, id)
}

/// Generate a target comment with all available service IDs
/// Format: [{phase_char}{mood_char}{vibe_char}] {tags} {source_id1} {source_id2} ...
/// If multiple service IDs exist, they will be included in this order: spotify, soundcloud, youtube
pub fn generate_target_comment(
    phase: char,
    mood: char,
    vibe: char,
    tags: &[String],
    spotify_id: Option<&str>,
    soundcloud_id: Option<&str>,
    youtube_id: Option<&str>,
) -> String {
    let mut source_ids = Vec::new();

    if let Some(id) = spotify_id {
        source_ids.push(create_source_id("spotify", id));
    }
    if let Some(id) = soundcloud_id {
        source_ids.push(create_source_id("soundcloud", id));
    }
    if let Some(id) = youtube_id {
        source_ids.push(create_source_id("youtube", id));
    }

    generate_from_parts(phase, mood, vibe, tags, &source_ids)
}

/// Generate a target comment preferring a specific service
/// If the preferred service ID is available, use it; otherwise use all available
#[allow(clippy::too_many_arguments)]
pub fn generate_target_comment_with_preference(
    phase: char,
    mood: char,
    vibe: char,
    tags: &[String],
    spotify_id: Option<&str>,
    soundcloud_id: Option<&str>,
    youtube_id: Option<&str>,
    preferred_service: &str,
) -> String {
    // Check preferred service first
    let preferred_id = match preferred_service {
        "spotify" => spotify_id,
        "soundcloud" => soundcloud_id,
        "youtube" => youtube_id,
        _ => None,
    };

    if let Some(id) = preferred_id {
        let source_id = create_source_id(preferred_service, id);
        return generate_from_parts_single(phase, mood, vibe, tags, Some(&source_id));
    }

    // Fallback to all available
    generate_target_comment(
        phase,
        mood,
        vibe,
        tags,
        spotify_id,
        soundcloud_id,
        youtube_id,
    )
}

/// Extract all source IDs from a comment (multiple may be present)
pub fn extract_all_source_ids_from_comment(comment: &str) -> Vec<String> {
    if comment.trim().is_empty() {
        return Vec::new();
    }

    let mut source_ids = Vec::new();
    let parts = split_quoted_tokens(comment);

    for part in &parts {
        if is_source_id(part) {
            source_ids.push(part.clone());
        }
    }

    source_ids
}

// ============================================================================
// Comment Diff & Fingerprint (tag roundtrip inbox)
// ============================================================================

/// Structured diff between the canonical DB comment and the on-disk comment.
/// Works on the PARSED structure (phase/mood/vibe + tags + source_ids), never
/// on the raw string alone.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommentDiff {
    /// Tags present on disk but missing in DB (user added them in Traktor).
    pub tags_added: Vec<String>,
    /// Tags present in DB but missing on disk (user removed them in Traktor).
    pub tags_removed: Vec<String>,
    /// Phase indicator change: (db_char, disk_char).
    pub phase_changed: Option<(char, char)>,
    /// Mood indicator change: (db_char, disk_char).
    pub mood_changed: Option<(char, char)>,
    /// Vibe indicator change: (db_char, disk_char).
    pub vibe_changed: Option<(char, char)>,
    /// Source IDs present on disk but missing in DB.
    pub source_ids_added: Vec<String>,
    /// Source IDs present in DB but missing on disk.
    pub source_ids_removed: Vec<String>,
    /// True when the disk comment could not be parsed AND its raw text differs
    /// from the canonical DB comment. In that case the delta cannot be expressed
    /// structurally — the raw disk comment is offered as a `comment` entry.
    pub raw_comment_changed: bool,
}

impl CommentDiff {
    /// A diff is empty when disk and DB represent the same parsed comment.
    pub fn is_empty(&self) -> bool {
        self.tags_added.is_empty()
            && self.tags_removed.is_empty()
            && self.phase_changed.is_none()
            && self.mood_changed.is_none()
            && self.vibe_changed.is_none()
            && self.source_ids_added.is_empty()
            && self.source_ids_removed.is_empty()
            && !self.raw_comment_changed
    }

    /// Total number of discrete deltas (used for logging/UI counts).
    pub fn delta_count(&self) -> usize {
        self.tags_added.len()
            + self.tags_removed.len()
            + self.phase_changed.map(|_| 1).unwrap_or(0)
            + self.mood_changed.map(|_| 1).unwrap_or(0)
            + self.vibe_changed.map(|_| 1).unwrap_or(0)
            + self.source_ids_added.len()
            + self.source_ids_removed.len()
            + usize::from(self.raw_comment_changed)
    }
}

fn char_diff(db: char, disk: char) -> Option<(char, char)> {
    if db == disk {
        None
    } else {
        Some((db, disk))
    }
}

/// Diff two parsed comments: canonical DB state vs. on-disk state.
/// `db` is the canonical comment, `disk` is what Traktor wrote to the file.
pub fn diff_comments(db: &ParsedComment, disk: &ParsedComment) -> CommentDiff {
    let tags_added: Vec<String> = disk
        .tags
        .difference(&db.tags)
        .cloned()
        .collect::<Vec<_>>();
    let tags_removed: Vec<String> = db
        .tags
        .difference(&disk.tags)
        .cloned()
        .collect::<Vec<_>>();

    let source_ids_added: Vec<String> = disk
        .source_ids
        .iter()
        .filter(|id| !db.source_ids.contains(id))
        .cloned()
        .collect();
    let source_ids_removed: Vec<String> = db
        .source_ids
        .iter()
        .filter(|id| !disk.source_ids.contains(id))
        .cloned()
        .collect();

    CommentDiff {
        tags_added: sorted_vec(tags_added),
        tags_removed: sorted_vec(tags_removed),
        phase_changed: char_diff(db.phase, disk.phase),
        mood_changed: char_diff(db.mood, disk.mood),
        vibe_changed: char_diff(db.vibe, disk.vibe),
        source_ids_added: sorted_vec(source_ids_added),
        source_ids_removed: sorted_vec(source_ids_removed),
        raw_comment_changed: false,
    }
}

/// Diff canonical DB comment vs. on-disk comment by raw strings.
///
/// - Both parseable → structural `diff_comments`.
/// - Disk unparseable & raw differs from the canonical DB comment string
///   → `raw_comment_changed = true` (candidate = raw disk text).
/// - Empty/Nothing → empty diff.
pub fn diff_comment_strings(db: Option<&str>, disk: Option<&str>) -> CommentDiff {
    // A missing/empty comment is the canonical empty state — parseable.
    // Only genuinely unparseable raw strings fall through to the raw path.
    let db_parsed = match db {
        Some(s) => parse_comment(s),
        None => Some(ParsedComment::empty()),
    };
    let disk_parsed = match disk {
        Some(s) => parse_comment(s),
        None => Some(ParsedComment::empty()),
    };

    match (db_parsed, disk_parsed) {
        (Some(db_p), Some(disk_p)) => diff_comments(&db_p, &disk_p),
        // One side unparseable: fall back to raw comparison. If the raw text
        // differs, the disk side is offered as an opaque `comment` delta.
        (db_p, disk_p) => {
            let db_raw = db.map(|s| s.trim()).unwrap_or("");
            let disk_raw = disk.map(|s| s.trim()).unwrap_or("");
            let structurally_same = db_p.is_some() && disk_p.is_none() && db_raw == disk_raw;
            let mut diff = CommentDiff::default();
            diff.raw_comment_changed = db_raw != disk_raw && !structurally_same;
            diff
        }
    }
}

fn sorted_vec(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

/// Stable fingerprint of a comment: SHA-256 over the canonical regenerated
/// form (`parse → generate`). Formatting/ordering differences produce the same
/// fingerprint. Unparseable comments are hashed as their trimmed raw text.
pub fn comment_fingerprint(comment: &str) -> String {
    use sha2::{Digest, Sha256};

    let canonical = match parse_comment(comment) {
        Some(parsed) => generate_comment(&parsed),
        None => comment.trim().to_string(),
    };

    let mut hasher = Sha256::new();
    hasher.update(canonical.as_bytes());
    let digest = hasher.finalize();
    // Hex digest, truncated to 32 chars for compact storage.
    let hex: String = digest.iter().map(|b| format!("{:02x}", b)).collect();
    hex[..32].to_string()
}

/// Fingerprint of a `None`/empty comment (the canonical empty state).
pub fn comment_fingerprint_opt(comment: Option<&str>) -> String {
    comment_fingerprint(comment.unwrap_or(""))
}

// ============================================================================
// Fuzzy tag matching & tag-inbox mapping application (full feature set)
// ============================================================================

/// Case-insensitive Levenshtein distance between two strings.
///
/// Used for the inbox's similar-tag suggestions: a new/typo tag is matched
/// against the existing tag vocabulary with distance ≤ 2 (spec default).
/// Non-ASCII characters are compared by their lowercase form (char-wise).
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    let b: Vec<char> = b
        .chars()
        .map(|c| c.to_lowercase().next().unwrap_or(c))
        .collect();
    let (la, lb) = (a.len(), b.len());
    if la == 0 {
        return lb;
    }
    if lb == 0 {
        return la;
    }

    let mut prev: Vec<usize> = (0..=lb).collect();
    let mut curr = vec![0usize; lb + 1];
    for i in 1..=la {
        curr[0] = i;
        for j in 1..=lb {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[lb]
}

/// Fuzzy-match `tag` against a list of existing tag names.
///
/// Criteria (spec default): case-insensitive Levenshtein distance ≤
/// `max_distance`, excluding the tag itself (distance 0 — it already exists
/// canonically and is not a typo of itself). Returns `(existing_tag, distance)`
/// sorted by distance ascending, then name ascending.
pub fn similar_tags(tag: &str, existing: &[String], max_distance: usize) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    for candidate in existing {
        let d = levenshtein_distance(tag, candidate);
        if d == 0 || d > max_distance {
            continue;
        }
        out.push((candidate.clone(), d));
    }
    out.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Apply open tag-inbox mappings (raw tag → canonical tag) to a generated
/// target comment.
///
/// Staging semantics: resolving a new/typo tag in the inbox only records a
/// decision (`tag_inbox` row). This function is what makes the canonical
/// spelling appear on the next write:
///
/// 1. Every tag in the generated target matching a mapped raw tag is replaced
///    by the mapped canonical tag.
/// 2. Every tag in the STORED comment matching a mapped raw tag is ADDED to
///    the target with its canonical spelling — a typed tag the user decided to
///    keep/rename/merge is written instead of being silently dropped.
///
/// Tags without a mapping are left untouched (no auto-apply). Mappings are
/// keyed by lowercase tag name. Returns the input unchanged when no mappings
/// exist or the target is unparseable.
pub fn apply_tag_mappings_to_target(
    target: &str,
    stored: Option<&str>,
    mappings: &std::collections::HashMap<String, String>,
) -> String {
    if mappings.is_empty() {
        return target.to_string();
    }
    let Some(mut parsed) = parse_comment(target) else {
        return target.to_string();
    };

    // 1. Replace mapped tags present in the generated target.
    let mut tags: HashSet<String> = HashSet::new();
    for t in &parsed.tags {
        match mappings.get(&t.to_lowercase()) {
            Some(canonical) => {
                tags.insert(canonical.clone());
            }
            None => {
                tags.insert(t.clone());
            }
        }
    }

    // 2. Add canonical spellings for stored tags the user has mapped.
    if let Some(stored_str) = stored
        && let Some(stored_parsed) = parse_comment(stored_str)
    {
        for t in &stored_parsed.tags {
            if let Some(canonical) = mappings.get(&t.to_lowercase()) {
                tags.insert(canonical.clone());
            }
        }
    }

    parsed.tags = tags;
    generate_comment(&parsed)
}

/// Get a specific service ID from a comment (e.g., "spotify")
pub fn get_service_id_from_comment(comment: &str, service: &str) -> Option<String> {
    let source_ids = extract_all_source_ids_from_comment(comment);

    for source_id in source_ids {
        if let Some(svc) = get_service_from_source_id(&source_id)
            && svc == service
        {
            return Some(source_id);
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_full_comment() {
        let comment = "[PMV] house sunny sp:123456789";
        let parsed = parse_comment(comment).unwrap();

        assert_eq!(parsed.phase, 'P');
        assert_eq!(parsed.mood, 'M');
        assert_eq!(parsed.vibe, 'V');
        assert!(parsed.tags.contains("house"));
        assert!(parsed.tags.contains("sunny"));
        assert_eq!(parsed.tags.len(), 2);
        assert_eq!(parsed.source_ids, vec!["sp:123456789".to_string()]);
    }

    #[test]
    fn test_parse_partial_pmv() {
        let comment = "[P_V] techno peak yt:dQw4w9WgXcQ";
        let parsed = parse_comment(comment).unwrap();

        assert_eq!(parsed.phase, 'P');
        assert_eq!(parsed.mood, '_');
        assert_eq!(parsed.vibe, 'V');
        assert!(parsed.tags.contains("techno"));
        assert!(parsed.tags.contains("peak"));
        assert_eq!(parsed.source_ids, vec!["yt:dQw4w9WgXcQ".to_string()]);
    }

    #[test]
    fn test_parse_no_pmv() {
        let comment = "house edit";
        let parsed = parse_comment(comment).unwrap();

        assert_eq!(parsed.phase, '_');
        assert_eq!(parsed.mood, '_');
        assert_eq!(parsed.vibe, '_');
        assert!(parsed.tags.contains("house"));
        assert!(parsed.tags.contains("edit"));
        assert!(parsed.source_ids.is_empty());
    }

    #[test]
    fn test_parse_empty() {
        let comment = "";
        let parsed = parse_comment(comment).unwrap();

        assert_eq!(parsed.phase, '_');
        assert_eq!(parsed.mood, '_');
        assert_eq!(parsed.vibe, '_');
        assert!(parsed.tags.is_empty());
        assert!(parsed.source_ids.is_empty());
    }

    #[test]
    fn test_generate_comment() {
        let parsed = ParsedComment {
            phase: 'P',
            mood: '_',
            vibe: 'V',
            tags: vec!["house".to_string(), "sunny".to_string()]
                .into_iter()
                .collect(),
            source_ids: vec!["sp:123456789".to_string()],
        };

        let generated = generate_comment(&parsed);
        // Note: tags might be in different order
        assert!(generated.starts_with("[P_V]"));
        assert!(generated.contains("house"));
        assert!(generated.contains("sunny"));
        // With multiple source IDs possible, just check it contains the source ID
        assert!(generated.contains("sp:123456789"));
    }

    #[test]
    fn test_extract_tags() {
        let comment = "[PMV] house sunny sp:123456789";
        let tags = extract_tags_from_comment(comment);

        assert_eq!(tags.len(), 2);
        assert!(tags.contains("house"));
        assert!(tags.contains("sunny"));
        assert!(!tags.contains("sp:123456789"));
    }

    #[test]
    fn test_is_source_id() {
        assert!(is_source_id("sp:123456789"));
        assert!(is_source_id("sc:890123"));
        assert!(is_source_id("yt:dQw4w9WgXcQ"));
        assert!(!is_source_id("spotify:123"));
        assert!(!is_source_id("house"));
    }

    #[test]
    fn test_create_source_id() {
        assert_eq!(create_source_id("spotify", "123456789"), "sp:123456789");
        assert_eq!(create_source_id("soundcloud", "890123"), "sc:890123");
        assert_eq!(create_source_id("youtube", "dQw4w9WgXcQ"), "yt:dQw4w9WgXcQ");
        assert_eq!(create_source_id("unknown", "123"), "unk:123");
    }

    #[test]
    fn test_get_service_from_source_id() {
        assert_eq!(get_service_from_source_id("sp:123"), Some("spotify"));
        assert_eq!(get_service_from_source_id("sc:456"), Some("soundcloud"));
        assert_eq!(get_service_from_source_id("yt:789"), Some("youtube"));
        assert_eq!(get_service_from_source_id("invalid"), None);
    }

    #[test]
    fn test_generate_from_parts() {
        let tags = vec!["house".to_string(), "sunny".to_string()];
        let source_ids = vec!["sp:123456789".to_string()];
        let comment = generate_from_parts('P', 'M', 'V', &tags, &source_ids);

        assert!(comment.starts_with("[PMV]"));
        assert!(comment.contains("house"));
        assert!(comment.contains("sunny"));
        // With multiple source IDs possible, just check it contains the source ID
        assert!(comment.contains("sp:123456789"));
    }

    // ── Quote-aware tokenizer tests ──────────────────────────────────────

    #[test]
    fn test_split_quoted_tokens_simple() {
        let tokens = split_quoted_tokens("hello world");
        assert_eq!(tokens, vec!["hello", "world"]);
    }

    #[test]
    fn test_split_quoted_tokens_quoted() {
        let tokens = split_quoted_tokens("hello \"zu späßen\" world");
        assert_eq!(tokens, vec!["hello", "zu späßen", "world"]);
    }

    #[test]
    fn test_split_quoted_tokens_all_quoted() {
        let tokens = split_quoted_tokens("\"zu späßen aufgelegt\"");
        assert_eq!(tokens, vec!["zu späßen aufgelegt"]);
    }

    #[test]
    fn test_split_quoted_tokens_leading_trailing() {
        let tokens = split_quoted_tokens("  \"multi word\"  simple  ");
        assert_eq!(tokens, vec!["multi word", "simple"]);
    }

    #[test]
    fn test_split_quoted_tokens_empty() {
        let tokens = split_quoted_tokens("");
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_split_quoted_tokens_only_whitespace() {
        let tokens = split_quoted_tokens("   ");
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_split_quoted_tokens_escaped_quote() {
        // Input: hello "say \"hi\"" world
        let tokens = split_quoted_tokens("hello \"say \\\"hi\\\"\" world");
        assert_eq!(tokens, vec!["hello", "say \"hi\"", "world"]);
    }

    #[test]
    fn test_split_quoted_tokens_escaped_backslash() {
        // Input: "back\\slash"
        let tokens = split_quoted_tokens("\"back\\\\slash\"");
        assert_eq!(tokens, vec!["back\\slash"]);
    }

    // ── quote_tag tests ──────────────────────────────────────────────────

    #[test]
    fn test_quote_tag_no_quoting_needed() {
        assert_eq!(quote_tag("hello"), "hello");
        assert_eq!(quote_tag("house"), "house");
        assert_eq!(quote_tag("droid"), "droid");
    }

    #[test]
    fn test_quote_tag_with_space() {
        assert_eq!(quote_tag("zu späßen aufgelegt"), "\"zu späßen aufgelegt\"");
    }

    #[test]
    fn test_quote_tag_with_embedded_quote() {
        // Tag contains a literal double quote → needs quoting + escaping
        assert_eq!(quote_tag("hello \"world\""), "\"hello \\\"world\\\"\"");
    }

    #[test]
    fn test_quote_tag_with_backslash() {
        // Tag contains a backslash → needs escaping
        assert_eq!(quote_tag("hello\\world"), "hello\\world");
    }

    #[test]
    fn test_quote_tag_with_space_and_backslash() {
        // Tag with space and backslash → needs both quoting and escaping
        assert_eq!(quote_tag("hello \\world test"), "\"hello \\\\world test\"");
    }

    // ── Parse with quoted tags tests ─────────────────────────────────────

    #[test]
    fn test_parse_quoted_tag_in_comment() {
        let comment = "[___] house \"zu späßen\" sp:abc123";
        let parsed = parse_comment(comment).unwrap();

        assert_eq!(parsed.phase, '_');
        assert_eq!(parsed.mood, '_');
        assert_eq!(parsed.vibe, '_');
        assert!(parsed.tags.contains("house"));
        assert!(parsed.tags.contains("zu späßen"));
        assert_eq!(parsed.tags.len(), 2);
        assert_eq!(parsed.source_ids, vec!["sp:abc123".to_string()]);
    }

    #[test]
    fn test_parse_multiword_tag_with_pmv() {
        let comment = "[P_V] \"zu späßen aufgelegt\"";
        let parsed = parse_comment(comment).unwrap();

        assert_eq!(parsed.phase, 'P');
        assert_eq!(parsed.mood, '_');
        assert_eq!(parsed.vibe, 'V');
        assert!(parsed.tags.contains("zu späßen aufgelegt"));
        assert_eq!(parsed.tags.len(), 1);
        assert!(parsed.source_ids.is_empty());
    }

    #[test]
    fn test_parse_only_quoted_tag() {
        let comment = "\"only this\"";
        let parsed = parse_comment(comment).unwrap();

        assert!(parsed.tags.contains("only this"));
        assert_eq!(parsed.tags.len(), 1);
        assert!(parsed.source_ids.is_empty());
    }

    // ── Generate with quoted tags tests ──────────────────────────────────

    #[test]
    fn test_generate_single_word_tags_no_quoting() {
        let comment = generate_from_parts('_', '_', '_', &["house".into(), "droid".into()], &[]);
        assert_eq!(comment, "[___] droid house");
    }

    #[test]
    fn test_generate_multiword_tag_gets_quoted() {
        let parsed = ParsedComment {
            phase: '_',
            mood: '_',
            vibe: '_',
            tags: vec!["house".into(), "zu späßen aufgelegt".into()]
                .into_iter()
                .collect(),
            source_ids: vec![],
        };
        let comment = generate_comment(&parsed);
        assert_eq!(comment, "[___] house \"zu späßen aufgelegt\"");
    }

    #[test]
    fn test_generate_multiword_tags_sorted() {
        let parsed = ParsedComment {
            phase: '_',
            mood: '_',
            vibe: '_',
            tags: vec![
                "zu späßen aufgelegt".into(),
                "droid".into(),
                "harmony liked".into(),
            ]
            .into_iter()
            .collect(),
            source_ids: vec![],
        };
        let comment = generate_comment(&parsed);
        // Sorted alphabetically: droid, harmony liked, zu späßen aufgelegt
        assert_eq!(
            comment,
            "[___] droid \"harmony liked\" \"zu späßen aufgelegt\""
        );
    }

    #[test]
    fn test_generate_multiword_with_source_ids() {
        let comment = generate_from_parts(
            '_',
            '_',
            '_',
            &["harmony liked".into(), "setlist25-130".into()],
            &["sp:abc".into()],
        );
        assert_eq!(comment, "[___] \"harmony liked\" setlist25-130 sp:abc");
    }

    // ── Round-trip tests ──────────────────────────────────────────────────

    #[test]
    fn test_roundtrip_simple_tags() {
        let tags = vec!["house".into(), "droid".into()];
        let comment = generate_from_parts('_', '_', '_', &tags, &[]);
        let parsed = parse_comment(&comment).unwrap();

        assert_eq!(parsed.phase, '_');
        assert_eq!(parsed.mood, '_');
        assert_eq!(parsed.vibe, '_');
        assert_eq!(parsed.tags.len(), 2);
        assert!(parsed.tags.contains("house"));
        assert!(parsed.tags.contains("droid"));
        assert!(parsed.source_ids.is_empty());
    }

    #[test]
    fn test_roundtrip_multiword_tags() {
        // This is the user's exact case:
        //   target comment: [___] dieses harmony liked setlist25-130 "zu späßen aufgelegt"
        let tags = vec![
            "dieses".into(),
            "harmony liked".into(),
            "setlist25-130".into(),
            "zu späßen aufgelegt".into(),
        ];
        let comment = generate_from_parts('_', '_', '_', &tags, &[]);
        assert_eq!(
            comment,
            "[___] dieses \"harmony liked\" setlist25-130 \"zu späßen aufgelegt\""
        );

        // Now parse it back
        let parsed = parse_comment(&comment).unwrap();
        assert!(parsed.tags.contains("dieses"));
        assert!(parsed.tags.contains("harmony liked"));
        assert!(parsed.tags.contains("setlist25-130"));
        assert!(parsed.tags.contains("zu späßen aufgelegt"));
        assert_eq!(parsed.tags.len(), 4);
    }

    #[test]
    fn test_roundtrip_with_source_ids() {
        let tags = vec!["house".into(), "zu späßen aufgelegt".into()];
        let comment =
            generate_from_parts('P', 'M', 'V', &tags, &["sp:abc".into(), "sc:def".into()]);
        // Generated: [PMV] house "zu späßen aufgelegt" sp:abc sc:def
        assert!(comment.starts_with("[PMV]"));

        let parsed = parse_comment(&comment).unwrap();
        assert_eq!(parsed.phase, 'P');
        assert_eq!(parsed.mood, 'M');
        assert_eq!(parsed.vibe, 'V');
        assert!(parsed.tags.contains("house"));
        assert!(parsed.tags.contains("zu späßen aufgelegt"));
        assert_eq!(parsed.tags.len(), 2);
        assert!(parsed.source_ids.contains(&"sp:abc".to_string()));
        assert!(parsed.source_ids.contains(&"sc:def".to_string()));
    }

    #[test]
    fn test_roundtrip_double_quote_in_tag() {
        // Tag: hello "world" (with literal double quotes)
        // Should round-trip: generate → parse → same tag
        let tags = vec!["hello \"world\"".into(), "simple".into()];
        let comment = generate_from_parts('_', '_', '_', &tags, &[]);
        // Generated: [___] "hello \"world\"" simple
        assert_eq!(comment, "[___] \"hello \\\"world\\\"\" simple");

        // Now parse it back
        let parsed = parse_comment(&comment).unwrap();
        assert!(parsed.tags.contains("hello \"world\""));
        assert!(parsed.tags.contains("simple"));
        assert_eq!(parsed.tags.len(), 2);
    }

    #[test]
    fn test_roundtrip_idempotent() {
        // Full round-trip: generate → parse → generate should produce identical result
        let original_tags = vec![
            "dieses".into(),
            "harmony liked".into(),
            "setlist25-130".into(),
            "zu späßen aufgelegt".into(),
        ];
        let original = generate_from_parts('_', '_', '_', &original_tags, &["sp:abc".into()]);

        let parsed = parse_comment(&original).unwrap();
        let regenerated = generate_comment(&parsed);

        assert_eq!(
            original, regenerated,
            "round-trip failed!\n  original:    {original}\n  regenerated: {regenerated}"
        );
    }

    // ── extract_all_source_ids_from_comment (quote-aware) ────────────────

    #[test]
    fn test_extract_all_source_ids_quoted_nearby() {
        // Source IDs next to quoted tags should still be found
        let comment = "[___] \"zu späßen\" sp:abc house sc:def";
        let ids = extract_all_source_ids_from_comment(comment);
        assert_eq!(ids, vec!["sp:abc", "sc:def"]);
    }

    #[test]
    fn test_extract_all_source_ids_all_quoted() {
        let comment = "\"only tags here\"";
        let ids = extract_all_source_ids_from_comment(comment);
        assert!(ids.is_empty());
    }

    // ── diff_comments tests ────────────────────────────────────────────────

    #[test]
    fn test_diff_comments_empty_when_identical() {
        let db = parse_comment("[PMV] house sunny sp:abc").unwrap();
        let disk = parse_comment("[PMV] house sunny sp:abc").unwrap();
        let diff = diff_comments(&db, &disk);
        assert!(diff.is_empty(), "identical comments must diff empty");
        assert_eq!(diff.delta_count(), 0);
    }

    #[test]
    fn test_diff_comments_empty_when_equal_up_to_order_and_case() {
        // Tags are lowercased + deduped by the parser; ordering is irrelevant.
        let db = parse_comment("[PMV] sunny house sp:abc").unwrap();
        let disk = parse_comment("[PMV] HOUSE sunny sp:abc").unwrap();
        let diff = diff_comments(&db, &disk);
        assert!(diff.is_empty());
    }

    #[test]
    fn test_diff_comments_tag_added_and_removed() {
        let db = parse_comment("[___] house sunny").unwrap();
        let disk = parse_comment("[___] house droid").unwrap();
        let diff = diff_comments(&db, &disk);
        assert_eq!(diff.tags_added, vec!["droid"]);
        assert_eq!(diff.tags_removed, vec!["sunny"]);
        assert!(diff.phase_changed.is_none());
        assert!(diff.source_ids_added.is_empty());
        assert_eq!(diff.delta_count(), 2);
    }

    #[test]
    fn test_diff_comments_pmv_changes() {
        let db = parse_comment("[___] house").unwrap();
        let disk = parse_comment("[P_V] house").unwrap();
        let diff = diff_comments(&db, &disk);
        assert_eq!(diff.phase_changed, Some(('_', 'P')));
        assert_eq!(diff.mood_changed, None);
        assert_eq!(diff.vibe_changed, Some(('_', 'V')));
        assert!(diff.tags_added.is_empty());
        assert!(diff.tags_removed.is_empty());
    }

    #[test]
    fn test_diff_comments_pmv_removed() {
        let db = parse_comment("[PMV] house").unwrap();
        let disk = parse_comment("[_M_] house").unwrap();
        let diff = diff_comments(&db, &disk);
        assert_eq!(diff.phase_changed, Some(('P', '_')));
        assert_eq!(diff.vibe_changed, Some(('V', '_')));
        assert_eq!(diff.mood_changed, None);
    }

    #[test]
    fn test_diff_comments_source_ids() {
        let db = parse_comment("[___] house sp:abc").unwrap();
        let disk = parse_comment("[___] house sp:abc sc:def").unwrap();
        let diff = diff_comments(&db, &disk);
        assert_eq!(diff.source_ids_added, vec!["sc:def"]);
        assert!(diff.source_ids_removed.is_empty());

        let diff2 = diff_comments(&disk, &db);
        assert_eq!(diff2.source_ids_removed, vec!["sc:def"]);
        assert!(diff2.source_ids_added.is_empty());
    }

    #[test]
    fn test_diff_comments_multiword_quoted_tags() {
        let db = parse_comment("[___] house").unwrap();
        let disk = parse_comment("[___] house \"zu späßen aufgelegt\"").unwrap();
        let diff = diff_comments(&db, &disk);
        assert_eq!(diff.tags_added, vec!["zu späßen aufgelegt"]);
    }

    #[test]
    fn test_diff_comment_strings_unparseable_disk() {
        // "[XYZ] ..." fails PMV validation → parse_comment returns None.
        let db = "[___] house";
        let disk = "[XYZ] raw junk";
        let diff = diff_comment_strings(Some(db), Some(disk));
        assert!(diff.raw_comment_changed, "unparseable differing disk must flag raw change");
        assert!(diff.tags_added.is_empty());

        // Same raw text on both sides → no diff even though disk is unparseable.
        let same = "[XYZ] raw junk";
        let diff2 = diff_comment_strings(Some(same), Some(same));
        assert!(diff2.is_empty());
    }

    #[test]
    fn test_diff_comment_strings_empty_sides() {
        let diff = diff_comment_strings(None, Some(""));
        assert!(diff.is_empty());

        let diff2 = diff_comment_strings(None, Some("[___] house"));
        assert_eq!(diff2.tags_added, vec!["house"]);
    }

    // ── comment_fingerprint tests ──────────────────────────────────────────

    #[test]
    fn test_comment_fingerprint_stable() {
        let fp1 = comment_fingerprint("[PMV] house sunny sp:abc");
        let fp2 = comment_fingerprint("[PMV] house sunny sp:abc");
        assert_eq!(fp1, fp2);
        assert_eq!(fp1.len(), 32);
    }

    #[test]
    fn test_comment_fingerprint_canonical() {
        // Same parsed content, different raw formatting/order → same fingerprint.
        let fp1 = comment_fingerprint("[PMV] house sunny sp:abc");
        let fp2 = comment_fingerprint("[PMV] sunny house sp:abc");
        assert_eq!(fp1, fp2);

        // Case differences normalize (tags are lowercased).
        let fp3 = comment_fingerprint("[PMV] HOUSE Sunny sp:abc");
        assert_eq!(fp1, fp3);
    }

    #[test]
    fn test_comment_fingerprint_distinguishes_content() {
        let fp1 = comment_fingerprint("[___] house");
        let fp2 = comment_fingerprint("[___] house sunny");
        assert_ne!(fp1, fp2);

        let fp3 = comment_fingerprint("[P__] house");
        assert_ne!(fp1, fp3);
    }

    #[test]
    fn test_comment_fingerprint_unparseable_raw() {
        let fp1 = comment_fingerprint("[XYZ] junk");
        let fp2 = comment_fingerprint("[XYZ] junk");
        assert_eq!(fp1, fp2);
        let fp3 = comment_fingerprint("[XYZ] other");
        assert_ne!(fp1, fp3);
    }

    #[test]
    fn test_comment_fingerprint_empty() {
        let fp1 = comment_fingerprint("");
        let fp2 = comment_fingerprint("   ");
        assert_eq!(fp1, fp2);
    }

    #[test]
    fn test_comment_fingerprint_roundtrip_equivalence() {
        // A generated comment and its parse→generate form share a fingerprint.
        let generated = generate_from_parts('_', '_', '_', &["droid".into(), "house".into()], &[]);
        let reparsed = generate_comment(&parse_comment(&generated).unwrap());
        assert_eq!(comment_fingerprint(&generated), comment_fingerprint(&reparsed));
    }

    // ── Tag-inbox: levenshtein / similar_tags / mapping application ──────

    #[test]
    fn test_levenshtein_distance_basic() {
        assert_eq!(levenshtein_distance("peak", "peak"), 0);
        assert_eq!(levenshtein_distance("peek", "peak"), 1); // e→a
        assert_eq!(levenshtein_distance("peeq", "peak"), 2); // eq→ak
        assert_eq!(levenshtein_distance("aufbau", "aufbauen"), 2); // insert n
        assert_eq!(levenshtein_distance("", "abc"), 3);
        assert_eq!(levenshtein_distance("abc", ""), 3);
        assert_eq!(levenshtein_distance("house", "droid"), 5);
    }

    #[test]
    fn test_levenshtein_distance_case_insensitive() {
        assert_eq!(levenshtein_distance("PEEK", "peak"), 1);
        assert_eq!(levenshtein_distance("Groovy", "groovy"), 0);
    }

    #[test]
    fn test_similar_tags_matches_and_self_exclusion() {
        let existing: Vec<String> = vec![
            "peak".into(),
            "peek".into(),
            "aufbauen".into(),
            "house".into(),
        ];
        // Typo "peek" → suggests "peak" (distance 1), excludes itself.
        let hits = similar_tags("peek", &existing, 2);
        assert!(hits.contains(&("peak".to_string(), 1)));
        assert!(!hits.iter().any(|(t, _)| t == "peek"), "self must be excluded");
        // "aufbau" → "aufbauen" (distance 2).
        assert!(similar_tags("aufbau", &existing, 2).contains(&("aufbauen".to_string(), 2)));
        // "house" is too far from every candidate.
        assert!(similar_tags("house", &existing, 2).is_empty());
        // max_distance = 1 drops the distance-2 hit.
        assert!(!similar_tags("aufbau", &existing, 1).contains(&("aufbauen".to_string(), 2)));
    }

    #[test]
    fn test_similar_tags_sorted_by_distance_then_name() {
        let existing: Vec<String> = vec!["aaa".into(), "aba".into(), "abb".into()];
        let hits = similar_tags("aaa", &existing, 2);
        assert_eq!(hits[0], ("aba".to_string(), 1));
        // "abb": aaa→abb is 2 (a→a, a→b, a→b). Only one distance-2 candidate here.
        assert!(hits.iter().all(|(_, d)| *d <= 2));
    }

    #[test]
    fn test_apply_tag_mappings_merge_typo_into_canonical() {
        use std::collections::HashMap;
        let mut mappings = HashMap::new();
        mappings.insert("peek".to_string(), "peak".to_string());

        // Target from the playlist chain already has the canonical tag.
        let target = "[_M_] peak sp:spotify:track:t1";
        let stored = "[_M_] peek sp:spotify:track:t1";
        assert_eq!(
            apply_tag_mappings_to_target(target, Some(stored), &mappings),
            "[_M_] peak sp:spotify:track:t1"
        );

        // Target has NO tags (tag only typed in the stored comment):
        // the mapped canonical tag must be written, not silently dropped.
        let target2 = "[___] sp:spotify:track:t2";
        let stored2 = "[___] peek sp:spotify:track:t2";
        assert_eq!(
            apply_tag_mappings_to_target(target2, Some(stored2), &mappings),
            "[___] peak sp:spotify:track:t2"
        );

        // Playlist-typo case: target resolves the raw tag, mapping rewrites it.
        let target3 = "[___] peek sp:spotify:track:t3";
        assert_eq!(
            apply_tag_mappings_to_target(target3, Some("[___] sp:spotify:track:t3"), &mappings),
            "[___] peak sp:spotify:track:t3"
        );
    }

    #[test]
    fn test_apply_tag_mappings_rename_to_self_keeps_tag() {
        use std::collections::HashMap;
        let mut mappings = HashMap::new();
        mappings.insert("aufbau".to_string(), "aufbau".to_string());

        // Rename-to-self = "keep this typed tag": the write must not drop it.
        let target = "[___] sp:spotify:track:t4";
        let stored = "[___] aufbau sp:spotify:track:t4";
        assert_eq!(
            apply_tag_mappings_to_target(target, Some(stored), &mappings),
            "[___] aufbau sp:spotify:track:t4"
        );
    }

    #[test]
    fn test_apply_tag_mappings_no_mapping_no_change() {
        use std::collections::HashMap;
        let mappings = HashMap::new();
        let target = "[___] house sp:spotify:track:t5";
        assert_eq!(
            apply_tag_mappings_to_target(target, Some("[___] peek sp:spotify:track:t5"), &mappings),
            target
        );
        // Unparseable target is returned untouched.
        assert_eq!(
            apply_tag_mappings_to_target("[A1B] raw garbage", None, &{
                let mut m = HashMap::new();
                m.insert("peek".to_string(), "peak".to_string());
                m
            }),
            "[A1B] raw garbage"
        );
    }
}
