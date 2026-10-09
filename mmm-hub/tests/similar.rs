//! `/api/hub/similar` — EffNet embedding neighbours (auth + ranking).

mod common;

fn blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

async fn put_embedding(app: &common::TestApp, track_id: i64, v: &[f32]) {
    sqlx::query(
        "INSERT INTO hub_track_embeddings (track_id, model, dims, embedding, from_preview, created_at)
         VALUES (?1, 'test', ?2, ?3, 0, '2026-01-01T00:00:00+00:00')",
    )
    .bind(track_id)
    .bind(v.len() as i64)
    .bind(blob(v))
    .execute(&app.pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn similar_requires_login() {
    let app = common::spawn().await;
    let resp = app
        .client()
        .get(app.url("/api/hub/similar?track=1"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn similar_ranks_neighbors_by_cosine() {
    let app = common::spawn().await;
    // Seed [1,0]; t_all is close [0.9,0.1], t_two is far [0,1].
    put_embedding(&app, app.seed.t_pl, &[1.0, 0.0]).await;
    put_embedding(&app, app.seed.t_all, &[0.9, 0.1]).await;
    put_embedding(&app, app.seed.t_two, &[0.0, 1.0]).await;

    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url(&format!("/api/hub/similar?track={}&limit=5", app.seed.t_pl)))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let v: serde_json::Value = resp.json().await.unwrap();
    let data = v["data"].as_array().unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(data[0]["trackId"].as_i64().unwrap(), app.seed.t_all);
    assert!(data[0]["score"].as_f64().unwrap() > data[1]["score"].as_f64().unwrap());
}

#[tokio::test]
async fn similar_without_embedding_is_empty() {
    let app = common::spawn().await;
    let cookie = app.session_cookie(app.seed.alice).await;
    let resp = app
        .client()
        .get(app.url(&format!("/api/hub/similar?track={}", app.seed.t_pl)))
        .header("cookie", cookie)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert!(v["data"].as_array().unwrap().is_empty());
}
