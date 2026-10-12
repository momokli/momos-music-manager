# Server Deployment

## Architecture

```
                       ┌──────────────┐
                       │   INTERNET   │
                       └──────┬───────┘
                              │
                    ┌─────────┴─────────┐
                    │  CADDY (Docker)   │
                    │  :80, :443        │
                    │                   │
                    │  domains:         │
                    │  music.klimk.es ──┤
                    │  deemix.klimk.es  │
                    └────────┬──────────┘
                             │ reverse_proxy :3000
                             │
┌────────────────────────────┼──────────────────────────────┐
│                    SERVER (lan)                            │
│                                                            │
│  momos-music-manager (systemd)                             │
│  ├─ Web UI at http://lan:3000  +  https://music.klimk.es  │
│  ├─ SQLite DB: data/library.db                             │
│  ├─ All music files: /home/momo/share/{flacs,stems}        │
│  │                                                         │
│  deemix (Docker, 2 instances)                              │
│  ├─ :6595 → /home/momo/share/flacs                         │
│  └─ :6599 → /home/momo/share/kids                          │
│                                                            │
│  NAS (Synology)                                            │
│  └─ backup target via SSH/rsync                            │
└────────────────────────────────────────────────────────────┘

                    ▲
                    │ browser + Tailscale
                    │
┌───────────────────┴──────────────────────────────────────┐
│                    MACBOOK                                  │
│                                                            │
│  Browser → https://music.klimk.es (or http://lan:3000)     │
│  sync-backpack.sh → rsync backpack files from server        │
│  ~/Music/{stems,flacs} ← local copy for Traktor            │
└────────────────────────────────────────────────────────────┘
```

## First-Time Setup (on server)

```bash
# 1. Install Rust toolchain
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source ~/.cargo/env

# 2. Install system dependencies
sudo apt install -y libsqlite3-dev pkg-config libssl-dev

# 3. Clone repo
git clone <repo-url> ~/momos-music-manager
cd ~/momos-music-manager

# 4. Verify it builds
cargo build --release

# 5. Copy the initial DB (from MacBook or let it create a fresh one)
# Option A: copy from MacBook
#   rsync mac:/path/to/library.db ~/momos-music-manager/data/library.db
# Option B: let the server create a fresh one on first run

# 6. Copy config template
mkdir -p ~/.config/momos-music-manager
cp deploy/config.toml ~/.config/momos-music-manager/config.toml

# 7. Install and start services
sudo cp deploy/momos-music-manager.service /etc/systemd/system/
sudo cp deploy/deemix.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now deemix
sudo systemctl enable --now momos-music-manager

# 8. Verify
curl http://localhost:3000/api/health

# 9. Set secrets (Spotify, SoundCloud)
sudo systemctl edit momos-music-manager
# Paste:
#   [Service]
#   Environment="SPOTIFY_CLIENT_ID=..."
#   Environment="SPOTIFY_CLIENT_SECRET=..."
#   Environment="SPOTIFY_REDIRECT_URI=https://music.klimk.es/callback"
#   Environment="PUBLIC_URL=https://music.klimk.es"
sudo systemctl restart momos-music-manager
```

## Caddy Reverse Proxy

The server runs Caddy in Docker (`~/caddy/docker-compose.yml`) to provide
HTTPS and domain-based routing. The music manager needs a public domain for
Spotify OAuth callbacks.

### 1. Add domain to Caddyfile

Edit `~/caddy/Caddyfile` and add:

```caddy
music.klimk.es {
    reverse_proxy 192.168.178.149:3000
    encode gzip
}
```

Then reload:

```bash
cd ~/caddy && docker compose restart caddy
```

### 2. Register redirect URI in Spotify Dashboard

Go to https://developer.spotify.com/dashboard → your app → Edit Settings →
add `https://music.klimk.es/callback` to Redirect URIs.

### 3. Update systemd environment

```bash
sudo systemctl edit momos-music-manager
```

Add (or verify):

