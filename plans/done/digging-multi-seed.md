## Plan: digging-multi-seed

**Status**: done ✅
**Branch**: `feat/digging-multi-seed`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

Build the core multi-seed suggestion engine for the Digging/Curator workflow. Given a set of seed files (loaded by tag name or file IDs), find similar tracks from the local library using Camelot harmonic mixing + BPM proximity, scored and ranked. Deduplicate by ISRC. This is the backend engine — Phase 1 of 5.

### Design Decisions (from user)

1. **Embedded player**: browser-native `<audio>` — stem.m4a + FLAC both play natively in modern browsers, just need Range-request streaming
2. **ISRC dedup**: one suggestion per ISRC, prefer stem.m4a (plays in browser) — both versions stay in DB
3. **Outlier handling**: BPM range computed from seed cluster, tracks outside range excluded entirely

### Real Data (from production DB)

Tag "Collapse-capital" (id 434):

| File ID | ISRC         | Title             | Artist                    | BPM   | Key |
| ------- | ------------ | ----------------- | ------------------------- | ----- | --- |
| 4042    | US7NS2500009 | Games People Play | Paula van Klar            | 140.0 | 3m  |
| 4362    | US7NS2500009 | Games People Play | Paula van Klar            | 139.0 | 3m  |
| 4196    | QZ5FN2650988 | The Void          | Maite Dedecker            | 141.0 | 8m  |
| 4428    | QZ5FN2650988 | The Void          | Maite Dedecker            | 140.0 | 8m  |
| 5757    | DGA0H2483973 | This Summer       | Anna Reusch               | 140.0 | 6m  |
| 5769    | DGA0H2483973 | This Summer       | Anna Reusch               | 139.0 | 6m  |
| 3904    | ?            | Mean One          | Elon Bass Luciano Bradini | 160.0 | 1m  |
| 4538    | ?            | Mean One          | Elon Bass                 | 160.0 | 1m  |

BPM cluster of the 3 target tracks: 139–141. "Mean One" at 160 is an outlier, falls outside default ±8 range.
Eligible pool: 2184 files with BPM+Key, 1728 unique ISRCs.

### Backend Changes

#### 1. `src/digging.rs` — New types

```rust
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingSuggestRequest {
    /// Seed files: either provide file IDs directly...
    pub seed_file_ids: Option<Vec<i64>>,
    /// ...or a tag name whose files become the seeds
    pub seed_tag: Option<String>,
    /// BPM tolerance (± from seed BPM range boundaries)
    pub bpm_range: Option<f64>,  // default 8.0
    /// Active Camelot jumps
    pub camelot_jumps: Option<Vec<String>>,
    /// Max suggestions to return
    pub limit: Option<i64>,  // default 20, max 50
    /// Deduplicate suggestions by ISRC
    pub dedup_by_isrc: Option<bool>,  // default true
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingSeed {
    pub id: i64,
    pub title: String,
    pub artist: String,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub genre: Option<String>,
    pub isrc: Option<String>,
    pub file_path: String,
    pub file_type: String,  // "flac", "stem.m4a", etc.
    pub play_count: i32,
    pub last_played: Option<i64>,
    pub duration_ms: Option<i64>,
    /// Tags on this file (from v_file_resolved_tags)
    pub tags: Vec<DiggingTag>,
    /// Whether this seed was excluded as a BPM outlier
    pub excluded_as_outlier: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingTag {
    pub id: i64,
    pub name: String,
    pub category_name: String,
    pub prefix: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingSuggestion {
    pub file_id: i64,
    pub title: String,
    pub artist: String,
    pub bpm: Option<f64>,
    pub musical_key: Option<String>,
    pub genre: Option<String>,
    pub isrc: Option<String>,
    pub file_path: String,
    pub file_type: String,
    pub play_count: i32,
    pub last_played: Option<i64>,
    pub duration_ms: Option<i64>,
    /// Which of the seeds this suggestion best matches
    pub matching_seed_id: i64,
    /// Camelot compatibility: "perfect", "good", "ok"
    pub camelot_compatibility: String,
    /// BPM difference from best-matching seed
    pub bpm_diff: Option<f64>,
    /// Tags shared with the best-matching seed
    pub shared_tags: Vec<String>,
    /// Scoring details (for transparency)
    pub score_breakdown: ScoreBreakdown,
    /// Combined score (lower = better)
    pub score: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScoreBreakdown {
    pub play_count_score: f64,
    pub recency_score: f64,
    pub bpm_score: f64,
    pub camelot_bonus: f64,
    pub tag_match_bonus: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiggingSuggestResponse {
    /// The seed tracks used (including excluded outliers)
    pub seeds: Vec<DiggingSeed>,
    /// The BPM range used for candidate search
    pub bpm_min: f64,
    pub bpm_max: f64,
    /// Scored + ranked suggestions
    pub suggestions: Vec<DiggingSuggestion>,
    /// Total candidates considered before ranking
    pub candidates_considered: usize,
}
```

