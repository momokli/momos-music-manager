//! Artist page album section (U1, #601): Deezer covers, badges and the
//! "In den Korb" action — against a mocked music-api, no network.
//!
//! Everything the cart touches is user-scoped: an album is only ever written
//! into the cart of a Tonie the caller owns.

mod common;

use std::collections::HashMap;

use axum::Router;
use axum::extract::{Path, Query};
use axum::response::Json;
use axum::routing::get;
use music_hub::config::Config;
use music_hub::tonies;
use serde_json::{Value, json};

const REFRESH: &str = "refresh-token-xyz";
const KEY: &str = "hub-tonies-test-secret";

async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://{addr}")
}

// ── mock music-api ──────────────────────────────────────────────────────────

fn album_summary(id: &str, title: &str, released: &str, cover: Option<&str>) -> Value {
    json!({
        "albumId": id,
        "title": title,
        "releaseDate": released,
        "recordType": "album",
        "coverUrl": cover
    })
}

/// `/deezer/search/artist`, `/deezer/artist/{id}/albums`, `/deezer/album/{id}`.
async fn spawn_music_api() -> String {
    let search = |Query(q): Query<HashMap<String, String>>| async move {
        if q.get("q").map(String::as_str) == Some("Nobody") {
            return Json(json!({"artists": []}));
        }
        Json(json!({
            "artists": [
                {"artistId": "999", "name": "Nova Coverband", "coverUrl": null},
                {"artistId": "dz1", "name": "Nova", "coverUrl": null}
            ]
        }))
    };

    let albums = |Path(id): Path<String>| async move {
        let body = if id == "dz1" {
            json!({
                "albums": [
                    album_summary("dz-458", "Werk Eins", "2026-11-06", Some("https://cdn.example/458.jpg")),
                    // No cover: the grid must render a placeholder tile.
                    album_summary("dz-459", "Werk Zwei", "2026-11-13", None)
                ],
                "total": 2,
                "next": null
            })
        } else {
            json!({"albums": [], "total": 0, "next": null})
        };
        Json(body)
    };

    let album = |Path(id): Path<String>| async move {
        Json(json!({
            "albumId": id,
            "title": format!("Detail {id}"),
            "releaseDate": "2026-11-06",
            "recordType": "album",
            "coverUrl": "https://cdn.example/458.jpg",
            "seconds": 1234,
            "tracks": [{"isrc": "A1", "title": "Kapitel 1", "durationMs": 123_400, "position": 1}]
        }))
    };

    let app = Router::new()
        .route("/deezer/search/artist", get(search))
        .route("/deezer/artist/{id}/albums", get(albums))
        .route("/deezer/album/{id}", get(album))
        .with_state(());
    serve(app).await
}

// ── harness ─────────────────────────────────────────────────────────────────

/// Boot the app against the mock. `token` = `None` simulates a missing
/// `MUSIC_API_TOKEN`; `music_base` overrides the API base (e.g. a dead port).
async fn boot(token: Option<&str>, music_base: Option<&str>) -> common::TestApp {
    let music_base = match music_base {
        Some(base) => base.to_string(),
        None => spawn_music_api().await,
    };
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let mut cfg = Config::for_test(url.clone());
    cfg.database_url = url;
    cfg.music_api_base = music_base;
    cfg.music_api_token = token.map(str::to_string);
    common::spawn_with(dir, cfg).await
}

/// A logged-in session for a seeded user, with a connected Tonies account.
async fn user(app: &common::TestApp, uid: i64) -> String {
    tonies::save_account(
        &app.pool,
        uid,
        Some("hh1"),
        Some("me@example.com"),
        REFRESH,
        KEY,
    )
    .await
    .unwrap();
    app.session_cookie(uid).await
}

