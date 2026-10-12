# Thoughts: Server / Multi-Device Architecture

> Brainstorming session — don't implement, just think.

---

## Current State

- Single Rust binary (Axum + embedded SPA)
- SQLite at `~/.local/share/momos-music-manager/library.db`
- `files` table: absolute local paths (`/Users/momo/Music/...`), validated against disk
- `folders` table: absolute local directory paths, must exist on disk
- File scanning reads audio metadata from local files (lofty/exiftool)
- Comment writing modifies actual audio files on disk
- Spotify/deemix syncs are purely API-based (no local files needed)
- Tags, playlists, service tracks are all metadata in the DB
- Frontend uses relative API calls (`API_BASE = window.location.origin`)
- No hardcoded localhost anywhere — already portable

---

## The Core Tension

| Thing                     | Needs local files on disk?                      |
| ------------------------- | ----------------------------------------------- |
| Spotify sync              | ❌ No — pure API                                |
| Deemix downloads          | ❌ No — downloads go to deemix server           |
| Tag editing (metadata)    | ❌ No — pure DB                                 |
| Browsing tracks/playlists | ❌ No — pure DB                                 |
| Browsing file metadata    | ❌ No — pure DB (title, artist, BPM, key, tags) |
| **File scanning**         | ✅ Yes — reads audio files                      |
| **Comment writing**       | ✅ Yes — mutates audio files                    |
| **Folder validation**     | ✅ Yes — checks path exists                     |
| **Local playback**        | ✅ Yes — needs file bytes                       |

**Key insight:** The vast majority of what you want to do on your phone (browse, categorize, tag) is **pure metadata** — it only touches the DB, not the filesystem. The filesystem-dependent operations are admin tasks you'd only do from your Mac anyway.

---

## Option Matrix

### Option 1: Tailscale — Mac stays on, phone connects via VPN

```
┌─────────────────────────────────────────────┐
│  Your Mac (runs app, has files)             │
│  localhost:3000 ←── Tailscale ──→ Phone     │
│  100.xxx.xxx.xxx:3000                       │
└─────────────────────────────────────────────┘
```

**How:** Install Tailscale on Mac + phone. Phone browser opens `http://<mac-tailscale-ip>:3000`. App is literally the same instance.

**Pros:**

- Zero code changes. Already works today.
- All features work (files, scanning, everything).
- Free (Tailscale free tier, up to 100 devices).
- Encrypted, private tunnel. No ports exposed to internet.
- You can also use it from a laptop, tablet, anywhere.

**Cons:**

- Mac must stay powered on and awake.
- Mac must not sleep (or you configure wake-on-LAN / amphetamine).
- Battery drain if on laptop battery.

**Mitigations:**

- Mac mini / always-on desktop — this is your use case anyway.
- "Wake for network access" in macOS Energy Saver settings.
- Cheap used Mac mini as a home server (~$100-200).

**Verdict:** 🟢 Simplest, zero-code solution. If the "Mac staying on" is acceptable, this is the answer.

---

### Option 2: Cloudflare Tunnel — like Tailscale but without VPN client

```
┌──────────────────────────────────────┐
│  Your Mac (runs app, has files)      │
│  localhost:3000 ←── cloudflared ──→  │
└──────────────┬───────────────────────┘
               │  Cloudflare edge
               ▼
     https://mmm.yourdomain.de
```

**How:** Run `cloudflared tunnel` on Mac. Exposes `localhost:3000` to a public URL (or with Cloudflare Access for auth). Phone opens the URL.

**Pros:**

- No VPN client needed on phone — just a browser.
- Free.
- Can add Cloudflare Access (email OTP) for security.
- You already have a domain.

**Cons:**

- Mac must stay on (same as Option 1).
- Slightly more setup than Tailscale.
- Publicly exposed (mitigated by Access/auth).

**Verdict:** 🟡 Good if you want browser-only access without VPN app. Still needs Mac on.

---

### Option 3: Cheap VPS — deploy the binary, sync DB only

