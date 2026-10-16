//! Track-page download UX (#460): availability-driven format buttons and an
//! htmx preflight that answers with a **toast** instead of the raw 502 the
//! streaming endpoint would otherwise render in a fresh tab.

mod common;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use music_hub::config::Config;
use serde_json::{Value, json};

const ISRC: &str = "USAAA0000009";

/// Mock music-api `/vault/track/{isrc}` with a configurable download satellite.
async fn mock_vault(state: Option<&'static str>, formats: &'static [&'static str]) -> String {
    let download = state.map(|s| {
        json!({ "state": s, "sourceFormat": "flac", "formats": formats })
    });
    let body = json!({
        "isrc": ISRC,
        "resolved": { "bpm": 126.0, "key": "11m", "camelot": "8A", "source": "essentia" },
        "satellites": { "download": download }
    });
    let app = Router::new().route(
        "/vault/track/{isrc}",
        get(move |Path(_): Path<String>| {
            let body: Value = body.clone();
            async move { Json(body).into_response() }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

async fn app_with(music_api: String) -> common::TestApp {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_test(format!("sqlite:{}/hub.db", dir.path().display()));
    cfg.music_api_base = music_api;
    cfg.music_api_token = Some("tok".to_string());
    let app = common::spawn_with(dir, cfg).await;
    sqlx::query("UPDATE hub_tracks SET isrc = ?1 WHERE id = ?2")
        .bind(ISRC)
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();
    app
}

fn hx_trigger(resp: &reqwest::Response) -> String {
    resp.headers()
        .get("HX-Trigger")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn ready_track_page_offers_one_button_per_format() {
    let app = app_with(mock_vault(Some("ready"), &["flac", "mp3-320"]).await).await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let id = app.seed.t_all;

    let html = app
        .client()
        .get(app.url(&format!("/track/{id}")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    // A preflight button per ready format — coherent labels …
    assert!(
        html.contains(&format!("/track/{id}/download/check?format=flac")),
        "FLAC button missing: {html}"
    );
    assert!(
        html.contains(&format!(
            "/track/{id}/download/check?format=mp3-320"
        )),
        "MP3 320 button missing: {html}"
    );
    assert!(html.contains("FLAC laden"), "FLAC label missing");
    assert!(html.contains("MP3 320 laden"), "MP3 320 label missing");
    // … and no bare, raw-error-prone download link in the top row any more.
    assert!(
        !html.contains(&format!("<a href=\"/track/{id}/download?format=")),
        "raw download link still present: {html}"
    );
}

#[tokio::test]
async fn not_ready_track_page_shows_a_passive_hint_not_buttons() {
    let app = app_with(mock_vault(Some("downloading"), &[]).await).await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let id = app.seed.t_all;

    let html = app
        .client()
        .get(app.url(&format!("/track/{id}")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        html.contains("Noch nicht verfügbar (downloading)"),
        "passive hint missing: {html}"
    );
    assert!(
        !html.contains("/download/check?format="),
        "no download button should exist while not ready: {html}"
    );
}

#[tokio::test]
async fn check_redirects_to_the_file_when_ready() {
    let app = app_with(mock_vault(Some("ready"), &["flac"]).await).await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let id = app.seed.t_all;

    let resp = app
        .client()
        .post(app.url(&format!("/track/{id}/download/check?format=flac")))
        .header("Cookie", &cookie)
        .header("hx-request", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(
        resp.headers().get("HX-Redirect").and_then(|v| v.to_str().ok()),
        Some(format!("/track/{id}/download?format=flac").as_str())
    );
    assert!(hx_trigger(&resp).contains("\"kind\":\"ok\""));
}

#[tokio::test]
async fn check_toasts_and_does_not_navigate_when_not_ready() {
    let app = app_with(mock_vault(Some("downloading"), &[]).await).await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let id = app.seed.t_all;

    let resp = app
        .client()
        .post(app.url(&format!("/track/{id}/download/check?format=flac")))
        .header("Cookie", &cookie)
        .header("hx-request", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert!(
        resp.headers().get("HX-Redirect").is_none(),
        "must not navigate to the streaming endpoint"
    );
    let trigger = hx_trigger(&resp);
    assert!(trigger.contains("\"kind\":\"err\""), "err toast: {trigger}");
    assert!(trigger.contains("Noch nicht"), "hint text: {trigger}");
}

#[tokio::test]
async fn check_without_htmx_falls_back_to_a_plain_redirect() {
    let app = app_with(mock_vault(Some("ready"), &["flac"]).await).await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let id = app.seed.t_all;

    let resp = app
        .client()
        .post(app.url(&format!("/track/{id}/download/check?format=flac")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()["location"],
        format!("/track/{id}/download?format=flac")
    );
}

#[tokio::test]
async fn zip_check_toasts_when_nothing_is_ready() {
    let app = common::spawn().await; // music-api unconfigured -> nothing ready
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .post(app.url(&format!("/playlist/{}/download/check", app.seed.pl_alice)))
        .header("Cookie", &cookie)
        .header("hx-request", "true")
        .form(&[("format", "flac")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert!(
        resp.headers().get("HX-Redirect").is_none(),
        "no ZIP redirect when nothing is available"
    );
    assert!(hx_trigger(&resp).contains("\"kind\":\"err\""));
}

/// The hub's format vocabulary must survive the round-trip (`mp3-320` must not be
/// silently downgraded to flac by the validator).
#[tokio::test]
async fn check_keeps_the_hyphenated_format() {
    let base = mock_vault(Some("ready"), &["flac", "mp3-320"]).await;
    // A music-api mock that also answers `/orders` isn't needed here.
    let _ = &base;
    let app = app_with(base).await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let id = app.seed.t_all;

    let resp = app
        .client()
        .post(app.url(&format!("/track/{id}/download/check?format=mp3-320")))
        .header("Cookie", &cookie)
        .header("hx-request", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.headers().get("HX-Redirect").and_then(|v| v.to_str().ok()),
        Some(format!("/track/{id}/download?format=mp3-320").as_str())
    );
}