```
Environment="PUBLIC_URL=https://music.klimk.es"
Environment="SPOTIFY_REDIRECT_URI=https://music.klimk.es/callback"
Environment="SPOTIFY_CLIENT_ID=your_spotify_client_id"
Environment="SPOTIFY_CLIENT_SECRET=your_spotify_client_secret"
```

Then restart:

```bash
sudo systemctl restart momos-music-manager
```

### Available domains (from `~/home_domains.txt`)

Pick one that's not already in the Caddyfile:

| Domain                | Status     |
| --------------------- | ---------- |
| `ts.zukkafabrik.de`   | Available  |
| `something.klimk.es`  | Create new |
| `something.monocu.be` | Create new |

(Any `*.klimk.es` subdomain works — Caddy auto-provisions TLS via Cloudflare DNS.)

## Deploying Updates

```bash
# From your MacBook, on any branch:
./deploy/deploy.sh
```

What it does:

1. Rsyncs code to server (excluding target/, .git/, node_modules/)
2. `cargo build --release` on server
3. Installs/updates systemd service
4. Restarts service
5. Health-checks

## Service Management

Startup order:

```
multi-user.target
├── docker.service          (system)
│   ├── caddy (Docker)      # reverse proxy, TLS
│   └── deemix               # systemd unit → docker compose up
│       ├── :6595 (main)
│       └── :6599 (kids)
└── momos-music-manager      # depends on deemix.service
    └── :3000
```

Commands:

```bash
# Status
sudo systemctl status momos-music-manager

# Logs (follow)
journalctl -u momos-music-manager -f

# Logs (last 100 lines)
journalctl -u momos-music-manager -n 100

# Restart
sudo systemctl restart momos-music-manager

# Stop
sudo systemctl stop momos-music-manager

# View resource usage
systemctl show momos-music-manager | grep -E 'Memory|CPU'
```

## Deemix Integration

The server runs two deemix instances in Docker:

| Instance | Port | Downloads to           |
| -------- | ---- | ---------------------- |
| main     | 6595 | /home/momo/share/flacs |
| kids     | 6599 | /home/momo/share/kids  |

After the music manager starts:

1. Open `https://music.klimk.es` → Services page
2. Configure deemix:
   - Main: `http://localhost:6595`
   - Kids: `http://localhost:6599` (optional)
3. Enter your Deezer ARL token
4. Click "Connect"

The music manager will now:

- Auto-download new tracks from subscribed playlists
- Queue downloads through the deemix UI
- Scan downloaded files into the library automatically

## Configuring Folders

The server needs to know where your music lives. Via the web UI at
`https://music.klimk.es/#folders`, add:

| Folder Path            | Type         | Auto-Backup |
| ---------------------- | ------------ | ----------- |
| /home/momo/share/flacs | FLAC library | ✓           |
| /home/momo/share/stems | Stem library | ✓           |
| /home/momo/share/kids  | Kids music   | —           |

Then configure backup per folder to point to the NAS.

## Health & Monitoring

The service includes:

- `ExecStartPost` health check (polls `/api/health` for 30s after start)
- Systemd `Restart=always` with 10s backoff
- Memory limits: soft 2 GB, hard 4 GB
- Startup timeout: 120s (first start with migrations can be slow)

Check health manually:

```bash
curl http://lan:3000/api/health
# → {"status":"ok","database":"connected"}

curl https://music.klimk.es/api/health
# → same, via Caddy
```

## Directory Layout on Server

```
~/momos-music-manager/
├── data/
│   └── library.db          # SQLite database
├── target/release/
│   └── momos-music-manager # Binary
├── frontend/               # SPA (embedded at build time)
├── src/                    # Rust source
├── migrations/             # SQL migrations
├── deploy/                 # This directory
│   ├── momos-music-manager.service
│   ├── deploy.sh
│   ├── config.toml
│   └── README.md

~/caddy/
├── Caddyfile               # Reverse proxy config
└── docker-compose.yml      # Caddy Docker setup

~/share/
├── flacs/                  # Deemix downloads (~175 GB)
├── stems/                  # Stem files (to be synced from NAS)
└── kids/                   # Kids music (deemix kids instance)
```
