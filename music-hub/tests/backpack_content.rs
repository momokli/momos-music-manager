//! Integration tests for backpack content materialisation (#682, slice B1).
//!
//! Drives `backpacks::refresh_item_tracks` / `refresh_all` and the read-only
//! stats helpers against a fresh temp SQLite (all migrations applied):
//! schema + cascade, per-kind backend literals, sum vs. `resolve_unbounded`,
//! availability (`x/y verfügbar`), idempotency, and empty sources.

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
    sqlx::migrate!("./migrations").run(&pool).await.expect("migrate");
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

async fn insert_track(pool: &SqlitePool, sid: &str, isrc: Option<&str>) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, title, artists, isrc, first_seen_at)
         VALUES ('spotify', ?1, ?1, 'X', ?2, ?3) RETURNING id",
    )
    .bind(sid)
    .bind(isrc)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert track")
}

async fn insert_tag(pool: &SqlitePool, owner: i64, slug: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at)
         VALUES (?1, ?2, ?2, ?3) RETURNING id",
    )
    .bind(owner)
    .bind(slug)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert tag")
}

async fn tag_track(pool: &SqlitePool, tag_id: i64, track_id: i64) {
    sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(track_id)
        .bind(tag_id)
        .execute(pool)
        .await
        .expect("tag track");
}

async fn insert_playlist(pool: &SqlitePool, user_id: i64, pid: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_playlists (user_id, service, playlist_id, name, items_available, fetched_at)
         VALUES (?1, 'spotify', ?2, ?2, 1, ?3) RETURNING id",
    )
    .bind(user_id)
    .bind(pid)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert playlist")
}

async fn playlist_track(pool: &SqlitePool, playlist_id: i64, track_id: i64, pos: i64) {
    sqlx::query("INSERT INTO hub_playlist_tracks (playlist_id, track_id, position, added_at) VALUES (?1, ?2, ?3, ?4)")
        .bind(playlist_id)
        .bind(track_id)
        .bind(pos)
        .bind(TS)
        .execute(pool)
        .await
        .expect("playlist track");
}

async fn materialised(pool: &SqlitePool, item_id: i64) -> Vec<(i64, String, String)> {
    sqlx::query_as(
        "SELECT track_id, backend, computed_at FROM hub_backpack_item_tracks
          WHERE item_id = ?1 ORDER BY track_id",
    )
    .bind(item_id)
    .fetch_all(pool)
    .await
    .expect("load materialised")
}

// ── 1. Schema + FK cascade ───────────────────────────────────────────────────

#[tokio::test]
async fn schema_has_table_pk_index_and_item_cascade() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "schemer").await;
    let bp = insert_backpack(&pool, user, "P").await;
    let t1 = insert_track(&pool, "s1", None).await;
    let t2 = insert_track(&pool, "s2", None).await;
    let tag = insert_tag(&pool, user, "moody").await;
    tag_track(&pool, tag, t1).await;
    tag_track(&pool, tag, t2).await;

    let item = backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    assert_eq!(materialised(&pool, item).await.len(), 2, "add materialises");

    // Index exists.
    let idx: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_hub_backpack_item_tracks_track'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert!(idx.is_some(), "track_id index exists");

    // PK (item_id, track_id): a duplicate insert fails.
    let dup = sqlx::query(
        "INSERT INTO hub_backpack_item_tracks (item_id, track_id, backend, computed_at)
         VALUES (?1, ?2, 'tag', ?3)",
    )
    .bind(item)
    .bind(t1)
    .bind(TS)
    .execute(&pool)
    .await;
    assert!(dup.is_err(), "PK rejects duplicate (item, track)");

    // Removing the item cascades its rows away.
    assert!(backpacks::remove_source(&pool, item).await.unwrap());
    let remaining: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_backpack_item_tracks WHERE item_id = ?1")
            .bind(item)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0, "ON DELETE CASCADE clears materialised rows");
}

#[tokio::test]
async fn track_delete_cascades_materialised_rows() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "trackcascade").await;
    let bp = insert_backpack(&pool, user, "P").await;
    let t1 = insert_track(&pool, "tc1", None).await;
    let t2 = insert_track(&pool, "tc2", None).await;
    let tag = insert_tag(&pool, user, "t").await;
    tag_track(&pool, tag, t1).await;
    tag_track(&pool, tag, t2).await;
    let item = backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    assert_eq!(materialised(&pool, item).await.len(), 2);

    sqlx::query("DELETE FROM hub_tracks WHERE id = ?1")
        .bind(t1)
        .execute(&pool)
        .await
        .unwrap();
    let rows = materialised(&pool, item).await;
    assert_eq!(rows.len(), 1, "track delete cascades");
    assert_eq!(rows[0].0, t2);
}

// ── 2. Backend literals per kind ─────────────────────────────────────────────

