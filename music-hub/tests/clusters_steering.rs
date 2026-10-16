//! Integration tests for tag-cluster **steering** (#268): the mutating
//! handlers (rename, member add/remove, delete) plus the schema/CASCADE
//! guarantees and the scope-aware read page.
//!
//! Handlers are deliberately quiet on missing rights (a no-op redirect, not an
//! error), so permissions are asserted against the DB, never the HTTP body.

mod common;

use music_hub::clusters;

const OLD: &str = "2020-01-01T00:00:00+00:00";

/// Insert a tag for `owner` and return its id.
async fn insert_tag(pool: &sqlx::SqlitePool, owner: i64, name: &str, ty: &str) -> i64 {
    // `hub_tags.type` is CHECK-constrained to NULL or a known type, so an empty
    // `ty` means "untyped" (NULL).
    let ty: Option<&str> = if ty.is_empty() { None } else { Some(ty) };
    sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, type, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5) RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .bind(name)
    .bind(ty)
    .bind(OLD)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Insert a cluster of `tag_type` with `label` and return its id.
async fn insert_cluster(pool: &sqlx::SqlitePool, tag_type: &str, label: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tag_clusters (tag_type, label, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?3) RETURNING id",
    )
    .bind(tag_type)
    .bind(label)
    .bind(OLD)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Add a tag to a cluster directly (bypassing the handlers).
async fn insert_member(pool: &sqlx::SqlitePool, cluster_id: i64, tag_id: i64) {
    sqlx::query(
        "INSERT OR IGNORE INTO hub_tag_cluster_members (cluster_id, tag_id, added_at)
         VALUES (?1, ?2, ?3)",
    )
    .bind(cluster_id)
    .bind(tag_id)
    .bind(OLD)
    .execute(pool)
    .await
    .unwrap();
}

