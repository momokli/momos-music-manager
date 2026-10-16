//! Semantic tag clusters (#261–#268).
//!
//! A cluster groups semantically-related tags of the **same type** (e.g. Mood
//! *dark* = dark, düster, …). Clusters are built automatically from tag-name
//! similarity (a conservative, deterministic baseline; a pluggable embedding
//! provider can replace it later) and are fully editable afterwards (rename,
//! move tags in/out, delete) — the collective's curation wins until the next
//! rebuild.

use askama::Template;
use axum::Router;
use axum::extract::{Form, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use sqlx::{Row, SqlitePool};

use crate::api::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/tag-clusters", get(clusters_page))
        .route("/tag-clusters/rebuild", post(clusters_rebuild))
        .route("/tag-clusters/{id}/rename", post(cluster_rename))
        .route("/tag-clusters/{id}/member/add", post(cluster_member_add))
        .route(
            "/tag-clusters/{id}/member/remove",
            post(cluster_member_remove),
        )
        .route("/tag-clusters/{id}/delete", post(cluster_delete))
        .with_state(state)
}

fn render<T: Template>(tpl: &T) -> Response {
    match tpl.render() {
        Ok(html) => Html(html).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Template-Fehler: {e}"),
        )
            .into_response(),
    }
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

// ── clustering heuristic (pure, unit-tested) ────────────────────────────────

