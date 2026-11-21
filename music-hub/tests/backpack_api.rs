//! Integration tests for the device-token-gated backpack read API (issue #367):
//! auth (401 without/with an unknown token), the exact list/export JSON shape,
//! owner scoping (foreign → 404) and the empty-backpack → `[]` case.

mod common;

use music_hub::backpacks::{self, SourceRef};
use music_hub::devices::hash_token;
use reqwest::StatusCode;
use serde_json::{Value, json};

const ALICE_TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// Insert a **bound** device owned by `user_id` whose device token hashes to
/// `token_hash`; returns the device id.
async fn bound_device(app: &common::TestApp, user_id: i64, name: &str, raw_token: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_devices (user_id, name, token_hash, created_at, paired_at)
         VALUES (?1, ?2, ?3, ?4, ?4) RETURNING id",
    )
    .bind(user_id)
    .bind(name)
    .bind(hash_token(raw_token))
    .bind("2026-01-01T00:00:00+00:00")
    .fetch_one(&app.pool)
    .await
    .expect("insert bound device")
}

/// Insert a hub track; returns its id.
async fn track(app: &common::TestApp, sid: &str, isrc: &str, title: &str, artists: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tracks (service, service_track_id, isrc, title, artists, first_seen_at)
         VALUES ('spotify', ?1, ?2, ?3, ?4, ?5) RETURNING id",
    )
    .bind(sid)
    .bind(isrc)
    .bind(title)
    .bind(artists)
    .bind("2026-01-01T00:00:00+00:00")
    .fetch_one(&app.pool)
    .await
    .expect("insert track")
}

/// Insert a tag owned by `user_id`; returns its id.
async fn tag(app: &common::TestApp, user_id: i64, slug: &str, name: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO hub_tags (owner_user_id, slug, name, created_at)
         VALUES (?1, ?2, ?3, ?4) RETURNING id",
    )
    .bind(user_id)
    .bind(slug)
    .bind(name)
    .bind("2026-01-01T00:00:00+00:00")
    .fetch_one(&app.pool)
    .await
    .expect("insert tag")
}

async fn resolve_tag(app: &common::TestApp, track_id: i64, tag_id: i64) {
    sqlx::query("INSERT INTO hub_track_resolved_tags (track_id, tag_id) VALUES (?1, ?2)")
        .bind(track_id)
        .bind(tag_id)
        .execute(&app.pool)
        .await
        .expect("insert resolved tag");
}

/// Set the typed classification of a tag (`hub_tags.type`).
async fn set_tag_type(app: &common::TestApp, tag_id: i64, ty: &str) {
    sqlx::query("UPDATE hub_tags SET type = ?2 WHERE id = ?1")
        .bind(tag_id)
        .bind(ty)
        .execute(&app.pool)
        .await
        .expect("set tag type");
}

/// Insert an external id for a track.
async fn external_id(app: &common::TestApp, track_id: i64, service: &str, external_id: &str) {
    sqlx::query(
        "INSERT INTO hub_track_external_ids (track_id, service, external_id)
         VALUES (?1, ?2, ?3)",
    )
    .bind(track_id)
    .bind(service)
    .bind(external_id)
    .execute(&app.pool)
    .await
    .expect("insert external id");
}

/// Insert `hub_track_audio` including the Camelot value.
async fn audio_with_camelot(
    app: &common::TestApp,
    track_id: i64,
    musical_key: Option<&str>,
    camelot: Option<&str>,
) {
    sqlx::query("INSERT INTO hub_track_audio (track_id, musical_key, camelot) VALUES (?1, ?2, ?3)")
        .bind(track_id)
        .bind(musical_key)
        .bind(camelot)
        .execute(&app.pool)
        .await
        .expect("insert audio");
}

