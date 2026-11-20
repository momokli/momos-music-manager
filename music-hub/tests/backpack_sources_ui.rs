//! Route-level integration tests for the backpack sources UI (#651, Slice B1).
//!
//! Drives the three new mutating routes against the real Axum router (`common`
//! harness): add per source kind, delete a row, clear all, the foreign-backpack
//! `404` on every route, and the idempotent duplicate add.

mod common;

use music_hub::backpacks::SourceRef;

/// Insert a tag owned by `owner` and (optionally) link it to `track_id` so it
/// resolves to a non-empty track set.
async fn insert_tag_and_link(
    pool: &sqlx::SqlitePool,
    owner: i64,
    slug: &str,
    track_id: Option<i64>,
) -> i64 {
    let tag: i64 = sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at)
         VALUES (?1, ?2, ?2, '2026-01-01T00:00:00+00:00') RETURNING id",
    )
    .bind(owner)
    .bind(slug)
    .fetch_one(pool)
    .await
    .expect("insert tag");
    if let Some(t) = track_id {
        sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
            .bind(t)
            .bind(tag)
            .execute(pool)
            .await
            .expect("link tag");
    }
    tag
}

async fn insert_backpack(pool: &sqlx::SqlitePool, user_id: i64, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_backpacks (user_id, name, is_active, created_at, updated_at)
         VALUES (?1, ?2, 0, '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')
         RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("insert backpack")
}

/// Insert a `hub_playlists` row owned by `user_id` plus its memberships, and
/// return the new id. This is the id space the resolver/label read and the
/// dropdown must offer — unlike `app.seed.pl_alice` (owned by `alice`).
async fn insert_playlist(
    pool: &sqlx::SqlitePool,
    user_id: i64,
    name: &str,
    tracks: &[i64],
) -> i64 {
    let pid = music_hub::spotify::random_token();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO hub_playlists
            (user_id, service, playlist_id, name, is_liked, items_available,
             is_owned, owner_name, collaborative, fetched_at)
         VALUES (?1, 'spotify', ?2, ?3, 0, 1, 1, 'Tester', 0, '2026-01-01T00:00:00+00:00')
         RETURNING id",
    )
    .bind(user_id)
    .bind(&pid)
    .bind(name)
    .fetch_one(pool)
    .await
    .expect("insert playlist");
    for (pos, t) in tracks.iter().enumerate() {
        sqlx::query(
            "INSERT INTO hub_playlist_tracks (playlist_id, track_id, position, added_at)
             VALUES (?1, ?2, ?3, '2026-01-01T00:00:00+00:00')",
        )
        .bind(id)
        .bind(t)
        .bind(pos as i64)
        .execute(pool)
        .await
        .expect("insert playlist membership");
    }
    id
}

/// The `value="…"` attributes of the `<option>`s inside the rendered
/// `<select name="source_playlist_id">` of the `/backpacks` HTML (empty
/// placeholder included).
fn playlist_option_values(html: &str) -> Vec<String> {
    let start = html
        .find("name=\"source_playlist_id\"")
        .expect("playlist select rendered");
    let rest = &html[start..];
    let end = rest.find("</select>").expect("select close tag");
    let block = &rest[..end];
    let mut values = Vec::new();
    let mut idx = 0;
    while let Some(p) = block[idx..].find("value=\"") {
        let s = idx + p + "value=\"".len();
        let e = block[s..].find('"').expect("closing quote") + s;
        values.push(block[s..e].to_string());
        idx = e;
    }
    values
}

async fn item_count(pool: &sqlx::SqlitePool, backpack_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM hub_backpack_items WHERE backpack_id = ?1")
        .bind(backpack_id)
        .fetch_one(pool)
        .await
        .expect("count items")
}

async fn post_form(
    app: &common::TestApp,
    path: &str,
    cookie: &str,
    form: &[(&str, String)],
) -> reqwest::Response {
    app.client()
        .post(app.url(path))
        .header("Cookie", cookie)
        .form(form)
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn add_tag_adds_and_resolves() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("ui-tag", "pw").await;
    let bp = insert_backpack(&app.pool, owner, "Tag-Backpack").await;
    let tag = insert_tag_and_link(&app.pool, owner, "dark", Some(app.seed.t_all)).await;

    let resp = post_form(
        &app,
        &format!("/backpacks/{bp}/sources"),
        &cookie,
        &[("source_kind", "tag".into()), ("source_tag_id", tag.to_string())],
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    let sources = music_hub::backpacks::list_sources(&app.pool, bp).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_kind, "tag");
    let count = music_hub::backpacks::resolve_count(&app.pool, bp).await.unwrap();
    assert!(count > 0, "tag source must resolve at least one track");
}

