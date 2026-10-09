# MMM Hub — Frozen Interface Contract

> Read this FIRST. Everything in `plans/mmm-hub/*.md` is written against this
> contract. If a detail contradicts this file, this file wins — or flag it as an
> open question instead of silently diverging.

**Status**: draft for review · **Owner**: coordinator · **Last updated**: 2026-10-06

---

## 0. What the Hub is

A separate, multi-user **ingest + exploration service** ("MMM Hub") that lives in
this repo as its own crate, decoupled from the single-user client core
(ADR-014). Users log in with a generic account (OIDC / Pocket ID), link
streaming services (Spotify in v1), and the backend pulls playlists / tracks /
likes into **one shared database** where every row carries a `user_id`. v1 has
no polished UI — the goal is a queryable data basis (SQL + a few read endpoints).

Frozen scope boundary for v1: **metadata only** (no audio hosting/downloading),
**Spotify only**, **no UI**.

---

## 1. Frozen decisions (contract)

| Area           | Decision                                                                                                                                                         |
| -------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Crate          | New, independent crate at `mmm-hub/` (like `music-api/`). Own `Cargo.toml`, own `migrations/`, own `tests/`. Root `cargo build` must stay unaffected.            |
| Stack          | Same as client: `axum` 0.8, `sqlx` 0.8 (sqlite), `tokio`, `rspotify` 0.15, `serde`, `tracing`, `chrono`, `anyhow`.                                               |
| Binary / CLI   | `mmm-hub serve --host <H> --port <P>`.                                                                                                                           |
| Default port   | `8080` (client keeps 3000).                                                                                                                                      |
| DB file        | `hub.db` (SQLite, WAL).                                                                                                                                          |
| Migrations     | Hub-owned chain `mmm-hub/migrations/NNN_*.sql`, additive, **never** touch the client chain in `migrations/`. One net-new migration per milestone where possible. |
| Host           | `192.168.178.200` (LAN).                                                                                                                                         |
| Prefix         | All hub tables/views are prefixed `hub_`.                                                                                                                        |
| Config         | Env vars, `HUB_`/`OIDC_`/`SPOTIFY_` prefixed (see §4).                                                                                                           |
| Version line   | Working assumption: own line `hub-v0.1.0` … `hub-v0.4.0` (decision **D1** may re-map to the app train; only the tags change, not the work).                      |
| Issue IDs      | Milestone-scoped local IDs `M<ms>-<n>` (e.g. `M1-3`). GitHub numbers are assigned on creation, not known here.                                                   |
| Write-set rule | One leaf issue = one PR = `Closes #<n>`. Conventional-Commit PR title. Never commit to `main`.                                                                   |

---

## 1a. Verified external constraints (2026-10-06)

Spotify shipped breaking Web API changes in 2026 that bound this plan. Facts
confirmed against the official Spotify docs by the coordinator (sources in
`verification.md`):

- **Dev mode = 5 authenticated users per app** (not 25), and the **app owner must
  hold Spotify Premium**. Extended Quota Mode is **organizations only (≥250k MAU)**
  since 2025-05-15 → unreachable for this project. "Multi-user" in practice means
  **≤5 allow-listed friends** unless we use **one Spotify app per user** (decision D6).
- **Playlist contents are owner/collaborator-only.** `GET /playlists/{id}/items`
  returns `items` **only** for playlists the user owns or collaborates on; merely
  _followed_ playlists return **metadata only** (no `items`). Liked songs
  (`GET /me/tracks`) are **unaffected**.
- **Batch fetch endpoints removed** (`GET /tracks?ids=`, `/albums`, `/artists`, …).
  Ingest pages playlist items at `limit ≤ 50`; the embedded track objects avoid the
  need for a separate batch fetch.
- **`isrc` is available** (`external_ids`, restored March 2026) but is **nullable and
  non-unique** → never a dedup key; identity is `(service, service_track_id)`.
- **Refresh tokens expire after 6 months** from original authorization; on expiry the
  token endpoint returns `400 invalid_grant` → user must re-run the Spotify flow.
  `/api/hub/me` must surface a "needs reconnect" state.
- **HTTPS is mandatory** for the Spotify redirect URI (a LAN IP is not loopback) and
  for the OIDC issuer (`openidconnect` `IssuerUrl` is https-only) → TLS must exist by
  **M2/M3**, not M4 (deployment ordering corrected).

