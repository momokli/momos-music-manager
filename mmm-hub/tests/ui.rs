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
    for path in ["/me/playlists", "/sql", "/search", "/import"] {
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
    assert!(html.contains("Spelunke"), "brand missing");
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
async fn playlists_page_shows_created_and_last_columns() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/me/playlists?filter=all"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(html.contains("Angelegt"), "created column header missing");
    assert!(
        html.contains("Letzte Änderung"),
        "last-added column header missing"
    );
    // The seeded membership carries a known added_at -> both columns render it
    // (date only, ISO-8601 prefix).
    assert!(html.contains("2026-01-01"), "derived date missing");

    // Both new columns are sortable.
    for sort in ["created", "last"] {
        let resp = app
            .client()
            .get(app.url(&format!("/me/playlists?filter=all&sort={sort}&dir=desc")))
            .header("Cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK, "sort={sort}");
        let html = body(resp).await;
        assert!(
            html.contains("Meine Playlists"),
            "sort={sort} should render"
        );
    }
}

#[tokio::test]
async fn tag_filters_tolerate_empty_numeric_params() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // Filter forms submit every field; empties must not 400 the request.
    for path in [
        "/tags?types=&q=dirty&owner=&collective=",
        "/tags?q=&types=&collective=&sort=name&dir=asc",
        "/digging?seed=&tol=&ttype=&wmin=",
        "/me/playlists?filter=all&sort=last&dir=desc",
    ] {
        let resp = app
            .client()
            .get(app.url(path))
            .header("Cookie", &cookie)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK, "GET {path}");
    }
}