#[tokio::test]
async fn add_playlist_adds_and_resolves() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("ui-pl", "pw").await;
    let bp = insert_backpack(&app.pool, owner, "Playlist-Backpack").await;
    // A playlist the caller owns, in the picker's id space (`hub_playlists`).
    let pl = insert_playlist(
        &app.pool,
        owner,
        "Testliste",
        &[app.seed.t_all, app.seed.t_two],
    )
    .await;

    // Use the value the *rendered* dropdown offers, not an internal id: this is
    // exactly what a real browser submits.
    let html = app
        .client()
        .get(app.url("/backpacks"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let options = playlist_option_values(&html);
    let value = options
        .iter()
        .find(|v| **v == pl.to_string())
        .unwrap_or_else(|| {
            panic!("dropdown must offer the caller's playlist {pl}, got {options:?}")
        })
        .clone();

    let resp = post_form(
        &app,
        &format!("/backpacks/{bp}/sources"),
        &cookie,
        &[
            ("source_kind", "playlist".into()),
            ("source_playlist_id", value),
        ],
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    let sources = music_hub::backpacks::list_sources(&app.pool, bp).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_kind, "playlist");
    assert_eq!(sources[0].source_playlist_id, Some(pl));

    // The label resolves against `hub_playlists` with the same id.
    let views = music_hub::backpacks::list_source_views(&app.pool, bp).await.unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].label, "Testliste");

    // And the source resolves to exactly the inserted membership tracks.
    let count = music_hub::backpacks::resolve_count(&app.pool, bp).await.unwrap();
    assert_eq!(count, 2);
}

/// Regression (#651 review blocker): the picker must offer only the caller's
/// `hub_playlists` — the same id space the resolver/label use. A
/// `hub_curated_playlists` row (the old, wrong source) and a foreign user's
/// playlist must never appear as an option.
#[tokio::test]
async fn playlist_picker_offers_only_own_hub_playlists() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("ui-picker", "pw").await;
    let _bp = insert_backpack(&app.pool, owner, "Picker-Backpack").await;

    let mine = insert_playlist(&app.pool, owner, "Meine Liste", &[app.seed.t_all]).await;
    // A curated playlist for the same user lives in a different id space and
    // must NOT be offered.
    let curated: i64 = sqlx::query_scalar(
        "INSERT INTO hub_curated_playlists
            (user_id, name, description, is_public, fill_mode, fill_limit, created_at, updated_at)
         VALUES (?1, 'Curated Liste', '', 0, 'manual', 50, '2026-01-01T00:00:00+00:00', '2026-01-01T00:00:00+00:00')
         RETURNING id",
    )
    .bind(owner)
    .fetch_one(&app.pool)
    .await
    .expect("insert curated playlist");
    // Another user's playlist must not leak either.
    let foreign = insert_playlist(&app.pool, app.seed.alice, "Fremde Liste", &[app.seed.t_all]).await;

    let html = app
        .client()
        .get(app.url("/backpacks"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let values = playlist_option_values(&html);
    assert!(values.contains(&mine.to_string()), "own playlist offered: {values:?}");
    assert!(
        !values.contains(&curated.to_string()),
        "curated playlist must not appear: {values:?}"
    );
    assert!(
        !values.contains(&foreign.to_string()),
        "foreign playlist must not appear: {values:?}"
    );
    // Only the caller's `hub_playlists` ids appear (placeholder + mine).
    let nonempty: Vec<&String> = values.iter().filter(|v| !v.is_empty()).collect();
    assert_eq!(nonempty, vec![&mine.to_string()]);
}

#[tokio::test]
async fn add_lens_adds_row() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("ui-lens", "pw").await;
    let bp = insert_backpack(&app.pool, owner, "Lens-Backpack").await;

    let resp = post_form(
        &app,
        &format!("/backpacks/{bp}/sources"),
        &cookie,
        &[("source_kind", "lens".into()), ("source_lens", "newest".into())],
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    let sources = music_hub::backpacks::list_sources(&app.pool, bp).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_kind, "lens");
    assert_eq!(sources[0].source_lens.as_deref(), Some("newest"));
}

#[tokio::test]
async fn add_track_adds_and_resolves() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("ui-track", "pw").await;
    let bp = insert_backpack(&app.pool, owner, "Track-Backpack").await;

    let resp = post_form(
        &app,
        &format!("/backpacks/{bp}/sources"),
        &cookie,
        &[
            ("source_kind", "track".into()),
            ("source_id", app.seed.t_all.to_string()),
        ],
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);

    let sources = music_hub::backpacks::list_sources(&app.pool, bp).await.unwrap();
    assert_eq!(sources.len(), 1);
    assert_eq!(sources[0].source_kind, "track");
    assert_eq!(sources[0].source_track_id, Some(app.seed.t_all));
    assert_eq!(
        music_hub::backpacks::resolve_count(&app.pool, bp).await.unwrap(),
        1
    );
}

