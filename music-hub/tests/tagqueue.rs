//! Integration tests for the tag queue (issues #219/#220) + direct track tagging.

mod common;

/// The queue detail fragment honours the per-user recommendation scope:
/// `?scope=all` shows another owner's cluster sibling, `?scope=mine` hides it,
/// and the scope toggle carries `aria-current` + `scope=` links.
#[tokio::test]
async fn queue_detail_honours_scope_for_cluster_recs() {
    use music_hub::tagtypes::{TagType, set as set_type};
    use music_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let bob = app.seed.bob;

    let dark = tags::ensure_tag(&app.pool, alice, "Dark").await.unwrap();
    let duester = tags::ensure_tag(&app.pool, bob, "Düster").await.unwrap();
    set_type(&app.pool, alice, dark, Some(TagType::Mood))
        .await
        .unwrap();
    set_type(&app.pool, bob, duester, Some(TagType::Mood))
        .await
        .unwrap();
    let cid: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tag_clusters (tag_type, label, created_at, updated_at)
         VALUES ('MOOD', 'dark', '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    for t in [dark, duester] {
        sqlx::query(
            "INSERT INTO hub_tag_cluster_members (cluster_id, tag_id, added_at)
             VALUES (?1, ?2, '2026-01-01T00:00:00+00:00')",
        )
        .bind(cid)
        .bind(t)
        .execute(&app.pool)
        .await
        .unwrap();
    }
    tags::tag_track(&app.pool, alice, app.seed.t_all, dark)
        .await
        .unwrap();

    let cookie = app.session_cookie(alice).await;
    let base = app.url(&format!("/tag-queue/{}", app.seed.t_all));

    let all_html = app
        .client()
        .get(format!("{base}?scope=all"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        all_html.contains("gleicher Cluster"),
        "cluster reason missing in queue fragment"
    );
    assert!(
        all_html.contains("Düster"),
        "cluster sibling must be visible with scope=all"
    );
    assert!(
        all_html.contains("Meine Tags") && all_html.contains("Alle"),
        "scope toggle missing in queue fragment"
    );
    assert!(
        all_html.contains("scope=mine") && all_html.contains("aria-current"),
        "scope toggle must link + mark the active scope"
    );

    let mine_html = app
        .client()
        .get(format!("{base}?scope=mine"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        !mine_html.contains("Düster"),
        "scope=mine must hide another owner's tag in the queue fragment"
    );
}

#[tokio::test]
async fn tag_queue_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url("/tag-queue"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn tag_queue_renders_for_logged_in_user() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url("/tag-queue"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(html.contains("Tag-Queue"), "heading missing");
    assert!(html.contains("Ripeness"), "ripeness column missing");
    assert!(html.contains("max Ripeness"), "threshold filter missing");
}

#[tokio::test]
async fn detail_partial_renders_track() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url(&format!("/tag-queue/{}", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(html.contains("Taggen"), "tag form missing");
}

#[tokio::test]
async fn tagging_a_track_creates_a_manual_link_and_resolves() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;
    let cookie = app.session_cookie(alice).await;

    // Pre-create the tag so we tag an existing tag.
    let tag = music_hub::tags::ensure_tag(&app.pool, alice, "Dark")
        .await
        .unwrap();

    // Direct call surfaces any rebuild error (the HTTP path ignores it).
    music_hub::tags::tag_track(&app.pool, alice, track, tag)
        .await
        .expect("tag_track");

    let resp = app
        .client()
        .post(app.url(&format!("/tag-queue/{track}/tag")))
        .header("Cookie", &cookie)
        .form(&[("name", "Dark")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(html.contains("Dark"), "detail should show the new tag");

    let manual: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_track_tag_manual WHERE track_id = ?1 AND tag_id = ?2 AND user_id = ?3",
    )
    .bind(track)
    .bind(tag)
    .bind(alice)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(manual, 1, "manual link created");

    let resolved: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_track_resolved_tags WHERE track_id = ?1 AND tag_id = ?2",
    )
    .bind(track)
    .bind(tag)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(resolved, 1, "rebuild folded the manual link in");
}

#[tokio::test]
async fn stream_without_isrc_is_bad_request() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url(&format!("/track/{}/stream", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn tag_queue_sort_is_selectable() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let html = app
        .client()
        .get(app.url("/tag-queue?sort=title"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Sortierung"), "sort control missing");
    assert!(
        html.contains("value=\"title\" selected"),
        "selected sort not reflected"
    );
}

#[tokio::test]
async fn tag_queue_tolerates_empty_max_param() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url("/tag-queue?q=&max=&sort=ripeness-desc"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn detail_shows_all_meta_and_relations() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let html = app
        .client()
        .get(app.url(&format!("/tag-queue/{}", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    for label in ["Album", "ISRC", "BPM", "Key", "Energy", "Genres", "Tags", "In Playlists"] {
        assert!(html.contains(label), "detail missing {label}");
    }
}