async fn audio(
    app: &common::TestApp,
    track_id: i64,
    bpm: Option<f64>,
    key: Option<&str>,
    energy: Option<f64>,
) {
    sqlx::query(
        "INSERT INTO hub_track_audio (track_id, bpm, musical_key, energy) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(track_id)
    .bind(bpm)
    .bind(key)
    .bind(energy)
    .execute(&app.pool)
    .await
    .expect("insert audio");
}

async fn json_body(resp: reqwest::Response) -> (StatusCode, Value) {
    let status = resp.status();
    let txt = resp.text().await.expect("body");
    let v = serde_json::from_str(&txt).unwrap_or(Value::Null);
    (status, v)
}

// ── auth ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn missing_or_unknown_token_is_401() {
    let app = common::spawn().await;

    // No Authorization header at all.
    let (status, v) = json_body(
        app.client()
            .get(app.url("/api/hub/backpacks"))
            .send()
            .await
            .expect("anon list"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(v.get("error").is_some(), "error object: {v}");

    // Unknown (never-registered) token.
    let (status, v) = json_body(
        app.client()
            .get(app.url("/api/hub/backpacks"))
            .header("Authorization", "Bearer deadbeef")
            .send()
            .await
            .expect("unknown list"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(v.get("error").is_some());

    // Same for the export route.
    let (status, _) = json_body(
        app.client()
            .get(app.url("/api/hub/backpacks/1/export"))
            .send()
            .await
            .expect("anon export"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn unbound_pending_token_is_401() {
    let app = common::spawn().await;
    // A pending (unbound) row: user_id NULL, no paired_at.
    sqlx::query(
        "INSERT INTO hub_devices (user_id, name, token_hash, created_at, expires_at)
         VALUES (NULL, 'pending', ?1, ?2, ?3)",
    )
    .bind(hash_token("pending-token"))
    .bind("2026-01-01T00:00:00+00:00")
    .bind("2099-01-01T00:00:00+00:00")
    .execute(&app.pool)
    .await
    .expect("insert pending");

    let (status, _) = json_body(
        app.client()
            .get(app.url("/api/hub/backpacks"))
            .header("Authorization", "Bearer pending-token")
            .send()
            .await
            .expect("pending list"),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ── list ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn list_returns_only_owned_backpacks() {
    let app = common::spawn().await;
    bound_device(&app, app.seed.alice, "alice-box", ALICE_TOKEN).await;

    let bp_active = backpacks::create_backpack(&app.pool, app.seed.alice, "Reise")
        .await
        .expect("create");
    let bp_idle = backpacks::create_backpack(&app.pool, app.seed.alice, "Chill")
        .await
        .expect("create");
    backpacks::set_active(&app.pool, app.seed.alice, bp_active)
        .await
        .expect("set active");
    // A backpack owned by bob must not leak into alice's list.
    backpacks::create_backpack(&app.pool, app.seed.bob, "Bob Only")
        .await
        .expect("create bob");

    let (status, v) = json_body(
        app.client()
            .get(app.url("/api/hub/backpacks"))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("list"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        v,
        json!({ "backpacks": [
            { "id": bp_active, "name": "Reise", "is_active": true },
            { "id": bp_idle, "name": "Chill", "is_active": false },
        ]}),
        "exact list shape: {v}"
    );
}

// ── export ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn export_has_exact_shape_with_tags_and_audio() {
    let app = common::spawn().await;
    bound_device(&app, app.seed.alice, "alice-box", ALICE_TOKEN).await;

    let t1 = track(&app, "s1", "DEAAA0000001", "Alpha", "Aaa").await;
    let t2 = track(&app, "s2", "DEAAA0000002", "Beta", "Bbb").await;

    // Tags on t1 in non-alphabetical insert order; API must sort by name.
    let tag_zeta = tag(&app, app.seed.alice, "zeta", "Zeta").await;
    let tag_alpha = tag(&app, app.seed.alice, "alpha", "Alpha").await;
    resolve_tag(&app, t1, tag_zeta).await;
    resolve_tag(&app, t1, tag_alpha).await;

    audio(&app, t1, Some(128.0), Some("A-Minor"), Some(7.0)).await;
    // t2 has no audio row at all -> bpm/key/energy null.

    let bp = backpacks::create_backpack(&app.pool, app.seed.alice, "Mix")
        .await
        .expect("create");
    backpacks::add_source(&app.pool, bp, SourceRef::Track(t1))
        .await
        .expect("add t1");
    backpacks::add_source(&app.pool, bp, SourceRef::Track(t2))
        .await
        .expect("add t2");

    let (status, v) = json_body(
        app.client()
            .get(app.url(&format!("/api/hub/backpacks/{bp}/export")))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("export"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        v,
        json!({
            "backpack": { "id": bp, "name": "Mix" },
            "comment_template": "{types} {tags}",
            "comment_template_name": Value::Null,
            "tracks": [
                {
                    "track_id": t1,
                    "isrc": "DEAAA0000001",
                    "title": "Alpha",
                    "artists": "Aaa",
                    "tags": ["Alpha", "Zeta"],
                    "types": [],
                    "source_ids": [],
                    "bpm": 128.0,
                    "key": "A-Minor",
                    "energy": 7.0,
                },
                {
                    "track_id": t2,
                    "isrc": "DEAAA0000002",
                    "title": "Beta",
                    "artists": "Bbb",
                    "tags": [],
                    "types": [],
                    "source_ids": [],
                    "bpm": null,
                    "key": null,
                    "energy": null,
                },
            ]
        }),
        "exact export shape: {v}"
    );
}

/// Acceptance #690: typed tags in `TYPE_NAMES` order (phase types in fixed
/// order, name-sorted within a family), untyped alphabetically last; `types`
/// carries one marker per family; `key` prefers Camelot; `source_ids` are
/// prefixed per service in fixed order and skip unknown services.
#[tokio::test]
async fn export_typed_tags_types_camelot_and_source_ids() {
    let app = common::spawn().await;
    bound_device(&app, app.seed.alice, "alice-box", ALICE_TOKEN).await;

    let t = track(&app, "s10", "DEAAA0000010", "Typed", "Zz").await;

    // Insert deliberately scrambled: type order must win over insertion order.
    let merkmal = tag(&app, app.seed.alice, "airy", "Airy").await;
    let phase = tag(&app, app.seed.alice, "peak", "Peak").await;
    let mood_b = tag(&app, app.seed.alice, "bass", "Bass").await;
    let mood = tag(&app, app.seed.alice, "deep", "Deep").await;
    let loose = tag(&app, app.seed.alice, "zzz", "Zzz").await;
    set_tag_type(&app, merkmal, "MERKMAL").await;
    set_tag_type(&app, phase, "PHASE_2").await;
    set_tag_type(&app, mood_b, "MOOD").await;
    set_tag_type(&app, mood, "MOOD").await;
    for tg in [loose, merkmal, phase, mood_b, mood] {
        resolve_tag(&app, t, tg).await;
    }

    // Camelot wins over the musical key.
    audio_with_camelot(&app, t, Some("G-Minor"), Some("8A")).await;
    external_id(&app, t, "youtube", "yt-1").await;
    external_id(&app, t, "spotify", "sp-2").await;
    external_id(&app, t, "soundcloud", "sc-1").await;
    external_id(&app, t, "spotify", "sp-1").await;
    external_id(&app, t, "tidal", "td-1").await;

    let bp = backpacks::create_backpack(&app.pool, app.seed.alice, "Typed")
        .await
        .expect("create");
    backpacks::add_source(&app.pool, bp, SourceRef::Track(t))
        .await
        .expect("add t");

    let (status, v) = json_body(
        app.client()
            .get(app.url(&format!("/api/hub/backpacks/{bp}/export")))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("export"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let tr = &v["tracks"][0];
    assert_eq!(tr["tags"], json!(["Bass", "Deep", "Peak", "Airy", "Zzz"]));
    assert_eq!(tr["types"], json!(["M", "P", "E"]));
    assert_eq!(tr["key"], json!("8A"));
    assert_eq!(
        tr["source_ids"],
        json!(["sp:sp-1", "sp:sp-2", "sc:sc-1", "yt:yt-1"])
    );
}

/// Acceptance #690: PHASE_1 + PHASE_4 collapse to a single `P`.
#[tokio::test]
async fn export_collapses_multiple_phases_to_one_marker() {
    let app = common::spawn().await;
    bound_device(&app, app.seed.alice, "alice-box", ALICE_TOKEN).await;

    let t = track(&app, "s11", "DEAAA0000011", "Phases", "Pp").await;
    let p1 = tag(&app, app.seed.alice, "p1", "Warmup").await;
    let p4 = tag(&app, app.seed.alice, "p4", "Peak").await;
    set_tag_type(&app, p1, "PHASE_1").await;
    set_tag_type(&app, p4, "PHASE_4").await;
    resolve_tag(&app, t, p1).await;
    resolve_tag(&app, t, p4).await;

    let bp = backpacks::create_backpack(&app.pool, app.seed.alice, "Phases")
        .await
        .expect("create");
    backpacks::add_source(&app.pool, bp, SourceRef::Track(t))
        .await
        .expect("add t");

    let (status, v) = json_body(
        app.client()
            .get(app.url(&format!("/api/hub/backpacks/{bp}/export")))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("export"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let tr = &v["tracks"][0];
    assert_eq!(tr["types"], json!(["P"]));
    assert_eq!(tr["tags"], json!(["Warmup", "Peak"]));
    assert_eq!(tr["key"], Value::Null, "no audio row → null key");
    assert_eq!(tr["source_ids"], json!([]));
}

/// Acceptance #690: with a musical key but no Camelot the key is normalised;
/// Camelot with surrounding whitespace still wins.
#[tokio::test]
async fn export_key_falls_back_to_normalised_musical_key() {
    let app = common::spawn().await;
    bound_device(&app, app.seed.alice, "alice-box", ALICE_TOKEN).await;

    let t1 = track(&app, "s12", "DEAAA0000012", "Musical", "Mm").await;
    let t2 = track(&app, "s13", "DEAAA0000013", "Spaced", "Ss").await;
    audio_with_camelot(&app, t1, Some("G -- Minor"), None).await;
    audio_with_camelot(&app, t2, Some("A-Minor"), Some(" 8A ")).await;

    let bp = backpacks::create_backpack(&app.pool, app.seed.alice, "Keys")
        .await
        .expect("create");
    backpacks::add_source(&app.pool, bp, SourceRef::Track(t1))
        .await
        .expect("add t1");
    backpacks::add_source(&app.pool, bp, SourceRef::Track(t2))
        .await
        .expect("add t2");

    let (status, v) = json_body(
        app.client()
            .get(app.url(&format!("/api/hub/backpacks/{bp}/export")))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("export"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let tracks = v["tracks"].as_array().expect("tracks array");
    let by_id = |id: i64| {
        tracks
            .iter()
            .find(|t| t["track_id"] == json!(id))
            .expect("track present")
    };
    assert_eq!(by_id(t1)["key"], json!("G-Minor"));
    assert_eq!(by_id(t2)["key"], json!("8A"));
}

#[tokio::test]
async fn empty_backpack_exports_empty_tracks() {
    let app = common::spawn().await;
    bound_device(&app, app.seed.alice, "alice-box", ALICE_TOKEN).await;
    let bp = backpacks::create_backpack(&app.pool, app.seed.alice, "Leer")
        .await
        .expect("create");

    let (status, v) = json_body(
        app.client()
            .get(app.url(&format!("/api/hub/backpacks/{bp}/export")))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("export"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "empty backpack is not a 500: {v}");
    assert_eq!(
        v,
        json!({
            "backpack": { "id": bp, "name": "Leer" },
            "comment_template": "{types} {tags}",
            "comment_template_name": Value::Null,
            "tracks": []
        })
    );
}

#[tokio::test]
async fn export_includes_device_comment_template() {
    let app = common::spawn().await;
    let did = bound_device(&app, app.seed.alice, "alice-box", ALICE_TOKEN).await;
    let bp = backpacks::create_backpack(&app.pool, app.seed.alice, "Mix")
        .await
        .expect("create");

    // No settings row yet -> the export falls back to the default template.
    let (status, v) = json_body(
        app.client()
            .get(app.url(&format!("/api/hub/backpacks/{bp}/export")))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("export"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["comment_template"], "{types} {tags}");

    // A stored per-device template is delivered verbatim.
    sqlx::query(
        "INSERT INTO hub_device_settings (device_id, preferred_format, comment_template, updated_at)
         VALUES (?1, 'flac', ?2, ?3)",
    )
    .bind(did)
    .bind("{key} / {bpm}")
    .bind("2026-01-01T00:00:00+00:00")
    .execute(&app.pool)
    .await
    .expect("insert settings");

    let (status, v) = json_body(
        app.client()
            .get(app.url(&format!("/api/hub/backpacks/{bp}/export")))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("export"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["comment_template"], "{key} / {bpm}");

    // An empty stored template resolves to the default.
    sqlx::query("UPDATE hub_device_settings SET comment_template = '' WHERE device_id = ?1")
        .bind(did)
        .execute(&app.pool)
        .await
        .expect("clear template");
    let (_, v) = json_body(
        app.client()
            .get(app.url(&format!("/api/hub/backpacks/{bp}/export")))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("export"),
    )
    .await;
    assert_eq!(v["comment_template"], "{types} {tags}");
    assert!(v["comment_template_name"].is_null());
    assert!(
        v.get("comment_template_id").is_none(),
        "the export must carry the resolved body, never the id"
    );
}

#[tokio::test]
async fn export_delivers_resolved_template_body_not_id() {
    let app = common::spawn().await;
    let did = bound_device(&app, app.seed.alice, "alice-box", ALICE_TOKEN).await;
    let bp = backpacks::create_backpack(&app.pool, app.seed.alice, "Mix")
        .await
        .expect("create");

    // A named template owned by alice, referenced by her device; the legacy free
    // text would resolve differently, so this proves the template body wins.
    let tid: i64 = sqlx::query_scalar(
        "INSERT INTO hub_comment_templates
            (owner_user_id, name, body, is_shared, created_at, updated_at)
         VALUES (?1, 'Mix-Key', '{key} / {bpm}', 0, ?2, ?2) RETURNING id",
    )
    .bind(app.seed.alice)
    .bind("2026-01-01T00:00:00+00:00")
    .fetch_one(&app.pool)
    .await
    .expect("insert template");
    sqlx::query(
        "INSERT INTO hub_device_settings
            (device_id, preferred_format, comment_template, comment_template_id, updated_at)
         VALUES (?1, 'flac', '{tags}', ?2, ?3)",
    )
    .bind(did)
    .bind(tid)
    .bind("2026-01-01T00:00:00+00:00")
    .execute(&app.pool)
    .await
    .expect("insert settings");

    let (status, v) = json_body(
        app.client()
            .get(app.url(&format!("/api/hub/backpacks/{bp}/export")))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("export"),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["comment_template"], "{key} / {bpm}", "template body wins");
    assert_eq!(v["comment_template_name"], "Mix-Key");
    assert!(
        v.get("comment_template_id").is_none(),
        "no id in the export: {v}"
    );
    assert!(
        !v["comment_template"].as_str().unwrap().contains("{tags}"),
        "not the legacy free text"
    );
}

#[tokio::test]
async fn foreign_and_unknown_backpack_are_404() {
    let app = common::spawn().await;
    bound_device(&app, app.seed.alice, "alice-box", ALICE_TOKEN).await;

    let bob_bp = backpacks::create_backpack(&app.pool, app.seed.bob, "Bob Only")
        .await
        .expect("create bob");

    // Alice's device cannot read bob's backpack (no existence leak).
    let (status, v) = json_body(
        app.client()
            .get(app.url(&format!("/api/hub/backpacks/{bob_bp}/export")))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("foreign export"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(v, json!({ "error": "unknown backpack" }));

    // Unknown id -> same 404.
    let (status, v) = json_body(
        app.client()
            .get(app.url("/api/hub/backpacks/999999/export"))
            .header("Authorization", format!("Bearer {ALICE_TOKEN}"))
            .send()
            .await
            .expect("unknown export"),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(v, json!({ "error": "unknown backpack" }));
}
