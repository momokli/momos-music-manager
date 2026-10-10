//! Integration tests for tag-overlap similarity v2 (#221/#222).

mod common;

async fn group(pool: &sqlx::SqlitePool, me: i64, name: &str, role: &str) -> i64 {
    let g = mmm_hub::tags::create_group(pool, me, name, "")
        .await
        .unwrap();
    if !role.is_empty() {
        // set_semantic_role via kind/role: role-only marker.
        mmm_hub::tags::set_group_kind_role(pool, me, g, "class", role)
            .await
            .unwrap();
    }
    g
}

async fn resolved(pool: &sqlx::SqlitePool, track: i64, tag: i64) {
    sqlx::query("INSERT OR IGNORE INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(track)
        .bind(tag)
        .execute(pool)
        .await
        .unwrap();
}

#[tokio::test]
async fn cross_group_matches_score_higher_than_same_group() {
    use mmm_hub::tags;
    let app = common::spawn().await;
    let me = app.seed.alice;
    let g1 = group(&app.pool, me, "Mood", "").await;
    let g2 = group(&app.pool, me, "Vibe", "").await;

    let t1 = tags::ensure_tag(&app.pool, me, "Dark").await.unwrap();
    let t2 = tags::ensure_tag(&app.pool, me, "Warehouse").await.unwrap();
    let t3 = tags::ensure_tag(&app.pool, me, "Melancholisch")
        .await
        .unwrap();
    tags::add_tag_to_group(&app.pool, me, t1, g1).await.unwrap();
    tags::add_tag_to_group(&app.pool, me, t2, g2).await.unwrap();
    tags::add_tag_to_group(&app.pool, me, t3, g1).await.unwrap();

    // a,b share {Dark(Mood), Warehouse(Vibe)} -> cross-group.
    // c,d share {Dark(Mood), Melancholisch(Mood)} -> same group.
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

    let e = mmm_hub::settings::engine(&app.pool).await;
    let cross =
        mmm_hub::similarity::tag_similarity(&app.pool, app.seed.t_all, app.seed.t_two, &e).await;
    let same =
        mmm_hub::similarity::tag_similarity(&app.pool, app.seed.t_pl, app.seed.t_alice, &e).await;
    assert!(
        cross > same,
        "cross-group {cross} should beat same-group {same}"
    );
    assert!(cross > 0.0);
}

#[tokio::test]
async fn rumpelkiste_helpers_find_the_group() {
    use mmm_hub::tags;
    let app = common::spawn().await;
    let me = app.seed.alice;
    let rumpel = group(&app.pool, me, "Rumpelkiste", "rumpelkiste").await;
    let t = tags::ensure_tag(&app.pool, me, "Sketches").await.unwrap();
    tags::add_tag_to_group(&app.pool, me, t, rumpel)
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

    let tags_set = mmm_hub::similarity::rumpelkiste_tag_ids(&app.pool).await;
    assert!(tags_set.contains(&t));
    let pls = mmm_hub::similarity::rumpelkiste_playlist_ids(&app.pool).await;
    assert!(pls.contains(&app.seed.pl_alice));
}
