//! Album picker UI (`plans/proposed/tonie-album-queue.md`, M4): the page and its
//! POST handlers, against a mocked music-api and a mocked Tonies/KC pair — no
//! network.
//!
//! Everything is user-scoped: a binding of another user must never be reachable.

mod common;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Form, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use music_hub::config::Config;
use music_hub::tonies;
use music_hub::tonies::AlbumItem;
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

// ── mocks ───────────────────────────────────────────────────────────────────

#[derive(Clone, Default)]
struct AuthState {
    refresh_count: Arc<Mutex<usize>>,
}

async fn token_handler(
    State(st): State<AuthState>,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    if form.get("grant_type").map(String::as_str) == Some("refresh_token") {
        *st.refresh_count.lock().unwrap() += 1;
        return Json(json!({"access_token": "acc-refreshed", "expires_in": 300})).into_response();
    }
    Json(json!({
        "access_token": "acc-login",
        "refresh_token": REFRESH,
        "expires_in": 300
    }))
    .into_response()
}

/// Keycloak + the read endpoints `/tonies` needs (one Kreativ-Tonie `ct1`).
async fn spawn_tonies_mock() -> String {
    let app = Router::new()
        .route(
            "/realms/tonies/protocol/openid-connect/token",
            post(token_handler),
        )
        .route(
            "/v2/me",
            get(|| async { Json(json!({"email": "me@example.com", "householdId": "hh1"})) }),
        )
        .route(
            "/v2/households",
            get(|| async { Json(json!({"households": [{"id": "hh1", "name": "Home"}]})) }),
        )
        .route(
            "/v2/households/{hid}/creativetonies",
            get(|| async {
                Json(json!([{
                    "id": "ct1",
                    "name": "Story One",
                    "imageUrl": "https://example.invalid/cover.png",
                    "secondsPresent": 3780,
                    "secondsRemaining": 1620,
                    "chapters": []
                }]))
            }),
        )
        .with_state(AuthState::default());
    serve(app).await
}

fn album_summary(id: &str, title: &str, released: &str, cover: Option<&str>) -> Value {
    json!({
        "albumId": id,
        "title": title,
        "releaseDate": released,
        "recordType": "album",
        "coverUrl": cover
    })
}

/// Like `album_summary`, but with a known length — the primary source #621
/// added to the artist list.
fn album_summary_secs(
    id: &str,
    title: &str,
    released: &str,
    cover: Option<&str>,
    seconds: i64,
) -> Value {
    let mut v = album_summary(id, title, released, cover);
    v["seconds"] = json!(seconds);
    v
}

/// How the mocked artist list carries album lengths (#621 primary source).
#[derive(Clone, Copy)]
enum ListSeconds {
    /// Every album carries its length → no detail call needed at all.
    All,
    /// No album carries a length → a cold list, full detail fallback.
    None,
    /// Only the first album carries a length → the per-tile skip must fire for
    /// that tile while the other still falls back to a detail call.
    FirstOnly,
}

/// Mock `music-api`: artist albums, artist search and album detail.
async fn spawn_music_api() -> String {
    spawn_music_api_counted(ListSeconds::All).await.0
}