/// Lowercase, strip punctuation, collapse whitespace.
pub fn normalize(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0usize; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = if ca == cb { 0 } else { 1 };
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Two normalised tag names are "similar" when they are equal, share a token
/// (len ≥ 3), one is a prefix of the other (len ≥ 4), or are within one edit of
/// each other (similar length, len ≥ 4).
pub fn similar(a: &str, b: &str) -> bool {
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let ta: Vec<&str> = a.split(' ').collect();
    let tb: Vec<&str> = b.split(' ').collect();
    for x in &ta {
        for y in &tb {
            if x == y && x.len() >= 3 {
                return true;
            }
        }
    }
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if short.len() >= 4 && long.starts_with(short) {
        return true;
    }
    if a.len().abs_diff(b.len()) <= 1 && a.len() >= 4 {
        let (ca, cb): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
        return levenshtein(&ca, &cb) <= 1;
    }
    false
}

/// Union-find components over `similar`-linked indices; only groups of ≥ 2.
pub fn cluster(normalized: &[String]) -> Vec<Vec<usize>> {
    let n = normalized.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut Vec<usize>, x: usize) -> usize {
        let mut r = x;
        while p[r] != r {
            r = p[r];
        }
        let mut c = x;
        while p[c] != r {
            let next = p[c];
            p[c] = r;
            c = next;
        }
        r
    }
    for i in 0..n {
        for j in (i + 1)..n {
            if similar(&normalized[i], &normalized[j]) {
                let (ri, rj) = (find(&mut parent, i), find(&mut parent, j));
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for i in 0..n {
        groups.entry(find(&mut parent, i)).or_default().push(i);
    }
    groups.into_values().filter(|g| g.len() >= 2).collect()
}

/// Rebuild the auto clusters for one tag type. Replaces existing clusters of
/// that type (manual edits are re-applied by the collective afterwards).
pub async fn rebuild(pool: &SqlitePool, tag_type: &str) -> anyhow::Result<usize> {
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, name FROM hub_tags WHERE COALESCE(type, '') = ?1 ORDER BY name")
            .bind(tag_type)
            .fetch_all(pool)
            .await?;
    let names: Vec<String> = rows.iter().map(|(_, n)| normalize(n)).collect();
    let comps = cluster(&names);

    let now = now_iso();
    sqlx::query("DELETE FROM hub_tag_clusters WHERE tag_type = ?1")
        .bind(tag_type)
        .execute(pool)
        .await?;
    let mut made = 0usize;
    for comp in comps {
        let label = comp
            .iter()
            .map(|&i| rows[i].1.as_str())
            .min_by_key(|n| n.len())
            .unwrap_or("Cluster")
            .to_string();
        let cid: i64 = sqlx::query_scalar(
            "INSERT INTO hub_tag_clusters (tag_type, label, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?3) RETURNING id",
        )
        .bind(tag_type)
        .bind(&label)
        .bind(&now)
        .fetch_one(pool)
        .await?;
        for &i in &comp {
            sqlx::query(
                "INSERT OR IGNORE INTO hub_tag_cluster_members (cluster_id, tag_id, added_at)
                 VALUES (?1, ?2, ?3)",
            )
            .bind(cid)
            .bind(rows[i].0)
            .bind(&now)
            .execute(pool)
            .await?;
        }
        made += 1;
    }
    Ok(made)
}

// ── page ─────────────────────────────────────────────────────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
struct ClusterQuery {
    /// `''` = untyped, otherwise the tag-type key. Bound to the `type` query
    /// key (not the Rust field name `type_`) so the `type=` links from
    /// `tag_clusters.html` and the rebuild redirect actually select a type.
    #[serde(rename = "type")]
    type_: Option<String>,
    scope: Option<String>,
}

#[derive(Template)]
#[template(path = "tag_clusters.html")]
struct ClustersPage {
    nav: crate::ui::Nav,
    flash: String,
    current_type: String,
    scope: String,
    types: Vec<TypeOpt>,
    cards: Vec<ClusterCard>,
    unclustered: i64,
    cluster_count: usize,
}

struct TypeOpt {
    value: String,
    label: String,
    selected: bool,
}

struct ClusterCard {
    id: i64,
    label: String,
    members: Vec<MemberChip>,
    addable: Vec<MemberChip>,
}

struct MemberChip {
    tag_id: i64,
    name: String,
    owner: String,
}

#[derive(Deserialize)]
struct RebuildForm {
    #[serde(default, rename = "type")]
    tag_type: Option<String>,
}

#[derive(Deserialize)]
struct RenameForm {
    label: String,
}

#[derive(Deserialize)]
struct MemberForm {
    tag_id: i64,
}

async fn clusters_page(
    State(st): State<AppState>,
    Query(q): Query<ClusterQuery>,
    headers: HeaderMap,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "clusters").await else {
        return Redirect::to("/login").into_response();
    };
    let current_type = q.type_.unwrap_or_default();
    let scope = match q.scope.as_deref() {
        Some("all") => "all",
        _ => "mine",
    }
    .to_string();
    let all = scope == "all";

    let types = std::iter::once(TypeOpt {
        value: String::new(),
        label: "Ohne Typ".to_string(),
        selected: current_type.is_empty(),
    })
    .chain(
        crate::tagtypes::options()
            .into_iter()
            .map(|(v, l, _)| TypeOpt {
                value: v.to_string(),
                label: l.to_string(),
                selected: current_type == v,
            }),
    )
    .collect();

    // Clusters of this type + their member tags (filtered by scope).
    let cluster_rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, label FROM hub_tag_clusters WHERE tag_type = ?1 ORDER BY label COLLATE NOCASE",
    )
    .bind(&current_type)
    .fetch_all(&st.pool)
    .await
    .unwrap_or_default();

    let me = nav.slug.clone();
    let mut cards = Vec::new();
    for (cid, label) in &cluster_rows {
        let members: Vec<MemberChip> = sqlx::query(
            "SELECT t.id AS tag_id, t.name AS name, u.slug AS owner
               FROM hub_tag_cluster_members m
               JOIN hub_tags t ON t.id = m.tag_id
               JOIN hub_users u ON u.id = t.owner_user_id
              WHERE m.cluster_id = ?1
              ORDER BY t.name",
        )
        .bind(cid)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| all || r.get::<Option<String>, _>("owner").as_deref() == Some(me.as_str()))
        .map(|r| MemberChip {
            tag_id: r.get("tag_id"),
            name: r.get::<Option<String>, _>("name").unwrap_or_default(),
            owner: r.get::<Option<String>, _>("owner").unwrap_or_default(),
        })
        .collect();
        if members.is_empty() && !all {
            continue;
        }
        // My tags of this type not already in this cluster → addable.
        let addable: Vec<MemberChip> = sqlx::query(
            "SELECT t.id AS tag_id, t.name AS name
               FROM hub_tags t
              WHERE COALESCE(t.type, '') = ?1 AND t.owner_user_id = ?2
                AND t.id NOT IN (SELECT tag_id FROM hub_tag_cluster_members WHERE cluster_id = ?3)
              ORDER BY t.name LIMIT 200",
        )
        .bind(&current_type)
        .bind(nav.id)
        .bind(cid)
        .fetch_all(&st.pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|r| MemberChip {
            tag_id: r.get("tag_id"),
            name: r.get::<Option<String>, _>("name").unwrap_or_default(),
            owner: me.clone(),
        })
        .collect();
        cards.push(ClusterCard {
            id: *cid,
            label: label.clone(),
            members,
            addable,
        });
    }

    // Count tags of this type that are in no cluster (for the current scope).
    let unclustered: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_tags t
          WHERE COALESCE(t.type,'') = ?1
            AND (?2 = 1 OR t.owner_user_id = ?3)
            AND t.id NOT IN (SELECT tag_id FROM hub_tag_cluster_members)",
    )
    .bind(&current_type)
    .bind(if all { 1 } else { 0 })
    .bind(nav.id)
    .fetch_one(&st.pool)
    .await
    .unwrap_or(0);

    render(&ClustersPage {
        nav,
        flash: String::new(),
        current_type,
        scope,
        types,
        cluster_count: cards.len(),
        cards,
        unclustered,
    })
}

