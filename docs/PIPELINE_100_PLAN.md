# Plan: Pipeline 100% Reliability

> Stand: 2026-07-18
> Branch: `feat/pipeline-100`

---

## Root Cause Analyse

**Erster Test (Jul 17):** 60/60 ready, 59 deemix, 1 spotDL. ABER: Dateien waren schon von vorherigen Runs im Output-Dir. Deemix hat sie neu gedownloaded (sequentiell), spotDL hat "file already exists" gesehen und geskipped.

**Zweiter Test (Jul 18, clean slate):** 2/62 ready, 60 failed. 60 parallele spotDL-Instanzen hämmern gleichzeitig auf YouTube → alle rc=1.

```
Thread 1: spotdl "Sexy Bitch"      ─┐
Thread 2: spotdl "Dear Jessie"      ─┤
Thread 3: spotdl "Vindarna viskar"  ─┤ 60× spotDL parallel
...                                  ─┤ → YouTube rate-limits ALLE
Thread 60: spotdl "SUBEME LA RADIO" ─┘
```

---

## Fix 1: Download-Semaphore

Nur N Downloads gleichzeitig. Deemix und spotDL teilen sich das Limit.

```python
import threading

DOWNLOAD_SLOTS = threading.Semaphore(3)  # max 3 concurrent CLI downloads

class DeemixDownloader:
    def download(self, url):
        with DOWNLOAD_SLOTS:
            return self._download(url)

class SpotdlDownloader:
    def download(self, url):
        with DOWNLOAD_SLOTS:
            return self._download(url)
```

**Warum 3?** Deemix macht ~3 API-Calls pro Download (tags, download URL, file). 3 parallele sind safe. spotDL braucht YouTube → nur 1 zur Zeit wäre ideal, aber 3 ist OK mit Cookie-File.

## Fix 2: Retry-Logik

Jeder Download-Stage bekommt 3 Versuche mit exponentiellem Backoff:

```python
MAX_RETRIES = 3
RETRY_DELAY = 5  # seconds base

for attempt in range(MAX_RETRIES):
    result = downloader.download(url)
    if result:
        break
    time.sleep(RETRY_DELAY * (2 ** attempt))
```

## Fix 3: File Integrity nach Download

Nach jedem erfolgreichen Download:
- Dateigröße ≥ 500KB
- `file` command sagt es ist Audio
- Wenn corrupt → lösche und retry

```python
def verify_file(path):
    if os.path.getsize(path) < MIN_FILE_SIZE:
        return False
    result = subprocess.run(['file', path], capture_output=True, text=True)
    return 'audio' in result.stdout.lower() or 'MPEG' in result.stdout
```

## Fix 4: SQLite Task-Store

Ersetzt das `_tasks: dict`. Persistiert über Restarts, queryable.

```sql
CREATE TABLE tasks (
    id TEXT PRIMARY KEY,
    spotify_url TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    title TEXT,
    artist TEXT,
    source TEXT,
    file_path TEXT,
    file_size INTEGER,
    error TEXT,
    retries INTEGER DEFAULT 0,
    stage TEXT,
    created_at REAL,
    completed_at REAL
);
```

FastAPI-Endpoints bleiben gleich, lesen/schreiben jetzt SQLite statt Dict.
Queue-History bleibt erhalten (kein Verlust bei Restart).

## Fix 5: End-to-End Test-Script

```bash
#!/bin/bash
# test_pipeline.sh — muss 100% ergeben

PLAYLIST="https://open.spotify.com/playlist/2UCh0hUr8OXrMykCO4HkI3"

# 1. Clean output
rm -f /opt/download-service/downloads/tracks/*.mp3

# 2. Submit playlist
curl -s -X POST http://localhost:8700/download/playlist \
  -H 'Content-Type: application/json' \
  -d "{\"url\": \"$PLAYLIST\"}"

# 3. Poll until all done (max 10 min)
for i in $(seq 1 60); do
  sleep 10
  READY=$(curl -s http://localhost:8700/queue | python3 -c "
import json,sys
d=json.load(sys.stdin)
print(sum(1 for t in d['tasks'] if t['status']=='ready'))
")
  FAILED=$(curl -s http://localhost:8700/queue | python3 -c "
import json,sys
d=json.load(sys.stdin)
print(sum(1 for t in d['tasks'] if t['status']=='failed'))
")
  echo "[$i] ready=$READY failed=$FAILED"
  TOTAL=$((READY + FAILED))
  if [ $TOTAL -ge 60 ]; then break; fi
done

# 4. Verify all files >500KB
CORRUPT=$(find /opt/download-service/downloads/tracks -name "*.mp3" -size -500k | wc -l)
echo "Corrupt files: $CORRUPT"
echo "Total files: $(ls /opt/download-service/downloads/tracks/*.mp3 | wc -l)"

# 5. Assert
if [ $FAILED -eq 0 ] && [ $CORRUPT -eq 0 ]; then
  echo "✅ 100% SUCCESS"
else
  echo "❌ FAILED: $FAILED failed, $CORRUPT corrupt"
  exit 1
fi
```

---

## Dateien

| Datei | Änderung |
|---|---|
| `pipeline.py` | Semaphore + Retry + File-Verify |
| `deemix_client.py` | Semaphore-Wrapper um `download()` |
| `spotdl_client.py` | Semaphore-Wrapper um `download()` |
| `db.py` | NEU: SQLite Task-Store |
| `main.py` | DB statt Dict, Migration bei Startup |
| `test_pipeline.sh` | NEU: End-to-End Test |

---

## Acceptance Criteria

- [ ] 60-Track-Playlist: **0 failed, 0 corrupt**
- [ ] Alle Dateien ≥ 500KB
- [ ] Tasks persistieren über Service-Restart
- [ ] `GET /queue` zeigt History (nicht leer nach Restart)
- [ ] Max 3 concurrent CLI-Downloads (gemessen via `ps aux | grep spotdl | wc -l`)
- [ ] `test_pipeline.sh` exit 0
