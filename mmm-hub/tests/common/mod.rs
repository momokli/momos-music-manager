//! Shared integration-test harness.
//!
//! Boots the real Axum router against a fresh temp SQLite DB (all migrations
//! run), seeds the deterministic fixtures from [`mmm_hub::db::testing`], and
//! serves it on an ephemeral loopback port so tests can drive it with `reqwest`.
//!
//! Each test file has `mod common;` and calls [`spawn`].

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use mmm_hub::api::AppState;
use mmm_hub::config::Config;
use mmm_hub::db::testing::Seed;
use sqlx::SqlitePool;

pub struct TestApp {
    pub base: String,
    pub pool: SqlitePool,
    pub seed: Seed,
    client: reqwest::Client,
    _dir: tempfile::TempDir,
}

/// A `reqwest` client that does not follow redirects (so tests can read
/// `Set-Cookie` off a 303) and never times out mid-test.
fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("build reqwest client")
}

impl TestApp {
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Insert a server-side session for `user_id` and return the `Cookie:`
    /// header value to authenticate follow-up requests.
    pub async fn session_cookie(&self, user_id: i64) -> String {
        let token = mmm_hub::spotify::random_token();
        sqlx::query(
            "INSERT INTO hub_web_sessions (id, user_id, created_at, expires_at)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&token)
        .bind(user_id)
        .bind("2026-01-01T00:00:00+00:00")
        .bind("2099-01-01T00:00:00+00:00")
        .execute(&self.pool)
        .await
        .expect("insert session");
        format!("hub_session={token}")
    }

    /// Create a user with a known password and return `(user_id, cookie)`.
    pub async fn user_with_password(&self, slug: &str, password: &str) -> (i64, String) {
        let hash = bcrypt::hash(password, 4).expect("bcrypt hash");
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO hub_users (slug, display_name, password_hash, created_at)
             VALUES (?1, ?1, ?2, ?3) RETURNING id",
        )
        .bind(slug)
        .bind(&hash)
        .bind("2026-01-01T00:00:00+00:00")
        .fetch_one(&self.pool)
        .await
        .expect("insert user");
        let cookie = self.session_cookie(id).await;
        (id, cookie)
    }
}

/// Boot a fresh hub instance with seeded fixture data.
pub async fn spawn() -> TestApp {
    let dir = tempfile::tempdir().expect("tempdir");
    let url = format!("sqlite:{}/hub.db", dir.path().display());

    let pool = mmm_hub::db::connect(&url).await.expect("connect + migrate");
    let ro_pool = mmm_hub::db::connect_readonly(&url)
        .await
        .expect("connect readonly");
    let seed = mmm_hub::db::testing::seed(&pool)
        .await
        .expect("seed fixtures");

    let cfg = Arc::new(Config::for_test(url.clone()));
    let state = AppState {
        pool: pool.clone(),
        ro_pool,
        cfg,
        oauth_states: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = mmm_hub::api::router(state.clone())
        .merge(mmm_hub::web::router(state.clone()))
        .merge(mmm_hub::pages::router(state.clone()));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local addr");
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    TestApp {
        base: format!("http://{addr}"),
        pool,
        seed,
        client: http(),
        _dir: dir,
    }
}