async fn member_count(pool: &sqlx::SqlitePool, cluster_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM hub_tag_cluster_members WHERE cluster_id = ?1")
        .bind(cluster_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn cluster_label(pool: &sqlx::SqlitePool, cluster_id: i64) -> String {
    sqlx::query_scalar("SELECT label FROM hub_tag_clusters WHERE id = ?1")
        .bind(cluster_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ── 1. login guard on every steering route ──────────────────────────────────
#[tokio::test]
async fn steering_routes_require_login() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let tag = insert_tag(&app.pool, alice, "mine", "MOOD").await;
    let cid = insert_cluster(&app.pool, "MOOD", "Original").await;
    insert_member(&app.pool, cid, tag).await;

    let routes: [(&str, &[(&str, &str)]); 5] = [
        (&format!("/tag-clusters/{cid}/rename"), &[("label", "Hacked")]),
        (
            &format!("/tag-clusters/{cid}/member/add"),
            &[("tag_id", "1")],
        ),
        (
            &format!("/tag-clusters/{cid}/member/remove"),
            &[("tag_id", "1")],
        ),
        (&format!("/tag-clusters/{cid}/delete"), &[]),
        // Mutating route too (`clusters_rebuild` → `nav()`); must be guarded.
        ("/tag-clusters/rebuild", &[("type", "MOOD")]),
    ];

    for (path, form) in routes {
        let resp = app
            .client()
            .post(app.url(path))
            .form(form)
            .send()
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::SEE_OTHER,
            "{path} without session must redirect"
        );
        assert_eq!(resp.headers()["location"], "/login", "{path} → /login");
    }

    // Nothing was mutated.
    assert_eq!(cluster_label(&app.pool, cid).await, "Original");
    assert_eq!(member_count(&app.pool, cid).await, 1);
}

// ── 2. schema sanity + CASCADE ──────────────────────────────────────────────
#[tokio::test]
async fn cluster_schema_exists_and_member_cascade() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let tag = insert_tag(&app.pool, alice, "mine", "MOOD").await;
    let cid = insert_cluster(&app.pool, "MOOD", "C").await;
    insert_member(&app.pool, cid, tag).await;
    assert_eq!(member_count(&app.pool, cid).await, 1);

    // Deleting the cluster removes its member rows (ON DELETE CASCADE).
    sqlx::query("DELETE FROM hub_tag_clusters WHERE id = ?1")
        .bind(cid)
        .execute(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        member_count(&app.pool, cid).await,
        0,
        "cluster delete must cascade to members"
    );

    // Deleting the tag likewise removes the membership row.
    let tag2 = insert_tag(&app.pool, alice, "mine2", "MOOD").await;
    let cid2 = insert_cluster(&app.pool, "MOOD", "C2").await;
    insert_member(&app.pool, cid2, tag2).await;
    sqlx::query("DELETE FROM hub_tags WHERE id = ?1")
        .bind(tag2)
        .execute(&app.pool)
        .await
        .unwrap();
    assert_eq!(
        member_count(&app.pool, cid2).await,
        0,
        "tag delete must cascade to members"
    );
}

// ── 3. scope=mine vs scope=all page read ────────────────────────────────────
#[tokio::test]
async fn clusters_page_scope_mine_vs_all() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let bob = app.seed.bob;
    let cookie = app.session_cookie(alice).await;

    // Untyped cluster (`tag_type = ''`) — the page reads the `type` query
    // param (see `ClusterQuery`), and an empty/absent type selects untyped
    // clusters.
    let mine = insert_tag(&app.pool, alice, "alpha-mine", "").await;
    let other = insert_tag(&app.pool, bob, "beta-other", "").await;
    let cid = insert_cluster(&app.pool, "", "Shared").await;
    insert_member(&app.pool, cid, mine).await;
    insert_member(&app.pool, cid, other).await;

    let get = |query: &str| {
        let app = &app;
        let cookie = cookie.clone();
        let url = app.url(&format!("/tag-clusters{query}"));
        async move {
            app.client()
                .get(url)
                .header("Cookie", &cookie)
                .send()
                .await
                .unwrap()
        }
    };

    let mine_resp = get("?scope=mine").await;
    assert_eq!(mine_resp.status(), reqwest::StatusCode::OK);
    let mine_html = mine_resp.text().await.unwrap();
    assert!(mine_html.contains("alpha-mine"), "own tag must show");
    assert!(!mine_html.contains("beta-other"), "foreign tag hidden (mine)");

    let all_resp = get("?scope=all").await;
    assert_eq!(all_resp.status(), reqwest::StatusCode::OK);
    let all_html = all_resp.text().await.unwrap();
    assert!(all_html.contains("alpha-mine"), "own tag must show (all)");
    assert!(all_html.contains("beta-other"), "foreign tag visible (all)");

    // Missing/unknown scope defaults to `mine`.
    let default_resp = get("").await;
    assert_eq!(default_resp.status(), reqwest::StatusCode::OK);
    let default_html = default_resp.text().await.unwrap();
    assert!(
        default_html.contains("alpha-mine"),
        "own tag must show when scope defaults to mine"
    );
    assert!(
        !default_html.contains("beta-other"),
        "missing scope must default to mine"
    );
}

// ── 3b. typed read path: ?type=<TYPE> selects that type (regression) ────────
//
// Pins the fix for the `ClusterQuery` field rename: the page must bind the
// `type` query key used by every caller (`tag_clusters.html` links, the
// rebuild redirect). Before the fix `type_` bound the key `type_`, so
// `?type=MOOD` silently rendered untyped clusters and typed clusters were
// unreachable from the UI (Issue #480).
#[tokio::test]
async fn clusters_page_typed_read_path() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    let mood_tag = insert_tag(&app.pool, alice, "mood-tag", "MOOD").await;
    let mood_cid = insert_cluster(&app.pool, "MOOD", "MoodCluster").await;
    insert_member(&app.pool, mood_cid, mood_tag).await;

    let untyped_tag = insert_tag(&app.pool, alice, "untyped-tag", "").await;
    let untyped_cid = insert_cluster(&app.pool, "", "UntypedCluster").await;
    insert_member(&app.pool, untyped_cid, untyped_tag).await;

    let html = app
        .client()
        .get(app.url("/tag-clusters?type=MOOD&scope=mine"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        html.contains("MoodCluster"),
        "?type=MOOD must render the MOOD cluster (Issue #480)"
    );
    assert!(
        !html.contains("UntypedCluster"),
        "?type=MOOD must not render untyped clusters"
    );
}

