# Test Guide

## Quick Start

```bash
# Run all tests (432 tests, ~30s)
cargo test

# Run only unit tests (pure functions)
cargo test --lib

# Run a specific integration test file
cargo test --test api_files
cargo test --test api_tracks
cargo test --test api_tags

# Run a single test
cargo test --test api_files -- files_list_default_limit

# Run tests for a specific module
cargo test --lib -- digging::tests
cargo test --lib -- config::tests

# Run coverage report
cargo llvm-cov --html --ignore-filename-regex 'tests/'
open ~/.cargo-target/llvm-cov/html/index.html
```

## Test Structure

### Unit Tests (`src/*.rs`)

Pure Rust functions tested with `#[cfg(test)] mod tests` within the source file.
Use `#[test]` for synchronous tests and `#[tokio::test]` for async tests.
No external dependencies, no database required.

**Covered modules:**
- `audio_extensions.rs` — 6 tests (AudioExtension enum)
- `backup/mod.rs` — 14 tests (path construction, output parsing)
- `comment.rs` — 37 tests (comment parsing/generation)
- `config.rs` — 18 tests (TOML parsing, env override, defaults)
- `db.rs` — 23 tests (BPM extraction, year parsing, tag queries)
- `deemix/cli.rs` — 9 tests (CLI arg parsing, DB URL resolution)
- `deemix/models.rs` — 9 tests (queue status deserialization)
- `digging.rs` — 34 tests (Camelot keys, scoring, ISRC dedup)
- `dump.rs` — 12 tests (serialization roundtrip)
- `embeddings.rs` — 6 tests (cosine similarity, normalization)
- `main.rs` — 10 tests (CLI subcommand parsing)
- `maintainer.rs` — 11 tests (scheduling, condition checks)
- `scan_cache.rs` — 13 tests (cache hit/miss, mode detection)
- `spotify/models.rs` — 4 tests (sync result, PlaylistInfo)
- `traktor.rs` — 8 tests (collection.nml parsing)

### Integration Tests (`tests/*.rs`)

Each test file covers a group of related API endpoints. Tests use Axum's
`TestClient` or `reqwest` against a self-hosted test app with in-memory SQLite.

**Test files and their coverage:**

| File | Tests | Covers |
|------|-------|--------|
| `api_files.rs` | 42 | `/api/files*` — list, filter, pagination, sort, CRUD |
| `api_tracks.rs` | 28 | `/api/tracks*` — list, filter, playlists, backpack, comments |
| `api_tags.rs` | 32 | `/api/tags*` — list, filter, parents, bulk ops, curation |
| `api_playlists.rs` | 28 | `/api/playlists*` — CRUD, subscriptions, archive, tags |
| `api_digging.rs` | 14 | `/api/digging/*` — suggest, search, ladder, tracks |
| `api_folders.rs` | 13 | `/api/folders*` — CRUD, scan, backup config, watch |
| `api_storage.rs` | 11 | `/api/storage/*` — status, settings, prune, backup |
| `api_file_variants.rs` | 8 | `/api/files/{id}/variants` — ISRC grouping |
| `api_services.rs` | 8 | `/api/services/*` — config, sync, auth, reset |
| `api_infrastructure.rs` | 6 | `/api/version`, `/api/embeddings/*`, `/api/traktor/*` |
| `api_spotify_sync.rs` | 5 | `/api/services/spotify/sync/*` — error paths |
| `api_deemix.rs` | 4 | `/api/services/deemix/queue/*` — CRUD error paths |
| `api_tag_categories.rs` | 3 | `/api/tag-categories/*` — CRUD |
| `api_tag_energy_levels.rs` | 3 | `/api/tag-energy-levels/*` — list, set, batch |
| `api_health.rs` | 2 | `/api/health`, `/api/version` |
| `api_dump.rs` | 2 | `/api/dump`, `/api/restore` |
| `api_tasks.rs` | 4 | `/api/tasks*` — list, status filter |
| `migration_integrity.rs` | 1 | All 16 migrations run cleanly from scratch |

## Adding a Test

### Integration test template

```rust
#[tokio::test]
async fn my_endpoint_test() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;

    let resp = client
        .get(format!("{}/api/tracks?limit=5", base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let json: serde_json::Value = resp.json().await.unwrap();
    let data = json["data"].as_array().unwrap();
    assert_eq!(data.len(), 5);
}
```

### Unit test template

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_some_function() {
        let result = some_function("input");
        assert_eq!(result, Expected::Value);
    }
}
```

### Seed Data

All integration tests use `tests/common/mod.rs` for seeding:

- `spawn_test_app()` — starts test app with in-memory SQLite, runs all migrations
- `seed_basic_data(pool)` — populates minimal data (4 files, 3 tracks, 2 playlists, 3 tags, 2 folders)
- Additional seed functions for specific scenarios (digging, storage, variants, etc.)

When adding a new test, reuse existing seed data first. Only create new seed
helpers if your test truly needs data the existing seeds don't provide.

## Coverage

**Current**: ~45.9% line coverage (`cargo llvm-cov --ignore-filename-regex 'tests/'`)
**Target**: ≥75% line coverage

Coverage is measured with `cargo-llvm-cov`. The biggest gaps are external service
modules (Spotify API, deemix, SSH backup) that require real credentials — these
are tested via error-path integration tests only.

**To improve coverage:**

1. Run `cargo llvm-cov --html --ignore-filename-regex 'tests/'`
2. Open `~/.cargo-target/llvm-cov/html/index.html`
3. Sort by uncovered lines, add tests for the worst offenders
4. Re-run and repeat

## Rules

1. **Every new endpoint and every new filter parameter MUST have a test**.
   This is not optional — test-less code will be rejected on review.

2. **Tests must be deterministic**. No random data, no time-dependent assertions,
   no external network calls. Every test uses an in-memory SQLite database.

3. **Assert specific values, not just presence**. Don't use `contains_key()` or
   field-presence checks alone — verify the actual value matches what the seed
   data produces.

4. **Test error paths**. Every CRUD endpoint should have a 400/404 test for
   invalid inputs and non-existent entities.

5. **Prefer integration tests for API behavior**. Unit tests are for pure
   functions only. If a function touches the database, test it through the API.

6. **Keep tests fast**. The full suite must complete in <30 seconds. If your
   test takes >100ms, check if it's doing too much.
