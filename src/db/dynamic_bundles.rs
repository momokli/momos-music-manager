//! Dynamic bundle database queries — CRUD + resolution.
//!
//! Dynamic bundles define filter criteria (base tags, BPM range, PMV categories,
//! file types) that are evaluated to determine which files belong to the bundle.
//! The resolution feeds into `file_resolved_tags` via `refresh_file_resolved_tags()`.

use anyhow::Result;
use sqlx::{Pool, Sqlite, SqliteConnection};

use super::types::*;

// ── Resolution ─────────────────────────────────────────────────────────────

/// Resolve a dynamic bundle's filter criteria against the files table.
/// Returns the file IDs that match all active filters.
pub async fn resolve_dynamic_bundle(pool: &Pool<Sqlite>, db: &DynamicBundle) -> Result<Vec<i64>> {
    let (sql, string_binds, int_binds, f64_binds) = build_resolve_sql(db);
    let mut q = sqlx::query_as::<_, (i64,)>(&sql);
    for v in &string_binds {
        q = q.bind(v.as_str());
    }
    for v in &int_binds {
        q = q.bind(*v);
    }
    for v in &f64_binds {
        q = q.bind(*v);
    }
    let rows: Vec<(i64,)> = q.fetch_all(pool).await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// Same as `resolve_dynamic_bundle` but runs inside an existing transaction.
pub async fn resolve_dynamic_bundle_in_tx(
    tx: &mut SqliteConnection,
    db: &DynamicBundle,
) -> Result<Vec<i64>> {
    let (sql, string_binds, int_binds, f64_binds) = build_resolve_sql(db);
    let mut q = sqlx::query_as::<_, (i64,)>(&sql);
    for v in &string_binds {
        q = q.bind(v.as_str());
    }
    for v in &int_binds {
        q = q.bind(*v);
    }
    for v in &f64_binds {
        q = q.bind(*v);
    }
    let rows: Vec<(i64,)> = q.fetch_all(&mut *tx).await?;
    Ok(rows.into_iter().map(|r| r.0).collect())
}

/// Build the resolution SQL and collect bind values.
/// Returns (sql_string, string_bind_values, int_bind_values, f64_bind_values).
/// BIND ORDER: string binds first, then int binds, then f64 binds.
/// WHERE clause order matches this: keys(string), pmv(string), base_tags(string),
/// rating_min(int), play_count_min(int), bpm(f64).
fn build_resolve_sql(db: &DynamicBundle) -> (String, Vec<String>, Vec<i64>, Vec<f64>) {
    let mut sql = String::from("SELECT DISTINCT vft.file_id FROM v_file_track_link vft");
    let mut string_binds: Vec<String> = Vec::new();
    let mut int_binds: Vec<i64> = Vec::new();
    let mut f64_binds: Vec<f64> = Vec::new();
    let mut needs_where = false;

    let mut push_where = |sql: &mut String, clause: &str, flag: &mut bool| {
        if *flag {
            sql.push_str(" AND ");
        } else {
            sql.push_str(" WHERE ");
            *flag = true;
        }
        sql.push_str(clause);
    };

    // ── Keys filter (string binds first) ──
    if let Some(ref keys_json) = db.keys {
        if let Ok(kv) = serde_json::from_str::<Vec<String>>(keys_json) {
            if !kv.is_empty() {
                let placeholders: Vec<String> = kv.iter().map(|_| "?".to_string()).collect();
                push_where(
                    &mut sql,
                    &format!(
                        r#"EXISTS (
                            SELECT 1 FROM v_file_track_link vft_k
                            JOIN files f_k ON f_k.id = vft_k.file_id
                            WHERE vft_k.track_id = vft.track_id
                              AND f_k.musical_key IN ({})
                        )"#,
                        placeholders.join(",")
                    ),
                    &mut needs_where,
                );
                for k in kv {
                    string_binds.push(k);
                }
            }
        }
    }

    // ── PMV categories (string binds) ──
    if let Some(ref pmv_json) = db.pmv_categories {
        if let Ok(categories) = serde_json::from_str::<Vec<String>>(pmv_json) {
            if !categories.is_empty() {
                let placeholders: Vec<String> =
                    categories.iter().map(|_| "?".to_string()).collect();
                push_where(
                    &mut sql,
                    &format!(
                        r#"EXISTS (
                            SELECT 1 FROM track_resolved_tags trt
                            WHERE trt.track_id = vft.track_id
                              AND LOWER(trt.prefix) IN ({})
                        )"#,
                        placeholders.join(",")
                    ),
                    &mut needs_where,
                );
                for p in categories {
                    string_binds.push(p.to_lowercase());
                }
            }
        }
    }

    // ── Base track filter (string binds) ──
    if !db.include_all_tracks {
        if let Some(ref base_tags_json) = db.base_tags {
            if let Ok(tags) = serde_json::from_str::<Vec<String>>(base_tags_json) {
                if !tags.is_empty() {
                    let placeholders: Vec<String> = tags.iter().map(|_| "?".to_string()).collect();
                    push_where(
                        &mut sql,
                        &format!(
                            r#"vft.track_id IN (
                                SELECT DISTINCT spt.track_id
                                FROM service_playlist_tracks spt
                                JOIN service_playlists sp ON sp.id = spt.playlist_id
                                WHERE (sp.archive_deleted = 1 OR spt.deleted_at IS NULL)
                                  AND LOWER(TRIM(sp.name)) IN ({})
                            )"#,
                            placeholders.join(",")
                        ),
                        &mut needs_where,
                    );
                    for t in tags {
                        string_binds.push(t.to_lowercase());
                    }
                }
            }
        }
    }

    // ── Rating minimum (int binds) ──
    if let Some(rating_min) = db.rating_min {
        push_where(
            &mut sql,
            "EXISTS (SELECT 1 FROM v_file_track_link vft_r JOIN files f_r ON f_r.id = vft_r.file_id WHERE vft_r.track_id = vft.track_id AND f_r.rating >= ?)",
            &mut needs_where,
        );
        int_binds.push(rating_min);
    }

    // ── Play count minimum (int binds) ──
    if let Some(pc_min) = db.play_count_min {
        push_where(
            &mut sql,
            "EXISTS (SELECT 1 FROM v_file_track_link vft_p JOIN files f_p ON f_p.id = vft_p.file_id WHERE vft_p.track_id = vft.track_id AND f_p.play_count >= ?)",
            &mut needs_where,
        );
        int_binds.push(pc_min);
    }

    // ── BPM range (f64 binds) ──
    if let Some(bpm_min) = db.bpm_min {
        push_where(
            &mut sql,
            "EXISTS (SELECT 1 FROM files f WHERE f.id = vft.file_id AND f.bpm >= ?)",
            &mut needs_where,
        );
        f64_binds.push(bpm_min);
    }
    if let Some(bpm_max) = db.bpm_max {
        push_where(
            &mut sql,
            "EXISTS (SELECT 1 FROM files f WHERE f.id = vft.file_id AND f.bpm <= ?)",
            &mut needs_where,
        );
        f64_binds.push(bpm_max);
    }

    (sql, string_binds, int_binds, f64_binds)
}

