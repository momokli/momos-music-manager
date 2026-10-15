# Plan: Issue #363 — `feat(hub): hub_devices registry (Hub side)`

**Repo**: `momokli/momos-music-manager` · **Milestone**: `kellner-0.1.0` (#37)
**Base**: main @ 92c580a · **Branch**: `feat/363-hub-devices-registry`
**Gate**: #361 **CLOSED** (ADR-081, merged #450). §10.2 resolved → Hub-side per-device
token registry. **No blocker.**
**Spec sources**: `plans/proposed/kellner-0.1.0.md` §3 (Hub-side surface) + §10.2/§7 (Ordering)
(`docs/DECISIONS.md` ADR-081), `plans/kellner/issues.md` (#363).
**Constraints**: write-set **only `music-hub/`** (never `kellner/`). Additive migration.
No real secrets in repo/tests. v1 scope — **#369 (per-device settings) is NOT implemented here.**

## Pairing model (resolved reading of ADR-081 §10.2)

The ADR says "link is the capability", "der Endpoint verlangt eine **eingeloggte Session**",
"Token wird **nach dem Binden entwertet** (+ Ablauf unbenutzt)", and "kein Copy-Paste".
Reconciled into a **two-token** model with a single durable credential column:

- **Device token** (durable credential, 32-byte hex): generated **on the device** (kellner
  generates it; hub only ever stores its SHA-256). It is the future `Bearer` credential for
  device-gated routes (#364/#365). Never placed in a browser URL.
- **Pairing token** (one-time capability): also generated on the device, sent inside the
  approval URL (`/kellner/anheuern?token=…`). The hub stores only its hash while the row is
  **pending** (unbound). On approval the pairing hash is **rotated to the device-token hash**,
  so a later-leaked link can no longer re-bind (ADR's re-hijack guarantee). Unused pending
  rows **expire** (TTL).
- Register is unauthenticated (bootstrap); list/revoke are **web-session gated** like other
  `/api/hub/*` routes. The approval page requires a logged-in session and is where the
  `user_id` binding is written.

## Betroffene Dateien (Write-Set)

| File | Change |
| --- | --- |
| `music-hub/migrations/051_hub_devices.sql` | **new** — `hub_devices` table + indexes (additive, next free number after `050_*`) |
| `music-hub/src/devices.rs` | **new** — model structs, token-hash helpers, queries, HTTP router (`POST/GET/DELETE /api/hub/devices`, approval GET/POST) |
| `music-hub/src/lib.rs` | add `pub mod devices;` |
| `music-hub/src/api.rs` | `.merge(crate::devices::router(state))` in `router()` |
| `music-hub/templates/kellner_anheuern.html` | **new** — approval page ("diesen kellner anheuern?", Ja/Nein) |
| `music-hub/tests/devices.rs` | **new** — integration tests (register → approve → list → revoke, expiry, single-use, isolation) |
| `music-hub/tests/common/mod.rs` | only if a shared helper is strictly needed (prefer none) |

No change to `kellner/`, no `docs/DECISIONS.md` change (ADR-081 already covers it).

## Schema (`051_hub_devices.sql`, additive)

```sql
CREATE TABLE hub_devices (
    id                INTEGER PRIMARY KEY,
    user_id           INTEGER REFERENCES hub_users(id) ON DELETE CASCADE, -- NULL while pending
    name              TEXT NOT NULL,                     -- device label (hostname)
    token_hash        TEXT NOT NULL,                     -- sha256 hex: pairing hash while pending, then device-token hash
    pending_token_hash TEXT,                             -- sha256 hex of device token held until binding (NULL once bound)
    created_at        TEXT NOT NULL,
    last_seen_at      TEXT,                              -- updated by device-gated routes (#364+); column only in #363
    paired_at         TEXT,                              -- set on binding
    expires_at        TEXT,                              -- pre-binding TTL; NULL after binding
    revoked_at        TEXT
);
CREATE UNIQUE INDEX idx_hub_devices_token_hash ON hub_devices(token_hash);
CREATE INDEX idx_hub_devices_user ON hub_devices(user_id);
```

- `pending_token_hash` is the (nullable) staging slot carrying the device-token hash between
  register and binding; on approval `token_hash = pending_token_hash; pending_token_hash = NULL`.
- While pending, `user_id`/`paired_at` are NULL and `expires_at` bounds the window.

## User Stories (in Reihenfolge)

1. **Migration + model/queries** (prereq for 2–5)
   - Acceptance: `051_hub_devices.sql` applies cleanly on an existing DB and on a fresh
     `sqlx` migrate run; `hub_devices` matches the column list above; `ON DELETE CASCADE` to
     `hub_users`; unique `token_hash`; commits are additive (no table altered/dropped).
     `devices.rs` exposes `Device` model + helpers `hash_token(&str) -> String` (SHA-256 hex,
     constant-time compare), `create_pending`, `find_pending_by_pairing_hash`,
     `bind_device`, `list_for_user`, `revoke`, `sweep_expired`. Unit tests for hash/pure logic.
   - Files: migration, `src/devices.rs`, `src/lib.rs`. ~180 LoC (+ migration ~25).

2. **`POST /api/hub/devices` — register / re-pair (bootstrap, no session)**
   - Acceptance: body `{ name, pairing_token_hash, device_token_hash }` (hex) creates a
     **pending** row (`user_id=NULL`, `token_hash=pairing_token_hash`,
     `pending_token_hash=device_token_hash`, `created_at=now`,
     `expires_at=now+15min`); returns `{ device_id, approval_path }` where
     `approval_path = "/kellner/anheuern?token=<pairing_token>"` (raw token echoed **only if
     the client sent the raw pairing token**; when hashes are sent, hub returns just
     `device_id` + path template). No secret is ever returned. Malformed/short tokens → 400.
     Reuses a live pending row per `name` (re-pair) instead of unbounded pending rows (**max 1
     pending row per name per 15 min**, avoids pending-row spam). Expired rows are swept.
   - Files: `src/devices.rs`. ~120 LoC.

3. **`GET /api/hub/devices` — list own devices (web session)**
   - Acceptance: unauthenticated → redirect to `/login` (matches existing `/api/hub/*`);
     authenticated → JSON array of the **caller's** rows only (`id, name, created_at,
     last_seen_at, paired_at, revoked_at`), never `token_hash`/`pending_token_hash`.
     Devices of other users are absent. Empty list → `[]`.
   - Files: `src/devices.rs`. ~60 LoC.

4. **`DELETE /api/hub/devices/{id}` — revoke (web session)**
   - Acceptance: sets `revoked_at=now` on a row **owned by the caller**; idempotent (second
     call still 200/204, timestamp unchanged); `404` for unknown id or another user's device
     (no existence leak); revoked device can no longer be used by future device-auth
     middleware (hash lookup ignores/queries `revoked_at IS NULL`).
   - Files: `src/devices.rs`. ~50 LoC.

5. **Pairing approval page — `GET`/`POST /kellner/anheuern` (web session)**
   - Acceptance:
     - `GET /kellner/anheuern?token=…` without session → redirect `/login` (preserve `token`
       via `next`). With session: look up pending row by `sha256(token)` where `user_id IS
       NULL`, `revoked_at IS NULL`, `expires_at > now`; render `kellner_anheuern.html` asking
       **"diesen kellner anheuern?"** with the device `name`; Ja (POST) / Nein (link home).
     - Invalid / expired / already-bound token → friendly 404/410 page, **no binding**.
     - `POST` (Ja) → bind: `user_id = session user`, `paired_at = now`,
       `token_hash = pending_token_hash`, `pending_token_hash = NULL`, `expires_at = NULL`.
       The pairing token is thereby **single-use / invalidated**; re-opening the link fails the
       pending lookup (regression test). Success page confirms the binding.
     - A device already bound to user A cannot be moved to user B by replaying the link.
   - Files: `src/devices.rs` (handlers + `#[template]`), `templates/kellner_anheuern.html`
     (~40 lines HTML), `src/api.rs` merge. ~140 LoC.

6. **Integration tests — `music-hub/tests/devices.rs`**
   - Acceptance: against `common::spawn()`; covers (a) register → pending row, no secrets
     leaked; (b) approval binds and returns 200, row shows `paired_at`; (c) **pairing token
     single-use** — second GET/POST fails; (d) **expiry** — a row with past `expires_at` is
     not bindable and is swept; (e) list is session-gated + user-isolated; (f) revoke sets
     `revoked_at`, is idempotent, 404s cross-user; (g) `token_hash` never appears in any
     response body. `cargo test -p music-hub` green.
   - Files: `tests/devices.rs`. ~220 LoC.

## Implementierungs-Reihenfolge

1 → 2 → 3 → 4 → 5 → 6. Story 1 blocks all; 2 blocks 5 (a device must exist to approve);
5 blocks the token-single-use/expiry cases in 6. 3 and 4 are independent of 5.

## Abgrenzung / Nicht-Ziel

- **#369** (per-device settings, `GET/PUT /api/hub/devices/{id}/settings`) — separate issue.
- Device-token **auth middleware** + `last_seen_at` touch live in #364/#365; #363 only adds
  the column and query support.
- No provider credentials, no `kellner/` code, no real token material committed to the repo.

## Risiken

- **Pending-row spam** via the unauthenticated register route → mitigated by TTL + "one live
  pending row per name" + sweep (documented; stronger rate limits can follow).
- **Ordering**: #369 follows #363 per ADR-081 §7 — keep this PR free of settings work.
- **Token semantics**: the two-token split (pairing vs device) must be explicit in code
  comments so future device-auth routes read `token_hash` correctly (device token, post-bind).