```
┌──────────────────────┐     ┌──────────────────────┐
│  Your Mac            │     │  VPS ($5/mo)          │
│  Has files           │     │  Runs app 24/7        │
│  Scans + writes tags │     │  Spotify/deemix sync  │
│  Pushes DB changes   │◄───►│  Serves web UI        │
└──────────────────────┘     └──────────┬───────────┘
                                        │
                                        ▼
                                      Phone
```

**How:** Deploy the Rust binary to a cheap VPS (Hetzner CX22 at ~€4/mo, or free Oracle Cloud tier). The VPS runs the app 24/7. You access it via `https://mmm.yourdomain.de`. The DB on the VPS is the **canonical metadata store**.

For file scanning: you scan from your Mac and it pushes file metadata to the VPS. Or you accept that the `files` table on the VPS has paths that don't resolve (the VPS doesn't have your audio files), but the metadata is still browsable.

**Sub-option 3a — "Bare repo" mode (metadata-only server):**

The server knows about your files (title, artist, BPM, key, tags, service links) but can't access the actual audio bytes. It's like a `git clone --bare` — all the metadata, none of the blobs.

- File scanning: done on Mac, results pushed to server DB
- Comment writing: done on Mac (or deferred)
- Everything else: works from server

The server flags files as "remote-only" or stores an `origin_host` field so the UI can indicate "this file lives on momo's Mac".

**Sub-option 3b — "Single DB, synced":**

Use Litestream or LiteFS to replicate the SQLite DB between Mac and VPS. Both run the same binary. The Mac is for file ops, the VPS is for 24/7 access and service syncs.

**Pros:**

- 24/7 availability without Mac being on.
- Spotify/deemix syncs run on server, always up to date.
- Proper domain, HTTPS, always reachable.

**Cons:**

- Monthly cost (~€4-5/mo).
- File scanning / comment writing workflow needs thought.
- DB sync adds complexity (or you accept two separate DBs).
- Deploying Rust binary to Linux VPS needs cross-compilation or building on VPS.

**Verdict:** 🟡 The "proper" server solution. More work but most flexible.

---

### Option 4: Raspberry Pi / home server

```
┌──────────────────────────────────────────┐
│  Raspberry Pi (or old laptop/Mac mini)    │
│  Runs app 24/7 on your LAN               │
│  Tailscale for remote access             │
│  NFS/SMB mount your music from Mac/NAS   │
└──────────────────────────────────────────┘
```

**How:** A small always-on device on your home network runs the app. Your music files are on a NAS or shared from your Mac via SMB/NFS, mounted on the Pi. The Pi sees the same file paths (or you configure paths accordingly).

**Pros:**

- 24/7 availability.
- One-time cost (~€50-100 for a Pi).
- No monthly VPS cost.
- Files are accessible (scanning, comment writing works if paths match).
- Tailscale gives phone access.

**Cons:**

- Pi setup and maintenance.
- File path mapping might be tricky (Mac paths vs Linux paths).
- Pi performance (SQLite on SD card is fine for this scale).
- Another device to manage.

**Verdict:** 🟢 Great middle ground if you're comfortable with a Pi. Better than VPS because you keep your files local.

---

### Option 5: "Server Mode" — split the app into two roles

This is the most architecturally interesting option but also the most work.

```
┌─────────────────────────┐     ┌─────────────────────────┐
│  Server (VPS/Pi)         │     │  Client (your Mac)       │
│  ┌───────────────────┐   │     │  ┌───────────────────┐   │
│  │ DB (canonical)    │◄──┼─────┼──┤ Local files       │   │
│  │ Web UI            │   │     │  │ File scanner       │   │
│  │ Spotify sync      │   │     │  │ Comment writer     │   │
│  │ Deemix sync       │   │     │  │                    │   │
│  │ Tag editing API   │   │     │  │                    │   │
│  └───────────────────┘   │     │  └───────────────────┘   │
└─────────────────────────┘     └─────────────────────────┘
```

**How the split works:**

The same binary runs in two modes:

- `momo serve --mode server` — canoncial DB, all service syncs, web UI
- `momo serve --mode client` — local file scanning, pushes metadata to server

