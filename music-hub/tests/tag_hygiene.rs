//! Integration tests for tag hygiene (#456): similar suggestions, merge (all
//! links re-pointed, deduplicated, idempotent, owner-scoped) and the
//! rename-clash → merge suggestion flow.

mod common;

use music_hub::tags::{self, MergeSummary, RenameOutcome};

const TS: &str = "2026-01-01T00:00:00+00:00";

/// Insert an own tag with an explicit type/sync flag; returns its id.
async fn tag(pool: &sqlx::SqlitePool, owner: i64, name: &str, ty: Option<&str>, sync: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, type, sync_playlist, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING id",
    )
    .bind(owner)
    .bind(name.to_lowercase())
    .bind(name)
    .bind(ty)
    .bind(sync)
    .bind(TS)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn exec(pool: &sqlx::SqlitePool, sql: &str, binds: &[i64]) {
    let mut q = sqlx::query(sql);
    for b in binds {
        q = q.bind(*b);
    }
    q.execute(pool).await.unwrap();
}

async fn count(pool: &sqlx::SqlitePool, sql: &str, binds: &[i64]) -> i64 {
    let mut q = sqlx::query_scalar::<_, i64>(sql);
    for b in binds {
        q = q.bind(*b);
    }
    q.fetch_one(pool).await.unwrap()
}

/// A seeded world with two alice tags (`src`/`tgt`) plus helper tags, a foreign
/// bob tag, and rows in every tag-referencing table for `src`.
struct World {
    app: common::TestApp,
    src: i64,
    tgt: i64,
    parent: i64,
    child: i64,
    bob_dark: i64,
    t_alice: i64,
    t_two: i64,
    t_carol: i64,
    pl_alice: i64,
    lens: i64,
}

