//! Migration integrity test.
//!
//! Creates a fresh in-memory DB, runs all 16 migrations, and asserts the
//! expected tables and views exist. This is the canary — if a migration breaks
//! the chain, this test catches it before any domain test runs.

mod common;

use sqlx::Row;

#[tokio::test]
async fn all_migrations_run_cleanly() {
    let pool = common::create_test_db().await;

    // Verify expected tables exist
    let tables: Vec<String> =
        sqlx::query("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();

    let expected_tables = [
        "deemix_downloads",
        "file_locations",
        "file_resolved_tags",
        "files",
        "folders",
        "playlist_subscriptions",
        "service_config",
        "service_playlist_tracks",
        "service_playlists",
        "service_tracks",
        "tag_categories",
        "tag_embeddings",
        "tag_energy_levels",
        "tag_parents",
        "tag_similarities",
        "tags",
        "track_resolved_tags",
    ];

    for table in &expected_tables {
        assert!(
            tables.contains(&table.to_string()),
            "Expected table '{}' not found after migrations. Existing tables: {:?}",
            table,
            tables
        );
    }

    // Verify expected views exist
    let views: Vec<String> =
        sqlx::query("SELECT name FROM sqlite_master WHERE type='view' ORDER BY name")
            .fetch_all(&pool)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.get::<String, _>("name"))
            .collect();

    let expected_views = [
        "unified_tracks",
        "v_file_track_link",
        "v_tag_playlist",
        "v_file_tags",
        "v_subscriptions",
        "v_tag_categories",
        "v_tags_with_categories",
        "v_resolved_tags",
        "v_file_resolved_tags",
        "v_playlist_tag_category",
        "v_tag_file_counts",
        "v_track_tags",
    ];

    for view in &expected_views {
        assert!(
            views.contains(&view.to_string()),
            "Expected view '{}' not found after migrations. Existing views: {:?}",
            view,
            views
        );
    }

    // Verify seed data from migration 001 exists
    let category_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tag_categories WHERE is_default = 1")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        category_count > 0,
        "No default tag categories found (migration 001 seed data missing)"
    );
}
