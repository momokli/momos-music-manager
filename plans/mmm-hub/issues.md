# MMM Hub — Epic, Milestone Templates & Leaf-Issue Backlog

> Companion to [`00-interface.md`](00-interface.md) (frozen contract). Every name
> below — tables, views, endpoints, env keys, migrations — is copied verbatim from
> that contract. If anything here contradicts it, the contract wins.
>
> **Status**: draft for review · **Owner**: agent D · **Last updated**: 2026-10-06
>
> **Rules for every leaf issue** (from `AGENT.md` + `.github/workflows/pr-quality.yml`):
> one issue = one branch = one PR; the PR title is a Conventional Commit; the PR
> body carries `Closes #<n>`; **never** commit to `main`. The GitHub issue number
> is assigned at creation — the `M<ms>-<n>` IDs here are planning-local only.

---

## 1. Epic issue

**Title**: `MMM Hub` · **Label**: `epic` · **Dispatched**: never (context only)

### Body

MMM Hub is a **separate, multi-user ingest + exploration service** that lives in
this repo as its own crate (`mmm-hub/`), decoupled from the single-user client
core. Users log in with a generic account (OIDC / Pocket ID), link streaming
services (Spotify in v1), and the backend pulls **liked songs** and
**owned/collaborative playlists** into **one shared SQLite database** where every
row carries a `user_id`. v1 has no polished UI — the goal is a queryable data
basis (SQL + a few read endpoints).

Frozen v1 scope: **metadata only** (no audio hosting/downloading), **Spotify
only**, **no UI**. Full contract: [`00-interface.md`](00-interface.md).

**Ingest scope (resolved D6/D7, post-Feb-2026 API).** One **shared Spotify app**
(the owner's app is grandfathered, so the 5-user dev-mode cap does not bind this
group). `GET /playlists/{id}/items` returns `items` **only for owned or
collaborative** playlists; merely **followed** playlists are stored as
**metadata only** and never yield track rows. **Liked songs** (`GET /me/tracks`)
are unaffected. There is **no batch fetch** (`GET /tracks?ids=`, `/albums`, … were
removed) — enrichment rides on the track objects embedded in the playlist-items
response, paged at `limit ≤ 50`. Identity is `(service, service_track_id)`;
`isrc` is nullable and never a dedup key. Refresh tokens expire **6 months** after
the original authorization → `/api/hub/me` surfaces `needsReconnect` with a
re-auth path. HTTPS is a **prerequisite of M2** (OIDC issuer + Spotify redirect
are not loopback).

This epic is **context only and is never dispatched**. Work happens exclusively
through the atomic leaf issues listed in the four milestones below; each leaf
issue is one PR that closes itself with `Closes #<n>`.

**Milestones**

| Milestone    | Outcome                                                                                                                                                                                             | Migration |
| ------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------- |
| `hub-v0.1.0` | Crate builds standalone; schema + 4 overlap views proven by integration tests over seed fixtures. No network, no auth.                                                                              | 001       |
| `hub-v0.2.0` | HTTPS/TLS in place, then OIDC login (Pocket ID), session cookie, `/api/hub/me`, unauthenticated access blocked.                                                                                     | 002       |
| `hub-v0.3.0` | Linked user's liked songs + owned/collaborative playlists in the shared DB (followed = metadata only); snapshot-aware re-sync; cross-user dedup by `(service, service_track_id)`; `needsReconnect`. | 003       |
| `hub-v0.4.0` | Read endpoints + guarded read-only SQL console; HTTPS already established in M2.                                                                                                                    | —         |

**Leaf issues**: `M1-1` … `M1-6`, `M2-1` … `M2-7`, `M3-1` … `M3-9`, `M4-1` … `M4-4`
(26 total). The per-iteration work order is the leaf checklist in each milestone
description, not this table.

**Out of scope (later 2.x line)**: SoundCloud/YouTube, cross-user tag subscribe,
any UI.

---

## 2. Milestone description templates

Each milestone is one iteration = one release. The description below is the
milestone body on GitHub: outcome statement, the leaf-issue checklist, and the
unlock line. Only the iteration currently being worked carries `Freigabe: ja`;
per the coordinator's instruction that is **only M1** for now.

### M1 — `hub-v0.1.0` — Data model & skeleton

**Outcome**: The `mmm-hub` crate builds standalone (root `cargo build` unaffected);
migration `001_hub_schema.sql` creates all `hub_*` tables and the four overlap
views; seed fixtures + integration tests prove `hub_v_shared_tracks` and
`hub_v_user_overlap` with exact row counts. No network, no auth.

**Leaf checklist**

- [ ] `M1-1` — `feat(hub): scaffold mmm-hub crate and CI job`
- [ ] `M1-2` — `feat(hub): migration 001 hub schema tables`
- [ ] `M1-3` — `feat(hub): migration 001 hub overlap views and indexes`
- [ ] `M1-4` — `test(hub): hub seed fixtures`
- [ ] `M1-5` — `test(hub): integration tests for hub overlap views`
- [ ] `M1-6` — `docs(hub): README stub and ADR for separate hub crate`

**Freigabe: ja**

### M2 — `hub-v0.2.0` — Accounts & sessions (OIDC / Pocket ID)