/// One played track by "Nova" plus the artist entities — enough for the page.
async fn seed_nova(app: &common::TestApp, uid: i64) -> i64 {
    let p = &app.pool;
    sqlx::query(
        "INSERT INTO hub_tracks (id, service, service_track_id, title, artists, album)
         VALUES (70001, 'spotify', 'sp70001', 'Track One', 'Nova', 'Album X')",
    )
    .execute(p)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO hub_traktor_tracks (user_id, track_id, play_count) VALUES (?1, 70001, 7)",
    )
    .bind(uid)
    .execute(p)
    .await
    .unwrap();
    music_hub::artists::backfill_artist_entities(p)
        .await
        .unwrap();
    sqlx::query_scalar("SELECT id FROM hub_artists WHERE name = 'Nova' COLLATE NOCASE")
        .fetch_one(p)
        .await
        .unwrap()
}

async fn bind(app: &common::TestApp, uid: i64, tonie_id: &str, name: &str) {
    tonies::upsert_binding(
        &app.pool,
        uid,
        tonie_id,
        Some(name),
        tonies::bindings::SOURCE_ALBUMS,
        Some(1),
        None,
        tonies::bindings::FORMAT_MP3_128,
        tonies::bindings::MODE_MANUAL,
    )
    .await
    .unwrap();
}

async fn page(app: &common::TestApp, cookie: &str, path: &str) -> String {
    let resp = app
        .client()
        .get(app.url(path))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "GET {path}");
    resp.text().await.unwrap()
}

async fn send_post(
    app: &common::TestApp,
    cookie: &str,
    path: &str,
    form: &[(&str, &str)],
) -> reqwest::Response {
    app.client()
        .post(app.url(path))
        .header("cookie", cookie)
        .form(&form.to_vec())
        .send()
        .await
        .unwrap()
}

fn toast(resp: &reqwest::Response) -> String {
    resp.headers()
        .get("hx-trigger")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

async fn cart_rows(pool: &sqlx::SqlitePool, uid: i64) -> Vec<(String, String, i64)> {
    sqlx::query_as(
        "SELECT tonie_id, IFNULL(title, ''), IFNULL(seconds, 0)
           FROM hub_tonie_cart_items WHERE user_id = ?1 ORDER BY tonie_id, position",
    )
    .bind(uid)
    .fetch_all(pool)
    .await
    .unwrap()
}

// ── tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn album_grid_renders_covers_and_placeholders() {
    let app = boot(Some("tok"), None).await;
    let uid = app.seed.alice;
    let cookie = user(&app, uid).await;
    let id = seed_nova(&app, uid).await;

    let html = page(&app, &cookie, &format!("/artist/{id}")).await;
    assert!(html.contains("Werk Eins"), "album title missing");
    assert!(
        html.contains("https://cdn.example/458.jpg"),
        "cover missing"
    );
    assert!(html.contains("2026-11-06"), "release date missing");
    // Second album has no cover → placeholder glyph, never a broken <img>.
    assert!(html.contains("Werk Zwei"));
    assert!(html.contains("&#9834;"), "placeholder glyph missing");
    assert!(
        html.contains("artist-album-cover-empty"),
        "placeholder tile class missing"
    );
    // The tile posts to the artist's cart route; the switch links to the stats view.
    assert!(
        html.contains(&format!("action=\"/artist/{id}/cart\"")),
        "cart form action missing"
    );
    assert!(
        html.contains(&format!("href=\"/artist/{id}?view=stats\"")),
        "stats toggle missing"
    );
}

#[tokio::test]
async fn stats_view_keeps_the_statistics_and_hides_albums() {
    let app = boot(Some("tok"), None).await;
    let uid = app.seed.alice;
    let cookie = user(&app, uid).await;
    let id = seed_nova(&app, uid).await;

    let html = page(&app, &cookie, &format!("/artist/{id}?view=stats")).await;
    assert!(html.contains("Meistgespielt"), "AR2 section missing");
    assert!(html.contains("Referenz-Künstler"), "AR6 section missing");
    assert!(!html.contains("Werk Eins"), "albums must be hidden");
    // The stats view still links back to the album view.
    assert!(html.contains(&format!("href=\"/artist/{id}\"")));
}