async fn clusters_rebuild(
    State(st): State<AppState>,
    headers: HeaderMap,
    Form(f): Form<RebuildForm>,
) -> Response {
    let Some(_nav) = crate::ui::nav(&st, &headers, "clusters").await else {
        return Redirect::to("/login").into_response();
    };
    let ty = f.tag_type.unwrap_or_default();
    let _ = rebuild(&st.pool, &ty).await;
    Redirect::to(&format!("/tag-clusters?type={}", urlencoding::encode(&ty))).into_response()
}

async fn cluster_rename(
    State(st): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    headers: HeaderMap,
    Form(f): Form<RenameForm>,
) -> Response {
    let Some(_nav) = crate::ui::nav(&st, &headers, "clusters").await else {
        return Redirect::to("/login").into_response();
    };
    if !f.label.trim().is_empty() {
        let _ =
            sqlx::query("UPDATE hub_tag_clusters SET label = ?2, updated_at = ?3 WHERE id = ?1")
                .bind(id)
                .bind(f.label.trim())
                .bind(now_iso())
                .execute(&st.pool)
                .await;
    }
    Redirect::to("/tag-clusters").into_response()
}

async fn cluster_member_add(
    State(st): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    headers: HeaderMap,
    Form(f): Form<MemberForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "clusters").await else {
        return Redirect::to("/login").into_response();
    };
    // Only move my own tags into a cluster (others' tags stay untouched).
    let owned: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM hub_tags WHERE id = ?1 AND owner_user_id = ?2")
            .bind(f.tag_id)
            .bind(nav.id)
            .fetch_optional(&st.pool)
            .await
            .ok()
            .flatten();
    if owned.is_some() {
        let _ = sqlx::query(
            "INSERT OR IGNORE INTO hub_tag_cluster_members (cluster_id, tag_id, added_at)
             VALUES (?1, ?2, ?3)",
        )
        .bind(id)
        .bind(f.tag_id)
        .bind(now_iso())
        .execute(&st.pool)
        .await;
    }
    Redirect::to("/tag-clusters").into_response()
}

async fn cluster_member_remove(
    State(st): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    headers: HeaderMap,
    Form(f): Form<MemberForm>,
) -> Response {
    let Some(nav) = crate::ui::nav(&st, &headers, "clusters").await else {
        return Redirect::to("/login").into_response();
    };
    let _ = sqlx::query(
        "DELETE FROM hub_tag_cluster_members
          WHERE cluster_id = ?1 AND tag_id = ?2
            AND tag_id IN (SELECT id FROM hub_tags WHERE owner_user_id = ?3)",
    )
    .bind(id)
    .bind(f.tag_id)
    .bind(nav.id)
    .execute(&st.pool)
    .await;
    Redirect::to("/tag-clusters").into_response()
}

async fn cluster_delete(
    State(st): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some(_nav) = crate::ui::nav(&st, &headers, "clusters").await else {
        return Redirect::to("/login").into_response();
    };
    let _ = sqlx::query("DELETE FROM hub_tag_clusters WHERE id = ?1")
        .bind(id)
        .execute(&st.pool)
        .await;
    Redirect::to("/tag-clusters").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_and_collapses() {
        assert_eq!(normalize("Dark-Mood!"), "dark mood");
        assert_eq!(normalize("  Peak  5 "), "peak 5");
    }

    #[test]
    fn similarity_rules() {
        assert!(similar("dark", "dark"));
        assert!(similar("dark", "darker"));
        assert!(similar("peak", "peaks"));
        assert!(similar("driving", "drivin")); // edit distance 1
        assert!(!similar("dark", "duster"));
        assert!(!similar("", "x"));
    }

    #[test]
    fn clusters_group_variants() {
        let names: Vec<String> = ["dark", "darker", "peak", "peaks", "solo"]
            .iter()
            .map(|s| normalize(s))
            .collect();
        let comps = cluster(&names);
        assert_eq!(comps.len(), 2, "dark* and peak* group, solo stays alone");
        let sizes: Vec<usize> = comps.iter().map(|c| c.len()).collect();
        assert!(sizes.contains(&2));
    }
}
