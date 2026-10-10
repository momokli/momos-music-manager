//! Integration tests for the artist explorer (`/artists` + `/artist/{name}`)
//! — plan AR1–AR6 (issues #227–#232).

mod common;

use reqwest::StatusCode;

async fn insert_track(
    pool: &sqlx::SqlitePool,
    id: i64,
    artists: &str,
    title: &str,
    album: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO hub_tracks (id, service, service_track_id, title, artists, album)
         VALUES (?1, 'spotify', ?2, ?3, ?4, ?5)",
    )
    .bind(id)
    .bind(format!("a{id}"))
    .bind(title)
    .bind(artists)
    .bind(album)
    .execute(pool)
    .await
    .unwrap();
}

async fn play(pool: &sqlx::SqlitePool, uid: i64, track_id: i64, count: i64) {
    sqlx::query(
        "INSERT INTO hub_traktor_tracks (user_id, track_id, play_count) VALUES (?1, ?2, ?3)",
    )
    .bind(uid)
    .bind(track_id)
    .bind(count)
    .execute(pool)
    .await
    .unwrap();
}

async fn playlist(pool: &sqlx::SqlitePool, uid: i64, pid: &str, name: &str, tracks: &[i64]) -> i64 {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_playlists (user_id, service, playlist_id, name, is_owned)
         VALUES (?1, 'spotify', ?2, ?3, 1) RETURNING id",
    )
    .bind(uid)
    .bind(pid)
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap();
    for tid in tracks {
        sqlx::query(
            "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position) VALUES (?1, ?2, 0)",
        )
        .bind(id)
        .bind(tid)
        .execute(pool)
        .await
        .unwrap();
    }
    id
}

async fn tag_track(pool: &sqlx::SqlitePool, track_id: i64, tag_id: i64) {
    sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(track_id)
        .bind(tag_id)
        .execute(pool)
        .await
        .unwrap();
}

/// Seed a small artist graph around "Nova":
/// * Nova (70001, 70002), Nova/Rex collab (70003), Rex (70004), Solo (70005), Kai (70007)
/// * Traktor plays for alice + bob, a b2b playlist for alice.
/// * Tags: Peak (Phase) on 70001/70004/70007; Dark (Mood) on 70002.
/// * Playlists: alice "Deep House" [70001, 70004]; carol "Techno" [70007].
async fn seed_artists(app: &common::TestApp) -> (i64, i64, i64) {
    let alice = app.seed.alice;
    let carol = app.seed.carol;
    let bob = app.seed.bob;
    let p = &app.pool;

    insert_track(p, 70001, "Nova", "Track One", Some("Album X")).await;
    insert_track(p, 70002, "Nova", "Track Two", Some("Album X")).await;
    insert_track(p, 70003, "Nova; Rex", "Collab Track", Some("Album Y")).await;
    insert_track(p, 70004, "Rex", "Rex Solo", Some("Album Y")).await;
    insert_track(p, 70005, "Solo", "Solo Only", None).await;
    insert_track(p, 70007, "Kai", "Kai Tune", None).await;

    play(p, alice, 70001, 10).await;
    play(p, alice, 70003, 3).await;
    play(p, bob, 70001, 5).await;
    play(p, bob, 70002, 2).await;

    // Traktor b2b set for alice.
    sqlx::query(
        "INSERT INTO hub_traktor_playlists (id, user_id, name, node_type)
         VALUES (1, ?1, 'Nova b2b Rex', 'playlist')",
    )
    .bind(alice)
    .execute(p)
    .await
    .unwrap();
    for tid in [70001_i64, 70004] {
        sqlx::query(
            "INSERT INTO hub_traktor_playlist_tracks (user_id, playlist_id, track_id)
             VALUES (?1, 1, ?2)",
        )
        .bind(alice)
        .bind(tid)
        .execute(p)
        .await
        .unwrap();
    }

    // Tags + groups.
    let phase = mmm_hub::tags::create_group(p, alice, "Phase", "")
        .await
        .unwrap();
    let mood = mmm_hub::tags::create_group(p, alice, "Mood", "")
        .await
        .unwrap();
    let peak = mmm_hub::tags::ensure_tag(p, alice, "Peak").await.unwrap();
    let dark = mmm_hub::tags::ensure_tag(p, alice, "Dark").await.unwrap();
    mmm_hub::tags::add_tag_to_group(p, alice, peak, phase)
        .await
        .unwrap();
    mmm_hub::tags::add_tag_to_group(p, alice, dark, mood)
        .await
        .unwrap();
    tag_track(p, 70001, peak).await;
    tag_track(p, 70002, dark).await;
    tag_track(p, 70004, peak).await;
    tag_track(p, 70007, peak).await;

    // Playlists.
    playlist(p, alice, "pl-deep", "Deep House", &[70001, 70004]).await;
    playlist(p, carol, "pl-tech", "Techno", &[70007]).await;

    (alice, bob, carol)
}

