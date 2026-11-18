//! Tonie-Verlauf (#604, Slice U4): the page, its empty states and the two
//! ledger mutations (`unlisten` / `reset`).
//!
//! Everything here is DB-backed — no Tonies mock, no music-api mock, no
//! network. That is the point of the page: it shows what the log already says,
//! even when the account is not connected.
//!
//! Covered acceptance criteria:
//! * the runs of a figure render (status, chapters, minutes, +added/−removed);
//! * a foreign user sees neither another user's figure nor their ledger;
//! * `unlisten` drops exactly one ledger row, `reset` only the caller's;
//! * no binding → a hint pointing at `/tonies`; no albums → "noch nichts gehört";
//! * both POSTs and the page require a session.

mod common;

use axum::http::StatusCode;

// ── helpers ─────────────────────────────────────────────────────────────────

async fn get_page(app: &common::TestApp, cookie: &str, path: &str) -> String {
    let resp = app
        .client()
        .get(app.url(path))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "GET {path}");
    resp.text().await.unwrap()
}

async fn send_post(
    app: &common::TestApp,
    cookie: Option<&str>,
    path: &str,
    form: &[(&str, &str)],
) -> reqwest::Response {
    let mut req = app.client().post(app.url(path)).form(&form.to_vec());
    if let Some(c) = cookie {
        req = req.header("cookie", c);
    }
    req.send().await.unwrap()
}

async fn bind(app: &common::TestApp, uid: i64, tonie_id: &str, name: &str) -> i64 {
    music_hub::tonies::upsert_binding(
        &app.pool,
        uid,
        tonie_id,
        Some(name),
        "albums",
        Some(15167501),
        "mp3-128",
        "manual",
    )
    .await
    .unwrap();
    music_hub::tonies::get_binding(&app.pool, uid, tonie_id)
        .await
        .unwrap()
        .unwrap()
        .id
}

#[allow(clippy::too_many_arguments)]
async fn insert_run(
    app: &common::TestApp,
    binding_id: i64,
    started_at: &str,
    status: &str,
    chapters: Option<i64>,
    seconds: Option<i64>,
    added: i64,
    removed: i64,
    skipped_json: &str,
    error: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO hub_tonie_syncs
             (binding_id, started_at, finished_at, status, chapters_before,
              chapters_after, seconds, added, removed, skipped_json, error)
         VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8, ?9, ?10)",
    )
    .bind(binding_id)
    .bind(started_at)
    .bind(format!("{started_at}Z"))
    .bind(status)
    .bind(chapters)
    .bind(seconds)
    .bind(added)
    .bind(removed)
    .bind(skipped_json)
    .bind(error)
    .execute(&app.pool)
    .await
    .unwrap();
}

async fn mark_seen(app: &common::TestApp, uid: i64, album_id: &str, title: &str, at: &str) {
    sqlx::query(
        "INSERT INTO hub_tonie_albums_seen
             (user_id, album_id, album_title, first_used_at, last_used_at)
         VALUES (?1, ?2, ?3, ?4, ?4)",
    )
    .bind(uid)
    .bind(album_id)
    .bind(title)
    .bind(at)
    .execute(&app.pool)
    .await
    .unwrap();
}

async fn seen_ids(app: &common::TestApp, uid: i64) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT album_id FROM hub_tonie_albums_seen WHERE user_id = ?1 ORDER BY album_id",
    )
    .bind(uid)
    .fetch_all(&app.pool)
    .await
    .unwrap()
}