#[tokio::test]
async fn delete_source_removes_row() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("ui-del", "pw").await;
    let bp = insert_backpack(&app.pool, owner, "Del-Backpack").await;
    let item = music_hub::backpacks::add_source(&app.pool, bp, SourceRef::Track(app.seed.t_all))
        .await
        .unwrap();

    let resp = post_form(
        &app,
        &format!("/backpacks/{bp}/sources/{item}/delete"),
        &cookie,
        &[],
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(item_count(&app.pool, bp).await, 0);
}

#[tokio::test]
async fn clear_removes_all() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("ui-clear", "pw").await;
    let bp = insert_backpack(&app.pool, owner, "Clear-Backpack").await;
    music_hub::backpacks::add_source(&app.pool, bp, SourceRef::Track(app.seed.t_all))
        .await
        .unwrap();
    music_hub::backpacks::add_source(&app.pool, bp, SourceRef::Track(app.seed.t_two))
        .await
        .unwrap();
    assert_eq!(item_count(&app.pool, bp).await, 2);

    let resp = post_form(&app, &format!("/backpacks/{bp}/clear"), &cookie, &[]).await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(item_count(&app.pool, bp).await, 0);
}

#[tokio::test]
async fn foreign_backpack_404_on_every_route() {
    let app = common::spawn().await;
    let (owner, _owner_cookie) = app.user_with_password("ui-foreign-owner", "pw").await;
    let (_caller, caller_cookie) = app.user_with_password("ui-foreign-caller", "pw").await;
    let bp = insert_backpack(&app.pool, owner, "Fremd").await;
    let item = music_hub::backpacks::add_source(&app.pool, bp, SourceRef::Track(app.seed.t_all))
        .await
        .unwrap();

    let add = post_form(
        &app,
        &format!("/backpacks/{bp}/sources"),
        &caller_cookie,
        &[
            ("source_kind", "track".into()),
            ("source_id", app.seed.t_two.to_string()),
        ],
    )
    .await;
    assert_eq!(add.status(), reqwest::StatusCode::NOT_FOUND, "add must 404");

    let del = post_form(
        &app,
        &format!("/backpacks/{bp}/sources/{item}/delete"),
        &caller_cookie,
        &[],
    )
    .await;
    assert_eq!(del.status(), reqwest::StatusCode::NOT_FOUND, "delete must 404");

    let clear = post_form(&app, &format!("/backpacks/{bp}/clear"), &caller_cookie, &[]).await;
    assert_eq!(clear.status(), reqwest::StatusCode::NOT_FOUND, "clear must 404");

    // The owner's backpack is untouched by the foreign caller.
    assert_eq!(item_count(&app.pool, bp).await, 1);
}

#[tokio::test]
async fn cross_backpack_item_delete_is_404() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("ui-xbp", "pw").await;
    let bp_a = insert_backpack(&app.pool, owner, "A").await;
    let bp_b = insert_backpack(&app.pool, owner, "B").await;
    // An item that belongs to backpack B …
    let item_b = music_hub::backpacks::add_source(&app.pool, bp_b, SourceRef::Track(app.seed.t_all))
        .await
        .unwrap();

    // … must not be deletable through backpack A's delete route.
    let resp = post_form(
        &app,
        &format!("/backpacks/{bp_a}/sources/{item_b}/delete"),
        &cookie,
        &[],
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);
    assert_eq!(item_count(&app.pool, bp_b).await, 1, "B's item survives");
    let _ = bp_a;
}

