//! Integration tests for tag-overlap similarity v2 (#221/#222).

mod common;

use music_hub::tagtypes::{TagType, set as set_type};

async fn resolved(pool: &sqlx::SqlitePool, track: i64, tag: i64) {
    sqlx::query("INSERT OR IGNORE INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(track)
        .bind(tag)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn cross_type_matches_score_higher_than_same_type() {
    use music_hub::tags;
    let app = common::spawn().await;
    let me = app.seed.alice;

    let t1 = tags::ensure_tag(&app.pool, me, "Dark").await.unwrap();
    let t2 = tags::ensure_tag(&app.pool, me, "Warehouse").await.unwrap();
    let t3 = tags::ensure_tag(&app.pool, me, "Melancholisch")
        .await
        .unwrap();
    set_type(&app.pool, me, t1, Some(TagType::Mood))
        .await
        .unwrap();
    set_type(&app.pool, me, t2, Some(TagType::Vibe))
        .await
        .unwrap();
    set_type(&app.pool, me, t3, Some(TagType::Mood))
        .await
        .unwrap();

    // a,b share {Dark(Mood), Warehouse(Vibe)} -> cross-type.
    // c,d share {Dark(Mood), Melancholisch(Mood)} -> same type.
    for (track, tags_) in [
        (app.seed.t_all, vec![t1, t2]),
        (app.seed.t_two, vec![t1, t2]),
        (app.seed.t_pl, vec![t1, t3]),
        (app.seed.t_alice, vec![t1, t3]),
    ] {
        for t in tags_ {
            resolved(&app.pool, track, t).await;
        }
    }

    let e = music_hub::settings::engine(&app.pool).await;
    let cross =
        music_hub::similarity::tag_similarity(&app.pool, app.seed.t_all, app.seed.t_two, &e).await;
    let same =
        music_hub::similarity::tag_similarity(&app.pool, app.seed.t_pl, app.seed.t_alice, &e).await;
    assert!(
        cross > same,
        "cross-type {cross} should beat same-type {same}"
    );
    assert!(cross > 0.0);
}

#[tokio::test]
async fn rumpelkiste_helpers_find_the_tags() {
    use music_hub::tags;
    let app = common::spawn().await;
    let me = app.seed.alice;
    let t = tags::ensure_tag(&app.pool, me, "Sketches").await.unwrap();
    set_type(&app.pool, me, t, Some(TagType::Rumpelkiste))
        .await
        .unwrap();

    // Link a playlist as a source of that tag.
    sqlx::query(
        "INSERT INTO hub_tag_sources (tag_id, playlist_id, user_id, service)
         VALUES (?1, ?2, ?3, 'spotify')",
    )
    .bind(t)
    .bind(app.seed.pl_alice)
    .bind(me)
    .execute(&app.pool)
    .await
    .unwrap();

    let tags_set = music_hub::similarity::rumpelkiste_tag_ids(&app.pool).await;
    assert!(tags_set.contains(&t));
    let pls = music_hub::similarity::rumpelkiste_playlist_ids(&app.pool).await;
    assert!(pls.contains(&app.seed.pl_alice));
}