#### 2. `src/digging.rs` — `get_multi_seed_suggestions()`

```rust
pub async fn get_multi_seed_suggestions(
    pool: &Pool<Sqlite>,
    req: &DiggingSuggestRequest,
) -> Result<DiggingSuggestResponse>
```

**Algorithm:**

1. **Resolve seeds**: if `seed_tag` is set, query `v_file_tags` for all files with that tag. Otherwise use `seed_file_ids`. Load full File rows + resolved tags.

2. **Outlier detection**: compute median BPM of seeds, exclude any seed whose BPM deviates >20 from median. Mark excluded seeds with `excluded_as_outlier: true`. Compute BPM range from non-excluded seeds: `[min(bpm) - range, max(bpm) + range]`.

3. **Candidate query**: fetch all files (not in seed set) within BPM range, that have both BPM and key:

   ```sql
   SELECT * FROM files
   WHERE id NOT IN (?,?,...)
     AND bpm IS NOT NULL
     AND musical_key IS NOT NULL
     AND bpm >= ? AND bpm <= ?
   ORDER BY play_count ASC, COALESCE(last_played, 0) ASC
   LIMIT ?  -- fetch 5x limit for scoring pool
   ```

4. **Camelot filtering**: for each candidate, parse its `musical_key` as Camelot. Check compatibility against each non-excluded seed using `are_keys_compatible()`. If compatible with at least one seed, keep. Track which seed was the best match.

5. **Scoring** (per candidate, best seed match):
   - `play_count_score = min(play_count, 100) * 2.0` — fresher tracks preferred
   - `recency_score = (1000 - min(days_since_played, 1000)) * 0.5` — unplayed = 0, recent = high
   - If never played: `recency_bonus = -50.0`
   - `bpm_score = |candidate_bpm - seed_bpm| * 1.5`
   - `camelot_bonus`: perfect = -30, good = -15, ok = 0
   - `tag_match_bonus`: count shared resolved tags with the matching seed, -5 per shared tag
   - `total_score = play_count_score + recency_score + bpm_score + camelot_bonus + tag_match_bonus`

6. **ISRC dedup**: if `dedup_by_isrc` is true, group candidates by ISRC. For each ISRC group, keep the one with the lowest score. If ISRC is NULL, treat each as unique. Prefer `stem.m4a` over `flac` when scores tie.

7. **Sort + limit**: sort by score ascending, truncate to `limit`.

