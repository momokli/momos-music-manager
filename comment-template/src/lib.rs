//! Deterministische Comment-Template-Semantik: Platzhalter-Expansion, Slugging,
//! Whitespace-Normalisierung, Deckel (512) und Validierung.
//!
//! Vertrag zwischen `music-hub` (Vorschau/Validierung) und `kellner` (Schreiben).
//!
//! # Zwei Prädikate (bewusst getrennt)
//!
//! [`render`] und [`validate`] beantworten **nicht** dieselbe Frage und dürfen
//! deshalb nicht vereinheitlicht werden:
//!
//! * [`render`] nutzt den *kellner*-Scan ([`first_unknown_placeholder`]): keine
//!   `{{`-Sonderbehandlung. Für `"{{tags}}"` findet er das erste `}` und liest
//!   den Namen `"{tags"` → unbekannt → Rückfall auf [`DEFAULT_TEMPLATE`].
//! * [`validate`] bildet die *Hub*-Semantik ab: `{{` gilt als escaptes Literal
//!   und wird übersprungen, also ist `"{{tags}}"` akzeptiert.
//!
//! `render` ruft **nicht** `validate` auf. Ein Vereinheitlichen würde zwingend
//! entweder die Hub-400-Prüfung oder das kellner-Render-Ergebnis ändern.

/// The placeholders a template may reference (mirrors the hub whitelist).
pub const ALLOWED_PLACEHOLDERS: [&str; 6] =
    ["types", "tags", "source_ids", "bpm", "key", "energy"];

/// Applied when the template is missing/empty (issue #689).
pub const DEFAULT_TEMPLATE: &str = "{types} {tags}";

/// Longest whole Comment, in Unicode scalar values (ID3/MP4 comment fields).
pub const COMMENT_MAX_CHARS: usize = 512;
/// Truncation marker (ASCII, so it never itself forces a re-cut).
const ELLIPSIS: &str = "...";

/// Faktum eines Tracks, das ein Template referenzieren kann.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackFacts {
    /// Typ-Kürzel in Hub-Reihenfolge, z.B. `["M", "G", "P", "E"]`.
    pub types: Vec<String>,
    pub tags: Vec<String>,
    pub source_ids: Vec<String>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub energy: Option<f64>,
}

/// Format an audio number deterministically: an integral value prints without a
/// decimal point (`128.0` → `"128"`), a fractional one as-is (`127.5`).
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        format!("{v}")
    }
}

/// German umlaut transliteration, applied before slugging (issue #689).
fn transliterate(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            'ä' => out.push_str("ae"),
            'ö' => out.push_str("oe"),
            'ü' => out.push_str("ue"),
            'ß' => out.push_str("ss"),
            'Ä' => out.push_str("Ae"),
            'Ö' => out.push_str("Oe"),
            'Ü' => out.push_str("Ue"),
            _ => out.push(c),
        }
    }
    out
}

/// Slug one tag/key name: every character outside `[A-Za-z0-9_-]` becomes `-`,
/// runs of `-` collapse to one and the edges are trimmed. Non-ASCII (e.g. `é`)
/// therefore becomes `-`. An all-separator input slugs to `""`.
fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            out.push(c);
            prev_dash = c == '-';
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// Render the `{tags}` token: trim → umlaut transliteration → slug → drop
/// empties → case-insensitive dedup keeping the **first** occurrence → join
/// with a single space. No sorting, no commas (issue #689).
fn render_tags(tags: &[String]) -> String {
    let mut names: Vec<String> = Vec::with_capacity(tags.len());
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for tag in tags {
        let slugged = slug(&transliterate(tag.trim()));
        if slugged.is_empty() {
            continue;
        }
        // Dedup is case-insensitive; the first spelling wins.
        if seen.insert(slugged.to_lowercase()) {
            names.push(slugged);
        }
    }
    names.join(" ")
}

/// Render the `{types}` token: keep entries that are 1–2 ASCII alphanumeric
/// characters, concatenate them without separators and wrap the result in
/// literal brackets (`[MGPE]`). No valid entry → the whole token disappears.
fn render_types(types: &[String]) -> String {
    let mut out = String::new();
    for t in types {
        let t = t.trim();
        let len = t.chars().count();
        if (1..=2).contains(&len) && t.chars().all(|c| c.is_ascii_alphanumeric()) {
            out.push_str(t);
        }
    }
    if out.is_empty() {
        String::new()
    } else {
        format!("[{out}]")
    }
}

