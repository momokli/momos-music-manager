# Local Sync: Hetzner → MacBook

> Files land on Hetzner: `/opt/download-service/downloads/tracks/`
> Need to reach MacBook: `~/Music/DJ/Incoming/` for Traktor

---

## Option A: rsync one-shot (simplest)

```bash
# Run manually before gig
rsync -avz --progress projectmellon.de:/opt/download-service/downloads/tracks/ ~/Music/DJ/Incoming/
```

**Pros:** Zero setup, predictable, no daemon
**Cons:** Manual, full scan every time, latency = human reaction time

---

## Option B: rsync cron (set and forget)

```bash
# crontab -e on MacBook
* * * * * rsync -az --delete projectmellon.de:/opt/download-service/downloads/tracks/ ~/Music/DJ/Incoming/ 2>&1 | grep -v "^$"
```

**Pros:** Automatic, runs every minute, `--delete` removes files deleted on server
**Cons:** Up to 60s latency, SSH connection overhead per run, loud on battery

---

## Option C: Mutagen (continuous, bidirectional)

```bash
# Install: brew install mutagen-io/mutagen/mutagen
mutagen sync create \
  --name=dj-tracks \
  --mode=one-way-safe \
  projectmellon.de:/opt/download-service/downloads/tracks \
  ~/Music/DJ/Incoming

# Monitor: mutagen sync monitor
# Pause:   mutagen sync pause dj-tracks
# Resume:  mutagen sync resume dj-tracks
```

**Pros:** ~1-2s latency, persistent SSH, bandwidth-efficient (rsync algo), bidirectional safe
**Cons:** Extra tool, daemon process, overkill for one-direction sync

---

## Option D: watchexec + rsync (event-driven, lightweight)

```bash
# Install: brew install watchexec
# Run on Hetzner: watches output dir, triggers rsync on change
watchexec -w /opt/download-service/downloads/tracks \
  "rsync -az /opt/download-service/downloads/tracks/ momo@macbook.local:~/Music/DJ/Incoming/"
```

**Pros:** Event-driven (<1s latency), no cron, only syncs when new files appear
**Cons:** Needs watchexec on Hetzner, MacBook hostname must resolve

---

## Option E: Tailscale + rsync (for remote gigs)

If MacBook and Hetzner aren't on same network (e.g., wedding venue):

```bash
# Both machines on Tailscale (already set up)
# Hetzner: 100.x.y.z via Tailscale
# MacBook: 100.a.b.c via Tailscale

rsync -az projectmellon.de:/opt/download-service/downloads/tracks/ ~/Music/DJ/Incoming/
```

**Pros:** Works anywhere with internet, encrypted
**Cons:** Depends on venue WiFi quality

---

## Recommendation

| Scenario | Best option |
|---|---|
| Pre-load before gig (at home) | **A** — one-shot rsync, done |
| During gig, same network | **C** — Mutagen, instant sync |
| During gig, remote venue | **E** — Tailscale + periodic rsync |
| Minimal setup, just works | **B** — cron every 30s |

**For wedding DJ flow:** Pre-load the entire wedding playlist BEFORE the event (Option A). During the event, guests request songs → Hetzner downloads → Option B or C syncs to MacBook → appears in Traktor within 30-60s.

---

## Implementation

### Pre-gig checklist

```bash
# 1. Submit wedding playlist to Hetzner
curl -X POST https://wish.zukkafabrik.de/download/playlist \
  -H 'Content-Type: application/json' \
  -d '{"url": "https://open.spotify.com/playlist/..."}'

# 2. Wait for all downloads (poll stats)
watch -n 10 'curl -s https://wish.zukkafabrik.de/stats | python3 -c "import json,sys;d=json.load(sys.stdin);print(f\"ready:{d[\"ready\"]} pending:{d[\"pending\"]}\")"'

# 3. Sync to MacBook
rsync -avz --progress projectmellon.de:/opt/download-service/downloads/tracks/ ~/Music/DJ/Incoming/

# 4. Verify
echo "Files: $(ls ~/Music/DJ/Incoming/*.mp3 | wc -l)"

# 5. Open Traktor → File Browser → DJ/Incoming
# All tracks ready with metadata (BPM/Key via folder watcher)
```

### Live sync during gig (Option B simplified)

```bash
#!/bin/bash
# ~/bin/sync-wedding.sh
while true; do
  rsync -az --delete projectmellon.de:/opt/download-service/downloads/tracks/ ~/Music/DJ/Incoming/
  sleep 30
done
```

---

## Decision

Pick one and I'll script it. My recommendation: **Option A for pre-load, Option B cron for live sync** — zero new dependencies, just rsync which is already on both machines.
