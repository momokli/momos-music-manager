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
}
