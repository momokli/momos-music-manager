//! Deterministische Comment-Template-Semantik: Platzhalter-Expansion, Slugging,
//! Whitespace-Normalisierung, Deckel (48/512) und Validierung.
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
pub const ALLOWED_PLACEHOLDERS: [&str; 4] = ["tags", "bpm", "key", "energy"];

/// Applied when the template is missing/empty (issue #653). It reproduces the
/// old fixed render in the new token form.
pub const DEFAULT_TEMPLATE: &str = "{tags} {bpm} {key} {energy}";

/// Longest single placeholder value, in Unicode scalar values.
pub const TOKEN_MAX_CHARS: usize = 48;
/// Longest whole Comment, in Unicode scalar values (ID3/MP4 comment fields).
pub const COMMENT_MAX_CHARS: usize = 512;
/// Truncation marker (ASCII, so it never itself forces a re-cut).
const ELLIPSIS: &str = "...";

/// Faktum eines Tracks, das ein Template referenzieren kann.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TrackFacts {
    pub tags: Vec<String>,
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

/// Slug one tag/key name: every character outside `[A-Za-z0-9_-]` becomes `-`,
/// runs of `-` collapse to one and the edges are trimmed. Non-ASCII (e.g. `ä`)
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

/// Render the `{tags}` token: trim → slug → drop empties → sort → dedup, joined
/// with `", "`.
fn render_tags(tags: &[String]) -> String {
    let mut names: Vec<String> = tags
        .iter()
        .map(|t| slug(t.trim()))
        .filter(|t| !t.is_empty())
        .collect();
    names.sort();
    names.dedup();
    names.join(", ")
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

/// Render the Comment text for one track from a device template.
///
/// Placeholders `{tags}`, `{bpm}`, `{key}` and `{energy}` expand to space-free
/// tokens (tag names are slugged + `", "`-joined); an unknown placeholder, or a
/// missing/blank template, warns on stderr and falls back to
/// [`DEFAULT_TEMPLATE`]. Empty placeholders disappear, leftover whitespace and
/// `|` are normalised, each token is capped at [`TOKEN_MAX_CHARS`] and the whole
/// Comment at [`COMMENT_MAX_CHARS`] with a `...` marker. Deterministic.
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

    let tags_token = truncate_chars(&render_tags(&facts.tags), TOKEN_MAX_CHARS);
    let bpm_token = truncate_chars(
        &facts
            .bpm
            .map(|b| format!("BPM-{}", fmt_num(b)))
            .unwrap_or_default(),
        TOKEN_MAX_CHARS,
    );
    let key_token = truncate_chars(
        &facts
            .key
            .as_deref()
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(|k| format!("KEY-{}", slug(k)))
            .unwrap_or_default(),
        TOKEN_MAX_CHARS,
    );
    let energy_token = truncate_chars(
        &facts
            .energy
            .map(|e| format!("ENERGY-{}", fmt_num(e)))
            .unwrap_or_default(),
        TOKEN_MAX_CHARS,
    );

    let expanded = tpl
        .replace("{tags}", &tags_token)
        .replace("{bpm}", &bpm_token)
        .replace("{key}", &key_token)
        .replace("{energy}", &energy_token);

    truncate_chars(&normalise_whitespace(&expanded), COMMENT_MAX_CHARS)
}