fn toast(headers: &axum::http::HeaderMap) -> String {
    headers
        .get("hx-trigger")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

/// A user with one figure (`ct1`), two logged runs and one listened album.
async fn seeded(app: &common::TestApp, slug: &str) -> (i64, String, i64) {
    let (uid, cookie) = app.user_with_password(slug, "pw").await;
    let binding_id = bind(app, uid, "ct1", "Story One").await;

    // Older run first, then the newest — `list_syncs` returns them newest first.
    insert_run(
        app,
        binding_id,
        "2026-11-11T08:00:00+00:00",
        "noop",
        Some(2),
        Some(3600),
        0,
        0,
        "[]",
        None,
    )
    .await;
    insert_run(
        app,
        binding_id,
        "2026-11-18T09:00:00+00:00",
        "ok",
        Some(3),
        Some(4260),
        2,
        1,
        r#"[{"title":"Folge 9","isrc":null,"reason":"cap_reached"}]"#,
        None,
    )
    .await;

    mark_seen(app, uid, "459", "Folge 459", "2026-11-18T09:01:00+00:00").await;
    (uid, cookie, binding_id)
}

// ── page ────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn page_renders_the_runs_of_a_figure() {
    let app = common::spawn().await;
    let (_uid, cookie, _binding) = seeded(&app, "hist-runs").await;

    let html = get_page(&app, &cookie, "/tonies/history").await;

    assert!(html.contains("Verlauf"), "page title");
    assert!(html.contains("Story One"), "figure name");
    assert!(html.contains("Alben · mp3-128"), "source label");
    assert!(html.contains("id=\"figure-ct1\""), "stable anchor");

    // What is on the figure: the newest run that reported a state.
    assert!(
        html.contains("3 Kapitel · 71 min · Stand 2026-11-18 09:00"),
        "current state: {html}"
    );
    // The newest run as one line, with both deltas.
    assert!(html.contains("Zuletzt:"), "last run line");
    assert!(
        html.contains("3 Kapitel &middot; 71 min &middot; +2 &middot; -1"),
        "chapters, minutes and both deltas: {html}"
    );
    // Both runs are listed.
    assert_eq!(html.matches("class=\"tonie-history-run-when\"").count(), 2);
    assert!(
        html.contains("keine Änderung"),
        "noop label of the older run"
    );
    // A skipped track names its reason.
    assert!(
        html.contains("1 übersprungen: Folge 9 (passten nicht mehr)"),
        "skipped track and reason"
    );

    assert!(!html.contains("{{ "), "unexpanded template block");
}

#[tokio::test]
async fn page_without_a_figure_points_at_the_tonies_page() {
    let app = common::spawn().await;
    let (_uid, cookie) = app.user_with_password("hist-empty", "pw").await;

    let html = get_page(&app, &cookie, "/tonies/history").await;

    assert!(html.contains("Keine Figur verbunden"), "empty hint");
    assert!(html.contains("href=\"/tonies\""), "link back to /tonies");
    assert!(html.contains("Noch nichts gehört"), "empty ledger hint");
    assert!(
        !html.contains("class=\"tonie-history-figure\""),
        "no figure card"
    );
}

#[tokio::test]
async fn another_user_sees_neither_figure_nor_ledger() {
    let app = common::spawn().await;
    seeded(&app, "hist-owner").await;
    let (_b_id, b_cookie) = app.user_with_password("hist-other", "pw").await;

    let html = get_page(&app, &b_cookie, "/tonies/history").await;

    assert!(
        !html.contains("Story One"),
        "foreign figure must not render"
    );
    assert!(
        !html.contains("figure-ct1"),
        "foreign anchor must not render"
    );
    assert!(!html.contains("Folge 459"), "foreign album must not render");
    assert!(html.contains("Keine Figur verbunden"));
    assert!(html.contains("Noch nichts gehört"));
}

