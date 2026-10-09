//! UI shell + navigation integration tests (issue #159).
//!
//! Verifies every shell page renders through the unified topbar: guarded pages
//! redirect to `/login` without a session, and authenticated pages carry the
//! nav, the user slug and an `aria-current` marker on the active item.

mod common;

async fn body(resp: reqwest::Response) -> String {
    resp.text().await.expect("response body")
}

#[tokio::test]
async fn dashboard_requires_login() {
    let app = common::spawn().await;
    let resp = app.client().get(app.url("/")).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn guarded_shell_pages_require_login() {
    let app = common::spawn().await;
    for path in ["/me/playlists", "/sql", "/search"] {
        let resp = app.client().get(app.url(path)).send().await.unwrap();
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::SEE_OTHER,
            "{path} should redirect when logged out"
        );
    }
}

#[tokio::test]
async fn dashboard_renders_shell_and_active_nav() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;

    // Shell: brand, nav items, user menu.
    assert!(html.contains("MMM Hub"), "brand missing");
    assert!(html.contains("Übersicht"), "nav item missing");
    assert!(html.contains("/me/playlists"), "playlists nav missing");
    assert!(html.contains("@alice"), "user slug missing");
    assert!(html.contains("Logout"), "logout missing");
    // The dashboard item is the active one.
    assert!(
        html.contains("aria-current=\"page\""),
        "active nav marker missing"
    );
}

#[tokio::test]
async fn playlists_page_renders() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/me/playlists"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("Meine Playlists"));
    assert!(html.contains("aria-current=\"page\""));
    // The fixture playlist shows up, with both owner columns.
    assert!(html.contains("Deep House"));
    assert!(html.contains("Besitzer"));
    assert!(html.contains("Alice"), "spotify owner missing from table");
    assert!(html.contains("@alice"), "hub user column missing");
}

#[tokio::test]
async fn track_page_renders_in_shell() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url(&format!("/track/{}", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("Shared Anthem"));
    assert!(html.contains("MMM Hub"), "shell missing on track page");
    assert!(html.contains("@alice"));
}

#[tokio::test]
async fn login_page_renders_without_session() {
    let app = common::spawn().await;
    let resp = app.client().get(app.url("/login")).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("action=\"/login\""));
    // Registration is closed by default -> no signup link.
    assert!(!html.contains("Registrieren"));
}

#[tokio::test]
async fn overlap_page_renders_shared_data() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/overlap"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("Entdecken"));
    // Fixture track shared by all three users.
    assert!(html.contains("Shared Anthem"));
    assert!(html.contains("@alice") && html.contains("@bob"));
    // One column per user + scope controls.
    assert!(html.contains("<th scope=\"col\">@alice"));
    assert!(html.contains("Eigene") && html.contains("Gefolgt"));
}

#[tokio::test]
async fn compare_view_columns_follow_user_selection() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let url = format!("/overlap?users={},{}", app.seed.alice, app.seed.bob);

    let resp = app
        .client()
        .get(app.url(&url))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("<th scope=\"col\">@alice"));
    assert!(html.contains("<th scope=\"col\">@bob"));
    assert!(
        !html.contains("<th scope=\"col\">@carol"),
        "carol should not be a column when deselected"
    );
    assert!(html.contains("Shared Anthem"));
}

#[tokio::test]
async fn compare_scope_owned_drops_followed_only_tracks() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/overlap?scope=owned"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    // t_all + t_two are shared via likes and stay.
    assert!(html.contains("Shared Anthem"));
    assert!(html.contains("Two Users"));
    // t_pl is shared only through alice's OWN + bob's FOLLOWED playlist;
    // with scope=owned bob's contribution drops, so the track is gone.
    assert!(
        !html.contains("Playlist Only"),
        "followed-only overlap should be filtered out"
    );
}

#[tokio::test]
async fn settings_page_renders() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/settings"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("Einstellungen"));
    assert!(html.contains("Spotify"));
    assert!(html.contains("/user/alice"));
}

