//! Integration tests for the tag recommendation engine (`src/recommend.rs`).
//!
//! Focus: the UI needs *context* for every suggestion — the tag's type and a
//! human-readable reason — and sort types (Setlist/Rumpelkiste) must never be
//! suggested.

mod common;

use mmm_hub::tags;
use mmm_hub::tagtypes::{TagType, set as set_type};

async fn insert_track(
    pool: &sqlx::SqlitePool,
    sid: &str,
    title: &str,
    artists: &str,
    album: &str,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, title, artists, album, first_seen_at)
         VALUES ('spotify', ?1, ?2, ?3, ?4, '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(sid)
    .bind(title)
    .bind(artists)
    .bind(album)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Materialize a track→tag link directly into `hub_track_resolved_tags`.
async fn link(pool: &sqlx::SqlitePool, track_id: i64, tag_id: i64) {
    sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(track_id)
        .bind(tag_id)
        .execute(pool)
        .await
        .unwrap();
}

/// A candidate tag carries its type label, and Setlist/Rumpelkiste tags are
/// never suggested even when a co-tagged track would otherwise surface them.
#[tokio::test]
async fn recommendations_carry_type_and_exclude_sort_types() {
    let app = common::spawn().await;
    let alice = app.seed.alice;

    let shared = tags::ensure_tag(&app.pool, alice, "shared-tag")
        .await
        .unwrap();
    let dark = tags::ensure_tag(&app.pool, alice, "Dark").await.unwrap();
    let peak = tags::ensure_tag(&app.pool, alice, "Peak").await.unwrap();
    set_type(&app.pool, alice, dark, Some(TagType::Mood))
        .await
        .unwrap();
    set_type(&app.pool, alice, peak, Some(TagType::Setlist))
        .await
        .unwrap();

    // Distinctive artist/album so unrelated fixtures can't add signals.
    let seed_t = insert_track(
        &app.pool,
        "rec_seed",
        "Seed",
        "RecTestArtist",
        "RecTestAlbum",
    )
    .await;
    let other = insert_track(
        &app.pool,
        "rec_other",
        "Other",
        "RecTestArtist",
        "RecTestAlbum",
    )
    .await;

    link(&app.pool, seed_t, shared).await;
    link(&app.pool, other, shared).await; // shared tag -> co-tagged candidate
    link(&app.pool, other, dark).await;
    link(&app.pool, other, peak).await; // must be filtered out

    let recs = mmm_hub::recommend::recommend_tags(&app.pool, seed_t, 20).await;

    let names: Vec<&str> = recs.iter().map(|r| r.name.as_str()).collect();
    assert!(
        names.contains(&"Dark"),
        "Dark should be recommended: {names:?}"
    );
    assert!(
        !names.contains(&"Peak"),
        "Setlist tag Peak must never be recommended: {names:?}"
    );
    assert!(
        !names.contains(&"shared-tag"),
        "a tag already on the seed track must not be recommended: {names:?}"
    );

    let dark_rec = recs.iter().find(|r| r.name == "Dark").unwrap();
    assert_eq!(
        dark_rec.type_label, "Mood",
        "type context must be populated"
    );
    assert!(
        !dark_rec.why().is_empty(),
        "every recommendation needs an explanation"
    );
}