**Outcome**: The hub is reachable over **HTTPS** behind a TLS reverse proxy first
(prerequisite of M2: Pocket ID's OIDC `IssuerUrl` is https-only and the Spotify
redirect URI cannot be a bare LAN IP). Then a user logs in via Pocket ID (OIDC,
PKCE); the server issues an HTTP-only session cookie backed by `hub_sessions`;
`GET /api/hub/me` returns the user + linked services (including
`needsReconnect`); every route except health and the auth flow rejects
unauthenticated access.

**Leaf checklist**

- [ ] `M2-1` — `feat(hub): TLS reverse proxy and HTTPS base URL`
- [ ] `M2-2` — `feat(hub): OIDC configuration from environment`
- [ ] `M2-3` — `feat(hub): migration 002 hub sessions table`
- [ ] `M2-4` — `feat(hub): OIDC login flow with PKCE`
- [ ] `M2-5` — `feat(hub): session middleware`
- [ ] `M2-6` — `feat(hub): health and me endpoints`
- [ ] `M2-7` — `test(hub): auth integration tests`

### M3 — `hub-v0.3.0` — Spotify link & ingest

**Outcome**: A linked user's **liked songs** and **owned/collaborative
playlists** land in the shared DB; **followed** playlists are stored as
**metadata only** (the post-Feb-2026 API returns `items` only for
owned/collaborative playlists, so no track rows for followed ones). Re-sync is
snapshot-aware (unchanged playlists are not re-fetched); the same Spotify track
from two users yields one `hub_tracks` row and two provenance rows, with identity
`(service, service_track_id)` (`isrc` is a nullable attribute, never a key). A
6-month refresh-token expiry surfaces as `needsReconnect` with a re-auth path.

**Leaf checklist**

- [ ] `M3-1` — `feat(hub): Spotify token storage`
- [ ] `M3-2` — `feat(hub): Spotify link flow`
- [ ] `M3-3` — `feat(hub): ingest playlists and likes`
- [ ] `M3-4` — `feat(hub): ingest playlist items`
- [ ] `M3-5` — `feat(hub): track dedup and snapshot re-sync`
- [ ] `M3-6` — `feat(hub): Spotify rate-limit handling`
- [ ] `M3-7` — `feat(hub): sync endpoint`
- [ ] `M3-8` — `feat(hub): surface needs_reconnect and re-auth flow`
- [ ] `M3-9` — `test(hub): ingest integration tests`

### M4 — `hub-v0.4.0` — Exploration surface

**Outcome**: Read endpoints (`/api/hub/users`, `/api/hub/tracks/{id}`,
`/api/hub/overlap`) and a feature-flagged read-only SQL console
(`/api/hub/query`) are live. The hub already runs on `192.168.178.200` over HTTPS
from M2 (`M2-1`), so this milestone carries **no deployment work**.

**Leaf checklist**

- [ ] `M4-1` — `feat(hub): read endpoints over the overlap views`
- [ ] `M4-2` — `feat(hub): guarded read-only SQL console`
- [ ] `M4-3` — `test(hub): read endpoint integration tests`
- [ ] `M4-4` — `docs(hub): README, CHANGELOG and ADR close-out`

---

## 3. Leaf-issue backlog

### M1 — `hub-v0.1.0` — Data model & skeleton

#### M1-1 — `feat(hub): scaffold mmm-hub crate and CI job`

- **Goal / why**: Create the independent crate so every later issue has a home,
  and prove the root workspace is unaffected.
