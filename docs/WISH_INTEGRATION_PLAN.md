# Plan: wish.zukkafabrik.de — Full Integration

> Stand: 2026-07-19
> State: Pipeline 100%, SQLite store, Cookies ready

---

## Current State

| Layer | Status |
|---|---|
| Download-Pipeline | ✅ 100% (84/84, deemix + spotDL) |
| SQLite Store | ✅ Persistiert, queryable |
| Guest Page | ✅ vanilla JS, Search + Queue + Upvote |
| Caddy | ✅ wish.zukkafabrik.de → :8700 |
| dufs | ✅ files.wish.zukkafabrik.de → :8321 |
| Spotify Cookies | ✅ ~/.config/spotdl/cookies.txt |
| Deno | ✅ /home/momo/.deno/bin/deno |

## What still needs connecting

### 1. Search → Real API

Current: guest page calls `/search?q=...` but might fail through Caddy SSL.

**Fix:** Verify CORS/SSL works, add error handling in guest page.

### 2. Request → Download → Queue Sync

Current flow works but needs testing:
```
Guest clicks "Want" → POST /download {url} → task created → pollStatus() every 15s
```

### 3. File Sync Hetzner → MacBook

Downloads land in `/opt/download-service/downloads/tracks/`. Need sync to Mac for Traktor.

**Approach:** One-shot rsync. Wedding setup: DJ runs before event, downloads are ready.

```bash
rsync -avz projectmellon.de:/opt/download-service/downloads/tracks/ ~/Music/DJ/Incoming/
```

Or continuous sync with Mutagen (was in earlier plan).

### 4. DJ Dashboard

Needs a view of all requests with status. Currently `GET /queue` and `GET /stats` exist.
Could add a simple admin page at `/admin` or just use the API directly.

### 5. Playlist Pre-load

DJ submits wedding playlist BEFORE the event:
```bash
curl -X POST https://wish.zukkafabrik.de/download/playlist \
  -H 'Content-Type: application/json' \
  -d '{"url": "https://open.spotify.com/playlist/..."}'
```

Then `rsync` files to MacBook, done.

---

## Implementation Steps

### Step 1: Verify Guest Page End-to-End

```bash
# Test search via domain
curl 'https://wish.zukkafabrik.de/search?q=Dancing+Queen'

# Test download via domain
curl -X POST https://wish.zukkafabrik.de/download \
  -H 'Content-Type: application/json' \
  -d '{"url":"spotify:track:4cOdK2wGLETKBW3PvgPWqT"}'

# Test status poll via domain
curl https://wish.zukkafabrik.de/stats
```

### Step 2: Fix Guest Page Issues

- [ ] Verify search returns results on the page
- [ ] Verify "Want" button triggers download
- [ ] Verify queue updates with status
- [ ] Fix any CORS/API issues

### Step 3: Add Quick Sync Script

```bash
#!/bin/bash
# sync-wedding.sh — pull all downloads to MacBook
rsync -avz projectmellon.de:/opt/download-service/downloads/tracks/ ~/Music/DJ/Incoming/
echo "Synced $(ls ~/Music/DJ/Incoming/*.mp3 | wc -l) files"
```

### Step 4: DJ Admin Page (optional, minimal)

Simple page at `/admin` showing:
- Stats (ready/failed/pending)
- Recent downloads
- Link to dufs file browser
- Sync reminder

---

## Files to touch

| File | Change |
|---|---|
| `download-service/static/request.html` | Fix API calls, error handling |
| `download-service/main.py` | Add simple /admin page, CORS if needed |
| `scripts/sync-wedding.sh` | NEW: rsync script |

---

## Acceptance Criteria

- [ ] `wish.zukkafabrik.de` search returns real Spotify results
- [ ] "Want" button triggers download, shows in queue
- [ ] Queue updates from pending → ready automatically
- [ ] `rsync` command pulls files to MacBook
- [ ] All 84+ tracks downloadable via both API and direct CLI
- [ ] Spotify cookies persistent (year-long validity)
