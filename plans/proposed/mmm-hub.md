# Plan: mmm-hub

**Status**: proposed
**Branch**: `feat/mmm-hub`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: yes — new, hub-owned migration chain in `mmm-hub/migrations/` (independent of the client chain in `migrations/`)
**Parent epic**: (to be created) `epic: mmm-hub`
**Methodology**: milestone = one iteration = one release; only leaf issues are worked (see the milestone checklists below).

### Description

A central, multi-user **ingest + exploration service** ("MMM Hub") that lives
next to the existing single-user client, not inside it. Users log in with a
generic account (OIDC), link their streaming accounts (Spotify for v1), and the
hub pulls playlists / tracks / likes into **one shared database** where each row
carries a `user_id`. The point of v1 is **not** a UI — it is a queryable data
basis: raw SQL and a few read endpoints over views that answer questions like
"who has this track, who liked it, and in which playlists does it sit".

This is deliberately decoupled from the client core (per ADR-014 the client is
single-user). The hub shares the _tech stack_ (axum + sqlx + rspotify), not the
schema, not the query layer, not the binary.

> **Detailed design** lives in `plans/mmm-hub/`: `00-interface.md` (frozen
> contract + verified external constraints), `data-and-ingest.md`, `auth.md`,
> `deployment.md`, `issues.md` (the leaf-issue backlog), `verification.md`
> (external-fact sources).
>
> ⚠️ **Verified constraints (2026-10-06) re-anchor the premise** — see
> `00-interface.md` §1a. In short: Spotify dev mode caps one app at **5 users**,
> **followed** playlists expose **no track items**, and refresh tokens die after
> **6 months**. This directly affects the open decisions D6/D7 below.

### Why a separate crate, not a feature of the client

- The client's schema has no user dimension and its `service_config` is a
  singleton connection. Bolting `user_id` onto 23 tables + 12 views is a deep,
  risky rewrite of working code.
- The hub needs a _different_ shape: global track identity shared across users,
  per-user service tokens, and overlap views. A lean schema is cheaper and
  clearer than a retrofitted one.
- Precedent already exists in this repo: `music-api/` is an independent crate
  with its own `Cargo.toml` and is not part of the root build. `mmm-hub/`
  follows the same pattern, so the root `cargo build` / CI stays untouched.

### Target host

Runs on the music server **`192.168.178.200`** (LAN). Own SQLite file
(`hub.db`), own port (default `8080`, configurable). The client keeps port 3000.

---

### Data model (hub-owned, `hub_` prefix)

Identity is global; provenance is per user. That split is what makes overlap
queries trivial.

**Authoritative schema:** the full DDL (columns, FKs, indexes, ISO-8601
timestamps) is in `plans/mmm-hub/data-and-ingest.md` §2; the contract's column
list is `00-interface.md` §5. Shape in one line:

- **Global identity** — `hub_tracks`, unique on `(service, service_track_id)`
  (ISRC is a nullable attribute, never a key).
- **Per-user provenance** — `hub_playlists` + `hub_playlist_tracks` (membership)
  and `hub_liked_tracks` (likes as their own relation, not a synthetic playlist).
- **Accounts** — `hub_users`, `hub_service_accounts` (per-user tokens,
  `reconnect_required`), `hub_sessions`.

**Tags = playlist names.** The client's model is "tag ⇔ playlist via
case-insensitive name matching". The hub inherits that idea for free: a user's
tags _are_ their playlist names, already stored in `hub_playlists.name`, joined
to tracks through `hub_playlist_tracks`. No separate tag table is needed in v1;
a materialized `hub_tags` can come later if name-matching queries get hot.

**Overlap views** (the whole point of v1):

```sql
-- every (track, user, why) fact
hub_v_track_presence       -- track_id, user_id, source ('liked' | 'playlist'), playlist_id
-- tracks liked or playlisted by >= 2 distinct users
hub_v_shared_tracks        -- track_id, user_count, user_ids
-- pairwise overlap counts between users
hub_v_user_overlap         -- user_a, user_b, shared_tracks
-- track -> (user, playlist) for "who has this where"
hub_v_track_playlists      -- track_id, user_id, playlist_id, playlist_name
```

