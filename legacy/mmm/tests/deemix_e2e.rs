//! Live end-to-end deemix lifecycle test (issue #32, section B.3).
//!
//! **Run by Momo against his own deemix instance — never in CI.** The ARL is
//! read from the `DEEMIX_ARL` env var and is **never** committed to the repo
//! (it lives on the deemix instance / `service_config`). This satisfies the
//! "ARL außerhalb des Repos" requirement.
//!
//! ```bash
//! # Connectivity + auth only:
//! DEEMIX_ARL=<your-arl> DEEMIX_HOST=http://localhost:6595 \
//!   cargo test --test deemix_e2e -- --ignored --nocapture
//!
//! # Full lifecycle (add → poll → verify → remove) against a playlist Momo owns:
//! DEEMIX_ARL=<your-arl> \
//! DEEMIX_TEST_URL=https://open.spotify.com/playlist/<id> \
//!   cargo test --test deemix_e2e -- --ignored --nocapture
//! ```

mod common;

use std::time::Duration;

use momos_music_manager::deemix::DeemixClient;

#[tokio::test]
#[ignore = "requires a live deemix instance + ARL; run manually (ARL via env, never committed)"]
async fn deemix_live_lifecycle_e2e() {
    let host =
        std::env::var("DEEMIX_HOST").unwrap_or_else(|_| "http://localhost:6595".to_string());
    let arl = std::env::var("DEEMIX_ARL").expect("DEEMIX_ARL env var is required");
    let test_url = std::env::var("DEEMIX_TEST_URL").ok();

    let pool = common::create_test_db().await;
    sqlx::query(
        "INSERT INTO service_config (service, access_token, metadata_json, is_connected, created_at, updated_at)
         VALUES ('deemix', ?, ?, 1, 0, 0)",
    )
    .bind(&arl)
    .bind(format!(r#"{{"host":"{host}"}}"#))
    .execute(&pool)
    .await
    .unwrap();

    let client = DeemixClient::new(&host, pool);

    // 1. ARL auth
    let login = client
        .login_arl(&arl)
        .await
        .expect("login_arl should succeed against the live instance");
    println!(
        "✅ authed as {:?}",
        login.user.name.as_deref().unwrap_or("unknown")
    );

    // 2. Connection + queue snapshot
    assert!(
        client.test_connection().await.expect("test_connection"),
        "test_connection should report the instance reachable"
    );
    let queue = client.get_queue().await.expect("get_queue");
    println!("✅ queue reachable: {} items", queue.len());

    // 3. Full lifecycle (only when a playlist URL was supplied)
    let Some(url) = test_url else {
        println!("ℹ️  DEEMIX_TEST_URL not set — skipped add/poll/verify/remove.");
        return;
    };

    client.add_to_queue(&url).await.expect("add_to_queue");
    println!("✅ added {url}");

    // Poll until terminal (completed / withErrors), bounded.
    let mut uuid = None;
    for _ in 0..120 {
        let progress = client
            .get_download_progress(&url)
            .await
            .expect("get_download_progress");
        match progress {
            Some(p) => {
                uuid = Some(p.uuid.clone());
                if p.finished {
                    break;
                }
                println!("   … {} / {} tracks ({}%)", p.downloaded, p.total, p.progress);
            }
            None => println!("   … not in queue yet"),
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }

    // Verification (target quality stem > flac > mp3)
    let verification = client
        .verify_download(&url)
        .await
        .expect("verify_download")
        .expect("playlist should still be in queue after polling");
    println!("✅ verification: {verification:?}");
    assert!(
        verification.verified,
        "download should be verified complete with files: {verification:?}"
    );

    // Remove from queue (cleanup)
    if let Some(uuid) = uuid {
        client.remove_from_queue(&uuid).await.expect("remove_from_queue");
        println!("✅ removed {uuid}");
    }
}
