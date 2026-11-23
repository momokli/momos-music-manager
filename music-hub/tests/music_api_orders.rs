//! Contract of the order call the tonie path uses (U0c): `intent: toys` with the
//! binding's format as `target`, so music-api never fetches FLAC for a tonie.

use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};

use music_hub::config::Config;
use music_hub::music_api;

#[derive(Clone, Default)]
struct Orders(Arc<Mutex<Vec<Value>>>);

async fn capture(State(st): State<Orders>, Json(body): Json<Value>) -> Json<Value> {
    st.0.lock().unwrap().push(body);
    Json(json!({ "orderId": "order-1", "status": "open" }))
}

async fn spawn_api() -> (String, Orders) {
    let st = Orders::default();
    let app = Router::new()
        .route("/orders", post(capture))
        .with_state(st.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (format!("http://{addr}"), st)
}

fn cfg_for(base: String) -> Config {
    let mut cfg = Config::for_test("sqlite::memory:");
    cfg.music_api_base = base;
    cfg.music_api_token = Some("t".to_string());
    cfg
}

#[tokio::test]
async fn toys_order_carries_intent_and_target() {
    let (base, seen) = spawn_api().await;
    let cfg = cfg_for(base);

    music_api::order_with_priority_intent(
        &cfg,
        &["CH7812690465".to_string()],
        5,
        "toys",
        Some("mp3-128"),
    )
    .await
    .unwrap();

    let calls = seen.0.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["intent"], "toys");
    assert_eq!(calls[0]["target"], "mp3-128");
    assert_eq!(calls[0]["priority"], 5);
    assert_eq!(calls[0]["items"][0]["isrc"], "CH7812690465");
}

#[tokio::test]
async fn library_order_keeps_the_old_shape() {
    let (base, seen) = spawn_api().await;
    let cfg = cfg_for(base);

    music_api::order_with_priority(&cfg, &["USQX91201487".to_string()], 3)
        .await
        .unwrap();

    let calls = seen.0.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["intent"], "library");
    assert!(
        calls[0].get("target").is_none(),
        "library keeps the configured default, no explicit target: {}",
        calls[0]
    );
}