### Auth — two layers

1. **Account login = OIDC (Pocket ID).** Authorization-code + PKCE via the
   `openidconnect` crate with discovery from
   `{issuer}/.well-known/openid-configuration`. On callback we upsert
   `hub_users` by `oidc_subject` and start a **server-side session**
   (`hub_sessions` table + HTTP-only cookie). No passwords in the hub.
2. **Service linking = per-user OAuth.** The _Spotify app_ (one client id /
   secret in hub config) is shared; each user authorizes against it. The OAuth
   `state` is a signed, single-use HMAC token binding the flow to the logged-in
   `user_id` (details in `auth.md`). Tokens land in `hub_service_accounts`.

Spotify scopes: `playlist-read-private`, `playlist-read-collaborative`,
`user-library-read`, `user-read-private` (`GET /me` no longer returns `email`).

### Spotify ingest (`mmm-hub/src/ingest/spotify.rs`)

**Authoritative algorithm:** `plans/mmm-hub/data-and-ingest.md` §4. Points that
differ from the pre-2026 API:

- **Likes** via `GET /me/tracks` → `hub_liked_tracks`.
- **Owned/collaborative playlists** via `GET /playlists/{id}/items` (`limit ≤ 50`);
  **followed** playlists are stored **metadata-only** (no track rows).
- **No batch fetch** (`GET /tracks?ids=` is gone) — the playlist-items response
  embeds full track objects.
- **Dedup** on `(service, service_track_id)` only; `snapshot_id` skips unchanged
  playlists; `429` vs `QUOTA_EXCEEDED` handled distinctly.

The hub does not reuse `spotify::sync_worker` (it assumes a single global
connection).

### Read surface (v1)

Read-only JSON over the views, plus a guarded SQL console for exploration:

```
GET  /api/hub/health
GET  /api/hub/me                             -- current session + linked services
GET  /api/hub/users                          -- all hub users (display names)
POST /api/hub/services/{service}/auth        -- returns Spotify authorize URL
GET  /api/hub/services/{service}/callback    -- OAuth redirect target
POST /api/hub/services/{service}/sync        -- kick off ingest for current user
GET  /api/hub/tracks/{id}                    -- presence: who, why, where
GET  /api/hub/overlap                        -- shared tracks / pairwise counts
POST /api/hub/query                          -- read-only SQL (behind auth + flag)
```

`/api/hub/query` opens a **separate read-only connection** (`PRAGMA query_only=1`,
`mode=ro`) and allows `SELECT`/`WITH` only. It exists because v1's goal is
"write SQL ourselves"; it is feature-flagged and can be dropped once real
endpoints cover the common questions.

### Deployment on 192.168.178.200

```
mmm-hub serve --host 0.0.0.0 --port 8080
```

Config (`hub.toml` / env): OIDC issuer + client id/secret + redirect URI,
Spotify app id/secret + redirect URI, `DATABASE_URL=sqlite:hub.db`.

---

### Milestones

Each milestone is one iteration = one release. `hub-v*` is a **separate version
line** (decision D1 below); if we decide to ride the app's release train instead,
rename these to the app's next tags.

| #   | Milestone             | Outcome                                                                                                                                                                          | Version      | Migration |
| --- | --------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------ | --------- |
| M1  | Data model & skeleton | Hub crate builds standalone; shared schema + 4 overlap views proven by integration tests over seed fixtures. No network, no auth.                                                | `hub-v0.1.0` | 001       |
| M2  | Accounts & sessions   | **HTTPS/TLS prerequisite** in place; a user logs in via Pocket ID (OIDC); session cookie; `/api/hub/me`; unauthenticated access blocked.                                         | `hub-v0.2.0` | 002       |
| M3  | Spotify link & ingest | A linked user's **liked songs + owned/collaborative playlists** land in the shared DB; followed = metadata-only; re-sync snapshot-aware; dedup on `(service, service_track_id)`. | `hub-v0.3.0` | 003       |
| M4  | Exploration surface   | Read endpoints + guarded read-only SQL console over the overlap views (deploy moved into M2).                                                                                    | `hub-v0.4.0` | —         |

