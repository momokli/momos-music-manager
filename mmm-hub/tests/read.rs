//! Integration tests for the JSON read endpoints (issue #142).
//!
//! Covers `/api/hub/users`, `/api/hub/tracks/{id}`, `/api/hub/overlap` and the
//! guarded `/api/hub/query` SQL console. Expectations are derived from the
//! deterministic fixtures in `src/db/testing.rs` and the overlap views in
//! `migrations/001_hub_schema.sql`.

mod common;

use serde_json::json;

/// `GET /api/hub/users` → three users, sorted by id, with the three slugs.
#[tokio::test]
async fn users_lists_three_sorted_slugs() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/api/hub/users"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let v: serde_json::Value = resp.json().await.unwrap();
    let users = v["data"].as_array().expect("data is an array");
    assert_eq!(users.len(), 3, "three seeded users");

    let slugs: Vec<&str> = users
        .iter()
        .map(|u| u["slug"].as_str().unwrap())
        .collect();
    assert_eq!(slugs, vec!["alice", "bob", "carol"]);

    let ids: Vec<i64> = users.iter().map(|u| u["id"].as_i64().unwrap()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "users are sorted by id");
}

/// `GET /api/hub/tracks/{t_all}` → the shared track plus per-user presence:
/// likes from alice/bob/carol and an extra alice entry via the "Deep House"
/// playlist. Liked-only rows never carry a `playlistName`.
#[tokio::test]
async fn track_detail_reports_presence() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url(&format!("/api/hub/tracks/{}", app.seed.t_all)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let v: serde_json::Value = resp.json().await.unwrap();
    let track = &v["data"]["track"];
    assert_eq!(track["service"], "spotify");
    assert_eq!(track["serviceTrackId"], "t_all");
    assert_eq!(track["title"], "Shared Anthem");

    let presence = v["data"]["presence"]
        .as_array()
        .expect("presence is an array");

    // Every "liked" row has no playlist name.
    for p in presence {
        if p["source"] == "liked" {
            assert!(
                p["playlistName"].is_null(),
                "liked presence must not have a playlistName: {p}"
            );
        }
    }

    // alice liked it.
    assert!(
        presence.iter().any(|p| p["user"] == "alice" && p["source"] == "liked"),
        "alice liked t_all"
    );
    // alice also has it via her "Deep House" playlist.
    assert!(
        presence
            .iter()
            .any(|p| p["user"] == "alice"
                && p["source"] == "playlist"
                && p["playlistName"] == "Deep House"),
        "alice has t_all in the Deep House playlist"
    );
    // bob + carol liked it.
    assert!(
        presence.iter().any(|p| p["user"] == "bob" && p["source"] == "liked"),
        "bob liked t_all"
    );
    assert!(
        presence.iter().any(|p| p["user"] == "carol" && p["source"] == "liked"),
        "carol liked t_all"
    );

    // Exactly 4 contributions: 3 likes + 1 playlist membership.
    assert_eq!(presence.len(), 4, "3 likes + 1 playlist membership");
}

/// A track that is present only via likes has no `playlistName` anywhere.
#[tokio::test]
async fn track_present_only_via_likes_has_no_playlist_name() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url(&format!("/api/hub/tracks/{}", app.seed.t_two)))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["data"]["track"]["serviceTrackId"], "t_two");

    let presence = v["data"]["presence"].as_array().unwrap();
    assert_eq!(presence.len(), 2, "alice + bob liked t_two");
    for p in presence {
        assert_eq!(p["source"], "liked");
        assert!(p["playlistName"].is_null(), "no playlistName on a liked-only track");
    }
}

/// `GET /api/hub/tracks/999999` → 404 with an `error` body.
#[tokio::test]
async fn track_detail_missing_is_404() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/api/hub/tracks/999999"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::NOT_FOUND);

    let v: serde_json::Value = resp.json().await.unwrap();
    assert!(v["error"].is_string(), "404 body carries an error message");
}

/// `GET /api/hub/overlap` → shared tracks and pairwise counts.
#[tokio::test]
async fn overlap_reports_shared_tracks_and_pairs() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .get(app.url("/api/hub/overlap"))
        .header("Cookie", &cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let v: serde_json::Value = resp.json().await.unwrap();

    let shared = v["data"]["shared"].as_array().unwrap();
    assert_eq!(shared.len(), 3, "t_all, t_two, t_pl are shared");

    // Helper: userCount for a given track id.
    let count_for = |track_id: i64| -> i64 {
        shared
            .iter()
            .find(|s| s["trackId"] == track_id)
            .unwrap_or_else(|| panic!("track {track_id} missing from shared"))["userCount"]
            .as_i64()
            .unwrap()
    };
    assert_eq!(count_for(app.seed.t_all), 3);
    assert_eq!(count_for(app.seed.t_two), 2);
    assert_eq!(count_for(app.seed.t_pl), 2);

    let pairs = v["data"]["pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 3, "three unordered user pairs");

    let shared_for = |a: &str, b: &str| -> i64 {
        pairs
            .iter()
            .find(|p| p["userA"] == a && p["userB"] == b)
            .unwrap_or_else(|| panic!("pair {a}/{b} missing"))["sharedTracks"]
            .as_i64()
            .unwrap()
    };
    assert_eq!(shared_for("alice", "bob"), 3);
    assert_eq!(shared_for("alice", "carol"), 1);
    assert_eq!(shared_for("bob", "carol"), 1);
}

/// `POST /api/hub/query` without a session → 401 with an `error` body.
#[tokio::test]
async fn query_requires_session() {
    let app = common::spawn().await;

    let resp = app
        .client()
        .post(app.url("/api/hub/query"))
        .json(&json!({ "sql": "SELECT 1 AS one" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);

    let v: serde_json::Value = resp.json().await.unwrap();
    assert!(v["error"].is_string(), "401 body carries an error message");
}

/// `POST /api/hub/query` with a session + a valid `SELECT` → columns + rows.
#[tokio::test]
async fn query_with_session_returns_rows() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .post(app.url("/api/hub/query"))
        .header("Cookie", &cookie)
        .json(&json!({ "sql": "SELECT id, slug FROM hub_users ORDER BY id" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);

    let v: serde_json::Value = resp.json().await.unwrap();
    let columns = v["data"]["columns"].as_array().expect("columns array");
    assert_eq!(columns, &vec![json!("id"), json!("slug")]);

    let rows = v["data"]["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 3);
}

/// `POST /api/hub/query` rejects non-read statements with 400, and the DB is
/// left intact afterwards.
#[tokio::test]
async fn query_rejects_non_read_and_keeps_db_intact() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;

    let resp = app
        .client()
        .post(app.url("/api/hub/query"))
        .header("Cookie", &cookie)
        .json(&json!({ "sql": "DROP TABLE hub_tracks" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::BAD_REQUEST);

    let v: serde_json::Value = resp.json().await.unwrap();
    assert!(v["error"].is_string(), "400 body carries an error message");

    // The table must still exist and be queryable.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM hub_tracks")
        .fetch_one(&app.pool)
        .await
        .expect("hub_tracks still queryable after rejected statement");
    assert_eq!(count, 6, "all six seeded tracks survived");
}