Or even simpler: the "client" is just a CLI that scans and pushes:

- `momo push` — scans local folders, pushes file metadata to server
- `momo pull` — pulls tag changes from server, writes comments to local files

**Communication:** REST API between client and server. Client authenticates with a shared secret or API key.

**Data flow:**

1. Mac: scan files → POST /api/files/batch → Server
2. Phone: edit tags → PUT /api/files/:id/tags → Server
3. Mac: pull changes → GET /api/files/changed → write comments to local files

**Pros:**

- Clean separation of concerns.
- The "git push/pull" mental model fits perfectly.
- Server is the source of truth for metadata.
- Mac could be offline for days, then sync when back.
- Could have multiple clients (studio Mac, laptop, etc.).

**Cons:**

- Significant implementation work.
- Conflict resolution (what if you edit tags on phone AND on Mac?).
- Two instances to manage.
- Needs auth between client and server.

**Verdict:** 🔴 Most powerful but most work. Probably overkill for a single user. But the mental model is the right one if this grows.

---

### Option 6: Just expose your Mac (simplest version of Option 1, no Tailscale)

If your Mac has a static IP or you're comfortable with dynamic DNS:

- Port forward 3000 on your router → Mac
- Dynamic DNS (duckdns.org, free)
- Access via `http://mmm.duckdns.org:3000`

Or if you already have nginx/Caddy somewhere, reverse proxy to your Mac via Tailscale.

**Verdict:** 🟡 Works but less secure than Tailscale. Tailscale is strictly better for this.

---

## What's Actually Needed for the Phone Use Case?

Let's be concrete. You want to:

1. Browse your tracks/files (title, artist, BPM, key, tags)
2. Assign/change tags on files (e.g., mark tracks with phase/mood/vibe)
3. See playlists
4. Maybe: use the digging page

All of this is **pure DB operations**. None of it touches a file on disk. The only file-system operations are:

- Scanning (you do this from Mac, probably periodically)
- Comment writing (you do this from Mac, after a scanning or tagging session)

So the question isn't "how do I run the full app on a server" — it's "how do I access the DB and web UI from my phone."

---

## My Recommendation

### Short-term (today): Option 1 — Tailscale

1. Install Tailscale on Mac: `brew install tailscale && tailscale up`
2. Install Tailscale on phone (App Store)
3. Open `http://<mac-ip>:3000` on phone browser
4. Done. Zero code changes.

This gets you going immediately. You can categorize tags from your phone during the day. The Mac needs to stay awake — use `caffeinate` or System Settings → Energy → "Prevent automatic sleeping."

**Total time:** 5 minutes.

### Medium-term (next few weeks): Option 1 + small improvements

Small code changes that make the remote-access experience better:

1. **Mobile-responsive CSS** — the current UI is desktop-first. A few media queries would make it usable on phone. (This is probably worth doing regardless.)
2. **PWA manifest** — add a `manifest.json` so you can "Add to Home Screen" and it feels like an app.
3. **Health check page** — already exists at `/api/health`, but maybe a nice mobile landing page.

### Long-term (if needed): Option 5 lite — "push/pull" model

If you find yourself wanting the server to be independent of your Mac:

1. **Deploy to a cheap VPS** (or a Pi at home).
2. **The VPS is the canonical DB.** It runs Spotify/deemix syncs, serves the web UI.
3. **Add `momo push` command**: Scans local folders on Mac, POSTs file metadata to the VPS API. The VPS inserts/updates the `files` table. File paths are stored as-is (or with a `host` tag so you know where they live).
4. **Add `momo pull` command**: Fetches changed tags from VPS, writes comments to local files whose tags changed.
5. **No "sync conflicts" to start:** Just last-write-wins. You're a single user, this is fine.

This gives you the "git-like" model: push your local file state to origin, pull tag changes back. The server is always up, always syncing Spotify.

**But honestly?** Start with Tailscale. It might solve 100% of your problem with 0% of the code changes.

---

## Files Table Thought Experiment

What if the `files` table on a server has paths like `/Users/momo/Music/...` but the server is Linux and doesn't have those paths?