/// The comma-separated, brace-wrapped list of allowed placeholders, e.g.
/// `{tags}, {bpm}, {key}, {energy}` — used in the rejection message so a client
/// can correct the template without guessing.
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
/// unknown placeholder {title} (allowed: {tags}, {bpm}, {key}, {energy})
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

    fn render_default(tags: &[&str], bpm: Option<f64>, key: Option<&str>, energy: Option<f64>) -> String {
        let owned: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
        render(
            DEFAULT_TEMPLATE,
            &TrackFacts {
                tags: owned,
                bpm,
                key: key.map(str::to_string),
                energy,
            },
        )
    }

    fn facts(tags: &[&str], bpm: Option<f64>, key: Option<&str>, energy: Option<f64>) -> TrackFacts {
        TrackFacts {
            tags: tags.iter().map(|s| s.to_string()).collect(),
            bpm,
            key: key.map(str::to_string),
            energy,
        }
    }

    #[test]
    fn bpm_renders_as_token() {
        assert_eq!(render_default(&[], Some(128.0), None, None), "BPM-128");
        // Fractional values keep their decimals, still one token.
        assert_eq!(render_default(&[], Some(127.5), None, None), "BPM-127.5");
    }

    #[test]
    fn tag_with_space_is_slugged() {
        assert_eq!(render_default(&["Hard Techno"], None, None, None), "Hard-Techno");
    }

    #[test]
    fn tags_are_trimmed_sorted_deduped_and_slugged() {
        assert_eq!(
            render_default(
                &["b", "a", "b", "  c ", "Hard Techno", "a-b"],
                None,
                None,
                None
            ),
            "Hard-Techno, a, a-b, b, c"
        );
    }

    #[test]
    fn all_placeholders_render_one_token_each() {
        assert_eq!(
            render_default(&["Techno"], Some(128.0), Some("8A"), Some(7.0)),
            "Techno BPM-128 KEY-8A ENERGY-7"
        );
    }

    #[test]
    fn empty_placeholders_leave_no_double_space_or_hanging_separator() {
        // Only tags present -> a single space between tokens, edges trimmed.
        assert_eq!(render_default(&["Techno"], None, None, None), "Techno");
        // Explicit separators: empty {key} leaves neither `|  |` nor trailing `|`.
        let owned = facts(&["Techno"], None, None, None);
        assert_eq!(
            render("{tags} | {bpm} | {key} | {energy}", &owned),
            "Techno"
        );
        assert_eq!(
            render(
                "{tags} | {bpm} | {key} | {energy}",
                &facts(&["Techno"], Some(128.0), None, Some(7.0))
            ),
            "Techno | BPM-128 | ENERGY-7"
        );
        // No tags and nothing else -> empty Comment.
        assert_eq!(render_default(&[], None, None, None), "");
    }

    #[test]
    fn empty_tag_list_drops_the_placeholder() {
        assert_eq!(render_default(&["  "], Some(120.0), None, None), "BPM-120");
    }

    #[test]
    fn long_tag_is_capped_at_48_chars_with_ellipsis() {
        let long = "x".repeat(80);
        let out = render_default(&[&long], None, None, None);
        assert_eq!(out.chars().count(), 48, "capped at the token ceiling");
        assert!(out.ends_with("..."), "truncated with the marker: {out:?}");
        assert!(out.starts_with(&"x".repeat(45)));
    }

    #[test]
    fn truncation_counts_unicode_scalars_not_bytes() {
        // `é` is 2 bytes; the cap is measured in scalar values, never mid-char.
        let long = "é".repeat(80);
        let out = truncate_chars(&long, 48);
        assert_eq!(out.chars().count(), 48, "48 scalar values");
        assert!(out.ends_with("..."));
        assert!(out.is_char_boundary(out.len()), "cut on a char boundary");
        assert_eq!(out.chars().take(45).collect::<String>(), "é".repeat(45));
    }

    #[test]
    fn comment_over_512_chars_is_capped_with_ellipsis() {
        // A template of literal filler pushes the whole Comment past 512 — each
        // `{tags}` token is itself capped at 48, but literal text is not.
        let tpl = format!("{{tags}} {}", "x ".repeat(300));
        let out = render(&tpl, &facts(&["Techno"], None, None, None));
        assert_eq!(out.chars().count(), 512, "capped at the comment ceiling");
        assert!(out.ends_with("..."), "truncated with the marker");
    }

    #[test]
    fn unknown_placeholder_falls_back_to_default() {
        let out = render("{title} {tags}", &facts(&["Techno"], Some(128.0), None, None));
        assert_eq!(out, "Techno BPM-128", "default template applied");
    }

    #[test]
    fn empty_template_falls_back_to_default() {
        assert_eq!(
            render("   ", &facts(&["Techno"], None, None, None)),
            "Techno"
        );
    }

    #[test]
    fn umlauts_and_punctuation_become_dashes() {
        // Documented rule: everything outside [A-Za-z0-9_-] -> `-`, runs collapse.
        assert_eq!(render_default(&["Röck & Roll"], None, None, None), "R-ck-Roll");
        assert_eq!(
            render_default(&["!!!"], None, None, None),
            "",
            "all separators -> dropped"
        );
    }

    #[test]
    fn render_is_deterministic_and_order_independent() {
        let tags = vec!["zeta".into(), "alpha".into(), "mu".into()];
        let first = render(
            DEFAULT_TEMPLATE,
            &TrackFacts {
                tags: tags.clone(),
                bpm: Some(120.0),
                key: Some("C-Major".into()),
                energy: Some(5.0),
            },
        );
        for _ in 0..10 {
            assert_eq!(
                render(
                    DEFAULT_TEMPLATE,
                    &TrackFacts {
                        tags: tags.clone(),
                        bpm: Some(120.0),
                        key: Some("C-Major".into()),
                        energy: Some(5.0),
                    }
                ),
                first
            );
        }
        let reordered = vec!["mu".into(), "zeta".into(), "alpha".into()];
        assert_eq!(
            render(
                DEFAULT_TEMPLATE,
                &TrackFacts {
                    tags: reordered,
                    bpm: Some(120.0),
                    key: Some("C-Major".into()),
                    energy: Some(5.0),
                }
            ),
            first
        );
    }

    #[test]
    fn validate_accepts_allowed_placeholders() {
        for t in [
            DEFAULT_TEMPLATE,
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
        assert!(err.contains("{tags}"), "lists allowed: {err}");
        assert!(err.contains("{energy}"), "lists allowed: {err}");
        assert!(validate("{tags} {artist}").is_err());
    }

    #[test]
    fn validate_accepts_double_brace_edge_cases() {
        // `{{` is an escaped literal, never an unknown placeholder.
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
        // A `{name}` after an escaped pair is still validated.
        assert!(validate("{{ {nope}").is_err());
    }

    #[test]
    fn validate_error_message_is_byte_stable() {
        assert_eq!(
            validate("{title}").unwrap_err(),
            "unknown placeholder {title} (allowed: {tags}, {bpm}, {key}, {energy})"
        );
    }

    #[test]
    fn render_and_validate_diverge_on_double_brace() {
        // Documented divergence: validate accepts `{{tags}}`, render falls back.
        assert!(validate("{{tags}}").is_ok());
        assert_eq!(render("{{tags}}", &facts(&["Techno"], None, None, None)), "Techno");
    }
}
