# MMM Hub — Deployment (LAN host `192.168.178.200`)

> **Owner**: agent C · **Status**: draft for review · **Last updated**: 2026-10-06
>
> Written against the frozen contract in [`00-interface.md`](00-interface.md). Where
> this document needs a value the contract does not freeze (domain name, backup
> target, the exact host that terminates TLS), it is marked **[ASSUMPTION]** and
> repeated in [Open questions](#9-open-questions--assumptions). Env keys, the port
> (`8080`), the binary (`mmm-hub serve`), the DB filename (`hub.db`) and both
> OAuth callback **paths** are taken verbatim from the contract and must not drift.

This document is the ops runbook for running the Hub as a long-lived systemd
service on the LAN music server. It mirrors the conventions already used by the
single-user client in [`deploy/`](../../deploy/) (user `momo`, Caddy reverse
proxy, journald logging) but is a **separate unit, separate DB, separate port**.

### Milestone ordering — TLS/reverse proxy is a prerequisite of M2, not part of M4

The contract's milestone table originally delivered HTTPS only in **M4**. The
2026-10-06 verification pass (`verification.md`, facts 2 and 6) moved that
requirement **earlier**, because the Hub cannot complete its OAuth flows without
the proxy/TLS in place:

- **OIDC (M2)** — the `openidconnect` crate's `IssuerUrl` is **https-only**, so
  Pocket ID must be reachable over HTTPS (directly or behind a TLS terminator)
  before any login code can run. An `http://` issuer is rejected outright.
- **Spotify (M3)** — Spotify only accepts `http://` redirect URIs that use a
  **loopback IP literal**; a bare LAN IP such as `http://192.168.178.200/...` is
  **rejected** (and `localhost` is not accepted either). The callback therefore
  must be HTTPS before the link/ingest flow works.

So the TLS + reverse-proxy work in [§4](#4-https--reverse-proxy) is a
**prerequisite of M2**, and the Spotify redirect registration a **prerequisite of
M3** — not a late M4 deployment detail. The rest of this document (systemd unit,
DB, backups, firewall, runbook) is what it takes to run the long-lived service
and can be stood up as soon as the service first needs to run (local-only for M1
dev, the deployed host for M2).

| Piece                                                                                                                                    | First milestone that consumes it      | Why                                                                  |
| ---------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------- | -------------------------------------------------------------------- |
| Caddy entry + trusted TLS cert for the hub host ([§4.3](#43-caddy-config))                                                               | **M2**                                | HTTPS origin for OIDC redirect/callback and `Secure` session cookies |
| HTTPS-reachable Pocket ID + `OIDC_ISSUER`/`OIDC_REDIRECT_URI` + Pocket ID client (§2, [§4.4](#44-redirect-uris-that-must-match-exactly)) | **M2**                                | OIDC login                                                           |
| `SPOTIFY_REDIRECT_URI` + Spotify dashboard registration (§2, §4.4)                                                                       | **M3**                                | Spotify link & ingest                                                |
| systemd unit, env file, DB, backups, firewall (§1, §2, §3, §5)                                                                           | **M2** (deployed) — local dev earlier | Run the service                                                      |
| Read endpoints + guarded SQL console flag (§5.1, §9)                                                                                     | **M4**                                | Exploration milestone                                                |

M1 (schema skeleton + integration tests) needs **none** of this — it has no
network and no auth.

---

## 0. Target layout (what we are building)

| Concern    | Value                                                       |
| ---------- | ----------------------------------------------------------- |
| Host       | `192.168.178.200` (LAN)                                     |
| Binary     | `/usr/local/bin/mmm-hub`                                    |
| Service    | `mmm-hub.service` (systemd, user `momo`)                    |
| Bind       | `127.0.0.1:8080` (loopback; Caddy terminates TLS in front)  |
| Public URL | `https://hub.klimk.es` **[ASSUMPTION]**                     |
| DB         | `/var/lib/mmm-hub/hub.db` (SQLite, WAL)                     |
| Env file   | `/etc/mmm-hub/hub.env` (mode `0640 root:momo`)              |
| Backups    | `/var/backups/mmm-hub/hub-<timestamp>.db` via systemd timer |
| Logs       | journald (`journalctl -u mmm-hub`)                          |

```
browser (LAN / Tailscale)
        │  https://hub.klimk.es
        ▼
   Caddy (Docker, :80/:443)          ← TLS termination, *.klimk.es via Cloudflare DNS
        │  reverse_proxy 127.0.0.1:8080
        ▼
   mmm-hub.service  (systemd, user momo)
        ├─ /usr/local/bin/mmm-hub serve --host 127.0.0.1 --port 8080
        └─ /var/lib/mmm-hub/hub.db  (WAL)
```

The Hub is **loopback-only**; nothing binds to the LAN interface directly. This
is deliberate — see [§5 Firewall / ports](#5-firewall--ports--logs).

---

## 1. systemd unit — `mmm-hub.service`

Install to `/etc/systemd/system/mmm-hub.service`, owned `root:root`, mode `0644`.

```systemd
[Unit]
Description=MMM Hub — multi-user ingest + exploration service
Documentation=file:/home/momo/momos-music-manager/plans/mmm-hub/deployment.md
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=momo
Group=momo
WorkingDirectory=/var/lib/mmm-hub

# ── Environment ──────────────────────────────────────────────────────────
# All HUB_/OIDC_/SPOTIFY_ keys live in this file (see §2). It contains
# secrets, so it is root-owned and mode 0640.
EnvironmentFile=/etc/mmm-hub/hub.env
Environment="RUST_LOG=info"

# ── Startup ──────────────────────────────────────────────────────────────
# The binary opens/creates hub.db and runs its own additive migrations on
# boot. Ensure the data dir exists first (idempotent).
ExecStartPre=+/usr/bin/install -d -o momo -g momo -m 0750 /var/lib/mmm-hub

ExecStart=/usr/local/bin/mmm-hub serve --host 127.0.0.1 --port 8080

Restart=always
RestartSec=10
TimeoutStartSec=120
TimeoutStopSec=30

# ── Hardening ────────────────────────────────────────────────────────────
NoNewPrivileges=yes
ProtectSystem=strict
ProtectHome=yes
PrivateTmp=yes
PrivateDevices=yes
ProtectKernelTunables=yes
ProtectKernelModules=yes
ProtectKernelLogs=yes
ProtectControlGroups=yes
ProtectClock=yes
ProtectHostname=yes
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictNamespaces=yes
RestrictRealtime=yes
RestrictSUIDSGID=yes
LockPersonality=yes
MemoryDenyWriteExecute=yes
SystemCallArchitectures=native
SystemCallFilter=@system-service
SystemCallErrorNumber=EPERM

# The ONLY writable path the service needs (DB + WAL + SHM live here).
ReadWritePaths=/var/lib/mmm-hub

# ── Resource limits ──────────────────────────────────────────────────────
MemoryMax=1G
TasksMax=256

[Install]
WantedBy=multi-user.target
```

Notes on the hardening choices:

- `ProtectSystem=strict` makes the whole filesystem read-only except
  `ReadWritePaths`; the DB directory is the only writable location. This is why
  the binary lives in `/usr/local/bin` and **not** under `/home/momo` — with
  `ProtectHome=yes` a binary in the home directory would be unreadable.
- `MemoryDenyWriteExecute=yes` is safe for a normal Rust/axum binary (no JIT).
  If a future dependency needs W+X pages, drop this line and note why.
- `SystemCallFilter=@system-service` is the standard allow-list; if the Hub
  later shells out (it should not in v1), relax it deliberately.
- The `+` prefix on `ExecStartPre` runs that one command as root (to create the
  data dir with the right owner); everything else runs as `momo`.

Reload and enable:

```bash
sudo install -m 0644 mmm-hub.service /etc/systemd/system/mmm-hub.service
sudo systemctl daemon-reload
sudo systemctl enable --now mmm-hub
```

---

## 2. Config file — `/etc/mmm-hub/hub.env`

Create the directory and file as root; the file holds secrets and must not be
world-readable.

```bash
sudo install -d -o root -g momo -m 0750 /etc/mmm-hub
sudo install -o root -g momo -m 0640 /dev/null /etc/mmm-hub/hub.env
sudoedit /etc/mmm-hub/hub.env
```

Every key below is taken from contract §4. Values are placeholders — replace
them. Lines starting with `#` are comments; systemd's `EnvironmentFile` accepts
`KEY=value` with no `export` and no shell expansion.

```ini
# /etc/mmm-hub/hub.env — MMM Hub runtime config (root:momo, 0640)
# Consumed by mmm-hub.service via EnvironmentFile=. Never commit real secrets.

# ── Bind ─────────────────────────────────────────────────────────────────
# Loopback only: Caddy (Docker) proxies to this. Do NOT use 0.0.0.0 unless
# you intentionally want the Hub reachable unencrypted on the LAN.
HUB_HOST=127.0.0.1
HUB_PORT=8080

# ── Public identity ──────────────────────────────────────────────────────
# Public base URL used for redirects + cookie scope. Must be the HTTPS URL
# users actually type (see §4). No trailing slash.
HUB_PUBLIC_URL=https://hub.klimk.es

# ── Database ─────────────────────────────────────────────────────────────
# Contract default is `sqlite:hub.db` (relative to WorkingDirectory). We pin
# an absolute path so the file location never depends on cwd.
HUB_DATABASE_URL=sqlite:///var/lib/mmm-hub/hub.db

# ── Feature flags ────────────────────────────────────────────────────────
# Read-only SQL console (POST /api/hub/query). Keep false in normal operation;
# flip to true only for a deliberate, temporary debugging session.
HUB_SQL_CONSOLE_ENABLED=false

# Session lifetime in seconds. 2592000 = 30 days (contract default).
HUB_SESSION_TTL_SECS=2592000

# ── OIDC / Pocket ID ─────────────────────────────────────────────────────
# Issuer URL of the Pocket ID instance (must match its `iss` claim exactly).
OIDC_ISSUER=https://id.klimk.es
# Client credentials registered in Pocket ID for the Hub.
OIDC_CLIENT_ID=mmm-hub
OIDC_CLIENT_SECRET=REPLACE_WITH_POCKET_ID_CLIENT_SECRET
# Must byte-for-byte match the redirect URI registered in Pocket ID. The path
# is frozen by the contract (00-interface.md §3: GET /api/hub/auth/callback);
# only the host is an [ASSUMPTION] (see §4.4). Consumed by M2.
OIDC_REDIRECT_URI=https://hub.klimk.es/api/hub/auth/callback

# ── Spotify (one shared app for all Hub users) ───────────────────────────
SPOTIFY_CLIENT_ID=REPLACE_WITH_SPOTIFY_CLIENT_ID
SPOTIFY_CLIENT_SECRET=REPLACE_WITH_SPOTIFY_CLIENT_SECRET
# MUST be HTTPS (Spotify only permits http:// for loopback IP literals; a bare
# LAN IP is rejected). Path is frozen by the contract:
# /api/hub/services/{service}/callback. Consumed by M3.
SPOTIFY_REDIRECT_URI=https://hub.klimk.es/api/hub/services/spotify/callback
```

After editing, `systemctl restart mmm-hub` and confirm the service picked the
file up:

```bash
sudo systemctl show mmm-hub -p EnvironmentFiles
```

---

## 3. SQLite operations

### 3.1 Location and permissions

| Path                          | Owner       | Mode   |
| ----------------------------- | ----------- | ------ |
| `/var/lib/mmm-hub/`           | `momo:momo` | `0750` |
| `/var/lib/mmm-hub/hub.db`     | `momo:momo` | `0640` |
| `/var/lib/mmm-hub/hub.db-wal` | `momo:momo` | `0640` |
| `/var/lib/mmm-hub/hub.db-shm` | `momo:momo` | `0640` |

The `-wal` and `-shm` sidecar files appear once WAL mode is active. The service
creates them; the directory must be writable by `momo` (guaranteed by
`ReadWritePaths=/var/lib/mmm-hub` in the unit).

### 3.2 WAL + busy_timeout

WAL is a **persistent** database property — set it once and it sticks. The
`busy_timeout` is **per connection** and must be set on every connection, so it
belongs in the app's connection setup (or the sqlx URL), not in a one-off
`sqlite3` call.

One-time WAL activation (run as `momo`, with the service stopped, right after
the first boot created `hub.db`):

```bash
sudo -u momo sqlite3 /var/lib/mmm-hub/hub.db \
  "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;"
# → wal
```

Recommended connection settings for the Hub (to be enforced in
`mmm-hub/src/db/mod.rs`; documented here so ops and code agree):

```sql
PRAGMA journal_mode = WAL;      -- persistent, set once
PRAGMA busy_timeout = 5000;     -- per connection, 5 s
PRAGMA synchronous  = NORMAL;   -- safe with WAL, faster than FULL
PRAGMA foreign_keys = ON;       -- enforce hub_* FKs
```

If sqlx is configured via URL, the equivalent is
`sqlite:///var/lib/mmm-hub/hub.db?mode=rwc&busy_timeout=5000` — but prefer
setting these explicitly in `SqliteConnectOptions` so they are not silently
lost. **Flag for agent A**: the DB module must set `busy_timeout` and
`foreign_keys` on every pooled connection.

### 3.3 Backup strategy

Use `VACUUM INTO` for a consistent, defragmented snapshot that is safe to take
while the service is running (it reads a consistent view; it does not block
writers for long). This is preferred over copying the file, which can capture a
torn WAL state.

Create `/usr/local/bin/mmm-hub-backup.sh`:

```bash
#!/usr/bin/env bash
set -euo pipefail

DB=/var/lib/mmm-hub/hub.db
DEST=/var/backups/mmm-hub
STAMP="$(date -u +%Y%m%dT%H%M%SZ)"
OUT="$DEST/hub-$STAMP.db"

install -d -o momo -g momo -m 0750 "$DEST"

# Consistent online snapshot (no service stop required).
sudo -u momo sqlite3 "$DB" "VACUUM INTO '$OUT';"

# Keep the last 14 daily snapshots.
find "$DEST" -maxdepth 1 -name 'hub-*.db' -type f -mtime +14 -delete

echo "backup ok: $OUT"
```

`mmm-hub-backup.service`:

```systemd
[Unit]
Description=MMM Hub — SQLite snapshot backup
After=mmm-hub.service

[Service]
Type=oneshot
User=root
ExecStart=/usr/local/bin/mmm-hub-backup.sh
```

`mmm-hub-backup.timer`:

```systemd
[Unit]
Description=MMM Hub — daily SQLite snapshot backup

[Timer]
OnCalendar=*-*-* 03:30:00
Persistent=true
RandomizedDelaySec=300

[Install]
WantedBy=timers.target
```

```bash
sudo install -m 0755 mmm-hub-backup.sh /usr/local/bin/mmm-hub-backup.sh
sudo install -m 0644 mmm-hub-backup.service /etc/systemd/system/
sudo install -m 0644 mmm-hub-backup.timer /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now mmm-hub-backup.timer
systemctl list-timers mmm-hub-backup.timer
```

Restore procedure (service stopped):

```bash
sudo systemctl stop mmm-hub
sudo -u momo cp /var/backups/mmm-hub/hub-<STAMP>.db /var/lib/mmm-hub/hub.db
sudo -u momo rm -f /var/lib/mmm-hub/hub.db-wal /var/lib/mmm-hub/hub.db-shm
sudo systemctl start mmm-hub
```

The `-wal`/`-shm` removal is required: a stale WAL from the old DB must never be
replayed against a restored file.

---

## 4. HTTPS / reverse proxy

> **Milestone note:** this whole section is a **prerequisite of M2** (OIDC) and
> **M3** (Spotify), _not_ a late M4 step — see
> [Milestone ordering](#milestone-ordering--tlsreverse-proxy-is-a-prerequisite-of-m2-not-part-of-m4)
> at the top of this document. Complete §4 before starting M2.

### 4.0 What M2 needs (OIDC) / What M3 needs (Spotify)

**M2 — accounts & sessions (`hub-v0.2.0`).** Blocked on TLS before any login code
can be exercised:

- A working HTTPS origin for the Hub: the Caddy site in
  [§4.3](#43-caddy-config) with a trusted certificate (no client-side trust
  configuration).
- Pocket ID itself served over HTTPS, with `OIDC_ISSUER` pointing at it — an
  `http://` issuer is rejected by `openidconnect`'s `IssuerUrl`.
- The Pocket ID client registered with redirect URI
  `https://<host>/api/hub/auth/callback`
  ([§4.4](#44-redirect-uris-that-must-match-exactly)).
- A `Secure` session cookie — which requires the browser to reach the Hub over
  HTTPS, so `HUB_PUBLIC_URL` must be the `https://` origin.

**M3 — Spotify link & ingest (`hub-v0.3.0`).** Blocked on TLS before the link
flow works:

- The same HTTPS origin, now with `SPOTIFY_REDIRECT_URI` set to
  `https://<host>/api/hub/services/spotify/callback`.
- That exact URI registered in the Spotify developer dashboard. Spotify rejects
  `http://192.168.178.200/...` (not loopback) and **does not allow `localhost`**;
  only `http://127.0.0.1:PORT` / `http://[::1]:PORT` literals are exempt from TLS.

In both cases Caddy terminates TLS and reverse-proxies to the Hub on
`127.0.0.1:8080`; the Hub process itself never speaks TLS.

### 4.1 Why HTTPS is mandatory (not optional)

Spotify's OAuth policy only allows **`http://` for loopback IP literals**
(`http://127.0.0.1:PORT/...`, `http://[::1]:PORT/...`); **`localhost` is not
accepted**, and every non-loopback host **must be `https://`**. A loopback
redirect is useless for the Hub anyway: it would land on _each user's own
machine_, not on the shared server, so the authorization code would never reach
the Hub. A bare LAN IP such as `http://192.168.178.200/...` is therefore
**rejected**.

A second, independent hard reason is OIDC: the `openidconnect` crate's
`IssuerUrl` is **https-only**, so Pocket ID must be served over HTTPS (or behind
a TLS terminator) before **M2** — an `http://` issuer is rejected outright.

Therefore the Hub must be reachable at a stable **HTTPS** hostname by **M2**, and
both `SPOTIFY_REDIRECT_URI` and `OIDC_REDIRECT_URI` must be HTTPS URLs on that
host. This is the single hard reason the Hub cannot be served as plain
`http://192.168.178.200:8080`.

### 4.2 DNS / domain options for a LAN

Three viable approaches, in order of preference:

1. **Real subdomain with public DNS + DNS-01 TLS (recommended).**
   Reuse the existing `*.klimk.es` setup: Caddy in Docker already auto-provisions
   certificates via the Cloudflare DNS challenge (see
   [`deploy/Caddyfile.snippet`](../../deploy/Caddyfile.snippet)). Add
   `hub.klimk.es` → `192.168.178.200` (A record, or a split-horizon record so it
   resolves to the LAN IP internally). Works from LAN, Tailscale and the
   internet; no client-side trust configuration.
2. **Split-horizon DNS.** Public DNS returns a placeholder; the LAN resolver
   (Pi-hole / router / `dnsmasq`) returns `192.168.178.200` for
   `hub.klimk.es`. Still needs a publicly resolvable name for the ACME
   DNS-01 challenge, so this is usually combined with option 1.
3. **`*.local` / mDNS only (fallback).** `hub.local` works on the LAN but
   cannot get a publicly trusted certificate (no public CA issues for `.local`).
   You would need a private CA installed on every user's device, and Spotify
   still requires the redirect URI to be HTTPS with a host it accepts — a
   `.local` name is fragile here. **Not recommended for the Spotify flow.**

**[ASSUMPTION]** This doc uses `hub.klimk.es`. Confirm the domain with the
coordinator before registering redirect URIs (see open questions).

### 4.3 Caddy config

Add to the server's Caddyfile (`~/caddy/Caddyfile`, the same Caddy instance
that fronts `music.klimk.es`), then reload:

```caddy
hub.klimk.es {
    reverse_proxy 127.0.0.1:8080
    encode gzip

    # Cookies are set by the Hub; make sure Caddy does not strip them.
    header {
        # HSTS is safe once you are certain the domain is HTTPS-only.
        Strict-Transport-Security "max-age=31536000; includeSubDomains"
        -Server
    }
}
```

Reload:

```bash
cd ~/caddy && docker compose restart caddy
# or, if Caddy runs as a named container:
docker exec mellon-caddy caddy reload --config /etc/caddy/Caddyfile
```

Because Caddy runs in Docker, `127.0.0.1:8080` inside the container is **not**
the host loopback. Use one of:

- `reverse_proxy host.docker.internal:8080` (Docker Desktop / with
  `extra_hosts: ["host.docker.internal:host-gateway"]`), or
- the host's LAN IP `reverse_proxy 192.168.178.200:8080` and bind the Hub to
  `HUB_HOST=0.0.0.0` **plus** a firewall rule that only allows the Docker
  bridge / LAN to reach `8080`, or
- run Caddy with `network_mode: host` so `127.0.0.1:8080` is the host loopback.

**[ASSUMPTION]** The existing Caddy is containerised; pick the networking mode
that matches `~/caddy/docker-compose.yml` on the host. If Caddy is _not_ in
Docker, plain `reverse_proxy 127.0.0.1:8080` is correct as written.

### 4.4 Redirect URIs that must match exactly

Both callback **paths** are frozen by the contract (`00-interface.md` §3): the
OIDC login callback is `GET /api/hub/auth/callback` and the service callback is
`GET /api/hub/services/{service}/callback`; with `{service}=spotify` that yields
the two URLs below. They must be **byte-for-byte identical** in three places: the
Hub env file, the provider dashboard, and (implicitly) `HUB_PUBLIC_URL`.

| Provider                    | Milestone | Where to register                   | Exact value                                                                   |
| --------------------------- | --------- | ----------------------------------- | ----------------------------------------------------------------------------- |
| Pocket ID                   | **M2**    | OIDC client → Redirect URIs         | `https://hub.klimk.es/api/hub/auth/callback`                                  |
| Spotify Developer Dashboard | **M3**    | App → Edit Settings → Redirect URIs | `https://hub.klimk.es/api/hub/services/spotify/callback`                      |
| Hub env                     | M2/M3     | `/etc/mmm-hub/hub.env`              | `OIDC_REDIRECT_URI` (M2) / `SPOTIFY_REDIRECT_URI` (M3) = the two values above |

(`hub.klimk.es` is the **[ASSUMPTION]** host — see open questions. Substitute the
confirmed host everywhere; the paths do not change.)

Checklist when wiring OAuth:

- No trailing slash, exact scheme (`https`), exact host, exact path.
- **M2:** register the OIDC redirect in Pocket ID and grant the OIDC client's
  **Access tab** to the users — Pocket ID otherwise refuses the login.
- **M3:** register the Spotify redirect in the developer dashboard; use the
  **single shared Spotify app** (contract §4/D7) — each user links their own
  account through it.
- `HUB_PUBLIC_URL=https://hub.klimk.es` (no trailing slash) so the Hub builds
  redirects and cookie scope from the same origin.
- After changing any redirect URI, restart `mmm-hub` **and** re-save the
  provider dashboard (Spotify caches the list).

---

## 5. Firewall / ports + logs

### 5.1 Port exposure

Required from **M2** onward: once the Hub serves multi-user OIDC/Spotify traffic
it must sit behind Caddy (loopback + TLS), never exposed raw on the LAN. The
unencrypted `0.0.0.0` bind is for short-lived dev/debug only.

Two supported postures:

| Posture                        | `HUB_HOST`  | Firewall                             | When                                                          |
| ------------------------------ | ----------- | ------------------------------------ | ------------------------------------------------------------- |
| **Recommended** — behind proxy | `127.0.0.1` | deny `8080` from LAN; allow `80/443` | Default. TLS is the only way in.                              |
| LAN-exposed (debug only)       | `0.0.0.0`   | allow `8080` from LAN                | Temporary debugging; **no TLS**, cookies/tokens in the clear. |

The contract default is `HUB_HOST=0.0.0.0`, but for a multi-user service holding
OAuth tokens and session cookies, **loopback + Caddy is the correct production
posture**. Keep `0.0.0.0` only for a short-lived debug session.

`ufw` example (loopback posture):

```bash
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp
sudo ufw deny 8080/tcp          # never expose the raw Hub
sudo ufw status numbered
```

If Caddy is containerised and must reach the host on the LAN IP, allow `8080`
**only** from the Docker bridge subnet instead of the whole LAN:

```bash
sudo ufw allow from 172.16.0.0/12 to any port 8080 proto tcp
```

### 5.2 Logs

The Hub logs via `tracing` to stdout/stderr, which systemd captures into
journald. No log files to rotate manually.

```bash
# Follow live
journalctl -u mmm-hub -f

# Last 200 lines
journalctl -u mmm-hub -n 200

# Since a point in time
journalctl -u mmm-hub --since "1 hour ago"

# Errors only
journalctl -u mmm-hub -p err
```

Verbosity is controlled by `RUST_LOG` in the unit (`info` by default). For a
debug session, override with a drop-in rather than editing the unit:

```bash
sudo systemctl edit mmm-hub
# [Service]
# Environment="RUST_LOG=mmm_hub=debug,tower_http=debug,sqlx=warn"
sudo systemctl restart mmm-hub
```

Bound journald disk usage in `/etc/systemd/journald.conf` (if not already set):

```ini
[Journal]
SystemMaxUse=500M
SystemMaxFileSize=50M
MaxRetentionSec=1month
```

```bash
sudo systemctl restart systemd-journald
```

---

## 6. Update / runbook

### 6.1 Deploy a new Hub build

The Hub is a separate crate (`mmm-hub/`); the root `cargo build` must stay
unaffected. Build the release binary and swap it atomically.

```bash
# On the build machine (or on the host):
cd mmm-hub
cargo build --release --locked
# → target/release/mmm-hub

# Copy to the host (adjust user/host as needed):
scp target/release/mmm-hub momo@192.168.178.200:/tmp/mmm-hub.new
```

On the host:

```bash
# 1. Snapshot the DB before any schema change.
sudo /usr/local/bin/mmm-hub-backup.sh

# 2. Install the new binary atomically, keeping the old one for rollback.
sudo install -o root -g root -m 0755 /tmp/mmm-hub.new /usr/local/bin/mmm-hub.new
sudo mv /usr/local/bin/mmm-hub /usr/local/bin/mmm-hub.prev
sudo mv /usr/local/bin/mmm-hub.new /usr/local/bin/mmm-hub

# 3. Restart (the binary runs its additive migrations on boot).
sudo systemctl restart mmm-hub

# 4. Health check (see §7).
curl -sf http://127.0.0.1:8080/api/hub/health
```

### 6.2 Migrate DB

Migrations are **additive** and owned by the Hub crate
(`mmm-hub/migrations/NNN_*.sql`); the binary applies them on startup. There is
no separate migrate command in the contract. Ops rules:

- Always take a backup (§3.3) before starting a build that ships a new
  migration.
- Never edit an already-applied migration; add a new `NNN_*.sql`.
- Verify the applied schema after boot with `sqlite3 hub.db ".schema"` (§7).
- If a migration fails, the service will not start — roll back the binary
  (§6.3) and restore the pre-migration snapshot if needed.

### 6.3 Rollback

```bash
# 1. Stop the service.
sudo systemctl stop mmm-hub

# 2. Restore the previous binary.
sudo mv /usr/local/bin/mmm-hub /usr/local/bin/mmm-hub.bad
sudo mv /usr/local/bin/mmm-hub.prev /usr/local/bin/mmm-hub

# 3. If the failed build applied a migration, restore the pre-update snapshot.
sudo -u momo cp /var/backups/mmm-hub/hub-<STAMP>.db /var/lib/mmm-hub/hub.db
sudo -u momo rm -f /var/lib/mmm-hub/hub.db-wal /var/lib/mmm-hub/hub.db-shm

# 4. Start and verify.
sudo systemctl start mmm-hub
curl -sf http://127.0.0.1:8080/api/hub/health
```

Because migrations are additive, a rollback of the binary alone is usually
enough; restore the snapshot only if the new migration is not backward
compatible with the old binary.

---

## 7. Verification commands

### 7.1 Health (local and via proxy)

```bash
# Directly against the service (loopback).
curl -s http://127.0.0.1:8080/api/hub/health
# → {"data":{"status":"ok","version":"hub-v0.4.0"}}

# Through Caddy / TLS.
curl -s https://hub.klimk.es/api/hub/health

# Fail the shell if unhealthy (useful in scripts).
curl -sf http://127.0.0.1:8080/api/hub/health > /dev/null && echo OK
```

### 7.2 Service state

```bash
systemctl status mmm-hub --no-pager
systemctl is-active mmm-hub
systemctl show mmm-hub -p EnvironmentFiles -p ExecStart
```

### 7.3 Database schema

```bash
# Full schema (canonical truth — trust this over migration files).
sqlite3 /var/lib/mmm-hub/hub.db ".schema"

# Tables + views only.
sqlite3 /var/lib/mmm-hub/hub.db ".tables"

# Confirm the hub_* prefix and the four overlap views exist.
sqlite3 /var/lib/mmm-hub/hub.db \
  "SELECT name, type FROM sqlite_master
   WHERE name LIKE 'hub_%' ORDER BY type, name;"

# Confirm WAL is active.
sqlite3 /var/lib/mmm-hub/hub.db "PRAGMA journal_mode;"
# → wal

# Confirm migrations were applied.
sqlite3 /var/lib/mmm-hub/hub.db \
  "SELECT version, description, success FROM _sqlx_migrations ORDER BY version;"
```

Expected views (contract §5): `hub_v_track_presence`, `hub_v_shared_tracks`,
`hub_v_user_overlap`, `hub_v_track_playlists`.

### 7.4 Auth smoke (after M2)

```bash
# Unauthenticated access to a protected endpoint must be blocked.
curl -s -o /dev/null -w '%{http_code}\n' http://127.0.0.1:8080/api/hub/me
# → 401 (or 302 to the OIDC login, depending on M2's choice)
```

---

## 8. First-time setup (condensed)

```bash
# 1. Create the service user if it does not exist (client already uses `momo`).
id momo || sudo useradd --system --create-home --shell /usr/sbin/nologin momo

# 2. Directories.
sudo install -d -o momo -g momo -m 0750 /var/lib/mmm-hub
sudo install -d -o root -g momo -m 0750 /etc/mmm-hub
sudo install -d -o momo -g momo -m 0750 /var/backups/mmm-hub

# 3. Binary.
sudo install -o root -g root -m 0755 mmm-hub /usr/local/bin/mmm-hub

# 4. Env file (edit secrets).
sudo install -o root -g momo -m 0640 /dev/null /etc/mmm-hub/hub.env
sudoedit /etc/mmm-hub/hub.env

# 5. systemd units.
sudo install -m 0644 mmm-hub.service /etc/systemd/system/
sudo install -m 0644 mmm-hub-backup.service /etc/systemd/system/
sudo install -m 0644 mmm-hub-backup.timer /etc/systemd/system/
sudo install -m 0755 mmm-hub-backup.sh /usr/local/bin/mmm-hub-backup.sh
sudo systemctl daemon-reload

# 6. Start.
sudo systemctl enable --now mmm-hub
sudo systemctl enable --now mmm-hub-backup.timer

# 7. Activate WAL once (service may be running; VACUUM INTO backup first).
sudo -u momo sqlite3 /var/lib/mmm-hub/hub.db "PRAGMA journal_mode=WAL;"

# 8. Verify.
curl -sf http://127.0.0.1:8080/api/hub/health
```

---

## 9. Open questions & assumptions

The Hub cannot be deployed until these are confirmed. Items the repo already
answers are marked **[RESOLVED FROM REPO]** with the source; the rest stay
**[ASSUMPTION]** and must be confirmed before **M2** (they gate the OIDC callback:
items 1, 2, 3, 4, 5).

1. **Existing reverse proxy / Caddy.** **[RESOLVED FROM REPO, in part]** The repo
   documents a **Caddy in Docker** configured at `~/caddy/`
   (`~/caddy/docker-compose.yml` + `~/caddy/Caddyfile`), currently serving
   `music.klimk.es` and `deemix.klimk.es`, with `*.klimk.es` TLS auto-provisioned
   via the Cloudflare DNS challenge (`deploy/README.md`,
   `deploy/Caddyfile.snippet`). The remaining unknown is **which host** runs that
   Caddy for the Hub — see item 2.
2. **Is `192.168.178.200` the Caddy host?** **[ASSUMPTION — unresolved]** The
   contract fixes the Hub host at `192.168.178.200`, but the client deploy docs
   place Caddy on `192.168.178.149` (`music.klimk.es → 192.168.178.149:3000`),
   while the README points `192.168.178.200` at a `music-api` instance on `:8710`.
   It is therefore **not** established that `.200` runs Caddy. This doc
   **assumes** the Hub is fronted by a Caddy that can reach
   `192.168.178.200:8080` — either `.200` runs its own Caddy, or the existing
   Caddy (wherever it lives) is extended to proxy `hub.klimk.es` to
   `192.168.178.200:8080` across the LAN. Confirm before M2; this decides the
   `reverse_proxy` target in §4.3 and the `HUB_HOST` posture in §5.1.
3. **Which domain for the Hub?** **[ASSUMPTION]** This doc assumes
   `hub.klimk.es`; the repo notes any unused `*.klimk.es` subdomain works
   (`deploy/README.md`, "Available domains"). Confirm whether it is a public A
   record, split-horizon, or Tailscale-only, and whether it points at `.200` or at
   the Caddy host. The host is substituted into both redirect URIs in §4.4.
4. **Pocket ID issuer URL.** **[ASSUMPTION]** Assumed `https://id.klimk.es`. It
   must be HTTPS (the `openidconnect` `IssuerUrl` is https-only) and reachable
   from the Hub host. Confirm the real issuer and that the OIDC client's
   **Access tab** grants the intended users (Pocket ID refuses silently
   otherwise).
5. **OIDC callback path.** **[RESOLVED]** The contract freezes it:
   `GET /api/hub/auth/callback` (`00-interface.md` §3). Only the host is an
   assumption (item 3); the path is registered in Pocket ID per §4.4.
6. **Caddy networking mode.** **[ASSUMPTION]** If Caddy is containerised, the
   `reverse_proxy` target depends on its mode (`network_mode: host`,
   `host.docker.internal`, or the LAN IP + firewall; see §4.3). Pick the one that
   matches `~/caddy/docker-compose.yml` on whichever host runs it.
7. **Backup target.** **[ASSUMPTION]** Local `/var/backups/mmm-hub` only, or also
   ship snapshots to the NAS (the client already rsyncs to a Synology)? The Hub
   holds OAuth tokens, so off-host backup has a security dimension.
8. **Service user.** **[ASSUMPTION]** Reuse `momo` (as the client does) or create
   a dedicated `mmm-hub` system user? A dedicated user is stricter but adds setup
   steps.
9. **`HUB_SQL_CONSOLE_ENABLED`.** **[ASSUMPTION]** Kept `false` in normal
   operation; enabled only via a deliberate, temporary drop-in override.
10. **Port conflicts — is `8080` free on the target host?** **[ASSUMPTION —
    unresolved]** Unverified. The repo's known ports are client `3000`, deemix
    `6595`/`6599`, telemetry `8330`, music-api `8710`; `8080` is not among them,
    so it is _probably_ free, but confirm on `.200` with
    `ss -ltnp | grep ':8080'` (or `sudo lsof -i :8080`) before M2. If taken, pick
    another `HUB_PORT` and update `HUB_PUBLIC_URL`, the Caddy upstream, and — if
    it is not proxied — nothing else.

Once these are answered, fold the confirmed values back into §0, §2 and §4 and
drop the **[ASSUMPTION]** markers.
