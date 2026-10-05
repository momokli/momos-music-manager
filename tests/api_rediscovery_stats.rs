//! Integration tests for `GET /api/rediscovery/stats` (Issue #82).
//!
//! Every test seeds the shared rediscovery scenario (Issue #80, anchor
//! `REDISCOVERY_SEED_EPOCH = 1_790_000_000`): fixture tracks 10–18 plus the
//! liked-songs base tracks 1–3.
//!
//! Fixture summary (see `db::testing::seed_rediscovery_scenario`):
//!   10 (120/1a House) · 11 (124/4m Techno) · 12 (128/8m House) ·
//!   13 (140/12a Techno) · 14 (155/1a House) · 15 (NULL/NULL Techno, file) ·
//!   16 (120/4m House, Backpack) · 17 (124/8m Techno, push fresh) ·
//!   18 (128/12a House, push old). All fixture files are local (owned).
//!
//! Base tracks: 1 (128/4m, local, liked) · 2 (140/8m, backup-only, liked) ·
//! 3 (no linked file, not liked).

mod common;

use serde_json::Value;
use std::collections::HashSet;

/// Spawn a fresh app and seed the rediscovery scenario.
async fn app() -> (reqwest::Client, String) {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_rediscovery_data(&pool).await;
    (client, base)
}

/// GET `/api/rediscovery/stats` and return the inner `data` object (asserts 200).
async fn stats(client: &reqwest::Client, base: &str, query: &str) -> Value {
    let url = if query.is_empty() {
        format!("{base}/api/rediscovery/stats")
    } else {
        format!("{base}/api/rediscovery/stats?{query}")
    };
    let resp = client.get(&url).send().await.unwrap();
    let status = resp.status();
    let body = resp.text().await.unwrap();
    assert_eq!(status, 200, "unexpected status {status} for {url}: {body}");
    serde_json::from_str::<Value>(&body).unwrap()["data"].clone()
}

/// GET `/api/rediscovery/candidates` and return the inner `data` object.
async fn candidates(client: &reqwest::Client, base: &str, query: &str) -> Value {
    let url = if query.is_empty() {
        format!("{base}/api/rediscovery/candidates")
    } else {
        format!("{base}/api/rediscovery/candidates?{query}")
    };
    let resp = client.get(&url).send().await.unwrap();
    let status = resp.status();
    let body = resp.text().await.unwrap();
    assert_eq!(status, 200, "unexpected status {status} for {url}: {body}");
    serde_json::from_str::<Value>(&body).unwrap()["data"].clone()
}

fn i(v: &Value, key: &str) -> i64 {
    v[key].as_i64().unwrap_or_else(|| panic!("field {key} not an int: {v}"))
}

/// Unscoped preset: keeps every seeded track (all older than 30 days) and turns
/// the push cooldown off so the raw facet counts are visible.
const ALL: &str = "touchedBeforeDays=30&excludeBackpack=false&excludePushedSinceDays=0";

// ── 1. Counters, invariants and bucket distribution ──────────────────────

#[tokio::test]
async fn counts_and_invariants() {
    let (client, base) = app().await;
    let data = stats(&client, &base, ALL).await;

    // 12 tracks: fixture 10–18 (9) + base 1–3 (3).
    let matching = i(&data, "matching");
    let with_bpm_and_key = i(&data, "withBpmAndKey");
    let needs_analysis = i(&data, "needsAnalysis");
    let not_owned = i(&data, "notOwned");

    assert_eq!(matching, 12, "{data}");
    assert_eq!(with_bpm_and_key, 10, "{data}");
    assert_eq!(needs_analysis, 1, "{data}"); // fixture 15 (file, NULL bpm/key)
    assert_eq!(not_owned, 2, "{data}"); // base 2 (backup only) + base 3 (no file)

    // Invariant: withBpmAndKey + needsAnalysis + noFile == matching.
    let no_file = matching - (with_bpm_and_key + needs_analysis);
    assert_eq!(no_file, 1, "only base track 3 has no linked file");
    assert_eq!(with_bpm_and_key + needs_analysis + no_file, matching);

    // Bucket distribution sums exactly to withBpmAndKey.
    let buckets = data["byBpmBucket"].as_object().unwrap();
    let bucket_sum: i64 = buckets.values().map(|v| v.as_i64().unwrap()).sum();
    assert_eq!(bucket_sum, with_bpm_and_key, "buckets {buckets:?}");

    // Expected distribution (validates the shared 5-BPM bucket definition).
    assert_eq!(buckets["120-124"], 4); // 10, 11, 16, 17
    assert_eq!(buckets["125-129"], 3); // 1, 12, 18
    assert_eq!(buckets["140-144"], 2); // 2, 13
    assert_eq!(buckets["155-159"], 1); // 14
    assert_eq!(buckets.len(), 4, "only populated buckets are emitted");

    // Liked/unliked split sums to matching.
    let liked = i(&data["byLiked"], "liked");
    let unliked = i(&data["byLiked"], "unliked");
    assert_eq!(liked, 11, "{data}");
    assert_eq!(unliked, 1, "{data}"); // base track 3 is not liked
    assert_eq!(liked + unliked, matching);
}

