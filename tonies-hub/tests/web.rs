//! Router + DB integration tests (tempfile DB, no process start).

use axum::body::Body;
use axum::http::{HeaderMap, Method, Request, StatusCode, header};
use axum::Router;
use http_body_util::BodyExt;
use sqlx::{Row, SqlitePool};
use tower::ServiceExt;

use tonies_hub::config::Config;
use tonies_hub::db;
use tonies_hub::web::{self, AppState};

struct TestApp {
    _dir: tempfile::TempDir,
    router: Router,
    pool: SqlitePool,
}

async fn setup() -> TestApp {
    let dir = tempfile::tempdir().unwrap();
    let url = format!("sqlite:{}", dir.path().join("tonies.db").display());
    let pool = db::connect(&url).await.unwrap();

    let hash = bcrypt::hash("pw", 10).unwrap();
    sqlx::query(
        "INSERT INTO users (slug, display_name, password_hash, is_admin, created_at)
         VALUES ('demo', 'Demo', ?1, 0, ?2)",
    )
    .bind(&hash)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(&pool)
    .await
    .unwrap();

    let state = AppState {
        pool: pool.clone(),
        config: std::sync::Arc::new(Config::for_test(&url)),
    };
    let router = web::router(state);
    TestApp {
        _dir: dir,
        router,
        pool,
    }
}

async fn send(router: &Router, req: Request<Body>) -> axum::response::Response {
    router.clone().oneshot(req).await.unwrap()
}

fn get(uri: &str) -> Request<Body> {
    Request::builder().method(Method::GET).uri(uri).body(Body::empty()).unwrap()
}

fn get_with_cookie(uri: &str, cookie: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap()
}

fn login_request(user: &str, pw: &str) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/login")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("username={user}&password={pw}")))
        .unwrap()
}

fn set_cookie(resp: &axum::response::Response) -> Option<String> {
    resp.headers()
        .get(header::SET_COOKIE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

async fn body_string(resp: axum::response::Response) -> String {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8_lossy(&bytes).to_string()
}

fn session_token(cookie: &str) -> String {
    cookie
        .split(';')
        .next()
        .unwrap()
        .trim()
        .strip_prefix("tonies_session=")
        .unwrap()
        .to_string()
}

async fn login(app: &TestApp) -> String {
    let resp = send(&app.router, login_request("demo", "pw")).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let cookie = set_cookie(&resp).expect("Set-Cookie fehlt");
    session_token(&cookie)
}

// ── migrations ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn migrations_create_all_tables() {
    let app = setup().await;
    for table in [
        "users",
        "sessions",
        "tonies_accounts",
        "bindings",
        "cart_items",
        "albums_seen",
        "syncs",
        "settings",
    ] {
        let row = sqlx::query(
            "SELECT count(*) AS n FROM sqlite_master WHERE type = 'table' AND name = ?1",
        )
        .bind(table)
        .fetch_one(&app.pool)
        .await
        .unwrap();
        let n: i64 = row.get("n");
        assert_eq!(n, 1, "Tabelle {table} fehlt");
    }

    let row = sqlx::query(
        "SELECT count(*) AS n FROM sqlite_master WHERE type = 'index' AND name LIKE 'idx_%'",
    )
    .fetch_one(&app.pool)
    .await
    .unwrap();
    let indexes: i64 = row.get("n");
    assert!(indexes >= 6, "zu wenige Indizes: {indexes}");
}

#[tokio::test]
async fn migrations_are_idempotent() {
    let app = setup().await;
    let url = format!(
        "sqlite:{}",
        app._dir.path().join("tonies.db").display()
    );
    let pool2 = db::connect(&url).await.unwrap();
    let row = sqlx::query("SELECT count(*) AS n FROM users")
        .fetch_one(&pool2)
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("n"), 1);
}

// ── public routes ──────────────────────────────────────────────────────────

#[tokio::test]
async fn health_without_cookie() {
    let app = setup().await;
    let resp = send(&app.router, get("/health")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("\"status\":\"ok\""), "body: {body}");
}

#[tokio::test]
async fn login_page_renders_mobile_shell() {
    let app = setup().await;
    let resp = send(&app.router, get("/login")).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("viewport"));
    assert!(body.contains("action=\"/login\""));
    assert!(!body.contains("music-hub"));
    assert!(!body.contains("cdn.jsdelivr.net"));
}

#[tokio::test]
async fn login_sets_session_cookie() {
    let app = setup().await;
    let resp = send(&app.router, login_request("demo", "pw")).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/");
    let cookie = set_cookie(&resp).expect("Set-Cookie fehlt");
    assert!(cookie.starts_with("tonies_session="));
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("Path=/"));
    assert!(cookie.contains("Max-Age=2592000"));
    assert!(cookie.contains("SameSite=Lax"));
    assert!(!cookie.contains("Secure"));
}

#[tokio::test]
async fn login_wrong_password() {
    let app = setup().await;
    let resp = send(&app.router, login_request("demo", "wrong")).await;
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(set_cookie(&resp).is_none());
    let body = body_string(resp).await;
    assert!(body.contains("Benutzername oder Passwort falsch."));
}

// ── auth guard ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn protected_redirects_to_login() {
    let app = setup().await;
    let resp = send(&app.router, get("/")).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/login");
}

#[tokio::test]
async fn protected_with_cookie_ok() {
    let app = setup().await;
    let token = login(&app).await;
    let resp = send(&app.router, get_with_cookie("/", &format!("tonies_session={token}"))).await;
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_string(resp).await;
    assert!(body.contains("Hallo demo!"));
}

#[tokio::test]
async fn logout_clears_cookie() {
    let app = setup().await;
    let req = Request::builder()
        .method(Method::POST)
        .uri("/logout")
        .body(Body::empty())
        .unwrap();
    let resp = send(&app.router, req).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/login");
    let cookie = set_cookie(&resp).expect("Set-Cookie fehlt");
    assert!(cookie.contains("tonies_session="));
    assert!(cookie.contains("Max-Age=0"));

    let resp = send(&app.router, get("/")).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
}

#[tokio::test]
async fn expired_session_rejected() {
    let app = setup().await;
    let row = sqlx::query("SELECT id FROM users WHERE slug = 'demo'")
        .fetch_one(&app.pool)
        .await
        .unwrap();
    let user_id: i64 = row.get("id");

    sqlx::query(
        "INSERT INTO sessions (id, user_id, created_at, expires_at)
         VALUES ('expired-token', ?1, ?2, ?3)",
    )
    .bind(user_id)
    .bind("2020-01-01T00:00:00+00:00")
    .bind("2020-01-02T00:00:00+00:00")
    .execute(&app.pool)
    .await
    .unwrap();

    let resp = send(&app.router, get_with_cookie("/", "tonies_session=expired-token")).await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    assert_eq!(resp.headers().get(header::LOCATION).unwrap(), "/login");
}

#[tokio::test]
async fn cookie_parsing_ignores_other_cookies() {
    let app = setup().await;
    let token = login(&app).await;
    let resp = send(
        &app.router,
        get_with_cookie("/", &format!("other=1; tonies_session={token}")),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
}

#[allow(dead_code)]
fn _assert_headers(_: HeaderMap) {}
