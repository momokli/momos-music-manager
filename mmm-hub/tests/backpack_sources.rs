//! Integration tests for the backpack sources layer (#329).
//!
//! Boots a fresh temp SQLite with all migrations applied (setup copied from
//! `backpack_schema.rs`), seeds user/backpack/tag/playlist/track and exercises
//! `backpacks::{add_source, list_sources, remove_source, clear_sources}`.

use std::str::FromStr;
use std::time::Duration;

use mmm_hub::backpacks::{self, BackpackItem, SourceKind, SourceRef};
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

async fn insert_tag(pool: &SqlitePool, owner_user_id: i64, slug: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at) VALUES (?1, ?2, ?2, ?3) RETURNING id",
    )
    .bind(owner_user_id)
    .bind(slug)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert tag")
}

async fn insert_playlist(pool: &SqlitePool, user_id: i64, ext: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_playlists (user_id, service, playlist_id) VALUES (?1, 'spotify', ?2) RETURNING id",
    )
    .bind(user_id)
    .bind(ext)
    .fetch_one(pool)
    .await
    .expect("insert playlist")
}

async fn insert_track(pool: &SqlitePool, ext: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id) VALUES ('spotify', ?1) RETURNING id",
    )
    .bind(ext)
    .fetch_one(pool)
    .await
    .expect("insert track")
}

async fn insert_backpack(pool: &SqlitePool, user_id: i64, name: &str, active: i64) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_backpacks (user_id, name, is_active, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?4) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(active)
    .bind(TS)
    .fetch_one(pool)
    .await
    .expect("insert backpack")
}

async fn backpack_updated_at(pool: &SqlitePool, backpack_id: i64) -> String {
    sqlx::query_scalar("SELECT updated_at FROM hub_backpacks WHERE id = ?1")
        .bind(backpack_id)
        .fetch_one(pool)
        .await
        .expect("updated_at")
}

/// (kind, tag, playlist, lens, track) of one persisted row, by item id.
async fn source_columns(pool: &SqlitePool, item_id: i64) -> (String, Option<i64>, Option<i64>, Option<String>, Option<i64>) {
    let row: (String, Option<i64>, Option<i64>, Option<String>, Option<i64>) = sqlx::query_as(
        "SELECT source_kind, source_tag_id, source_playlist_id, source_lens, source_track_id
           FROM hub_backpack_items WHERE id = ?1",
    )
    .bind(item_id)
    .fetch_one(pool)
    .await
    .expect("source columns");
    row
}

async fn count_items(pool: &SqlitePool, backpack_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM hub_backpack_items WHERE backpack_id = ?1")
        .bind(backpack_id)
        .fetch_one(pool)
        .await
        .expect("count")
}

/// Assert that exactly one source column is set and it matches `source_kind`.
fn assert_exactly_one(item: &BackpackItem) {
    let set = [
        item.source_tag_id.is_some(),
        item.source_playlist_id.is_some(),
        item.source_lens.is_some(),
        item.source_track_id.is_some(),
    ];
    assert_eq!(
        set.iter().filter(|b| **b).count(),
        1,
        "item {} must have exactly one source column set",
        item.id
    );
    let expected = match item.source_kind.as_str() {
        "tag" => item.source_tag_id.is_some(),
        "playlist" => item.source_playlist_id.is_some(),
        "lens" => item.source_lens.is_some(),
        "track" => item.source_track_id.is_some(),
        other => panic!("unexpected source_kind {other}"),
    };
    assert!(expected, "item {} kind/column mismatch", item.id);
}

#[test]
fn source_kind_round_trips_and_rejects_unknown() {
    for kind in [SourceKind::Tag, SourceKind::Playlist, SourceKind::Lens, SourceKind::Track] {
        let parsed: SourceKind = SourceKind::from_str(kind.as_str()).expect("parse");
        assert_eq!(parsed, kind);
    }
    assert_eq!(SourceKind::Tag.as_str(), "tag");
    assert_eq!(SourceKind::Playlist.as_str(), "playlist");
    assert_eq!(SourceKind::Lens.as_str(), "lens");
    assert_eq!(SourceKind::Track.as_str(), "track");
    assert!(SourceKind::from_str("bogus").is_err());
}

#[test]
fn source_ref_kind_matches_variant() {
    assert_eq!(SourceRef::Tag(1).kind(), SourceKind::Tag);
    assert_eq!(SourceRef::Playlist(1).kind(), SourceKind::Playlist);
    assert_eq!(SourceRef::Lens("newest".into()).kind(), SourceKind::Lens);
    assert_eq!(SourceRef::Track(1).kind(), SourceKind::Track);
}