| Operation           | Works? | Why                                              |
| ------------------- | ------ | ------------------------------------------------ |
| List files          | ✅     | Pure SQL query                                   |
| Filter by BPM/key   | ✅     | Pure SQL query                                   |
| Edit tags           | ✅     | Pure SQL (via tag_file junction)                 |
| Show file metadata  | ✅     | Title, artist, BPM, key all in DB                |
| Scan files          | ❌     | Needs actual audio files                         |
| Write comments      | ❌     | Needs to modify actual audio files               |
| Validate folders    | ❌     | `normalize_and_validate_folder_path` checks disk |
| Show "linked" badge | ✅     | Service IDs are in the DB                        |

**The only things that break are scanner and comment writer.** Everything else is DB-only and works fine on a server with no music files.

For the phone use case, this is perfect — you don't need scanning or comment writing from your phone.

The server could even show a badge like "📁 Remote file" or hide the "Scan" button for files that aren't locally accessible.

---

## Small, High-Impact Code Changes (Regardless of Option)

These would improve remote access no matter which option you choose:

1. **Mobile CSS** — a few media queries in `style.css`. The nav sidebar could collapse to a hamburger. Tables could get horizontal scroll. Font sizes could adjust. This is probably 100-200 lines of CSS.

2. **PWA support** — add `<link rel="manifest">` and a service worker so the app can be "installed" on the phone home screen. Feels native.

3. **Login page** — currently the app is wide open (no auth). If you expose it to the internet (even via Tailscale), you might want a simple password gate. Could be as simple as a config option `server.password = "hunter2"` that enables HTTP Basic Auth.

4. **"Remote file" indicator** — if the server can't access a file's path, show a small icon/tooltip. Helps distinguish "this file is on my Mac" from "this track is a Spotify track."

---

## Decision Framework

| Question                                       | Answer determines...                                    |
| ---------------------------------------------- | ------------------------------------------------------- |
| Is your Mac always on anyway?                  | → Option 1 (Tailscale) is the clear winner              |
| Do you want this working tomorrow?             | → Option 1 (Tailscale), zero code                       |
| Do you want to turn off your Mac sometimes?    | → Option 3 (VPS) or 4 (Pi)                              |
| Do you want Spotify/deemix syncing 24/7?       | → Option 3 (VPS) or 4 (Pi) with server as sync runner   |
| Is the VPS cost (~€5/mo) acceptable?           | → Option 3                                              |
| Would you rather buy a Pi once?                | → Option 4                                              |
| Is the "git push/pull" model appealing?        | → Option 5 (but this is a project, not a quick fix)     |
| Do you want to access from multiple locations? | → Tailscale works everywhere, VPS also works everywhere |

---

### Option 7: Peer-to-Peer Sync — "Steam Cloud for your music metadata"

This is the model you're describing: every node is an equal peer, each works
fully offline with its own local DB, and when connectivity is available they
sync state with each other. No master, no slave — just nodes at eye level.

```
┌───────────────────┐         ┌───────────────────┐
│  Your Mac          │         │  VPS / Pi / Relay  │
│  Full local DB     │         │  (dumb sync relay) │
│  Has audio files   │◄────────►  Stores sync log   │
│  Works offline     │         │  Doesn't run app   │
└───────────────────┘         └───────────────────┘
                                        ▲
                                        │
                               ┌────────┴──────────┐
                               │  Your phone?       │
                               │  (connects to      │
                               │   any node's UI)   │
                               └───────────────────┘
```

#### How Steam Cloud actually works (the inspiration)

Steam doesn't do master-slave or real consensus. It's much simpler:

1. Each device has its own local save files
2. When you finish playing, Steam uploads the save to the cloud with a timestamp
3. When you start on another device, Steam checks: "is cloud newer than local?"
4. If yes → download. If conflict → ask user. Otherwise → use local.
5. The "cloud" is just blob storage with timestamps — it doesn't run the game

Translated to our world: the "save file" is the SQLite database (or at least
the metadata portions of it). The "game" is the app running on each node.