#[tokio::test]
async fn cart_add_writes_the_row_and_redirects() {
    let app = boot(Some("tok"), None).await;
    let uid = app.seed.alice;
    let cookie = user(&app, uid).await;
    let id = seed_nova(&app, uid).await;
    bind(&app, uid, "ct1", "Story One").await;

    let resp = send_post(
        &app,
        &cookie,
        &format!("/artist/{id}/cart"),
        &[("album_id", "dz-458"), ("album_title", "Werk Eins")],
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        resp.headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok()),
        Some(format!("/artist/{id}").as_str())
    );
    assert!(toast(&resp).contains("\"kind\":\"ok\""), "{}", toast(&resp));

    // Title + duration come from `deezer_album`, not from the posted title.
    let rows = cart_rows(&app.pool, uid).await;
    assert_eq!(
        rows,
        vec![("ct1".to_string(), "Detail dz-458".to_string(), 1234)]
    );

    // PRG: a second POST must not add a duplicate.
    let resp = send_post(
        &app,
        &cookie,
        &format!("/artist/{id}/cart"),
        &[("album_id", "dz-458")],
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert!(toast(&resp).contains("schon im Korb"), "{}", toast(&resp));
    assert_eq!(cart_rows(&app.pool, uid).await.len(), 1);
}