#[tokio::test]
async fn backend_literals_per_kind() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "backend").await;
    let bp = insert_backpack(&pool, user, "P").await;
    let t1 = insert_track(&pool, "b1", None).await;
    let t2 = insert_track(&pool, "b2", None).await;
    let t3 = insert_track(&pool, "b3", None).await;

    let tag = insert_tag(&pool, user, "tg").await;
    tag_track(&pool, tag, t1).await;
    let pl = insert_playlist(&pool, user, "pl").await;
    playlist_track(&pool, pl, t2, 0).await;

    let i_tag = backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    let i_pl = backpacks::add_source(&pool, bp, SourceRef::Playlist(pl)).await.unwrap();
    let i_lens = backpacks::add_source(&pool, bp, SourceRef::Lens("newest".to_string()))
        .await
        .unwrap();
    let i_track = backpacks::add_source(&pool, bp, SourceRef::Track(t3)).await.unwrap();

    assert_eq!(materialised(&pool, i_tag).await[0].1, "tag");
    assert_eq!(materialised(&pool, i_pl).await[0].1, "playlist");
    assert_eq!(materialised(&pool, i_track).await[0].1, "track");
    let lens_backends: Vec<String> = materialised(&pool, i_lens)
        .await
        .into_iter()
        .map(|(_, b, _)| b)
        .collect();
    assert!(!lens_backends.is_empty());
    assert!(
        lens_backends.iter().all(|b| b == "lens:newest"),
        "lens backend is lens:<key>: {lens_backends:?}"
    );
}

// ── 3. Sum over items == resolve_unbounded ───────────────────────────────────

#[tokio::test]
async fn materialised_union_matches_resolver() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "union").await;
    let bp = insert_backpack(&pool, user, "P").await;
    let t1 = insert_track(&pool, "u1", None).await;
    let t2 = insert_track(&pool, "u2", None).await;
    let t3 = insert_track(&pool, "u3", None).await;
    let t4 = insert_track(&pool, "u4", None).await;

    let tag = insert_tag(&pool, user, "u-tag").await;
    tag_track(&pool, tag, t1).await;
    tag_track(&pool, tag, t2).await;
    let pl = insert_playlist(&pool, user, "u-pl").await;
    playlist_track(&pool, pl, t2, 0).await;
    playlist_track(&pool, pl, t3, 1).await;
    // liked lens (user likes t3 + t4).
    sqlx::query("INSERT INTO hub_liked_tracks (user_id, track_id, liked_at) VALUES (?1, ?2, ?3)")
        .bind(user)
        .bind(t3)
        .bind(TS)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO hub_liked_tracks (user_id, track_id, liked_at) VALUES (?1, ?2, ?3)")
        .bind(user)
        .bind(t4)
        .bind(TS)
        .execute(&pool)
        .await
        .unwrap();

    backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Playlist(pl)).await.unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Lens("liked".to_string()))
        .await
        .unwrap();

    let distinct: i64 = sqlx::query_scalar(
        "SELECT COUNT(DISTINCT mt.track_id)
           FROM hub_backpack_items i
           JOIN hub_backpack_item_tracks mt ON mt.item_id = i.id
          WHERE i.backpack_id = ?1",
    )
    .bind(bp)
    .fetch_one(&pool)
    .await
    .unwrap();
    let resolved = backpacks::resolve_unbounded(&pool, bp).await.unwrap().len() as i64;
    assert_eq!(distinct, resolved, "materialised union == resolver");
    assert_eq!(distinct, 4, "tag ∪ playlist ∪ liked = {{t1,t2,t3,t4}}");

    let stats = backpacks::backpack_stats(&pool, bp).await.unwrap();
    assert_eq!(stats.track_count, resolved);
}

// ── 4. Availability: x/y verfügbar ──────────────────────────────────────────

#[tokio::test]
async fn availability_counts_ready_over_with_isrc() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "avail").await;
    let bp = insert_backpack(&pool, user, "P").await;
    let pl = insert_playlist(&pool, user, "src").await;

    // 5 tracks: 2 ready, 1 non-ready with ISRC, 1 empty ISRC (ready in cache!),
    // 1 NULL ISRC.
    let t1 = insert_track(&pool, "a1", Some("ISRC1")).await;
    let t2 = insert_track(&pool, "a2", Some("ISRC2")).await;
    let t3 = insert_track(&pool, "a3", Some("ISRC3")).await;
    let t4 = insert_track(&pool, "a4", Some("")).await;
    let t5 = insert_track(&pool, "a5", None).await;
    for (pos, t) in [t1, t2, t3, t4, t5].iter().enumerate() {
        playlist_track(&pool, pl, *t, pos as i64).await;
    }

    for (isrc, state) in [("ISRC1", "ready"), ("ISRC2", "ready"), ("ISRC3", "missing"), ("", "ready")] {
        sqlx::query("INSERT INTO hub_music_state (isrc, state, checked_at) VALUES (?1, ?2, ?3)")
            .bind(isrc)
            .bind(state)
            .bind(TS)
            .execute(&pool)
            .await
            .unwrap();
    }

    let item = backpacks::add_source(&pool, bp, SourceRef::Playlist(pl)).await.unwrap();

    let st = backpacks::source_stats(&pool, bp).await.unwrap();
    assert_eq!(st.len(), 1);
    assert_eq!(st[0].item_id, item);
    assert_eq!(st[0].track_count, 5);
    assert_eq!(st[0].ready, 2, "only ready + non-empty ISRC counts");
    assert_eq!(st[0].with_isrc, 3, "empty/NULL ISRC excluded from y");

    let bs = backpacks::backpack_stats(&pool, bp).await.unwrap();
    assert_eq!(bs.track_count, 5);
    assert_eq!(bs.ready, 2);
    assert_eq!(bs.with_isrc, 3);
}