async fn seed_world() -> World {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let bob = app.seed.bob;

    let src = tag(&app.pool, alice, "Dark", Some("VIBE"), 0).await;
    let tgt = tag(&app.pool, alice, "Darker", None, 1).await;
    let parent = tag(&app.pool, alice, "Techno", Some("GENRE"), 0).await;
    let child = tag(&app.pool, alice, "Set", Some("SETLIST"), 0).await;
    let lens = tag(&app.pool, alice, "Lens", None, 0).await;
    let bob_dark = tag(&app.pool, bob, "Dark", Some("MOOD"), 0).await;

    let t_alice = app.seed.t_alice;
    let t_two = app.seed.t_two;
    let t_carol = app.seed.t_carol;
    let pl_alice = app.seed.pl_alice;

    // hub_tag_sources: both tags point at the same playlist (PK collision path).
    for t in [src, tgt] {
        exec(
            &app.pool,
            "INSERT INTO hub_tag_sources (tag_id, playlist_id, user_id, service, created_at, keep_on_remove)
             VALUES (?1, ?2, ?3, 'spotify', ?4, 0)",
            &[t, pl_alice, alice],
        )
        .await;
        let _ = TS;
    }

    // Pre-existing (materialized) resolved rows with a duplicate (t_alice, src)/(t_alice, tgt).
    for (tr, tg) in [(t_alice, src), (t_alice, tgt), (t_two, src)] {
        exec(
            &app.pool,
            "INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)",
            &[tr, tg],
        )
        .await;
    }

    // Manual links: (t_alice, src) and (t_alice, tgt) collide on the PK.
    for tg in [src, tgt] {
        exec(
            &app.pool,
            "INSERT INTO hub_track_tag_manual (track_id, tag_id, user_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
            &[t_alice, tg, alice],
        )
        .await;
        let _ = TS;
    }
    exec(
        &app.pool,
        "INSERT INTO hub_track_tag_manual (track_id, tag_id, user_id, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        &[t_two, src, alice],
    )
    .await;

    // Archived kept link.
    exec(
        &app.pool,
        "INSERT INTO hub_track_tag_archive (tag_id, track_id, source_playlist_id, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        &[src, t_carol, pl_alice],
    )
    .await;

    // Membership history (no PK; 1:1 re-point expected).
    for tr in [t_alice, t_two] {
        sqlx::query(
            "INSERT INTO hub_tag_membership_history (track_id, tag_id, added_at)
             VALUES (?1, ?2, ?3)",
        )
        .bind(tr)
        .bind(src)
        .bind(TS)
        .execute(&app.pool)
        .await
        .unwrap();
    }

    // Parents: solves both dims + the self-loop (src, tgt).
    exec(
        &app.pool,
        "INSERT INTO hub_tag_parents (tag_id, parent_tag_id, created_at) VALUES (?1, ?2, ?3)",
        &[src, parent],
    )
    .await;
    exec(
        &app.pool,
        "INSERT INTO hub_tag_parents (tag_id, parent_tag_id, created_at) VALUES (?1, ?2, ?3)",
        &[child, src],
    )
    .await;
    exec(
        &app.pool,
        "INSERT INTO hub_tag_parents (tag_id, parent_tag_id, created_at) VALUES (?1, ?2, ?3)",
        &[src, tgt],
    )
    .await;

    // Clusters: (cluster1, src) collides with an existing (cluster1, tgt).
    let cluster1: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tag_clusters (tag_type, label, created_at, updated_at)
         VALUES ('VIBE', 'c1', ?1, ?1) RETURNING id",
    )
    .bind(TS)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    let cluster2: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tag_clusters (tag_type, label, created_at, updated_at)
         VALUES ('VIBE', 'c2', ?1, ?1) RETURNING id",
    )
    .bind(TS)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    for (c, t) in [(cluster1, src), (cluster1, tgt), (cluster2, src)] {
        sqlx::query(
            "INSERT INTO hub_tag_cluster_members (cluster_id, tag_id, added_at)
             VALUES (?1, ?2, ?3)",
        )
        .bind(c)
        .bind(t)
        .bind(TS)
        .execute(&app.pool)
        .await
        .unwrap();
    }

    // Backpack with two tag items.
    let bp: i64 = sqlx::query_scalar(
        "INSERT INTO hub_backpacks (user_id, name, is_active, created_at, updated_at)
         VALUES (?1, 'bp', 0, ?2, ?2) RETURNING id",
    )
    .bind(alice)
    .bind(TS)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    for _ in 0..2 {
        sqlx::query(
            "INSERT INTO hub_backpack_items (backpack_id, source_kind, source_tag_id, position, added_at)
             VALUES (?1, 'tag', ?2, 0, ?3)",
        )
        .bind(bp)
        .bind(src)
        .bind(TS)
        .execute(&app.pool)
        .await
        .unwrap();
    }

    // Smart tag filter referencing src + tgt + parent (dedupe/remap expected).
    sqlx::query(
        "INSERT INTO hub_smart_tags (tag_id, filter_json, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?3)",
    )
    .bind(lens)
    .bind(format!(
        "{{\"version\":1,\"tag_ids\":[{src},{tgt},{parent}]}}"
    ))
    .bind(TS)
    .execute(&app.pool)
    .await
    .unwrap();

    // Curated playlist filled from src.
    sqlx::query(
        "INSERT INTO hub_curated_playlists (user_id, name, description, is_public, fill_mode, fill_tag_id, fill_limit, daily, created_at, updated_at)
         VALUES (?1, 'Cur', '', 0, 'tag', ?2, 50, 0, ?3, ?3)",
    )
    .bind(alice)
    .bind(src)
    .bind(TS)
    .execute(&app.pool)
    .await
    .unwrap();

    World {
        app,
        src,
        tgt,
        parent,
        child,
        bob_dark,
        t_alice,
        t_two,
        t_carol,
        pl_alice,
        lens,
    }
}

// ── US2: merge re-points every link, deduplicates, owner-scoped, idempotent ──