#[tokio::test]
async fn duplicate_source_is_idempotent() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("ui-dup", "pw").await;
    let bp = insert_backpack(&app.pool, owner, "Dup-Backpack").await;

    let form = [
        ("source_kind", "track".to_string()),
        ("source_id", app.seed.t_all.to_string()),
    ];
    let first = post_form(&app, &format!("/backpacks/{bp}/sources"), &cookie, &form).await;
    assert_eq!(first.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(item_count(&app.pool, bp).await, 1);

    let second = post_form(&app, &format!("/backpacks/{bp}/sources"), &cookie, &form).await;
    assert_eq!(second.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(
        item_count(&app.pool, bp).await,
        1,
        "a duplicate target must not insert a second row"
    );
}

#[tokio::test]
async fn unknown_source_kind_flashes_and_writes_nothing() {
    let app = common::spawn().await;
    let (owner, cookie) = app.user_with_password("ui-bad", "pw").await;
    let bp = insert_backpack(&app.pool, owner, "Bad-Backpack").await;

    let resp = post_form(
        &app,
        &format!("/backpacks/{bp}/sources"),
        &cookie,
        &[("source_kind", "bogus".into()), ("source_id", "1".into())],
    )
    .await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(item_count(&app.pool, bp).await, 0);
}

/// Regression (#651 review blocker): a real browser form submit serialises
/// **every** field, empties as `key=`. A bare `Option<i64>` rejects "" with
/// "cannot parse integer from empty string", so the whole form extractor 422s
/// every real submission. These tests post the full body, empty fields
/// included, and demand a 303 with the correct effect.
mod real_form_serialization {
    use super::*;

    /// Full body shape of a real submit for `kind`, mirroring the <form> fields
    /// in `backpacks.html`: all four pickers + the two numeric ids are always
    /// present, unused ones empty.
    fn real_body(kind: &str, tag: &str, playlist: &str, lens: &str, id: &str) -> Vec<(&'static str, String)> {
        vec![
            ("source_kind", kind.to_string()),
            ("source_tag_id", tag.to_string()),
            ("source_playlist_id", playlist.to_string()),
            ("source_lens", lens.to_string()),
            ("source_id", id.to_string()),
        ]
    }

    #[tokio::test]
    async fn tag_full_body_with_empty_fields_is_303() {
        let app = common::spawn().await;
        let (owner, cookie) = app.user_with_password("ui-real-tag", "pw").await;
        let bp = insert_backpack(&app.pool, owner, "Real-Tag").await;
        let tag = insert_tag_and_link(&app.pool, owner, "real", Some(app.seed.t_all)).await;

        let body = real_body("tag", &tag.to_string(), "", "", "");
        let resp = post_form(&app, &format!("/backpacks/{bp}/sources"), &cookie, &body).await;
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::SEE_OTHER,
            "empty sibling fields must not 422 the real tag submit"
        );
        let sources = music_hub::backpacks::list_sources(&app.pool, bp).await.unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_kind, "tag");
    }

    #[tokio::test]
    async fn playlist_full_body_with_empty_fields_is_303() {
        let app = common::spawn().await;
        let (owner, cookie) = app.user_with_password("ui-real-pl", "pw").await;
        let bp = insert_backpack(&app.pool, owner, "Real-Pl").await;
        let pl = insert_playlist(&app.pool, owner, "Real Liste", &[app.seed.t_all]).await;

        let body = real_body("playlist", "", &pl.to_string(), "", "");
        let resp = post_form(&app, &format!("/backpacks/{bp}/sources"), &cookie, &body).await;
        assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
        let sources = music_hub::backpacks::list_sources(&app.pool, bp).await.unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_kind, "playlist");
        assert_eq!(sources[0].source_playlist_id, Some(pl));
    }

    #[tokio::test]
    async fn lens_full_body_with_empty_fields_is_303() {
        let app = common::spawn().await;
        let (owner, cookie) = app.user_with_password("ui-real-lens", "pw").await;
        let bp = insert_backpack(&app.pool, owner, "Real-Lens").await;

        let body = real_body("lens", "", "", "newest", "");
        let resp = post_form(&app, &format!("/backpacks/{bp}/sources"), &cookie, &body).await;
        assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
        let sources = music_hub::backpacks::list_sources(&app.pool, bp).await.unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_lens.as_deref(), Some("newest"));
    }

    #[tokio::test]
    async fn track_full_body_with_empty_fields_is_303() {
        let app = common::spawn().await;
        let (owner, cookie) = app.user_with_password("ui-real-track", "pw").await;
        let bp = insert_backpack(&app.pool, owner, "Real-Track").await;

        let body = real_body("track", "", "", "", &app.seed.t_all.to_string());
        let resp = post_form(&app, &format!("/backpacks/{bp}/sources"), &cookie, &body).await;
        assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
        let sources = music_hub::backpacks::list_sources(&app.pool, bp).await.unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source_track_id, Some(app.seed.t_all));
    }

    /// An empty `source_lens` on a lens submit is "Linse fehlt": a flash, not a
    /// 422 and not an insert.
    #[tokio::test]
    async fn lens_empty_value_flashes_and_writes_nothing() {
        let app = common::spawn().await;
        let (owner, cookie) = app.user_with_password("ui-real-lens-empty", "pw").await;
        let bp = insert_backpack(&app.pool, owner, "Real-Lens-Empty").await;

        let body = real_body("lens", "", "", "", "");
        let resp = post_form(&app, &format!("/backpacks/{bp}/sources"), &cookie, &body).await;
        assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
        assert_eq!(item_count(&app.pool, bp).await, 0);
    }
}

#[tokio::test]
async fn sources_route_requires_login() {
    let app = common::spawn().await;
    let resp = post_form(&app, "/backpacks/1/sources", "", &[("source_kind", "lens".into())]).await;
    assert_eq!(resp.status(), reqwest::StatusCode::SEE_OTHER);
    assert_eq!(resp.headers()["location"], "/login");
}