Everything after M4 (SoundCloud/YouTube, cross-user tag playlist subscribe, UI)
is a **later milestone line (2.x)** and out of scope for this plan's first cut.

The **work order is the leaf checklist in each milestone description**, not this
table. Each iteration is unlocked by its own `Freigabe: ja` line.

---

#### M1 `hub-v0.1.0` — Data model & skeleton

Leaf issues `M1-1` … `M1-6` — the authoritative backlog (titles, scope, write sets,
acceptance criteria, labels, dependencies) lives in `plans/mmm-hub/issues.md`
(§ M1) and is **not duplicated here**, to avoid drift.

---

#### M2 `hub-v0.2.0` — Accounts & sessions (OIDC / Pocket ID)

Leaf issues `M2-1` … `M2-7` (see `plans/mmm-hub/issues.md` § M2). `M2-1` is the
**TLS / HTTPS reverse-proxy prerequisite** (OIDC issuer must be HTTPS); the rest
covers config, migration 002, the PKCE login flow, session middleware, health/me,
and auth tests.

---

#### M3 `hub-v0.3.0` — Spotify link & ingest

Leaf issues `M3-1` … `M3-9` (see `plans/mmm-hub/issues.md` § M3). Key shape:
token storage (`invalid_grant` → `needsReconnect`), the link flow, ingest of
**owned/collaborative** playlist items (`GET /playlists/{id}/items`, `limit ≤ 50`)

- likes (`/me/tracks`), **followed playlists metadata-only**, dedup on
  `(service, service_track_id)`, snapshot re-sync, 429 vs `QUOTA_EXCEEDED`, the sync
  endpoint, the reconnect/re-auth surface, and ingest tests.

---

#### M4 `hub-v0.4.0` — Exploration surface

Leaf issues `M4-1` … `M4-4` (see `plans/mmm-hub/issues.md` § M4): read endpoints
over the views (`/users`, `/tracks/{id}`, `/overlap`), the guarded read-only SQL
console, its tests, and the README/CHANGELOG/ADR close-out. (Deploy moved to M2.)

---

### Planning decisions (open)

- **D1 — Version line.** Own `hub-v*` line starting `v0.1.0` (proposed) vs. riding
  the app's release train (`1.15.0`, …). Own line keeps cadences independent but
  means a second release process + tag namespace.
- **D2 — Focus milestone & runners.** The client's autonomous runners work _the_
  focus milestone. A hub focus milestone needs a rule for how it interleaves with
  the app's current focus — separate `roadmap` epic, explicitly not auto-dispatched
  until picked up?
- **D3 — Deploy topology.** Reverse proxy + domain with real TLS. The deployment
  doc found an existing Caddy (`*.klimk.es`) and proposes `hub.klimk.es`; still to
  confirm: is `.200` the Caddy host, is `8080` free. Blocks M2/M3's redirect-URI
  acceptance (must be HTTPS).
- **D4 — Session mechanism.** `tower-sessions` vs. hand-rolled `hub_sessions`
  table. Lean: table (explicit, fewer deps).
- **D5 — Token at rest.** Store Spotify tokens plaintext in `hub.db` (LAN, friend
  group) vs. encrypt with a hub key. v1 lean: plaintext + file perms, revisit later.
- **D6 — Ingest scope. RESOLVED.** API-conform: **liked songs + owned/collaborative
  playlists**; **followed** playlists are stored as **metadata only** (no track
  ingest). No non-API source in v1.
- **D7 — The 5-user cap. RESOLVED.** **One shared Spotify app**; the owner's app is
  **grandfathered** (pre-2026), so the cap is not binding for this group.
  Grandfathering lifts the _account/user_ cap only — the playlist-items restriction
  and 6-month refresh expiry still apply.

---

### Issue / epic shape on GitHub

- One `epic` issue: **“MMM Hub”** (context only, never dispatched), listing the
  milestones and linking each leaf issue.
- Four milestones (`hub-v0.1.0` … `hub-v0.4.0`); the leaf checklist lives in each
  milestone description; add `Freigabe: ja` only to the iteration being worked.
