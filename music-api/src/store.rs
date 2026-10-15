//! Content-addressed object store.
//!
//! The caller supplies the **SHA-256 of the bytes it sends** as the key. The
//! store hashes what it actually receives and rejects a mismatch, so a `PUT`
//! can neither claim a wrong key nor corrupt an existing object. It never parses
//! audio: canonicalisation (what makes two files with different comments dedupe)
//! is the caller's job — here the key is simply whatever the caller computed.

use std::path::{Path, PathBuf};

use axum::Json;
use axum::body::Body;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::{Pool, Sqlite};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::io::ReaderStream;

use crate::AppState;

/// Hard cap on a single upload. Stems can be large; 4 GiB is well past them and
/// still bounds a hostile `PUT`.
pub const DEFAULT_MAX_UPLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Store {
    pub root: PathBuf,
    pub max_bytes: u64,
}

impl Store {
    pub fn new(root: PathBuf, max_bytes: u64) -> Self {
        Self { root, max_bytes }
    }

    /// `<root>/<aa>/<bb>/<hash>` — two levels of fan-out keep directories small.
    pub fn path_for(&self, hash: &str) -> PathBuf {
        self.root
            .join(&hash[0..2])
            .join(&hash[2..4])
            .join(hash)
    }

    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }
}

/// A key is exactly 64 lowercase hex characters (SHA-256).
pub fn valid_hash(h: &str) -> bool {
    h.len() == 64
        && h.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct ObjectMeta {
    pub hash: String,
    pub size: i64,
    pub content_type: Option<String>,
    pub original_path: Option<String>,
    pub isrc: Option<String>,
    pub name: Option<String>,
    pub group_key: Option<String>,
    pub stem_type: Option<String>,
    pub created_at: i64,
}

pub async fn init(pool: &Pool<Sqlite>, store: &Store) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS store_objects (
            hash          TEXT PRIMARY KEY,
            size          INTEGER NOT NULL,
            content_type  TEXT,
            original_path TEXT,
            isrc          TEXT,
            name          TEXT,
            group_key     TEXT,
            stem_type     TEXT,
            created_at    INTEGER NOT NULL
        );
        "#,
    )
    .execute(pool)
    .await?;

    // Additive columns for stores created before stem grouping existed.
    for col in ["group_key TEXT", "stem_type TEXT"] {
        let _ = sqlx::query(&format!("ALTER TABLE store_objects ADD COLUMN {col}"))
            .execute(pool)
            .await;
    }

    tokio::fs::create_dir_all(store.tmp_dir()).await?;
    Ok(())
}

/// Metadata for [`import_file`].
#[derive(Debug, Default, Clone)]
pub struct ImportMeta {
    pub content_type: Option<String>,
    pub original_path: Option<String>,
    pub isrc: Option<String>,
    pub name: Option<String>,
    /// Track the object belongs to (stem grouping).
    pub group_key: Option<String>,
    /// Stem part (`vocals`/`bass`/`drums`/`instrumental`/`other`) when applicable.
    pub stem_type: Option<String>,
}

