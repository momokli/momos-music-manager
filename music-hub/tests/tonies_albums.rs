//! Album sources (`plans/proposed/tonie-album-queue.md`): the resolver talks to
//! a mocked music-api (no network), and the fit only ever places **whole**
//! albums.

use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};
use sqlx::SqlitePool;
use std::sync::{Arc, Mutex};

use music_hub::config::Config;
use music_hub::tonies;

const TS: &str = "2026-01-01T00:00:00+00:00";

#[derive(Clone, Default)]
struct Mock {
    /// `isrc` values music-api reports as ready.
    ready: Arc<Mutex<Vec<String>>>,
}

fn album_summary(id: &str, title: &str, released: &str) -> Value {
    json!({
        "albumId": id,
        "title": title,
        "releaseDate": released,
        "recordType": "album",
        "coverUrl": format!("https://cdn.example/{id}.jpg"),
        "nbTracks": null
    })
}

fn album_detail(id: &str, title: &str, released: &str, isrcs: &[&str]) -> Value {
    let tracks: Vec<Value> = isrcs
        .iter()
        .enumerate()
        .map(|(i, isrc)| {
            json!({
                "isrc": isrc,
                "title": format!("{title} Kapitel {:02}", i + 1),
                "durationMs": 100_000,
                "position": i + 1
            })
        })
        .collect();
    json!({
        "albumId": id,
        "title": title,
        "releaseDate": released,
        "recordType": "album",
        "coverUrl": format!("https://cdn.example/{id}.jpg"),
        "seconds": isrcs.len() as i64 * 100,
        "tracks": tracks
    })
}

async fn spawn_music_api() -> (String, Mock) {
    let st = Mock::default();
    let albums_state = st.clone();
    let album_state = st.clone();

    let albums = move |State(_s): State<Mock>| {
        let _ = &albums_state;
        async move {
            Json(json!({
                "albums": [
                    album_summary("459", "Folge 459", "2026-11-13"),
                    album_summary("458", "Folge 458", "2026-11-06"),
                    album_summary("457", "Folge 457", "2026-10-30")
                ],
                "total": 3,
                "next": null
            }))
        }
    };
    let album = move |State(_s): State<Mock>, Path(id): Path<String>| {
        let _ = &album_state;
        async move {
            let body = match id.as_str() {
                "459" => album_detail("459", "Folge 459", "2026-11-13", &["A1", "A2"]),
                "458" => album_detail("458", "Folge 458", "2026-11-06", &["B1", "B2"]),
                _ => album_detail("457", "Folge 457", "2026-10-30", &["C1", "C2"]),
            };
            Json(body)
        }
    };
    let isrc = |State(s): State<Mock>, Path(isrc): Path<String>| async move {
        let ready = s.ready.lock().unwrap().contains(&isrc);
        Json(json!({ "isrc": isrc, "state": if ready { "ready" } else { "absent" } }))
    };

    let app = Router::new()
        .route("/deezer/artist/{id}/albums", get(albums))
        .route("/deezer/album/{id}", get(album))
        .route("/isrc/{isrc}", get(isrc))
        .with_state(st.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), st)
}

async fn setup() -> (tempfile::TempDir, SqlitePool, Config, Mock, i64) {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let pool = music_hub::db::connect(&url).await.expect("connect");
    let (base, mock) = spawn_music_api().await;

    let mut cfg = Config::for_test(url);
    cfg.music_api_base = base;
    cfg.music_api_token = Some("test-token".to_string());

    let uid: i64 = sqlx::query_scalar(
        "INSERT INTO hub_users (slug, display_name, created_at) VALUES ('u', 'u', ?1) RETURNING id",
    )
    .bind(TS)
    .fetch_one(&pool)
    .await
    .unwrap();

    (dir, pool, cfg, mock, uid)
}

async fn insert_binding(pool: &SqlitePool, uid: i64, artist: i64, mode: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tonie_bindings
             (user_id, tonie_id, tonie_name, source_kind, source_id, format, cap_seconds,
              mode, order_by, album_mode, created_at, updated_at)
         VALUES (?1, 'ct1', 'Tonie', 'albums', ?2, 'mp3-128', 5400, 'manual', 'playlist', ?3, ?4, ?4)
         RETURNING id",
    )
    .bind(uid)
    .bind(artist)
    .bind(mode)
    .bind(TS)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn binding(pool: &SqlitePool, uid: i64) -> tonies::Binding {
    tonies::get_binding(pool, uid, "ct1")
        .await
        .unwrap()
        .unwrap()
}

fn ids(albums: &[tonies::AlbumPlan]) -> Vec<&str> {
    albums.iter().map(|a| a.album_id.as_str()).collect()
}