/// Mock `music-api` that counts `/deezer/album/{id}` (detail) calls. `secs`
/// controls whether the artist list carries each album's length (the #621
/// primary path, so the grid needs no detail call); a cold list makes the hub
/// fall back to the album detail.
async fn spawn_music_api_counted(
    secs: ListSeconds,
) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let hits: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    let counter = hits.clone();

    async fn bump(
        State(hits): State<Arc<AtomicUsize>>,
        req: axum::extract::Request,
        next: axum::middleware::Next,
    ) -> Response {
        if req.uri().path().starts_with("/deezer/album/") {
            hits.fetch_add(1, Ordering::SeqCst);
        }
        next.run(req).await
    }

    let albums = move |Path(id): Path<String>| async move {
        let body = if id == "15167501" {
            let a459 = match secs {
                ListSeconds::All | ListSeconds::FirstOnly => album_summary_secs(
                    "459",
                    "Folge 459",
                    "2026-11-13",
                    Some("https://cdn.example/459.jpg"),
                    200,
                ),
                ListSeconds::None => album_summary(
                    "459",
                    "Folge 459",
                    "2026-11-13",
                    Some("https://cdn.example/459.jpg"),
                ),
            };
            // `458` has no cover: the grid has to render a placeholder tile.
            let a458 = match secs {
                ListSeconds::All => album_summary_secs("458", "Folge 458", "2026-11-06", None, 200),
                ListSeconds::None | ListSeconds::FirstOnly => {
                    album_summary("458", "Folge 458", "2026-11-06", None)
                }
            };
            json!({
                "albums": [a459, a458],
                "total": 2,
                "next": null
            })
        } else {
            json!({"albums": [], "total": 0, "next": null})
        };
        Json(body)
    };
    let search = |axum::extract::Query(q): axum::extract::Query<HashMap<String, String>>| async move {
        let needle = q.get("q").cloned().unwrap_or_default();
        if needle == "nothing" {
            return Json(json!({"artists": []}));
        }
        Json(json!({
            "artists": [
                {"artistId": "15167501", "name": "PAW Patrol", "coverUrl": null},
                {"artistId": "999", "name": "PAW Patrol Coverband", "coverUrl": null}
            ]
        }))
    };
    let album = |Path(id): Path<String>| async move {
        if id.starts_with("gone") {
            return (
                axum::http::StatusCode::NOT_FOUND,
                Json(json!({"error": "album not found"})),
            );
        }
        (
            axum::http::StatusCode::OK,
            Json(json!({
                "albumId": id,
                "title": format!("Folge {id}"),
                "releaseDate": "2026-11-13",
                "recordType": "album",
                "coverUrl": "https://cdn.example/459.jpg",
                "seconds": 200,
                "tracks": [{"isrc": "A1", "title": "Kapitel 1", "durationMs": 200_000, "position": 1}]
            })),
        )
    };

    let app = Router::new()
        .route("/deezer/artist/{id}/albums", get(albums))
        .route("/deezer/search/artist", get(search))
        .route("/deezer/album/{id}", get(album))
        .with_state(())
        .layer(axum::middleware::from_fn_with_state(
            counter,
            bump,
        ));
    (serve(app).await, hits)
}

// ── harness ─────────────────────────────────────────────────────────────────

/// Boot the web app against a mocked Tonies/KC pair and a mocked music-api.
/// `token` = `None` simulates a missing `MUSIC_API_TOKEN`.
async fn boot(token: Option<&str>) -> common::TestApp {
    boot_with_music(token, spawn_music_api().await).await
}

/// Boot against a pre-built (possibly counted) music-api mock base URL.
async fn boot_with_music(token: Option<&str>, music_base: String) -> common::TestApp {
    let tonies_base = spawn_tonies_mock().await;

    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let mut cfg = Config::for_test(url.clone());
    cfg.database_url = url;
    cfg.tonies_auth_base = tonies_base.clone();
    cfg.tonies_api_base = tonies_base;
    cfg.tonies_secret_key = Some(KEY.to_string());
    cfg.music_api_base = music_base;
    cfg.music_api_token = token.map(str::to_string);
    common::spawn_with(dir, cfg).await
}

/// A logged-in user with a connected Tonies account.
async fn user(app: &common::TestApp, slug: &str) -> (i64, String) {
    let (uid, cookie) = app.user_with_password(slug, "pw").await;
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
    (uid, cookie)
}

/// Bind `ct1` to the album source of Deezer artist `artist_id`.
async fn bind_albums(app: &common::TestApp, uid: i64, artist_id: Option<i64>, mode: &str) {
    tonies::upsert_binding(
        &app.pool,
        uid,
        "ct1",
        Some("Story One"),
        "albums",
        artist_id,
        None,
        "mp3-128",
        "manual",
    )
    .await
    .unwrap();
    tonies::set_binding_album_mode(&app.pool, uid, "ct1", mode)
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
    assert_eq!(resp.status(), StatusCode::OK, "GET {path}");
    resp.text().await.unwrap()
}