// ── CRUD ───────────────────────────────────────────────────────────────────

/// List all dynamic bundles.
pub async fn get_dynamic_bundles(pool: &Pool<Sqlite>) -> Result<Vec<DynamicBundle>> {
    let bundles = sqlx::query_as::<_, DynamicBundle>("SELECT * FROM dynamic_bundles ORDER BY name")
        .fetch_all(pool)
        .await?;
    Ok(bundles)
}

/// Get a single dynamic bundle by ID.
pub async fn get_dynamic_bundle(pool: &Pool<Sqlite>, id: i64) -> Result<Option<DynamicBundle>> {
    let bundle = sqlx::query_as::<_, DynamicBundle>("SELECT * FROM dynamic_bundles WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(bundle)
}

/// Create a dynamic bundle.
///
/// 1. Creates a Setlist-category tag with the given name.
/// 2. Inserts the dynamic bundle row referencing the tag.
/// 3. Returns the created bundle (without re-resolving).
pub async fn create_dynamic_bundle(
    pool: &Pool<Sqlite>,
    name: &str,
    base_tags: Option<Vec<String>>,
    include_all_tracks: bool,
    bpm_min: Option<f64>,
    bpm_max: Option<f64>,
    pmv_categories: Option<Vec<String>>,
    file_types: Option<Vec<String>>,
    exclude_wav_sources: bool,
    keys: Option<Vec<String>>,
    rating_min: Option<i64>,
    play_count_min: Option<i64>,
) -> Result<DynamicBundle> {
    // Find Setlist category
    let cat_id: i64 = sqlx::query_scalar("SELECT id FROM tag_categories WHERE name = 'Setlist'")
        .fetch_one(pool)
        .await?;

    // Create the tag
    let tag = crate::db::tags::create_tag(pool, name, cat_id).await?;

    // Serialize JSON arrays
    let base_tags_json = base_tags
        .filter(|v| !v.is_empty())
        .map(|v| serde_json::to_string(&v).unwrap_or_default());
    let pmv_categories_json = pmv_categories
        .filter(|v| !v.is_empty())
        .map(|v| serde_json::to_string(&v).unwrap_or_default());
    let file_types_json = file_types
        .filter(|v| !v.is_empty())
        .map(|v| serde_json::to_string(&v).unwrap_or_default());
    let keys_json = keys
        .filter(|v| !v.is_empty())
        .map(|v| serde_json::to_string(&v).unwrap_or_default());

    let now = chrono::Utc::now().timestamp();

    let bundle = sqlx::query_as::<_, DynamicBundle>(
        r#"
        INSERT INTO dynamic_bundles (name, tag_id, base_tags, include_all_tracks, bpm_min, bpm_max, pmv_categories, file_types, exclude_wav_sources, keys, rating_min, play_count_min, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        RETURNING *
        "#,
    )
    .bind(name)
    .bind(tag.id)
    .bind(base_tags_json)
    .bind(include_all_tracks)
    .bind(bpm_min)
    .bind(bpm_max)
    .bind(pmv_categories_json)
    .bind(file_types_json)
    .bind(exclude_wav_sources)
    .bind(keys_json)
    .bind(rating_min)
    .bind(play_count_min)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await?;

    Ok(bundle)
}

/// Update a dynamic bundle's filter criteria. All parameters are `Option` to
/// allow partial updates. `None` means "don't change this field".
///
/// Also updates the associated tag name if `name` is provided.
pub async fn update_dynamic_bundle(
    pool: &Pool<Sqlite>,
    id: i64,
    name: Option<&str>,
    base_tags: Option<Option<Vec<String>>>,
    include_all_tracks: Option<bool>,
    bpm_min: Option<Option<f64>>,
    bpm_max: Option<Option<f64>>,
    pmv_categories: Option<Option<Vec<String>>>,
    file_types: Option<Option<Vec<String>>>,
    exclude_wav_sources: Option<bool>,
    keys: Option<Option<Vec<String>>>,
    rating_min: Option<Option<i64>>,
    play_count_min: Option<Option<i64>>,
) -> Result<DynamicBundle> {
    // Fetch existing bundle first
    let existing = get_dynamic_bundle(pool, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Dynamic bundle not found"))?;

    // If name changed, update the tag name too
    if let Some(new_name) = name {
        crate::db::tags::update_tag(pool, existing.tag_id, Some(new_name), None).await?;
    }

    let mut set_clauses: Vec<String> = Vec::new();
    let mut string_params: Vec<String> = Vec::new();

    if name.is_some() {
        set_clauses.push("name = ?".to_string());
        string_params.push(name.unwrap().to_string());
    }

    if let Some(val) = base_tags {
        let json = val
            .filter(|v| !v.is_empty())
            .map(|v| serde_json::to_string(&v).unwrap_or_default());
        set_clauses.push("base_tags = ?".to_string());
        string_params.push(json.unwrap_or_default());
    }

    if let Some(val) = include_all_tracks {
        set_clauses.push("include_all_tracks = ?".to_string());
        string_params.push((if val { 1 } else { 0 }).to_string());
    }

    if let Some(val) = bpm_min {
        match val {
            Some(v) => {
                set_clauses.push("bpm_min = ?".to_string());
                string_params.push(v.to_string());
            }
            None => {
                set_clauses.push("bpm_min = NULL".to_string());
            }
        }
    }

    if let Some(val) = bpm_max {
        match val {
            Some(v) => {
                set_clauses.push("bpm_max = ?".to_string());
                string_params.push(v.to_string());
            }
            None => {
                set_clauses.push("bpm_max = NULL".to_string());
            }
        }
    }

    if let Some(val) = pmv_categories {
        let json = val
            .filter(|v| !v.is_empty())
            .map(|v| serde_json::to_string(&v).unwrap_or_default());
        set_clauses.push("pmv_categories = ?".to_string());
        string_params.push(json.unwrap_or_default());
    }

    if let Some(val) = file_types {
        let json = val
            .filter(|v| !v.is_empty())
            .map(|v| serde_json::to_string(&v).unwrap_or_default());
        set_clauses.push("file_types = ?".to_string());
        string_params.push(json.unwrap_or_default());
    }

    if let Some(val) = exclude_wav_sources {
        set_clauses.push("exclude_wav_sources = ?".to_string());
        string_params.push((if val { 1 } else { 0 }).to_string());
    }

    if let Some(val) = keys {
        let json = val
            .filter(|v| !v.is_empty())
            .map(|v| serde_json::to_string(&v).unwrap_or_default());
        set_clauses.push("keys = ?".to_string());
        string_params.push(json.unwrap_or_default());
    }

    if let Some(val) = rating_min {
        match val {
            Some(v) => {
                set_clauses.push("rating_min = ?".to_string());
                string_params.push(v.to_string());
            }
            None => {
                set_clauses.push("rating_min = NULL".to_string());
            }
        }
    }

    if let Some(val) = play_count_min {
        match val {
            Some(v) => {
                set_clauses.push("play_count_min = ?".to_string());
                string_params.push(v.to_string());
            }
            None => {
                set_clauses.push("play_count_min = NULL".to_string());
            }
        }
    }

    if set_clauses.is_empty() {
        return Ok(existing);
    }

    let now = chrono::Utc::now().timestamp();
    set_clauses.push("updated_at = ?".to_string());
    string_params.push(now.to_string());

    let query_str = format!(
        "UPDATE dynamic_bundles SET {} WHERE id = ? RETURNING *",
        set_clauses.join(", ")
    );

    let mut query = sqlx::query_as::<_, DynamicBundle>(&query_str);
    for p in &string_params {
        query = query.bind(p.clone());
    }
    query = query.bind(id);

    let bundle = query.fetch_one(pool).await?;
    Ok(bundle)
}

/// Delete a dynamic bundle by ID.
/// The associated tag is automatically deleted via ON DELETE CASCADE.
pub async fn delete_dynamic_bundle(pool: &Pool<Sqlite>, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM dynamic_bundles WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Get the number of files matching a dynamic bundle's criteria (without resolving).
pub async fn get_dynamic_bundle_file_count(pool: &Pool<Sqlite>, db: &DynamicBundle) -> Result<i64> {
    let file_ids = resolve_dynamic_bundle(pool, db).await?;
    Ok(file_ids.len() as i64)
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::SqlitePool;

    async fn create_test_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();

        // Core tables
        sqlx::query("CREATE TABLE IF NOT EXISTS tag_categories (id INTEGER PRIMARY KEY, name TEXT NOT NULL, icon TEXT NOT NULL DEFAULT '', prefix TEXT NOT NULL DEFAULT '', sort_order INTEGER NOT NULL DEFAULT 0, is_default BOOLEAN NOT NULL DEFAULT 0)")
            .execute(&pool).await.unwrap();
        sqlx::query("INSERT OR IGNORE INTO tag_categories (id, name, icon, prefix, sort_order, is_default) VALUES (1, 'Setlist', 'fa-list-music', 'S', 0, 1), (2, 'Phase', 'fa-layers', 'P', 1, 1), (3, 'Mood', 'fa-heart', 'M', 2, 1), (4, 'Vibe', 'fa-sparkles', 'V', 3, 1)")
            .execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS tags (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, category_id INTEGER NOT NULL, sort_order INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL DEFAULT 0, backpack BOOLEAN NOT NULL DEFAULT 0)")
            .execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS files (id INTEGER PRIMARY KEY AUTOINCREMENT, file_path TEXT NOT NULL DEFAULT '', file_hash TEXT NOT NULL DEFAULT '', file_type TEXT NOT NULL DEFAULT '', file_size INTEGER NOT NULL DEFAULT 0, last_modified INTEGER NOT NULL DEFAULT 0, last_scanned INTEGER NOT NULL DEFAULT 0, rating INTEGER NOT NULL DEFAULT 0, play_count INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT NULL DEFAULT 0, bpm REAL, source_of INTEGER, isrc TEXT, title TEXT, artist TEXT, musical_key TEXT)")
            .execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS service_tracks (id INTEGER PRIMARY KEY AUTOINCREMENT, title TEXT NOT NULL DEFAULT '', artist TEXT NOT NULL DEFAULT '', service TEXT NOT NULL DEFAULT '', service_id TEXT NOT NULL DEFAULT '', isrc TEXT, imported_at INTEGER NOT NULL DEFAULT 0)")
            .execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS service_playlists (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL DEFAULT '', service TEXT NOT NULL DEFAULT '', playlist_id TEXT NOT NULL DEFAULT '', archive_deleted BOOLEAN NOT NULL DEFAULT 0)")
            .execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS service_playlist_tracks (playlist_id INTEGER NOT NULL, track_id INTEGER NOT NULL, position INTEGER NOT NULL DEFAULT 0, added_at INTEGER, deleted_at INTEGER)")
            .execute(&pool).await.unwrap();
        // Mock view as table for testing
        sqlx::query("CREATE TABLE IF NOT EXISTS v_file_track_link (file_id INTEGER NOT NULL, track_id INTEGER NOT NULL)")
            .execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS track_resolved_tags (track_id INTEGER NOT NULL, tag_id INTEGER NOT NULL, tag_name TEXT NOT NULL, category_id INTEGER NOT NULL, category_name TEXT NOT NULL, prefix TEXT NOT NULL, created_at INTEGER NOT NULL DEFAULT 0)")
            .execute(&pool).await.unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS dynamic_bundles (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL, tag_id INTEGER NOT NULL, base_tags TEXT, include_all_tracks BOOLEAN NOT NULL DEFAULT 0, bpm_min REAL, bpm_max REAL, pmv_categories TEXT, file_types TEXT, exclude_wav_sources BOOLEAN NOT NULL DEFAULT 1, keys TEXT, rating_min INTEGER, play_count_min INTEGER, created_at INTEGER DEFAULT 0, updated_at INTEGER DEFAULT 0)")
            .execute(&pool).await.unwrap();

        pool
    }

    /// Helper: insert a file with an optional BPM + link it to a track via v_file_track_link
    async fn insert_file(
        pool: &SqlitePool,
        id: i64,
        file_type: &str,
        bpm: Option<f64>,
        source_of: Option<i64>,
        track_id: i64,
    ) {
        let bpm_str = bpm.map(|v| v.to_string()).unwrap_or("NULL".to_string());
        let src_str = source_of
            .map(|v| v.to_string())
            .unwrap_or("NULL".to_string());
        sqlx::query(&format!(
            "INSERT INTO files (id, file_path, file_hash, file_type, file_size, last_modified, last_scanned, bpm, source_of, isrc, title, artist, musical_key, created_at, updated_at) VALUES ({id}, '/test/f{id}.{t}', 'h{id}', '{t}', 100, 0, 0, {bpm}, {src}, 'ISRC{id}', 'Title{id}', 'Artist{id}', '4m', 0, 0)",
            id = id, t = file_type, bpm = bpm_str, src = src_str
        )).execute(pool).await.unwrap();
        sqlx::query("INSERT OR IGNORE INTO service_tracks (id, title, artist, service, service_id, isrc) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(track_id).bind(format!("Title{}", track_id)).bind(format!("Artist{}", track_id))
            .bind("test").bind(format!("test:track:{}", track_id)).bind(format!("ISRC{}", track_id))
            .execute(pool).await.unwrap();
        sqlx::query("INSERT INTO v_file_track_link (file_id, track_id) VALUES (?, ?)")
            .bind(id)
            .bind(track_id)
            .execute(pool)
            .await
            .unwrap();
    }

    /// Helper: link a tag to a track via track_resolved_tags (simulating the playlist→tag→track chain)
    async fn link_tag_to_track(pool: &SqlitePool, track_id: i64, tag_name: &str, prefix: &str) {
        let cat_id = match prefix.to_uppercase().as_str() {
            "P" => 2,
            "M" => 3,
            "V" => 4,
            _ => 1,
        };
        sqlx::query("INSERT INTO tags (id, name, category_id, created_at) VALUES (?, ?, ?, 0) ON CONFLICT(id) DO NOTHING")
            .bind(track_id + 100).bind(tag_name).bind(cat_id)
            .execute(pool).await.unwrap();
        let cat_name = match cat_id {
            2 => "Phase",
            3 => "Mood",
            4 => "Vibe",
            _ => "Setlist",
        };
        sqlx::query("INSERT INTO track_resolved_tags (track_id, tag_id, tag_name, category_id, category_name, prefix, created_at) VALUES (?, ?, ?, ?, ?, ?, 0)")
            .bind(track_id).bind(track_id + 100).bind(tag_name).bind(cat_id).bind(cat_name).bind(prefix)
            .execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn test_resolve_dynamic_bundle_bpm_range() {
        let pool = create_test_db().await;

        // Files: id=1 @120BPM (track 1), id=2 @140BPM (track 2), id=3 @160BPM (track 3)
        insert_file(&pool, 1, "flac", Some(120.0), None, 1).await;
        insert_file(&pool, 2, "flac", Some(140.0), None, 2).await;
        insert_file(&pool, 3, "flac", Some(160.0), None, 3).await;

        let db = DynamicBundle {
            id: 1,
            name: "Test Bundle".to_string(),
            tag_id: 1,
            base_tags: None,
            include_all_tracks: true,
            bpm_min: Some(130.0),
            bpm_max: Some(150.0),
            pmv_categories: None,
            file_types: None,
            exclude_wav_sources: false,
            keys: None,
            rating_min: None,
            play_count_min: None,
            created_at: 0,
            updated_at: 0,
        };

        let file_ids = resolve_dynamic_bundle(&pool, &db).await.unwrap();
        assert_eq!(file_ids.len(), 1, "Only file 2 (140 BPM) should match");
        assert!(file_ids.contains(&2), "File 2 should be in results");
    }

    #[tokio::test]
    async fn test_resolve_dynamic_bundle_all_tracks() {
        let pool = create_test_db().await;
        insert_file(&pool, 1, "flac", None, None, 1).await;
        insert_file(&pool, 2, "flac", None, None, 2).await;

        let db = DynamicBundle {
            id: 1,
            name: "All".to_string(),
            tag_id: 1,
            base_tags: None,
            include_all_tracks: true,
            bpm_min: None,
            bpm_max: None,
            pmv_categories: None,
            file_types: None,
            exclude_wav_sources: false,
            keys: None,
            rating_min: None,
            play_count_min: None,
            created_at: 0,
            updated_at: 0,
        };

        let file_ids = resolve_dynamic_bundle(&pool, &db).await.unwrap();
        assert_eq!(
            file_ids.len(),
            2,
            "All tracks mode returns all linked files"
        );
    }

    #[tokio::test]
    async fn test_resolve_dynamic_bundle_bpm_and_pmv() {
        let pool = create_test_db().await;
        insert_file(&pool, 1, "flac", Some(120.0), None, 1).await;
        insert_file(&pool, 2, "stem.m4a", Some(140.0), None, 2).await;
        insert_file(&pool, 3, "flac", Some(160.0), None, 3).await;

        link_tag_to_track(&pool, 2, "dark", "M").await;

        let db = DynamicBundle {
            id: 1,
            name: "BPM+PMV".to_string(),
            tag_id: 1,
            base_tags: None,
            include_all_tracks: true,
            bpm_min: Some(130.0),
            bpm_max: Some(150.0),
            pmv_categories: Some(r#"["m"]"#.to_string()),
            file_types: None,
            exclude_wav_sources: false,
            keys: None,
            rating_min: None,
            play_count_min: None,
            created_at: 0,
            updated_at: 0,
        };

        let file_ids = resolve_dynamic_bundle(&pool, &db).await.unwrap();
        assert_eq!(
            file_ids.len(),
            1,
            "Only file 2 matches BPM 130-150 AND Mood tag"
        );
        assert!(file_ids.contains(&2));
    }

    #[tokio::test]
    async fn test_resolve_dynamic_bundle_keys() {
        let pool = create_test_db().await;
        insert_file(&pool, 1, "flac", None, None, 1).await;
        insert_file(&pool, 2, "flac", None, None, 2).await;

        // Set musical key on files
        sqlx::query("UPDATE files SET musical_key = '4m' WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE files SET musical_key = '8m' WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();

        let db = DynamicBundle {
            id: 1,
            name: "Key 4m".to_string(),
            tag_id: 1,
            base_tags: None,
            include_all_tracks: true,
            bpm_min: None,
            bpm_max: None,
            pmv_categories: None,
            file_types: None,
            exclude_wav_sources: false,
            keys: Some(r#"["4m"]"#.to_string()),
            rating_min: None,
            play_count_min: None,
            created_at: 0,
            updated_at: 0,
        };

        let file_ids = resolve_dynamic_bundle(&pool, &db).await.unwrap();
        assert_eq!(file_ids.len(), 1, "Only file 1 has key 4m");
        assert!(file_ids.contains(&1));
    }

    #[tokio::test]
    async fn test_resolve_dynamic_bundle_rating_min() {
        let pool = create_test_db().await;
        insert_file(&pool, 1, "flac", None, None, 1).await;
        insert_file(&pool, 2, "flac", None, None, 2).await;

        sqlx::query("UPDATE files SET rating = 3 WHERE id = 1")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE files SET rating = 1 WHERE id = 2")
            .execute(&pool)
            .await
            .unwrap();

        let db = DynamicBundle {
            id: 1,
            name: "Rating>=2".to_string(),
            tag_id: 1,
            base_tags: None,
            include_all_tracks: true,
            bpm_min: None,
            bpm_max: None,
            pmv_categories: None,
            file_types: None,
            exclude_wav_sources: false,
            keys: None,
            rating_min: Some(2),
            play_count_min: None,
            created_at: 0,
            updated_at: 0,
        };

        let file_ids = resolve_dynamic_bundle(&pool, &db).await.unwrap();
        assert_eq!(file_ids.len(), 1, "Only file 1 has rating >= 2");
        assert!(file_ids.contains(&1));
    }

    #[tokio::test]
    async fn test_get_dynamic_bundle_file_count() {
        let pool = create_test_db().await;
        insert_file(&pool, 1, "flac", Some(120.0), None, 1).await;
        insert_file(&pool, 2, "flac", Some(140.0), None, 2).await;
        insert_file(&pool, 3, "flac", Some(99.0), None, 3).await;

        let db = DynamicBundle {
            id: 1,
            name: "BPM>100".to_string(),
            tag_id: 1,
            base_tags: None,
            include_all_tracks: true,
            bpm_min: Some(100.0),
            bpm_max: None,
            pmv_categories: None,
            file_types: None,
            exclude_wav_sources: false,
            keys: None,
            rating_min: None,
            play_count_min: None,
            created_at: 0,
            updated_at: 0,
        };

        assert_eq!(get_dynamic_bundle_file_count(&pool, &db).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_resolve_dynamic_bundle_base_tags() {
        let pool = create_test_db().await;

        // Create playlists + tracks + links
        sqlx::query("INSERT INTO service_playlists (id, name, service, playlist_id) VALUES (1, 'house', 'test', 'p:1'), (2, 'techno', 'test', 'p:2')")
            .execute(&pool).await.unwrap();

        // Track 1 in playlist "house", Track 2 in playlist "techno", Track 3 in both
        insert_file(&pool, 1, "flac", None, None, 1).await;
        insert_file(&pool, 2, "flac", None, None, 2).await;
        insert_file(&pool, 3, "flac", None, None, 3).await;

        sqlx::query("INSERT INTO service_playlist_tracks (playlist_id, track_id) VALUES (1, 1), (2, 2), (1, 3), (2, 3)")
            .execute(&pool).await.unwrap();

        let db = DynamicBundle {
            id: 1,
            name: "Base Tags".to_string(),
            tag_id: 1,
            base_tags: Some(r#"["house","techno"]"#.to_string()),
            include_all_tracks: false,
            bpm_min: None,
            bpm_max: None,
            pmv_categories: None,
            file_types: None,
            exclude_wav_sources: false,
            keys: None,
            rating_min: None,
            play_count_min: None,
            created_at: 0,
            updated_at: 0,
        };

        let file_ids = resolve_dynamic_bundle(&pool, &db).await.unwrap();
        assert_eq!(
            file_ids.len(),
            3,
            "All three files have a track in at least one base playlist"
        );
    }

    #[tokio::test]
    async fn test_resolve_dynamic_bundle_no_links_returns_empty() {
        let pool = create_test_db().await;

        // Files exist but have no v_file_track_link entries
        sqlx::query("INSERT INTO files (id, file_path, file_hash, file_type, file_size, last_modified, last_scanned, created_at, updated_at) VALUES (1, '/test/a.flac', 'h1', 'flac', 100, 0, 0, 0, 0)")
            .execute(&pool).await.unwrap();

        let db = DynamicBundle {
            id: 1,
            name: "No Links".to_string(),
            tag_id: 1,
            base_tags: None,
            include_all_tracks: true,
            bpm_min: None,
            bpm_max: None,
            pmv_categories: None,
            file_types: None,
            exclude_wav_sources: false,
            keys: None,
            rating_min: None,
            play_count_min: None,
            created_at: 0,
            updated_at: 0,
        };

        let file_ids = resolve_dynamic_bundle(&pool, &db).await.unwrap();
        assert_eq!(
            file_ids.len(),
            0,
            "Files without v_file_track_link should not appear (track-based resolution)"
        );
    }
}