/// Place the already-canonicalised file at `src` into the store under `hash` and
/// record its metadata. Idempotent: an existing object is left untouched (but its
/// metadata row is still ensured).
///
/// Returns `true` when the bytes were newly stored.
///
/// This is the filesystem/DB entry point used by the `store-import` migration; the
/// HTTP `PUT` handler is the normal path for MMM uploads.
///
/// The caller is responsible for handing in **canonicalised** bytes (see MMM's
/// `store::canonicalise_to`, which clears the Comment tag) so that the SHA-256
/// matches objects uploaded by MMM.
pub async fn import_file(
    pool: &Pool<Sqlite>,
    store: &Store,
    hash: &str,
    src: &Path,
    meta: &ImportMeta,
) -> anyhow::Result<bool> {
    if !valid_hash(hash) {
        anyhow::bail!("invalid hash {hash}");
    }
    let dest = store.path_for(hash);
    let size = std::fs::metadata(src)?.len();

    let stored = if dest.exists() {
        false
    } else {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Rename is cheapest when src already lives on the store's filesystem;
        // a staging directory elsewhere gives EXDEV, so fall back to a copy
        // through the store's own tmp dir (same filesystem as the destination).
        match std::fs::rename(src, &dest) {
            Ok(()) => true,
            Err(e) if e.raw_os_error() == Some(18) => {
                std::fs::create_dir_all(store.tmp_dir())?;
                let tmp = store
                    .tmp_dir()
                    .join(format!("{}-import", uuid::Uuid::new_v4()));
                std::fs::copy(src, &tmp)?;
                std::fs::rename(&tmp, &dest)?;
                let _ = std::fs::remove_file(src);
                true
            }
            Err(e) => return Err(e.into()),
        }
    };

    sqlx::query(
        "INSERT INTO store_objects
           (hash, size, content_type, original_path, isrc, name, group_key, stem_type, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(hash) DO UPDATE SET
           group_key = COALESCE(store_objects.group_key, excluded.group_key),
           stem_type = COALESCE(store_objects.stem_type, excluded.stem_type)",
    )
    .bind(hash)
    .bind(size as i64)
    .bind(&meta.content_type)
    .bind(&meta.original_path)
    .bind(&meta.isrc)
    .bind(&meta.name)
    .bind(&meta.group_key)
    .bind(&meta.stem_type)
    .bind(crate::db::now())
    .execute(pool)
    .await?;

    Ok(stored)
}

// ── Handlers ───────────────────────────────────────────────────────────────

fn bad_request(msg: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": msg })),
    )
        .into_response()
}

/// `PUT /objects/{sha256}` — store the body under its own digest.
pub async fn put(
    State(state): State<Arc<AppState>>,
    UrlPath(hash): UrlPath<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if !valid_hash(&hash) {
        return bad_request("key must be a 64-char lowercase hex sha256");
    }
    let dest = state.store.path_for(&hash);

    let hdr = |k: &str| {
        headers
            .get(k)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
    };

    // Optional stem-grouping metadata (#359). Absent headers leave the columns
    // untouched (COALESCE), so a bare re-upload never wipes them.
    let group_key = hdr("x-group-key");
    let stem_type = hdr("x-stem-type");

    // Idempotent: an existing object is never rewritten. Drain the body so the
    // client sees a clean response.
    if let Ok(meta) = tokio::fs::metadata(&dest).await {
        let mut stream = body.into_data_stream();
        while stream.next().await.is_some() {}
        // The file is the source of truth for `GET`; ensure a row exists so
        // orphan-prune cannot later reclaim a live object (e.g. after an earlier
        // insert failed). An existing row is left untouched.
        if let Err(e) = sqlx::query(
            "INSERT INTO store_objects (hash, size, content_type, original_path, isrc, name, group_key, stem_type, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(hash) DO UPDATE SET
               group_key = COALESCE(store_objects.group_key, excluded.group_key),
               stem_type = COALESCE(store_objects.stem_type, excluded.stem_type)",
        )
        .bind(&hash)
        .bind(meta.len() as i64)
        .bind(hdr("content-type"))
        .bind(hdr("x-original-path"))
        .bind(hdr("x-isrc"))
        .bind(hdr("x-name"))
        .bind(&group_key)
        .bind(&stem_type)
        .bind(crate::db::now())
        .execute(&state.pool)
        .await
        {
            return internal(format!("recording object: {e}"));
        }
        return (
            StatusCode::OK,
            Json(serde_json::json!({ "stored": false, "present": true })),
        )
            .into_response();
    }

    let tmp = state
        .store
        .tmp_dir()
        .join(format!("{}-{}", uuid::Uuid::new_v4(), hash));
    if let Err(e) = tokio::fs::create_dir_all(state.store.tmp_dir()).await {
        return internal(format!("creating tmp dir: {e}"));
    }

    let mut file = match tokio::fs::File::create(&tmp).await {
        Ok(f) => f,
        Err(e) => return internal(format!("creating temp file: {e}")),
    };

    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    let mut stream = body.into_data_stream();

    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => {
                let _ = tokio::fs::remove_file(&tmp).await;
                return bad_request(&format!("read error: {e}"));
            }
        };
        written += chunk.len() as u64;
        if written > state.store.max_bytes {
            let _ = tokio::fs::remove_file(&tmp).await;
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                Json(serde_json::json!({ "error": "upload exceeds the size limit" })),
            )
                .into_response();
        }
        hasher.update(&chunk);
        if let Err(e) = file.write_all(&chunk).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return internal(format!("writing temp file: {e}"));
        }
    }
    if let Err(e) = file.flush().await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return internal(format!("flushing temp file: {e}"));
    }
    drop(file);

    let digest = hex(&hasher.finalize());
    if digest != hash {
        let _ = tokio::fs::remove_file(&tmp).await;
        return bad_request("sha256 of the body does not match the key");
    }

    if let Some(parent) = dest.parent() {
        if let Err(e) = tokio::fs::create_dir_all(parent).await {
            let _ = tokio::fs::remove_file(&tmp).await;
            return internal(format!("creating object dir: {e}"));
        }
    }

    // Record the row *before* publishing the file: "file without a row" is the
    // one state orphan-prune reclaims, so the row must never be the step that
    // fails silently. If it does, the PUT fails and nothing is published
    // (concurrent scans are additionally covered by the grace period in
    // `prune_orphans`).
    if let Err(e) = sqlx::query(
        "INSERT INTO store_objects (hash, size, content_type, original_path, isrc, name, group_key, stem_type, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(hash) DO UPDATE SET
           group_key = COALESCE(store_objects.group_key, excluded.group_key),
           stem_type = COALESCE(store_objects.stem_type, excluded.stem_type)",
    )
    .bind(&hash)
    .bind(written as i64)
    .bind(hdr("content-type"))
    .bind(hdr("x-original-path"))
    .bind(hdr("x-isrc"))
    .bind(hdr("x-name"))
    .bind(&group_key)
    .bind(&stem_type)
    .bind(crate::db::now())
    .execute(&state.pool)
    .await
    {
        let _ = tokio::fs::remove_file(&tmp).await;
        return internal(format!("recording object: {e}"));
    }

    if let Err(e) = tokio::fs::rename(&tmp, &dest).await {
        // The bytes were never published; drop the row we just wrote so we do
        // not leave a dangling row behind.
        let _ = sqlx::query("DELETE FROM store_objects WHERE hash = ?")
            .bind(&hash)
            .execute(&state.pool)
            .await;
        let _ = tokio::fs::remove_file(&tmp).await;
        return internal(format!("moving object into place: {e}"));
    }

    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "stored": true, "size": written })),
    )
        .into_response()
}