#[tokio::test]
async fn merge_repoints_all_links_and_dedupes() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;

    let sum = tags::merge_tags(&w.app.pool, alice, w.src, w.tgt)
        .await
        .unwrap();
    assert!(!sum.noop, "a real merge is not a no-op");
    // Colliding rows (src+tgt already shared the same playlist/track/user) are
    // ignored by the OR IGNORE path, so only non-colliding rows move.
    assert!(
        sum.resolved >= 1,
        "at least one resolved row moved: {sum:?}"
    );
    assert!(sum.history >= 2 && sum.parents >= 2 && sum.smart_tags == 1);

    // Source row is gone.
    assert_eq!(
        count(
            &w.app.pool,
            "SELECT COUNT(*) FROM hub_tags WHERE id = ?1",
            &[w.src]
        )
        .await,
        0
    );

    // No orphaned references to the source anywhere.
    for sql in [
        "SELECT COUNT(*) FROM hub_tag_sources WHERE tag_id = ?1",
        "SELECT COUNT(*) FROM hub_track_resolved_tags WHERE tag_id = ?1",
        "SELECT COUNT(*) FROM hub_track_tag_manual WHERE tag_id = ?1",
        "SELECT COUNT(*) FROM hub_tag_membership_history WHERE tag_id = ?1",
        "SELECT COUNT(*) FROM hub_track_tag_archive WHERE tag_id = ?1",
        "SELECT COUNT(*) FROM hub_tag_cluster_members WHERE tag_id = ?1",
        "SELECT COUNT(*) FROM hub_backpack_items WHERE source_tag_id = ?1",
        "SELECT COUNT(*) FROM hub_tag_parents WHERE tag_id = ?1 OR parent_tag_id = ?1",
        "SELECT COUNT(*) FROM hub_curated_playlists WHERE fill_tag_id = ?1",
    ] {
        assert_eq!(count(&w.app.pool, sql, &[w.src]).await, 0, "orphan: {sql}");
    }

    // hub_track_resolved_tags: no duplicate (t_alice,tgt); tgt carries exactly
    // the union of its sources {t_all,t_pl} + manual {t_alice,t_two} + archive {t_carol}.
    assert_eq!(
        count(
            &w.app.pool,
            "SELECT COUNT(*) FROM hub_track_resolved_tags WHERE tag_id = ?1 AND track_id = ?2",
            &[w.tgt, w.t_alice]
        )
        .await,
        1,
        "track carrying both tags must not be duplicated"
    );
    assert_eq!(
        count(
            &w.app.pool,
            "SELECT COUNT(*) - COUNT(DISTINCT track_id) FROM hub_track_resolved_tags WHERE tag_id = ?1",
            &[w.tgt]
        )
        .await,
        0,
        "no duplicate (track,tag) rows"
    );
    for t in [w.t_alice, w.t_two, w.t_carol] {
        assert_eq!(
            count(
                &w.app.pool,
                "SELECT COUNT(*) FROM hub_track_resolved_tags WHERE tag_id=?1 AND track_id=?2",
                &[w.tgt, t]
            )
            .await,
            1,
            "target must resolve track {t}"
        );
    }

    // Sources/backpack/parents/clusters re-pointed (no duplicates).
    assert_eq!(
        count(
            &w.app.pool,
            "SELECT COUNT(*) FROM hub_tag_sources WHERE tag_id=?1 AND playlist_id=?2",
            &[w.tgt, w.pl_alice]
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &w.app.pool,
            "SELECT COUNT(*) FROM hub_backpack_items WHERE source_tag_id = ?1",
            &[w.tgt]
        )
        .await,
        2
    );
    assert_eq!(
        count(
            &w.app.pool,
            "SELECT COUNT(*) FROM hub_tag_cluster_members WHERE tag_id = ?1",
            &[w.tgt]
        )
        .await,
        2
    );
    assert_eq!(
        count(
            &w.app.pool,
            "SELECT COUNT(*) FROM hub_tag_parents WHERE tag_id = ?1 AND parent_tag_id = ?2",
            &[w.tgt, w.parent]
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &w.app.pool,
            "SELECT COUNT(*) FROM hub_tag_parents WHERE tag_id = ?1 AND parent_tag_id = ?2",
            &[w.child, w.tgt]
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &w.app.pool,
            "SELECT COUNT(*) FROM hub_tag_parents WHERE tag_id = parent_tag_id",
            &[]
        )
        .await,
        0,
        "self-loop removed"
    );
    assert_eq!(
        count(
            &w.app.pool,
            "SELECT COUNT(*) FROM hub_tag_membership_history WHERE tag_id = ?1",
            &[w.tgt]
        )
        .await,
        2
    );

    // Smart filter remapped + deduplicated: [tgt, parent].
    let json: String =
        sqlx::query_scalar("SELECT filter_json FROM hub_smart_tags WHERE tag_id = ?1")
            .bind(w.lens)
            .fetch_one(&w.app.pool)
            .await
            .unwrap();
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let ids: Vec<i64> = v["tag_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_i64().unwrap())
        .collect();
    assert_eq!(ids, vec![w.tgt, w.parent], "src remapped to tgt, deduped");

    // Attributes (D3): target keeps its name/slug, type falls back to source,
    // sync_playlist is OR-ed.
    let (name, ty, sync): (String, Option<String>, i64) =
        sqlx::query_as("SELECT name, type, sync_playlist FROM hub_tags WHERE id = ?1")
            .bind(w.tgt)
            .fetch_one(&w.app.pool)
            .await
            .unwrap();
    assert_eq!(name, "Darker");
    assert_eq!(ty.as_deref(), Some("VIBE"), "type falls back to source");
    assert_eq!(sync, 1, "sync_playlist OR-ed");
}

