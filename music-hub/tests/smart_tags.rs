//! Integration tests for smart tags / dynamic lenses (#298).
//!
//! Seeded world: alice owns a `GENRE` base tag (`Techno Lens`) fed by a playlist
//! (`pl-lens`) and materialized via `tags::rebuild`, covering three tracks with
//! distinct BPM/energy. bob is an unrelated user (ownership boundary). Smart
//! tags over those tracks are resolved at runtime and must never be materialized.

mod common;

use music_hub::tags::{self, SmartFilter};

struct World {
    app: common::TestApp,
    alice: i64,
    bob: i64,
    alice_cookie: String,
    bob_cookie: String,
    /// alice's base `GENRE` tag feeding the smart filters.
    genre: i64,
    /// The playlist feeding `genre` (used to add extra member tracks).
    pl: i64,
    t_low: i64,
    t_mid: i64,
    t_high: i64,
}

async fn insert_track(pool: &sqlx::SqlitePool, sid: &str, title: &str, artists: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, title, artists, first_seen_at)
         VALUES ('spotify', ?1, ?2, ?3, '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(sid)
    .bind(title)
    .bind(artists)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_audio(pool: &sqlx::SqlitePool, track_id: i64, bpm: f64, energy: f64) {
    sqlx::query("INSERT INTO hub_track_audio (track_id, bpm, energy) VALUES (?1, ?2, ?3)")
        .bind(track_id)
        .bind(bpm)
        .bind(energy)
        .execute(pool)
        .await
        .unwrap();
}

async fn insert_tag(pool: &sqlx::SqlitePool, owner: i64, slug: &str, name: &str, ty: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, type, created_at)
         VALUES (?1, ?2, ?3, ?4, '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(owner)
    .bind(slug)
    .bind(name)
    .bind(ty)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_playlist(pool: &sqlx::SqlitePool, owner: i64, pid: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_playlists (user_id, service, playlist_id, name, fetched_at)
         VALUES (?1, 'spotify', ?2, ?3, '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(owner)
    .bind(pid)
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_membership(pool: &sqlx::SqlitePool, playlist_id: i64, track_id: i64, pos: i64) {
    sqlx::query("INSERT INTO hub_playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, ?3)")
        .bind(playlist_id)
        .bind(track_id)
        .bind(pos)
        .execute(pool)
        .await
        .unwrap();
}

async fn link_source(pool: &sqlx::SqlitePool, tag_id: i64, playlist_id: i64, owner: i64) {
    sqlx::query(
        "INSERT INTO hub_tag_sources (tag_id, playlist_id, user_id, service, created_at)
         VALUES (?1, ?2, ?3, 'spotify', '2026-01-01T00:00:00+00:00')",
    )
    .bind(tag_id)
    .bind(playlist_id)
    .bind(owner)
    .execute(pool)
    .await
    .unwrap();
}

async fn resolved_count(pool: &sqlx::SqlitePool, tag_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM hub_track_resolved_tags WHERE tag_id = ?1")
        .bind(tag_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// alice's smart tag with the given filter; returns its tag id.
async fn make_smart(w: &World, name: &str, filter: &SmartFilter) -> i64 {
    tags::create_smart_tag(&w.app.pool, w.alice, name, None, filter)
        .await
        .unwrap()
}

fn base_filter(w: &World) -> SmartFilter {
    SmartFilter {
        tag_ids: vec![w.genre],
        ..Default::default()
    }
}

async fn seed_world() -> World {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let bob = app.seed.bob;
    let alice_cookie = app.session_cookie(alice).await;
    let bob_cookie = app.session_cookie(bob).await;

    let genre = insert_tag(&app.pool, alice, "lens-techno", "Techno Lens", "GENRE").await;
    let t_low = insert_track(&app.pool, "lens-low", "Low Beat", "Aaa").await;
    let t_mid = insert_track(&app.pool, "lens-mid", "Mid Beat", "Bbb").await;
    let t_high = insert_track(&app.pool, "lens-high", "High Beat", "Ccc").await;
    insert_audio(&app.pool, t_low, 100.0, 2.0).await;
    insert_audio(&app.pool, t_mid, 128.0, 5.0).await;
    insert_audio(&app.pool, t_high, 140.0, 9.0).await;

    // Feed the base tag from a playlist and materialize it, so the base set is
    // rebuild-safe (survives `tags::rebuild`).
    let pl = insert_playlist(&app.pool, alice, "pl-lens", "Lens Source").await;
    for (pos, tid) in [t_low, t_mid, t_high].into_iter().enumerate() {
        insert_membership(&app.pool, pl, tid, pos as i64).await;
    }
    link_source(&app.pool, genre, pl, alice).await;
    tags::rebuild(&app.pool).await.unwrap();

    World {
        app,
        alice,
        bob,
        alice_cookie,
        bob_cookie,
        genre,
        pl,
        t_low,
        t_mid,
        t_high,
    }
}

// ── 1. login gate ──────────────────────────────────────────────────────────

#[tokio::test]
async fn smart_tag_create_requires_login() {
    let w = seed_world().await;
    let resp = w
        .app
        .client()
        .post(w.app.url("/tags/smart/create"))
        .form(&[
            ("name", "No Login".to_string()),
            ("tag_ids", w.genre.to_string()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    assert_eq!(resp.headers()["location"], "/login");
}

// ── 2. create + view ───────────────────────────────────────────────────────

#[tokio::test]
async fn create_and_view_smart_tag() {
    let w = seed_world().await;
    let resp = w
        .app
        .client()
        .post(w.app.url("/tags/smart/create"))
        .header("Cookie", &w.alice_cookie)
        .form(&[
            ("name", "All Techno".to_string()),
            ("tag_ids", w.genre.to_string()),
            ("sort", "title".to_string()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    let loc = resp.headers()["location"].to_str().unwrap().to_string();
    assert!(loc.starts_with("/tag/"), "unexpected redirect: {loc}");
    let id: i64 = loc.rsplit('/').next().unwrap().parse().unwrap();

    let body = w
        .app
        .client()
        .get(w.app.url(&format!("/tag/{id}")))
        .header("Cookie", &w.alice_cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("Low Beat"), "missing Low Beat");
    assert!(body.contains("Mid Beat"), "missing Mid Beat");
    assert!(body.contains("High Beat"), "missing High Beat");
    assert!(body.contains("dynamisch"), "missing dynamic badge");
}

// ── 3. bpm filter ──────────────────────────────────────────────────────────

#[tokio::test]
async fn bpm_filter_narrows_result() {
    let w = seed_world().await;
    let mut filter = base_filter(&w);
    filter.bpm_min = Some(120.0);
    filter.bpm_max = Some(135.0);
    let id = make_smart(&w, "Mid Tempo", &filter).await;

    // Baseline: the base tag resolves all three tracks.
    let base = make_smart(&w, "All Tempos", &base_filter(&w)).await;
    let mut all = tags::resolve_smart_tag(&w.app.pool, base).await.unwrap();
    all.sort_unstable();
    let mut expect = vec![w.t_low, w.t_mid, w.t_high];
    expect.sort_unstable();
    assert_eq!(all, expect);

    let ids = tags::resolve_smart_tag(&w.app.pool, id).await.unwrap();
    assert_eq!(ids, vec![w.t_mid], "only the 128 bpm track is in range");

    // A missing audio row is filtered out by a set lower bound: the track is in
    // the base tag but has no `hub_track_audio` row.
    let orphan = insert_track(&w.app.pool, "lens-orphan", "No Audio", "Ddd").await;
    insert_membership(&w.app.pool, w.pl, orphan, 9).await;
    tags::rebuild(&w.app.pool).await.unwrap();
    assert_eq!(
        resolved_count(&w.app.pool, w.genre).await,
        4,
        "orphan joined the base tag"
    );
    let ids = tags::resolve_smart_tag(&w.app.pool, id).await.unwrap();
    assert_eq!(ids, vec![w.t_mid], "track without audio dropped by bpm bound");
}

// ── 4. energy filter ───────────────────────────────────────────────────────

#[tokio::test]
async fn energy_filter_narrows_result() {
    let w = seed_world().await;
    let mut filter = base_filter(&w);
    filter.energy_min = Some(6.0);
    let id = make_smart(&w, "High Energy", &filter).await;

    let ids = tags::resolve_smart_tag(&w.app.pool, id).await.unwrap();
    assert_eq!(ids, vec![w.t_high], "only the energy-9 track qualifies");
}

// ── 5. edit updates resolution ─────────────────────────────────────────────

#[tokio::test]
async fn edit_updates_resolution() {
    let w = seed_world().await;
    let id = make_smart(&w, "Editable", &base_filter(&w)).await;
    assert_eq!(tags::resolve_smart_tag(&w.app.pool, id).await.unwrap().len(), 3);

    let resp = w
        .app
        .client()
        .post(w.app.url(&format!("/tag/{id}/smart")))
        .header("Cookie", &w.alice_cookie)
        .form(&[
            ("tag_ids", w.genre.to_string()),
            ("bpm_min", "120".to_string()),
            ("bpm_max", "135".to_string()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);

    let ids = tags::resolve_smart_tag(&w.app.pool, id).await.unwrap();
    assert_eq!(ids, vec![w.t_mid], "edited filter narrows to one track");

    let body = w
        .app
        .client()
        .get(w.app.url(&format!("/tag/{id}")))
        .header("Cookie", &w.alice_cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("Mid Beat"));
    assert!(!body.contains("Low Beat"), "narrowed edition hides Low Beat");
}

// ── 6. never materialized ──────────────────────────────────────────────────

#[tokio::test]
async fn smart_tag_not_materialized() {
    let w = seed_world().await;
    let id = make_smart(&w, "Never Materialized", &base_filter(&w)).await;

    let summary = tags::rebuild(&w.app.pool).await.unwrap();
    assert!(summary.resolved >= 3, "base tag materialized");
    assert_eq!(
        resolved_count(&w.app.pool, id).await,
        0,
        "smart tag must have no materialized rows"
    );
    let ids = tags::resolve_smart_tag(&w.app.pool, id).await.unwrap();
    assert_eq!(ids.len(), 3, "runtime resolution still yields the set");
}

// ── 7. dynamic badge in the list ───────────────────────────────────────────

#[tokio::test]
async fn dynamic_badge_in_list() {
    let w = seed_world().await;
    let _ = make_smart(&w, "Badged Lens", &base_filter(&w)).await;

    let body = w
        .app
        .client()
        .get(w.app.url("/tags"))
        .header("Cookie", &w.alice_cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(body.contains("Badged Lens"), "smart tag listed");
    assert!(body.contains("dynamisch"), "dynamic badge rendered");
}

// ── 8. non-owner cannot edit ───────────────────────────────────────────────

#[tokio::test]
async fn update_denied_for_non_owner() {
    let w = seed_world().await;
    let id = make_smart(&w, "Alice Only Lens", &base_filter(&w)).await;
    let before = tags::smart_filter(&w.app.pool, id).await.unwrap();

    let mut tighter = base_filter(&w);
    tighter.bpm_min = Some(900.0);
    let resp = w
        .app
        .client()
        .post(w.app.url(&format!("/tag/{id}/smart")))
        .header("Cookie", &w.bob_cookie)
        .form(&[
            ("tag_ids", w.genre.to_string()),
            ("bpm_min", "900".to_string()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);

    let after = tags::smart_filter(&w.app.pool, id).await.unwrap();
    assert_eq!(before.bpm_min, after.bpm_min, "bob may not change the filter");
    assert_eq!(after.bpm_min, None);
    let _ = w.bob;
}

// ── 9. empty filter rejected ───────────────────────────────────────────────

#[tokio::test]
async fn empty_filter_rejected() {
    let w = seed_world().await;
    let resp = w
        .app
        .client()
        .post(w.app.url("/tags/smart/create"))
        .header("Cookie", &w.alice_cookie)
        .form(&[("name", "Nothing".to_string())])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 303);
    assert_eq!(resp.headers()["location"], "/tags");

    let smart = tags::list_user_smart_tags(&w.app.pool, w.alice).await;
    assert!(
        smart.iter().all(|(_, n)| n != "Nothing"),
        "no smart tag created without a criterion"
    );
}

// ── 10. a same-named NORMAL tag is not silently converted ──────────────────

#[tokio::test]
async fn create_does_not_convert_existing_normal_tag() {
    let w = seed_world().await;
    // A normal tag created the way the UI does (slug = normalize_name(name)).
    let normal = tags::ensure_tag(&w.app.pool, w.alice, "Plain Normal").await.unwrap();
    let res =
        tags::create_smart_tag(&w.app.pool, w.alice, "Plain Normal", None, &base_filter(&w)).await;
    assert!(res.is_err(), "same-named normal tag must not be converted");

    // The normal tag stays non-smart.
    assert!(
        tags::smart_filter(&w.app.pool, normal).await.is_none(),
        "normal tag must not gain a smart filter"
    );
}

// ── 11. foreign tag ids are rejected (cross-owner guard) ───────────────────

#[tokio::test]
async fn create_rejects_foreign_tag_ids() {
    let w = seed_world().await;
    // A tag owned by bob (a different owner than the caller alice).
    let bob_tag = insert_tag(&w.app.pool, w.bob, "bob-secret", "Bob Secret", "GENRE").await;
    let filter = SmartFilter {
        tag_ids: vec![bob_tag],
        ..Default::default()
    };
    let res = tags::create_smart_tag(&w.app.pool, w.alice, "Foreign Leak", None, &filter).await;
    assert!(res.is_err(), "foreign tag id must be rejected");

    let row: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM hub_smart_tags st JOIN hub_tags t ON t.id = st.tag_id
          WHERE t.owner_user_id = ?1 AND t.slug = 'foreign-leak'",
    )
    .bind(w.alice)
    .fetch_one(&w.app.pool)
    .await
    .unwrap();
    assert_eq!(row, 0, "no smart tag row may be written on rejection");
}
