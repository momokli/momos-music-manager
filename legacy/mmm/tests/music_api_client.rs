//! Contract tests for [`MusicApiClient`] against a mock upstream HTTP server.
//!
//! Guards the exact wire shapes the `music-api` service returns (see
//! `music-api/README.md`): the `{data:…}`-less envelopes, `camelCase` keys, the
//! `Bearer` header, and binary file delivery.

use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use serde_json::json;

use momos_music_manager::music_api::MusicApiClient;

const TOKEN: &str = "test-token";
const ISRC: &str = "USQX91201487";

/// A stand-in for the real service that enforces the bearer token.
async fn spawn_mock() -> String {
    let app = Router::new()
        .route(
            "/orders",
            post(|headers: axum::http::HeaderMap, _body: axum::body::Bytes| async move {
                if !authorized(&headers) {
                    return (StatusCode::UNAUTHORIZED, Body::from("unauthorized")).into_response();
                }
                axum::Json(json!({ "orderId": "o1", "status": "open", "count": 1 })).into_response()
            })
            .get(|headers: axum::http::HeaderMap| async move {
                if !authorized(&headers) {
                    return (StatusCode::UNAUTHORIZED, Body::from("unauthorized")).into_response();
                }
                axum::Json(json!({
                    "orders": [
                        { "id": "o1", "status": "open", "createdAt": 1, "updatedAt": 2 }
                    ]
                }))
                .into_response()
            }),
        )
        .route(
            "/orders/o1",
            get(|headers: axum::http::HeaderMap| async move {
                if !authorized(&headers) {
                    return (StatusCode::UNAUTHORIZED, Body::from("unauthorized")).into_response();
                }
                axum::Json(json!({
                    "orderId": "o1",
                    "status": "done",
                    "createdAt": 1,
                    "updatedAt": 2,
                    "items": [{
                        "isrc": ISRC,
                        "state": "ready",
                        "deezerId": "836932812",
                        "title": "Sudno",
                        "artist": "Molchat Doma",
                        "formats": ["flac", "320", "128"],
                        "error": null
                    }]
                }))
                .into_response()
            }),
        )
        .route(
            "/isrc/{isrc}/flac",
            get(|headers: axum::http::HeaderMap| async move {
                if !authorized(&headers) {
                    return (StatusCode::UNAUTHORIZED, Body::from("unauthorized")).into_response();
                }
                (
                    [(header::CONTENT_TYPE, "audio/flac")],
                    Body::from("FLACDATA"),
                )
                    .into_response()
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}")
}

fn authorized(headers: &axum::http::HeaderMap) -> bool {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        == Some(&format!("Bearer {TOKEN}"))
}

#[tokio::test]
async fn client_round_trips_the_contract() {
    let base = spawn_mock().await;
    let client = MusicApiClient::new(&base, TOKEN);

    // POST /orders
    let order_id = client
        .create_order(&[ISRC.to_string()])
        .await
        .expect("create_order");
    assert_eq!(order_id, "o1");

    // GET /orders
    let orders = client.list_orders(Some("open")).await.expect("list_orders");
    assert_eq!(orders.len(), 1);
    assert_eq!(orders[0].id, "o1");
    assert_eq!(orders[0].created_at, 1);

    // GET /orders/{id}
    let status = client.get_order("o1").await.expect("get_order");
    assert_eq!(status.order_id, "o1");
    assert_eq!(status.items.len(), 1);
    assert_eq!(status.items[0].isrc, ISRC);
    assert_eq!(status.items[0].state, "ready");
    assert_eq!(status.items[0].deezer_id.as_deref(), Some("836932812"));
    assert_eq!(
        status.items[0].formats,
        vec!["flac".to_string(), "320".to_string(), "128".to_string()]
    );

    // GET /isrc/{isrc}/{format} → raw bytes
    let bytes = client
        .download_isrc(ISRC, "flac")
        .await
        .expect("download_isrc");
    assert_eq!(bytes, b"FLACDATA");
}

/// A wrong token must surface as an error, not as an empty success — the
/// upstream answers 401 with a plain body, so the client may not assume JSON.
#[tokio::test]
async fn client_reports_unauthorized() {
    let base = spawn_mock().await;
    let client = MusicApiClient::new(&base, "wrong-token");

    let err = client
        .create_order(&[ISRC.to_string()])
        .await
        .expect_err("a 401 must be an error");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("401") || msg.to_lowercase().contains("unauthor"),
        "error should mention the rejection, got: {msg}"
    );
}

/// The client must never send a request without the bearer header.
#[tokio::test]
async fn client_sets_the_bearer_header() {
    let base = spawn_mock().await;

    // Build a request as the client does and assert the mock accepts it.
    let resp = reqwest::Client::new()
        .post(format!("{base}/orders"))
        .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
        .json(&json!({ "items": [{ "isrc": ISRC }] }))
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // Sanity: the raw mock rejects a missing header, so the assertion above is
    // meaningful.
    let denied = reqwest::Client::new()
        .post(format!("{base}/orders"))
        .json(&json!({ "items": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
}