// ── AR1 ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn artists_requires_login() {
    let app = common::spawn().await;
    for path in ["/artists", "/artist/Nova"] {
        let resp = app.client().get(app.url(path)).send().await.unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::SEE_OTHER,
            "{path} should redirect"
        );
        assert_eq!(
            resp.headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok()),
            Some("/login"),
            "{path} should redirect to /login"
        );
    }
}

#[tokio::test]
async fn artists_list_renders_and_filters() {
    let app = common::spawn().await;
    let (alice, ..) = seed_artists(&app).await;
    let cookie = app.session_cookie(alice).await;

    let html = app
        .client()
        .get(app.url("/artists"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Nova") && html.contains("Rex") && html.contains("Solo"));

    // name search
    let html = app
        .client()
        .get(app.url("/artists?q=nova"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Nova"), "search hit missing");
    assert!(
        !html.contains(">Solo<"),
        "non-matching artist leaked into search"
    );

    // "played by me" (alice plays Nova only)
    let html = app
        .client()
        .get(app.url("/artists?plays=1"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Nova"));
    assert!(
        !html.contains(">Solo<"),
        "unplayed artist leaked into plays=1"
    );

    // "tagged" (Solo has no tags, Kai/Rex/Nova do)
    let html = app
        .client()
        .get(app.url("/artists?tags=1"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Nova") && html.contains("Kai"));
    assert!(
        !html.contains(">Solo<"),
        "untagged artist leaked into tags=1"
    );
}

// ── AR2 + AR3 + AR4 + AR5 ────────────────────────────────────────────────────

#[tokio::test]
async fn artist_profile_most_played_is_per_user_with_collapsible_others() {
    let app = common::spawn().await;
    let (alice, ..) = seed_artists(&app).await;
    let cookie = app.session_cookie(alice).await;

    let html = app
        .client()
        .get(app.url("/artist/Nova"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(html.contains("Meistgespielt"));
    assert!(html.contains("Track One"), "my most-played track missing");
    // alice: 10 plays on Track One; bob: 5 + 2 = 7 plays, collapsed.
    assert!(html.contains(">10<"), "my play count missing");
    assert!(html.contains("<details"), "other users must be collapsible");
    assert!(html.contains("@bob"), "other user missing");
    assert!(html.contains("7 Plays"), "other user's total plays missing");
}

#[tokio::test]
async fn artist_profile_tags_playlists_collabs_b2b_albums() {
    let app = common::spawn().await;
    let (alice, ..) = seed_artists(&app).await;
    let cookie = app.session_cookie(alice).await;

    let html = app
        .client()
        .get(app.url("/artist/Nova"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    // AR3 tag distribution + top tags.
    assert!(html.contains("Verteilung nach Gruppen"));
    assert!(html.contains("Phase"), "group distribution missing");
    assert!(html.contains("Peak"), "top tag missing");
    // AR4 playlists (mine).
    assert!(html.contains("Deep House"), "my playlist missing");
    // AR5 collabs + b2b + album co-artists.
    assert!(html.contains("Kollaborationspartner"));
    assert!(html.contains("Nova b2b Rex"), "b2b set missing");
    assert!(html.contains("Album X"), "album panel missing");
}

#[tokio::test]
async fn artist_profile_renders_explicit_empty_states() {
    let app = common::spawn().await;
    let (alice, ..) = seed_artists(&app).await;
    let cookie = app.session_cookie(alice).await;

    let html = app
        .client()
        .get(app.url("/artist/Solo"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    assert!(
        html.contains("Keine Multi-Artist-Tracks"),
        "collab empty state"
    );
    assert!(html.contains("Keine b2b-Sets"), "b2b empty state");
    assert!(html.contains("Keine Album-Daten"), "album empty state");
    assert!(html.contains("In keinen Playlists"), "playlist empty state");
    assert!(
        html.contains("Keine Tags für die Tracks"),
        "tag empty state"
    );
}

#[tokio::test]
async fn artist_unknown_is_404() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url("/artist/DoesNotExist"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ── AR6 ──────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn artist_references_are_visible_per_selected_user() {
    let app = common::spawn().await;
    let (alice, _, carol) = seed_artists(&app).await;
    let alice_cookie = app.session_cookie(alice).await;

    // Default view = alice: Rex is in her library, Kai is not.
    let html = app
        .client()
        .get(app.url("/artist/Nova"))
        .header("Cookie", &alice_cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Referenz-Künstler"));
    assert!(
        !html.contains("Kai"),
        "Kai must not be visible in alice's references"
    );

    // carol's references include Kai, not Rex.
    let carol_cookie = app.session_cookie(carol).await;
    let html = app
        .client()
        .get(app.url("/artist/Nova?user=carol"))
        .header("Cookie", &carol_cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("Kai"), "carol's reference Kai missing");
    assert!(html.contains("@carol"), "user selector chip missing");
}
