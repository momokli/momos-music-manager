# Deploying music-hub

Live layout:

```
Browser ──https──▶ caddy-caddy-1 (Docker on lan, 192.168.178.149)
                    │  DNS-01 TLS via Cloudflare
                    └──reverse_proxy──▶ 192.168.178.200:8080
                                          music-hub (systemd, music-catalog)
                                            SQLite: /home/momo/music-hub/hub.db
```

- **Domain**: `hub.zukkafabrik.de` → home IP `80.131.52.54` (explicit A record).
  Managed via `~/home_domains.txt` on `lan` + `update_dns.sh`
  (`cloudflare-ddns.service`, `User=momo`, reads `$HOME/home_domains.txt`).
- **Auth**: the hub has its **own login** (own user store + server-side session
  cookie). No HTTP basic auth in front of it. The read-only sibling sites
  (`data.`/`schema.`) _do_ use basic auth.

## Build on .200

```bash
# from a machine that can reach .200 (e.g. via the lan host)
tar czf - -C music-hub --exclude target --exclude 'hub.db*' --exclude .env . \
  | ssh -J lan 192.168.178.200 'mkdir -p ~/music-hub && tar xzf - -C ~/music-hub && find ~/music-hub -name "._*" -delete'
ssh -J lan 192.168.178.200 'cd ~/music-hub && ~/.cargo/bin/cargo build --release --locked && sudo systemctl restart music-hub'
```

## Env (`/home/momo/music-hub/.env`)

```
SPOTIFY_CLIENT_ID=...
SPOTIFY_CLIENT_SECRET=...
SPOTIFY_REDIRECT_URI=https://hub.zukkafabrik.de/api/hub/services/spotify/callback
HUB_HOST=0.0.0.0
HUB_PORT=8080
HUB_DATABASE_URL=sqlite:/home/momo/music-hub/hub.db
MUSIC_API_BASE=http://127.0.0.1:8710
MUSIC_API_TOKEN=...
```

The hub binds `0.0.0.0:8080` so Caddy on `.149` can reach it; `.200` is not
routable from outside the LAN, so this is effectively LAN-only.

## systemd

```bash
sudo cp music-hub/deploy/music-hub.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now music-hub
```

## Spotify link (per user, in the browser)

Users sign up in the web UI and click **Spotify verbinden**. This uses the
public HTTPS redirect, so no port-forwarding is needed:

- `.env`: `SPOTIFY_REDIRECT_URI=https://hub.zukkafabrik.de/api/hub/services/spotify/callback`
- Register that exact URI in the Spotify app's Redirect URIs.

After connecting, the background worker pulls likes + playlist items
(staggered, 429-aware). To queue a full initial load:

```bash
./target/release/music-hub backfill          # all users: enable playlists + re-queue likes
# or, per user, the dashboard toggles / POST /api/hub/services/spotify/sync
```

The loopback redirect (`http://127.0.0.1:8888/callback`) is still supported for
local/CLI token grabs via `music-hub auth --user <slug>`.

## Caddy

See `Caddyfile.snippet`. Always `caddy validate` before `caddy reload`.

## Read-only public copy (Datasette / SchemaSpy)

Two read-only sites expose a **sanitized copy** of the DB — never `hub.db`:

- `data.zukkafabrik.de` — Datasette (`datasette.service`, port 8001), basic auth.
- `schema.zukkafabrik.de` — SchemaSpy ER diagrams (`schemaspy-site.service`, port 8002), basic auth.

`make-public.sql` + `make-public-db.sh` build `/home/momo/music-hub/hub-public.db`
without `password_hash`, tokens, or sessions, then restart Datasette. Refresh it
automatically with a systemd timer (`make-public-db.timer`, every 10 min):

```bash
bash ~/music-hub/make-public-db.sh
systemctl status make-public-db.timer
```

SchemaSpy is regenerated on demand:

```bash
docker run --rm -v /home/momo/music-hub:/db -v /home/momo/music-hub/schemaspy:/out \
  schemaspy/schemaspy:latest -t sqlite-xerial -db /db/hub.db \
  -dp /usr/local/lib/sqlite-jdbc.jar -cat % -s main -sso -o /out
```

## Rollback

- Caddy: `~/caddy/Caddyfile.bak-prehub` on `lan`.
- DNS list: `~/home_domains.txt.bak-prehub` on `lan` (then re-run `update_dns.sh`).
- Service: `sudo systemctl disable --now music-hub`.