// ── 4. rename persists only for a non-empty label ───────────────────────────
#[tokio::test]
async fn rename_persists_only_nonempty_label() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;
    let cid = insert_cluster(&app.pool, "MOOD", "Original").await;

    let resp = app
        .client()
        .post(app.url(&format!("/tag-clusters/{cid}/rename")))
        .header("Cookie", &cookie)
        .form(&[("label", "Neu")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/tag-clusters");
    assert_eq!(cluster_label(&app.pool, cid).await, "Neu");
    let updated: String = sqlx::query_scalar("SELECT updated_at FROM hub_tag_clusters WHERE id = ?1")
        .bind(cid)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_ne!(updated, OLD, "updated_at must be refreshed");

    // Whitespace-only label is refused: label stays put.
    let resp = app
        .client()
        .post(app.url(&format!("/tag-clusters/{cid}/rename")))
        .header("Cookie", &cookie)
        .form(&[("label", "   ")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        cluster_label(&app.pool, cid).await,
        "Neu",
        "blank label must not overwrite"
    );
}

// ── 5. member add: only my own tags ─────────────────────────────────────────
#[tokio::test]
async fn member_add_only_own_tags() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let bob = app.seed.bob;
    let cookie = app.session_cookie(alice).await;
    let cid = insert_cluster(&app.pool, "MOOD", "C").await;

    let own = insert_tag(&app.pool, alice, "own", "MOOD").await;
    let foreign = insert_tag(&app.pool, bob, "foreign", "MOOD").await;

    let add = |tag_id: i64| {
        let app = &app;
        let cookie = cookie.clone();
        async move {
            app.client()
                .post(app.url(&format!("/tag-clusters/{cid}/member/add")))
                .header("Cookie", &cookie)
                .form(&[("tag_id", &tag_id.to_string())])
                .send()
                .await
                .unwrap()
        }
    };

    let resp = add(own).await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/tag-clusters");
    assert_eq!(member_count(&app.pool, cid).await, 1);

    // Idempotent (INSERT OR IGNORE) — a second add stays at one row.
    add(own).await;
    assert_eq!(member_count(&app.pool, cid).await, 1, "add is idempotent");

    // A foreign tag is silently ignored.
    add(foreign).await;
    assert_eq!(
        member_count(&app.pool, cid).await,
        1,
        "foreign tag must not be added"
    );
    let has_foreign: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_tag_cluster_members WHERE cluster_id = ?1 AND tag_id = ?2",
    )
    .bind(cid)
    .bind(foreign)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(has_foreign, 0);
}

// ── 6. member remove: only my own tags ──────────────────────────────────────
#[tokio::test]
async fn member_remove_only_own_tags() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let bob = app.seed.bob;
    let cookie = app.session_cookie(alice).await;
    let cid = insert_cluster(&app.pool, "MOOD", "C").await;

    let own = insert_tag(&app.pool, alice, "own", "MOOD").await;
    let foreign = insert_tag(&app.pool, bob, "foreign", "MOOD").await;
    insert_member(&app.pool, cid, own).await;
    insert_member(&app.pool, cid, foreign).await;
    assert_eq!(member_count(&app.pool, cid).await, 2);

    let remove = |tag_id: i64| {
        let app = &app;
        let cookie = cookie.clone();
        async move {
            app.client()
                .post(app.url(&format!("/tag-clusters/{cid}/member/remove")))
                .header("Cookie", &cookie)
                .form(&[("tag_id", &tag_id.to_string())])
                .send()
                .await
                .unwrap()
        }
    };

    // Removing a foreign tag is a no-op: the row survives.
    let resp = remove(foreign).await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        member_count(&app.pool, cid).await,
        2,
        "foreign member must remain"
    );

    // Removing my own tag works.
    remove(own).await;
    assert_eq!(member_count(&app.pool, cid).await, 1, "own member removed");
    let has_own: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_tag_cluster_members WHERE cluster_id = ?1 AND tag_id = ?2",
    )
    .bind(cid)
    .bind(own)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(has_own, 0);
}