- **Scope**:
  - New crate at `mmm-hub/` with its own `Cargo.toml` (axum 0.8, sqlx 0.8 sqlite,
    tokio, rspotify 0.15, serde, tracing, chrono, anyhow).
  - `mmm-hub/src/main.rs` — CLI `mmm-hub serve --host <H> --port <P>` (default
    `HUB_HOST` / `HUB_PORT`).
  - `mmm-hub/src/config.rs` — env config skeleton reading the `HUB_*` keys.
  - `mmm-hub/src/db/mod.rs` — pool + migrate stub (no migrations yet).
  - `.github/workflows/mmm-hub-ci.yml` — informational job that builds and tests
    inside `mmm-hub/` (mirrors the client's `cargo build --locked` / `cargo test`).
- **Write set**: `mmm-hub/Cargo.toml`, `mmm-hub/src/main.rs`,
  `mmm-hub/src/config.rs`, `mmm-hub/src/db/mod.rs`,
  `.github/workflows/mmm-hub-ci.yml`
- **Acceptance criteria**:
  - [ ] `cd mmm-hub && cargo build` succeeds; root `cargo build` still succeeds
  - [ ] `mmm-hub serve --host 127.0.0.1 --port 8080` starts and binds
  - [ ] CI job runs `cd mmm-hub && cargo build --locked && cargo test --locked`
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:api`
- **Depends on**: —
- **Size**: M · **Migration**: no

#### M1-2 — `feat(hub): migration 001 hub schema tables`

- **Goal / why**: Establish the shared, user-dimensioned data model the whole hub
  is built on.
- **Scope**:
  - `mmm-hub/migrations/001_hub_schema.sql` — create `hub_users`,
    `hub_sessions`, `hub_service_accounts`, `hub_tracks`, `hub_playlists`,
    `hub_playlist_tracks`, `hub_liked_tracks` with exactly the columns and
    constraints in the contract (§5), including `UNIQUE(service, service_track_id)`,
    `UNIQUE(user_id, service)` and `UNIQUE(user_id, service, playlist_id)`.
  - `service CHECK IN ('spotify','soundcloud','youtube')` on
    `hub_service_accounts`.
- **Write set**: `mmm-hub/migrations/001_hub_schema.sql`
- **Acceptance criteria**:
  - [ ] Fresh DB runs `001_hub_schema.sql` end-to-end without error
  - [ ] All seven tables exist with the contract columns
  - [ ] Migration-integrity test creates a fresh DB and runs all migrations
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:db`
- **Depends on**: `M1-1`
- **Size**: M · **Migration**: yes (`001_hub_schema.sql`)

#### M1-3 — `feat(hub): migration 001 hub overlap views and indexes`

- **Goal / why**: The four views are the entire point of v1 — they turn raw
  membership rows into presence / overlap answers.
- **Scope**:
  - Append to `mmm-hub/migrations/001_hub_schema.sql` the views
    `hub_v_track_presence` (`track_id, user_id, source, playlist_id, playlist_name`),
    `hub_v_shared_tracks` (`track_id, user_count, user_ids`, ≥ 2 distinct users),
    `hub_v_user_overlap` (`user_a_id, user_b_id, shared_tracks`),
    `hub_v_track_playlists` (`track_id, user_id, playlist_id, playlist_name`).
  - Supporting indexes for the view joins (`hub_playlist_tracks(track_id)`,
    `hub_liked_tracks(track_id)`, `hub_playlists(user_id)`).
- **Write set**: `mmm-hub/migrations/001_hub_schema.sql`
- **Acceptance criteria**:
  - [ ] All four views exist and are queryable on a fresh DB
  - [ ] `hub_v_shared_tracks` returns only tracks with ≥ 2 distinct users
  - [ ] `hub_v_user_overlap` emits one row per unordered user pair
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:db`
- **Depends on**: `M1-2` (same file — must land after the tables)
- **Size**: M · **Migration**: yes (`001_hub_schema.sql`)

#### M1-4 — `test(hub): hub seed fixtures`

- **Goal / why**: Deterministic fixtures are the substrate for every view test
  and later ingest test.
- **Scope**:
  - `mmm-hub/src/db/testing.rs` — seed 3 users, overlapping playlists and likes
    (a track shared by exactly 2 users, one shared by all 3, one unique per user).
  - Register a seed entry point used by integration tests.
- **Write set**: `mmm-hub/src/db/testing.rs`
- **Acceptance criteria**:
  - [ ] Seeding a fresh DB yields the documented row counts
  - [ ] Fixture is idempotent (re-seed does not duplicate)
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:db`
- **Depends on**: `M1-2`
- **Size**: M · **Migration**: no

#### M1-5 — `test(hub): integration tests for hub overlap views`

- **Goal / why**: Prove the schema + views with hand-crafted data and exact
  assertions — the M1 acceptance gate.
- **Scope**:
  - `mmm-hub/tests/views.rs` — one test per view: `hub_v_track_presence`,
    `hub_v_shared_tracks`, `hub_v_user_overlap`, `hub_v_track_playlists`.
  - Assert exact row counts and field values over the M1-4 fixtures.
- **Write set**: `mmm-hub/tests/views.rs`
- **Acceptance criteria**:
  - [ ] One test per view, each asserting exact row counts
  - [ ] `hub_v_shared_tracks` excludes single-user tracks
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:db`
- **Depends on**: `M1-2`, `M1-3`, `M1-4`
- **Size**: M · **Migration**: no

#### M1-6 — `docs(hub): README stub and ADR for separate hub crate`

- **Goal / why**: Record the architectural decision (ADR-014 lineage) and give
  the crate a landing page.
- **Scope**:
  - `mmm-hub/README.md` — stub: what the hub is, how to run `mmm-hub serve`.
  - `docs/DECISIONS.md` — ADR: multi-user hub as a separate crate, shared DB with
    a user dimension.
  - `plans/README.md` — add the `mmm-hub` plan to the index.
- **Write set**: `mmm-hub/README.md`, `docs/DECISIONS.md`, `plans/README.md`
- **Acceptance criteria**:
  - [ ] ADR follows the ADR-### format (date, status, context, decision, consequences)
  - [ ] `plans/README.md` lists `mmm-hub`
  - [ ] Validation: `cd mmm-hub && cargo test` (docs change must not break the build)
- **Labels**: `enhancement`
- **Depends on**: `M1-1`
- **Size**: S · **Migration**: no

---

### M2 — `hub-v0.2.0` — Accounts & sessions (OIDC / Pocket ID)

#### M2-1 — `feat(hub): TLS reverse proxy and HTTPS base URL`

- **Goal / why**: Both external integrations need HTTPS before M2 can reach the
  outside world: Pocket ID's OIDC `IssuerUrl` is **https-only**, and the Spotify
  redirect URI must be HTTPS because a bare LAN IP (`192.168.178.200`) is **not**
  loopback. This makes TLS a **prerequisite of M2**, not a deploy-afterthought.
- **Scope**:
  - `mmm-hub/deploy/Caddyfile` — TLS-terminating reverse proxy in front of the hub
    port for the public hostname on `192.168.178.200`.
  - `mmm-hub/deploy/mmm-hub.service` — systemd unit running `mmm-hub serve`.
  - `mmm-hub/deploy/README.md` — install/run notes; document that
    `HUB_PUBLIC_URL`, `SPOTIFY_REDIRECT_URI` and `OIDC_REDIRECT_URI` must be
    `https://…` and registered verbatim with Spotify / Pocket ID.
- **Write set**: `mmm-hub/deploy/Caddyfile`, `mmm-hub/deploy/mmm-hub.service`,
  `mmm-hub/deploy/README.md`
- **Acceptance criteria**:
  - [ ] Unit starts the hub behind the proxy and restarts on failure
  - [ ] Caddy terminates TLS and proxies to `HUB_PORT`; `HUB_PUBLIC_URL` is HTTPS
  - [ ] `SPOTIFY_REDIRECT_URI` / `OIDC_REDIRECT_URI` documented as HTTPS
  - [ ] Validation: `cd mmm-hub && cargo test` (deploy files lint-checked by review)
- **Labels**: `enhancement`
- **Depends on**: `M1-1`
- **Size**: M · **Migration**: no

#### M2-2 — `feat(hub): OIDC configuration from environment`

- **Goal / why**: The auth flow needs validated OIDC settings before it can run.
- **Scope**:
  - Extend `mmm-hub/src/config.rs` with `OIDC_ISSUER`, `OIDC_CLIENT_ID`,
    `OIDC_CLIENT_SECRET`, `OIDC_REDIRECT_URI`, plus `HUB_PUBLIC_URL` and
    `HUB_SESSION_TTL_SECS` (default `2592000`).
  - Reject an `OIDC_ISSUER` that is not `https://` (the `openidconnect` `IssuerUrl`
    is https-only) with a clear startup error.
  - Fail fast with a clear error when a required OIDC key is missing.
- **Write set**: `mmm-hub/src/config.rs`
- **Acceptance criteria**:
  - [ ] All keys parsed from env with the contract defaults
  - [ ] Non-HTTPS `OIDC_ISSUER` and missing required key produce actionable startup errors
  - [ ] Unit test for parsing + defaults
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:api`
- **Depends on**: `M1-1`
- **Size**: S · **Migration**: no

#### M2-3 — `feat(hub): migration 002 hub sessions table`

- **Goal / why**: Server-side sessions (decision D4: hand-rolled table, not
  `tower-sessions`).
- **Scope**:
  - `mmm-hub/migrations/002_hub_sessions.sql` — `hub_sessions` (`id TEXT PK`,
    `user_id FK hub_users`, `created_at`, `last_seen_at`, `expires_at`) + index on
    `expires_at`.
- **Write set**: `mmm-hub/migrations/002_hub_sessions.sql`
- **Acceptance criteria**:
  - [ ] Fresh DB runs 001 + 002 end-to-end
  - [ ] `hub_sessions` matches the contract columns
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:db`
- **Depends on**: `M1-2`
- **Size**: S · **Migration**: yes (`002_hub_sessions.sql`)

#### M2-4 — `feat(hub): OIDC login flow with PKCE`

- **Goal / why**: A user must be able to log in via Pocket ID and be upserted by
  `oidc_subject`.
- **Scope**:
  - `mmm-hub/src/auth/oidc.rs` — discovery from
    `{issuer}/.well-known/openid-configuration`, authorize redirect + callback
    with PKCE (S256), ID-token validation (`openidconnect` 4.x).
  - Upsert `hub_users` by `oidc_subject` (set `display_name`, `email`).
  - `mmm-hub/src/auth/mod.rs` — module wiring.
- **Write set**: `mmm-hub/src/auth/oidc.rs`, `mmm-hub/src/auth/mod.rs`
- **Acceptance criteria**:
  - [ ] Callback creates a `hub_users` row keyed by `oidc_subject`
  - [ ] Second login upserts the same row (no duplicate)
  - [ ] PKCE S256 verifier/challenge round-trips
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:api`
- **Depends on**: `M2-2`, `M2-3`
- **Size**: L · **Migration**: no

#### M2-5 — `feat(hub): session middleware`

- **Goal / why**: Every route except health and the auth flow must require a
  valid session.
- **Scope**:
  - `mmm-hub/src/auth/middleware.rs` — issue HTTP-only session cookie, resolve
    cookie → `hub_sessions` → `hub_users`, reject unauthenticated requests.
  - Wire the router in `mmm-hub/src/main.rs`: public = `/api/hub/health` +
    `/api/hub/services/{service}/callback`; everything else guarded.
- **Write set**: `mmm-hub/src/auth/middleware.rs`, `mmm-hub/src/main.rs`
- **Acceptance criteria**:
  - [ ] Guarded route without cookie → `401`
  - [ ] Expired session (`expires_at` past) → `401`
  - [ ] Health + callback reachable without a session
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:api`
- **Depends on**: `M2-4`
- **Size**: M · **Migration**: no

#### M2-6 — `feat(hub): health and me endpoints`

- **Goal / why**: Liveness plus the first authenticated read surface, including
  the `needsReconnect` field M3 fills in.
- **Scope**:
  - `mmm-hub/src/api/health.rs` — `GET /api/hub/health` →
    `{ "data": { "status": "ok", "version": "…" } }`.
  - `mmm-hub/src/api/me.rs` — `GET /api/hub/me` → current user + `services`
    array (`{ "service":"spotify", "connected": …, "needsReconnect": … }`).
  - `mmm-hub/src/api/mod.rs` — register both handlers.
- **Write set**: `mmm-hub/src/api/health.rs`, `mmm-hub/src/api/me.rs`,
  `mmm-hub/src/api/mod.rs`
- **Acceptance criteria**:
  - [ ] `/api/hub/health` returns `status: ok` + version, unauthenticated
  - [ ] `/api/hub/me` returns the session user and a `services` array
  - [ ] Each service entry carries `connected` and `needsReconnect` booleans
  - [ ] Response envelope matches `{ "data": … }`
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:api`
- **Depends on**: `M2-3`
- **Size**: M · **Migration**: no

#### M2-7 — `test(hub): auth integration tests`

- **Goal / why**: Prove the login → session → guarded-route chain end-to-end.
- **Scope**:
  - `mmm-hub/tests/auth.rs` — login callback creates user + session; guarded
    route rejects without cookie; `/api/hub/me` returns the right user.
  - Use a test issuer fixture or mocked OIDC discovery (no live Pocket ID) with an
    HTTPS issuer URL.
- **Write set**: `mmm-hub/tests/auth.rs`
- **Acceptance criteria**:
  - [ ] Login callback creates a `hub_users` row + a `hub_sessions` row
  - [ ] Guarded route without cookie → `401`; with cookie → `200`
  - [ ] `/api/hub/me` `services[0]` includes `connected` + `needsReconnect`
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:api`
- **Depends on**: `M2-5`, `M2-6`
- **Size**: M · **Migration**: no

---

### M3 — `hub-v0.3.0` — Spotify link & ingest

#### M3-1 — `feat(hub): Spotify token storage`

- **Goal / why**: Tokens must persist per user so ingest can run without
  re-authorising, and a dead **refresh token** (6-month lifetime from the original
  authorization) must degrade to a `needs_reconnect` state instead of a failed
  sync.
- **Scope**:
  - `mmm-hub/src/db/service_accounts.rs` — upsert/read `hub_service_accounts`
    (`access_token`, `refresh_token`, `token_expiry`, `scopes`, `remote_user_id`,
    `display_name`, `connected_at`), `UNIQUE(user_id, service)`.
  - Store `connected_at` as the **authorization timestamp** (set on first
    authorization, reset on every re-auth) — the reference point for the 6-month
    refresh-token lifetime.
  - `mmm-hub/src/ingest/tokens.rs` — refresh-on-expiry helper; on the token
    endpoint's `400 invalid_grant` clear the stored tokens while **keeping the
    account row**, so the account reads as needing reconnection (no silent
    failure, no retry loop).
- **Write set**: `mmm-hub/src/db/service_accounts.rs`,
  `mmm-hub/src/ingest/tokens.rs`
- **Acceptance criteria**:
  - [ ] Tokens upsert keyed to `(user_id, 'spotify')` without duplicates
  - [ ] Expired access token triggers refresh and persists the new expiry
  - [ ] `connected_at` is recorded at authorization and reset on re-auth
  - [ ] `invalid_grant` clears the tokens (row kept) so `/api/hub/me` reports
        `needsReconnect: true` (see `M3-8`)
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:spotify`
- **Depends on**: `M2-7`
- **Size**: M · **Migration**: no

#### M3-2 — `feat(hub): Spotify link flow`

- **Goal / why**: Bind a Spotify account to the logged-in hub user, and provide
  the re-auth path for a user whose refresh token has expired.
- **Scope**:
  - `mmm-hub/src/api/services.rs` — `POST /api/hub/services/{service}/auth`
    returns `{ "data": { "authorizeUrl": … } }` with `state` carrying the session
    user; `GET /api/hub/services/{service}/callback` exchanges the code and binds
    tokens to that `user_id`.
  - Request scopes `user-library-read`, `playlist-read-private`,
    `playlist-read-collaborative`, `user-read-private`, `user-read-email`.
  - Re-running the link flow is the **re-auth path**: it overwrites the tokens and
    resets `connected_at`, clearing the needs-reconnect state.
  - `SPOTIFY_REDIRECT_URI` must be the HTTPS URL established in `M2-1`.
  - `{service}` other than `spotify` → `501 Not Implemented`.
- **Write set**: `mmm-hub/src/api/services.rs`
- **Acceptance criteria**:
  - [ ] Auth endpoint returns a Spotify authorize URL with a signed `state`
  - [ ] Callback stores tokens for the `state`'s user and records `connected_at`
  - [ ] Re-auth overwrites tokens and clears a prior needs-reconnect state
  - [ ] Non-`spotify` service → `501`
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:spotify`, `area:api`
- **Depends on**: `M3-1`
- **Size**: M · **Migration**: no

#### M3-3 — `feat(hub): ingest playlists and likes`

- **Goal / why**: Pull the user's playlist list (all visibility) and liked tracks
  into the shared DB (ingest step A), classifying playlists so only
  owned/collaborative ones are ingestable.
- **Scope**:
  - `mmm-hub/src/ingest/spotify.rs` — `GET /me`, `GET /me/playlists` (paged) →
    upsert `hub_playlists`; `GET /me/tracks` (paged) → upsert `hub_tracks` +
    `hub_liked_tracks` from the embedded track objects.
  - Classify each playlist as **owned/collaborative** (`owner.id ==
hub_service_accounts.remote_user_id`, or the playlist's `collaborative` flag)
    versus **followed**; followed playlists are stored as metadata only (no
    track ingest).
  - Store `snapshot_id`, `track_count`, `is_liked`, `fetched_at` on
    `hub_playlists`.
  - Do **not** call removed batch endpoints (`GET /tracks?ids=`, `/albums`, …).
- **Write set**: `mmm-hub/src/ingest/spotify.rs`
- **Acceptance criteria**:
  - [ ] Playlists upsert into `hub_playlists` keyed by
        `(user_id, service, playlist_id)`
  - [ ] Owned/collaborative vs followed is distinguished per playlist
  - [ ] Likes upsert into `hub_tracks` + `hub_liked_tracks` keyed by
        `(user_id, track_id)` from the embedded track objects
  - [ ] No removed batch endpoint is called
  - [ ] Re-run is idempotent (no duplicate rows)
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:spotify`
- **Depends on**: `M3-1`
- **Size**: L · **Migration**: no

#### M3-4 — `feat(hub): ingest playlist items`

- **Goal / why**: Fill `hub_tracks` + `hub_playlist_tracks` for the playlists the
  API actually exposes items for (ingest step B).
- **Scope**:
  - `mmm-hub/src/ingest/playlist_items.rs` — for **owned/collaborative**
    playlists only, `GET /playlists/{id}/items` paged at `limit ≤ 50` (offset/
    `next`), upserting `hub_tracks` + `hub_playlist_tracks` (`position`,
    `added_at`).
  - **Skip followed playlists** — the API returns metadata only (no `items`), so
    they yield no `hub_playlist_tracks` rows.
  - Filter to `type == "track"` and `is_local == false` (drop local files and
    episodes); enrich from the track objects embedded in the items response
    rather than a separate batch fetch.
- **Write set**: `mmm-hub/src/ingest/playlist_items.rs`
- **Acceptance criteria**:
  - [ ] Uses `GET /playlists/{id}/items` with `limit ≤ 50` and paginates via `next`
  - [ ] Followed playlists produce **no** `hub_playlist_tracks` rows
  - [ ] Local files / episodes are filtered out (only `type == "track"`, non-local)
  - [ ] Tracks upsert into `hub_tracks` keyed by `(service, service_track_id)`
  - [ ] Membership rows land in `hub_playlist_tracks` with `position` + `added_at`
  - [ ] No batch-fetch endpoint is called
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:spotify`
- **Depends on**: `M3-3`
- **Size**: M · **Migration**: no

#### M3-5 — `feat(hub): track dedup and snapshot re-sync`

- **Goal / why**: One global `hub_tracks` row per Spotify track across users, and
  unchanged playlists must not be re-fetched.
- **Scope**:
  - `mmm-hub/src/ingest/dedup.rs` — dedup by **`(service, service_track_id)`**
    only; `isrc` is a nullable secondary attribute and is **never** a key.
    Snapshot-aware skip when `snapshot_id` is unchanged.
  - `mmm-hub/migrations/003_hub_ingest_indexes.sql` — indexes supporting dedup +
    re-sync (`hub_playlist_tracks(track_id)`, `hub_liked_tracks(track_id)`);
    **no** index/constraint on `isrc`.
- **Write set**: `mmm-hub/src/ingest/dedup.rs`,
  `mmm-hub/migrations/003_hub_ingest_indexes.sql`
- **Acceptance criteria**:
  - [ ] Same track from two users → one `hub_tracks` row, two provenance rows
  - [ ] Dedup key is `(service, service_track_id)`; no `isrc` uniqueness/index
  - [ ] A track missing `isrc` still dedups correctly
  - [ ] Re-sync with unchanged `snapshot_id` does not re-fetch playlist items
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:spotify`, `area:db`
- **Depends on**: `M3-3`, `M3-4`
- **Size**: L · **Migration**: yes (`003_hub_ingest_indexes.sql`)

#### M3-6 — `feat(hub): Spotify rate-limit handling`

- **Goal / why**: Spotify 429s must not fail or stall a sync; reuse the client's
  cooldown pattern and distinguish transient rate-limiting from dev-mode quota
  exhaustion.
- **Scope**:
  - `mmm-hub/src/ingest/rate_limit.rs` — honour `Retry-After`, bounded backoff,
    surface a clear error after the retry budget.
  - Handle dev-mode **quota exhaustion**: a `429` whose body carries
    `reason: "QUOTA_EXCEEDED"` (and **no** `Retry-After`) is not retried on a tight
    loop — surface it as a slower / non-immediately-retryable condition.
- **Write set**: `mmm-hub/src/ingest/rate_limit.rs`
- **Acceptance criteria**:
  - [ ] A `429` with `Retry-After` waits and retries
  - [ ] `reason: "QUOTA_EXCEEDED"` (no `Retry-After`) is not retried on a tight loop
  - [ ] Retry budget exhausted → actionable error, no infinite loop
  - [ ] Unit test with a mocked 429 response (both variants)
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:spotify`
- **Depends on**: `M3-1`
- **Size**: S · **Migration**: no

#### M3-7 — `feat(hub): sync endpoint`

- **Goal / why**: Give the user a way to kick off ingest for their own account.
- **Scope**:
  - `mmm-hub/src/api/sync.rs` — `POST /api/hub/services/{service}/sync` →
    `{ "data": { "started": true } }`, running ingest for the session user.
  - Register the route in `mmm-hub/src/main.rs`.
- **Write set**: `mmm-hub/src/api/sync.rs`, `mmm-hub/src/main.rs`
- **Acceptance criteria**:
  - [ ] Sync runs ingest for the session user only
  - [ ] Unlinked service → clear error; non-`spotify` → `501`
  - [ ] A `needsReconnect` account is reported, not silently re-synced
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:api`, `area:spotify`
- **Depends on**: `M3-5`
- **Size**: S · **Migration**: no

#### M3-8 — `feat(hub): surface needs_reconnect and re-auth flow`

- **Goal / why**: A 6-month refresh-token expiry must be **visible** and
  **recoverable**, not a silent ingest failure (`/api/hub/me` is the contract's
  state surface).
- **Scope**:
  - `mmm-hub/src/api/me.rs` — compute `needsReconnect` per service account: true
    when `connected_at + 6 months` has passed **or** the stored tokens were cleared
    after an `invalid_grant`.
  - `mmm-hub/src/api/services.rs` — the re-auth path: re-invoking
    `POST /api/hub/services/spotify/auth` re-runs the link flow (`M3-2`) and clears
    the needs-reconnect state.
  - Distinguish three states in the `services` array: never linked
    (`connected:false, needsReconnect:false`), linked
    (`connected:true, needsReconnect:false`), and reconnect required
    (`connected:false, needsReconnect:true`).
- **Write set**: `mmm-hub/src/api/me.rs`, `mmm-hub/src/api/services.rs`
- **Acceptance criteria**:
  - [ ] `/api/hub/me` returns `needsReconnect: true` when `connected_at` is > 6
        months ago or the tokens were cleared
  - [ ] Re-invoking the auth endpoint clears the state and resets `connected_at`
  - [ ] The three linked/never-linked/needs-reconnect states are distinguishable
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:spotify`, `area:api`
- **Depends on**: `M3-1`, `M3-2`
- **Size**: M · **Migration**: no

#### M3-9 — `test(hub): ingest integration tests`

- **Goal / why**: Prove the full link → ingest → dedup chain against a mocked
  Spotify API, including the API's access restrictions.
- **Scope**:
  - `mmm-hub/tests/ingest.rs` — mocked Spotify responses; assert playlists,
    tracks, likes, followed-vs-owned behaviour, and cross-user dedup row counts.
  - Cover `needsReconnect` after a mocked `invalid_grant`.
- **Write set**: `mmm-hub/tests/ingest.rs`
- **Acceptance criteria**:
  - [ ] Ingest of two users' overlapping libraries yields one shared track row
        deduped by `(service, service_track_id)`
  - [ ] An owned playlist ingests items; a followed playlist yields metadata only
        (no `hub_playlist_tracks` rows)
  - [ ] A `429 QUOTA_EXCEEDED` response does not cause an infinite retry loop
  - [ ] Snapshot-unchanged re-sync performs no playlist-item fetch
  - [ ] An `invalid_grant` surfaces `needsReconnect: true` via `/api/hub/me`
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:spotify`
- **Depends on**: `M3-7`, `M3-8`
- **Size**: M · **Migration**: no

---

### M4 — `hub-v0.4.0` — Exploration surface

#### M4-1 — `feat(hub): read endpoints over the overlap views`

- **Goal / why**: The queryable data basis becomes reachable over HTTP.
- **Scope**:
  - `mmm-hub/src/api/read.rs` — `GET /api/hub/users`,
    `GET /api/hub/tracks/{id}` (presence: who / why / where),
    `GET /api/hub/overlap` (shared tracks + pairwise counts).
  - Register routes in `mmm-hub/src/api/mod.rs` and `mmm-hub/src/main.rs`.
- **Write set**: `mmm-hub/src/api/read.rs`, `mmm-hub/src/api/mod.rs`,
  `mmm-hub/src/main.rs`
- **Acceptance criteria**:
  - [ ] `/api/hub/users` returns `{ id, displayName }` rows
  - [ ] `/api/hub/tracks/{id}` returns the track + `presence` array
  - [ ] `/api/hub/overlap` returns `shared` + `pairs`
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:api`
- **Depends on**: `M3-9`
- **Size**: M · **Migration**: no

#### M4-2 — `feat(hub): guarded read-only SQL console`

- **Goal / why**: Power-user exploration without a UI, safely.
- **Scope**:
  - `mmm-hub/src/db/readonly.rs` — separate read-only connection
    (`mode=ro`, `PRAGMA query_only=1`).
  - `mmm-hub/src/api/query.rs` — `POST /api/hub/query`, `SELECT`/`WITH` only,
    gated by `HUB_SQL_CONSOLE_ENABLED` (default `false`).
- **Write set**: `mmm-hub/src/db/readonly.rs`, `mmm-hub/src/api/query.rs`
- **Acceptance criteria**:
  - [ ] Non-`SELECT`/`WITH` statements are rejected
  - [ ] Disabled flag → `404`/`403`; enabled → `{ columns, rows }`
  - [ ] Writes fail even if a statement slips through (`query_only`)
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:api`, `area:db`
- **Depends on**: `M4-1` (route registration in `api/mod.rs` / `main.rs`)
- **Size**: M · **Migration**: no

#### M4-3 — `test(hub): read endpoint integration tests`

- **Goal / why**: Lock the read surface + console behaviour with tests.
- **Scope**:
  - `mmm-hub/tests/read.rs` — `/api/hub/users`, `/api/hub/tracks/{id}`,
    `/api/hub/overlap`, and `/api/hub/query` (reject non-`SELECT`, flag off/on).
- **Write set**: `mmm-hub/tests/read.rs`
- **Acceptance criteria**:
  - [ ] Each read endpoint has a test asserting the response shape
  - [ ] `/api/hub/query` rejects a non-`SELECT` statement
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`, `area:api`
- **Depends on**: `M4-1`, `M4-2`
- **Size**: M · **Migration**: no

#### M4-4 — `docs(hub): README, CHANGELOG and ADR close-out`

- **Goal / why**: Close the release with run/deploy docs and the decision record.
- **Scope**:
  - `mmm-hub/README.md` — final run notes; reference the HTTPS deploy set up in
    `M2-1` (`192.168.178.200`).
  - `CHANGELOG.md` — `hub-v0.4.0` entry (Added / Changed / Fixed).
  - `docs/DECISIONS.md` — ADR close-out (owned-only ingest / followed-metadata,
    token-at-rest + 6-month re-auth, session mechanism).
  - `plans/README.md` — mark the `mmm-hub` plan status.
- **Write set**: `mmm-hub/README.md`, `CHANGELOG.md`, `docs/DECISIONS.md`,
  `plans/README.md`
- **Acceptance criteria**:
  - [ ] README documents run + deploy + all `HUB_*` / `OIDC_*` / `SPOTIFY_*` keys
  - [ ] CHANGELOG has the `hub-v0.4.0` entry
  - [ ] ADRs record the ingest-scope/token/session decisions
  - [ ] Validation: `cd mmm-hub && cargo test`
- **Labels**: `enhancement`
- **Depends on**: `M4-1`, `M4-2`, `M4-3`
- **Size**: S · **Migration**: no

---

## 4. Dependency / order table

`→` means "must land before". Items on the same line after `‖` are
**file-disjoint and parallelizable** once their dependencies are met.

### M1 — `hub-v0.1.0`

| Order | Issues (parallel group) | Notes                                                                                                   |
| ----- | ----------------------- | ------------------------------------------------------------------------------------------------------- |
| 1     | `M1-1`                  | Scaffold must exist first.                                                                              |
| 2     | `M1-2` ‖ `M1-6`         | Tables and docs are disjoint.                                                                           |
| 3     | `M1-3` ‖ `M1-4`         | Views and seed fixtures are disjoint; `M1-3` shares `001_hub_schema.sql` with `M1-2`, so it follows it. |
| 4     | `M1-5`                  | Needs tables + views + fixtures.                                                                        |

**Parallelizable**: `M1-2`/`M1-6`, then `M1-3`/`M1-4`. `M1-2` and `M1-3` are
**not** parallel (same migration file).

### M2 — `hub-v0.2.0`

| Order | Issues (parallel group)  | Notes                                                                                                    |
| ----- | ------------------------ | -------------------------------------------------------------------------------------------------------- |
| 1     | `M2-1` ‖ `M2-2` ‖ `M2-3` | TLS deploy files, config and migration are disjoint. `M2-1` is the HTTPS prerequisite for the OIDC flow. |
| 2     | `M2-4` ‖ `M2-6`          | OIDC flow (needs config + sessions) and `me` handler (needs sessions) touch disjoint files.              |
| 3     | `M2-5`                   | Session middleware needs the OIDC flow; owns `main.rs` router wiring.                                    |
| 4     | `M2-7`                   | End-to-end auth tests.                                                                                   |

**Parallelizable**: `M2-1`, `M2-2`, `M2-3`, then `M2-4` + `M2-6`. `M2-1` (TLS +
reverse proxy) must be live before the OIDC/Spotify flows are exercised against
real endpoints. `M2-5` and `M2-7` are serial tails.

### M3 — `hub-v0.3.0`

| Order | Issues (parallel group)  | Notes                                                                               |
| ----- | ------------------------ | ----------------------------------------------------------------------------------- |
| 1     | `M3-1`                   | Token storage is the base for everything (incl. the authorization timestamp).       |
| 2     | `M3-2` ‖ `M3-3` ‖ `M3-6` | Link flow, playlists+likes ingest and rate-limit helper are disjoint.               |
| 3     | `M3-4`                   | Playlist items need the owned/collaborative classification from `M3-3`.             |
| 4     | `M3-5` ‖ `M3-8`          | Dedup/re-sync (`M3-5`) and needs-reconnect surfacing (`M3-8`) touch disjoint files. |
| 5     | `M3-7`                   | Sync endpoint wires the ingest together.                                            |
| 6     | `M3-9`                   | Ingest integration tests.                                                           |

**Parallelizable**: `M3-2`, `M3-3`, `M3-6`, then `M3-5` + `M3-8`. `M3-4` → `M3-7` →
`M3-9` are serial (`M3-9` also needs `M3-8`).

### M4 — `hub-v0.4.0`

| Order | Issues (parallel group) | Notes                                                                 |
| ----- | ----------------------- | --------------------------------------------------------------------- |
| 1     | `M4-1`                  | Read endpoints over the overlap views.                                |
| 2     | `M4-2`                  | Console route registration follows `M4-1`'s `api/mod.rs` / `main.rs`. |
| 3     | `M4-3`                  | Read + console tests.                                                 |
| 4     | `M4-4`                  | Docs close-out.                                                       |

**Parallelizable**: none — the four issues share `api/*` and docs files, so M4 is
serial. (Deploy no longer appears here; the TLS/reverse-proxy work moved to
`M2-1` as an M2 prerequisite.)

### Cross-milestone

M1 → M2 → M3 → M4 are sequential at the milestone level (each is a release).
Within a milestone, the parallel groups above are the only safe concurrency; a
leaf issue never starts before its listed dependencies have merged to `main`.

Every leaf issue is one branch, one PR, one `Closes #<n>`, with a
Conventional-Commit title — and none of them ever commits to `main`.
