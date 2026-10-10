//! Integration tests for the Traktor ingest (issues #211–#213).

mod common;

use sqlx::Row;

const NML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<NML VERSION="19">
 <COLLECTION ENTRIES="3">
  <ENTRY>
   <LOCATION DIR="/Music/" FILE="a.mp3" VOLUME="Mac"/>
   <TITLE>Alpha</TITLE>
   <ARTIST>Artist A</ARTIST>
   <INFO PLAYCOUNT="7" LAST_PLAY="2024/3/1" RATING="255"/>
  </ENTRY>
  <ENTRY>
   <LOCATION DIR="/Music/" FILE="b.mp3" VOLUME="Mac"/>
   <TITLE>Beta</TITLE>
   <ARTIST>Artist B</ARTIST>
   <INFO PLAYCOUNT="0" RATING="0"/>
  </ENTRY>
  <ENTRY>
   <LOCATION DIR="/Music/" FILE="c.mp3" VOLUME="Mac"/>
   <TITLE>Gamma</TITLE>
   <ARTIST>Artist C</ARTIST>
   <INFO PLAYCOUNT="2" RATING="153"/>
  </ENTRY>
 </COLLECTION>
 <PLAYLISTS>
  <NODE TYPE="FOLDER" NAME="Root">
   <SUBNODES COUNT="2">
    <NODE TYPE="PLAYLIST" NAME="Peak Time">
     <PLAYLIST ENTRIES="2" TYPE="LIST">
      <ENTRY><PRIMARYKEY TYPE="TRACK" KEY="a.mp3"/></ENTRY>
      <ENTRY><PRIMARYKEY TYPE="TRACK" KEY="b.mp3"/></ENTRY>
     </PLAYLIST>
    </NODE>
    <NODE TYPE="PLAYLIST" NAME="History 2024">
     <PLAYLIST ENTRIES="1" TYPE="LIST">
      <ENTRY><PRIMARYKEY TYPE="TRACK" KEY="a.mp3"/></ENTRY>
     </PLAYLIST>
    </NODE>
   </SUBNODES>
  </NODE>
 </PLAYLISTS>
</NML>
"#;

async fn seed_tracks(pool: &sqlx::SqlitePool) {
    sqlx::query(
        "INSERT INTO hub_tracks (id, service, service_track_id, title, artists) VALUES
            (9001, 'local', 'tk1', 'Alpha', 'Artist A'),
            (9002, 'local', 'tk2', 'Beta',  'Artist B')",
    )
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn traktor_import_stores_meta_and_playlists() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    seed_tracks(&app.pool).await;

    let stats = mmm_hub::traktor::import_str(&app.pool, alice, NML)
        .await
        .unwrap();
    assert_eq!(stats.entries, 3, "three collection entries");
    assert_eq!(stats.matched, 2, "Alpha + Beta matched, Gamma not");
    // Collection + Peak Time + History 2024.
    assert_eq!(stats.playlists, 3);

    // Per-track meta.
    let (pc, lp, rating) = {
        let r = sqlx::query(
            "SELECT play_count, last_played, rating FROM hub_traktor_tracks
              WHERE user_id = ?1 AND track_id = 9001",
        )
        .bind(alice)
        .fetch_one(&app.pool)
        .await
        .unwrap();
        (
            r.get::<i64, _>("play_count"),
            r.get::<Option<String>, _>("last_played"),
            r.get::<Option<i64>, _>("rating"),
        )
    };
    assert_eq!(pc, 7);
    assert_eq!(lp.as_deref(), Some("2024/3/1"));
    assert_eq!(rating, Some(5), "255 -> 5 stars");

    let rating_b: Option<i64> = sqlx::query_scalar(
        "SELECT rating FROM hub_traktor_tracks WHERE user_id = ?1 AND track_id = 9002",
    )
    .bind(alice)
    .fetch_one(&app.pool)
    .await
    .unwrap();
    assert_eq!(rating_b, None, "rating 0 -> none");

    // Node types.
    let types: Vec<(String, String)> = sqlx::query_as(
        "SELECT name, node_type FROM hub_traktor_playlists WHERE user_id = ?1 ORDER BY node_type, name",
    )
    .bind(alice)
    .fetch_all(&app.pool)
    .await
    .unwrap();
    assert!(types.contains(&("Collection".into(), "collection".into())));
    assert!(types.contains(&("Peak Time".into(), "playlist".into())));
    assert!(types.contains(&("History 2024".into(), "session".into())));
}

#[tokio::test]
async fn traktor_view_aggregates_signals() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    seed_tracks(&app.pool).await;
    mmm_hub::traktor::import_str(&app.pool, alice, NML)
        .await
        .unwrap();

    let r = sqlx::query("SELECT * FROM hub_v_track_traktor WHERE track_id = 9001")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(r.get::<i64, _>("play_count"), 7);
    assert_eq!(r.get::<Option<i64>, _>("rating"), Some(5));
    // Collection + Peak Time (not session).
    assert_eq!(r.get::<i64, _>("playlist_occurrence"), 2);
    // History 2024.
    assert_eq!(r.get::<i64, _>("session_occurrence"), 1);

    // Beta is in Collection + Peak Time only.
    let r = sqlx::query("SELECT * FROM hub_v_track_traktor WHERE track_id = 9002")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(r.get::<i64, _>("playlist_occurrence"), 2);
    assert_eq!(r.get::<i64, _>("session_occurrence"), 0);
}

#[tokio::test]
async fn traktor_import_is_idempotent() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    seed_tracks(&app.pool).await;

    mmm_hub::traktor::import_str(&app.pool, alice, NML)
        .await
        .unwrap();
    mmm_hub::traktor::import_str(&app.pool, alice, NML)
        .await
        .unwrap();

    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_traktor_playlists WHERE user_id = ?1")
            .bind(alice)
            .fetch_one(&app.pool)
            .await
            .unwrap();
    assert_eq!(n, 3, "re-import replaces, does not duplicate");

    let m: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_traktor_tracks WHERE user_id = ?1")
        .bind(alice)
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(m, 2);
}