#[tokio::test]
async fn page_requires_a_session() {
    let app = common::spawn().await;

    let resp = app
        .client()
        .get(app.url("/tonies/history"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers().get("location").unwrap(),
        "/login",
        "anonymous GET goes to /login"
    );

    for path in ["/tonies/history/unlisten", "/tonies/history/reset"] {
        let resp = send_post(&app, None, path, &[("album_id", "459")]).await;
        assert_eq!(
            resp.status(),
            StatusCode::UNAUTHORIZED,
            "anonymous POST {path}"
        );
    }
}

// ── ledger mutations ────────────────────────────────────────────────────────

#[tokio::test]
async fn unlisten_drops_exactly_one_ledger_row() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("hist-unlisten", "pw").await;
    mark_seen(&app, uid, "459", "Folge 459", "2026-11-18T09:01:00+00:00").await;
    mark_seen(&app, uid, "458", "Folge 458", "2026-11-17T09:01:00+00:00").await;

    let before = get_page(&app, &cookie, "/tonies/history").await;
    assert!(before.contains("Folge 459"));
    assert!(before.contains("Folge 458"));

    let resp = send_post(
        &app,
        Some(&cookie),
        "/tonies/history/unlisten",
        &[("album_id", "459")],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let headers = resp.headers().clone();
    let location = headers
        .get("location")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        location.starts_with("/tonies/history?msg="),
        "PRG back to the page: {location}"
    );
    assert!(
        toast(&headers).contains("\"kind\":\"ok\""),
        "toast: {}",
        toast(&headers)
    );

    assert_eq!(seen_ids(&app, uid).await, vec!["458".to_string()]);

    let after = get_page(&app, &cookie, "/tonies/history").await;
    assert!(!after.contains("Folge 459"), "unlistened album is gone");
    assert!(after.contains("Folge 458"), "the other album stays");
}

#[tokio::test]
async fn unlisten_never_touches_a_foreign_row() {
    let app = common::spawn().await;
    let (owner, _owner_cookie) = app.user_with_password("hist-owner-2", "pw").await;
    let (other, other_cookie) = app.user_with_password("hist-other-2", "pw").await;
    mark_seen(&app, owner, "459", "Folge 459", "2026-11-18T09:01:00+00:00").await;

    let resp = send_post(
        &app,
        Some(&other_cookie),
        "/tonies/history/unlisten",
        &[("album_id", "459")],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);

    assert_eq!(
        seen_ids(&app, owner).await,
        vec!["459".to_string()],
        "the owner's ledger is untouched"
    );
    assert!(seen_ids(&app, other).await.is_empty());
}

#[tokio::test]
async fn reset_clears_only_my_ledger() {
    let app = common::spawn().await;
    let (mine, my_cookie) = app.user_with_password("hist-reset-a", "pw").await;
    let (theirs, _their_cookie) = app.user_with_password("hist-reset-b", "pw").await;
    mark_seen(&app, mine, "459", "Folge 459", "2026-11-18T09:01:00+00:00").await;
    mark_seen(&app, mine, "458", "Folge 458", "2026-11-17T09:01:00+00:00").await;
    mark_seen(
        &app,
        theirs,
        "700",
        "Folge 700",
        "2026-11-16T09:01:00+00:00",
    )
    .await;

    let resp = send_post(&app, Some(&my_cookie), "/tonies/history/reset", &[]).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let headers = resp.headers().clone();
    assert!(
        headers
            .get("location")
            .unwrap()
            .to_str()
            .unwrap()
            .starts_with("/tonies/history?msg="),
        "PRG back to the page"
    );
    assert!(toast(&headers).contains("\"kind\":\"ok\""));

    assert!(seen_ids(&app, mine).await.is_empty(), "my ledger is empty");
    assert_eq!(
        seen_ids(&app, theirs).await,
        vec!["700".to_string()],
        "the other user's ledger survives"
    );

    let html = get_page(&app, &my_cookie, "/tonies/history").await;
    assert!(html.contains("Noch nichts gehört"));
}

#[tokio::test]
async fn unlisten_without_an_album_is_an_error_toast() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("hist-noid", "pw").await;
    mark_seen(&app, uid, "459", "Folge 459", "2026-11-18T09:01:00+00:00").await;

    let resp = send_post(
        &app,
        Some(&cookie),
        "/tonies/history/unlisten",
        &[("album_id", "  ")],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert!(toast(resp.headers()).contains("\"kind\":\"err\""));
    assert_eq!(
        seen_ids(&app, uid).await,
        vec!["459".to_string()],
        "nothing was dropped"
    );
}