// ── 7. delete cascades members ──────────────────────────────────────────────
#[tokio::test]
async fn delete_cluster_cascades_members() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;
    let cid = insert_cluster(&app.pool, "MOOD", "C").await;
    let t1 = insert_tag(&app.pool, alice, "one", "MOOD").await;
    let t2 = insert_tag(&app.pool, alice, "two", "MOOD").await;
    insert_member(&app.pool, cid, t1).await;
    insert_member(&app.pool, cid, t2).await;
    assert_eq!(member_count(&app.pool, cid).await, 2);

    let resp = app
        .client()
        .post(app.url(&format!("/tag-clusters/{cid}/delete")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/tag-clusters");

    let clusters: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_tag_clusters WHERE id = ?1")
        .bind(cid)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(clusters, 0, "cluster row removed");
    assert_eq!(member_count(&app.pool, cid).await, 0, "members cascade away");
}

// ── 8. rebuild replaces manual member edits, deterministically ───────────────
#[tokio::test]
async fn rebuild_replaces_manual_edits_deterministically() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    insert_tag(&app.pool, alice, "dark", "MOOD").await;
    insert_tag(&app.pool, alice, "darker", "MOOD").await;
    let peak = insert_tag(&app.pool, alice, "peak", "MOOD").await;

    let made = clusters::rebuild(&app.pool, "MOOD").await.unwrap();
    assert_eq!(made, 1, "dark + darker form one cluster");

    let initial: Vec<String> = sqlx::query_scalar(
        "SELECT t.name FROM hub_tag_clusters c
           JOIN hub_tag_cluster_members m ON m.cluster_id = c.id
           JOIN hub_tags t ON t.id = m.tag_id
          WHERE c.tag_type = 'MOOD' ORDER BY t.name",
    )
    .fetch_all(&app.pool)
    .await
    .unwrap();
    assert_eq!(initial, vec!["dark".to_string(), "darker".to_string()]);

    // A manual edit: drop `peak` into the auto-cluster.
    let cid: i64 = sqlx::query_scalar("SELECT id FROM hub_tag_clusters WHERE tag_type = 'MOOD'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    insert_member(&app.pool, cid, peak).await;

    // Rebuild discards manual edits: the type's clusters are deleted and
    // rebuilt from similarity alone.
    let made2 = clusters::rebuild(&app.pool, "MOOD").await.unwrap();
    assert_eq!(made2, 1);
    let after: Vec<String> = sqlx::query_scalar(
        "SELECT t.name FROM hub_tag_clusters c
           JOIN hub_tag_cluster_members m ON m.cluster_id = c.id
           JOIN hub_tags t ON t.id = m.tag_id
          WHERE c.tag_type = 'MOOD' ORDER BY t.name",
    )
    .fetch_all(&app.pool)
    .await
    .unwrap();
    assert_eq!(
        after,
        vec!["dark".to_string(), "darker".to_string()],
        "manual member edit (peak) is replaced"
    );

    // Deterministic: iterating again yields the identical member set.
    clusters::rebuild(&app.pool, "MOOD").await.unwrap();
    let again: Vec<String> = sqlx::query_scalar(
        "SELECT t.name FROM hub_tag_clusters c
           JOIN hub_tag_cluster_members m ON m.cluster_id = c.id
           JOIN hub_tags t ON t.id = m.tag_id
          WHERE c.tag_type = 'MOOD' ORDER BY t.name",
    )
    .fetch_all(&app.pool)
    .await
    .unwrap();
    assert_eq!(again, after, "rebuild must be deterministic");
}