**Resolved decisions (2026-10-06):** D6 — accept the API-conform ingest scope
(**liked songs + owned/collaborative playlists**; _followed_ playlists are stored
as metadata only). D7 — **one shared Spotify app**; the owner's app is grandfathered
(pre-2026) so the 5-user cap is not binding for this group. NOTE: grandfathering
lifts the _account/user_ cap only — the **playlist-items restriction** and the
**6-month refresh-token expiry** still apply.

---

## 2. Milestones

| #   | Milestone             | Outcome                                                                                                                | Version      | Migration |
| --- | --------------------- | ---------------------------------------------------------------------------------------------------------------------- | ------------ | --------- |
| M1  | Data model & skeleton | Crate builds standalone; schema + 4 overlap views proven by integration tests over seed fixtures. No network, no auth. | `hub-v0.1.0` | 001       |
| M2  | Accounts & sessions   | OIDC login (Pocket ID), session cookie, `/api/hub/me`, unauthenticated access blocked.                                 | `hub-v0.2.0` | 002       |
| M3  | Spotify link & ingest | Linked user's playlists/tracks/likes in the shared DB; snapshot-aware re-sync; cross-user dedup proven.                | `hub-v0.3.0` | 003       |
| M4  | Exploration & deploy  | Read endpoints + guarded read-only SQL console; hub live on `192.168.178.200` over HTTPS.                              | `hub-v0.4.0` | —         |

After M4 (SoundCloud/YouTube, cross-user tag subscribe, UI) = later 2.x line,
out of scope here.

---

## 3. HTTP surface (frozen paths + auth requirement)

All responses JSON. Error shape mirrors the client's `ApiResponse { data }`.

| Method | Path                                   | Auth       | Purpose                          | v1 response sketch                                                                                                  |
| ------ | -------------------------------------- | ---------- | -------------------------------- | ------------------------------------------------------------------------------------------------------------------- |
| GET    | `/api/hub/health`                      | no         | liveness + version               | `{ "data": { "status": "ok", "version": "…" } }`                                                                    |
| GET    | `/api/hub/auth/login`                  | no         | start OIDC login (PKCE)          | `302` → Pocket ID `/authorize`                                                                                      |
| GET    | `/api/hub/auth/callback`               | state      | OIDC redirect target             | `302` → hub, sets session cookie                                                                                    |
| POST   | `/api/hub/auth/logout`                 | yes        | end session                      | `{ "data": { "loggedOut": true } }`                                                                                 |
| GET    | `/api/hub/me`                          | yes        | current user + linked services   | `{ "data": { "user": {…}, "services": [ { "service":"spotify", "connected":true, "needsReconnect":false, … } ] } }` |
| POST   | `/api/hub/services/{service}/auth`     | yes        | start service link               | `{ "data": { "authorizeUrl": "https://accounts.spotify.com/…" } }`                                                  |
| GET    | `/api/hub/services/{service}/callback` | state      | OAuth redirect target            | `302` back to hub                                                                                                   |
| POST   | `/api/hub/services/{service}/sync`     | yes        | kick off ingest for current user | `{ "data": { "started": true } }`                                                                                   |
| GET    | `/api/hub/users`                       | yes        | all hub users (display names)    | `{ "data": [ { "id", "displayName" } ] }`                                                                           |
| GET    | `/api/hub/tracks/{id}`                 | yes        | presence: who / why / where      | `{ "data": { "track": {…}, "presence": [ { "userId", "source", "playlistId", "playlistName" } ] } }`                |
| GET    | `/api/hub/overlap`                     | yes        | shared tracks + pairwise counts  | `{ "data": { "shared": [ … ], "pairs": [ … ] } }`                                                                   |
| POST   | `/api/hub/query`                       | yes + flag | read-only SQL console            | `{ "data": { "columns": […], "rows": [ … ] } }`                                                                     |

`{service}` ∈ `spotify` (v1). Other services → `501 Not Implemented`.

---

## 4. Config keys (env)

| Key                       | Default         | Meaning                                              |
| ------------------------- | --------------- | ---------------------------------------------------- |
| `HUB_HOST`                | `0.0.0.0`       | bind host                                            |
| `HUB_PORT`                | `8080`          | bind port                                            |
| `HUB_PUBLIC_URL`          | —               | public base URL (for redirects/cookies)              |
| `HUB_DATABASE_URL`        | `sqlite:hub.db` | sqlx URL                                             |
| `HUB_SQL_CONSOLE_ENABLED` | `false`         | enable `POST /api/hub/query`                         |
| `HUB_SESSION_TTL_SECS`    | `2592000` (30d) | session lifetime                                     |
| `HUB_STATE_SECRET`        | random per boot | HMAC key signing OAuth `state` (OIDC + Spotify link) |
| `OIDC_ISSUER`             | —               | Pocket ID issuer URL                                 |
| `OIDC_CLIENT_ID`          | —               | OIDC client id                                       |
| `OIDC_CLIENT_SECRET`      | —               | OIDC client secret                                   |
| `OIDC_REDIRECT_URI`       | —               | must match Pocket ID registration                    |
| `SPOTIFY_CLIENT_ID`       | —               | one shared Spotify app                               |
| `SPOTIFY_CLIENT_SECRET`   | —               | ditto                                                |
| `SPOTIFY_REDIRECT_URI`    | —               | must be HTTPS (see deployment doc)                   |

