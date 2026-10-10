//! `/downloads` page + music-api state-cache tests (format column, summary,
//! track-page status from cache).
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