#[tokio::test]
async fn add_and_list_round_trip_per_kind() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let tag = insert_tag(&pool, user, "dark").await;
    let pl = insert_playlist(&pool, user, "pl-1").await;
    let tr = insert_track(&pool, "t-1").await;

    let tag_item = backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    let pl_item = backpacks::add_source(&pool, bp, SourceRef::Playlist(pl)).await.unwrap();
    let lens_item = backpacks::add_source(&pool, bp, SourceRef::Lens("newest".into())).await.unwrap();
    let tr_item = backpacks::add_source(&pool, bp, SourceRef::Track(tr)).await.unwrap();

    let items = backpacks::list_sources(&pool, bp).await.unwrap();
    assert_eq!(items.len(), 4);
    for item in &items {
        assert_exactly_one(item);
    }
    let by_id = |id: i64| items.iter().find(|i| i.id == id).unwrap();
    assert_eq!(by_id(tag_item).source_kind, "tag");
    assert_eq!(by_id(tag_item).source_tag_id, Some(tag));
    assert_eq!(by_id(pl_item).source_kind, "playlist");
    assert_eq!(by_id(pl_item).source_playlist_id, Some(pl));
    assert_eq!(by_id(lens_item).source_kind, "lens");
    assert_eq!(by_id(lens_item).source_lens.as_deref(), Some("newest"));
    assert_eq!(by_id(tr_item).source_kind, "track");
    assert_eq!(by_id(tr_item).source_track_id, Some(tr));
}

#[tokio::test]
async fn exactly_one_source_column_per_row() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let tag = insert_tag(&pool, user, "dark").await;
    let pl = insert_playlist(&pool, user, "pl-1").await;
    let tr = insert_track(&pool, "t-1").await;

    let tag_item = backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    let pl_item = backpacks::add_source(&pool, bp, SourceRef::Playlist(pl)).await.unwrap();
    let lens_item = backpacks::add_source(&pool, bp, SourceRef::Lens("liked".into())).await.unwrap();
    let tr_item = backpacks::add_source(&pool, bp, SourceRef::Track(tr)).await.unwrap();

    for id in [tag_item, pl_item, lens_item, tr_item] {
        let (kind, tag_c, pl_c, lens_c, tr_c) = source_columns(&pool, id).await;
        let set = [tag_c.is_some(), pl_c.is_some(), lens_c.is_some(), tr_c.is_some()];
        assert_eq!(set.iter().filter(|b| **b).count(), 1, "row {id} kind={kind}");
        match kind.as_str() {
            "tag" => assert_eq!(tag_c, Some(tag)),
            "playlist" => assert_eq!(pl_c, Some(pl)),
            "lens" => assert_eq!(lens_c.as_deref(), Some("liked")),
            "track" => assert_eq!(tr_c, Some(tr)),
            other => panic!("unexpected kind {other}"),
        }
    }
}

#[tokio::test]
async fn lens_validation() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;

    let ok = backpacks::add_source(&pool, bp, SourceRef::Lens("newest".into())).await;
    assert!(ok.is_ok(), "known lens key must be accepted");
    assert_eq!(count_items(&pool, bp).await, 1);

    let bad = backpacks::add_source(&pool, bp, SourceRef::Lens("bogus".into())).await;
    let err = bad.expect_err("unknown lens key must error");
    assert!(
        err.to_string().contains("unknown lens key"),
        "error should name the bad key, got: {err}"
    );
    assert_eq!(count_items(&pool, bp).await, 1, "no row written for bad lens");
}

#[tokio::test]
async fn remove_source_returns_true_then_false() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let tag = insert_tag(&pool, user, "dark").await;
    let item = backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();

    assert!(backpacks::remove_source(&pool, item).await.unwrap(), "first delete removes");
    assert_eq!(count_items(&pool, bp).await, 0);
    assert!(!backpacks::remove_source(&pool, item).await.unwrap(), "second delete is a no-op");
}

#[tokio::test]
async fn position_preserves_append_order() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let t1 = insert_tag(&pool, user, "one").await;
    let t2 = insert_tag(&pool, user, "two").await;
    let t3 = insert_tag(&pool, user, "three").await;

    let a = backpacks::add_source(&pool, bp, SourceRef::Tag(t1)).await.unwrap();
    let b = backpacks::add_source(&pool, bp, SourceRef::Tag(t2)).await.unwrap();
    let c = backpacks::add_source(&pool, bp, SourceRef::Tag(t3)).await.unwrap();

    let items = backpacks::list_sources(&pool, bp).await.unwrap();
    let ids: Vec<i64> = items.iter().map(|i| i.id).collect();
    assert_eq!(ids, vec![a, b, c]);
    let positions: Vec<i64> = items.iter().map(|i| i.position).collect();
    assert_eq!(positions, vec![1, 2, 3]);
}

