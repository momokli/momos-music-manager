//! Comment-tag rendering + writing for backpack tracks (issue #367, #653).
//!
//! [`render_from_template`] is pure and deterministic: it expands a
//! device-supplied template (`{tags} {bpm} {key} {energy}`) into a Comment
//! string. Values are rendered as **space-free tokens** (`BPM-128`, `KEY-8A`,
//! `ENERGY-7`); tag names are slugged (`[^A-Za-z0-9_-]` → `-`, runs collapsed,
//! edges trimmed) because the template's own whitespace is the field separator.
//! [`write_comment`] writes the render into the file's primary tag with `lofty`,
//! but **only** when it differs from the existing Comment — an unchanged render
//! leaves the file (and its mtime) untouched.

use std::path::Path;

use anyhow::{Context, Result, anyhow};
use lofty::config::WriteOptions;
use lofty::prelude::*;
use lofty::tag::Tag;

/// The placeholders a template may reference (mirrors the hub whitelist).
const ALLOWED_PLACEHOLDERS: [&str; 4] = ["tags", "bpm", "key", "energy"];

/// Applied when the template is missing/empty (issue #653). It reproduces the
/// old fixed render in the new token form.
pub const DEFAULT_TEMPLATE: &str = "{tags} {bpm} {key} {energy}";

/// Longest single placeholder value, in Unicode scalar values.
const TOKEN_MAX_CHARS: usize = 48;
/// Longest whole Comment, in Unicode scalar values (ID3/MP4 comment fields).
const COMMENT_MAX_CHARS: usize = 512;
/// Truncation marker (ASCII, so it never itself forces a re-cut).
const ELLIPSIS: &str = "...";

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
pub fn render_from_template(
    template: &str,
    tags: &[String],
    bpm: Option<f64>,
    key: Option<&str>,
    energy: Option<f64>,
) -> String {
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

    let tags_token = truncate_chars(&render_tags(tags), TOKEN_MAX_CHARS);
    let bpm_token = truncate_chars(
        &bpm.map(|b| format!("BPM-{}", fmt_num(b))).unwrap_or_default(),
        TOKEN_MAX_CHARS,
    );
    let key_token = truncate_chars(
        &key.map(str::trim)
            .filter(|k| !k.is_empty())
            .map(|k| format!("KEY-{}", slug(k)))
            .unwrap_or_default(),
        TOKEN_MAX_CHARS,
    );
    let energy_token = truncate_chars(
        &energy.map(|e| format!("ENERGY-{}", fmt_num(e))).unwrap_or_default(),
        TOKEN_MAX_CHARS,
    );

    let expanded = tpl
        .replace("{tags}", &tags_token)
        .replace("{bpm}", &bpm_token)
        .replace("{key}", &key_token)
        .replace("{energy}", &energy_token);

    truncate_chars(&normalise_whitespace(&expanded), COMMENT_MAX_CHARS)
}

