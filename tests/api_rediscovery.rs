//! Integration tests for `GET /api/rediscovery/candidates` (Issue #81).
//!
//! Every test spawns a fresh in-memory app and seeds the shared rediscovery
//! scenario (Issue #80, anchor `REDISCOVERY_SEED_EPOCH = 1_790_000_000`).
//!
//! Fixture tracks 10–18 (see `db::testing::seed_rediscovery_scenario`):
//!   RECENT (90d) = 10, 15 · AGED_2Y = 11 · AGED_5Y = 12, 13, 14, 16, 17, 18
//!   track 16 = Backpack, playlist_count 1 · track 17 = push 30d ago
//!   track 18 = push 5y ago · track 15 = NULL bpm/key
//!
//! Base tracks 1–3 from the liked-songs seed also appear in
//! `v_track_forgotten_facts` but carry no `genre`, so the `genres` facet
//! cleanly scopes assertions to the 10–18 set where needed.
//!
//! `touchedBeforeDays=N` keeps tracks whose `last_touched_at < now - N days`
//! (older than N days); a *smaller* N is more inclusive.

mod common;

use serde_json::Value;
use std::collections::HashSet;

/// Spawn a fresh app and seed the rediscovery scenario.
async fn app() -> (reqwest::Client, String) {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_rediscovery_data(&pool).await;
    (client, base)
}

/// GET the candidates endpoint and return the inner `data` object (asserts 200).
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

fn ids(data: &Value) -> Vec<i64> {
    data["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["trackId"].as_i64().unwrap())
        .collect()
}

fn id_set(data: &Value) -> HashSet<i64> {
    ids(data).into_iter().collect()
}

fn total(data: &Value) -> i64 {
    data["total"].as_i64().unwrap()
}

fn row_for<'a>(data: &'a Value, track_id: i64) -> Option<&'a Value> {
    data["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["trackId"].as_i64() == Some(track_id))
}

