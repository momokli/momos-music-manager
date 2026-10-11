# Plan: music-api-queue-engine

**Status**: implemented (awaiting PR)
**Branch**: `feat/music-api-queue-engine` (in-repo worktree `.queue-wt`)
**Ready for review**: yes
**Depends on**: `main`
**Migration needed**: no (music-api owns its schema in `db::init` via additive `ALTER TABLE`; not the MMM `migrations/` dir)

### Description

Turn the implicit ISRC/URL queue in `music-api` (the `tracks.state` / `url_tracks.state`
column plus a flat worker tick) into a **durable, self-healing queue**: per-row
`attempts` + exponential backoff, bounded retries so `failed` is no longer terminal,
a **lease/visibility timeout** that re-queues rows stranded in `downloading` after a
crash/restart, real `pause`/`resume` (global worker + per-order), a manual retry
endpoint, and a transparent control surface (`GET /queue`, `GET /worker`, `GET /logs`)
the hub can drive. SQLite stays the sole data backend; the hot path stays indexed and
O(limit) per tick.

### Why

- `failed` was terminal (`README` "known limits", `worker.rs`): one transient
  deemix/Deezer hiccup lost a track permanently — the only retry was a manual re-order.
- A process restart while a row is `downloading` stranded it: nothing in the old
  `pending_tracks` re-selected it.
- `POST /orders/{id}/status` accepted `paused`/`cancelled` but only wrote
  `orders.status`; the old `pending_tracks` ignored order status, so **pause did nothing**.
- Queue performance is load-bearing: the `pending_tracks` correlated subquery took
  ~98s until `idx_order_items_isrc` was added (`7f659f2`).

### Design (as implemented)

New columns on `tracks` and `url_tracks` (additive):

| Column             | Meaning                                                      |
| ------------------ | ------------------------------------------------------------ |
| `attempts`         | transient failures so far                                    |
| `next_attempt_at`  | `pending` becomes due at this epoch; `NULL` = due now        |
| `lease_expires_at` | set while `downloading`; refreshed as a heartbeat           |
| `paused`           | order-driven hold (`1` while an owning order is paused/cancelled) |

Indexes: `idx_tracks_due(state, paused, next_attempt_at)`,
`idx_tracks_lease(state, lease_expires_at)`, and the `url_tracks` mirrors.

- **Selection**: `db::due_tracks` / `db::due_url_tracks` — `state='pending' AND
  paused=0 AND (next_attempt_at IS NULL OR next_attempt_at <= now)`, ordered
  `priority DESC, created_at ASC`.
- **Retry/backoff**: `db::schedule_retry` (and `schedule_url_retry`) increment
  `attempts`; at `max_attempts` they go terminal (`failed`), otherwise re-queue with
  `backoff_delay(n) = min(base << (n-1), max)`. `backoff_delay` is pure + unit-tested.
  Wired into every transient failure point (deemix add/reject, deezer lookup, download
  status, file-not-found, lease expiry). `absent`/`CantStream` stay terminal.
- **Lease/reaper**: `mark_downloading` sets `lease_expires_at`; `advance_downloads`
  calls `renew_lease` while deemix still reports progress; a new `recover_expired_leases`
  phase at the top of each tick re-queues rows whose lease lapsed (covers restarts).
- **Pause**: global persisted gate (`queue_control` table) honoured at the top of
  `tick`; per-order `set_order_status`/`set_url_order_status` now set the `paused` hold.
- **Control API**: `GET /worker`, `POST /worker/pause`, `POST /worker/resume`,
  `POST /isrc/{isrc}/retry`, `POST /url-tracks/{id}/retry`, `GET /logs`, and an
  extended `GET /queue` (`counts` + `paused` + `attempts`/`nextAttemptAt`). The
  hub's control console consumes these.
- **Config**: `QUEUE_MAX_ATTEMPTS` (3), `QUEUE_RETRY_BASE_SECS` (60),
  `QUEUE_RETRY_MAX_SECS` (1800).

### Resolved decisions

- **Branch base**: `main` (project rule). The other worktree `.hub-wt` is actively
  switching branches and is not a stable base; its console route shapes are a
  superset-compatible subset here and reconcile at merge.
- **Pause semantics**: per-row `paused` flag (simple; last-writer-wins for tracks
  shared across orders — documented).
- **Lease length**: reused `DOWNLOAD_TIMEOUT_SECS`, but refreshed as a heartbeat so a
  deep deemix backlog is not killed while it is still progressing.

### Files modified

| File                               | Change                                                                 |
| ---------------------------------- | ---------------------------------------------------------------------- |
| `music-api/src/db.rs`              | columns + indexes + `queue_control`; `due_tracks`/`due_url_tracks`; `RetryPolicy` + `backoff_delay`; `schedule_retry`/`schedule_url_retry`; `renew_lease`/`renew_url_lease`; `recover_expired_leases`; `retry_isrc`/`retry_url_track`; `queue_counts`; pause helpers; pause wiring in order status |
| `music-api/src/models.rs`          | `attempts`/`next_attempt_at`/`lease_expires_at`/`paused` on `Track` + `UrlTrack` |
| `music-api/src/worker.rs`          | pause gate + reaper in `tick`; retries at failure points; lease heartbeat |
| `music-api/src/api.rs`             | `/worker` + pause/resume; retry endpoints; `/logs`; extended `/queue`; notify/logs |
| `music-api/src/config.rs`          | queue tuning knobs                                                     |
| `music-api/src/lib.rs`             | `logbuf` module + `AppState.logs`                                      |
| `music-api/src/logbuf.rs`          | new: bounded event ring buffer                                         |
| `music-api/src/main.rs`            | construct `logs`                                                       |
| `music-api/tests/api.rs`           | harness fields + 5 new integration tests                              |
| `music-api/tests/url_orders.rs`    | harness fields                                                        |
| `music-api/README.md`              | pipeline/states/endpoints/limits                                       |
| `docs/DECISIONS.md`                | new ADR                                                                |

### Acceptance Criteria

- [x] `cargo build --locked` passes
- [x] `cargo test --locked` passes — **57 tests** (25 unit + 21 `api` + 11 `url_orders`)
- [x] Transient failures retry up to `QUEUE_MAX_ATTEMPTS` with exponential backoff;
      `failed` is only reached at the cap
- [x] A row left `downloading` past its lease is re-queued (`expired_lease_is_requeued`)
- [x] `POST /orders/{id}/status {paused}` stops its tracks; `{open}` resumes
      (`pausing_an_order_holds_its_tracks`)
- [x] `POST /worker/pause` / `/resume` / `GET /worker` work and gate the tick
      (`worker_pause_resume_control`)
- [x] `POST /isrc/{isrc}/retry` and `/url-tracks/{id}/retry` reset terminal rows
      (`retry_resets_a_failed_track`)
- [x] `GET /queue` reports per-state counts + `attempts`/`nextAttemptAt`
      (`queue_reports_counts`)
- [ ] Hub console wiring to the control endpoints (separate `mmm-hub` PR)

### Out of scope

- Multi-worker / atomic job claiming, an external queue, broker semantics
- On-disk garbage collection and HTTP range/partial delivery
- The hub-side console UI (`mmm-hub`) — this PR exposes the endpoints only