#### What would sync, and what wouldn't

| Data                    | Sync? | Why                                                      |
| ----------------------- | ----- | -------------------------------------------------------- |
| Tags + tag categories   | ✅    | Pure metadata, universal                                 |
| Service tracks          | ✅    | Same Spotify track on every node                         |
| Playlists               | ✅    | Pure metadata                                            |
| File metadata           | ✅    | Title, artist, BPM, key, ISRC — universal once extracted |
| Tag-file associations   | ✅    | Which tags are on which file                             |
| Service config/creds    | ❌/⚠️ | Maybe sync, maybe per-node (depends on trust model)      |
| File paths              | ⚠️    | `/Users/momo/Music/...` only makes sense on Mac          |
| File hashes             | ✅    | SHA of the audio bytes — same everywhere                 |
| Folder paths            | ⚠️    | `/Volumes/NAS/...` only makes sense on one machine       |
| Actual audio files      | ❌    | Way too big, not the point                               |
| Embedded tag embeddings | ✅    | Computed from tags, deterministic                        |
| Deemix download queue   | ⚠️    | Probably only on the node that runs deemix               |

**The trick for file paths:** A file is uniquely identified by its **hash**,
not its path. The path is just "where this file lives on this particular node."
Two nodes can have the same file at different paths (or one node might not have
the file at all). The sync layer matches files by hash.

This means you could have:

- Mac: `/Users/momo/Music/Techno/kicktrack.flac`
- Pi: `/mnt/nas/Music/Techno/kicktrack.flac`
- VPS: no file at all (but still knows the metadata)

All three agree on: title, artist, BPM, key, tags, ISRC. Because they matched
by file hash.

#### Technical approach: sync log (event sourcing)

The core idea: don't try to merge SQLite files (that's fragile). Instead,
record every mutation as an event and replay them.

**Add a `sync_log` table:**

```sql
CREATE TABLE sync_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id TEXT NOT NULL,           -- e.g. "mac-studio", "pi-livingroom"
    event_type TEXT NOT NULL,        -- "insert", "update", "delete"
    table_name TEXT NOT NULL,        -- "tags", "tag_file", "files", ...
    row_id INTEGER NOT NULL,         -- the PK of the affected row
    column_name TEXT,                -- NULL for insert/delete
    old_value TEXT,                  -- NULL for insert
    new_value TEXT,                  -- NULL for delete
    created_at INTEGER NOT NULL      -- Unix timestamp (wall clock of writing node)
);
```

Every time the app writes to any synced table, it also writes a sync_log entry
(in the same SQLite transaction for atomicity).

**Sync protocol (between two nodes):**

```
Node A connects to Node B:
  A: "My last synced log id from you is 42. What's new?"
  B: "Here are events 43-57."
  A: Applies events 43-57 to local DB with conflict resolution
  A: "Here are my events since you last synced from me."
  B: Applies them too.
  Both: Remember the new high-water mark for each other.
```

**Conflict resolution: last-write-wins (LWW)**

```
When applying an event from a remote node:
  1. Look up the current state of that row locally
  2. If local.last_modified > event.created_at → skip (local is newer)
  3. If local.last_modified < event.created_at → apply (remote is newer)
  4. If equal → tiebreak by node_id (deterministic but arbitrary)
```

This isn't perfect (you could lose a change if clocks are skewed), but it's
the same tradeoff Steam and virtually every consumer sync service makes.
For a single user editing tags, conflicts are rare and LWW is good enough.

**The relay node:**

You need at least one node that's reachable when the others aren't online
simultaneously. This relay doesn't run the full app — it's just:

- A SQLite database storing the union of all sync logs
- A simple HTTP endpoint: `POST /sync/push`, `GET /sync/pull?since=X&from=nodeA`
- Could be a €3/mo VPS running a 100-line Rust binary, or even just a PHP
  script on shared hosting, or an S3 bucket with JSON files

The relay is dumb — it doesn't understand the data, just stores and forwards
sync events. All intelligence is in the nodes.

#### What offline + sync looks like in practice

**Scenario: Tagging on the train**

