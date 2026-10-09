# mmm-hub (walking skeleton)

A separate, multi-user **Spotify ingest + exploration** service. Each user authorizes
Spotify once (via a **loopback** redirect — no public HTTPS needed), the hub pulls their
**liked songs** and **owned/collaborative playlists** into **one shared SQLite DB**, and
you query overlaps with SQL or a few read endpoints.

> **Skeleton scope.** No auth yet (LAN-only, no OIDC), no deploy/TLS. This is the
> walking-skeleton slice of the plan in `../plans/proposed/mmm-hub.md` (issues #119–#144).
> Table/view names already match the contract so it converges with M1–M4.

## What it does

- `auth` – one-time Spotify OAuth on `http://127.0.0.1:8888/callback` (the **only**
  `http://` Spotify still allows), stores the refresh token per user.
- `ingest` – pulls likes (`/me/tracks`) and, for **owned/collaborative** playlists,
  their items (`/playlists/{id}/items`). **Followed** playlists are stored as metadata
  only (the post-Feb-2026 API returns no `items` for them).
- `serve` – HTTP read surface + a read-only SQL console.
- `query` – run a read-only SQL against the hub DB from the terminal.

## Setup

```bash
cd mmm-hub
cp .env.example .env      # fill in SPOTIFY_CLIENT_ID / SPOTIFY_CLIENT_SECRET
# Register the redirect URI you use in the Spotify dashboard, e.g. local dev:
#   http://127.0.0.1:8080/api/hub/services/spotify/callback
cargo build
```

`SPOTIFY_*` come from your existing (grandfathered) Spotify app.

## Web UI

Open the hub in a browser (default `http://127.0.0.1:8080`, or `https://…` behind Caddy):

1. **Sign up** with a username + password (own user store in SQLite; no OIDC yet).
2. Click **Spotify verbinden** — it redirects to Spotify and back.
3. The dashboard shows whether Spotify is **verbunden** (and the Spotify display name).

Each user links their own Spotify account; the tokens live per user in
`hub_service_accounts`. The web OAuth uses the **redirect URI from the config**
(HTTPS in production, loopback for local dev) — no port-forwarding needed.

## Usage

```bash
cargo run -- serve                 # web UI + JSON API + SQL console
cargo run -- ingest --user <slug>  # pull likes + owned/collaborative playlists
cargo run -- query "SELECT * FROM hub_v_user_overlap"
cargo run -- users
cargo run -- seed-demo             # deterministic demo data (no Spotify)
```

After connecting Spotify in the browser, run `ingest` (CLI for now) to pull the
data, then refresh the web page or hit the JSON endpoints.

HTTP endpoints (no auth, LAN-only):

```
GET  /api/hub/health
GET  /api/hub/users
GET  /api/hub/playlists
GET  /api/hub/overlap
GET  /api/hub/tracks/{id}
POST /api/hub/query     {"sql":"SELECT ..."}
```

## Data model

- `hub_tracks` – global track identity, unique on `(service, service_track_id)` (ISRC is
  a nullable attribute, never a key).
- `hub_playlists` / `hub_playlist_tracks` – per-user playlists + membership
  (`items_available = 0` marks a followed, metadata-only playlist).
- `hub_liked_tracks` – likes as their own relation.
- Views: `hub_v_track_presence`, `hub_v_track_playlists`, `hub_v_shared_tracks`,
  `hub_v_user_overlap`.

## Notes / known limits

- **Refresh tokens expire after 6 months** (Spotify, from the original authorization).
  If ingest fails with `invalid_grant`, re-run `auth`.
- **No follow-playlist tracks** – a Spotify API restriction, not a hub limitation.
- Skeleton deviations from the contract: `hub_users.slug` (OIDC subject comes in M2),
  `hub_playlists.items_available`, `hub_service_accounts.authorized_at`.
- `GET /me/items`-style batch endpoints are gone; ingest relies on the track objects
  embedded in playlist-items responses.
