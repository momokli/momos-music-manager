## Plan: auto-deemix-subscriptions

**Status**: done ✅
**Branch**: `feat/auto-deemix-subscriptions`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

When the subscription poller discovers new tracks (or polls for the first time),
automatically trigger a deemix download via `ensure_queued()` — which checks the
live deemix queue and uses `retry_download` (UUID-based re-scan) if already
queued, or `add_to_queue` if new. Also inserts into `deemix_downloads` for
immediate UI status.

### Files modified

- `src/deemix/client.rs` — added `from_db()` constructor and `ensure_queued()` method
- `src/poller.rs` — auto-download trigger on first poll (`last_polled_at IS NULL`) and new tracks
- `src/api.rs` — delegated `load_deemix_client_from_db()` to `DeemixClient::from_db()`
- `frontend/pages/playlists.js` — updated subscribe button tooltip

### Acceptance Criteria

- [x] First poll ever triggers full deemix download (like manual 🔄 restart)
- [x] New tracks found triggers re-scan via `retry_download`
- [x] Already-queued playlists use UUID-based retry, not duplicate add
- [x] `deemix_downloads` table updated after auto-download for immediate UI
- [x] Graceful skip when deemix not configured (debug log)
- [x] Push-button manual re-download still works unchanged
- [x] Backend compiles (`cargo build`)

---

