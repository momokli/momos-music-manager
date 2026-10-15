//! Integration tests for the unified tag input (#449): the `GET /tags/search`
//! fuzzy/ranked endpoint (scoped to the caller's own tags) and the tag-add
//! endpoints accepting `tag_id` (link an existing tag) or `name` (+`type`)
//! (create-and-link) — the two flows the shared `_partials/tag_input.html`
//! component drives.

mod common;

use music_hub::tagtypes::{TagType, set as set_type};

/// POST a form with a session cookie; returns the response status.
async fn post_form(
    app: &common::TestApp,
    path: &str,
    cookie: &str,
    form: &[(&str, &str)],
) -> reqwest::StatusCode {
    app.client()
        .post(app.url(path))
        .header("Cookie", cookie)
        .form(form)
        .send()
        .await
        .unwrap()
        .status()
}

/// `GET /tags/search` → the ranked `(name, typeLabel)` pairs, in response order.
async fn search(app: &common::TestApp, cookie: &str, q: &str) -> Vec<(String, String)> {
    let body = app
        .client()
        .get(app.url(&format!("/tags/search?q={q}")))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_str(&body).expect("json body");
    v["tags"]
        .as_array()
        .expect("tags array")
        .iter()
        .map(|t| {
            (
                t["name"].as_str().unwrap_or_default().to_string(),
                t["typeLabel"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Count manual track↔tag links for one user.
async fn manual_links(app: &common::TestApp, track: i64, tag: i64, user: i64) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_track_tag_manual
          WHERE track_id = ?1 AND tag_id = ?2 AND user_id = ?3",
    )
    .bind(track)
    .bind(tag)
    .bind(user)
    .fetch_one(&app.pool)
    .await
    .unwrap()
}

/// Ranking buckets: exact > prefix > word-prefix > substring > fuzzy; and the
/// search must never offer another user's tags.
#[tokio::test]
async fn tag_search_ranks_and_scopes_to_my_tags() {
    use music_hub::tags;
    let app = common::spawn().await;
    let (alice, bob) = (app.seed.alice, app.seed.bob);

    let exact = tags::ensure_tag(&app.pool, alice, "Dark").await.unwrap();
    set_type(&app.pool, alice, exact, Some(TagType::Mood))
        .await
        .unwrap();
    tags::ensure_tag(&app.pool, alice, "Dark Mood")
        .await
        .unwrap(); // prefix
    tags::ensure_tag(&app.pool, alice, "Warehouse Dark")
        .await
        .unwrap(); // word-prefix
    tags::ensure_tag(&app.pool, alice, "Bedarken")
        .await
        .unwrap(); // substring
    tags::ensure_tag(&app.pool, alice, "Dork").await.unwrap(); // fuzzy (lev ≤ 1)
    tags::ensure_tag(&app.pool, alice, "Unrelated")
        .await
        .unwrap(); // no match
    tags::ensure_tag(&app.pool, bob, "Dark Rave").await.unwrap(); // foreign

    let cookie = app.session_cookie(alice).await;
    let hits = search(&app, &cookie, "dark").await;
    let names: Vec<&str> = hits.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(
        names,
        vec!["Dark", "Dark Mood", "Warehouse Dark", "Bedarken", "Dork"],
        "unexpected ranking: {hits:?}"
    );
    // The type label rides along on the top hit.
    assert_eq!(hits[0].1, "Mood");
    // A foreign tag and a non-matching own tag never appear.
    assert!(!names.contains(&"Dark Rave"), "foreign tag leaked");
    assert!(!names.contains(&"Unrelated"), "non-match leaked");

    // Search requires a session.
    let unauth = app
        .client()
        .get(app.url("/tags/search?q=dark"))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(unauth, reqwest::StatusCode::UNAUTHORIZED);
}

/// An empty query lists the caller's own tags (the combobox searches on focus).
#[tokio::test]
async fn tag_search_empty_query_returns_mine() {
    use music_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;
    tags::ensure_tag(&app.pool, alice, "Zebra").await.unwrap();
    tags::ensure_tag(&app.pool, app.seed.bob, "Aardvark")
        .await
        .unwrap();

    let cookie = app.session_cookie(alice).await;
    let hits = search(&app, &cookie, "").await;
    let names: Vec<&str> = hits.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"Zebra"), "own tag missing: {hits:?}");
    assert!(!names.contains(&"Aardvark"), "foreign tag leaked");
}

/// Picking an existing result posts `tag_id` → the tag is linked, not re-created.
#[tokio::test]
async fn link_existing_tag_via_tag_id() {
    use music_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;
    let tag = tags::ensure_tag(&app.pool, alice, "Dark").await.unwrap();
    let cookie = app.session_cookie(alice).await;
    let tid = tag.to_string();

    let status = post_form(
        &app,
        &format!("/track/{track}/tag"),
        &cookie,
        &[("tag_id", tid.as_str())],
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::SEE_OTHER);
    assert_eq!(manual_links(&app, track, tag, alice).await, 1);
}

/// The "create new" modal posts `name` (+`type`) → create-and-link.
#[tokio::test]
async fn create_and_link_new_tag_via_name_and_type() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;
    let cookie = app.session_cookie(alice).await;

    let status = post_form(
        &app,
        &format!("/track/{track}/tag"),
        &cookie,
        &[("name", "Neuzeit"), ("type", "MOOD")],
    )
    .await;
    assert_eq!(status, reqwest::StatusCode::SEE_OTHER);

    let (id, ty): (i64, Option<String>) = sqlx::query_as(
        "SELECT id, type FROM hub_tags WHERE name = 'Neuzeit' AND owner_user_id = ?1",
    )
    .bind(alice)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(ty.as_deref(), Some("MOOD"), "type not applied");
    assert_eq!(manual_links(&app, track, id, alice).await, 1, "not linked");
}

/// A `tag_id` the caller does not own is refused (`tag_track` requires ownership).
#[tokio::test]
async fn cannot_link_another_users_tag() {
    use music_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;
    let bob_tag = tags::ensure_tag(&app.pool, app.seed.bob, "Bobs Tag")
        .await
        .unwrap();
    let cookie = app.session_cookie(alice).await;
    let tid = bob_tag.to_string();

    let _ = post_form(
        &app,
        &format!("/track/{track}/tag"),
        &cookie,
        &[("tag_id", tid.as_str())],
    )
    .await;
    assert_eq!(manual_links(&app, track, bob_tag, alice).await, 0);
}

/// The shared component is rendered on the surfaces (`data-taginput` hook).
#[tokio::test]
async fn tag_input_component_is_rendered() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let track = app.seed.t_all;
    let cookie = app.session_cookie(alice).await;

    let frag = app
        .client()
        .get(app.url(&format!("/track/{track}/actions/tag")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(frag.contains("data-taginput"), "fragment missing combobox");
    assert!(
        frag.contains(&format!("data-action=\"/track/{track}/actions/tag\"")),
        "fragment missing action URL"
    );

    let page = app
        .client()
        .get(app.url(&format!("/track/{track}")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        page.contains("data-taginput"),
        "track page missing combobox"
    );
}