1. You're on your Mac, offline. You add tags to 15 tracks.
2. Each tag assignment writes to `tag_file` + a `sync_log` entry.
3. You're still offline — nothing happens.
4. You get home, Mac connects to WiFi.
5. The sync worker sees connectivity to the relay, pushes all pending events.
6. Later, you open the app on your phone (which connects to the relay's web UI
   or to your Pi which already synced). The tags are there.

**Scenario: Conflict**

1. You tag a track as "💚 favorite" on your phone at 14:32.
2. You tag the same track as "🗑️ trash" on your Mac at 14:33 (while offline).
3. Both sync to the relay.
4. Your Mac pulls the phone's event — sees timestamp 14:32 vs its local 14:33.
   Mac's is newer, so it keeps "🗑️ trash" and discards the phone's event.
5. Your phone pulls the Mac's event — sees timestamp 14:33 vs its local 14:32.
   Remote is newer, so it overwrites "💚 favorite" with "🗑️ trash".
6. Both now agree. The phone's earlier tag was silently lost.

This is acceptable for a single-user system. The alternative (asking the user
to resolve conflicts) is more correct but adds friction.

#### What needs to change in the codebase

| Change                                   | Effort | Notes                                                         |
| ---------------------------------------- | ------ | ------------------------------------------------------------- |
| Add `sync_log` table                     | Small  | Schema migration                                              |
| Add `node_id` field to config            | Small  | `config.toml` → `[node].id = "mac-studio"`                    |
| Write sync_log on every mutation         | Medium | Wrap DB writes in a helper that also logs                     |
| Add `last_modified` to all synced tables | Medium | Most tables already have `created_at`/`updated_at`            |
| Sync worker (push/pull loop)             | Large  | Background task, HTTP client, conflict resolution             |
| Relay binary                             | Medium | 100-200 lines, just stores and serves sync logs               |
| Identify files by hash, not path         | Medium | Already have `file_hash`, just need to use it for matching    |
| Handle "file exists on node A but not B" | Small  | Files with unknown hashes are shown as "remote" with metadata |

**Total: probably 2-3 weeks of focused work.** Not a weekend hack, but not
a months-long project either.

#### The beautiful part

Once you have this sync infrastructure:

- **Adding a new node is trivial.** Spin up the app anywhere, point it at the
  relay, and it pulls the full state. No manual DB copies.
- **The relay is disposable.** It's just a log. You could delete it and
  re-sync from any node that has the full history. Or you could have
  multiple relays.
- **Nodes can come and go.** Your laptop, your studio Mac, a Pi, a VPS —
  they all converge to the same state when connected.
- **Spotify sync can run anywhere.** The node that's online most (VPS or Pi)
  runs the syncs. Other nodes get the results via the sync log.

#### Comparison with the other options

|                            | Option 1 (Tailscale) | Option 5 (Server mode) | Option 7 (P2P sync) |
| -------------------------- | -------------------- | ---------------------- | ------------------- |
| Works offline              | ❌ (needs Mac on)    | ❌ (needs server)      | ✅                  |
| Multi-device               | ✅ (same instance)   | ✅ (server is hub)     | ✅ (all peers)      |
| No single point of failure | ❌                   | ❌                     | ✅ (relay is dumb)  |
| Conflict resolution        | N/A (single writer)  | N/A (single writer)    | Last-write-wins     |
| Implementation effort      | 0                    | Medium                 | High                |
| Operational complexity     | None                 | Low                    | Medium              |

---

## What Do You Think?

A few prompts to guide our tandem discussion:

1. **Mac always-on?** Are you okay with your Mac staying awake during the day? Or is that a dealbreaker?

2. **VPS comfort?** Are you comfortable setting up a Linux VPS, or would you rather keep things on hardware you own?

3. **Pi at home?** Do you have a Raspberry Pi or old machine lying around that could be a home server?

4. **"Git model" appeal?** Does the idea of `momo push` / `momo pull` resonate with you, or does it feel like overengineering?

5. **Timeline?** Do you want something working this week, or are you up for a small project?
