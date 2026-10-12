//! Integration tests for the album view (Artist → Album → Track).

mod common;

async fn seed_album(pool: &sqlx::SqlitePool) {
    sqlx::query(
        "INSERT INTO hub_tracks (id, service, service_track_id, title, artists, album) VALUES
            (70001, 'local', 'alb-t1', 'Alpha', 'Arti', 'Testalbum'),
            (70002, 'local', 'alb-t2', 'Beta',  'Arti', 'Testalbum')",
    )
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn albums_requires_login() {
    let app = common::spawn().await;
    let resp = app.client().get(app.url("/albums")).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn albums_list_and_detail_render() {
    let app = common::spawn().await;
    seed_album(&app.pool).await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let html = app
        .client()
        .get(app.url("/albums"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Testalbum"), "album not listed");
    assert!(html.contains("/album/Testalbum"), "album link missing");

    let html = app
        .client()
        .get(app.url("/album/Testalbum"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(
        html.contains("Alpha") && html.contains("Beta"),
        "tracklist missing"
    );
    assert!(html.contains("/artist/Arti"), "artist link missing");
    assert!(html.contains("/track/70001"), "track link missing");
}

#[tokio::test]
async fn unknown_album_is_404() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url("/album/DoesNotExist"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
}