- Leaf issues labelled: `enhancement`, `area:db` / `area:api` / `area:spotify` as
  appropriate, plus `epic`-parent reference. Exactly one leaf issue per PR
  (`Closes #<n>`), per the repo's PR gate.

### Files to modify

| File                                    | Change                                                                                             |
| --------------------------------------- | -------------------------------------------------------------------------------------------------- |
| `mmm-hub/Cargo.toml`                    | New independent crate (axum, sqlx, rspotify, openidconnect, tower-sessions or hand-rolled session) |
| `mmm-hub/migrations/001_hub_schema.sql` | New hub schema + overlap views                                                                     |
| `mmm-hub/src/**`                        | config, db, auth, ingest, api modules (new)                                                        |
| `mmm-hub/tests/**`                      | Integration tests (views, auth state, ingest mapping)                                              |
| `mmm-hub/README.md`                     | Run + deploy notes for 192.168.178.200                                                             |
| `plans/README.md`                       | Add this plan to the index                                                                         |
| `docs/DECISIONS.md`                     | ADR: multi-user hub as a separate crate, shared DB with user dimension (added at M1)               |
| `.github/workflows/`                    | Informational build/test job for `mmm-hub` (M1)                                                    |

### Acceptance Criteria

- [ ] `mmm-hub/` builds independently (`cargo build` inside it); root `cargo build` unaffected
- [ ] Migration 001 creates all `hub_*` tables + the four overlap views on a fresh DB
- [ ] A seed fixture with 3 users, overlapping playlists and likes produces correct `hub_v_shared_tracks` / `hub_v_user_overlap`
- [ ] Integration tests cover each view with hand-crafted data (exact row counts)
- [ ] OIDC login upserts `hub_users` by subject and issues a session cookie
- [ ] Session middleware rejects unauthenticated access (except health + auth routes)
- [ ] Spotify link stores tokens in `hub_service_accounts` keyed to the session's `user_id`
- [ ] Ingest upserts playlists/tracks/likes; a re-sync with unchanged snapshots does not re-fetch tracks
- [ ] Track dedup: the same Spotify track from two users yields **one** `hub_tracks` row and two provenance rows
- [ ] `GET /api/hub/tracks/{id}` returns all users/playlists for the track
- [ ] `GET /api/hub/overlap` returns shared tracks and pairwise counts
- [ ] `/api/hub/query` rejects non-`SELECT` statements and is read-only
- [ ] `cargo test` passes inside `mmm-hub/`

### Verified constraints (2026-10-06 — they change the premise)

Confirmed against the official Spotify docs (sources in
`plans/mmm-hub/verification.md`):

- **5 users per app, owner needs Premium.** Dev mode allows **5 authenticated
  users** (allow-listed); Extended Quota Mode is **organizations-only (≥250k
  MAU)** since 2025-05-15 → unreachable. A public hub is **not** possible on one
  shared app; the realistic shapes are “≤5 friends” (D7) or one app per user.
- **Followed playlists expose no track items.** `GET /playlists/{id}/items` works
  only for playlists the user **owns or collaborates on**; followed playlists give
  metadata only. Liked songs are unaffected. This narrows the cross-user overlap
  to liked songs + owned/collaborative playlists (D6).
- **HTTPS required early.** The Spotify redirect URI cannot be a bare LAN IP, and
  the OIDC issuer must be HTTPS → TLS must land by **M2/M3**, not M4.
- **Refresh tokens expire after 6 months** → a “needs reconnect” state in
  `/api/hub/me` and an easy re-auth path.
- **`isrc` is nullable/non-unique** → dedup on `(service, service_track_id)`.

Other open items: session storage (`tower-sessions` vs. table — lean table, D4);
the hub does **not** depend on the root crate (only the stack is shared);
cross-user visibility is opt-in by participation.

### Out of scope (v1)

- Audio hosting / file distribution (metadata only — avoids Spotify ToS and
  copyright issues entirely).
- Ingesting **followed** playlists' tracks — Spotify's API does not expose them
  in dev mode (owned/collaborative only). Pending D6.
- SoundCloud / YouTube (client framework not implemented yet).
- Any polished UI — exploration happens via SQL / read endpoints first.
- Tag embeddings, harmonic/BPM analysis, comment writing — those stay in the
  client.
