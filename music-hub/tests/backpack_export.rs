//! Integration tests for the backpack export (#333): m3u8 / Traktor NML / ZIP.
//!
//! Two layers:
//! * loader behaviour on a fresh temp SQLite (`setup()`) — `export_tracks`
//!   resolve-order/score preservation, empty-metadata fallback, unknown
//!   backpack, and `backpack_owned_by` owner scoping;
//! * HTTP export against the `common` harness (`spawn()`) — login gate, owner
//!   404, m3u8 body, Traktor XML, the ZIP 502 (unconfigured music-api) and an
//!   empty backpack.

mod common;

use std::str::FromStr;
use std::time::Duration;

use music_hub::backpacks::{self, SourceRef};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

const TS: &str = "2026-01-01T00:00:00+00:00";

/// Fresh temp DB, FKs on, all migrations applied.
async fn setup() -> (tempfile::TempDir, SqlitePool) {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let opts = SqliteConnectOptions::from_str(&url)
        .expect("parse url")
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .busy_timeout(Duration::from_secs(15))
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(opts)
        .await
        .expect("connect");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("migrate");
    (dir, pool)
}

async fn insert_user(pool: &SqlitePool, slug: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_users (slug, display_name, created_at) VALUES (?1, ?1, ?2) RETURNING id",
    )
    .bind(slug)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert user")
}

async fn insert_backpack(pool: &SqlitePool, user_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_backpacks (user_id, name, is_active, created_at, updated_at)
         VALUES (?1, ?2, 0, ?3, ?3) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert backpack")
}

async fn insert_track(
    pool: &SqlitePool,
    sid: &str,
    title: Option<&str>,
    artists: Option<&str>,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, title, artists)
         VALUES ('spotify', ?1, ?2, ?3) RETURNING id",
    )
    .bind(sid)
    .bind(title)
    .bind(artists)
    .fetch_one(pool)
    .await
    .expect("insert track")
}

// ── US1: export_tracks loader ────────────────────────────────────────────────

#[tokio::test]
async fn export_tracks_preserves_resolve_order_and_score() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, alice, "B").await;

    // Two tracks; the second is contributed by two sources (higher score), so it
    // must sort in front of the single-source track.
    let t_low = insert_track(&pool, "low", Some("Low"), Some("Zzz")).await;
    let t_high = insert_track(&pool, "high", Some("High"), Some("Aaa")).await;

    let tag: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at) VALUES (?1, 'tag', 'Tag', ?2) RETURNING id",
    )
    .bind(alice)
    .bind(TS)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(t_high)
        .bind(tag)
        .execute(&pool)
        .await
        .unwrap();

    backpacks::add_source(&pool, bp, SourceRef::Tag(tag))
        .await
        .unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Track(t_high))
        .await
        .unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Track(t_low))
        .await
        .unwrap();

    let out = backpacks::export_tracks(&pool, bp).await.unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].track_id, t_high, "higher score first");
    assert_eq!(out[0].score, 2);
    assert_eq!(out[1].track_id, t_low);
    assert_eq!(out[1].score, 1);
    assert_eq!(out[0].title, "High");
    assert_eq!(out[0].artists, "Aaa");
}

#[tokio::test]
async fn export_tracks_missing_metadata_is_empty_not_error() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, alice, "B").await;

    let t_null = insert_track(&pool, "null", None, None).await;
    backpacks::add_source(&pool, bp, SourceRef::Track(t_null))
        .await
        .unwrap();

    let out = backpacks::export_tracks(&pool, bp).await.unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].track_id, t_null);
    assert_eq!(out[0].title, "");
    assert_eq!(out[0].artists, "");
}

#[tokio::test]
async fn export_tracks_unknown_backpack_is_empty() {
    let (_dir, pool) = setup().await;
    let out = backpacks::export_tracks(&pool, 999_999).await.unwrap();
    assert!(out.is_empty());
}

#[tokio::test]
async fn backpack_owned_by_scopes_to_owner() {
    let (_dir, pool) = setup().await;
    let alice = insert_user(&pool, "alice").await;
    let bob = insert_user(&pool, "bob").await;
    let bp = insert_backpack(&pool, alice, "Mine").await;

    let own = backpacks::backpack_owned_by(&pool, bp, alice)
        .await
        .unwrap();
    assert_eq!(own.map(|b| b.id), Some(bp));

    let foreign = backpacks::backpack_owned_by(&pool, bp, bob).await.unwrap();
    assert!(foreign.is_none(), "foreign owner sees nothing");

    let missing = backpacks::backpack_owned_by(&pool, 999_999, alice)
        .await
        .unwrap();
    assert!(missing.is_none());
}

// ── US4/US6: HTTP export ─────────────────────────────────────────────────────

/// A backpack (owned by `owner`) whose only source is the seeded playlist
/// `pl_alice` — it resolves to exactly `[t_all, t_pl]`.
async fn backpack_with_seeded_playlist(app: &common::TestApp, owner: i64) -> i64 {
    let bp = insert_backpack(&app.pool, owner, "Reisekoffer").await;
    backpacks::add_source(&app.pool, bp, SourceRef::Playlist(app.seed.pl_alice))
        .await
        .unwrap();
    bp
}