/// Write `comment` into the file's primary tag.
///
/// Returns `Ok(true)` when the file was written (the Comment differed), and
/// `Ok(false)` when the existing Comment already equals `comment` (no write, so
/// the mtime is preserved). Formats are covered by `lofty` (FLAC/MP3/M4A/…).
/// An empty `comment` clears the Comment field.
pub fn write_comment(path: &Path, comment: &str) -> Result<bool> {
    let mut tagged = lofty::read_from_path(path)
        .map_err(|e| anyhow!("{e}"))
        .with_context(|| format!("lofty read {}", path.display()))?;

    let existing = tagged
        .primary_tag()
        .and_then(|t| t.comment())
        .map(|c| c.to_string())
        .unwrap_or_default();
    if existing == comment {
        return Ok(false);
    }

    let tag_type = tagged.primary_tag_type();
    let tag = match tagged.primary_tag_mut() {
        Some(t) => t,
        None => {
            tagged.insert_tag(Tag::new(tag_type));
            tagged
                .primary_tag_mut()
                .expect("just inserted a tag of the primary type")
        }
    };
    if comment.is_empty() {
        tag.remove_comment();
    } else {
        tag.set_comment(comment.to_string());
    }

    tagged
        .save_to_path(path, WriteOptions::default())
        .map_err(|e| anyhow!("{e}"))
        .with_context(|| format!("lofty save {}", path.display()))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    fn render(tags: &[&str], bpm: Option<f64>, key: Option<&str>, energy: Option<f64>) -> String {
        let owned: Vec<String> = tags.iter().map(|s| s.to_string()).collect();
        render_from_template(DEFAULT_TEMPLATE, &owned, bpm, key, energy)
    }

    #[test]
    fn bpm_renders_as_token() {
        assert_eq!(render(&[], Some(128.0), None, None), "BPM-128");
        // Fractional values keep their decimals, still one token.
        assert_eq!(render(&[], Some(127.5), None, None), "BPM-127.5");
    }

    #[test]
    fn tag_with_space_is_slugged() {
        assert_eq!(render(&["Hard Techno"], None, None, None), "Hard-Techno");
    }

    #[test]
    fn tags_are_trimmed_sorted_deduped_and_slugged() {
        assert_eq!(
            render(&["b", "a", "b", "  c ", "Hard Techno", "a-b"], None, None, None),
            "Hard-Techno, a, a-b, b, c"
        );
    }

    #[test]
    fn all_placeholders_render_one_token_each() {
        assert_eq!(
            render(&["Techno"], Some(128.0), Some("8A"), Some(7.0)),
            "Techno BPM-128 KEY-8A ENERGY-7"
        );
    }

    #[test]
    fn empty_placeholders_leave_no_double_space_or_hanging_separator() {
        // Only tags present -> a single space between tokens, edges trimmed.
        assert_eq!(render(&["Techno"], None, None, None), "Techno");
        // Explicit separators: empty {key} leaves neither `|  |` nor trailing `|`.
        let owned: Vec<String> = vec!["Techno".into()];
        assert_eq!(
            render_from_template("{tags} | {bpm} | {key} | {energy}", &owned, None, None, None),
            "Techno"
        );
        assert_eq!(
            render_from_template(
                "{tags} | {bpm} | {key} | {energy}",
                &owned,
                Some(128.0),
                None,
                Some(7.0)
            ),
            "Techno | BPM-128 | ENERGY-7"
        );
        // No tags and nothing else -> empty Comment.
        assert_eq!(render(&[], None, None, None), "");
    }

    #[test]
    fn empty_tag_list_drops_the_placeholder() {
        assert_eq!(
            render(&["  "], Some(120.0), None, None),
            "BPM-120"
        );
    }

    #[test]
    fn long_tag_is_capped_at_48_chars_with_ellipsis() {
        let long = "x".repeat(80);
        let out = render(&[&long], None, None, None);
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
        let owned: Vec<String> = vec!["Techno".into()];
        let tpl = format!("{{tags}} {}", "x ".repeat(300));
        let out = render_from_template(&tpl, &owned, None, None, None);
        assert_eq!(out.chars().count(), 512, "capped at the comment ceiling");
        assert!(out.ends_with("..."), "truncated with the marker");
    }

    #[test]
    fn unknown_placeholder_falls_back_to_default() {
        let owned: Vec<String> = vec!["Techno".into()];
        let out = render_from_template("{title} {tags}", &owned, Some(128.0), None, None);
        assert_eq!(out, "Techno BPM-128", "default template applied");
    }

    #[test]
    fn empty_template_falls_back_to_default() {
        let owned: Vec<String> = vec!["Techno".into()];
        assert_eq!(
            render_from_template("   ", &owned, None, None, None),
            "Techno"
        );
    }

    #[test]
    fn umlauts_and_punctuation_become_dashes() {
        // Documented rule: everything outside [A-Za-z0-9_-] -> `-`, runs collapse.
        assert_eq!(render(&["Röck & Roll"], None, None, None), "R-ck-Roll");
        assert_eq!(render(&["!!!"], None, None, None), "", "all separators -> dropped");
    }

    #[test]
    fn render_is_deterministic_and_order_independent() {
        let tags = vec!["zeta".into(), "alpha".into(), "mu".into()];
        let first = render_from_template(DEFAULT_TEMPLATE, &tags, Some(120.0), Some("C-Major"), Some(5.0));
        for _ in 0..10 {
            assert_eq!(
                render_from_template(DEFAULT_TEMPLATE, &tags, Some(120.0), Some("C-Major"), Some(5.0)),
                first
            );
        }
        let reordered = vec!["mu".into(), "zeta".into(), "alpha".into()];
        assert_eq!(
            render_from_template(DEFAULT_TEMPLATE, &reordered, Some(120.0), Some("C-Major"), Some(5.0)),
            first
        );
    }

    #[test]
    fn write_comment_writes_then_skips_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("test.mp3");
        std::fs::copy(fixture("test.mp3"), &dest).unwrap();

        assert!(write_comment(&dest, "Techno BPM-128").unwrap(), "first write");
        let mtime = std::fs::metadata(&dest).unwrap().modified().unwrap();
        assert!(!write_comment(&dest, "Techno BPM-128").unwrap(), "unchanged -> no write");
        assert_eq!(
            std::fs::metadata(&dest).unwrap().modified().unwrap(),
            mtime,
            "mtime preserved on an unchanged render"
        );
        assert!(write_comment(&dest, "House").unwrap(), "changed -> write");
    }

    #[test]
    fn write_comment_clears_on_empty() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("test.flac");
        std::fs::copy(fixture("test.flac"), &dest).unwrap();

        assert!(write_comment(&dest, "Techno").unwrap());
        assert!(write_comment(&dest, "").unwrap(), "clearing changes the file");
        assert!(!write_comment(&dest, "").unwrap(), "already empty -> no write");
    }
}
