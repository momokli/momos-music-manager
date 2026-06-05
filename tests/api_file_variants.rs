//! Integration tests for `GET /api/files/{id}/variants`.
//!
//! Tests the file variants endpoint which returns all file version variants
//! for a given file — grouped by ISRC and WAV source relationships (`source_of`).
//!
//! Seed data:
//!
//! **`seed_basic_data`**:
//! | File | Type     | ISRC  | Notes                                  |
//! |------|----------|-------|----------------------------------------|
//! | 1    | flac     | US001 | FLAC version of "Title One"            |
//! | 2    | stem.m4a | US001 | Stem version (same ISRC as file 1)      |
//! | 3    | flac     | US002 | No other variants                       |
//!
//! **`seed_wav_variant_data`** (linked to file 2 via `source_of`):
//! | File | Type | stem_type      | source_of |
//! |------|------|----------------|-----------|
//! | 20   | wav  | vocals         | 2         |
//! | 21   | wav  | bass           | 2         |
//! | 22   | wav  | drums          | 2         |
//! | 23   | wav  | instrumental   | 2         |
//! | 24   | wav  | other          | 2         |

mod common;

use serde_json::Value;

// ── Not found ──────────────────────────────────────────────────────────────

#[tokio::test]
/// `GET /api/files/9999/variants` returns 404 when the file does not exist.
async fn file_variants_not_found() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/files/9999/variants", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 404, "non-existent file ID should return 404");
}

// ── Stem has WAV children ──────────────────────────────────────────────────

