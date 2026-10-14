//! Feedback round: tag-task card shows applied tags, has a play button, updates
//! in place via htmx, and supports a "nur meine Playlists" filter.

mod common;

use music_hub::config::Config;

async fn get_page(app: &common::TestApp, cookie: &str, qs: &str) -> String {
    let resp = app
        .client()
        .get(app.url(&format!("/tag-task{qs}")))
        .header("Cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK, "GET /tag-task{qs}");
    resp.text().await.unwrap()
}

/// The HTML fragment of the card whose track-home link is `/track/{id}`.
fn card_html(html: &str, id: i64) -> String {
    let anchor = format!("/track/{id}\"");
    let start = html
        .find(&anchor)
        .unwrap_or_else(|| panic!("card for track {id} not found"));
    let rest = &html[start..];
    let end = rest.find("</article>").expect("card must close") + "</article>".len();
    rest[..end].to_string()
}

async fn add_tag(app: &common::TestApp, user: i64, track: i64, name: &str, ty: &str) {
    let tag_id = music_hub::tags::ensure_tag(&app.pool, user, name)
        .await
        .unwrap();
    let t = music_hub::tagtypes::TagType::parse(ty).unwrap();
    music_hub::tagtypes::set(&app.pool, user, tag_id, Some(t))
        .await
        .unwrap();
    music_hub::tags::tag_track(&app.pool, user, track, tag_id)
        .await
        .unwrap();
}

#[tokio::test]
async fn card_shows_applied_tags() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    add_tag(&app, alice, app.seed.t_all, "Dunkel", "MOOD").await;

    let html = get_page(&app, &cookie, "").await;
    let card = card_html(&html, app.seed.t_all);
    assert!(
        card.contains("Dunkel"),
        "the card must show the tag that was just applied"
    );
}

#[tokio::test]
async fn card_has_play_button_when_playable() {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}/hub.db", dir.path().display());
    let mut cfg = Config::for_test(url);
    cfg.music_api_token = Some("test-token".to_string());
    let app = common::spawn_with(dir, cfg).await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    // Give the track an ISRC so it becomes streamable.
    sqlx::query("UPDATE hub_tracks SET isrc = 'US1234567890' WHERE id = ?1")
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();

    let html = get_page(&app, &cookie, "").await;
    let card = card_html(&html, app.seed.t_all);
    assert!(
        card.contains(&format!("/track/{}/stream", app.seed.t_all)),
        "a playable card must carry the stream player"
    );
    assert!(card.contains("hub-playbtn"), "play button missing");
}

#[tokio::test]
async fn htmx_tag_returns_the_card_fragment() {
    let app = common::spawn().await;
    let alice = app.seed.alice;
    let cookie = app.session_cookie(alice).await;

    let resp = app
        .client()
        .post(app.url(&format!("/tag-task/{}/tag", app.seed.t_all)))
        .header("Cookie", &cookie)
        .header("HX-Request", "true")
        .form(&[
            ("name", "Informativ"),
            ("type", "MERKMAL"),
            ("cadence", "daily"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body = resp.text().await.unwrap();
    assert!(
        body.contains("hub-tag-task-card"),
        "htmx response must be the card fragment"
    );
    assert!(
        body.contains("Informativ"),
        "the freshly added tag must appear in the fragment"
    );
}

#[tokio::test]
async fn mine_filter_limits_cards_to_my_playlists() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // Alice's playlists contain t_all + t_pl (not t_two).
    let html = get_page(&app, &cookie, "?mine=1").await;
    assert!(
        html.contains(&format!("/track/{}", app.seed.t_all)),
        "t_all is in alice's playlist and must stay"
    );
    assert!(
        !html.contains(&format!("/track/{}\"", app.seed.t_two)),
        "t_two is not in alice's playlists and must be filtered out"
    );
}