fn reasons(row: &Value) -> Vec<String> {
    row["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap().to_string())
        .collect()
}

fn date_of(unix_seconds: i64) -> String {
    chrono::DateTime::from_timestamp(unix_seconds, 0)
        .unwrap()
        .format("%Y-%m-%d")
        .to_string()
}

/// `touchedBeforeDays=30` keeps every seeded track (all older than 30 days),
/// neutralising the primary facet so the other facets can be tested alone.
const INCLUSIVE: &str = "touchedBeforeDays=30";

// ── 1. touchedBeforeDays (primary signal) ────────────────────────────────

#[tokio::test]
async fn touched_before_days_uses_last_touched() {
    let (client, base) = app().await;

    // 365 days: the RECENT tracks (10, 15) are too fresh, the aged ones are in.
    let q = "genres=House,Techno&excludeBackpack=false&excludePushedSinceDays=0&touchedBeforeDays=365&limit=200";
    let data = candidates(&client, &base, q).await;
    let set = id_set(&data);
    for id in [12, 13, 14, 16, 17, 18] {
        assert!(set.contains(&id), "track {id} should be forgotten at 365d");
    }
    assert!(!set.contains(&10), "track 10 is recent (90d)");
    assert!(!set.contains(&15), "track 15 is recent (90d)");

    // A smaller window pulls the recent tracks back in.
    let q = format!(
        "genres=House,Techno&excludeBackpack=false&excludePushedSinceDays=0&{INCLUSIVE}&limit=200"
    );
    let data = candidates(&client, &base, &q).await;
    let set = id_set(&data);
    assert!(set.contains(&10));
    assert!(set.contains(&15));
}

// ── 2. maxPlaylists (curated count only) ─────────────────────────────────

#[tokio::test]
async fn max_playlists_filters_curated_count() {
    let (client, base) = app().await;
    let scope = format!(
        "genres=House&excludeBackpack=false&excludePushedSinceDays=0&{INCLUSIVE}&limit=200"
    );

    // maxPlaylists=0 removes track 16 (curated count 1).
    let data = candidates(&client, &base, &format!("{scope}&maxPlaylists=0")).await;
    assert!(!id_set(&data).contains(&16), "track 16 has 1 curated playlist");
    assert!(id_set(&data).contains(&12));

    // Without the cap, track 16 is back.
    let data = candidates(&client, &base, &scope).await;
    assert!(id_set(&data).contains(&16));
}

// ── 3. likedOnly ─────────────────────────────────────────────────────────

#[tokio::test]
async fn liked_only_returns_only_liked_tracks() {
    let (client, base) = app().await;
    let q = format!("likedOnly=true&excludeBackpack=false&excludePushedSinceDays=0&{INCLUSIVE}&limit=200");
    let data = candidates(&client, &base, &q).await;

    let rows = data["candidates"].as_array().unwrap();
    assert!(!rows.is_empty());
    for row in rows {
        assert!(
            row["likedAt"].as_i64().is_some(),
            "likedOnly must only return liked tracks"
        );
    }
}

// ── 4. excludeBackpack (default true) ────────────────────────────────────

#[tokio::test]
async fn exclude_backpack_default_excludes_backpack_track() {
    let (client, base) = app().await;
    let scope = format!("genres=House&excludePushedSinceDays=0&{INCLUSIVE}&limit=200");

    // Default excludeBackpack=true keeps backpack track 16 out.
    let data = candidates(&client, &base, &scope).await;
    assert!(!id_set(&data).contains(&16));

    // Explicitly disabling it includes track 16.
    let data = candidates(&client, &base, &format!("{scope}&excludeBackpack=false")).await;
    assert!(id_set(&data).contains(&16));
}

// ── 5. excludePushedSinceDays (ledger 033) ───────────────────────────────

#[tokio::test]
async fn exclude_pushed_since_days_excludes_fresh_push() {
    let (client, base) = app().await;
    let scope = format!(
        "genres=House,Techno&excludeBackpack=false&{INCLUSIVE}&limit=200"
    );

    // 180 days: track 17 (fresh push, 30d) is out; track 18 (old push) stays.
    let data = candidates(&client, &base, &format!("{scope}&excludePushedSinceDays=180")).await;
    let set = id_set(&data);
    assert!(!set.contains(&17), "track 17 was pushed 30d ago");
    assert!(set.contains(&18), "track 18's push is old");

    // 0 disables the exclusion entirely.
    let data = candidates(&client, &base, &format!("{scope}&excludePushedSinceDays=0")).await;
    assert!(id_set(&data).contains(&17));
}

// ── 6. requireBpm / requireKey ───────────────────────────────────────────

#[tokio::test]
async fn require_bpm_and_key_drop_unanalysed_tracks() {
    let (client, base) = app().await;
    let scope = format!(
        "genres=Techno&excludeBackpack=false&excludePushedSinceDays=0&{INCLUSIVE}&limit=200"
    );

    // Track 15 (RECENT, NULL bpm/key) is in scope at 30d but dropped by the flag.
    let data = candidates(&client, &base, &format!("{scope}&requireBpm=true")).await;
    let set = id_set(&data);
    assert!(!set.contains(&15), "track 15 has no bpm");
    assert!(set.contains(&11));
    assert!(set.contains(&13));

    let data = candidates(&client, &base, &format!("{scope}&requireKey=true")).await;
    assert!(!id_set(&data).contains(&15), "track 15 has no key");
}

// ── 7. bpmMin/bpmMax, keys, genres ───────────────────────────────────────

#[tokio::test]
async fn bpm_range_keys_and_genres() {
    let (client, base) = app().await;
    let scope = format!("excludeBackpack=false&excludePushedSinceDays=0&{INCLUSIVE}&limit=200");

    // bpm 124..=140 within Techno → {11, 13, 17}; 15 (NULL bpm) and House out.
    let data = candidates(
        &client,
        &base,
        &format!("genres=Techno&{scope}&bpmMin=124&bpmMax=140"),
    )
    .await;
    let set = id_set(&data);
    assert!(set.contains(&11) && set.contains(&13) && set.contains(&17));
    assert!(!set.contains(&15));
    assert!(!set.contains(&12), "track 12 is House");

    // keys=8m within Techno → track 17 (8m) only; 11 is 4m.
    let data = candidates(&client, &base, &format!("genres=Techno&{scope}&keys=8m")).await;
    let set = id_set(&data);
    assert!(set.contains(&17));
    assert!(!set.contains(&11));

    // genres=Techno → {11, 13, 15, 17}.
    let data = candidates(&client, &base, &format!("genres=Techno&{scope}")).await;
    let set = id_set(&data);
    for id in [11, 13, 15, 17] {
        assert!(set.contains(&id), "Techno should include {id}");
    }
}

// ── 8. playCountMax / notPlayedSinceDays ─────────────────────────────────

#[tokio::test]
async fn play_count_max_and_not_played_since() {
    let (client, base) = app().await;
    let scope = format!(
        "genres=House&excludeBackpack=false&excludePushedSinceDays=0&{INCLUSIVE}&limit=200"
    );

    // playCountMax=0 removes track 16 (play_count 1).
    let data = candidates(&client, &base, &format!("{scope}&playCountMax=0")).await;
    assert!(!id_set(&data).contains(&16));
    assert!(id_set(&data).contains(&12));

    // notPlayedSinceDays=30 keeps tracks whose file was never played (NULL).
    let data = candidates(&client, &base, &format!("{scope}&notPlayedSinceDays=30")).await;
    assert!(id_set(&data).contains(&12));
}

// ── 9. reasons match the applied facets ──────────────────────────────────

#[tokio::test]
async fn reasons_match_applied_facets() {
    let (client, base) = app().await;

    // Track 12 under (near) defaults: House facet, backpack excluded, push facet on.
    let q = "genres=House&touchedBeforeDays=365&excludePushedSinceDays=180&limit=200";
    let data = candidates(&client, &base, q).await;
    let row = row_for(&data, 12).expect("track 12 present");
    let rs = reasons(row);
    assert!(rs.contains(&format!("last-touched-{}", date_of(1632320000))), "{rs:?}");
    assert!(rs.contains(&"only-in-1-playlist".to_string()), "{rs:?}");
    assert!(rs.contains(&"never-pushed".to_string()), "{rs:?}");
    assert!(rs.contains(&"liked".to_string()), "{rs:?}");
    assert!(rs.contains(&"genre-match:House".to_string()), "{rs:?}");
    assert!(rs.contains(&"not-in-backpack".to_string()), "{rs:?}");

    // Track 16 with backpack exclusion off: no `not-in-backpack`, still liked.
    let q = "genres=House&excludeBackpack=false&touchedBeforeDays=365&excludePushedSinceDays=180&limit=200";
    let data = candidates(&client, &base, q).await;
    let row = row_for(&data, 16).expect("track 16 present");
    let rs = reasons(row);
    assert!(rs.contains(&"liked".to_string()), "{rs:?}");
    assert!(rs.contains(&"only-in-1-playlist".to_string()), "{rs:?}");
    assert!(!rs.contains(&"not-in-backpack".to_string()), "{rs:?}");
}

// ── 10. seeded shuffle is reproducible ───────────────────────────────────

#[tokio::test]
async fn seeded_shuffle_is_reproducible() {
    let (client, base) = app().await;
    let scope =
        format!("sort=random&excludeBackpack=false&excludePushedSinceDays=0&{INCLUSIVE}&limit=200");

    let d1 = candidates(&client, &base, &format!("{scope}&seed=42")).await;
    let d2 = candidates(&client, &base, &format!("{scope}&seed=42")).await;
    assert_eq!(ids(&d1), ids(&d2), "same seed must give the same order");

    let d3 = candidates(&client, &base, &format!("{scope}&seed=43")).await;
    assert_ne!(ids(&d1), ids(&d3), "different seed should reorder (usually)");
}

// ── 11. limit / offset / total ───────────────────────────────────────────

#[tokio::test]
async fn limit_offset_and_total() {
    let (client, base) = app().await;
    let scope = format!(
        "genres=House,Techno&excludeBackpack=false&excludePushedSinceDays=0&{INCLUSIVE}"
    );

    let all = candidates(&client, &base, &format!("{scope}&limit=200")).await;
    let all_ids = ids(&all);
    let expected_total = total(&all);
    assert_eq!(expected_total, 9, "all 9 fixture tracks are in scope");
    assert_eq!(all_ids.len(), 9);

    let p0 = candidates(&client, &base, &format!("{scope}&limit=2&offset=0")).await;
    let p1 = candidates(&client, &base, &format!("{scope}&limit=2&offset=2")).await;

    assert_eq!(total(&p0), expected_total, "total is pre-pagination");
    assert_eq!(total(&p1), expected_total, "total is constant across pages");
    assert_eq!(ids(&p0), all_ids[0..2].to_vec());
    assert_eq!(ids(&p1), all_ids[2..4].to_vec());
    assert_eq!(p0["limit"].as_i64(), Some(2));
    assert_eq!(p1["offset"].as_i64(), Some(2));
}

// ── 12. invalid params → 400 ─────────────────────────────────────────────

#[tokio::test]
async fn invalid_bpm_min_gt_max_returns_400() {
    let (client, base) = app().await;
    let resp = client
        .get(format!(
            "{base}/api/rediscovery/candidates?bpmMin=140&bpmMax=120"
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
async fn unknown_sort_returns_400() {
    let (client, base) = app().await;
    let resp = client
        .get(format!("{base}/api/rediscovery/candidates?sort=bogus"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

// ── 13. response field contract ──────────────────────────────────────────

#[tokio::test]
async fn response_rows_carry_all_fields() {
    let (client, base) = app().await;
    let data = candidates(&client, &base, "limit=1").await;
    let row = &data["candidates"].as_array().unwrap()[0];

    for field in [
        "trackId",
        "spotifyId",
        "title",
        "artist",
        "playlistCount",
        "likedAt",
        "lastAddedAt",
        "touchedYearsAgo",
        "bpm",
        "musicalKey",
        "genre",
        "inBackpack",
        "owned",
        "reasons",
    ] {
        assert!(row.get(field).is_some(), "missing response field {field}");
    }
    assert!(row["reasons"].is_array());
}