#[tokio::test]
async fn merge_is_idempotent() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;

    tags::merge_tags(&w.app.pool, alice, w.src, w.tgt)
        .await
        .unwrap();
    let resolved = count(
        &w.app.pool,
        "SELECT COUNT(*) FROM hub_track_resolved_tags",
        &[],
    )
    .await;
    let manual = count(
        &w.app.pool,
        "SELECT COUNT(*) FROM hub_track_tag_manual",
        &[],
    )
    .await;

    // Second, identical call: the source is gone -> Ok no-op, counts unchanged.
    let second = tags::merge_tags(&w.app.pool, alice, w.src, w.tgt)
        .await
        .unwrap();
    assert!(
        second.noop
            && second
                == MergeSummary {
                    noop: true,
                    ..Default::default()
                }
    );
    assert_eq!(
        count(
            &w.app.pool,
            "SELECT COUNT(*) FROM hub_track_resolved_tags",
            &[]
        )
        .await,
        resolved
    );
    assert_eq!(
        count(
            &w.app.pool,
            "SELECT COUNT(*) FROM hub_track_tag_manual",
            &[]
        )
        .await,
        manual
    );

    // source == target is also a no-op.
    let same = tags::merge_tags(&w.app.pool, alice, w.tgt, w.tgt)
        .await
        .unwrap();
    assert!(same.noop);
}

#[tokio::test]
async fn merge_rejects_foreign_tags() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;

    // Foreign source (bob's tag) -> error, nothing changed.
    let err = tags::merge_tags(&w.app.pool, alice, w.bob_dark, w.tgt).await;
    assert!(err.is_err(), "cannot merge a foreign source");
    assert_eq!(
        count(
            &w.app.pool,
            "SELECT COUNT(*) FROM hub_tags WHERE id = ?1",
            &[w.bob_dark]
        )
        .await,
        1
    );

    // Foreign target -> error, source untouched.
    let err = tags::merge_tags(&w.app.pool, alice, w.src, w.bob_dark).await;
    assert!(err.is_err(), "cannot merge into a foreign target");
    assert_eq!(
        count(
            &w.app.pool,
            "SELECT COUNT(*) FROM hub_tags WHERE id = ?1",
            &[w.src]
        )
        .await,
        1
    );
    assert_eq!(
        count(
            &w.app.pool,
            "SELECT COUNT(*) FROM hub_track_tag_manual WHERE tag_id = ?1",
            &[w.src]
        )
        .await,
        2
    );
}

