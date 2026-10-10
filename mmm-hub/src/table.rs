//! Shared table primitives: server-side **sortable headers** and tokenised
//! **fuzzy full-text search** used uniformly across every list view.
//!
//! Search is order-independent: the query is split into alphanumeric tokens and
//! a row matches when *every* token appears (as a case-insensitive substring) in
//! *any* of the given columns. So "warehouse dark" and "dark warehouse" match the
//! same rows, and partial words ("wareh") match too.
//!
//! Sorting is whitelisted per view (`field -> SQL expression`), so the `?sort=`
//! parameter can never inject SQL. Header links are built from the raw query
//! string, preserving all other filters.

use sqlx::{QueryBuilder, Sqlite};

/// Lowercased alphanumeric tokens of a free-text query (punctuation dropped).
pub fn tokens(q: &str) -> Vec<String> {
    q.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| t.to_lowercase())
        .collect()
}

/// Does `hay` (already lowercased) contain every token of `toks`? Order-independent;
/// an empty token list always matches. For in-memory (Rust-aggregated) lists.
pub fn matches(hay: &str, toks: &[String]) -> bool {
    toks.is_empty() || toks.iter().all(|t| hay.contains(t.as_str()))
}

/// Bounded Levenshtein distance; returns `max + 1` once it is known to exceed
/// `max` (so callers can test `<= 1` cheaply).
pub fn levenshtein(a: &str, b: &str, max: usize) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    if a.len().abs_diff(b.len()) > max {
        return max + 1;
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![0usize; b.len() + 1];
        cur[0] = i;
        let mut row_min = cur[0];
        for j in 1..=b.len() {
            let cost = if a[i - 1] == b[j - 1] { 0 } else { 1 };
            let v = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
            cur[j] = v;
            row_min = row_min.min(v);
        }
        if row_min > max {
            return max + 1;
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Typo-tolerant variant of [`matches`]: every token either appears as a
/// substring of `hay`, or is within edit distance `<= 1` of one of `hay`'s words.
/// Used only where matching happens in Rust (the SQL pages use a stricter pass
/// first and fall back to this on an empty result).
pub fn matches_typo(hay: &str, toks: &[String]) -> bool {
    toks.iter().all(|t| {
        if t.len() < 3 || hay.contains(t.as_str()) {
            // Very short tokens stay strict to avoid noisy over-matching.
            return hay.contains(t.as_str());
        }
        hay.split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty() && w.len().abs_diff(t.len()) <= 1)
            .any(|w| levenshtein(t, w, 1) <= 1)
    })
}

/// An SQL fragment (AND-able) plus its binds: every query token must appear in
/// at least one of `cols`. Returns `None` for an empty query or no columns.
///
/// `cols` may be plain columns or expressions (they are wrapped in
/// `COALESCE(...,'')`), e.g. `"t.title"` or `"album"` (an alias after wrapping).
pub fn fuzzy_clause(q: &str, cols: &[&str]) -> Option<(String, Vec<String>)> {
    let toks = tokens(q);
    if toks.is_empty() || cols.is_empty() {
        return None;
    }
    let per = cols
        .iter()
        .map(|c| format!("instr(lower(COALESCE({c}, '')), ?) > 0"))
        .collect::<Vec<_>>()
        .join(" OR ");
    let mut sql = String::new();
    let mut binds: Vec<String> = Vec::new();
    for (i, t) in toks.iter().enumerate() {
        if i > 0 {
            sql.push_str(" AND ");
        }
        sql.push('(');
        sql.push_str(&per);
        sql.push(')');
        for _ in cols {
            binds.push(t.clone());
        }
    }
    Some((sql, binds))
}

/// Append the fuzzy clause to a query already sitting inside a `WHERE 1 = 1`.
/// Builds placeholders via `push_bind` (never embeds `?` directly).
pub fn push_fuzzy(qb: &mut QueryBuilder<'_, Sqlite>, q: &str, cols: &[&str]) {
    let toks = tokens(q);
    if toks.is_empty() || cols.is_empty() {
        return;
    }
    qb.push(" AND (");
    for (i, t) in toks.iter().enumerate() {
        if i > 0 {
            qb.push(" AND ");
        }
        qb.push("(");
        for (j, c) in cols.iter().enumerate() {
            if j > 0 {
                qb.push(" OR ");
            }
            qb.push(format!("instr(lower(COALESCE({c}, '')), "));
            qb.push_bind(t.clone());
            qb.push(") > 0");
        }
        qb.push(")");
    }
    qb.push(")");
}

/// Append a whitelisted `ORDER BY`. `allowed` maps sort fields to SQL
/// expressions; an unknown/absent `field` falls back to `default_field`.
pub fn order_by(
    qb: &mut QueryBuilder<'_, Sqlite>,
    field: &str,
    dir: &str,
    allowed: &[(&str, &str)],
    default_field: &str,
) {
    let expr = allowed
        .iter()
        .find(|(f, _)| *f == field)
        .or_else(|| allowed.iter().find(|(f, _)| *f == default_field))
        .map(|(_, c)| *c)
        .unwrap_or(default_field);
    let d = if dir.eq_ignore_ascii_case("desc") {
        "DESC"
    } else {
        "ASC"
    };
    qb.push(" ORDER BY ").push(expr).push(" ").push(d);
}

/// Normalise a request's `?sort=`/`?dir=` against a whitelist. Returns the
/// resolved `(field, dir)`; `dir` defaults to `default_dir` for the default
/// field (e.g. `desc` for "most first") and `asc` otherwise.
pub fn resolve_sort(
    sort: Option<&str>,
    dir: Option<&str>,
    allowed: &[(&str, &str)],
    default_field: &str,
    default_dir: &str,
) -> (String, String) {
    let field = sort
        .map(|s| s.trim())
        .filter(|s| allowed.iter().any(|(f, _)| *f == *s))
        .unwrap_or(default_field)
        .to_string();
    let dir = match dir.map(|d| d.trim().to_lowercase()).as_deref() {
        Some("asc") => "asc".to_string(),
        Some("desc") => "desc".to_string(),
        _ if field == default_field => default_dir.to_string(),
        _ => "asc".to_string(),
    };
    (field, dir)
}

/// A rendered, clickable table header for one column.
pub struct SortHead {
    pub label: String,
    pub href: String,
    pub active: bool,
    /// `▲`/`▼` when active, else empty.
    pub arrow: String,
}

/// Build a `SortHead` for `field`. The href keeps every other query parameter
/// and toggles asc/desc (a click on the active column flips direction).
pub fn sort_head(
    raw_query: &str,
    field: &str,
    label: &str,
    cur_field: &str,
    cur_dir: &str,
) -> SortHead {
    let active = cur_field == field;
    let next_dir = if active && cur_dir.eq_ignore_ascii_case("asc") {
        "desc"
    } else {
        "asc"
    };
    let mut parts: Vec<String> = Vec::new();
    for kv in raw_query.split('&') {
        if kv.is_empty() {
            continue;
        }
        let k = kv.split('=').next().unwrap_or("");
        if k == "sort" || k == "dir" {
            continue;
        }
        parts.push(kv.to_string());
    }
    parts.push(format!("sort={}", urlencoding::encode(field)));
    parts.push(format!("dir={next_dir}"));
    SortHead {
        label: label.to_string(),
        href: format!("?{}", parts.join("&")),
        active,
        arrow: if active {
            if cur_dir.eq_ignore_ascii_case("asc") {
                "▲".to_string()
            } else {
                "▼".to_string()
            }
        } else {
            String::new()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_strip_punctuation_and_case() {
        assert_eq!(tokens("Dark, Warehouse!"), vec!["dark", "warehouse"]);
        assert_eq!(tokens("   "), Vec::<String>::new());
    }

    #[test]
    fn fuzzy_clause_is_order_independent() {
        let a = fuzzy_clause("dark warehouse", &["name"]).unwrap();
        let b = fuzzy_clause("warehouse dark", &["name"]).unwrap();
        assert_eq!(a.1.len(), 2);
        // Same bind multiset regardless of word order.
        let mut a2 = a.1;
        let mut b2 = b.1;
        a2.sort();
        b2.sort();
        assert_eq!(a2, b2);
    }

    #[test]
    fn resolve_sort_whitelists_and_defaults() {
        let allowed = [("name", "t.name"), ("tracks", "track_count")];
        assert_eq!(
            resolve_sort(Some("tracks"), Some("desc"), &allowed, "name", "asc"),
            ("tracks".to_string(), "desc".to_string())
        );
        // Unknown field -> default.
        assert_eq!(
            resolve_sort(Some("DROP TABLE"), None, &allowed, "name", "desc"),
            ("name".to_string(), "desc".to_string())
        );
    }

    #[test]
    fn sort_head_toggles_and_preserves_params() {
        let h = sort_head("q=dark&mine=1", "name", "Tag", "name", "asc");
        assert!(h.active);
        assert_eq!(h.arrow, "▲");
        assert!(h.href.contains("q=dark"));
        assert!(h.href.contains("mine=1"));
        assert!(h.href.contains("sort=name"));
        assert!(h.href.contains("dir=desc"));
        assert!(!h.href.contains("sort=name&dir=asc&sort"));
    }

    #[test]
    fn levenshtein_bounded() {
        assert_eq!(levenshtein("dark", "dark", 1), 0);
        assert_eq!(levenshtein("dork", "dark", 1), 1);
        assert_eq!(levenshtein("darkk", "dark", 1), 1);
        // Exceeds max -> returns max + 1.
        assert_eq!(levenshtein("kitten", "sitting", 2), 3);
    }

    #[test]
    fn matches_typo_tolerates_one_edit() {
        let toks = |q: &str| tokens(q);
        // Exact substring still matches.
        assert!(matches_typo("warehouse dark", &toks("dark")));
        // One substitution.
        assert!(matches_typo("warehouse dark", &toks("dork")));
        // One insertion on the data word.
        assert!(matches_typo("warehouse dark", &toks("darkk")));
        assert!(!matches_typo("warehouse dark", &toks("zebra")));
        // Short tokens stay strict.
        assert!(!matches_typo("warehouse", &toks("wx")));
    }
}