#[tokio::test]
async fn availability_is_distinct_across_items() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "distinct").await;
    let bp = insert_backpack(&pool, user, "P").await;
    let t1 = insert_track(&pool, "d1", Some("D1")).await;
    let t2 = insert_track(&pool, "d2", Some("D2")).await;
    let tag = insert_tag(&pool, user, "d-tag").await;
    tag_track(&pool, tag, t1).await;
    tag_track(&pool, tag, t2).await;
    let pl = insert_playlist(&pool, user, "d-pl").await;
    playlist_track(&pool, pl, t1, 0).await;
    for isrc in ["D1", "D2"] {
        sqlx::query("INSERT INTO hub_music_state (isrc, state, checked_at) VALUES (?1, 'ready', ?2)")
            .bind(isrc)
            .bind(TS)
            .execute(&pool)
            .await
            .unwrap();
    }
    backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Playlist(pl)).await.unwrap();

    let bs = backpacks::backpack_stats(&pool, bp).await.unwrap();
    assert_eq!(bs.track_count, 2, "distinct, not summed");
    assert_eq!(bs.ready, 2);
    assert_eq!(bs.with_isrc, 2);
}

// ── 5. Idempotency ──────────────────────────────────────────────────────────

#[tokio::test]
async fn refresh_is_idempotent_and_preserves_computed_at() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "idem").await;
    let bp = insert_backpack(&pool, user, "P").await;
    let t1 = insert_track(&pool, "i1", None).await;
    let t2 = insert_track(&pool, "i2", None).await;
    let tag = insert_tag(&pool, user, "i-tag").await;
    tag_track(&pool, tag, t1).await;
    tag_track(&pool, tag, t2).await;

    let item = backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    let before = materialised(&pool, item).await;
    assert_eq!(before.len(), 2);

    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    assert_eq!(
        backpacks::refresh_item_tracks(&pool, item).await.unwrap(),
        0,
        "second refresh writes nothing"
    );
    assert_eq!(
        backpacks::refresh_all(&pool).await.unwrap(),
        0,
        "refresh_all writes nothing unchanged"
    );
    let after = materialised(&pool, item).await;
    assert_eq!(before, after, "rows and computed_at unchanged");

    // A changed source is re-materialised.
    let t3 = insert_track(&pool, "i3", None).await;
    tag_track(&pool, tag, t3).await;
    assert_eq!(
        backpacks::refresh_item_tracks(&pool, item).await.unwrap(),
        3,
        "changed source rewrites the set"
    );
    assert_eq!(materialised(&pool, item).await.len(), 3);
    // And a following run is a no-op again.
    assert_eq!(backpacks::refresh_item_tracks(&pool, item).await.unwrap(), 0);
}

// ── 6. Sources without resolvable tracks ────────────────────────────────────

#[tokio::test]
async fn empty_sources_materialise_zero_rows() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "empty").await;
    let bp = insert_backpack(&pool, user, "P").await;
    // Tag with no members and an empty "liked" lens.
    let tag = insert_tag(&pool, user, "nobody").await;
    let i_tag = backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    let i_lens = backpacks::add_source(&pool, bp, SourceRef::Lens("liked".to_string()))
        .await
        .unwrap();

    assert!(materialised(&pool, i_tag).await.is_empty());
    assert!(materialised(&pool, i_lens).await.is_empty());

    let st = backpacks::source_stats(&pool, bp).await.unwrap();
    assert_eq!(st.len(), 2);
    assert!(st.iter().all(|s| s.track_count == 0));

    let bs = backpacks::backpack_stats(&pool, bp).await.unwrap();
    assert_eq!(bs.track_count, 0);
    assert_eq!(bs.ready, 0);
    assert_eq!(bs.with_isrc, 0);
}

#[tokio::test]
async fn refresh_of_missing_item_is_ok() {
    let (_dir, pool) = setup().await;
    assert_eq!(backpacks::refresh_item_tracks(&pool, 9999).await.unwrap(), 0);
    // Unknown backpack stats are all-zero, not an error.
    let stats = backpacks::backpack_stats(&pool, 9999).await.unwrap();
    assert_eq!(stats, backpacks::BackpackStats::default());
    assert!(backpacks::source_stats(&pool, 9999).await.unwrap().is_empty());
    assert_eq!(backpacks::refresh_all(&pool).await.unwrap(), 0);
}