#[tokio::test]
/// `GET /api/files/2/variants` returns the stem file's variants, including
/// the 5 WAV source files (vocals, bass, drums, instrumental, other) that
/// are linked via `source_of`.
async fn file_variants_stem_has_wav_children() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_wav_variant_data(&pool).await;

    let resp = client
        .get(format!("{}/api/files/2/variants", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let data = json["data"]
        .as_object()
        .expect("response data should be an object");

    // Check top-level identity fields
    assert_eq!(data["fileId"], 2, "fileId should be 2");
    assert_eq!(
        data["title"], "Title One",
        "title should match the seed data"
    );
    assert_eq!(
        data["artist"], "Artist A",
        "artist should match the seed data"
    );
    assert_eq!(data["isrc"], "US001", "ISRC should match the seed data");

    // Check variants array
    let variants = data["variants"]
        .as_array()
        .expect("variants should be an array");

    // Should have at least: the stem itself (id=2) + 5 WAV children (ids 20-24)
    // Potentially also file 1 (same ISRC FLAC) depending on implementation
    assert!(
        variants.len() >= 6,
        "should have at least 6 variants (stem + 5 WAVs), got {}",
        variants.len()
    );

    // Find WAV variants and verify their stem_type values
    let wav_variants: Vec<&Value> = variants.iter().filter(|v| v["fileType"] == "wav").collect();

    assert_eq!(wav_variants.len(), 5, "should have exactly 5 WAV variants");

    let stem_types: Vec<String> = wav_variants
        .iter()
        .map(|v| v["stemType"].as_str().unwrap().to_string())
        .collect();
    let expected = vec!["vocals", "bass", "drums", "instrumental", "other"];

    // Each expected stem_type should be present (order is `ORDER BY stem_type`)
    for &st in &expected {
        assert!(
            stem_types.contains(&st.to_string()),
            "WAV variant with stem_type '{}' should be present (got: {:?})",
            st,
            stem_types
        );
    }
}

// ── FLAC has no WAV children ───────────────────────────────────────────────

#[tokio::test]
/// `GET /api/files/1/variants` returns the FLAC file's variants. File 1 has
/// no `source_of` children, but it shares ISRC US001 with file 2 (the stem),
/// so both files 1 and 2 should appear as variants (via ISRC matching).
async fn file_variants_flac_shares_isrc_with_stem() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_wav_variant_data(&pool).await;

    let resp = client
        .get(format!("{}/api/files/1/variants", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let data = json["data"]
        .as_object()
        .expect("response data should be an object");

    assert_eq!(data["fileId"], 1, "fileId should be 1");
    assert_eq!(data["isrc"], "US001", "ISRC should be US001");

    let variants = data["variants"]
        .as_array()
        .expect("variants should be an array");

    // Should include file 1 (flac) + file 2 (stem) + 5 WAVs (20-24)
    // via ISRC matching
    assert!(
        variants.len() >= 7,
        "should have at least 7 variants (flac + stem + 5 WAVs), got {}",
        variants.len()
    );

    let variant_ids: Vec<i64> = variants.iter().map(|v| v["id"].as_i64().unwrap()).collect();

    assert!(
        variant_ids.contains(&1),
        "variants should include the flac file (id=1)"
    );
    assert!(
        variant_ids.contains(&2),
        "variants should include the stem file (id=2) via ISRC match"
    );
    assert!(
        variant_ids.contains(&20),
        "variants should include WAV vocals (id=20) via source_of"
    );
    assert!(
        variant_ids.contains(&24),
        "variants should include WAV other (id=24) via source_of"
    );
}

// ── No other variants ──────────────────────────────────────────────────────

#[tokio::test]
/// `GET /api/files/3/variants` returns the FLAC file with ISRC US002. Since
/// no other file shares ISRC US002 and file 3 has no `source_of` children,
/// the variants array should contain only file 3 itself.
async fn file_variants_no_other_variants() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_wav_variant_data(&pool).await;

    let resp = client
        .get(format!("{}/api/files/3/variants", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let data = json["data"]
        .as_object()
        .expect("response data should be an object");

    assert_eq!(data["fileId"], 3, "fileId should be 3");
    assert_eq!(data["isrc"], "US002", "ISRC should be US002");

    let variants = data["variants"]
        .as_array()
        .expect("variants should be an array");

    // Only file 3 itself — no shared ISRC, no source_of children
    assert_eq!(
        variants.len(),
        1,
        "file 3 should only have itself as a variant (no other US002 files)"
    );
    assert_eq!(
        variants[0]["id"], 3,
        "the only variant should be file 3 itself"
    );
    assert_eq!(variants[0]["fileType"], "flac", "file type should be flac");
}

// ── WAV shows parent stem and siblings ─────────────────────────────────────

#[tokio::test]
/// `GET /api/files/20/variants` queries a WAV source file. The endpoint
/// should traverse `source_of` upward to include the parent stem (file 2)
/// and its sibling WAVs (files 21-24), plus ISRC-matched files (file 1).
async fn file_variants_wav_shows_stem_and_siblings() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_wav_variant_data(&pool).await;

    let resp = client
        .get(format!("{}/api/files/20/variants", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let data = json["data"]
        .as_object()
        .expect("response data should be an object");

    assert_eq!(data["fileId"], 20, "fileId should be 20");

    let variants = data["variants"]
        .as_array()
        .expect("variants should be an array");

    let variant_ids: Vec<i64> = variants.iter().map(|v| v["id"].as_i64().unwrap()).collect();

    // Should include file 1 (same ISRC via ISRC), file 2 (parent stem),
    // all 5 WAVs (siblings + self)
    assert!(
        variant_ids.contains(&1),
        "variants should include flac (id=1) via ISRC match"
    );
    assert!(
        variant_ids.contains(&2),
        "variants should include parent stem (id=2) via source_of traversal"
    );
    assert!(
        variant_ids.contains(&20),
        "variants should include self (id=20)"
    );
    assert!(
        variant_ids.contains(&21),
        "variants should include sibling WAV (id=21)"
    );
    assert!(
        variant_ids.contains(&24),
        "variants should include sibling WAV (id=24)"
    );
}

// ── Backup status ──────────────────────────────────────────────────────────

#[tokio::test]
/// Verify that WAV source variants have `backedUp: true`, matching the
/// `file_locations` entries created by `seed_wav_variant_data`.
async fn file_variants_backed_up_status() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_wav_variant_data(&pool).await;

    let resp = client
        .get(format!("{}/api/files/2/variants", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let data = json["data"]
        .as_object()
        .expect("response data should be an object");

    let variants = data["variants"]
        .as_array()
        .expect("variants should be an array");

    // All 5 WAVs should be backed up
    let wav_variants: Vec<&Value> = variants.iter().filter(|v| v["fileType"] == "wav").collect();

    for wav in &wav_variants {
        assert!(
            wav["backedUp"].as_bool().unwrap_or(false),
            "WAV file id={} should have backedUp=true",
            wav["id"].as_i64().unwrap_or(0)
        );
    }

    // File 2 (stem) should also be backed up (from seed_basic_data)
    let stem_variants: Vec<&Value> = variants.iter().filter(|v| v["id"] == 2).collect();
    if !stem_variants.is_empty() {
        assert!(
            stem_variants[0]["backedUp"].as_bool().unwrap_or(false),
            "stem file (id=2) should have backedUp=true"
        );
    }
}

// ── File type + stem type fields ───────────────────────────────────────────

#[tokio::test]
/// Verify each variant has the correct `fileType` and `stemType` fields:
/// - FLAC: fileType=flac, stemType=null
/// - stem.m4a: fileType=stem.m4a, stemType=null
/// - WAV: fileType=wav, stemType=<specific type>
async fn file_variants_type_fields() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_wav_variant_data(&pool).await;

    let resp = client
        .get(format!("{}/api/files/2/variants", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let data = json["data"]
        .as_object()
        .expect("response data should be an object");

    let variants = data["variants"]
        .as_array()
        .expect("variants should be an array");

    for v in variants {
        let file_type = v["fileType"].as_str().unwrap_or("");
        let stem_type = &v["stemType"];

        match file_type {
            "wav" => {
                // WAV files should have a non-null stemType
                assert!(
                    !stem_type.is_null(),
                    "WAV variant id={} should have a stemType",
                    v["id"].as_i64().unwrap_or(0)
                );
                let st = stem_type.as_str().unwrap_or("");
                assert!(
                    ["vocals", "bass", "drums", "instrumental", "other"].contains(&st),
                    "WAV stemType '{}' should be one of the known types",
                    st
                );
            }
            "stem.m4a" | "flac" => {
                // Non-WAV files should have null stemType
                assert!(
                    stem_type.is_null(),
                    "{} variant id={} should have stemType=null (got: {:?})",
                    file_type,
                    v["id"].as_i64().unwrap_or(0),
                    stem_type
                );
            }
            _ => {
                // Unknown type — just verify it has an id and file_type
                assert!(v["id"].as_i64().is_some(), "variant should have an id");
            }
        }
    }
}

// ── File size present ──────────────────────────────────────────────────────

#[tokio::test]
/// Verify that each variant includes a `fileSize` field with the correct value
/// matching the seed data.
async fn file_variants_has_file_size() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    common::seed_wav_variant_data(&pool).await;

    let resp = client
        .get(format!("{}/api/files/2/variants", base))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200, "expected 200 OK");

    let json: Value = resp.json().await.unwrap();
    let data = json["data"]
        .as_object()
        .expect("response data should be an object");

    let variants = data["variants"]
        .as_array()
        .expect("variants should be an array");

    // Each variant must have a positive fileSize
    for v in variants {
        let size = v["fileSize"]
            .as_i64()
            .expect("each variant should have a fileSize");
        assert!(
            size > 0,
            "fileSize should be positive for variant id={}",
            v["id"].as_i64().unwrap_or(0)
        );
    }

    // Verify specific values from seed data
    let size_map: std::collections::HashMap<i64, i64> = variants
        .iter()
        .map(|v| (v["id"].as_i64().unwrap(), v["fileSize"].as_i64().unwrap()))
        .collect();

    assert_eq!(
        size_map.get(&1),
        Some(&5_000_000),
        "file 1 should have fileSize=5000000"
    );
    assert_eq!(
        size_map.get(&2),
        Some(&8_000_000),
        "file 2 should have fileSize=8000000"
    );
    assert_eq!(
        size_map.get(&20),
        Some(&2_000_000),
        "WAV 20 should have fileSize=2000000"
    );
}