/// Render the `{source_ids}` token: drop empty/blank entries, join the rest with
/// a single space. Deliberately **not** slugged (ids contain `:`).
fn render_source_ids(source_ids: &[String]) -> String {
    source_ids
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Truncate `s` to `max` Unicode scalar values, ending on `...` when cut.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(ELLIPSIS.chars().count());
    let mut out: String = s.chars().take(keep).collect();
    out.push_str(ELLIPSIS);
    out
}

/// Collapse whitespace to single spaces, trim the edges and clean separator
/// `|` (drop leading/trailing, collapse consecutive) so empty placeholders leave
/// neither `double  space` nor a hanging `|`.
fn normalise_whitespace(s: &str) -> String {
    let mut tokens: Vec<&str> = s.split_whitespace().collect();
    while tokens.first() == Some(&"|") {
        tokens.remove(0);
    }
    while tokens.last() == Some(&"|") {
        tokens.pop();
    }
    let mut out: Vec<&str> = Vec::with_capacity(tokens.len());
    for t in tokens {
        if t == "|" && out.last() == Some(&"|") {
            continue;
        }
        out.push(t);
    }
    out.join(" ")
}

/// The first `{name}` token in `template` that is not an allowed placeholder.
///
/// This is the *kellner* scan used by [`render`]: byte-based, `find('}')` after
/// `{`, and **no** `{{`-escape handling (see the crate-level note).
fn first_unknown_placeholder(template: &str) -> Option<String> {
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(rel) = template[i + 1..].find('}') {
                let name = &template[i + 1..i + 1 + rel];
                if !ALLOWED_PLACEHOLDERS.contains(&name) {
                    return Some(name.to_string());
                }
                i = i + 1 + rel + 1;
                continue;
            }
        }
        i += 1;
    }
    None
}

/// Expand `template` against `facts`, using `tags` as the tag source.
fn expand(template: &str, facts: &TrackFacts, tags: &[String]) -> String {
    let types_token = render_types(&facts.types);
    let tags_token = render_tags(tags);
    let source_ids_token = render_source_ids(&facts.source_ids);
    let bpm_token = facts
        .bpm
        .map(|b| format!("BPM-{}", b.round() as i64))
        .unwrap_or_default();
    let key_token = facts
        .key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(|k| format!("KEY-{}", slug(k)))
        .unwrap_or_default();
    let energy_token = facts
        .energy
        .map(|e| format!("ENERGY-{}", fmt_num(e)))
        .unwrap_or_default();

    template
        .replace("{types}", &types_token)
        .replace("{tags}", &tags_token)
        .replace("{source_ids}", &source_ids_token)
        .replace("{bpm}", &bpm_token)
        .replace("{key}", &key_token)
        .replace("{energy}", &energy_token)
}

/// Render the Comment text for one track from a device template.
///
/// Placeholders `{types}`, `{tags}`, `{source_ids}`, `{bpm}`, `{key}` and
/// `{energy}` expand to space-free tokens; an unknown placeholder, or a
/// missing/blank template, warns on stderr and falls back to
/// [`DEFAULT_TEMPLATE`]. Empty placeholders disappear and leftover whitespace
/// and `|` are normalised.
///
/// Budget instead of a per-token cut (issue #689): if the expanded Comment
/// exceeds [`COMMENT_MAX_CHARS`], whole tags are dropped from the **back** of
/// the tag list and the template is re-expanded — never a half tag. Only when
/// the text is still too long without any tags is it cut at
/// [`COMMENT_MAX_CHARS`] with a `...` marker. Deterministic.
///
/// Uses the kellner scan ([`first_unknown_placeholder`]) for the fallback, **not**
/// [`validate`]; see the crate-level note on the two predicates.
pub fn render(template: &str, facts: &TrackFacts) -> String {
    let trimmed = template.trim();
    let tpl = if trimmed.is_empty() {
        DEFAULT_TEMPLATE
    } else if let Some(name) = first_unknown_placeholder(trimmed) {
        eprintln!(
            "kellner: comment template: unknown placeholder {{{name}}} — falling back to default template"
        );
        DEFAULT_TEMPLATE
    } else {
        trimmed
    };

    let mut tags = facts.tags.clone();
    loop {
        let rendered = normalise_whitespace(&expand(tpl, facts, &tags));
        if rendered.chars().count() <= COMMENT_MAX_CHARS {
            return rendered;
        }
        if tags.pop().is_none() {
            return truncate_chars(&rendered, COMMENT_MAX_CHARS);
        }
    }
}