#[tokio::test]
async fn cascade_deletes_items() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let tag = insert_tag(&pool, user, "dark").await;
    let tr = insert_track(&pool, "t-1").await;

    backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Track(tr)).await.unwrap();
    assert_eq!(count_items(&pool, bp).await, 2);

    // Deleting the source tag cascades to its item.
    sqlx::query("DELETE FROM hub_tags WHERE id = ?1")
        .bind(tag)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(count_items(&pool, bp).await, 1, "tag delete cascades to item");

    // Deleting the backpack cascades to the remaining items.
    sqlx::query("DELETE FROM hub_backpacks WHERE id = ?1")
        .bind(bp)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(count_items(&pool, bp).await, 0, "backpack delete cascades to items");
}

#[tokio::test]
async fn writes_bump_backpack_updated_at() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let tag = insert_tag(&pool, user, "dark").await;
    let tr = insert_track(&pool, "t-1").await;

    assert_eq!(backpack_updated_at(&pool, bp).await, TS);

    let item = backpacks::add_source(&pool, bp, SourceRef::Tag(tag)).await.unwrap();
    let after_add = backpack_updated_at(&pool, bp).await;
    assert_ne!(after_add, TS, "add_source must bump updated_at");

    backpacks::add_source(&pool, bp, SourceRef::Track(tr)).await.unwrap();

    assert!(backpacks::remove_source(&pool, item).await.unwrap());
    let after_remove = backpack_updated_at(&pool, bp).await;
    assert!(after_remove > after_add, "remove_source must advance updated_at");

    let removed = backpacks::clear_sources(&pool, bp).await.unwrap();
    assert_eq!(removed, 1, "clear_sources reports rows deleted");
    let after_clear = backpack_updated_at(&pool, bp).await;
    assert!(after_clear > after_remove, "clear_sources must advance updated_at");
}

#[tokio::test]
async fn list_sources_empty_for_unknown_backpack() {
    let (_dir, pool) = setup().await;
    let items = backpacks::list_sources(&pool, 999).await.unwrap();
    assert!(items.is_empty());
}

// --- Edge cases beyond the plan's AC 1-7 -------------------------------------

/// All three `curate::LENSES` keys (not just one) must be accepted and persisted.
#[tokio::test]
async fn all_known_lens_keys_are_accepted() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;

    for key in ["newest", "liked", "unheard"] {
        let id = backpacks::add_source(&pool, bp, SourceRef::Lens(key.into()))
            .await
            .unwrap_or_else(|e| panic!("lens {key} must be accepted: {e}"));
        let (_kind, _tag, _pl, lens, _tr) = source_columns(&pool, id).await;
        assert_eq!(lens.as_deref(), Some(key));
    }
    assert_eq!(count_items(&pool, bp).await, 3);
}

/// A rejected lens key writes **no** row *and* leaves the backpack timestamp alone.
#[tokio::test]
async fn unknown_lens_writes_no_row_and_does_not_bump_updated_at() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;

    let before = backpack_updated_at(&pool, bp).await;
    assert!(
        backpacks::add_source(&pool, bp, SourceRef::Lens("bogus".into())).await.is_err()
    );
    assert_eq!(count_items(&pool, bp).await, 0);
    assert_eq!(
        backpack_updated_at(&pool, bp).await,
        before,
        "a failed add_source must not bump updated_at"
    );
}

/// Positions are numbered independently per backpack, and an append after a delete
/// takes `MAX+1` (no compaction / no position reuse).
#[tokio::test]
async fn position_is_per_backpack_and_monotonic_after_delete() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let a = insert_backpack(&pool, user, "A", 0).await;
    let b = insert_backpack(&pool, user, "B", 0).await;
    let t1 = insert_tag(&pool, user, "one").await;
    let t2 = insert_tag(&pool, user, "two").await;

    backpacks::add_source(&pool, a, SourceRef::Tag(t1)).await.unwrap();
    backpacks::add_source(&pool, b, SourceRef::Tag(t1)).await.unwrap();
    backpacks::add_source(&pool, b, SourceRef::Tag(t2)).await.unwrap();

    let pos_a: Vec<i64> = backpacks::list_sources(&pool, a)
        .await
        .unwrap()
        .iter()
        .map(|i| i.position)
        .collect();
    let pos_b: Vec<i64> = backpacks::list_sources(&pool, b)
        .await
        .unwrap()
        .iter()
        .map(|i| i.position)
        .collect();
    assert_eq!(pos_a, vec![1], "each backpack numbers its sources independently");
    assert_eq!(pos_b, vec![1, 2]);

    // Delete the *first* item: the surviving item keeps position 2 (no compaction)
    // and a fresh append takes MAX+1 = 3.
    let first = backpacks::list_sources(&pool, b).await.unwrap().remove(0);
    assert!(backpacks::remove_source(&pool, first.id).await.unwrap());
    backpacks::add_source(&pool, b, SourceRef::Tag(t1)).await.unwrap();

    let pos_b2: Vec<i64> = backpacks::list_sources(&pool, b)
        .await
        .unwrap()
        .iter()
        .map(|i| i.position)
        .collect();
    assert_eq!(pos_b2, vec![2, 3], "append uses MAX+1; existing positions are not compacted");
}