// ── US3/US4: rename clash -> merge suggestion ───────────────────────────────

#[tokio::test]
async fn rename_clash_suggests_merge() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let cookie = w.app.session_cookie(alice).await;

    // Engine level: renaming src to the target's name yields a Clash.
    let outcome = tags::rename_tag_outcome(&w.app.pool, alice, w.src, "Darker")
        .await
        .unwrap();
    assert_eq!(
        outcome,
        RenameOutcome::Clash {
            tag_id: w.tgt,
            name: "Darker".into()
        }
    );

    // HTTP: redirect carries the merge suggestion.
    let resp = w
        .app
        .client()
        .post(w.app.url(&format!("/tag/{}/rename", w.src)))
        .header("Cookie", &cookie)
        .form(&[("name", "Darker")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    let loc = resp.headers()["location"].to_str().unwrap().to_string();
    assert!(
        loc.contains(&format!("suggest_merge={}", w.tgt)),
        "clash must propose a merge: {loc}"
    );

    // The detail page renders the suggestion banner + a merge form to the target.
    let html = w
        .app
        .client()
        .get(w.app.url(&loc))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("existiert bereits"), "banner text present");
    assert!(
        html.contains(&format!("name=\"into\" value=\"{}\"", w.tgt)),
        "merge form targets the existing tag"
    );
}

#[tokio::test]
async fn rename_keeps_working_without_clash() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;
    let out = tags::rename_tag_outcome(&w.app.pool, alice, w.src, "Dunkel")
        .await
        .unwrap();
    assert_eq!(out, RenameOutcome::Renamed);
    let name: String = sqlx::query_scalar("SELECT name FROM hub_tags WHERE id = ?1")
        .bind(w.src)
        .fetch_one(&w.app.pool)
        .await
        .unwrap();
    assert_eq!(name, "Dunkel");
}

// ── US4/US6: similar route + engine ─────────────────────────────────────────

#[tokio::test]
async fn similar_route_requires_login_and_scopes_to_owner() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;

    // No session -> redirect to /login.
    let anon = w
        .app
        .client()
        .get(w.app.url(&format!("/tags/{}/similar", w.src)))
        .send()
        .await
        .unwrap();
    assert_eq!(anon.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(anon.headers()["location"].to_str().unwrap(), "/login");

    // Logged in: own similar tags only (bob's identically-named "Dark" excluded).
    let cookie = w.app.session_cookie(alice).await;
    let v: serde_json::Value = w
        .app
        .client()
        .get(w.app.url(&format!("/tags/{}/similar", w.src)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<String> = v["tags"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"Darker".to_string()), "got {names:?}");
    assert_eq!(
        names.iter().filter(|n| n.as_str() == "Dark").count(),
        0,
        "no self / foreign"
    );

    // A foreign tag id yields no suggestions for alice.
    let foreign: serde_json::Value = w
        .app
        .client()
        .get(w.app.url(&format!("/tags/{}/similar", w.bob_dark)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(foreign["tags"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn similar_ranks_and_excludes_others() {
    let w = seed_world().await;
    let alice = w.app.seed.alice;

    // "Dork" is one edit from "Dark" (rank 1); "Darker" only shares the prefix.
    let dork = tag(&w.app.pool, alice, "Dork", Some("MOOD"), 0).await;
    let sims = tags::similar_tags(&w.app.pool, alice, w.src, 10).await;
    let names: Vec<&str> = sims.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["Dork", "Darker"], "got {names:?}");
    assert!(
        sims[0].score > sims[1].score,
        "closer edit distance ranks first"
    );
    assert_eq!(sims[0].id, dork);
    // Bob's "Dark" is never suggested (owner scope), nor the seed itself.
    assert!(!names.contains(&"Dark"));

    // A foreign tag as seed -> no candidates.
    assert!(
        tags::similar_tags(&w.app.pool, alice, w.bob_dark, 10)
            .await
            .is_empty()
    );
}
