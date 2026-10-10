//! `/downloads` page + music-api state-cache tests (format column, summary,
//! track-page status from cache) and the all-tracks table (filters, pagination,
//! order-all).
//!
//! The music-api service is unconfigured in tests, so the live queue is skipped
//! and only the cached `hub_music_state` overview is exercised.

mod common;

async fn seed_state(
    app: &common::TestApp,
    isrc: &str,
    state: &str,
    sf: Option<&str>,
    fmts: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO hub_music_state (isrc, state, source_format, formats, checked_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )
    .bind(isrc)
    .bind(state)
    .bind(sf)
    .bind(fmts)
    .bind("2026-01-01T00:00:00+00:00")
    .execute(&app.pool)
    .await
    .expect("insert hub_music_state");
}

/// Give a seeded track an ISRC so it shows up in the all-tracks table.
async fn set_isrc(app: &common::TestApp, track_id: i64, isrc: &str) {
    sqlx::query("UPDATE hub_tracks SET isrc = ?1 WHERE id = ?2")
        .bind(isrc)
        .bind(track_id)
        .execute(&app.pool)
        .await
        .expect("set isrc");
}

/// Seed a cached state that also carries an error/reason.
async fn seed_state_error(app: &common::TestApp, isrc: &str, state: &str, error: &str) {
    sqlx::query(
        "INSERT INTO hub_music_state (isrc, state, error, checked_at) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(isrc)
    .bind(state)
    .bind(error)
    .bind("2026-01-01T00:00:00+00:00")
    .execute(&app.pool)
    .await
    .expect("insert hub_music_state error");
}

#[tokio::test]
async fn downloads_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url("/downloads"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn downloads_refresh_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .post(app.url("/downloads/refresh"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn downloads_renders_cached_summary() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    seed_state(
        &app,
        "ISRC00000001",
        "ready",
        Some("flac"),
        Some("flac,mp3-320"),
    )
    .await;
    seed_state(
        &app,
        "ISRC00000002",
        "ready",
        Some("mp3-320"),
        Some("mp3-320"),
    )
    .await;
    seed_state(
        &app,
        "ISRC00000003",
        "ready",
        Some("mp3-128"),
        Some("mp3-128"),
    )
    .await;
    seed_state(&app, "ISRC00000004", "pending", None, None).await;
    seed_state(&app, "ISRC00000005", "downloading", None, None).await;
    seed_state(&app, "ISRC00000006", "absent", None, None).await;
    seed_state(&app, "ISRC00000007", "failed", None, None).await;

    let resp = app
        .client()
        .get(app.url("/downloads"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();

    assert!(html.contains("Downloads"), "page title missing");
    assert!(html.contains("Cache-Übersicht"), "summary missing");
    assert!(html.contains("7 ISRCs"), "total missing: {html}");
    // Format distribution over the three ready entries.
    assert!(
        html.contains("flac 1 · mp3-320 1 · mp3-128 1"),
        "format distribution missing: {html}"
    );
    // Nav marks the downloads item active.
    assert!(html.contains("aria-current=\"page\""), "active nav missing");
}

#[tokio::test]
async fn downloads_refresh_redirects_when_unconfigured() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .post(app.url("/downloads/refresh"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    let loc = resp.headers()["location"].to_str().unwrap().to_string();
    assert!(loc.starts_with("/downloads"), "unexpected redirect: {loc}");
}

#[tokio::test]
async fn cached_state_reads_format() {
    let app = common::spawn().await;
    seed_state(
        &app,
        "ISRCREADY01",
        "ready",
        Some("mp3-320"),
        Some("mp3-320,flac"),
    )
    .await;

    let c = mmm_hub::music_api::cached_state(&app.pool, "ISRCREADY01")
        .await
        .expect("cached state");
    assert_eq!(c.state, "ready");
    assert!(c.ready());
    assert_eq!(c.source_format.as_deref(), Some("mp3-320"));
    assert_eq!(c.formats, vec!["mp3-320".to_string(), "flac".to_string()]);
    assert_eq!(c.label(), "ready · mp3-320");

    assert!(
        mmm_hub::music_api::cached_state(&app.pool, "NOPE")
            .await
            .is_none()
    );
}

#[tokio::test]
async fn cache_summary_counts_states_and_formats() {
    let app = common::spawn().await;
    seed_state(&app, "ISRC00000001", "ready", Some("flac"), Some("flac")).await;
    seed_state(
        &app,
        "ISRC00000002",
        "ready",
        Some("mp3-320"),
        Some("mp3-320"),
    )
    .await;
    seed_state(
        &app,
        "ISRC00000003",
        "ready",
        Some("mp3-128"),
        Some("mp3-128"),
    )
    .await;
    seed_state(&app, "ISRC00000004", "pending", None, None).await;
    seed_state(&app, "ISRC00000005", "downloading", None, None).await;
    seed_state(&app, "ISRC00000006", "absent", None, None).await;
    seed_state(&app, "ISRC00000007", "failed", None, None).await;

    let s = mmm_hub::music_api::cache_summary(&app.pool).await;
    assert_eq!(s.total, 7);
    assert_eq!(s.ready, 3);
    assert_eq!(s.ready_flac, 1);
    assert_eq!(s.ready_320, 1);
    assert_eq!(s.ready_128, 1);
    assert_eq!(s.ready_other, 0);
    assert_eq!(s.pending, 1);
    assert_eq!(s.downloading, 1);
    assert_eq!(s.absent, 1);
    assert_eq!(s.failed, 1);
    assert_eq!(s.errors, 0);
}

#[tokio::test]
async fn cached_state_reads_error() {
    let app = common::spawn().await;
    seed_state_error(&app, "ISRCERR0001", "absent", "no data").await;

    let c = mmm_hub::music_api::cached_state(&app.pool, "ISRCERR0001")
        .await
        .expect("cached state");
    assert_eq!(c.state, "absent");
    assert!(!c.ready());
    assert_eq!(c.error.as_deref(), Some("no data"));
    assert_eq!(c.label(), "absent · no data");
}

#[tokio::test]
async fn cache_summary_counts_errors() {
    let app = common::spawn().await;
    seed_state_error(&app, "ISRCERR0001", "absent", "no data").await;
    seed_state_error(&app, "ISRCERR0002", "failed", "download timeout").await;
    seed_state(&app, "ISRCERR0003", "ready", Some("flac"), Some("flac")).await;

    let s = mmm_hub::music_api::cache_summary(&app.pool).await;
    assert_eq!(s.total, 3);
    assert_eq!(s.errors, 2);
}

#[tokio::test]
async fn downloads_table_renders_error_reason() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    set_isrc(&app, app.seed.t_all, "USAAA0000001").await;
    seed_state_error(&app, "USAAA0000001", "failed", "download timeout").await;

    let resp = app
        .client()
        .get(app.url("/downloads"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(
        html.contains("Fehler"),
        "error column header missing: {html}"
    );
    assert!(
        html.contains("download timeout"),
        "error reason not rendered: {html}"
    );
}

#[test]
fn format_bucket_classifies() {
    use mmm_hub::music_api::format_bucket;
    assert_eq!(format_bucket(Some("flac"), ""), "flac");
    assert_eq!(format_bucket(Some("mp3-320"), ""), "320");
    assert_eq!(format_bucket(Some("mp3-128"), ""), "128");
    // Falls back to the raw formats list when source_format is missing.
    assert_eq!(format_bucket(None, "flac,mp3-320"), "flac");
    assert_eq!(format_bucket(None, "mp3-320"), "320");
    assert_eq!(format_bucket(None, ""), "other");
}

#[tokio::test]
async fn track_page_shows_cached_music_state() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    sqlx::query("UPDATE hub_tracks SET isrc = 'ISRCTRACK01' WHERE id = ?1")
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();
    seed_state(&app, "ISRCTRACK01", "ready", Some("flac"), Some("flac")).await;

    let resp = app
        .client()
        .get(app.url(&format!("/track/{}", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(
        html.contains("ready · flac"),
        "cached music-api state missing on track page: {html}"
    );
}

// ── all-tracks table (filters, pagination, order-all) ───────────────────────

#[tokio::test]
async fn downloads_table_lists_hub_tracks_with_state() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    set_isrc(&app, app.seed.t_all, "USAAA0000001").await;
    set_isrc(&app, app.seed.t_two, "USAAA0000002").await;
    seed_state(&app, "USAAA0000001", "ready", Some("flac"), Some("flac")).await;

    let resp = app
        .client()
        .get(app.url("/downloads"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();

    assert!(html.contains("Alle Hub-Tracks"), "table heading missing");
    // Both ISRC-bearing tracks are listed.
    assert!(html.contains("USAAA0000001"), "ready track missing");
    assert!(html.contains("USAAA0000002"), "unknown track missing");
    // The cached state + format are shown; the uncached one defaults to unknown.
    assert!(html.contains("ready"), "ready state missing");
    assert!(html.contains("flac"), "ready format missing");
    assert!(html.contains("unknown"), "default unknown state missing");
    // Total reflects the two ISRC-bearing tracks.
    assert!(html.contains("Alle Hub-Tracks (2)"), "row total wrong");
}

#[tokio::test]
async fn downloads_status_filter_is_server_side() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    set_isrc(&app, app.seed.t_all, "USAAA0000001").await;
    set_isrc(&app, app.seed.t_two, "USAAA0000002").await;
    seed_state(&app, "USAAA0000001", "ready", Some("flac"), Some("flac")).await;

    let resp = app
        .client()
        .get(app.url("/downloads?status=ready"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();

    assert!(html.contains("USAAA0000001"), "ready track should remain");
    assert!(
        !html.contains("USAAA0000002"),
        "non-ready track should be filtered out"
    );
    assert!(html.contains("Alle Hub-Tracks (1)"), "filtered total wrong");
}

#[tokio::test]
async fn downloads_format_filter_matches_bucket() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    set_isrc(&app, app.seed.t_all, "USAAA0000001").await;
    set_isrc(&app, app.seed.t_two, "USAAA0000002").await;
    seed_state(&app, "USAAA0000001", "ready", Some("flac"), Some("flac")).await;
    seed_state(
        &app,
        "USAAA0000002",
        "ready",
        Some("mp3-320"),
        Some("mp3-320"),
    )
    .await;

    let resp = app
        .client()
        .get(app.url("/downloads?format=flac"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    let html = resp.text().await.unwrap();
    assert!(html.contains("USAAA0000001"), "flac track should remain");
    assert!(
        !html.contains("USAAA0000002"),
        "320 track should be filtered out of flac"
    );
    assert!(html.contains("Alle Hub-Tracks (1)"), "format total wrong");
}

#[tokio::test]
async fn downloads_text_search_filters_title_artist_isrc() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    set_isrc(&app, app.seed.t_all, "USAAA0000001").await; // title "Shared Anthem"
    set_isrc(&app, app.seed.t_two, "USAAA0000002").await; // title "Two Users"

    let resp = app
        .client()
        .get(app.url("/downloads?q=anthem"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    let html = resp.text().await.unwrap();
    assert!(
        html.contains("USAAA0000001"),
        "matching title should remain"
    );
    assert!(
        !html.contains("USAAA0000002"),
        "non-matching title should be filtered out"
    );
}

#[tokio::test]
async fn downloads_pagination_splits_rows() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // 150 ISRC-bearing tracks -> 2 pages of 100/50.
    for i in 0..150 {
        sqlx::query(
            "INSERT INTO hub_tracks (service, service_track_id, isrc, title, artists, first_seen_at)
             VALUES ('spotify', ?1, ?2, ?3, ?4, '2026-01-01T00:00:00+00:00')",
        )
        .bind(format!("pg-{i:03}"))
        .bind(format!("PAG{i:010}"))
        .bind(format!("Title {i:03}"))
        .bind(format!("Artist {i:03}"))
        .execute(&app.pool)
        .await
        .unwrap();
    }

    let page1 = app
        .client()
        .get(app.url("/downloads"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    let html1 = page1.text().await.unwrap();
    assert!(html1.contains("Alle Hub-Tracks (150)"), "total wrong");
    assert!(html1.contains("Seite 1 von 2"), "page indicator wrong");
    assert!(
        html1.contains("PAG0000000000"),
        "first row missing on page 1"
    );
    assert!(
        !html1.contains("PAG0000000149"),
        "last row should not be on page 1"
    );

    let page2 = app
        .client()
        .get(app.url("/downloads?page=2"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    let html2 = page2.text().await.unwrap();
    assert!(html2.contains("Seite 2 von 2"), "page 2 indicator wrong");
    assert!(
        html2.contains("PAG0000000149"),
        "last row missing on page 2"
    );
    assert!(
        !html2.contains("PAG0000000000"),
        "first row should not be on page 2"
    );
}

#[tokio::test]
async fn order_all_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .post(app.url("/downloads/order-all"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn order_all_redirects_with_flash_when_unconfigured() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    set_isrc(&app, app.seed.t_all, "USAAA0000001").await;

    let resp = app
        .client()
        .post(app.url("/downloads/order-all"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    let loc = resp.headers()["location"].to_str().unwrap();
    assert!(
        loc.starts_with("/downloads"),
        "should redirect to /downloads"
    );
    assert!(loc.contains("msg="), "should carry a flash message");
}