8. **Load tags**: for each suggestion, load resolved tags via `v_file_resolved_tags` for the `shared_tags` field (intersection with matching seed's tags).

#### 3. `src/api.rs` — Handler + Route

**Route**: `.route("/api/digging/suggest", post(digging_suggest_handler))`

**Handler**:

```rust
async fn digging_suggest_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<DiggingSuggestRequest>,
) -> impl IntoResponse {
    match crate::digging::get_multi_seed_suggestions(&state.db, &request).await {
        Ok(response) => Json(ApiResponse { data: response }).into_response(),
        Err(e) => internal_error(e).into_response(),
    }
}
```

Validation:

- Either `seed_file_ids` or `seed_tag` must be provided (400 if neither)
- At least 1 seed file must be found (404 if tag resolves to no files)
- `limit` clamped to 1..50, default 20
- `bpm_range` clamped to 1..30, default 8.0
- `camelot_jumps` defaults to all jumps if not provided

#### 4. `src/api.rs` — Audio Streaming Endpoint

**Route**: `.route("/api/files/{id}/stream", get(file_stream_handler))`

**Handler**:

```rust
async fn file_stream_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<i64>,
    request: axum::http::Request<Body>,
) -> impl IntoResponse
```

- Look up file by ID, get `file_path`
- Open file, get size
- Support `Range` header for seeking (HTTP 206 Partial Content)
- Content-Type based on extension: `.flac` → `audio/flac`, `.m4a` → `audio/mp4`, `.mp3` → `audio/mpeg`, `.wav` → `audio/wav`, `.aif`/`.aiff` → `audio/aiff`
- Accept-Ranges: bytes
- Without Range header: stream entire file (HTTP 200)
- Security: only serve files that are in the `files` table (no arbitrary path traversal)

#### 5. `src/digging.rs` — Audio format preference for dedup

```rust
/// When deduplicating by ISRC, prefer formats that play in browsers.
/// stem.m4a > mp3 > flac > wav > aiff > other
fn audio_format_preference(file_type: &str) -> u8 {
    match file_type.to_lowercase().as_str() {
        "stem.m4a" | "m4a" => 0,
        "mp3" | "mpeg" => 1,
        "flac" => 2,
        "wav" | "wave" => 3,
        "aif" | "aiff" => 4,
        _ => 5,
    }
}
```

### Existing code to reference

- `src/digging.rs`: `CamelotKey`, `parse_camelot_key()`, `are_keys_compatible()`, `ScoredTrack`, `get_suggestions()` (single-seed — can borrow scoring logic)
- `src/api.rs`: `get_tags_for_file()` in db.rs returns resolved tags via `v_file_resolved_tags`
- `migrations/001_initial_schema.sql`: `v_file_resolved_tags` view (in migration 002)
- `frontend/shared/utils.js`: `fetchJSON()` for API calls

### Files to modify

- `src/digging.rs` — new types + `get_multi_seed_suggestions()` + ISRC dedup helper
- `src/api.rs` — `digging_suggest_handler` + `file_stream_handler` + routes

### Acceptance Criteria

- [x] `POST /api/digging/suggest` with tag name resolves seed files from `v_file_tags`
- [x] `POST /api/digging/suggest` with seed file IDs works directly
- [x] BPM outlier detection excludes "Mean One" (160 BPM) when seeds are the 3 collapse-capital tracks at 139-141
- [x] BPM range computed as [min(bpm)-range, max(bpm)+range] from non-outlier seeds only
- [x] Candidates filtered to BPM range, must have BPM + key
- [x] Camelot compatibility checked against all non-excluded seeds (OR logic)
- [x] Scoring: play_count, recency, bpm_diff, camelot_bonus, tag_match_bonus all contribute correctly
- [x] ISRC dedup: same ISRC appears only once, stem.m4a preferred over flac
- [x] NULL ISRC files treated as unique (not deduplicated)
- [x] Response includes `seeds` array with outlier flags, `bpm_min`/`bpm_max`, `suggestions` with score_breakdown
- [x] `GET /api/files/{id}/stream` returns audio with correct Content-Type
- [x] `GET /api/files/{id}/stream` supports Range header (HTTP 206) for seeking
- [x] `GET /api/files/{id}/stream` returns 404 for non-existent file or file not in DB
- [x] 400 if neither seed_file_ids nor seed_tag provided
- [x] 404 if seed_tag resolves to no files
- [x] Backend compiles (`cargo build`)
- [x] Test with curl against real data: `curl -X POST localhost:3000/api/digging/suggest -H 'Content-Type: application/json' -d '{"seedTag":"Collapse-capital","limit":10}'`

---

