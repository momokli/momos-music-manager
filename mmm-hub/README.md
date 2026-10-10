# mmm-hub

A separate, multi-user **Spotify ingest + exploration** service. Each DJ gets a
local account (username + password, no OIDC), links their own Spotify, and the hub
pulls their **liked songs** and **owned/collaborative playlists** into **one shared
SQLite DB** — so you can ask "who has this track, who liked it, and in which
playlists does it sit".

It is the multi-tenant little brother of the single-user Momo's Music Manager:
same tech stack (axum + sqlx + SQLite), but its own crate, its own schema, its own DB.

> **Working on the hub? Read [`AGENT.md`](AGENT.md) first** — deploy loop, SSH/hosts,
> env, full schema (migrations 001–024), feature map, engine config, and gotchas.

## What it does

- `auth` – one-time Spotify OAuth via a **loopback** redirect
  (`http://127.0.0.1:8888/callback`) for a single user; stores the refresh token
  per user in `hub_service_accounts`.
- `ingest` – pulls likes (`/me/tracks`) and, for **owned/collaborative** playlists,
  their items (`/playlists/{id}/items`). **Followed** playlists are stored as
  metadata only (the post-Feb-2026 API returns no `items` for them).
- `serve` – web UI + JSON API + read-only SQL console, and spawns the background
  sync worker.
- `query` – run read-only SQL against the hub DB from the terminal.

## Accounts & sessions

Local accounts, **not OIDC/Pocket ID**. Users sign up with a username + password;
the password is bcrypt-hashed into `hub_users.password_hash` and usernames are
case-insensitive (`COLLATE NOCASE`). Login creates a server-side session row in
`hub_web_sessions` and sets the `hub_session` cookie (HttpOnly, `SameSite=Lax`,
30-day TTL).

## Spotify link (per user)

Each user links their own Spotify account; tokens live per user in
`hub_service_accounts`. The **web** flow uses the public HTTPS redirect
`https://hub.zukkafabrik.de/api/hub/services/spotify/callback` (register that exact
URI in the Spotify app), so no port-forwarding is needed. `auth` uses the loopback
redirect instead, for a one-time local token grab.

## Background sync worker

`src/worker.rs` runs continuously while there is work:

- **likes** for connected accounts whose `likes_status` is `queued`/null, and
- **playlist items** for playlists with `enabled_for_fetch = 1 AND items_available = 0`.

It processes one request stream at a time, is **staggered** (a small per-page
delay, a capped number of playlists/accounts per pass) and **429/QUOTA_EXCEEDED
aware** (hard backoff, quota backoff). It is triggered by the UI toggles
(Fetch / enable-all), `mmm-hub backfill`, or `POST /api/hub/services/{service}/sync`.

## Setup

```bash
cd mmm-hub
cp .env.example .env      # SPOTIFY_CLIENT_ID / SPOTIFY_CLIENT_SECRET, ...
cargo build
```

## Usage

```bash
cargo run -- serve                                    # web UI + JSON API + SQL console + worker
cargo run -- auth --user <slug>                       # one-time loopback Spotify link
cargo run -- ingest --user <slug>                     # pull likes + owned/collaborative playlists
cargo run -- fetch-playlists --user <slug>            # refresh playlist metadata (owned + followed)
cargo run -- set-password --user <slug> --password X  # set/reset a local account password
cargo run -- backfill                                 # queue a full initial load for all users
cargo run -- query "SELECT * FROM hub_v_user_overlap"
cargo run -- users
cargo run -- seed-demo                                # deterministic demo data (no Spotify)
```

## Web UI

Every page is session-gated and rendered through one shell (`templates/base.html`:
topbar nav + user menu). The dashboard is `/`.

| Route               | What it shows                                                               |
| ------------------- | --------------------------------------------------------------------------- |
| `/`                 | Overview: Spotify status, quick actions, stats, recent tracks               |
| `/me/playlists`     | Your playlists with fetch toggles + server-side filter (htmx row swap)      |
| `/overlap`          | Entdecken: shared tracks + pairwise overlaps                                |
| `/search`           | Track search across the shared catalog                                      |
| `/track/{id}`       | Per-user playlist presence + per-ISRC availability via internal `music-api` |
| `/user/{slug}`      | A user's likes + playlists                                                  |
| `/playlist/{id}`    | One playlist's tracks                                                       |
| `/settings`         | Account + Spotify connect/sync/disconnect                                   |
| `/sql`              | Read-only SQL console with presets                                          |
| `/login`, `/signup` | Minimal auth shell                                                          |

## JSON API

```
GET  /api/hub/health
GET  /api/hub/users
GET  /api/hub/me                       # session user + linked services (incl. needsReconnect)
GET  /api/hub/tracks/{id}
GET  /api/hub/overlap
GET  /api/hub/playlists
POST /api/hub/query                    {"sql":"SELECT ..."}   # session-gated
POST /api/hub/services/{service}/sync  # queue a fresh sync for the current user
```

## Data model

Migrations **`001`–`024`** — the live schema is the source of truth
(`sqlite3 hub.db .schema`). See [`AGENT.md`](AGENT.md) §5 for the full table/view map.
The early migrations include:

- `hub_users` (handle `slug`, bcrypt `password_hash`), `hub_service_accounts`
  (per-user tokens), `hub_web_sessions` (server-side sessions).
- `hub_tracks` – global track identity, unique on `(service, service_track_id)`
  (ISRC is a nullable attribute, never a key); `hub_track_external_ids` for
  per-service IDs/URLs.
- `hub_playlists` / `hub_playlist_tracks` – per-user playlists + membership
  (`items_available = 0` marks a followed, metadata-only playlist);
  `hub_liked_tracks` – likes as their own relation.
- Views: `hub_v_track_presence`, `hub_v_track_playlists`, `hub_v_shared_tracks`,
  `hub_v_user_overlap`.

## Deployment

Runs on the LAN host **`music-catalog` / `192.168.178.200`**, behind Caddy on `lan`
at **`https://hub.zukkafabrik.de`**. Sibling read-only tools serve the same data
from a **sanitized copy** `hub-public.db`: Datasette at `data.zukkafabrik.de` and
SchemaSpy at `schema.zukkafabrik.de`. See `deploy/`.

## Notes / known limits

- **Refresh tokens expire after 6 months** (Spotify, from the original
  authorization). `/api/hub/me` surfaces `needsReconnect`; re-run `auth` (or
  reconnect in the UI).
- **No follow-playlist tracks** – a Spotify API restriction, not a hub limitation.
- `GET /me/items`-style batch endpoints are gone; ingest relies on the track objects
  embedded in playlist-items responses.