#[tokio::test]
async fn playlist_tag_creation_redirects_to_tag() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .post(app.url(&format!("/playlist/{}/tag", app.seed.pl_alice)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    let loc = resp.headers()["location"].to_str().unwrap().to_string();
    assert!(loc.starts_with("/tag/"), "expected /tag/<id>, got {loc}");
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
    assert!(html.contains("Spelunke"), "shell missing on track page");
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
async fn import_page_renders_form_and_playlists() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // Seed a YouTube playlist for alice so the listing is non-empty.
    sqlx::query(
        "INSERT INTO hub_playlists (user_id, service, playlist_id, name, is_liked, track_count, items_available, fetched_at)
         VALUES (?1, 'youtube', 'yt-pl-1', 'YouTube Mix', 0, 3, 1, '2026-01-01T00:00:00+00:00')",
    )
    .bind(app.seed.alice)
    .execute(&app.pool)
    .await
    .unwrap();

    let resp = app
        .client()
        .get(app.url("/import"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;

    // Form: service select + URL field, posting back to /import.
    assert!(html.contains("action=\"/import\""), "import form missing");
    assert!(html.contains("name=\"service\""), "service field missing");
    assert!(html.contains("name=\"url\""), "url field missing");
    assert!(html.contains("SoundCloud"), "service option missing");
    // The seeded playlist shows up in the listing.
    assert!(html.contains("YouTube Mix"), "imported playlist missing");
    // The nav marks Import as the active item.
    assert!(
        html.contains("aria-current=\"page\""),
        "active nav marker missing"
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
async fn overlap_bpm_and_key_filters() {
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

    // Seed BPM/key for two of the three shared tracks; leave t_pl without data.
    let feat = |tid: i64, bpm: f64, camelot: &str| {
        let pool = app.pool.clone();
        let camelot = camelot.to_string();
        async move {
            sqlx::query(
                "INSERT INTO hub_track_features (track_id, found, bpm, camelot)
                 VALUES (?1, 1, ?2, ?3)",
            )
            .bind(tid)
            .bind(bpm)
            .bind(&camelot)
            .execute(&pool)
            .await
            .unwrap();
        }
    };
    feat(app.seed.t_all, 128.0, "8A").await;
    feat(app.seed.t_two, 140.0, "9A").await;

    // No filter: all three shared tracks.
    let all = fetch(&app, &cookie, "/overlap").await;
    assert!(all.contains("Shared Anthem"));
    assert!(all.contains("Two Users"));
    assert!(all.contains("Playlist Only"));
    // BPM/Key columns are rendered.
    assert!(all.contains(">128<"), "bpm column must show 128");
    assert!(all.contains(">8A<"), "key column must show 8A");

    // BPM range 100..135: only the 128-BPM track (t_pl has no data -> excluded).
    let bpm = fetch(&app, &cookie, "/overlap?bpm_min=100&bpm_max=135").await;
    assert!(bpm.contains("Shared Anthem"));
    assert!(!bpm.contains("Two Users"));
    assert!(!bpm.contains("Playlist Only"));

    // Exact key 8A: only t_all (t_two is 9A, t_pl has no key).
    let key = fetch(&app, &cookie, "/overlap?key=8A").await;
    assert!(key.contains("Shared Anthem"));
    assert!(!key.contains("Two Users"));
    assert!(!key.contains("Playlist Only"));

    // Harmonic 8A also admits the adjacent 9A (t_two).
    let harm = fetch(&app, &cookie, "/overlap?key=8A&key_harmonic=1").await;
    assert!(harm.contains("Shared Anthem"));
    assert!(harm.contains("Two Users"));
    assert!(!harm.contains("Playlist Only"));

    // The picker/scope links carry the active filters.
    let withf = fetch(&app, &cookie, "/overlap?bpm_min=100&key=8A").await;
    assert!(withf.contains("bpm_min=100"));
    assert!(withf.contains("key=8A"));
}

#[tokio::test]
async fn overlap_bpm_survives_large_track_set() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    sqlx::query(
        "INSERT INTO hub_track_features (track_id, found, bpm, camelot) VALUES (?1, 1, 128, ?2)",
    )
    .bind(app.seed.t_all)
    .bind("8A")
    .execute(&app.pool)
    .await
    .unwrap();

    // Put >900 filler tracks in alice's playlist so the audio lookup must chunk
    // (a single `IN (...)` would exceed SQLite's bind-variable limit and silently
    // drop all BPM/key data).
    for i in 0..950 {
        let sid = format!("filler-{i}");
        let tid: i64 = sqlx::query_scalar(
            "INSERT INTO hub_tracks (service, service_track_id, title, artists, first_seen_at)
             VALUES ('spotify', ?1, ?2, 'F', '2026-01-01T00:00:00+00:00') RETURNING id",
        )
        .bind(&sid)
        .bind(&sid)
        .fetch_one(&app.pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position, added_at)
             VALUES (?1, ?2, 0, '2026-01-01T00:00:00+00:00')",
        )
        .bind(app.seed.pl_alice)
        .bind(tid)
        .execute(&app.pool)
        .await
        .unwrap();
    }

    let resp = app
        .client()
        .get(app.url("/overlap"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    assert!(
        html.contains(">128<"),
        "BPM must survive a >900-track id set (chunked lookup)"
    );
    assert!(html.contains(">8A<"));
}

#[tokio::test]
async fn overlap_sort_and_playlist_filter() {
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

    for (tid, bpm, key, energy) in [
        (app.seed.t_all, 128.0, "8A", 0.9),
        (app.seed.t_two, 140.0, "9A", 0.5),
        (app.seed.t_pl, 120.0, "3B", 0.1),
    ] {
        sqlx::query(
            "INSERT INTO hub_track_features (track_id, found, bpm, camelot, energy)
             VALUES (?1, 1, ?2, ?3, ?4)",
        )
        .bind(tid)
        .bind(bpm)
        .bind(key)
        .bind(energy)
        .execute(&app.pool)
        .await
        .unwrap();
    }

    // BPM ascending: Playlist Only (120) < Shared Anthem (128) < Two Users (140).
    let asc = fetch(&app, &cookie, "/overlap?sort=bpm&dir=asc").await;
    let i_pl = asc.find("Playlist Only").unwrap();
    let i_all = asc.find("Shared Anthem").unwrap();
    let i_two = asc.find("Two Users").unwrap();
    assert!(i_pl < i_all && i_all < i_two, "bpm asc ordering");

    // BPM descending: the 140 track comes before the 128 one.
    let desc = fetch(&app, &cookie, "/overlap?sort=bpm&dir=desc").await;
    assert!(desc.find("Two Users").unwrap() < desc.find("Shared Anthem").unwrap());

    // Energy ascending: the 0.1 track before the 0.9 one.
    let e = fetch(&app, &cookie, "/overlap?sort=energy&dir=asc").await;
    assert!(e.find("Playlist Only").unwrap() < e.find("Shared Anthem").unwrap());

    // Playlist filter: with only carol's playlist, the playlist-only track
    // (present via alice's/bob's playlists) drops out, likes stay.
    let carol = fetch(&app, &cookie, &format!("/overlap?pl={}", app.seed.pl_carol)).await;
    assert!(!carol.contains("Playlist Only"));
    assert!(carol.contains("Shared Anthem"));
    assert!(carol.contains("gefiltert"), "picker marks active filter");
}

#[tokio::test]
async fn overlap_enrich_enqueues_missing_features() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // Give the shared tracks a Spotify id so they're eligible.
    for (tid, sid) in [
        (app.seed.t_all, "sp-all"),
        (app.seed.t_two, "sp-two"),
        (app.seed.t_pl, "sp-pl"),
    ] {
        sqlx::query(
            "INSERT INTO hub_track_external_ids (track_id, service, external_id, url)
             VALUES (?1, 'spotify', ?2, '')",
        )
        .bind(tid)
        .bind(sid)
        .execute(&app.pool)
        .await
        .unwrap();
    }

    let resp = app
        .client()
        .post(app.url("/overlap/enrich"))
        .header("Cookie", &cookie)
        .form(&[("users", ""), ("scope", "all")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    // Three shared tracks (present for >= 2 users) are queued.
    let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_feature_requests")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(queued, 3);

    // Tracks that already have usable features (`found = 1`) are not queued.
    sqlx::query(
        "INSERT INTO hub_track_features (track_id, found, source) VALUES (?1, 1, 'reccobeats')",
    )
    .bind(app.seed.t_all)
    .execute(&app.pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM hub_feature_requests")
        .execute(&app.pool)
        .await
        .unwrap();
    app.client()
        .post(app.url("/overlap/enrich"))
        .header("Cookie", &cookie)
        .form(&[("users", ""), ("scope", "all")])
        .send()
        .await
        .unwrap();
    let queued: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_feature_requests")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    assert_eq!(queued, 2, "track with features must be skipped");
}

#[tokio::test]
async fn overlap_text_and_tag_filters() {
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

    // Full-text over title.
    let by_title = fetch(&app, &cookie, "/overlap?q=Anthem").await;
    assert!(by_title.contains("Shared Anthem"));
    assert!(!by_title.contains("Two Users"));
    assert!(!by_title.contains("Playlist Only"));

    // Full-text over artists (case-insensitive).
    let by_artist = fetch(&app, &cookie, "/overlap?q=bbb").await;
    assert!(by_artist.contains("Two Users"));
    assert!(!by_artist.contains("Shared Anthem"));

    // Tag filter: alice's playlist (t_all + t_pl) becomes a tag, renamed.
    let tag = mmm_hub::tags::create_from_playlist(&app.pool, app.seed.alice, app.seed.pl_alice)
        .await
        .unwrap();
    mmm_hub::tags::rename_tag(&app.pool, app.seed.alice, tag, "Housey")
        .await
        .unwrap();
    let by_tag = fetch(&app, &cookie, "/overlap?tag=Housey").await;
    assert!(by_tag.contains("Shared Anthem"));
    assert!(by_tag.contains("Playlist Only"));
    assert!(!by_tag.contains("Two Users"));

    // Combined text + tag.
    let combined = fetch(&app, &cookie, "/overlap?q=Shared&tag=Housey").await;
    assert!(combined.contains("Shared Anthem"));
    assert!(!combined.contains("Playlist Only"));

    // The tag datalist is rendered with the tag option.
    assert!(by_title.contains("hub-tag-options"));
    assert!(by_tag.contains("Housey"));
}

#[tokio::test]
async fn tag_hierarchy_is_used_in_filters() {
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

    // Child tag (from alice's playlist, has t_all) with parent "house".
    let child = mmm_hub::tags::create_from_playlist(&app.pool, app.seed.alice, app.seed.pl_alice)
        .await
        .unwrap();
    let parent = mmm_hub::tags::ensure_tag(&app.pool, app.seed.alice, "house")
        .await
        .unwrap();
    mmm_hub::tags::add_tag_parent(&app.pool, child, parent)
        .await
        .unwrap();

    // Filtering by the PARENT name also matches the child's tracks.
    let by_parent = fetch(&app, &cookie, "/overlap?tag=house").await;
    assert!(by_parent.contains("Shared Anthem"));
    let none = fetch(&app, &cookie, "/overlap?tag=zzzzzz").await;
    assert!(none.contains("Keine gemeinsamen Tracks"));

    // The tag page shows the parent + child relationships.
    let tp = fetch(&app, &cookie, &format!("/tag/{child}")).await;
    assert!(tp.contains("Parent-Tags"));
    assert!(tp.contains("house"));
}

#[tokio::test]
async fn tag_type_weight_drives_digging_score_and_parents_visible() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // Tag (from alice's playlist, has t_all) with the Mood type (weight 1.0).
    let tag = mmm_hub::tags::create_from_playlist(&app.pool, app.seed.alice, app.seed.pl_alice)
        .await
        .unwrap();
    mmm_hub::tagtypes::set(
        &app.pool,
        app.seed.alice,
        tag,
        Some(mmm_hub::tagtypes::TagType::Mood),
    )
    .await
    .unwrap();

    // /tags shows the Parent-Tags column.
    let tags_html = app
        .client()
        .get(app.url("/tags"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert!(body(tags_html).await.contains("Parent-Tags"));

    // Digging: the candidate carries tag weight 1.0 -> wmin 1 keeps it, 2 drops it.
    let seed = app.seed.t_pl;
    let keeps = app
        .client()
        .get(app.url(&format!("/digging?seed={seed}&wmin=1")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    let keeps_html = body(keeps).await;
    assert!(keeps_html.contains("Shared Anthem"));
    // Seed↔candidate agreement signal is rendered (t_all shares the seed's tag).
    assert!(keeps_html.contains("Seed-Match"));
    let drops = app
        .client()
        .get(app.url(&format!("/digging?seed={seed}&wmin=2")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert!(!body(drops).await.contains("Shared Anthem"));

    // The tag page shows the type.
    let tp = app
        .client()
        .get(app.url(&format!("/tag/{tag}")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert!(body(tp).await.contains("Mood"));
}

#[tokio::test]
async fn ranking_engine_is_settings_driven() {
    let app = common::spawn().await;

    // Defaults.
    let e = mmm_hub::settings::engine(&app.pool).await;
    assert_eq!(e.shared_factor, 3.0);
    assert_eq!(e.candidate_factor, 1.0);
    assert!(!e.parent_match);

    // Override via settings; the engine picks it up.
    mmm_hub::settings::set(&app.pool, mmm_hub::settings::ENGINE_SHARED_FACTOR, "7.5")
        .await
        .unwrap();
    mmm_hub::settings::set(&app.pool, mmm_hub::settings::ENGINE_PARENT_MATCH, "1")
        .await
        .unwrap();
    let e = mmm_hub::settings::engine(&app.pool).await;
    assert_eq!(e.shared_factor, 7.5);
    assert!(e.parent_match);

    // The admin page exposes the engine knobs (as an admin).
    sqlx::query("UPDATE hub_users SET is_admin = 1 WHERE id = ?1")
        .bind(app.seed.alice)
        .execute(&app.pool)
        .await
        .unwrap();
    let cookie = app.session_cookie(app.seed.alice).await;
    let html = app
        .client()
        .get(app.url("/admin"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    let html = body(html).await;
    assert!(html.contains("engine_shared_factor"));
    assert!(html.contains("engine_parent_match"));
}

#[tokio::test]
async fn tag_type_set_and_shown_over_http() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let tag = mmm_hub::tags::create_from_playlist(&app.pool, app.seed.alice, app.seed.pl_alice)
        .await
        .unwrap();
    let resp = app
        .client()
        .post(app.url(&format!("/tag/{tag}/type")))
        .header("Cookie", &cookie)
        .form(&[("type", "VIBE")])
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        mmm_hub::tagtypes::get(&app.pool, tag).await,
        Some(mmm_hub::tagtypes::TagType::Vibe)
    );

    // The tag page shows the type.
    let tp = app
        .client()
        .get(app.url(&format!("/tag/{tag}")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert!(body(tp).await.contains("Vibe"));
}

#[tokio::test]
async fn playlist_music_api_controls_and_endpoints() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // Give one playlist track an ISRC so the endpoints do real work.
    sqlx::query("UPDATE hub_tracks SET isrc = 'USABC1234567' WHERE id = ?1")
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();

    // music-api is unconfigured in tests -> controls hidden on the playlist page.
    let page = app
        .client()
        .get(app.url(&format!("/playlist/{}", app.seed.pl_alice)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(page.status(), reqwest::StatusCode::OK);
    assert!(!body(page).await.contains("Fehlende ordern"));

    // Order endpoint redirects (music-api unconfigured -> error flash).
    let order = app
        .client()
        .post(app.url(&format!("/playlist/{}/order", app.seed.pl_alice)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(order.status(), reqwest::StatusCode::SEE_OTHER);

    // Download endpoint: nothing fetchable -> 502 (no files available).
    let dl = app
        .client()
        .get(app.url(&format!("/playlist/{}/download", app.seed.pl_alice)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(dl.status(), reqwest::StatusCode::BAD_GATEWAY);

    // Progress indicator endpoint works.
    let prog = app
        .client()
        .get(app.url(&format!("/playlist/{}/progress", app.seed.pl_alice)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(prog.status(), reqwest::StatusCode::OK);
    assert!(body(prog).await.contains("bereit"));

    // Single-track download: unconfigured music-api -> 502.
    let td = app
        .client()
        .get(app.url(&format!("/track/{}/download", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(td.status(), reqwest::StatusCode::BAD_GATEWAY);

    // Tag download: nothing fetchable -> 502.
    let tag = mmm_hub::tags::create_from_playlist(&app.pool, app.seed.alice, app.seed.pl_alice)
        .await
        .unwrap();
    let tagdl = app
        .client()
        .get(app.url(&format!("/tag/{tag}/download")))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(tagdl.status(), reqwest::StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn playlist_prioritize_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .post(app.url(&format!("/playlist/{}/prioritize", app.seed.pl_alice)))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}

#[tokio::test]
async fn playlist_prioritize_flashes_when_unconfigured() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // Give a playlist track an ISRC so the handler has a non-empty ISRC list.
    sqlx::query("UPDATE hub_tracks SET isrc = 'USABC1234567' WHERE id = ?1")
        .bind(app.seed.t_all)
        .execute(&app.pool)
        .await
        .unwrap();

    let resp = app
        .client()
        .post(app.url(&format!("/playlist/{}/prioritize", app.seed.pl_alice)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    let loc = resp.headers()["location"].to_str().unwrap();
    assert!(
        loc.starts_with(&format!("/playlist/{}", app.seed.pl_alice)),
        "should redirect to the playlist page: {loc}"
    );
    assert!(loc.contains("msg="), "should carry a flash message: {loc}");
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
    assert!(
        html.trim_start().starts_with("<tr"),
        "expected a row fragment"
    );
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
    assert!(
        html.contains("@alice") && html.contains("@bob"),
        "user rows missing"
    );
    // Guard against unrendered askama placeholders leaking as literal text.
    assert!(
        !html.contains("{u."),
        "unrendered askama placeholder leaked"
    );
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
    assert!(
        html.contains("Spotify-Besitzer: Alice"),
        "spotify owner missing"
    );
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
async fn collective_membership_is_recorded() {
    let app = common::spawn().await;
    let col = mmm_hub::tags::create_collective(&app.pool, app.seed.alice, "Crew", "🤝")
        .await
        .unwrap();

    let d = mmm_hub::tags::collective_detail(&app.pool, app.seed.alice, col)
        .await
        .unwrap();
    assert!(d.is_owner);
    assert!(d.members.iter().any(|(s, r)| s == "alice" && r == "owner"));

    // Add bob as a member; he now belongs to the collective.
    mmm_hub::tags::set_collective_member(&app.pool, app.seed.alice, col, "bob", "member")
        .await
        .unwrap();
    let d = mmm_hub::tags::collective_detail(&app.pool, app.seed.bob, col)
        .await
        .unwrap();
    assert!(!d.is_owner);
    assert!(d.members.iter().any(|(s, r)| s == "bob" && r == "member"));
    assert!(
        mmm_hub::tags::collectives_i_belong(&app.pool, app.seed.bob)
            .await
            .iter()
            .any(|(id, _, _)| *id == col)
    );

    // Leaving removes the membership.
    mmm_hub::tags::leave_collective(&app.pool, app.seed.bob, col)
        .await
        .unwrap();
    assert!(
        !mmm_hub::tags::collectives_i_belong(&app.pool, app.seed.bob)
            .await
            .iter()
            .any(|(id, _, _)| *id == col)
    );
}

#[tokio::test]
async fn tag_type_can_be_set_changed_and_cleared() {
    use mmm_hub::tagtypes::{TagType, get as get_type, set as set_type};
    let app = common::spawn().await;

    let tag = mmm_hub::tags::create_from_playlist(&app.pool, app.seed.alice, app.seed.pl_alice)
        .await
        .unwrap();
    assert_eq!(get_type(&app.pool, tag).await, None);

    set_type(&app.pool, app.seed.alice, tag, Some(TagType::Mood))
        .await
        .unwrap();
    assert_eq!(get_type(&app.pool, tag).await, Some(TagType::Mood));

    // Change to a different type.
    set_type(&app.pool, app.seed.alice, tag, Some(TagType::Vibe))
        .await
        .unwrap();
    assert_eq!(get_type(&app.pool, tag).await, Some(TagType::Vibe));

    // Clear it.
    set_type(&app.pool, app.seed.alice, tag, None)
        .await
        .unwrap();
    assert_eq!(get_type(&app.pool, tag).await, None);
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
async fn track_page_shows_tags_with_types() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // Tag from alice's playlist (which contains t_all) with the Mood type.
    let tag = mmm_hub::tags::create_from_playlist(&app.pool, app.seed.alice, app.seed.pl_alice)
        .await
        .unwrap();
    mmm_hub::tagtypes::set(
        &app.pool,
        app.seed.alice,
        tag,
        Some(mmm_hub::tagtypes::TagType::Mood),
    )
    .await
    .unwrap();

    let resp = app
        .client()
        .get(app.url(&format!("/track/{}", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    // Tag name (== playlist name) links to the tag page.
    assert!(html.contains(&format!("/tag/{tag}")));
    assert!(html.contains("Deep House"));
    // Uniform grouped tag cloud: type header (icon + name) is rendered.
    assert!(html.contains("💜 Mood"), "type icon + name must be shown");
    assert!(html.contains("hub-cloud-h"), "grouped cloud header missing");
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
async fn digging_bpm_tolerance_is_absolute() {
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

    // Seed t_pl = 130; its co-occurring candidate t_all = 133 (diff 3).
    for (tid, bpm) in [(app.seed.t_pl, 130.0_f64), (app.seed.t_all, 133.0_f64)] {
        sqlx::query(
            "INSERT INTO hub_track_features (track_id, found, bpm, camelot)
             VALUES (?1, 1, ?2, '8A')",
        )
        .bind(tid)
        .bind(bpm)
        .execute(&app.pool)
        .await
        .unwrap();
    }

    let seed = app.seed.t_pl;
    // tol=0 (exakt) and tol=2 -> the 3-BPM-away candidate is out.
    assert!(
        !fetch(&app, &cookie, &format!("/digging?seed={seed}&bpm=1&tol=0"))
            .await
            .contains("Shared Anthem")
    );
    assert!(
        !fetch(&app, &cookie, &format!("/digging?seed={seed}&bpm=1&tol=2"))
            .await
            .contains("Shared Anthem")
    );
    // tol=3 -> included, and the tolerance selector is rendered.
    let h3 = fetch(&app, &cookie, &format!("/digging?seed={seed}&bpm=1&tol=3")).await;
    assert!(h3.contains("Shared Anthem"));
    assert!(h3.contains("BPM-Toleranz"));
    assert!(h3.contains("±3"));
}

#[tokio::test]
async fn digging_shows_playlists_and_tags_per_track() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    // Promote alice's playlist (contains t_all) to a tag and give it a type.
    let tag = mmm_hub::tags::create_from_playlist(&app.pool, app.seed.alice, app.seed.pl_alice)
        .await
        .unwrap();
    mmm_hub::tagtypes::set(
        &app.pool,
        app.seed.alice,
        tag,
        Some(mmm_hub::tagtypes::TagType::Mood),
    )
    .await
    .unwrap();

    // Seed t_pl co-occurs with t_all in alice's playlist.
    let resp = app
        .client()
        .get(app.url(&format!("/digging?seed={}", app.seed.t_pl)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = body(resp).await;
    // Column headers exist.
    assert!(html.contains("Bei uns"));
    assert!(html.contains("Tags"));
    // The candidate's owning playlist is linked, per user.
    assert!(html.contains(&format!("/playlist/{}", app.seed.pl_alice)));
    assert!(html.contains("@alice"));
    // Tag with its type icon + name.
    assert!(html.contains("💜 Mood"));
}

#[tokio::test]
async fn digging_filters_by_tag_owner_group_and_playlist_owner() {
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

    // alice's playlist (has t_all) becomes a Mood tag.
    let tag = mmm_hub::tags::create_from_playlist(&app.pool, app.seed.alice, app.seed.pl_alice)
        .await
        .unwrap();
    mmm_hub::tagtypes::set(
        &app.pool,
        app.seed.alice,
        tag,
        Some(mmm_hub::tagtypes::TagType::Mood),
    )
    .await
    .unwrap();

    let seed = app.seed.t_pl;
    // Tag owner filter.
    assert!(
        fetch(&app, &cookie, &format!("/digging?seed={seed}&towner=alice"))
            .await
            .contains("Shared Anthem")
    );
    assert!(
        !fetch(
            &app,
            &cookie,
            &format!("/digging?seed={seed}&towner=nobody")
        )
        .await
        .contains("Shared Anthem")
    );
    // Tag type filter.
    assert!(
        fetch(&app, &cookie, &format!("/digging?seed={seed}&ttype=MOOD"))
            .await
            .contains("Shared Anthem")
    );
    // Playlist owner filter: t_all is in alice's playlist, not bob's.
    assert!(
        fetch(&app, &cookie, &format!("/digging?seed={seed}&powner=alice"))
            .await
            .contains("Shared Anthem")
    );
    assert!(
        !fetch(&app, &cookie, &format!("/digging?seed={seed}&powner=bob"))
            .await
            .contains("Shared Anthem")
    );

    // The filter selects are rendered.
    let html = fetch(&app, &cookie, &format!("/digging?seed={seed}")).await;
    assert!(html.contains("name=\"towner\""));
    assert!(html.contains("name=\"ttype\""));
    assert!(html.contains("name=\"powner\""));
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
        mmm_hub::settings::get(&app.pool, "lastfm_api_key")
            .await
            .as_deref(),
        Some("KEY123")
    );
    assert!(mmm_hub::settings::registration_open(&app.pool).await);
}

#[tokio::test]
async fn tags_and_collectives_are_renameable() {
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
