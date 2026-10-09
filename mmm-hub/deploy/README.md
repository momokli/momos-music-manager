# Deploying mmm-hub (walking skeleton)

Live layout:

```
Browser ──https──▶ caddy-caddy-1 (Docker on lan, 192.168.178.149)
                    │  DNS-01 TLS via Cloudflare; basicauth
                    └──reverse_proxy──▶ 192.168.178.200:8080
                                          mmm-hub (systemd, music-catalog)
                                            SQLite: /home/momo/mmm-hub/hub.db
```

- **Domain**: `hub.zukkafabrik.de` → home IP `80.131.52.54` (explicit A record).
  Managed via `~/home_domains.txt` on `lan` + `update_dns.sh`
  (`cloudflare-ddns.service`, `User=momo`, reads `$HOME/home_domains.txt`).
- **Auth**: HTTP basic, same password as `music.klimk.es`.

## Build on .200

```bash
# from a machine that can reach .200 (e.g. via the lan host)
tar czf - -C mmm-hub --exclude target --exclude 'hub.db*' --exclude .env . \
  | ssh -J lan 192.168.178.200 'rm -rf ~/mmm-hub && mkdir -p ~/mmm-hub && tar xzf - -C ~/mmm-hub'
ssh -J lan 192.168.178.200 'cd ~/mmm-hub && ~/.cargo/bin/cargo build --release --locked'
```

## Env (`/home/momo/mmm-hub/.env`)

```
SPOTIFY_CLIENT_ID=...
SPOTIFY_CLIENT_SECRET=...
SPOTIFY_REDIRECT_URI=http://127.0.0.1:8888/callback
HUB_HOST=0.0.0.0
HUB_PORT=8080
HUB_DATABASE_URL=sqlite:/home/momo/mmm-hub/hub.db
```

The hub binds `0.0.0.0:8080` so Caddy on `.149` can reach it; `.200` is not
routable from outside the LAN, so this is effectively LAN-only.

## systemd

```bash
sudo cp mmm-hub/deploy/mmm-hub.service /etc/systemd/system/
sudo systemctl daemon-reload && sudo systemctl enable --now mmm-hub
```

## Spotify link (per user, in the browser)

Users sign up in the web UI and click **Spotify verbinden**. This uses the
public HTTPS redirect, so no port-forwarding is needed:

- `.env`: `SPOTIFY_REDIRECT_URI=https://hub.zukkafabrik.de/api/hub/services/spotify/callback`
- Register that exact URI in the Spotify app's Redirect URIs.

After connecting, pull the data (CLI, from `~/mmm-hub`):

```bash
./target/release/mmm-hub ingest --user <slug>
```

## Caddy

See `Caddyfile.snippet`. Always `caddy validate` before `caddy reload`.

## Rollback

- Caddy: `~/caddy/Caddyfile.bak-prehub` on `lan`.
- DNS list: `~/home_domains.txt.bak-prehub` on `lan` (then re-run `update_dns.sh`).
- Service: `sudo systemctl disable --now mmm-hub`.
