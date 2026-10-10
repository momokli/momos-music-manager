//! Integration tests for archive (keep_on_remove) + bi-way tag→playlist sync.

mod common;

#[tokio::test]
async fn archive_keeps_a_tag_link_after_rebuild() {
    use mmm_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let tag = tags::ensure_tag(&app.pool, alice, "Kept").await.unwrap();

    tags::archive_tag(&app.pool, tag, app.seed.t_all, None)
        .await
        .unwrap();

    let resolved: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_track_resolved_tags WHERE track_id = ?1 AND tag_id = ?2",
    )
    .bind(app.seed.t_all)
    .bind(tag)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(resolved, 1, "archived link survives rebuild");

    // A second rebuild must not duplicate it.
    tags::rebuild(&app.pool).await.unwrap();
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_track_resolved_tags WHERE track_id = ?1 AND tag_id = ?2",
    )
    .bind(app.seed.t_all)
    .bind(tag)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn tagging_syncs_to_linked_playlist() {
    use mmm_hub::tags;
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let pl = app.seed.pl_alice;
    let track = app.seed.t_all;

    let tag = tags::ensure_tag(&app.pool, alice, "Groovy").await.unwrap();
    // Link alice's playlist as a source of the tag and opt into sync.
    sqlx::query(
        "INSERT INTO hub_tag_sources (tag_id, playlist_id, user_id, service)
         VALUES (?1, ?2, ?3, 'spotify')",
    )
    .bind(tag)
    .bind(pl)
    .bind(alice)
    .execute(&app.pool)
    .await
    .unwrap();
    tags::set_tag_sync(&app.pool, alice, tag, true)
        .await
        .unwrap();

    // Before tagging, the track is not in the playlist.
    let before: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2",
    )
    .bind(pl)
    .bind(track)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    // (The seed may already contain it; record the baseline.)
    tags::tag_track(&app.pool, alice, track, tag).await.unwrap();

    let after: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_playlist_tracks WHERE playlist_id = ?1 AND track_id = ?2",
    )
    .bind(pl)
    .bind(track)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert!(
        after >= 1,
        "tagged track synced into the playlist (before={before})"
    );

    // Without sync, nothing new is pushed.
    let tag2 = tags::ensure_tag(&app.pool, alice, "NoSync").await.unwrap();
    sqlx::query(
        "INSERT INTO hub_tag_sources (tag_id, playlist_id, user_id, service)
         VALUES (?1, ?2, ?3, 'spotify')",
    )
    .bind(tag2)
    .bind(app.seed.pl_bob)
    .bind(alice)
    .execute(&app.pool)
    .await
    .unwrap();
    let pushed = tags::sync_tag_to_playlists(&app.pool, alice, tag2, track)
        .await
        .unwrap();
    assert!(pushed.is_empty(), "no sync without the opt-in flag");
}
