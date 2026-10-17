//! Integration tests for the card-based tag-task UI (#347).
//!
//! The page renders the persisted #346 task as cards (most-played first),
//! shows the still-missing describing families as chips and allows direct
//! tagging plus a manual `done`/`undo` toggle.

mod common;

/// Tag a track with one typed tag through the public `tags` API (full pipeline).
async fn add_tag(app: &common::TestApp, user: i64, track: i64, name: &str, ty: &str) {
    let tag_id = music_hub::tags::ensure_tag(&app.pool, user, name)
        .await
        .unwrap();
    let t = music_hub::tagtypes::TagType::parse(ty).unwrap();
    music_hub::tagtypes::set(&app.pool, user, tag_id, Some(t))
        .await
        .unwrap();
    music_hub::tags::tag_track(&app.pool, &app.cfg, user, track, tag_id)
        .await
        .unwrap();
}

async fn set_plays(app: &common::TestApp, user: i64, track: i64, plays: i64) {
    sqlx::query(
        "INSERT INTO hub_traktor_tracks (user_id, track_id, play_count, imported_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(user_id, track_id) DO UPDATE SET play_count = excluded.play_count",
    )
    .bind(user)
    .bind(track)
    .bind(plays)
    .bind("2026-01-01T00:00:00+00:00")
    .execute(&app.pool)
    .await
    .unwrap();
}

async fn get_page(app: &common::TestApp, cookie: &str) -> String {
    let resp = app
        .client()
        .get(app.url("/tag-task"))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    resp.text().await.unwrap()
}

/// The HTML fragment of the card whose track-home link is `/track/{id}`.
fn card_html(html: &str, id: i64) -> String {
    let anchor = format!("/track/{id}\"");
    let start = html.find(&anchor).unwrap_or_else(|| {
        panic!("card for track {id} not found (anchor {anchor} missing)")
    });
    let rest = &html[start..];
    let end = rest.find("</article>").expect("card must close") + "</article>".len();
    rest[..end].to_string()
}

#[tokio::test]
async fn page_requires_login() {
    let app = common::spawn().await;
    let resp = app.client().get(app.url("/tag-task")).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn page_renders_cards_most_played_first() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    set_plays(&app, alice, app.seed.t_two, 10).await;
    set_plays(&app, alice, app.seed.t_all, 3).await;

    let html = get_page(&app, &cookie).await;
    assert!(html.contains("Tag-Task"), "heading missing");

    // Most-played track must be ranked first (rendered before the other one).
    let first = html.find(&format!("/track/{}", app.seed.t_two)).unwrap();
    let second = html.find(&format!("/track/{}", app.seed.t_all)).unwrap();
    assert!(first < second, "t_two (10 plays) must come before t_all (3)");
    assert!(html.contains("#1"), "rank badge missing");
    assert!(html.contains("10 Plays"), "plays count missing");

    // Active nav entry.
    assert!(
        html.contains("aria-current=\"page\""),
        "active nav state missing"
    );
}

#[tokio::test]
async fn page_shows_missing_family_chips() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    // t_all covers MOOD + GENRE only -> VIBE, PHASE, MERKMAL still missing.
    add_tag(&app, alice, app.seed.t_all, "Dunkel", "MOOD").await;
    add_tag(&app, alice, app.seed.t_all, "House", "GENRE").await;

    let html = get_page(&app, &cookie).await;
    let card = card_html(&html, app.seed.t_all);
    assert!(card.contains("Fehlt: Vibe"), "VIBE chip missing");
    assert!(card.contains("Fehlt: Phase"), "PHASE chip missing");
    assert!(card.contains("Fehlt: Merkmal"), "MERKMAL chip missing");
    assert!(
        !card.contains("Fehlt: Genre"),
        "covered family must not be a chip"
    );
}

#[tokio::test]
async fn tagging_from_card_applies_and_clears_missing() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    // Everything but MERKMAL covered -> only one chip left.
    add_tag(&app, alice, app.seed.t_all, "Dunkel", "MOOD").await;
    add_tag(&app, alice, app.seed.t_all, "House", "GENRE").await;
    add_tag(&app, alice, app.seed.t_all, "Warm", "VIBE").await;
    add_tag(&app, alice, app.seed.t_all, "Peak", "PHASE_1").await;

    let before = get_page(&app, &cookie).await;
    assert!(
        card_html(&before, app.seed.t_all).contains("Fehlt: Merkmal"),
        "MERKMAL must be missing before tagging"
    );

    // Tag the missing family straight from the card.
    let resp = app
        .client()
        .post(app.url(&format!("/tag-task/{}/tag", app.seed.t_all)))
        .header("Cookie", &cookie)
        .form(&[("name", "Informativ"), ("type", "MERKMAL")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/tag-task");

    // The consumed family is gone at the next render.
    let after = get_page(&app, &cookie).await;
    let card = card_html(&after, app.seed.t_all);
    assert!(
        !card.contains("Fehlt: Merkmal"),
        "tagged family must disappear"
    );
    assert!(
        card.contains("Keine Familien offen"),
        "empty-family placeholder missing"
    );
}

#[tokio::test]
async fn done_hides_card_and_undone_restores_it() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    let before = get_page(&app, &cookie).await;
    assert!(before.contains(&format!("/track/{}", app.seed.t_two)));

    // Mark done -> card disappears.
    for _ in 0..2 {
        let resp = app
            .client()
            .post(app.url(&format!("/tag-task/{}/done", app.seed.t_two)))
            .header("Cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
        assert_eq!(resp.headers()["location"], "/tag-task");
    }
    let hidden = get_page(&app, &cookie).await;
    assert!(
        !hidden.contains(&format!("/track/{}", app.seed.t_two)),
        "done track must be suppressed"
    );

    // Undo -> card comes back.
    let resp = app
        .client()
        .post(app.url(&format!("/tag-task/{}/undone", app.seed.t_two)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    let restored = get_page(&app, &cookie).await;
    assert!(
        restored.contains(&format!("/track/{}", app.seed.t_two)),
        "undone track must reappear"
    );
}

#[tokio::test]
async fn empty_state_when_everything_is_done() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    for t in [
        app.seed.t_all,
        app.seed.t_two,
        app.seed.t_pl,
        app.seed.t_alice,
        app.seed.t_bob,
        app.seed.t_carol,
    ] {
        music_hub::tagdone::set_done(&app.pool, alice, t)
            .await
            .unwrap();
    }

    let html = get_page(&app, &cookie).await;
    assert!(
        html.contains("keine offenen Tracks"),
        "empty-state text missing"
    );
}

#[tokio::test]
async fn post_endpoints_require_login() {
    let app = common::spawn().await;
    for path in ["tag", "done", "undone"] {
        let resp = app
            .client()
            .post(app.url(&format!("/tag-task/{}/{path}", app.seed.t_all)))
            .form(&[("name", "Dark")])
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER, "{path}");
        assert_eq!(resp.headers()["location"], "/login", "{path}");
    }
}