/// `clear_sources` returns 0 for an empty backpack and only touches its target.
#[tokio::test]
async fn clear_sources_empty_and_isolated_to_target_backpack() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let a = insert_backpack(&pool, user, "A", 0).await;
    let b = insert_backpack(&pool, user, "B", 0).await;
    let tag = insert_tag(&pool, user, "dark").await;

    assert_eq!(
        backpacks::clear_sources(&pool, a).await.unwrap(),
        0,
        "clearing an empty backpack deletes nothing"
    );

    backpacks::add_source(&pool, a, SourceRef::Tag(tag)).await.unwrap();
    backpacks::add_source(&pool, b, SourceRef::Tag(tag)).await.unwrap();

    assert_eq!(backpacks::clear_sources(&pool, a).await.unwrap(), 1);
    assert_eq!(count_items(&pool, a).await, 0);
    assert_eq!(count_items(&pool, b).await, 1, "sibling backpack untouched");
}

/// Deleting a referenced playlist or track cascades to its backpack item too.
#[tokio::test]
async fn cascade_deletes_playlist_and_track_items() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let pl = insert_playlist(&pool, user, "pl-9").await;
    let tr = insert_track(&pool, "t-9").await;

    backpacks::add_source(&pool, bp, SourceRef::Playlist(pl)).await.unwrap();
    backpacks::add_source(&pool, bp, SourceRef::Track(tr)).await.unwrap();
    assert_eq!(count_items(&pool, bp).await, 2);

    sqlx::query("DELETE FROM hub_playlists WHERE id = ?1")
        .bind(pl)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(count_items(&pool, bp).await, 1, "playlist delete cascades to item");

    sqlx::query("DELETE FROM hub_tracks WHERE id = ?1")
        .bind(tr)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(count_items(&pool, bp).await, 0, "track delete cascades to item");
}

/// Removing a non-existent item returns false and must not bump any timestamp.
#[tokio::test]
async fn remove_unknown_item_is_false_without_bump() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let before = backpack_updated_at(&pool, bp).await;

    assert!(!backpacks::remove_source(&pool, 424242).await.unwrap());
    assert_eq!(
        backpack_updated_at(&pool, bp).await,
        before,
        "a no-op remove must not bump updated_at"
    );
}

/// Foreign keys are enforced: unknown tag or backpack ids fail and write nothing.
#[tokio::test]
async fn add_source_with_unknown_reference_fails_and_writes_nothing() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let tr = insert_track(&pool, "t-1").await;

    assert!(
        backpacks::add_source(&pool, bp, SourceRef::Tag(9999)).await.is_err(),
        "unknown tag id must violate the FK"
    );
    assert_eq!(count_items(&pool, bp).await, 0);

    assert!(
        backpacks::add_source(&pool, 8888, SourceRef::Track(tr)).await.is_err(),
        "unknown backpack id must violate the FK"
    );
}

/// Defence in depth: the DB CHECK still rejects a row with two source columns.
#[tokio::test]
async fn db_check_rejects_two_source_columns() {
    let (_dir, pool) = setup().await;
    let user = insert_user(&pool, "alice").await;
    let bp = insert_backpack(&pool, user, "B", 0).await;
    let tag = insert_tag(&pool, user, "dark").await;
    let tr = insert_track(&pool, "t-1").await;

    let res = sqlx::query(
        "INSERT INTO hub_backpack_items
            (backpack_id, source_kind, source_tag_id, source_track_id, position, added_at)
         VALUES (?1, 'tag', ?2, ?3, 1, ?4)",
    )
    .bind(bp)
    .bind(tag)
    .bind(tr)
    .bind(TS)
    .execute(&pool)
    .await;
    assert!(res.is_err(), "the exactly-one-source CHECK must reject two columns");
    assert_eq!(count_items(&pool, bp).await, 0);
}
