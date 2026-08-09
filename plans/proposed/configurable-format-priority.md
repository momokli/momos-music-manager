## Plan: configurable-format-priority

**Status**: proposed
**Branch**: `feat/configurable-format-priority`
**Depends on**: `feat/fix-backpack-local-tracking`
**Migration needed**: no

### Description

Replace hardcoded `format_preference()` ranking with a user-configurable priority
list stored in `service_config`, editable from the Storage page UI.

### API Contract (design-first)

```
GET  /api/storage/settings/format-priority
  → 200 { data: { priorities: ["stem.m4a", "flac", "mp3", "wav", "aiff"] } }

PUT  /api/storage/settings/format-priority
  ← { priorities: ["stem.m4a", "mp3", "flac"] }
  → 200 { data: { priorities: [...] } }
  → 400 { error: "priorities must be a non-empty array" }
  → 400 { error: "unknown format: xyz" }
```

### Agent Decomposition (TDD — tests written BEFORE implementation)

Two agents with **completely disjoint write scopes**:

---

#### Agent A: Backend TDD (`src/db/files.rs` + `src/api/storage.rs` + `tests/api_storage.rs`)

**Step 1 — Write failing tests** (commit these first, they WILL fail):

In `src/db/files.rs` `#[cfg(test)] mod tests`:

```rust
// Test: new format_preference respects configured order
#[test]
fn test_format_preference_with_config() {
    let prio = vec!["mp3".to_string(), "flac".to_string(), "stem.m4a".to_string()];
    assert!(format_preference_with("mp3", &prio) < format_preference_with("flac", &prio));
    assert!(format_preference_with("flac", &prio) < format_preference_with("stem.m4a", &prio));
    assert_eq!(format_preference_with("wav", &prio), u8::MAX); // not in list
}

// Test: default priorities match current hardcoded order
#[test]
fn test_default_priorities() {
    let defaults = default_format_priorities();
    assert_eq!(defaults[0], "stem.m4a");
    assert_eq!(defaults[1], "flac");
    assert_eq!(defaults[2], "mp3");
    assert_eq!(defaults[3], "wav");
}
```

In `tests/api_storage.rs`:

```rust
#[tokio::test]
async fn storage_format_priority_get_defaults() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    let resp = client.get(format!("{}/api/storage/settings/format-priority", base))
        .send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let json: serde_json::Value = resp.json().await.unwrap();
    let prio = json["data"]["priorities"].as_array().unwrap();
    assert!(prio.len() >= 4, "should have at least 4 default formats");
}

#[tokio::test]
async fn storage_format_priority_put_and_get() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    // PUT custom order
    let put = client.put(format!("{}/api/storage/settings/format-priority", base))
        .json(&serde_json::json!({"priorities": ["mp3", "stem.m4a", "flac"]}))
        .send().await.unwrap();
    assert_eq!(put.status(), 200);
    // GET should return the custom order
    let get = client.get(format!("{}/api/storage/settings/format-priority", base))
        .send().await.unwrap();
    let json: serde_json::Value = get.json().await.unwrap();
    let prio = json["data"]["priorities"].as_array().unwrap();
    assert_eq!(prio[0], "mp3");
    assert_eq!(prio[1], "stem.m4a");
}

#[tokio::test]
async fn storage_format_priority_put_invalid() {
    let (client, base, pool) = common::spawn_test_app().await;
    common::seed_basic_data(&pool).await;
    // Empty array → 400
    let resp = client.put(format!("{}/api/storage/settings/format-priority", base))
        .json(&serde_json::json!({"priorities": []}))
        .send().await.unwrap();
    assert_eq!(resp.status(), 400);
    // Unknown format → 400
    let resp = client.put(format!("{}/api/storage/settings/format-priority", base))
        .json(&serde_json::json!({"priorities": ["xyz"]}))
        .send().await.unwrap();
    assert_eq!(resp.status(), 400);
}
```

**Step 2 — Implement to make tests pass**:

1. `src/db/files.rs`:
   - Rename `format_preference` → `format_preference_with(file_type, priorities)`
   - Add `default_format_priorities() -> Vec<String>` returning the current hardcoded list
   - Add `load_format_priorities(pool) -> Vec<String>` reading from `service_config`
   - Keep old `format_preference(file_type)` as a wrapper calling `format_preference_with(file_type, &default_format_priorities())` for backward compat
   - Update `get_backpack_pull_candidates()` to call `load_format_priorities()` and pass to `format_preference_with()`

2. `src/api/storage.rs`:
   - Add `format_priority_get_handler` → loads from `service_config`, returns JSON
   - Add `format_priority_put_handler` → validates, stores JSON array in `service_config`
   - Validate: non-empty array, all values are known audio extensions
   - Add routes: `.route("/api/storage/settings/format-priority", get(...).put(...))`

**Step 3 — Run tests, iterate until green**:

```bash
cargo test --lib -- format_preference
cargo test --test api_storage -- storage_format_priority
```

**Files touched**: `src/db/files.rs`, `src/api/storage.rs`, `tests/api_storage.rs`

---

#### Agent B: Frontend (`frontend/pages/storage.js` + `frontend/style.css`)

**Step 1 — Design verification**: Before writing code, manually verify the API:

```bash
curl -s http://localhost:3000/api/storage/settings/format-priority | jq
curl -s -X PUT ... -d '{"priorities":["mp3","flac"]}' | jq
```

**Step 2 — Implement**:

Add a "Format Priority" card to the Storage page below the existing stats cards.

HTML structure:

```html
<div class="card" id="format-priority-card">
  <h3><i class="fas fa-sort-amount-down"></i> Format Priority</h3>
  <p class="help-text">When pulling from backup, higher formats are preferred.</p>
  <ul class="format-priority-list" id="format-priority-list">
    <!-- JS-populated: draggable items with ▲▼ buttons -->
  </ul>
  <div class="format-priority-actions">
    <input type="text" id="format-priority-add" placeholder="flac" class="input-text" />
    <button id="format-priority-add-btn" class="btn">Add</button>
    <button id="format-priority-reset" class="btn btn-ghost">Reset defaults</button>
    <button id="format-priority-save" class="btn btn-primary">Save</button>
  </div>
</div>
```

JS behavior:

1. `loadFormatPriority()` — GET the endpoint, render list items with ▲▼ buttons
2. Click ▲/▼ → swap with neighbor, update data array
3. "Add" button → append new format to list (validate against known formats)
4. "Reset defaults" → fetch hardcoded defaults (or just use known list)
5. "Save" → PUT the endpoint, show toast
6. Drag-to-reorder (optional, nice-to-have — use HTML5 drag API or skip for v1)

**Step 3 — Manual test**:

1. Open `http://localhost:3000/#storage`
2. Change order → Save → refresh page → verify order persisted
3. Trigger backpack sync → verify preferred format pulled

**Files touched**: `frontend/pages/storage.js`, `frontend/style.css`

---

### Execution Order

Agents A and B can run **simultaneously** — zero file conflicts.

After both complete:

1. `cargo build` — verify compilation
2. `cargo test` — all tests pass (647 existing + new ones)
3. `./test-backpack.sh` — all 15 integration tests still pass

### Acceptance Criteria

- [ ] `GET /api/storage/settings/format-priority` returns default priority list
- [ ] `PUT /api/storage/settings/format-priority` persists custom order
- [ ] Empty array rejected with 400
- [ ] Unknown format rejected with 400
- [ ] `get_backpack_pull_candidates()` uses configured priority
- [ ] Default hardcoded order preserved when no config exists
- [ ] Unit tests: `format_preference_with` ordering + defaults
- [ ] Integration tests: GET defaults, PUT+GET roundtrip, PUT invalid
- [ ] Frontend: Format Priority card renders with ▲▼ reorder buttons
- [ ] Frontend: Save persists to backend, survives page refresh
- [ ] Frontend: Add format input validates against known extensions
- [ ] Frontend: Reset restores default order
- [ ] `cargo build` passes
- [ ] All existing 647 tests pass
- [ ] `./test-backpack.sh` passes (15/15)

---