// ── 2. Facet parity with candidates ──────────────────────────────────────

#[tokio::test]
async fn facet_parity_with_candidates() {
    let (client, base) = app().await;
    let q = "genres=House&touchedBeforeDays=365&excludeBackpack=false&excludePushedSinceDays=180&limit=200";

    let cand = candidates(&client, &base, q).await;
    let st = stats(&client, &base, q).await;

    let total = cand["total"].as_i64().unwrap();
    assert_eq!(
        i(&st, "matching"),
        total,
        "stats.matching must equal candidates.total for identical facets"
    );

    // Cross-check the candidate id set is fully counted.
    let ids: HashSet<i64> = cand["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["trackId"].as_i64().unwrap())
        .collect();
    assert_eq!(ids.len() as i64, total);
}

// ── 3. likedOnly ─────────────────────────────────────────────────────────

#[tokio::test]
async fn liked_only_has_no_unliked() {
    let (client, base) = app().await;
    let data = stats(
        &client,
        &base,
        &format!("likedOnly=true&excludeBackpack=false&excludePushedSinceDays=0&touchedBeforeDays=30"),
    )
    .await;

    let matching = i(&data, "matching");
    assert!(matching > 0);
    assert_eq!(i(&data["byLiked"], "liked"), matching, "{data}");
    assert_eq!(i(&data["byLiked"], "unliked"), 0, "{data}");
}

// ── 4. excludePushedSinceDays / pushedRecently ───────────────────────────

#[tokio::test]
async fn pushed_recently_matches_cooldown_fallout() {
    let (client, base) = app().await;
    let scope = "touchedBeforeDays=30&excludeBackpack=false";

    // Active cooldown (180d): track 17 (pushed 30d ago) drops out of matching
    // and is reported as pushedRecently.
    let data = stats(&client, &base, &format!("{scope}&excludePushedSinceDays=180")).await;
    assert_eq!(i(&data, "matching"), 11, "{data}");
    assert_eq!(i(&data, "pushedRecently"), 1, "{data}");

    // Disabled (0): nothing falls out; pushedRecently must be 0.
    let data = stats(&client, &base, &format!("{scope}&excludePushedSinceDays=0")).await;
    assert_eq!(i(&data, "matching"), 12, "{data}");
    assert_eq!(i(&data, "pushedRecently"), 0, "{data}");
}

// ── 5. Validation & non-pagination ───────────────────────────────────────

#[tokio::test]
async fn bpm_min_gt_max_returns_400() {
    let (client, base) = app().await;
    let resp = client
        .get(format!(
            "{base}/api/rediscovery/stats?bpmMin=140&bpmMax=120"
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let json: Value = resp.json().await.unwrap();
    assert!(
        json["error"].as_str().unwrap_or_default().contains("bpmMin"),
        "error message should mention bpmMin: {json}"
    );
}

#[tokio::test]
async fn stats_ignores_sort_limit_offset_seed() {
    let (client, base) = app().await;
    // An unknown `sort`, a page size and an offset must not affect the counts.
    let data = stats(
        &client,
        &base,
        &format!("{ALL}&sort=bogus&limit=2&offset=5&seed=7"),
    )
    .await;
    assert_eq!(i(&data, "matching"), 12, "{data}");
    assert_eq!(i(&data, "withBpmAndKey"), 10, "{data}");
}

// ── 6. Response field contract ───────────────────────────────────────────

#[tokio::test]
async fn response_carries_all_fields() {
    let (client, base) = app().await;
    let data = stats(&client, &base, ALL).await;

    for field in [
        "matching",
        "withBpmAndKey",
        "needsAnalysis",
        "notOwned",
        "pushedRecently",
        "byBpmBucket",
        "byLiked",
    ] {
        assert!(data.get(field).is_some(), "missing response field {field}");
    }
    assert!(data["byBpmBucket"].is_object());
    assert!(data["byLiked"].is_object());
    assert!(data["byLiked"]["liked"].is_i64());
    assert!(data["byLiked"]["unliked"].is_i64());
}