/// The comma-separated, brace-wrapped list of allowed placeholders, e.g.
/// `{types}, {tags}, …` — used in the rejection message so a client can correct
/// the template without guessing.
fn allowed_placeholder_list() -> String {
    ALLOWED_PLACEHOLDERS
        .iter()
        .map(|p| format!("{{{p}}}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Platzhalter eines Templates validieren (Hub-Semantik).
///
/// Reject any `{name}` token whose `name` is not in [`ALLOWED_PLACEHOLDERS`].
///
/// A doubled brace (`{{`) is treated as an escaped literal and skipped, so
/// templates that contain `{{`-like sequences are accepted. An unclosed `{`
/// (no following `}`) is treated as literal text and accepted. The returned
/// error names the offending token and lists the allowed placeholders:
///
/// ```text
/// unknown placeholder {title} (allowed: {types}, {tags}, {source_ids}, {bpm}, {key}, {energy})
/// ```
///
/// This is the *hub* semantics and is intentionally **not** shared with
/// [`render`]'s fallback scan; see the crate-level note on the two predicates.
pub fn validate(template: &str) -> Result<(), String> {
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '{' {
            i += 1;
            continue;
        }
        // `{{` is an escaped literal brace — skip both, not a placeholder.
        if i + 1 < chars.len() && chars[i + 1] == '{' {
            i += 2;
            continue;
        }
        match chars[i + 1..].iter().position(|&c| c == '}') {
            Some(rel) => {
                let name: String = chars[i + 1..i + 1 + rel].iter().collect();
                if !ALLOWED_PLACEHOLDERS.contains(&name.as_str()) {
                    return Err(format!(
                        "unknown placeholder {{{name}}} (allowed: {})",
                        allowed_placeholder_list()
                    ));
                }
                i += 1 + rel + 1;
            }
            // Unclosed `{` → literal text.
            None => i += 1,
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owned(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|s| s.to_string()).collect()
    }

    /// Facts with only the classic fields set; `types`/`source_ids` empty.
    fn facts(
        tags: &[&str],
        bpm: Option<f64>,
        key: Option<&str>,
        energy: Option<f64>,
    ) -> TrackFacts {
        TrackFacts {
            types: vec![],
            tags: owned(tags),
            source_ids: vec![],
            bpm,
            key: key.map(str::to_string),
            energy,
        }
    }

    fn render_default(
        tags: &[&str],
        bpm: Option<f64>,
        key: Option<&str>,
        energy: Option<f64>,
    ) -> String {
        render(DEFAULT_TEMPLATE, &facts(tags, bpm, key, energy))
    }

    // ---- {types} ---------------------------------------------------------

    #[test]
    fn default_template_renders_types_then_tags() {
        let f = TrackFacts {
            types: owned(&["M", "G", "P", "E"]),
            tags: owned(&["Hard Techno", "Euphoric"]),
            ..Default::default()
        };
        assert_eq!(render("", &f), "[MGPE] Hard-Techno Euphoric");
        assert_eq!(render(DEFAULT_TEMPLATE, &f), "[MGPE] Hard-Techno Euphoric");
    }

    #[test]
    fn types_token_is_bracket_wrapped_and_concatenated() {
        let f = TrackFacts {
            types: owned(&["M", "G", "P", "E"]),
            ..Default::default()
        };
        assert_eq!(render("{types}", &f), "[MGPE]");
    }

    #[test]
    fn types_filters_invalid_entries() {
        let f = TrackFacts {
            types: owned(&["M", "", "x y", "M2"]),
            ..Default::default()
        };
        assert_eq!(render("{types}", &f), "[MM2]");
    }

    #[test]
    fn empty_types_drops_token_without_double_space() {
        let f = facts(&["Techno"], None, None, None);
        assert_eq!(render(DEFAULT_TEMPLATE, &f), "Techno");
        assert_eq!(render("{types} | {tags}", &f), "Techno");
        assert_eq!(
            render("{types} {tags}", &TrackFacts::default()),
            "",
            "both empty -> empty comment, no brackets"
        );
    }

    // ---- {tags} ----------------------------------------------------------

    #[test]
    fn tags_keep_input_order_and_join_with_one_space() {
        assert_eq!(render_default(&["zebra", "Alpha"], None, None, None), "zebra Alpha");
        assert_eq!(
            render_default(&["Hard Techno", "Euphoric"], None, None, None),
            "Hard-Techno Euphoric"
        );
    }

    #[test]
    fn tags_dedup_keeps_first_occurrence() {
        assert_eq!(render_default(&["a", "A"], None, None, None), "a");
        assert_eq!(
            render_default(&["b", "a", "b", "  c "], None, None, None),
            "b a c"
        );
    }

    #[test]
    fn tags_transliterate_umlauts_before_slugging() {
        assert_eq!(
            render_default(&["Zu späßen", "Füße"], None, None, None),
            "Zu-spaessen Fuesse"
        );
        assert_eq!(render_default(&["Röck & Roll"], None, None, None), "Roeck-Roll");
        assert_eq!(render_default(&["Ä Ö Ü ß"], None, None, None), "Ae-Oe-Ue-ss");
    }

    #[test]
    fn tag_with_space_is_slugged() {
        assert_eq!(render_default(&["Hard Techno"], None, None, None), "Hard-Techno");
    }

    #[test]
    fn empty_or_all_separator_tags_are_dropped() {
        assert_eq!(
            render("{tags} {bpm}", &facts(&["  ", "!!!"], Some(120.0), None, None)),
            "BPM-120"
        );
    }

    // ---- {source_ids} ----------------------------------------------------

    #[test]
    fn source_ids_are_space_joined_without_slugging() {
        let f = TrackFacts {
            source_ids: owned(&["sp:abc", "sc:def"]),
            ..Default::default()
        };
        assert_eq!(render("{source_ids}", &f), "sp:abc sc:def");
    }

    #[test]
    fn empty_source_ids_drop_the_token() {
        let f = TrackFacts {
            tags: owned(&["Techno"]),
            source_ids: owned(&["", "  "]),
            ..Default::default()
        };
        assert_eq!(render("{source_ids} {tags}", &f), "Techno");
    }

    // ---- {bpm} -----------------------------------------------------------

    #[test]
    fn bpm_rounds_to_integer() {
        assert_eq!(render("{bpm}", &facts(&[], Some(147.963), None, None)), "BPM-148");
        assert_eq!(render("{bpm}", &facts(&[], Some(128.5), None, None)), "BPM-129");
        assert_eq!(render("{bpm}", &facts(&[], Some(128.0), None, None)), "BPM-128");
    }

    // ---- {key} / {energy} (opt-in, unchanged) ----------------------------

    #[test]
    fn key_and_energy_stay_optional_tokens() {
        assert_eq!(
            render(
                "{tags} {bpm} {key} {energy}",
                &facts(&["Techno"], Some(128.0), Some("8A"), Some(7.0))
            ),
            "Techno BPM-128 KEY-8A ENERGY-7"
        );
        assert_eq!(
            render("{key} / {bpm}", &facts(&[], Some(127.5), Some("A-Minor"), None)),
            "KEY-A-Minor / BPM-128"
        );
    }

    #[test]
    fn empty_placeholders_leave_no_double_space_or_hanging_separator() {
        let f = facts(&["Techno"], None, None, None);
        assert_eq!(render("{tags}", &f), "Techno");
        assert_eq!(render("{tags} | {bpm} | {key} | {energy}", &f), "Techno");
        assert_eq!(
            render(
                "{tags} | {bpm} | {key} | {energy}",
                &facts(&["Techno"], Some(128.0), None, Some(7.0))
            ),
            "Techno | BPM-128 | ENERGY-7"
        );
        assert_eq!(render("{tags}", &TrackFacts::default()), "");
    }

    // ---- Budget ----------------------------------------------------------

    #[test]
    fn forty_ten_char_tags_fit_within_the_budget() {
        let tags: Vec<String> = (0..40).map(|i| format!("{i:0>10}")).collect();
        let f = TrackFacts {
            tags,
            ..Default::default()
        };
        let out = render(DEFAULT_TEMPLATE, &f);
        assert_eq!(out.chars().count(), 40 * 10 + 39, "all tags kept");
        assert!(out.chars().count() <= COMMENT_MAX_CHARS);
        assert!(!out.ends_with("..."));
    }

    #[test]
    fn sixty_tags_drop_whole_tags_from_the_back() {
        let tags: Vec<String> = (0..60).map(|i| format!("{i:0>10}")).collect();
        let f = TrackFacts {
            tags,
            ..Default::default()
        };
        let out = render(DEFAULT_TEMPLATE, &f);
        assert!(out.chars().count() <= COMMENT_MAX_CHARS, "within budget");
        assert!(!out.contains("..."), "budget trim is not a cut: {out:?}");
        // 46 ten-char tags joined by single spaces = 505 chars.
        let kept: Vec<&str> = out.split(' ').collect();
        assert_eq!(kept.len(), 46);
        for (i, token) in kept.iter().enumerate() {
            assert_eq!(token.len(), 10, "no half tag: {token:?}");
            assert_eq!(*token, format!("{i:0>10}"));
        }
    }

    #[test]
    fn literal_over_budget_is_truncated_with_ellipsis() {
        let tpl = format!("{{tags}} {}", "x ".repeat(300));
        let out = render(&tpl, &facts(&["Techno"], None, None, None));
        assert_eq!(out.chars().count(), COMMENT_MAX_CHARS);
        assert!(out.ends_with("..."));
    }

    #[test]
    fn truncation_counts_unicode_scalars_not_bytes() {
        let long = "é".repeat(600);
        let out = truncate_chars(&long, COMMENT_MAX_CHARS);
        assert_eq!(out.chars().count(), COMMENT_MAX_CHARS);
        assert!(out.ends_with("..."));
        assert!(out.is_char_boundary(out.len()), "cut on a char boundary");
        assert_eq!(
            out.chars().take(COMMENT_MAX_CHARS - 3).collect::<String>(),
            "é".repeat(COMMENT_MAX_CHARS - 3)
        );
    }

    // ---- Fallback / determinism -----------------------------------------

    #[test]
    fn unknown_placeholder_falls_back_to_default() {
        let out = render("{title} {tags}", &facts(&["Techno"], Some(128.0), None, None));
        assert_eq!(out, "Techno", "default template applied");
    }

    #[test]
    fn empty_template_falls_back_to_default() {
        assert_eq!(render("   ", &facts(&["Techno"], None, None, None)), "Techno");
    }

    #[test]
    fn render_is_deterministic() {
        let f = TrackFacts {
            types: owned(&["M", "G"]),
            tags: owned(&["zeta", "alpha"]),
            source_ids: owned(&["sp:1"]),
            bpm: Some(120.0),
            key: Some("C-Major".into()),
            energy: Some(5.0),
        };
        let first = render("{types} {tags} {source_ids} {bpm} {key} {energy}", &f);
        for _ in 0..10 {
            assert_eq!(
                render("{types} {tags} {source_ids} {bpm} {key} {energy}", &f),
                first
            );
        }
    }

    // ---- validate (unchanged semantics) ---------------------------------

    #[test]
    fn validate_accepts_allowed_placeholders() {
        for t in [
            DEFAULT_TEMPLATE,
            "{types} {tags} {source_ids} {bpm} {key} {energy}",
            "{tags}",
            "{bpm} {key} {energy}",
            "tags: {tags}",
            "",
            "   ",
        ] {
            assert!(validate(t).is_ok(), "{t:?} should be accepted");
        }
    }

    #[test]
    fn validate_rejects_unknown_placeholder() {
        let err = validate("{title}").unwrap_err();
        assert!(err.contains("{title}"), "names the token: {err}");
        assert!(err.contains("{types}"), "lists allowed: {err}");
        assert!(err.contains("{source_ids}"), "lists allowed: {err}");
        assert!(err.contains("{energy}"), "lists allowed: {err}");
        assert!(validate("{tags} {artist}").is_err());
    }

    #[test]
    fn validate_accepts_double_brace_edge_cases() {
        for t in [
            "{{",
            "{{tags}}",
            "a {{ b",
            "{{{tags}}}",
            "}",
            "{tags",
            "no braces",
        ] {
            assert!(validate(t).is_ok(), "{t:?} should be accepted");
        }
        assert!(validate("{{ {nope}").is_err());
    }

    #[test]
    fn validate_error_message_is_byte_stable() {
        assert_eq!(
            validate("{title}").unwrap_err(),
            "unknown placeholder {title} (allowed: {types}, {tags}, {source_ids}, {bpm}, {key}, {energy})"
        );
    }

    #[test]
    fn render_and_validate_diverge_on_double_brace() {
        assert!(validate("{{tags}}").is_ok());
        assert_eq!(render("{{tags}}", &facts(&["Techno"], None, None, None)), "Techno");
    }
}