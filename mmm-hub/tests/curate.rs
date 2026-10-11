//! Curated playlists (#316): resolution from tags/lenses and pushing to Spotify
//! against a mock API.

use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::routing::{post, put};
use axum::{Json, Router};
use mmm_hub::config::Config;
use mmm_hub::curate;
use mmm_hub::{db, ingest};
use serde_json::{Value, json};
use sqlx::SqlitePool;

const NOW: &str = "2026-01-01T00:00:00+00:00";

mod common;

#[tokio::test]
async fn track_actions_require_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url(&format!("/track/{}/actions", app.seed.t_all)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn add_to_new_playlist_creates_playlist_and_item() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("zoe", "pw").await;
    let track = app.seed.t_all;

    let resp = app
        .client()
        .post(app.url(&format!("/track/{track}/actions/playlist")))
        .header("Cookie", &cookie)
        .form(&[("new_name", "Digs")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert!(resp.headers().contains_key("HX-Trigger"));

    let pid: i64 = sqlx::query_scalar(
        "SELECT id FROM hub_curated_playlists WHERE user_id = ?1 AND name = 'Digs'",
    )
    .bind(uid)
    .fetch_one(&app.pool)
    .await
    .expect("playlist created");
    let linked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_curated_items WHERE playlist_id = ?1 AND track_id = ?2",
    )
    .bind(pid)
    .bind(track)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(linked, 1);
}

#[tokio::test]
async fn add_to_existing_playlist_links_item() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("zoe", "pw").await;
    let track = app.seed.t_all;
    let pid = curate::create_manual(&app.pool, uid, "Existing")
        .await
        .unwrap();

    let resp = app
        .client()
        .post(app.url(&format!("/track/{track}/actions/playlist")))
        .header("Cookie", &cookie)
        .form(&[("playlist_id", &pid.to_string())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let linked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_curated_items WHERE playlist_id = ?1 AND track_id = ?2",
    )
    .bind(pid)
    .bind(track)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(linked, 1);
}

#[tokio::test]
async fn add_tag_creates_and_links_tag() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("zoe", "pw").await;
    let track = app.seed.t_all;

    let resp = app
        .client()
        .post(app.url(&format!("/track/{track}/actions/tag")))
        .header("Cookie", &cookie)
        .form(&[("name", "peak"), ("type", "PHASE_5")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let tag_id: i64 =
        sqlx::query_scalar("SELECT id FROM hub_tags WHERE owner_user_id = ?1 AND name = 'peak'")
            .bind(uid)
            .fetch_one(&app.pool)
            .await
            .expect("tag created");
    let linked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_track_tag_manual WHERE track_id = ?1 AND tag_id = ?2",
    )
    .bind(track)
    .bind(tag_id)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(linked, 1);
}

#[tokio::test]
async fn picker_lists_playlists() {
    let app = common::spawn().await;
    let (uid, cookie) = app.user_with_password("zoe", "pw").await;
    curate::create_manual(&app.pool, uid, "Meine Liste")
        .await
        .unwrap();

    let resp = app
        .client()
        .get(app.url(&format!("/track/{}/actions/playlist", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(html.contains("Meine Liste"), "picker should list playlist");
}

#[tokio::test]
async fn curate_requires_login() {
    let app = common::spawn().await;
    let resp = app.client().get(app.url("/curate")).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn create_then_render_detail() {
    let app = common::spawn().await;
    let (_, cookie) = app.user_with_password("zoe", "pw").await;

    let resp = app
        .client()
        .post(app.url("/curate"))
        .header("Cookie", &cookie)
        .form(&[
            ("name", "My Mix"),
            ("fill_mode", "lens"),
            ("fill_lens", "newest"),
            ("fill_limit", "10"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    let location = resp.headers()["location"].to_str().unwrap().to_string();
    assert!(location.starts_with("/curate/"), "got {location}");

    let detail = app
        .client()
        .get(app.url(&location))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(detail.status(), reqwest::StatusCode::OK);
    let html = detail.text().await.unwrap();
    assert!(html.contains("My Mix"), "detail should show name: {html}");
    assert!(html.contains("Nach Spotify pushen"), "push button missing");
}

#[derive(Clone, Default)]
struct Captured {
    created: Arc<Mutex<Vec<Value>>>,
    replaced: Arc<Mutex<Vec<Value>>>,
    added: Arc<Mutex<Vec<Value>>>,
}

async fn mock_spotify() -> (String, Captured) {
    let cap = Captured::default();

    async fn me() -> Json<Value> {
        Json(json!({ "id": "dave", "display_name": "Dave" }))
    }
    async fn create_playlist(
        State(cap): State<Captured>,
        Path(_uid): Path<String>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        cap.created.lock().unwrap().push(body);
        Json(json!({ "id": "newpl" }))
    }
    async fn replace_items(
        State(cap): State<Captured>,
        Path(_id): Path<String>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        cap.replaced.lock().unwrap().push(body);
        Json(json!({}))
    }
    async fn add_items(
        State(cap): State<Captured>,
        Path(_id): Path<String>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        cap.added.lock().unwrap().push(body);
        Json(json!({ "snapshot_id": "s" }))
    }

    let app = Router::new()
        .route("/me", axum::routing::get(me))
        .route("/users/{id}/playlists", post(create_playlist))
        .route("/playlists/{id}/items", put(replace_items).post(add_items))
        .with_state(cap.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), cap)
}

struct Fixture {
    cfg: Arc<Config>,
    pool: SqlitePool,
    cap: Captured,
    user_id: i64,
    _dir: tempfile::TempDir,
}

async fn setup() -> Fixture {
    let (base, cap) = mock_spotify().await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let pool = db::connect(&url).await.unwrap();
    let mut cfg = Config::for_test(url);
    cfg.spotify_api_base = base;

    let user_id = ingest::ensure_user(&pool, "dave").await.unwrap();
    sqlx::query(
        "INSERT INTO hub_service_accounts
             (user_id, service, access_token, refresh_token, token_expiry, scopes, authorized_at, connected_at)
         VALUES (?1, 'spotify', 'tok', 'ref', '2099-01-01T00:00:00+00:00',
                 'playlist-modify-private', ?2, ?2)",
    )
    .bind(user_id)
    .bind(NOW)
    .execute(&pool)
    .await
    .unwrap();

    Fixture {
        cfg: Arc::new(cfg),
        pool,
        cap,
        user_id,
        _dir: dir,
    }
}

/// Insert a hub track with a Spotify external id; returns the hub track id.
async fn track(pool: &SqlitePool, sid: &str, title: &str, seen: &str) -> i64 {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, title, artists, first_seen_at)
         VALUES ('spotify', ?1, ?2, 'Artist', ?3) RETURNING id",
    )
    .bind(sid)
    .bind(title)
    .bind(seen)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO hub_track_external_ids (track_id, service, external_id)
         VALUES (?1, 'spotify', ?2)",
    )
    .bind(id)
    .bind(sid)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn create_curated(
    pool: &SqlitePool,
    user_id: i64,
    mode: &str,
    tag_id: Option<i64>,
    lens: Option<&str>,
    limit: i64,
) -> CuratedSpec {
    let cp = curate::get(
        pool,
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO hub_curated_playlists
                (user_id, name, description, is_public, fill_mode, fill_tag_id, fill_lens,
                 fill_limit, daily, created_at, updated_at)
             VALUES (?1, 'Mix', '', 0, ?2, ?3, ?4, ?5, 0, ?6, ?6) RETURNING id",
        )
        .bind(user_id)
        .bind(mode)
        .bind(tag_id)
        .bind(lens)
        .bind(limit)
        .bind(NOW)
        .fetch_one(pool)
        .await
        .unwrap(),
    )
    .await
    .unwrap()
    .unwrap();
    CuratedSpec { cp }
}

struct CuratedSpec {
    cp: curate::CuratedPlaylist,
}

#[tokio::test]
async fn resolve_newest_lens_orders_by_first_seen() {
    let fx = setup().await;
    let a = track(&fx.pool, "a", "A", "2026-01-01T00:00:00+00:00").await;
    let b = track(&fx.pool, "b", "B", "2026-03-01T00:00:00+00:00").await;
    let c = track(&fx.pool, "c", "C", "2026-02-01T00:00:00+00:00").await;

    let spec = create_curated(&fx.pool, fx.user_id, "lens", None, Some("newest"), 10).await;
    let ids = curate::resolve_track_ids(&fx.pool, &spec.cp).await.unwrap();
    assert_eq!(ids, vec![b, c, a], "newest first");
}

#[tokio::test]
async fn resolve_tag_lens_returns_only_tagged_tracks() {
    let fx = setup().await;
    let a = track(&fx.pool, "a", "A", NOW).await;
    let _b = track(&fx.pool, "b", "B", NOW).await;
    let tag_id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at)
         VALUES (?1, 'dark', 'dark', ?2) RETURNING id",
    )
    .bind(fx.user_id)
    .bind(NOW)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(a)
        .bind(tag_id)
        .execute(&fx.pool)
        .await
        .unwrap();

    let spec = create_curated(&fx.pool, fx.user_id, "tag", Some(tag_id), None, 10).await;
    let ids = curate::resolve_track_ids(&fx.pool, &spec.cp).await.unwrap();
    assert_eq!(ids, vec![a]);
}

#[tokio::test]
async fn push_creates_playlist_and_replaces_items() {
    let fx = setup().await;
    let a = track(&fx.pool, "aaa", "A", "2026-03-01T00:00:00+00:00").await;
    let b = track(&fx.pool, "bbb", "B", "2026-02-01T00:00:00+00:00").await;

    let spec = create_curated(&fx.pool, fx.user_id, "lens", None, Some("newest"), 10).await;
    let res = curate::push(&fx.pool, &fx.cfg, &spec.cp).await.unwrap();
    assert_eq!(res.pushed, 2);
    assert_eq!(res.spotify_playlist_id, "newpl");
    let _ = (a, b);

    // Playlist was created with our name, items replaced with spotify:track uris.
    let created = fx.cap.created.lock().unwrap().clone();
    assert_eq!(created.len(), 1);
    assert_eq!(created[0]["name"], "Mix");

    let replaced = fx.cap.replaced.lock().unwrap().clone();
    assert_eq!(replaced.len(), 1);
    let uris = replaced[0]["uris"].as_array().unwrap();
    assert_eq!(
        uris.iter().map(|u| u.as_str().unwrap()).collect::<Vec<_>>(),
        vec!["spotify:track:aaa", "spotify:track:bbb"]
    );

    // DB records the push.
    let (pid, status): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT spotify_playlist_id, last_push_status FROM hub_curated_playlists WHERE id = ?1",
    )
    .bind(spec.cp.id)
    .fetch_one(&fx.pool)
    .await
    .unwrap();
    assert_eq!(pid.as_deref(), Some("newpl"));
    assert_eq!(status.as_deref(), Some("ok"));
}

#[tokio::test]
async fn push_reuses_existing_spotify_playlist() {
    let fx = setup().await;
    track(&fx.pool, "aaa", "A", NOW).await;
    let spec = create_curated(&fx.pool, fx.user_id, "lens", None, Some("newest"), 10).await;
    sqlx::query("UPDATE hub_curated_playlists SET spotify_playlist_id = 'existing' WHERE id = ?1")
        .bind(spec.cp.id)
        .execute(&fx.pool)
        .await
        .unwrap();

    let cp = curate::get(&fx.pool, spec.cp.id).await.unwrap().unwrap();
    let res = curate::push(&fx.pool, &fx.cfg, &cp).await.unwrap();
    assert_eq!(res.spotify_playlist_id, "existing");
    assert!(fx.cap.created.lock().unwrap().is_empty(), "no new playlist");
    assert_eq!(fx.cap.replaced.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn add_to_spotify_playlist_writes_to_spotify() {
    let (base, cap) = mock_spotify().await;
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let mut cfg = Config::for_test(url);
    cfg.spotify_api_base = base;
    let app = common::spawn_with(dir, cfg).await;
    let (uid, cookie) = app.user_with_password("zoe", "pw").await;
    sqlx::query(
        "INSERT INTO hub_service_accounts
             (user_id, service, access_token, refresh_token, token_expiry, scopes, authorized_at, connected_at)
         VALUES (?1, 'spotify', 'tok', 'ref', '2099-01-01T00:00:00+00:00',
                 'playlist-modify-private', ?2, ?2)",
    )
    .bind(uid)
    .bind(NOW)
    .execute(&app.pool)
    .await
    .unwrap();
    let pl_id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_playlists (user_id, service, playlist_id, name, is_owned)
         VALUES (?1, 'spotify', 'rem', 'My Rem', 1) RETURNING id",
    )
    .bind(uid)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    let tid = track(&app.pool, "spTRK", "T", NOW).await;

    let resp = app
        .client()
        .post(app.url(&format!("/track/{tid}/actions/playlist")))
        .header("Cookie", &cookie)
        .form(&[("sp_playlist_id", &pl_id.to_string())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let added = cap.added.lock().unwrap().clone();
    assert_eq!(added.len(), 1, "one POST to Spotify");
    let uris = added[0]["uris"].as_array().unwrap();
    assert_eq!(uris[0].as_str().unwrap(), "spotify:track:spTRK");
}

#[tokio::test]
async fn daily_due_selects_only_daily_pushed_playlists() {
    let fx = setup().await;
    track(&fx.pool, "aaa", "A", NOW).await;

    let due = create_curated(&fx.pool, fx.user_id, "lens", None, Some("newest"), 10).await;
    sqlx::query(
        "UPDATE hub_curated_playlists
            SET daily = 1, spotify_playlist_id = 'p1', last_push_at = '2020-01-01T00:00:00+00:00'
          WHERE id = ?1",
    )
    .bind(due.cp.id)
    .execute(&fx.pool)
    .await
    .unwrap();

    // Never pushed + daily but no remote id yet → not due.
    let notyet = create_curated(&fx.pool, fx.user_id, "lens", None, Some("newest"), 10).await;
    sqlx::query("UPDATE hub_curated_playlists SET daily = 1 WHERE id = ?1")
        .bind(notyet.cp.id)
        .execute(&fx.pool)
        .await
        .unwrap();

    let cutoff = "2026-01-01T00:00:00+00:00";
    let rows = curate::daily_due(&fx.pool, cutoff).await.unwrap();
    let ids: Vec<i64> = rows.iter().map(|c| c.id).collect();
    assert_eq!(ids, vec![due.cp.id]);
}
