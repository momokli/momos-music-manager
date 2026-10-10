//! Integration test for the playlist "prioritize" action (music-api high prio).

mod common;

#[tokio::test]
async fn prioritize_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .post(app.url(&format!("/playlist/{}/prioritize", app.seed.pl_alice)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn prioritize_without_music_api_flashes() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .post(app.url(&format!("/playlist/{}/prioritize", app.seed.pl_alice)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    // No music-api token in tests -> redirect back to the playlist with a flash.
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    let loc = resp.headers()["location"].to_str().unwrap_or("");
    assert!(loc.starts_with("/playlist/"), "unexpected location: {loc}");
}