---

## 5. Schema contract (names + required columns frozen)

Tables:

- `hub_users` — `id PK`, `oidc_subject UNIQUE NOT NULL`, `display_name`, `email`, `created_at`
- `hub_sessions` — `id TEXT PK`, `user_id FK hub_users`, `created_at`, `last_seen_at`, `expires_at`
- `hub_service_accounts` — `id PK`, `user_id FK`, `service CHECK IN ('spotify','soundcloud','youtube')`, `remote_user_id`, `display_name`, `access_token`, `refresh_token`, `token_expiry`, `scopes`, `connected_at`, `updated_at`, `UNIQUE(user_id, service)`
- `hub_tracks` (global identity) — `id PK`, `service NOT NULL`, `service_track_id NOT NULL`, `isrc`, `title`, `artists`, `album`, `duration_ms`, `explicit`, `image_url`, `first_seen_at`, `UNIQUE(service, service_track_id)`
- `hub_playlists` (per user) — `id PK`, `user_id FK`, `service`, `playlist_id`, `name`, `description`, `is_liked`, `track_count`, `snapshot_id`, `fetched_at`, `UNIQUE(user_id, service, playlist_id)`
- `hub_playlist_tracks` — `playlist_id FK`, `track_id FK`, `position`, `added_at`, `PK(playlist_id, track_id)` — deliberate v1: duplicate occurrences of one track in a playlist collapse to a single row (irrelevant for overlap queries; widen later with a surrogate PK if occurrence multiplicity is ever needed)
- `hub_liked_tracks` — `user_id FK`, `track_id FK`, `liked_at`, `PK(user_id, track_id)`

Views (the point of v1):

- `hub_v_track_presence` — `track_id, user_id, source ('liked'|'playlist'), playlist_id, playlist_name`
- `hub_v_shared_tracks` — `track_id, user_count, user_ids` (≥ 2 distinct users)
- `hub_v_user_overlap` — `user_a_id, user_b_id, shared_tracks`
- `hub_v_track_playlists` — `track_id, user_id, playlist_id, playlist_name`

Timestamps are **ISO-8601 `TEXT`** throughout (Spotify already returns ISO-8601 for
`added_at`; more readable for the SQL-exploration goal than epoch ints).

Column _additions_ are allowed by the owning doc; **renames/removals** need a
flag back to the coordinator.

---

## 6. Crate layout (target)

```
mmm-hub/
├── Cargo.toml
├── README.md
├── migrations/
│   ├── 001_hub_schema.sql
│   ├── 002_hub_sessions.sql
│   └── 003_*.sql
├── src/
│   ├── main.rs          # CLI (serve)
│   ├── config.rs        # env config
│   ├── db/
│   │   ├── mod.rs       # pool + migrate
│   │   └── testing.rs   # seed fixtures
│   ├── auth/            # OIDC + sessions + middleware
│   ├── ingest/          # spotify.rs
│   └── api/             # health, me, services, read, query
└── tests/               # integration tests
```

---

## 7. Deliverable docs (disjoint write sets — do NOT touch other files)

| File                               | Owned by                |
| ---------------------------------- | ----------------------- |
| `plans/mmm-hub/00-interface.md`    | coordinator (this file) |
| `plans/mmm-hub/data-and-ingest.md` | agent A                 |
| `plans/mmm-hub/auth.md`            | agent B                 |
| `plans/mmm-hub/deployment.md`      | agent C                 |
| `plans/mmm-hub/issues.md`          | agent D                 |
| `plans/mmm-hub/verification.md`    | agent E                 |

The umbrella plan `plans/proposed/mmm-hub.md` and `plans/README.md` are owned by
the coordinator and assembled after the agents report.

**No agent runs `git add` / `git commit` / branch operations.**
Every markdown file must end with normal prose — **never** end a file with a
stray triple-backtick fence; keep all code fences balanced.
