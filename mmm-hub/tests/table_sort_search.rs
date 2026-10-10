//! Integration tests for the shared table primitives: order-independent fuzzy
//! search and server-side header sorting.

mod common;

async fn get(app: &common::TestApp, cookie: &str, path: &str) -> String {
    let resp = app
        .client()
        .get(app.url(path))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "GET {path}");
    resp.text().await.unwrap()
}

/// The tag search matches every token in any order and anywhere in the row.
#[tokio::test]
async fn tags_search_is_fuzzy_and_order_independent() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;
    mmm_hub::tags::ensure_tag(&app.pool, alice, "Warehouse Dark")
        .await
        .unwrap();
    mmm_hub::tags::ensure_tag(&app.pool, alice, "Fusion Breakbeat")
        .await
        .unwrap();

    for q in ["warehouse+dark", "dark+warehouse", "wareh+dark"] {
        let html = get(&app, &cookie, &format!("/tags?q={q}")).await;
        assert!(html.contains("Warehouse Dark"), "q={q} missed the tag");
        assert!(
            !html.contains("Fusion Breakbeat"),
            "q={q} leaked another tag"
        );
    }
}

/// Clicking a column sorts server-side; the header link toggles direction.
#[tokio::test]
async fn tags_sort_by_track_count() {
    use mmm_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    let big = tags::ensure_tag(&app.pool, alice, "Many Tracks")
        .await
        .unwrap();
    tags::ensure_tag(&app.pool, alice, "Zero Tracks")
        .await
        .unwrap();
    for tr in [app.seed.t_all, app.seed.t_two, app.seed.t_pl] {
        sqlx::query(
            "INSERT OR IGNORE INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)",
        )
        .bind(tr)
        .bind(big)
        .execute(&app.pool)
        .await
        .unwrap();
    }

    // Descending: "Many Tracks" before "Zero Tracks".
    let html = get(&app, &cookie, "/tags?sort=tracks&dir=desc").await;
    let a = html.find("Many Tracks").expect("big tag present");
    let b = html.find("Zero Tracks").expect("small tag present");
    assert!(a < b, "desc sort: many should come first");

    // Ascending flips it.
    let html = get(&app, &cookie, "/tags?sort=tracks&dir=asc").await;
    let a = html.find("Many Tracks").unwrap();
    let b = html.find("Zero Tracks").unwrap();
    assert!(b < a, "asc sort: zero should come first");

    // The header links keep the current filter (`q`) and carry sort/dir.
    let html = get(&app, &cookie, "/tags?q=many").await;
    assert!(html.contains("sort=tracks"), "sortable header missing");
}

/// The track search matches tokens in any order (partial words too).
#[tokio::test]
async fn search_page_is_order_independent() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    for q in ["shared+anthem", "anthem+shared", "anthe+shared"] {
        let html = get(&app, &cookie, &format!("/search?q={q}")).await;
        assert!(html.contains("Shared Anthem"), "q={q} missed the track");
    }
    // A non-matching token must exclude the row.
    let html = get(&app, &cookie, "/search?q=shared+zzz").await;
    assert!(
        !html.contains("Shared Anthem"),
        "noise token leaked a match"
    );
}

/// Search columns are sortable via the header links.
#[tokio::test]
async fn search_sort_headers_work() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let html = get(&app, &cookie, "/search?q=shared&sort=title&dir=desc").await;
    assert!(html.contains("sort=title"), "sortable header missing");
    assert!(html.contains("Shared Anthem"), "result missing");
}