/// `HEAD /objects/{sha256}` — is it backed up?
pub async fn head(
    State(state): State<Arc<AppState>>,
    UrlPath(hash): UrlPath<String>,
) -> Response {
    if !valid_hash(&hash) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    match tokio::fs::metadata(state.store.path_for(&hash)).await {
        Ok(m) => {
            let mut resp = StatusCode::OK.into_response();
            if let Ok(v) = header::HeaderValue::from_str(&m.len().to_string()) {
                resp.headers_mut().insert(header::CONTENT_LENGTH, v);
            }
            resp
        }
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// `GET /objects/{sha256}` — download, with single-range support.
pub async fn get(
    State(state): State<Arc<AppState>>,
    UrlPath(hash): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    if !valid_hash(&hash) {
        return bad_request("key must be a 64-char lowercase hex sha256");
    }
    stream_object(&state.store, &hash, "application/octet-stream", &headers).await
}

fn parse_range(headers: &HeaderMap, total: u64) -> Option<(u64, u64)> {
    let raw = headers.get(header::RANGE)?.to_str().ok()?;
    let spec = raw.strip_prefix("bytes=")?;
    if spec.contains(',') {
        return None; // multi-range: serve whole
    }
    let (a, b) = spec.split_once('-')?;
    let start: u64 = if a.is_empty() { 0 } else { a.parse().ok()? };
    let end: u64 = if b.is_empty() {
        total.saturating_sub(1)
    } else {
        b.parse().ok()?
    };
    Some((start, end.min(total.saturating_sub(1))))
}

#[derive(Debug, Deserialize)]
pub struct CheckRequest {
    pub hashes: Vec<String>,
}

/// `POST /objects/check` — which of these keys are present?
pub async fn check(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CheckRequest>,
) -> Response {
    let mut present = Vec::new();
    let mut missing = Vec::new();
    for hash in req.hashes {
        if !valid_hash(&hash) {
            missing.push(hash);
            continue;
        }
        if tokio::fs::metadata(state.store.path_for(&hash)).await.is_ok() {
            present.push(hash);
        } else {
            missing.push(hash);
        }
    }
    Json(serde_json::json!({ "present": present, "missing": missing })).into_response()
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    offset: Option<i64>,
    limit: Option<i64>,
}

/// `GET /objects` — paged listing for browsing/restore.
pub async fn list(State(state): State<Arc<AppState>>, Query(q): Query<ListQuery>) -> Response {
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let offset = q.offset.unwrap_or(0).max(0);
    let total: i64 = match sqlx::query_scalar("SELECT COUNT(*) FROM store_objects")
        .fetch_one(&state.pool)
        .await
    {
        Ok(n) => n,
        Err(e) => return internal(format!("counting objects: {e}")),
    };
    let objects: Vec<ObjectMeta> = match sqlx::query_as(
        "SELECT hash, size, content_type, original_path, isrc, name, group_key, stem_type, created_at
           FROM store_objects ORDER BY created_at DESC LIMIT ? OFFSET ?",
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(&state.pool)
    .await
    {
        Ok(o) => o,
        Err(e) => return internal(format!("listing objects: {e}")),
    };

    Json(serde_json::json!({ "objects": objects, "total": total })).into_response()
}

// ── ISRC → object variants + store-backed file fallback (#359) ───────────────

/// Read-only variant kind used by the `/isrc/{isrc}/objects` view (#359).
///
/// An object with a `stem_type` is a `stem`; otherwise the kind is derived from
/// the extension of `name`/`original_path` (`flac`, `mp3`, `audio` for
/// m4a/wav/aiff/ogg, else `other`). No fuzzy derivation from file names.
pub fn classify_kind(
    stem_type: Option<&str>,
    name: Option<&str>,
    original_path: Option<&str>,
) -> &'static str {
    if stem_type.map(|s| !s.trim().is_empty()).unwrap_or(false) {
        return "stem";
    }
    let ext_of = |p: Option<&str>| -> Option<String> {
        let p = p?;
        let base = p.rsplit('/').next().unwrap_or(p);
        let (_, e) = base.rsplit_once('.')?;
        Some(e.to_ascii_lowercase())
    };
    match ext_of(name).or_else(|| ext_of(original_path)).as_deref() {
        Some("flac") => "flac",
        Some("mp3") => "mp3",
        Some("m4a") | Some("wav") | Some("aiff") | Some("ogg") => "audio",
        _ => "other",
    }
}

fn variant_json(o: &ObjectMeta) -> serde_json::Value {
    let kind = classify_kind(
        o.stem_type.as_deref(),
        o.name.as_deref(),
        o.original_path.as_deref(),
    );
    serde_json::json!({
        "hash": o.hash,
        "kind": kind,
        "size": o.size,
        "contentType": o.content_type,
        "name": o.name,
        "originalPath": o.original_path,
        "stemType": o.stem_type,
        "groupKey": o.group_key,
        "createdAt": o.created_at,
    })
}

const OBJECT_COLUMNS: &str = "hash, size, content_type, original_path, isrc, name, group_key, stem_type, created_at";

/// `GET /isrc/{isrc}/objects` — resolve an ISRC to its object variants (#359).
///
/// `404` only when the ISRC is entirely unknown (no track **and** no object); a
/// known-but-empty ISRC answers `200` with `count: 0`.
pub async fn isrc_objects(
    State(state): State<Arc<AppState>>,
    UrlPath(isrc): UrlPath<String>,
) -> Response {
    let isrc = crate::db::normalize_key(&isrc);
    let track = crate::db::get_track(&state.pool, &isrc).await.ok().flatten();

    let primary: Vec<ObjectMeta> = match sqlx::query_as(&format!(
        "SELECT {OBJECT_COLUMNS} FROM store_objects WHERE isrc = ? ORDER BY created_at DESC"
    ))
    .bind(&isrc)
    .fetch_all(&state.pool)
    .await
    {
        Ok(v) => v,
        Err(e) => return internal(format!("listing isrc objects: {e}")),
    };

    if track.is_none() && primary.is_empty() {
        return StatusCode::NOT_FOUND.into_response();
    }

    // Stems are linked best-effort via the group_key of this ISRC's own objects
    // (set by `store_import`). No filename heuristics.
    let mut group_keys: Vec<String> = primary.iter().filter_map(|o| o.group_key.clone()).collect();
    group_keys.sort();
    group_keys.dedup();

    let mut stems: Vec<ObjectMeta> = Vec::new();
    for gk in &group_keys {
        let rows: Vec<ObjectMeta> = sqlx::query_as(&format!(
            "SELECT {OBJECT_COLUMNS} FROM store_objects
              WHERE stem_type IS NOT NULL AND group_key = ?
              ORDER BY created_at DESC"
        ))
        .bind(gk)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
        stems.extend(rows);
    }

    let variants: Vec<serde_json::Value> = primary.iter().map(variant_json).collect();
    let groups: Vec<serde_json::Value> = group_keys
        .iter()
        .map(|gk| {
            let gstems: Vec<serde_json::Value> = stems
                .iter()
                .filter(|s| s.group_key.as_deref() == Some(gk.as_str()))
                .map(variant_json)
                .collect();
            (gk, gstems)
        })
        .filter(|(_, s)| !s.is_empty())
        .map(|(gk, s)| serde_json::json!({ "groupKey": gk, "stems": s }))
        .collect();

    Json(
        serde_json::json!({ "isrc": isrc, "count": variants.len(), "variants": variants, "groups": groups }),
    )
    .into_response()
}

/// The store object backing `GET /isrc/{isrc}/{format}` when no local file is
/// available (#359). `flac` → kind `flac`; `320`/`128` → kind `mp3` (the newest
/// such object: the store keeps no bitrate).
pub struct ResolvedFile {
    pub hash: String,
    pub content_type: Option<String>,
}

/// Resolve an ISRC + delivery format to a servable store object (#359).
/// `None` when nothing matches or the object file is absent.
pub async fn resolve_isrc_object(
    pool: &Pool<Sqlite>,
    store: &Store,
    isrc: &str,
    format: &str,
) -> Option<ResolvedFile> {
    let want = match format {
        "flac" => "flac",
        "320" | "128" => "mp3",
        _ => return None,
    };
    let rows: Vec<ObjectMeta> = sqlx::query_as(&format!(
        "SELECT {OBJECT_COLUMNS} FROM store_objects WHERE isrc = ? ORDER BY created_at DESC"
    ))
    .bind(isrc)
    .fetch_all(pool)
    .await
    .ok()?;
    for o in rows {
        if classify_kind(o.stem_type.as_deref(), o.name.as_deref(), o.original_path.as_deref())
            != want
        {
            continue;
        }
        if tokio::fs::metadata(store.path_for(&o.hash)).await.is_ok() {
            return Some(ResolvedFile {
                hash: o.hash,
                content_type: o.content_type,
            });
        }
    }
    None
}

/// Stream an object file with single-range support and an explicit
/// `Content-Type`. Returns `404` when the file is absent.
pub async fn stream_object(
    store: &Store,
    hash: &str,
    content_type: &'static str,
    headers: &HeaderMap,
) -> Response {
    let path = store.path_for(hash);
    let mut file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let total = match file.metadata().await {
        Ok(m) => m.len(),
        Err(e) => return internal(format!("stat object: {e}")),
    };

    if let Some((start, end)) = parse_range(headers, total) {
        if start >= total || start > end {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{total}"))],
            )
                .into_response();
        }
        if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return internal("seeking object");
        }
        let len = end - start + 1;
        let stream = ReaderStream::new(file.take(len));
        let mut resp = Body::from_stream(stream).into_response();
        resp.headers_mut()
            .insert(header::CONTENT_TYPE, header::HeaderValue::from_static(content_type));
        resp.headers_mut().insert(
            header::CONTENT_RANGE,
            header::HeaderValue::from_str(&format!("bytes {start}-{end}/{total}")).unwrap(),
        );
        resp.headers_mut().insert(
            header::CONTENT_LENGTH,
            header::HeaderValue::from_str(&len.to_string()).unwrap(),
        );
        *resp.status_mut() = StatusCode::PARTIAL_CONTENT;
        return resp;
    }

    let stream = ReaderStream::new(file);
    let mut resp = Body::from_stream(stream).into_response();
    resp.headers_mut()
        .insert(header::CONTENT_TYPE, header::HeaderValue::from_static(content_type));
    resp.headers_mut().insert(
        header::CONTENT_LENGTH,
        header::HeaderValue::from_str(&total.to_string()).unwrap(),
    );
    resp
}