#[tokio::test]
async fn export_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url("/backpacks/1/export"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn export_foreign_backpack_is_404_and_own_is_200() {
    let app = common::spawn().await;
    let (owner_a, cookie_a) = app.user_with_password("owner-a", "pw").await;
    let (_owner_b, cookie_b) = app.user_with_password("owner-b", "pw").await;
    let bp = backpack_with_seeded_playlist(&app, owner_a).await;

    let foreign = app
        .client()
        .get(app.url(&format!("/backpacks/{bp}/export?format=m3u8")))
        .header("Cookie", &cookie_b)
        .send()
        .await
        .unwrap();
    assert_eq!(foreign.status(), reqwest::StatusCode::NOT_FOUND);

    let own = app
        .client()
        .get(app.url(&format!("/backpacks/{bp}/export?format=m3u8")))
        .header("Cookie", &cookie_a)
        .send()
        .await
        .unwrap();
    assert_eq!(own.status(), reqwest::StatusCode::OK);
}

#[tokio::test]
async fn export_m3u8_body_has_resolved_tracks_in_order() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("packer", "pw").await;
    let bp = backpack_with_seeded_playlist(&app, owner).await;

    let resp = app
        .client()
        .get(app.url(&format!("/backpacks/{bp}/export?format=m3u8")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(
        resp.headers()["content-type"],
        "audio/x-mpegurl; charset=utf-8"
    );
    let cd = resp.headers()["content-disposition"].to_str().unwrap();
    assert!(cd.contains("attachment"));
    assert!(cd.contains(".m3u8"));

    let body = resp.text().await.unwrap();
    assert!(body.starts_with("#EXTM3U\n"), "m3u8 header");
    assert!(body.contains("#PLAYLIST:Reisekoffer"));
    // pl_alice = [t_all (pos 0), t_pl (pos 1)]; same score, tie-break track_id.
    let first = format!("/track/{}/stream", app.seed.t_all);
    let second = format!("/track/{}/stream", app.seed.t_pl);
    assert!(body.contains(&first));
    assert!(body.contains(&second));
    assert!(
        body.find(&first).unwrap() < body.find(&second).unwrap(),
        "resolve order preserved"
    );
}

#[tokio::test]
async fn export_traktor_body_parses_as_xml() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("packer", "pw").await;
    let bp = backpack_with_seeded_playlist(&app, owner).await;

    let resp = app
        .client()
        .get(app.url(&format!("/backpacks/{bp}/export?format=traktor")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    assert_eq!(
        resp.headers()["content-type"],
        "application/xml; charset=utf-8"
    );
    assert!(
        resp.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains(".nml")
    );

    let body = resp.text().await.unwrap();
    let doc = roxmltree::Document::parse(&body).expect("valid NML");
    let pl_nodes: Vec<_> = doc
        .descendants()
        .filter(|n| n.has_tag_name("NODE") && n.attribute("TYPE") == Some("PLAYLIST"))
        .collect();
    assert_eq!(pl_nodes.len(), 1);
    assert_eq!(pl_nodes[0].attribute("NAME"), Some("Reisekoffer"));
    let pks: Vec<_> = doc
        .descendants()
        .filter(|n| n.has_tag_name("PRIMARYKEY"))
        .map(|n| n.attribute("KEY").unwrap().to_string())
        .collect();
    // default audio format is flac; basenames mirror the ZIP export.
    assert_eq!(
        pks,
        vec!["Aaa - Shared Anthem.flac", "Ccc - Playlist Only.flac"]
    );
}

#[tokio::test]
async fn export_zip_is_502_without_music_api() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("packer", "pw").await;
    let bp = backpack_with_seeded_playlist(&app, owner).await;

    let resp = app
        .client()
        .get(app.url(&format!("/backpacks/{bp}/export?format=zip&audio=flac")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_GATEWAY);
    let body = resp.text().await.unwrap();
    assert!(body.contains("Keine Dateien über music-api verfügbar"));
}

#[tokio::test]
async fn export_empty_backpack_yields_valid_empty_documents() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("packer", "pw").await;
    let bp = insert_backpack(&app.pool, owner, "Leer").await;

    let m3u8 = app
        .client()
        .get(app.url(&format!("/backpacks/{bp}/export?format=m3u8")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(m3u8.status(), reqwest::StatusCode::OK);
    let body = m3u8.text().await.unwrap();
    assert_eq!(body, "#EXTM3U\n#PLAYLIST:Leer\n");

    let nml = app
        .client()
        .get(app.url(&format!("/backpacks/{bp}/export?format=traktor")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(nml.status(), reqwest::StatusCode::OK);
    let body = nml.text().await.unwrap();
    let doc = roxmltree::Document::parse(&body).expect("valid empty NML");
    let coll = doc
        .descendants()
        .find(|n| n.has_tag_name("COLLECTION"))
        .unwrap();
    assert_eq!(coll.attribute("ENTRIES"), Some("0"));
}