#[tokio::test]
async fn page_without_tonies_account_offers_no_cart_button() {
    let app = boot(Some("tok"), None).await;
    let uid = app.seed.bob;
    let cookie = app.session_cookie(uid).await;
    let id = seed_nova(&app, uid).await;

    let html = page(&app, &cookie, &format!("/artist/{id}")).await;
    assert!(html.contains("Werk Eins"), "albums still render");
    assert!(
        !html.contains("In den Korb"),
        "no cart button without a Tonies account"
    );

    // The handler still answers (no 500) and writes nothing.
    let resp = send_post(
        &app,
        &cookie,
        &format!("/artist/{id}/cart"),
        &[("album_id", "dz-458")],
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert!(
        toast(&resp).contains("Erst eine Figur verbinden"),
        "{}",
        toast(&resp)
    );
    assert!(cart_rows(&app.pool, uid).await.is_empty());
}

#[tokio::test]
async fn account_without_binding_reports_it_instead_of_500() {
    let app = boot(Some("tok"), None).await;
    let uid = app.seed.alice;
    let cookie = user(&app, uid).await;
    let id = seed_nova(&app, uid).await;

    // The account exists, so the button is offered — and refuses politely.
    let html = page(&app, &cookie, &format!("/artist/{id}")).await;
    assert!(html.contains("In den Korb"));

    let resp = send_post(
        &app,
        &cookie,
        &format!("/artist/{id}/cart"),
        &[("album_id", "dz-458")],
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert!(
        toast(&resp).contains("Erst eine Figur verbinden"),
        "{}",
        toast(&resp)
    );
    assert!(cart_rows(&app.pool, uid).await.is_empty());
}

#[tokio::test]
async fn one_binding_uses_a_hidden_tonie_field() {
    let app = boot(Some("tok"), None).await;
    let uid = app.seed.alice;
    let cookie = user(&app, uid).await;
    let id = seed_nova(&app, uid).await;
    bind(&app, uid, "ct1", "Story One").await;

    let html = page(&app, &cookie, &format!("/artist/{id}")).await;
    assert!(!html.contains("<select name=\"tonie_id\""), "no select");
    assert!(
        html.contains("name=\"tonie_id\" value=\"ct1\""),
        "hidden target missing"
    );
}

#[tokio::test]
async fn several_bindings_offer_a_select_and_route_to_the_chosen_one() {
    let app = boot(Some("tok"), None).await;
    let uid = app.seed.alice;
    let cookie = user(&app, uid).await;
    let id = seed_nova(&app, uid).await;
    bind(&app, uid, "ct1", "Story One").await;
    bind(&app, uid, "ct2", "Story Two").await;

    let html = page(&app, &cookie, &format!("/artist/{id}")).await;
    assert!(html.contains("<select name=\"tonie_id\""), "select missing");
    assert!(html.contains("Story Two"), "tonie label missing");

    let resp = send_post(
        &app,
        &cookie,
        &format!("/artist/{id}/cart"),
        &[("album_id", "dz-458"), ("tonie_id", "ct2")],
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    let rows = cart_rows(&app.pool, uid).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "ct2");

    // A Tonie that is not bound for this user is refused.
    let resp = send_post(
        &app,
        &cookie,
        &format!("/artist/{id}/cart"),
        &[("album_id", "dz-459"), ("tonie_id", "someone-else")],
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert!(
        toast(&resp).contains("Unbekannte Figur"),
        "{}",
        toast(&resp)
    );
    assert_eq!(cart_rows(&app.pool, uid).await.len(), 1);
}

#[tokio::test]
async fn badges_mark_listened_and_carted_albums() {
    let app = boot(Some("tok"), None).await;
    let uid = app.seed.alice;
    let cookie = user(&app, uid).await;
    let id = seed_nova(&app, uid).await;
    bind(&app, uid, "ct1", "Story One").await;

    sqlx::query(
        "INSERT INTO hub_tonie_albums_seen (user_id, album_id, album_title, first_used_at, last_used_at)
         VALUES (?1, 'dz-459', 'Werk Zwei', '2026-11-01T00:00:00+00:00', '2026-11-01T00:00:00+00:00')",
    )
    .bind(uid)
    .execute(&app.pool)
    .await
    .unwrap();
    tonies::cart::add(&app.pool, uid, "ct1", "dz-458", "Werk Eins", None, 600)
        .await
        .unwrap();

    let html = page(&app, &cookie, &format!("/artist/{id}")).await;
    assert!(html.contains("gehört"), "seen badge missing");
    assert!(html.contains("auf Tonie"), "cart badge missing");
}

#[tokio::test]
async fn missing_token_renders_a_hint_without_error() {
    let app = boot(None, None).await;
    let uid = app.seed.alice;
    let cookie = user(&app, uid).await;
    let id = seed_nova(&app, uid).await;

    let html = page(&app, &cookie, &format!("/artist/{id}")).await;
    assert!(
        html.contains("MUSIC_API_TOKEN ist nicht gesetzt"),
        "token hint missing"
    );
    assert!(!html.contains("Werk Eins"));
}

#[tokio::test]
async fn unreachable_music_api_renders_a_hint_without_error() {
    // Nothing listens on port 1: the request fails immediately.
    let app = boot(Some("tok"), Some("http://127.0.0.1:1")).await;
    let uid = app.seed.alice;
    let cookie = user(&app, uid).await;
    let id = seed_nova(&app, uid).await;

    // The name→id search is the first call; it fails, so the page notes it.
    let html = page(&app, &cookie, &format!("/artist/{id}")).await;
    assert!(
        html.contains("Kein Deezer-Künstler") || html.contains("Alben nicht ladbar"),
        "hint missing"
    );
    assert!(html.contains("Meistgespielt") || html.contains("Alben"));
}

#[tokio::test]
async fn stored_deezer_external_id_skips_the_search() {
    let app = boot(Some("tok"), None).await;
    let uid = app.seed.alice;
    let cookie = user(&app, uid).await;
    let id = seed_nova(&app, uid).await;

    // A Deezer id on the artist wins over the name search (which would pick
    // "Nova Coverband" from the mock's first hit).
    sqlx::query(
        "INSERT INTO hub_artist_external_ids (artist_id, service, external_id)
         VALUES (?1, 'deezer', 'dz1')",
    )
    .bind(id)
    .execute(&app.pool)
    .await
    .unwrap();

    let html = page(&app, &cookie, &format!("/artist/{id}")).await;
    assert!(html.contains("Werk Eins"), "albums missing");
}