fn toast_kind(resp: &axum::http::HeaderMap) -> String {
    resp.get("hx-trigger")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
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

// ── tests ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn page_requires_session() {
    let app = boot(Some("tok")).await;
    let resp = app
        .client()
        .get(app.url("/tonies/ct1/albums"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn page_renders_without_music_api_token() {
    let app = boot(None).await;
    let (uid, cookie) = user(&app, "alb-notok").await;
    bind_albums(&app, uid, Some(15167501), "newest").await;

    // No token: a hint instead of a 500, and the rest of the page still renders.
    let html = page(&app, &cookie, "/tonies/ct1/albums").await;
    assert!(html.contains("MUSIC_API_TOKEN"), "hint expected: {html}");
    assert!(html.contains("Album-Modus"));
    assert!(html.contains("id=\"tonies-albums\""));
    // Guard against a half-rendered template (unexpanded block or missing form).
    assert!(!html.contains("{{ "), "unexpanded template block");
    assert!(html.contains("hx-post=\"/tonies/ct1/albums/mode\""));
    assert!(html.contains("Noch keine Alben im Korb."));
}

#[tokio::test]
async fn unchecked_page_offers_search_and_nothing_else() {
    let app = boot(Some("tok")).await;
    let (_uid, cookie) = user(&app, "alb-empty").await;

    let html = page(&app, &cookie, "/tonies/ct1/albums").await;
    assert!(html.contains("Noch kein Deezer-Künstler"), "empty state");
    assert!(
        html.contains("Erst einen Künstler verknüpfen."),
        "grid hint"
    );
    assert!(html.contains("Noch keine Alben im Korb."));
    assert!(html.contains("Noch kein Album abgespielt."));
}

#[tokio::test]
async fn artist_search_lists_hits() {
    let app = boot(Some("tok")).await;
    let (_uid, cookie) = user(&app, "alb-search").await;

    let html = page(&app, &cookie, "/tonies/ct1/albums?q=paul").await;
    assert!(html.contains("PAW Patrol"), "hit expected: {html}");
    assert!(html.contains("Als Quelle setzen"));
    assert!(html.contains("value=\"15167501\""));
}

#[tokio::test]
async fn search_without_hits_reports_it() {
    let app = boot(Some("tok")).await;
    let (uid, cookie) = user(&app, "alb-nohit").await;
    bind_albums(&app, uid, Some(15167501), "newest").await;

    let html = page(&app, &cookie, "/tonies/ct1/albums?q=nothing").await;
    assert!(html.contains("Keine Treffer für"), "no-hit hint missing");
    assert!(!html.contains("Als Quelle setzen"), "no hit, no button");
}

#[tokio::test]
async fn set_artist_binds_the_album_source() {
    let app = boot(Some("tok")).await;
    let (uid, cookie) = user(&app, "alb-set").await;

    let resp = send_post(
        &app,
        &cookie,
        "/tonies/ct1/albums/artist",
        &[("artist_id", "15167501"), ("artist_name", "PAW Patrol")],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let headers = resp.headers().clone();
    assert!(
        toast_kind(&headers).contains("\"kind\":\"ok\""),
        "toast: {:?}",
        toast_kind(&headers)
    );
    let body = resp.text().await.unwrap();
    assert!(body.contains("Folge 459"), "grid after binding: {body}");

    let b = tonies::get_binding(&app.pool, uid, "ct1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.source_kind, "albums");
    assert_eq!(b.source_id, Some(15167501));
    assert_eq!(b.format, "mp3-128", "defaults to mp3-128");
    assert_eq!(b.mode, "manual", "defaults to manual");
}

/// #620: setting the artist persists the name on the binding and the page shows
/// it instead of the bare `Deezer-Artist #<id>`.
#[tokio::test]
async fn set_artist_persists_the_artist_name() {
    let app = boot(Some("tok")).await;
    let (uid, cookie) = user(&app, "alb-artname").await;

    let resp = send_post(
        &app,
        &cookie,
        "/tonies/ct1/albums/artist",
        &[("artist_id", "15167501"), ("artist_name", "PAW Patrol")],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(toast_kind(resp.headers()).contains("\"kind\":\"ok\""));

    let b = tonies::get_binding(&app.pool, uid, "ct1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.source_name.as_deref(), Some("PAW Patrol"));

    let html = page(&app, &cookie, "/tonies/ct1/albums").await;
    assert!(html.contains("PAW Patrol"), "name rendered: {html}");
    assert!(
        !html.contains("Deezer-Artist #"),
        "the id fallback must not appear once the name is known: {html}"
    );
}

/// #620: a binding without a name (e.g. a row from before the migration) still
/// renders, falling back to the id.
#[tokio::test]
async fn binding_without_name_falls_back_to_the_id() {
    let app = boot(Some("tok")).await;
    let (uid, cookie) = user(&app, "alb-fallback").await;
    // `bind_albums` seeds the binding with `source_name = None`.
    bind_albums(&app, uid, Some(15167501), "newest").await;

    let html = page(&app, &cookie, "/tonies/ct1/albums").await;
    assert!(
        html.contains("Verknüpft: Deezer-Artist"),
        "linked hint expected: {html}"
    );
    assert!(
        html.contains("#15167501"),
        "id fallback expected (name is unknown): {html}"
    );
}

#[tokio::test]
async fn set_artist_rejects_a_non_numeric_id() {
    let app = boot(Some("tok")).await;
    let (uid, cookie) = user(&app, "alb-badid").await;

    let resp = send_post(
        &app,
        &cookie,
        "/tonies/ct1/albums/artist",
        &[("artist_id", "paw-patrol")],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(toast_kind(resp.headers()).contains("\"kind\":\"err\""));
    assert!(
        tonies::get_binding(&app.pool, uid, "ct1")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn mode_button_sets_album_mode() {
    let app = boot(Some("tok")).await;
    let (uid, cookie) = user(&app, "alb-mode").await;
    bind_albums(&app, uid, Some(15167501), "newest").await;

    let resp = send_post(
        &app,
        &cookie,
        "/tonies/ct1/albums/mode",
        &[("album_mode", "oldest")],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(toast_kind(resp.headers()).contains("\"kind\":\"ok\""));
    assert_eq!(
        tonies::get_binding(&app.pool, uid, "ct1")
            .await
            .unwrap()
            .unwrap()
            .album_mode,
        "oldest"
    );

    // An unknown mode is rejected, the stored one survives.
    let resp = send_post(
        &app,
        &cookie,
        "/tonies/ct1/albums/mode",
        &[("album_mode", "random")],
    )
    .await;
    assert!(toast_kind(resp.headers()).contains("\"kind\":\"err\""));
    assert_eq!(
        tonies::get_binding(&app.pool, uid, "ct1")
            .await
            .unwrap()
            .unwrap()
            .album_mode,
        "oldest"
    );
}

#[tokio::test]
async fn add_writes_the_basket_row() {
    let app = boot(Some("tok")).await;
    let (uid, cookie) = user(&app, "alb-add").await;
    bind_albums(&app, uid, Some(15167501), "basket").await;

    let resp = send_post(
        &app,
        &cookie,
        "/tonies/ct1/albums/add",
        &[("album_id", "459"), ("album_title", "Folge 459")],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(toast_kind(resp.headers()).contains("\"kind\":\"ok\""));
    let body = resp.text().await.unwrap();
    assert!(body.contains("Folge 459"), "basket entry rendered");
    assert!(
        body.contains("data-cart"),
        "cart list needs its own hook, the page renders three lists"
    );

    let items = tonies::cart::list(&app.pool, uid, "ct1").await.unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].album_id, "459");
    assert_eq!(items[0].album_title, "Folge 459");
    assert_eq!(items[0].position, 0);

    // Adding twice is refused, not duplicated.
    let resp = send_post(
        &app,
        &cookie,
        "/tonies/ct1/albums/add",
        &[("album_id", "459"), ("album_title", "Folge 459")],
    )
    .await;
    assert!(toast_kind(resp.headers()).contains("\"kind\":\"err\""));
    assert_eq!(
        tonies::cart::list(&app.pool, uid, "ct1")
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn move_and_remove_keep_the_order() {
    let app = boot(Some("tok")).await;
    let (uid, cookie) = user(&app, "alb-order").await;
    bind_albums(&app, uid, Some(15167501), "basket").await;
    tonies::cart::replace_all(
        &app.pool,
        uid,
        "ct1",
        &[
            album_item("459", "Folge 459", 200),
            album_item("458", "Folge 458", 200),
        ],
    )
    .await
    .unwrap();

    let resp = send_post(
        &app,
        &cookie,
        "/tonies/ct1/albums/move",
        &[("album_id", "458"), ("direction", "up")],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let ids: Vec<String> = tonies::cart::list(&app.pool, uid, "ct1")
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.album_id)
        .collect();
    assert_eq!(ids, vec!["458", "459"]);

    // Already on top: nothing moves.
    let resp = send_post(
        &app,
        &cookie,
        "/tonies/ct1/albums/move",
        &[("album_id", "458"), ("direction", "up")],
    )
    .await;
    assert!(toast_kind(resp.headers()).contains("\"kind\":\"err\""));

    let resp = send_post(
        &app,
        &cookie,
        "/tonies/ct1/albums/remove",
        &[("album_id", "458")],
    )
    .await;
    assert!(toast_kind(resp.headers()).contains("\"kind\":\"ok\""));
    let ids: Vec<String> = tonies::cart::list(&app.pool, uid, "ct1")
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.album_id)
        .collect();
    assert_eq!(ids, vec!["459"]);
}

#[tokio::test]
async fn unlisten_drops_the_ledger_entry() {
    let app = boot(Some("tok")).await;
    let (uid, cookie) = user(&app, "alb-unlisten").await;
    bind_albums(&app, uid, Some(15167501), "newest").await;
    tonies::mark_seen(
        &app.pool,
        uid,
        &[tonies::AlbumPlan {
            album_id: "459".to_string(),
            title: "Folge 459".to_string(),
            ..Default::default()
        }],
    )
    .await
    .unwrap();
    assert!(!tonies::seen_ids(&app.pool, uid).await.unwrap().is_empty());

    let resp = send_post(
        &app,
        &cookie,
        "/tonies/ct1/albums/unlisten",
        &[("album_id", "459")],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(toast_kind(resp.headers()).contains("\"kind\":\"ok\""));
    assert!(
        tonies::seen_ids(&app.pool, uid).await.unwrap().is_empty(),
        "ledger entry must be gone"
    );
}

#[tokio::test]
async fn reset_clears_the_ledger_of_the_caller_only() {
    let app = boot(Some("tok")).await;
    let (a, ca) = user(&app, "alb-reset-a").await;
    let (b, _cb) = user(&app, "alb-reset-b").await;
    bind_albums(&app, a, Some(15167501), "newest").await;
    for uid in [a, b] {
        tonies::mark_seen(
            &app.pool,
            uid,
            &[tonies::AlbumPlan {
                album_id: "459".to_string(),
                title: "Folge 459".to_string(),
                ..Default::default()
            }],
        )
        .await
        .unwrap();
    }

    let resp = send_post(&app, &ca, "/tonies/ct1/albums/reset", &[]).await;
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(toast_kind(resp.headers()).contains("\"kind\":\"ok\""));
    assert!(tonies::seen_ids(&app.pool, a).await.unwrap().is_empty());
    assert!(
        !tonies::seen_ids(&app.pool, b).await.unwrap().is_empty(),
        "another user's ledger must survive"
    );
}

#[tokio::test]
async fn grid_marks_listened_albums_and_placeholders_missing_covers() {
    let app = boot(Some("tok")).await;
    let (uid, cookie) = user(&app, "alb-grid").await;
    bind_albums(&app, uid, Some(15167501), "newest").await;
    tonies::mark_seen(
        &app.pool,
        uid,
        &[tonies::AlbumPlan {
            album_id: "459".to_string(),
            title: "Folge 459".to_string(),
            ..Default::default()
        }],
    )
    .await
    .unwrap();

    let html = page(&app, &cookie, "/tonies/ct1/albums").await;
    assert!(html.contains("gehört"), "seen marker missing");
    // Album 458 has no cover in the mock -> placeholder tile, never a broken img.
    assert!(html.contains("tonies-album-cover-empty"));
}

/// M4 acceptance (`plans/proposed/tonie-album-queue.md`): a tile shows the cover
/// and the album length, and a selection made by `POST …/albums/add` survives the
/// re-render.
#[tokio::test]
async fn grid_tile_shows_cover_and_length_and_marks_the_cart() {
    let app = boot(Some("tok")).await;
    let (uid, cookie) = user(&app, "alb-tile").await;
    bind_albums(&app, uid, Some(15167501), "basket").await;

    let html = page(&app, &cookie, "/tonies/ct1/albums").await;
    let tile = grid_tile(&html, "Folge 459");
    assert!(
        tile.contains("src=\"https://cdn.example/459.jpg\""),
        "cover URL in an img src: {tile}"
    );
    // The mock's album detail says 200 s -> "3 min".
    assert!(tile.contains("3 min"), "length in the tile: {tile}");
    assert!(!tile.contains("im Korb"), "not in the cart yet: {tile}");

    // Exactly the request the tile's button sends.
    let resp = send_post(
        &app,
        &cookie,
        "/tonies/ct1/albums/add",
        &[("album_id", "459"), ("album_title", "Folge 459")],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.unwrap();

    let tile = grid_tile(&body, "Folge 459");
    assert!(tile.contains("im Korb"), "selection after the save: {tile}");
    assert!(
        !tile.contains("In den Korb"),
        "the add button is gone: {tile}"
    );
    assert!(tile.contains("3 min"), "length stays: {tile}");

    // The cart row carries the length too: the user compares it against the
    // tonie's free minutes.
    let row = cart_row(&body, "459");
    assert!(row.contains("3 min"), "length in the cart row: {row}");
}

/// #621: the artist list carries each length, so a fully catalogued artist
/// costs **no** `/deezer/album/{id}` detail call for the tiles.
#[tokio::test]
async fn catalogued_albums_need_no_detail_call() {
    use std::sync::atomic::Ordering;

    let (music_base, hits) = spawn_music_api_counted(ListSeconds::All).await;
    let app = boot_with_music(Some("tok"), music_base).await;
    let (uid, cookie) = user(&app, "alb-nocall").await;
    bind_albums(&app, uid, Some(15167501), "newest").await;

    let html = page(&app, &cookie, "/tonies/ct1/albums").await;
    assert!(
        grid_tile(&html, "Folge 459").contains("3 min"),
        "length comes from the list"
    );
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "a catalogued artist must trigger no /deezer/album call"
    );
}

/// #621 fallback: a cold list (no `seconds`) still resolves each tile's length
/// via the album detail — one call per tile that lacks a length, batched.
#[tokio::test]
async fn cold_list_falls_back_to_the_detail_batch() {
    use std::sync::atomic::Ordering;

    let (music_base, hits) = spawn_music_api_counted(ListSeconds::None).await;
    let app = boot_with_music(Some("tok"), music_base).await;
    let (uid, cookie) = user(&app, "alb-cold").await;
    bind_albums(&app, uid, Some(15167501), "newest").await;

    let html = page(&app, &cookie, "/tonies/ct1/albums").await;
    assert!(grid_tile(&html, "Folge 459").contains("3 min"));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "one detail call for each of the two length-less tiles"
    );
}

/// #621 per-tile skip: when only some tiles carry a length from the list, the
/// detail fallback fires **only** for the tiles that lack one — not for the
/// whole page. Proves the skip is per tile, not per batch/page.
#[tokio::test]
async fn mixed_list_only_dials_the_length_less_tiles() {
    use std::sync::atomic::Ordering;

    let (music_base, hits) = spawn_music_api_counted(ListSeconds::FirstOnly).await;
    let app = boot_with_music(Some("tok"), music_base).await;
    let (uid, cookie) = user(&app, "alb-mixed").await;
    bind_albums(&app, uid, Some(15167501), "newest").await;

    let html = page(&app, &cookie, "/tonies/ct1/albums").await;
    // Both tiles show a length: 459 from the list, 458 from the detail fallback.
    assert!(grid_tile(&html, "Folge 459").contains("3 min"));
    assert!(grid_tile(&html, "Folge 458").contains("3 min"));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "only the length-less tile (458) triggers a detail call"
    );
}

#[tokio::test]
async fn unresolved_basket_entry_stays_visible() {
    let app = boot(Some("tok")).await;
    let (uid, cookie) = user(&app, "alb-unresolved").await;
    bind_albums(&app, uid, Some(15167501), "basket").await;
    // Not in the artist catalogue, not in the album cache.
    tonies::cart::replace_all(
        &app.pool,
        uid,
        "ct1",
        &[album_item("gone-42", "Verschwundene Folge", 0)],
    )
    .await
    .unwrap();

    let html = page(&app, &cookie, "/tonies/ct1/albums").await;
    assert!(html.contains("Verschwundene Folge"), "entry stays visible");
    assert!(html.contains("nicht auflösbar"), "entry is flagged");
    // The seeded entry has `seconds = 0`: an unknown length renders nothing,
    // never "0 min".
    assert!(!html.contains("0 min"), "unknown length must stay silent");
}

#[tokio::test]
async fn catalogue_album_is_not_flagged_unresolved() {
    let app = boot(Some("tok")).await;
    let (uid, cookie) = user(&app, "alb-cached").await;
    bind_albums(&app, uid, Some(15167501), "basket").await;
    // Not in the artist page, but music-api resolves it (the catalogue owns the
    // cache now, the hub keeps none).
    tonies::cart::replace_all(
        &app.pool,
        uid,
        "ct1",
        &[album_item("777", "Alte Folge", 200)],
    )
    .await
    .unwrap();

    let html = page(&app, &cookie, "/tonies/ct1/albums").await;
    assert!(html.contains("Alte Folge"));
    assert!(
        !html.contains("nicht auflösbar"),
        "an album music-api resolves must not be flagged"
    );
}

/// Drag & drop posts an absolute position; the cart order is the chapter order.
#[tokio::test]
async fn move_to_reorders_the_cart() {
    let app = boot(Some("tok")).await;
    let (uid, cookie) = user(&app, "alb-move-to").await;
    bind_albums(&app, uid, Some(15167501), "basket").await;
    tonies::cart::replace_all(
        &app.pool,
        uid,
        "ct1",
        &[
            album_item("459", "Folge 459", 200),
            album_item("458", "Folge 458", 200),
            album_item("457", "Folge 457", 200),
        ],
    )
    .await
    .unwrap();

    let resp = send_post(
        &app,
        &cookie,
        "/tonies/ct1/albums/move-to",
        &[("album_id", "457"), ("position", "0")],
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    let ids: Vec<String> = tonies::cart::list(&app.pool, uid, "ct1")
        .await
        .unwrap()
        .into_iter()
        .map(|i| i.album_id)
        .collect();
    assert_eq!(ids, vec!["457", "459", "458"], "drag & drop order wins");

    // An unknown album is a no-op, never a panic or a 500.
    let resp = send_post(
        &app,
        &cookie,
        "/tonies/ct1/albums/move-to",
        &[("album_id", "nope"), ("position", "1")],
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn card_shows_album_mode_and_picker_link() {
    let app = boot(Some("tok")).await;
    let (uid, cookie) = user(&app, "alb-card").await;
    bind_albums(&app, uid, Some(15167501), "oldest").await;

    let html = page(&app, &cookie, "/tonies").await;
    assert!(html.contains("Alben wählen"), "picker link missing: {html}");
    assert!(html.contains("/tonies/ct1/albums"));
    assert!(html.contains("Aelteste zuerst"), "album mode label missing");
    // The playlist-only track order is not shown for album bindings.
    assert!(!html.contains("Playlist-Reihenfolge"));
}

#[tokio::test]
async fn posts_require_session_and_a_binding() {
    let app = boot(Some("tok")).await;
    // No session.
    let resp = send_post(&app, "", "/tonies/ct1/albums/add", &[("album_id", "1")]).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Session, but no binding for this Tonie -> 404 (never another user's).
    let (_uid, cookie) = user(&app, "alb-nobind").await;
    let resp = send_post(
        &app,
        &cookie,
        "/tonies/ctX/albums/add",
        &[("album_id", "1")],
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

/// One cart entry (title, seconds) for seeding the cart in tests.
#[allow(dead_code)]
fn album_item(album_id: &str, title: &str, seconds: i64) -> AlbumItem {
    AlbumItem {
        album_id: album_id.to_string(),
        album_title: title.to_string(),
        seconds,
        position: 0,
    }
}

/// The `<article>` grid tile of the album titled `title`.
///
/// A title alone is ambiguous — the cart list shows it too — so the slice is
/// taken from the album grid only.
fn grid_tile(html: &str, title: &str) -> String {
    let start = html
        .find("class=\"tonies-album-grid\"")
        .expect("album grid rendered");
    let end = html[start..].find("</section>").expect("grid closed") + start;
    for block in html[start..end].split("<article").skip(1) {
        let block = block.split("</article>").next().unwrap_or_default();
        if block.contains(title) {
            return block.to_string();
        }
    }
    panic!("no grid tile for {title}");
}

/// The `<li data-album>` cart row of `album_id` (the cart is the one list with
/// `data-cart`; the page renders three lists).
fn cart_row(html: &str, album_id: &str) -> String {
    let cart = html.find("data-cart").expect("cart list rendered");
    let needle = format!("data-album=\"{album_id}\"");
    let at = html[cart..]
        .find(&needle)
        .unwrap_or_else(|| panic!("no cart row for {album_id}"))
        + cart;
    let start = html[..at].rfind("<li").expect("cart row start");
    let end = at + html[at..].find("</li>").expect("cart row end");
    html[start..end].to_string()
}