#[tokio::test]
async fn newest_and_oldest_are_mirror_images() {
    let (_d, pool, cfg, _mock, uid) = setup().await;
    insert_binding(&pool, uid, 15167501, "newest").await;

    let newest = tonies::resolve(&pool, &cfg, uid, &binding(&pool, uid).await)
        .await
        .unwrap();
    assert_eq!(ids(&newest), vec!["459", "458", "457"]);

    sqlx::query("UPDATE hub_tonie_bindings SET album_mode = 'oldest'")
        .execute(&pool)
        .await
        .unwrap();
    let oldest = tonies::resolve(&pool, &cfg, uid, &binding(&pool, uid).await)
        .await
        .unwrap();
    assert_eq!(ids(&oldest), vec!["457", "458", "459"]);
}

#[tokio::test]
async fn fit_places_whole_albums_only() {
    let (_d, pool, cfg, _mock, uid) = setup().await;
    insert_binding(&pool, uid, 15167501, "newest").await;

    let albums = tonies::resolve(&pool, &cfg, uid, &binding(&pool, uid).await)
        .await
        .unwrap();
    // Each mocked album is 200 s. A 500 s cap fits two, the third is reported.
    let fit = tonies::fit_albums(albums, 500, |_| true);
    assert_eq!(ids(&fit.albums), vec!["459", "458"]);
    assert_eq!(fit.seconds, 400);
    assert_eq!(fit.skipped.len(), 1);
    assert_eq!(fit.skipped[0].title, "Folge 457");
}

#[tokio::test]
async fn partially_ready_album_is_skipped_as_a_unit() {
    let (_d, pool, cfg, mock, uid) = setup().await;
    insert_binding(&pool, uid, 15167501, "newest").await;
    // Everything ready except B1 (second album) -> that album is skipped whole.
    *mock.ready.lock().unwrap() = vec!["A1".into(), "A2".into(), "C1".into(), "C2".into()];
    let albums = tonies::resolve(&pool, &cfg, uid, &binding(&pool, uid).await)
        .await
        .unwrap();

    let ready = |isrc: &str| mock.ready.lock().unwrap().contains(&isrc.to_string());
    let fit = tonies::fit_albums(albums, 5400, ready);
    assert_eq!(ids(&fit.albums), vec!["459", "457"]);
    assert_eq!(fit.skipped.len(), 1);
    assert_eq!(fit.skipped[0].title, "Folge 458");
    assert_eq!(fit.skipped[0].reason, tonies::SkipReason::AlbumNotReady);
}

#[tokio::test]
async fn listened_albums_are_skipped_and_unlisten_brings_them_back() {
    let (_d, pool, cfg, _mock, uid) = setup().await;
    insert_binding(&pool, uid, 15167501, "oldest").await;

    let albums = tonies::resolve(&pool, &cfg, uid, &binding(&pool, uid).await)
        .await
        .unwrap();
    assert_eq!(ids(&albums), vec!["457", "458", "459"]);

    // Fingerprint the first album as listened.
    tonies::mark_seen(&pool, uid, &albums[..1]).await.unwrap();
    let after = tonies::resolve(&pool, &cfg, uid, &binding(&pool, uid).await)
        .await
        .unwrap();
    assert_eq!(ids(&after), vec!["458", "459"]);

    tonies::unlisten_album(&pool, uid, "457").await.unwrap();
    let back = tonies::resolve(&pool, &cfg, uid, &binding(&pool, uid).await)
        .await
        .unwrap();
    assert_eq!(ids(&back), vec!["457", "458", "459"]);
}

#[tokio::test]
async fn basket_order_wins_over_the_ledger() {
    let (_d, pool, cfg, _mock, uid) = setup().await;
    let binding_id = insert_binding(&pool, uid, 15167501, "basket").await;

    // User picked 457 first, then 459; the ledger must not filter either.
    tonies::set_album_items(
        &pool,
        binding_id,
        &[
            ("457".to_string(), "Folge 457".to_string(), 200),
            ("459".to_string(), "Folge 459".to_string(), 200),
        ],
    )
    .await
    .unwrap();
    let albums = tonies::resolve(&pool, &cfg, uid, &binding(&pool, uid).await)
        .await
        .unwrap();
    assert_eq!(ids(&albums), vec!["457", "459"]);
}

#[tokio::test]
async fn album_mode_round_trips_through_the_store() {
    let (_d, pool, _cfg, _mock, uid) = setup().await;
    insert_binding(&pool, uid, 15167501, "newest").await;
    assert_eq!(binding(&pool, uid).await.album_mode, "newest");

    assert!(
        tonies::set_binding_album_mode(&pool, uid, "ct1", "oldest")
            .await
            .unwrap()
    );
    assert_eq!(binding(&pool, uid).await.album_mode, "oldest");
    // Someone else's tonie is never touched.
    assert!(
        !tonies::set_binding_album_mode(&pool, uid + 1, "ct1", "basket")
            .await
            .unwrap()
    );
}
