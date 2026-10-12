//! Analysis → music-api metadata: the hub reads its own BPM/key (from
//! `hub_track_analysis`) to push to music-api's `POST /metadata`.

mod common;

#[tokio::test]
async fn analysis_metadata_reads_isrc_bpm_key() {
    let app = common::spawn().await;

    // Attach an ISRC + an analysis row (Traktor BPM/key) to a seeded track.
    sqlx::query("UPDATE hub_tracks SET isrc = 'USAAA0000009' WHERE id = ?1")
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO hub_track_analysis (track_id, bpm, key, camelot, source, analyzed_at)
         VALUES (?1, 128.0, '11m', '8A', 'traktor', '2026-01-01T00:00:00+00:00')",
    )
    .bind(app.seed.t_all)
    .execute(&app.pool)
    .await
    .unwrap();

    let items = music_hub::music_api::analysis_metadata(&app.pool, 10)
        .await
        .unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].isrc, "USAAA0000009");
    assert_eq!(items[0].bpm, Some(128.0));
    assert_eq!(items[0].key.as_deref(), Some("11m"));
    assert_eq!(items[0].camelot.as_deref(), Some("8A"));
    assert_eq!(items[0].source.as_deref(), Some("traktor"));
}

#[tokio::test]
async fn analysis_metadata_empty_without_rows() {
    let app = common::spawn().await;
    let items = music_hub::music_api::analysis_metadata(&app.pool, 10)
        .await
        .unwrap();
    assert!(items.is_empty());
}