#[tokio::test]
async fn playlist_filter_is_server_side() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // All playlists include the fixture playlist.
    let all = app
        .client()
        .get(app.url("/me/playlists?filter=all"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert!(body(all).await.contains("Deep House"));

    // No fixture playlist has an error, so the error filter yields no rows.
    let err = app
        .client()
        .get(app.url("/me/playlists?filter=error"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    let html = body(err).await;
    assert!(!html.contains("Deep House"));
    assert!(html.contains("Keine Playlists in dieser Ansicht"));
}

#[tokio::test]
async fn playlist_tag_and_owner_filters() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    async fn fetch(app: &common::TestApp, cookie: &str, path: &str) -> String {
        let resp = app
            .client()
            .get(app.url(path))
            .header("Cookie", cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        body(resp).await
    }

    // Before any tag: everything is untagged, nothing is tagged.
    let untagged = fetch(&app, &cookie, "/me/playlists?filter=untagged").await;
    assert!(untagged.contains("Deep House"));
    assert!(untagged.contains("Shared Collab"));
    let tagged = fetch(&app, &cookie, "/me/playlists?filter=tagged").await;
    assert!(tagged.contains("Keine Playlists in dieser Ansicht"));

    // Promote one playlist to a tag and give it a distinct name so the Tags
    // column is unambiguous (the playlist itself stays "Deep House").
    let tag = mmm_hub::tags::create_from_playlist(&app.pool, app.seed.alice, app.seed.pl_alice)
        .await
        .unwrap();
    mmm_hub::tags::rename_tag(&app.pool, app.seed.alice, tag, "Housey")
        .await
        .unwrap();

    // The Tags column now shows the tag for that playlist only.
    let all = fetch(&app, &cookie, "/me/playlists?filter=all").await;
    assert!(all.contains("Housey"), "tag column must show the tag");

    // tagged = only the tagged playlist; untagged = the other one.
    let tagged = fetch(&app, &cookie, "/me/playlists?filter=tagged").await;
    assert!(tagged.contains("Deep House"));
    assert!(!tagged.contains("Shared Collab"));
    let untagged = fetch(&app, &cookie, "/me/playlists?filter=untagged").await;
    assert!(!untagged.contains(">Deep House</a>"));
    assert!(untagged.contains("Shared Collab"));

    // Owner filter: fixture playlists are owned by "Alice".
    let alice = fetch(&app, &cookie, "/me/playlists?filter=all&owner=Alice").await;
    assert!(alice.contains("Deep House"));
    let nobody = fetch(&app, &cookie, "/me/playlists?filter=all&owner=Nobody").await;
    assert!(nobody.contains("Keine Playlists in dieser Ansicht"));

    // The owner picker is rendered.
    assert!(all.contains("name=\"owner\""));
}

#[tokio::test]
async fn toggle_returns_row_fragment_for_htmx() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .post(app.url(&format!("/api/hub/playlists/{}/toggle", app.seed.pl_alice)))
        .header("Cookie", &cookie)
        .header("HX-Request", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.trim_start().starts_with("<tr"), "expected a row fragment");
    assert!(html.contains("Deep House"));
}

#[tokio::test]
async fn toggle_without_htmx_redirects() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .post(app.url(&format!("/api/hub/playlists/{}/toggle", app.seed.pl_alice)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/me/playlists");
}

#[tokio::test]
async fn sync_html_swaps_for_htmx_and_redirects_otherwise() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let htmx = app
        .client()
        .post(app.url("/api/hub/services/spotify/sync-html"))
        .header("Cookie", &cookie)
        .header("HX-Request", "true")
        .send()
        .await
        .unwrap();
    assert_eq!(htmx.status(), reqwest::StatusCode::OK);
    assert!(body(htmx).await.contains("synchronisiert"));

    let plain = app
        .client()
        .post(app.url("/api/hub/services/spotify/sync-html"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(plain.status(), reqwest::StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn track_page_distinguishes_owned_and_followed_playlists() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // t_pl is in alice's OWN "Deep House" and bob's FOLLOWED "Deep House".
    let resp = app
        .client()
        .get(app.url(&format!("/track/{}", app.seed.t_pl)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    // Compact view: owned/followed playlists appear as tagged chips.
    assert!(html.contains("Deep House"));
    assert!(html.contains("hub-tag-own"), "own tag missing");
    assert!(html.contains("hub-tag-follow"), "followed tag missing");
    assert!(html.contains("@alice") && html.contains("@bob"), "user rows missing");
    // Guard against unrendered askama placeholders leaking as literal text.
    assert!(!html.contains("{u."), "unrendered askama placeholder leaked");
}

#[tokio::test]
async fn playlist_detail_shows_both_owners() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url(&format!("/playlist/{}", app.seed.pl_alice)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("@alice"), "hub user missing");
    assert!(html.contains("Spotify-Besitzer: Alice"), "spotify owner missing");
}

#[tokio::test]
async fn similar_playlists_view() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/playlists/similar"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("Ähnliche Playlists"));
    // alice and bob both have "Deep House" -> normalised key "deep house".
    assert!(html.contains("deep house"));
    assert!(html.contains("<th scope=\"col\">@alice"));
    assert!(html.contains("<th scope=\"col\">@bob"));
    // Scope filter + shared (collaborative) detection.
    assert!(html.contains("Collaborativ"));
    assert!(html.contains("shared collab"));
    assert!(html.contains("geteilt"), "shared badge missing");
}

#[tokio::test]
async fn similar_scope_contributed_keeps_only_collaborative() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/playlists/similar?scope=contributed"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("shared collab"));
    // "Deep House" is owned (alice) / followed (bob), not collaborative.
    assert!(!html.contains("deep house"));
}

#[tokio::test]
async fn collective_members_are_contributors() {
    let app = common::spawn().await;
    let col = mmm_hub::tags::create_collective(&app.pool, app.seed.alice, "Crew", "🤝")
        .await
        .unwrap();
    let g = mmm_hub::tags::create_group(&app.pool, app.seed.alice, "Mood", "💜")
        .await
        .unwrap();
    mmm_hub::tags::set_group_collective(&app.pool, app.seed.alice, g, Some(col))
        .await
        .unwrap();

    // Bob isn't in the collective yet -> no role on the group.
    assert!(mmm_hub::tags::effective_role_of(&app.pool, app.seed.bob, g)
        .await
        .is_none());

    // Add him to the *collective* -> contributor on the group.
    mmm_hub::tags::set_collective_member(&app.pool, app.seed.alice, col, "bob", "member")
        .await
        .unwrap();
    assert_eq!(
        mmm_hub::tags::effective_role_of(&app.pool, app.seed.bob, g)
            .await
            .as_deref(),
        Some("contributor")
    );
    assert!(mmm_hub::tags::can_contribute(&app.pool, app.seed.bob, g).await);
    assert!(mmm_hub::tags::list_groups_for(&app.pool, app.seed.bob)
        .await
        .iter()
        .any(|x| x.id == g && x.inherited));
}

#[tokio::test]
async fn groups_have_roles_and_hold_tags() {
    let app = common::spawn().await;

    // Alice creates a group and puts a tag in it.
    let g = mmm_hub::tags::create_group(&app.pool, app.seed.alice, "Mood", "💜")
        .await
        .unwrap();
    let tag = mmm_hub::tags::create_from_playlist(&app.pool, app.seed.alice, app.seed.pl_alice)
        .await
        .unwrap();
    mmm_hub::tags::add_tag_to_group(&app.pool, app.seed.alice, tag, g)
        .await
        .unwrap();
    assert!(
        mmm_hub::tags::groups_for_tag(&app.pool, tag)
            .await
            .iter()
            .any(|(id, _, _)| *id == g)
    );

    // Bob isn't a member -> it shows up in discover; he subscribes.
    assert!(
        mmm_hub::tags::list_discover_groups(&app.pool, app.seed.bob)
            .await
            .iter()
            .any(|x| x.id == g)
    );
    mmm_hub::tags::subscribe(&app.pool, app.seed.bob, g)
        .await
        .unwrap();
    assert!(
        mmm_hub::tags::list_groups_for(&app.pool, app.seed.bob)
            .await
            .iter()
            .any(|x| x.id == g && x.role == "subscriber")
    );

    // Subscribers can't contribute; the owner promotes him to contributor.
    assert!(!mmm_hub::tags::can_contribute(&app.pool, app.seed.bob, g).await);
    mmm_hub::tags::set_member_role(&app.pool, app.seed.alice, g, "bob", "contributor")
        .await
        .unwrap();
    assert!(mmm_hub::tags::can_contribute(&app.pool, app.seed.bob, g).await);

    // Many-to-many: the same tag can join a second group.
    let g2 = mmm_hub::tags::create_group(&app.pool, app.seed.alice, "Vibe", "🌈")
        .await
        .unwrap();
    mmm_hub::tags::add_tag_to_group(&app.pool, app.seed.alice, tag, g2)
        .await
        .unwrap();
    assert_eq!(mmm_hub::tags::groups_for_tag(&app.pool, tag).await.len(), 2);
}

#[tokio::test]
async fn tags_are_explicit_and_per_user() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // No tags exist until a user creates one.
    let before = mmm_hub::tags::rebuild(&app.pool).await.expect("rebuild");
    assert_eq!(before.tags, 0, "tags must not be auto-derived");

    // Promote alice's playlist to a tag (1:1).
    let tag_id = mmm_hub::tags::create_from_playlist(&app.pool, app.seed.alice, app.seed.pl_alice)
        .await
        .expect("create tag");
    assert!(tag_id > 0);

    let after = mmm_hub::tags::rebuild(&app.pool).await.unwrap();
    assert_eq!(after.tags, 1);

    let resp = app
        .client()
        .get(app.url("/tags"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("alice"), "owner column should be shown");
}

#[tokio::test]
async fn digging_internal_suggestions() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // Without a seed -> prompt.
    let no_seed = app
        .client()
        .get(app.url("/digging"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert!(body(no_seed).await.contains("Kein Seed"));

    // Seed t_pl: co-occurs with t_all in alice's "Deep House" playlist.
    let resp = app
        .client()
        .get(app.url(&format!("/digging?seed={}", app.seed.t_pl)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("Hub-intern"));
    assert!(html.contains("Shared Anthem"));
}

#[tokio::test]
async fn registration_closed_by_default() {
    let app = common::spawn().await;

    let login = app.client().get(app.url("/login")).send().await.unwrap();
    assert!(!body(login).await.contains("Registrieren"));

    let resp = app
        .client()
        .post(app.url("/signup"))
        .form(&[("username", "newbie"), ("password", "pw1234")])
        .send()
        .await
        .unwrap();
    // Rendered "closed" page, not a 303 session redirect.
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_users WHERE slug = 'newbie'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "no account should be created while closed");
}

#[tokio::test]
async fn admin_page_is_admin_only() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let denied = app
        .client()
        .get(app.url("/admin"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), reqwest::StatusCode::FORBIDDEN);

    sqlx::query("UPDATE hub_users SET is_admin = 1 WHERE id = ?1")
        .bind(app.seed.alice)
        .execute(&app.pool)
        .await
        .unwrap();
    let ok = app
        .client()
        .get(app.url("/admin"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(ok.status(), reqwest::StatusCode::OK);
    let html = body(ok).await;
    assert!(html.contains("Last.fm API-Key"));
    assert!(html.contains("Registrierung offen"));
}

#[tokio::test]
async fn admin_can_save_settings() {
    let app = common::spawn().await;
    sqlx::query("UPDATE hub_users SET is_admin = 1 WHERE id = ?1")
        .bind(app.seed.alice)
        .execute(&app.pool)
        .await
        .unwrap();
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .post(app.url("/admin"))
        .header("Cookie", &cookie)
        .form(&[("lastfm_api_key", "KEY123"), ("registration_open", "on")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        mmm_hub::settings::get(&app.pool, "lastfm_api_key").await.as_deref(),
        Some("KEY123")
    );
    assert!(mmm_hub::settings::registration_open(&app.pool).await);
}

#[tokio::test]
async fn tags_groups_collectives_are_renameable() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // Tag rename (owner) + ownership guard.
    let tag = mmm_hub::tags::create_from_playlist(&app.pool, app.seed.alice, app.seed.pl_alice)
        .await
        .unwrap();
    mmm_hub::tags::rename_tag(&app.pool, app.seed.alice, tag, "Neuer Name")
        .await
        .unwrap();
    let d = mmm_hub::tags::tag_detail(&app.pool, tag).await.unwrap();
    assert_eq!(d.name, "Neuer Name");
    assert!(
        mmm_hub::tags::rename_tag(&app.pool, app.seed.bob, tag, "Hacked")
            .await
            .is_err(),
        "a non-owner must not be able to rename"
    );

    // Group rename + icon over HTTP (owner).
    let g = mmm_hub::tags::create_group(&app.pool, app.seed.alice, "Mood", "💜")
        .await
        .unwrap();
    let resp = app
        .client()
        .post(app.url(&format!("/groups/{g}/update")))
        .header("Cookie", &cookie)
        .form(&[("name", "Vibe"), ("icon", "🌈")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    let html = body(
        app.client()
            .get(app.url(&format!("/groups/{g}")))
            .header("Cookie", &cookie)
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert!(html.contains("Vibe"), "renamed group must be shown");
    assert!(html.contains("🌈"), "new icon must be shown");

    // Collective rename + icon (owner) + ownership guard.
    let col = mmm_hub::tags::create_collective(&app.pool, app.seed.alice, "Crew", "🤝")
        .await
        .unwrap();
    mmm_hub::tags::update_collective(&app.pool, app.seed.alice, col, "Squad", "🚀")
        .await
        .unwrap();
    let cd = mmm_hub::tags::collective_detail(&app.pool, app.seed.alice, col)
        .await
        .unwrap();
    assert_eq!(cd.name, "Squad");
    assert_eq!(cd.icon, "🚀");
    assert!(
        mmm_hub::tags::update_collective(&app.pool, app.seed.bob, col, "Nope", "")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn no_shell_page_returns_500() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    for path in ["/", "/me/playlists", "/sql", "/search?q=shared", "/login"] {
        let resp = app
            .client()
            .get(app.url(path))
            .header("Cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert!(
            resp.status().is_success() || resp.status().is_redirection(),
            "{path} returned {}",
            resp.status()
        );
    }
}