// ── Prune: delete / bulk prune / orphan reconcile / tmp sweep / stats ────────

/// Default age after which entries in [`Store::tmp_dir`] are treated as
/// abandoned uploads by [`sweep_tmp`].
pub const DEFAULT_TMP_MAX_AGE_SECS: u64 = 60 * 60;

/// Default grace period during which reconcile ([`prune_orphans`]) leaves
/// file-/row-side-only objects alone. A `PUT` publishes its row and file a
/// moment apart, so a just-started upload can legitimately look like a
/// half-object to a concurrent scan; within the grace window nothing is
/// reclaimed.
pub const DEFAULT_ORPHAN_MIN_AGE_SECS: u64 = 60 * 60;

/// Size of an object in bytes, preferring the file on disk and falling back to
/// the recorded row. `None` when neither file nor row exists.
pub async fn object_size(pool: &Pool<Sqlite>, store: &Store, hash: &str) -> Option<u64> {
    if let Ok(m) = tokio::fs::metadata(store.path_for(hash)).await {
        return Some(m.len());
    }
    let size: Option<i64> = sqlx::query_scalar("SELECT size FROM store_objects WHERE hash = ?")
        .bind(hash)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    size.map(|s| s.max(0) as u64)
}

/// Remove the object file **and** its `store_objects` row.
///
/// Returns `true` when something was removed, `false` when neither file nor row
/// existed. A file-removal error other than "not found" propagates unchanged and
/// leaves the row in place. The reverse is best-effort: if the `DELETE` fails
/// after the file is already gone, the row is left dangling (file removed, row
/// present) — a non-destructive half-state that [`prune_orphans`] reconciles.
pub async fn delete_object(
    pool: &Pool<Sqlite>,
    store: &Store,
    hash: &str,
) -> anyhow::Result<bool> {
    let mut removed_file = false;
    match tokio::fs::remove_file(store.path_for(hash)).await {
        Ok(()) => removed_file = true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let res = sqlx::query("DELETE FROM store_objects WHERE hash = ?")
        .bind(hash)
        .execute(pool)
        .await?;
    Ok(removed_file || res.rows_affected() > 0)
}

/// `DELETE /objects/{sha256}` — remove one object (file + row).
pub async fn delete(
    State(state): State<Arc<AppState>>,
    UrlPath(hash): UrlPath<String>,
) -> Response {
    if !valid_hash(&hash) {
        return bad_request("key must be a 64-char lowercase hex sha256");
    }
    match delete_object(&state.pool, &state.store, &hash).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal(format!("deleting object: {e}")),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneRequest {
    #[serde(default)]
    pub hashes: Vec<String>,
    #[serde(default)]
    pub dry_run: bool,
}

/// `POST /objects/prune` — consumer-driven bulk delete of explicit keys.
///
/// `dryRun` reports what would happen without touching anything; `freedBytes`
/// counts the bytes actually released (or that *would* be, for `dryRun`).
/// Idempotent: absent or invalid keys land in `missing`.
pub async fn prune(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PruneRequest>,
) -> Response {
    let mut deleted: Vec<String> = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    let mut freed_bytes: u64 = 0;

    for hash in req.hashes {
        if !valid_hash(&hash) {
            missing.push(hash);
            continue;
        }
        if req.dry_run {
            match object_size(&state.pool, &state.store, &hash).await {
                Some(size) => {
                    freed_bytes += size;
                    deleted.push(hash);
                }
                None => missing.push(hash),
            }
        } else {
            let size = object_size(&state.pool, &state.store, &hash).await;
            match delete_object(&state.pool, &state.store, &hash).await {
                Ok(true) => {
                    freed_bytes += size.unwrap_or(0);
                    deleted.push(hash);
                }
                Ok(false) => missing.push(hash),
                Err(e) => return internal(format!("pruning object: {e}")),
            }
        }
    }

    Json(serde_json::json!({
        "deleted": deleted,
        "missing": missing,
        "freedBytes": freed_bytes,
        "dryRun": req.dry_run,
    }))
    .into_response()
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PruneOrphansRequest {
    #[serde(default)]
    pub dry_run: bool,
    /// Opt-in: also sweep abandoned uploads under `tmp/` (never on `dryRun`).
    #[serde(default)]
    pub include_tmp: bool,
    /// Grace period: only reconcile objects older than this many seconds.
    /// Defaults to [`DEFAULT_ORPHAN_MIN_AGE_SECS`]; `0` disables the grace
    /// window.
    #[serde(default = "default_orphan_min_age_secs")]
    pub min_age_secs: u64,
}

fn default_orphan_min_age_secs() -> u64 {
    DEFAULT_ORPHAN_MIN_AGE_SECS
}

fn hex2(s: &str) -> bool {
    s.len() == 2
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `POST /objects/prune/orphans` — reconcile the file tree against the rows.
///
/// Removes (a) object files with no `store_objects` row and (b) `store_objects`
/// rows with no file. `tmp/` is excluded unless `includeTmp` is set.
pub async fn prune_orphans(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PruneOrphansRequest>,
) -> Response {
    let dry = req.dry_run;
    let grace = std::time::Duration::from_secs(req.min_age_secs);
    let now_secs = crate::db::now();
    let mut files_removed: u64 = 0;
    let mut rows_removed: u64 = 0;
    let mut freed_bytes: u64 = 0;

    // Load the full row set up front and fail closed: a DB hiccup must never be
    // read as "no row" for a live file. If the rows cannot be read, abort the
    // run before deleting anything.
    let rows: Vec<(String, i64, i64)> =
        match sqlx::query_as("SELECT hash, size, created_at FROM store_objects")
            .fetch_all(&state.pool)
            .await
        {
            Ok(r) => r,
            Err(e) => return internal(format!("listing objects for orphan scan: {e}")),
        };
    let known: std::collections::HashSet<String> =
        rows.iter().map(|(h, _, _)| h.clone()).collect();

    // (a) Object files without a row: walk <root>/<aa>/<bb>/<hash>.
    if let Ok(mut aa_dir) = tokio::fs::read_dir(&state.store.root).await {
        while let Ok(Some(aa)) = aa_dir.next_entry().await {
            let aa_path = aa.path();
            if !aa_path.is_dir() {
                continue;
            }
            if !aa.file_name().into_string().map(|n| hex2(&n)).unwrap_or(false) {
                continue;
            }
            let mut bb_dir = match tokio::fs::read_dir(&aa_path).await {
                Ok(d) => d,
                Err(_) => continue,
            };
            while let Ok(Some(bb)) = bb_dir.next_entry().await {
                let bb_path = bb.path();
                if !bb_path.is_dir() {
                    continue;
                }
                if !bb.file_name().into_string().map(|n| hex2(&n)).unwrap_or(false) {
                    continue;
                }
                let mut files = match tokio::fs::read_dir(&bb_path).await {
                    Ok(d) => d,
                    Err(_) => continue,
                };
                while let Ok(Some(entry)) = files.next_entry().await {
                    let hash = match entry.file_name().into_string() {
                        Ok(n) if valid_hash(&n) => n,
                        _ => continue,
                    };
                    if known.contains(hash.as_str()) {
                        continue;
                    }
                    // Grace period: an in-flight `PUT` may not have published
                    // its row+file pair yet, so leave young files alone. An
                    // unreadable mtime counts as too young (fail-closed).
                    let meta = match entry.metadata().await {
                        Ok(m) => m,
                        Err(_) => continue,
                    };
                    let too_young = meta
                        .modified()
                        .ok()
                        .and_then(|m| std::time::SystemTime::now().duration_since(m).ok())
                        .map(|age| age < grace)
                        .unwrap_or(true);
                    if too_young {
                        continue;
                    }
                    let size = meta.len();
                    if dry {
                        files_removed += 1;
                        freed_bytes += size;
                    } else if tokio::fs::remove_file(entry.path()).await.is_ok() {
                        files_removed += 1;
                        freed_bytes += size;
                    }
                }
            }
        }
    }

    // (b) Rows without a file.
    for (hash, size, created_at) in rows {
        let present = match tokio::fs::metadata(state.store.path_for(&hash)).await {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            // Any other stat error (e.g. permissions) is treated as present so
            // a valid object is never dropped by mistake.
            Err(_) => true,
        };
        if present {
            continue;
        }
        // Same grace period on the row side: `PUT` writes its row just before
        // the file lands, so a row this young is not an orphan yet.
        if now_secs.saturating_sub(created_at) < req.min_age_secs as i64 {
            continue;
        }
        if dry {
            rows_removed += 1;
            freed_bytes += size.max(0) as u64;
        } else {
            let res = sqlx::query("DELETE FROM store_objects WHERE hash = ?")
                .bind(&hash)
                .execute(&state.pool)
                .await;
            if res.map(|r| r.rows_affected() > 0).unwrap_or(false) {
                rows_removed += 1;
                freed_bytes += size.max(0) as u64;
            }
        }
    }

    // Opt-in, non-dry sweep of abandoned uploads under tmp/.
    if req.include_tmp && !dry {
        if let Ok((files, bytes)) = sweep_tmp(
            &state.store,
            std::time::Duration::from_secs(DEFAULT_TMP_MAX_AGE_SECS),
        )
        .await
        {
            files_removed += files;
            freed_bytes += bytes;
        }
    }

    Json(serde_json::json!({
        "filesRemoved": files_removed,
        "rowsRemoved": rows_removed,
        "freedBytes": freed_bytes,
        "dryRun": dry,
    }))
    .into_response()
}

/// Delete entries directly under `store.tmp_dir()` older than `max_age`.
///
/// Bounded (only the flat tmp directory is scanned) and never auto-triggered on
/// the request path unless the caller opts in. Returns `(files_removed, bytes)`.
pub async fn sweep_tmp(
    store: &Store,
    max_age: std::time::Duration,
) -> anyhow::Result<(u64, u64)> {
    let mut dir = match tokio::fs::read_dir(store.tmp_dir()).await {
        Ok(d) => d,
        Err(_) => return Ok((0, 0)),
    };
    let now = std::time::SystemTime::now();
    let mut files: u64 = 0;
    let mut bytes: u64 = 0;
    while let Ok(Some(entry)) = dir.next_entry().await {
        let meta = match entry.metadata().await {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_file() {
            continue;
        }
        let old_enough = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .map(|age| age >= max_age)
            .unwrap_or(false);
        if old_enough && tokio::fs::remove_file(entry.path()).await.is_ok() {
            files += 1;
            bytes += meta.len();
        }
    }
    Ok((files, bytes))
}

/// `GET /objects/stats` — object count and total stored bytes.
pub async fn stats(State(state): State<Arc<AppState>>) -> Response {
    match sqlx::query_as::<_, (i64, i64)>(
        "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM store_objects",
    )
    .fetch_one(&state.pool)
    .await
    {
        Ok((count, total)) => {
            Json(serde_json::json!({ "count": count, "totalBytes": total })).into_response()
        }
        Err(e) => internal(format!("reading object stats: {e}")),
    }
}

fn internal(msg: impl std::fmt::Display) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({ "error": msg.to_string() })),
    )
        .into_response()
}

/// Used by the binary to lay out the store on disk.
pub fn root_exists(root: &Path) -> bool {
    root.exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_validation() {
        assert!(valid_hash(&"a".repeat(64)));
        assert!(valid_hash(&"0123456789abcdef".repeat(4)));
        assert!(!valid_hash(&"A".repeat(64)), "uppercase must be rejected");
        assert!(!valid_hash(&"a".repeat(63)));
        assert!(!valid_hash(&"g".repeat(64)));
    }

    #[test]
    fn sharding_and_hex() {
        let s = Store::new(PathBuf::from("/root"), 10);
        let h = "abcdef0123456789".repeat(4);
        let p = s.path_for(&h);
        assert_eq!(p, PathBuf::from(format!("/root/ab/cd/{h}")));
        assert_eq!(hex(&[0x00, 0xff, 0x1a]), "00ff1a");
    }

    #[test]
    fn range_parsing() {
        let mut h = HeaderMap::new();
        h.insert(header::RANGE, "bytes=10-19".parse().unwrap());
        assert_eq!(parse_range(&h, 100), Some((10, 19)));

        h.insert(header::RANGE, "bytes=90-".parse().unwrap());
        assert_eq!(parse_range(&h, 100), Some((90, 99)));

        // Clamped to the end.
        h.insert(header::RANGE, "bytes=0-1000".parse().unwrap());
        assert_eq!(parse_range(&h, 100), Some((0, 99)));

        // Multi-range falls back to the whole file.
        h.insert(header::RANGE, "bytes=0-1,5-6".parse().unwrap());
        assert_eq!(parse_range(&h, 100), None);
    }
}
