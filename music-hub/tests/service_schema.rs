//! Migration 056: `hub_service_accounts.service` is a slug, not an enumeration.
//!
//! Before: `CHECK (service IN ('spotify','soundcloud','youtube'))` — every new source
//! forced a table rebuild. Now only the *shape* is checked; the allowlist lives in code.

mod common;

use sqlx::SqlitePool;

const TS: &str = "2026-01-01T00:00:00+00:00";

async fn insert_account(pool: &SqlitePool, user_id: i64, service: &str) -> sqlx::Result<i64> {
    sqlx::query_scalar(
        "INSERT INTO hub_service_accounts (user_id, service, access_token, updated_at)
         VALUES (?1, ?2, 'tok', ?3) RETURNING id",
    )
    .bind(user_id)
    .bind(service)
    .bind(TS)
    .fetch_one(pool)
    .await
}

#[tokio::test]
async fn apple_music_is_accepted() {
    let app = common::spawn().await;
    let (user_id, _) = app.user_with_password("svc_apple", "pw").await;

    let id = insert_account(&app.pool, user_id, "apple_music")
        .await
        .expect("apple_music must be insertable after 056");
    assert!(id > 0);
}

#[tokio::test]
async fn any_well_formed_slug_is_accepted() {
    let app = common::spawn().await;
    let (user_id, _) = app.user_with_password("svc_any", "pw").await;

    // The point of 056: a new source must not require a migration.
    for service in ["apple_music", "deezer", "tidal2", "qobuz"] {
        insert_account(&app.pool, user_id, service)
            .await
            .unwrap_or_else(|e| panic!("{service} should be accepted, got {e}"));
    }
}

#[tokio::test]
async fn uppercase_and_empty_are_rejected() {
    let app = common::spawn().await;
    let (user_id, _) = app.user_with_password("svc_bad", "pw").await;

    for bad in [
        "Apple_Music",
        "SPOTIFY",
        "",
        " leading",
        "with-dash",
        "with space",
    ] {
        let res = insert_account(&app.pool, user_id, bad).await;
        assert!(
            res.is_err(),
            "{bad:?} must be rejected by the shape check, but was accepted"
        );
    }
}

#[tokio::test]
async fn unique_user_service_survives() {
    let app = common::spawn().await;
    let (user_id, _) = app.user_with_password("svc_uniq", "pw").await;

    insert_account(&app.pool, user_id, "spotify")
        .await
        .expect("first insert");
    let dup = insert_account(&app.pool, user_id, "spotify").await;
    assert!(
        dup.is_err(),
        "UNIQUE(user_id, service) must still be enforced"
    );
}

#[tokio::test]
async fn legacy_like_columns_survive() {
    let app = common::spawn().await;
    let (user_id, _) = app.user_with_password("svc_legacy", "pw").await;

    let id = insert_account(&app.pool, user_id, "spotify")
        .await
        .expect("insert");
    // The three columns added by 005 must still exist and be writable.
    sqlx::query(
        "UPDATE hub_service_accounts
            SET likes_synced_at = ?2, likes_status = 'ok', likes_error = NULL
          WHERE id = ?1",
    )
    .bind(id)
    .bind(TS)
    .execute(&app.pool)
    .await
    .expect("likes_* columns must be writable");

    let (synced, status): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT likes_synced_at, likes_status FROM hub_service_accounts WHERE id = ?1",
    )
    .bind(id)
    .fetch_one(&app.pool)
    .await
    .expect("read back");
    assert_eq!(synced.as_deref(), Some(TS));
    assert_eq!(status.as_deref(), Some("ok"));
}

#[tokio::test]
async fn cascade_on_user_delete_survives() {
    let app = common::spawn().await;
    let (user_id, _) = app.user_with_password("svc_cascade", "pw").await;
    insert_account(&app.pool, user_id, "apple_music")
        .await
        .expect("insert");

    sqlx::query("DELETE FROM hub_users WHERE id = ?1")
        .bind(user_id)
        .execute(&app.pool)
        .await
        .expect("delete user");

    let left: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM hub_service_accounts WHERE user_id = ?1")
            .bind(user_id)
            .fetch_one(&app.pool)
            .await
            .expect("count");
    assert_eq!(left, 0, "ON DELETE CASCADE must survive the rebuild");
}

#[tokio::test]
async fn spotify_accounts_still_work_end_to_end() {
    let app = common::spawn().await;
    let (user_id, cookie) = app.user_with_password("svc_spotify", "pw").await;

    sqlx::query(
        "INSERT INTO hub_service_accounts
             (user_id, service, remote_user_id, access_token, refresh_token, token_expiry,
              authorized_at, connected_at, updated_at)
         VALUES (?1, 'spotify', 'remote-1', 'at', 'rt', ?2, ?2, ?2, ?2)",
    )
    .bind(user_id)
    .bind(TS)
    .execute(&app.pool)
    .await
    .expect("insert spotify account");

    let resp = app
        .client()
        .get(app.url("/api/hub/me"))
        .header("Cookie", &cookie)
        .send()
        .await
        .expect("GET /api/hub/me");
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = resp.json().await.expect("json");
    let services = body["data"]["services"].as_array().expect("services array");
    assert_eq!(services.len(), 1, "spotify account must survive: {body}");
    assert_eq!(services[0]["service"], "spotify");
}
