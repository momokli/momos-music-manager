//! Integration tests for the typed settings registry + validation (#217).
//!
//! Everything the ranking engine reads must be declared in `settings::SETTINGS`
//! with a default (and a range, for numbers), validated on write, and editable
//! in the admin UI.

mod common;

use mmm_hub::settings::{self, SETTINGS, SettingKind};

#[test]
fn registry_is_well_formed_and_covers_the_engine() {
    // No duplicate keys.
    let mut keys: Vec<&str> = SETTINGS.iter().map(|s| s.key).collect();
    let n = keys.len();
    keys.sort_unstable();
    keys.dedup();
    assert_eq!(keys.len(), n, "duplicate setting keys in registry");

    // Every engine parameter is registered with a default; numbers have a range.
    for key in [
        settings::ENGINE_SHARED_FACTOR,
        settings::ENGINE_CANDIDATE_FACTOR,
        settings::ENGINE_BASE_USERS,
        settings::ENGINE_BASE_PLAYLISTS,
        settings::ENGINE_BASE_LIKES,
        settings::ENGINE_BASE_SOURCES,
        settings::ENGINE_TAG_WEIGHT,
        settings::ENGINE_META_WEIGHT,
        settings::ENGINE_TRAK_WEIGHT,
        settings::ENGINE_TAG_POINTS,
        settings::ENGINE_TRAK_PLAYCOUNT_CAP,
        settings::ENGINE_TAG_OVERLAP_BASE,
        settings::ENGINE_CROSS_GROUP_BONUS,
        settings::ENGINE_SIM_FACTOR,
        settings::ENGINE_RIPENESS_FACTOR,
        settings::ENGINE_META_TITLE,
        settings::ENGINE_META_ARTIST,
        settings::ENGINE_META_ALBUM,
        settings::ENGINE_META_COVER,
        settings::ENGINE_META_BPM,
        settings::ENGINE_META_KEY,
        settings::ENGINE_META_GENRE,
        settings::ENGINE_COOC_METRIC,
        settings::ENGINE_REC_TAG,
        settings::ENGINE_REC_PLAYLIST,
        settings::ENGINE_REC_ARTIST,
        settings::ENGINE_REC_ALBUM,
    ] {
        let s = settings::spec(key).unwrap_or_else(|| panic!("missing spec for {key}"));
        assert!(!s.default.is_empty(), "{key} has no default");
        if matches!(s.kind, SettingKind::Num) {
            assert!(
                s.min.is_some() && s.max.is_some(),
                "{key} is numeric but has no range"
            );
        }
    }
}

#[test]
fn validation_enforces_ranges_and_types() {
    let sf = settings::spec(settings::ENGINE_SHARED_FACTOR).unwrap();
    assert_eq!(settings::validate(sf, " 3.5 ").unwrap(), "3.5");
    assert!(
        settings::validate(sf, "abc").is_err(),
        "non-numeric accepted"
    );
    assert!(settings::validate(sf, "-1").is_err(), "below min accepted");
    assert!(
        settings::validate(sf, "1000").is_err(),
        "above max accepted"
    );

    let tp = settings::spec(settings::ENGINE_TAG_POINTS).unwrap();
    assert!(settings::validate(tp, "100, 50, 1").is_ok());
    assert!(settings::validate(tp, "100,x").is_err(), "bad CSV accepted");
    assert!(settings::validate(tp, "  ").is_err(), "empty CSV accepted");

    let pm = settings::spec(settings::ENGINE_PARENT_MATCH).unwrap();
    assert_eq!(settings::validate(pm, "on").unwrap(), "1");
    assert_eq!(settings::validate(pm, "false").unwrap(), "0");

    let cm = settings::spec(settings::ENGINE_COOC_METRIC).unwrap();
    assert_eq!(settings::validate(cm, "jaccard").unwrap(), "jaccard");
    assert!(
        settings::validate(cm, "nonsense").is_err(),
        "bad enum accepted"
    );
}

/// Changing a registered weight actually changes results.
#[tokio::test]
async fn changing_a_meta_weight_changes_ripeness() {
    let app = common::spawn().await;
    let e0 = settings::engine(&app.pool).await;
    let base = mmm_hub::scoring::ripeness(&app.pool, app.seed.t_all, &e0)
        .await
        .meta_score;
    // t_all has title + artist -> at least 2.0 with the default weights.
    assert!(base >= 2.0, "unexpected baseline meta score {base}");

    // Bump the artist weight from 1 -> 5: the (present) artist field adds +4.
    settings::set(&app.pool, settings::ENGINE_META_ARTIST, "5")
        .await
        .unwrap();
    let e1 = settings::engine(&app.pool).await;
    let bumped = mmm_hub::scoring::ripeness(&app.pool, app.seed.t_all, &e1)
        .await
        .meta_score;
    assert!(
        (bumped - base - 4.0).abs() < 1e-6,
        "artist weight bump must add 4.0: {base} -> {bumped}"
    );
}

/// The admin UI renders typed widgets with the range hint.
#[tokio::test]
async fn admin_page_renders_typed_engine_inputs() {
    let app = common::spawn().await;
    sqlx::query("UPDATE hub_users SET is_admin = 1 WHERE id = ?1")
        .bind(app.seed.alice)
        .execute(&app.pool)
        .await
        .unwrap();
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/admin"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let html = resp.text().await.unwrap();
    assert!(
        html.contains("name=\"engine_shared_factor\""),
        "engine field missing"
    );
    assert!(html.contains("type=\"number\""), "numeric widget missing");
    assert!(html.contains("Default"), "range/default hint missing");
    assert!(
        html.contains("name=\"engine_cooc_metric\""),
        "co-occurrence metric select missing"
    );
}
