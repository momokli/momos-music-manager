# Wedding DJ Request Tool — Implementation Plan v2

> **Stand**: 2026-07-17
> **Projekt**: Momo's Music Manager
> **Basiert auf**: [layerd_download_reasearch.md](./layerd_download_reasearch.md)

---

## Zusammenfassung

Auf Hochzeiten als DJ: Gäste per QR-Code → Webseite → Song suchen → requesten.
Backend delegiert den Download an einen separaten **Download-Pipeline-Service**
auf dem Hetzner-Server (deemix → spotDL-Fallback). Fertige Dateien werden via
Mutagen in Echtzeit aufs MacBook synchronisiert und erscheinen in Traktor.

---

## Architektur (3-Tier)

```
┌──────────────────┐     ┌──────────────────────────────┐     ┌──────────────────────────┐
│  GÄSTE (Handys)  │     │  MACBOOK (DJ-Laptop)         │     │  HETZNER (Download-Srv)  │
│                  │     │                              │     │                          │
│  QR-Code scannen │────→│  momos-music-manager         │     │  Python FastAPI          │
│  → Song suchen   │     │  :3000/#request (Gäste-UI)   │     │  :8000                   │
│  → Requesten     │     │  :3000/#requests (DJ-UI)     │     │                          │
│                  │     │                              │     │  POST /download          │
│                  │     │  POST /api/requests/search   │     │  {"url":"spotify:..."}   │
│                  │     │  POST /api/requests          │──http──→│                          │
│                  │     │  GET  /api/requests          │     │  Stage 1: Spotify API    │
│                  │     │  PUT  /api/requests/{id}     │     │    → ISRC, Metadata      │
│                  │     │                              │     │  Stage 2: deemix (320k)  │
│                  │     │                              │     │    → Deezer Download      │
│                  │     │                              │     │  Stage 3: spotDL (128k)  │
│                  │     │                              │     │    → YouTube Fallback     │
│                  │     │                              │     │  Stage 4: ID3 Tagging     │
│                  │     │                              │     │                          │
│                  │     │  ~/Music/DJ/Incoming/        │←─mutagen─│  /opt/downloads/tracks/  │
│                  │     │        │                     │     │                          │
│                  │     │  ┌─────▼──────────┐          │     └──────────────────────────┘
│                  │     │  │ Folder Watcher │          │
│                  │     │  │ (alle 5 Min)   │          │
│                  │     │  │ → BPM/Key      │          │
│                  │     │  └─────┬──────────┘          │
│                  │     │        │                     │
│                  │     │  ┌─────▼──────────┐          │
│                  │     │  │ Traktor        │          │
│                  │     │  │ File-Browser    │          │
│                  │     │  └────────────────┘          │
└──────────────────┘     └──────────────────────────────┘     └──────────────────────────┘
```

### Warum separates Download-Backend?

| Aspekt         | MacBook-only (Plan v1)                | Hetzner + MacBook (Plan v2)                             |
| -------------- | ------------------------------------- | ------------------------------------------------------- |
| Download-Speed | Abhängig von Venue-WiFi               | Immer schnell (Rechenzentrum)                           |
| deemix/spotDL  | Python-Abhängigkeit auf Mac           | Läuft auf Server, MacBook bleibt clean                  |
| Verfügbarkeit  | Nur wenn MacBook an + online          | Downloads laufen 24/7 durch                             |
| Skalierung     | Ein Gerät                             | Mehrere DJs könnten sharen                              |
| Offline-Fail   | Gäste sehen "Download fehlgeschlagen" | Downloads gehen trotzdem durch, sync verzögert sich nur |

---

## Download Pipeline (Hetzner-Server)

### Tech-Stack

| Komponente        | Tool                             | Grund                                                 |
| ----------------- | -------------------------------- | ----------------------------------------------------- |
| **Web-Framework** | FastAPI (Python)                 | Async, leichtgewichtig, gut für Subprocess-Management |
| **Primary DL**    | deemix CLI (`bambanah/deemix`)   | Deezer, 320kbps MP3/FLAC                              |
| **ISRC Lookup**   | Deezer REST API direkt           | Klarer 200/404 für Fallback-Entscheidung              |
| **Fallback DL**   | spotDL mit eigenen Creds         | YouTube-Matching, aktiv gepflegt                      |
| **Metadata**      | Spotify API (Client Credentials) | Headless, kein User-Login                             |
| **ID3 Tagging**   | `mutagen` (Python lib)           | Cover-Art, Metadaten schreiben                        |
| **Sync**          | Mutagen (filesystem sync)        | ~1-2s Latenz, über SSH/Tailscale                      |

### Stage 1: Spotify Metadata (Client Credentials Flow)

```
Input:  spotify:track:xxx
Output: { isrc, title, artist, album, cover_url, duration_ms }

Flow:
1. Client Credentials Auth → access_token (1h gültig, mit Cache)
2. GET https://api.spotify.com/v1/tracks/{id}
3. Extrahiere ISRC, Artist, Title, Album, Cover-URL
```

**Kein User-Login nötig** — Client Credentials Flow ist server-to-server [^4].
Token-Caching vermeidet Rate-Limits.

### Stage 2: Deezer Download (deemix CLI)

```
Input:  ISRC (aus Stage 1)
Output: FLAC/MP3-Datei + Erfolgsstatus

Flow:
1. GET https://api.deezer.com/track/isrc:{ISRC}
   → 200: { id: 123456, title: "...", link: "https://deezer.com/track/123456" }
   → 404: → Stage 3 (Fallback)
2. Subprocess: python3 -m deemix --bitrate 320 --arl {ARL} {deezer_url}
3. Return: Pfad zur heruntergeladenen Datei
```

**Wichtig**: `bambanah/deemix` Fork verwenden (Original von RemixDev ist inaktiv) [^10].
ARL regelmäßig erneuern (läuft ab), Fehler-Handling für `NotLoggedIn`.

### Stage 3: YouTube Fallback (spotDL)

```
Input:  Spotify-Track-URL (aus Original-Request)
Output: MP3-Datei + Erfolgsstatus

Flow:
1. spotdl download {spotify_url} --bitrate 128 --output {dir}
   --cookie-file cookies.txt (YouTube Auth, vermeidet 429)
2. Return: Pfad zur heruntergeladenen Datei
```

**Limits**: Ohne YouTube Premium max 128kbps. Mit YTMusic Premium + M4A: bis 256kbps [^15].
Eigene Spotify-Client-ID/Secret in spotDL-Config eintragen (Shared-Creds sind ratelimited) [^6].

### Stage 4: ID3 Tagging + Move

Nach erfolgreichem Download (Stage 2 oder 3):

1. Mit `mutagen` (Python): Cover-Art aus Stage 1 einbetten
2. ID3-Tags schreiben: Title, Artist, Album, ISRC, Year
3. Datei nach `/opt/downloads/tracks/` verschieben
4. Mutagen sync erkennt neue Datei → überträgt auf MacBook

### FastAPI Endpoints

#### `POST /download`

```json
// Request
{ "url": "spotify:track:xxx" }

// Response (sofort, Download läuft async im Hintergrund)
{ "id": "uuid", "status": "downloading", "title": "Dancing Queen", "artist": "ABBA" }
```

#### `GET /download/{id}`

```json
// Response
{
  "id": "uuid",
  "status": "ready",
  "file_path": "/opt/downloads/tracks/ABBA - Dancing Queen.mp3"
}
// Mögliche status: "pending", "stage1_metadata", "stage2_deemix", "stage3_spotdl", "stage4_tagging", "ready", "failed"
```

#### `GET /health`

```json
{ "status": "ok", "deemix_authenticated": true, "spotify_token_valid": true }
```

### Python-Projekt-Struktur (Hetzner)

```
/opt/download-service/
├── main.py              # FastAPI app
├── pipeline.py          # Stage 1→2→3→4 Logik
├── deemix_client.py     # deemix CLI wrapper
├── spotdl_client.py     # spotDL CLI wrapper
├── spotify_client.py    # Client Credentials Flow + Metadata
├── tagger.py            # mutagen ID3 tagging
├── requirements.txt     # fastapi, httpx, mutagen
├── config.toml          # ARL, Spotify Creds, Cookie-Path
└── downloads/
    └── tracks/          # Fertige Dateien, von Mutagen gesynct
```

---

## Momos-Music-Manager (MacBook)

### Änderungen am bestehenden Projekt

Momos-music-manager wird **nicht** selbst downloaden. Es managed nur:

- Request-Queue (Datenbank)
- Gäste-Suche (Spotify API via bestehende OAuth)
- DJ-Dashboard
- Delegation an Hetzner-Download-Service

### Datenbank: `migrations/022_song_requests.sql`

```sql
CREATE TABLE IF NOT EXISTS song_requests (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    guest_name TEXT,
    session_id TEXT NOT NULL,
    title TEXT NOT NULL,
    artist TEXT,
    album TEXT,
    source TEXT NOT NULL,          -- 'spotify'
    source_id TEXT NOT NULL,       -- spotify:track:xxx
    source_url TEXT,               -- https://open.spotify.com/track/xxx
    cover_url TEXT,
    duration_ms INTEGER,
    isrc TEXT,

    status TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending','downloading','ready','played','rejected','failed')),

    -- Hetzner download tracking
    download_task_id TEXT,         -- UUID vom Hetzner-Service
    download_path TEXT,            -- lokaler Pfad nach Sync
    file_id INTEGER REFERENCES files(id),

    priority INTEGER NOT NULL DEFAULT 0,
    dj_notes TEXT,
    requested_at INTEGER NOT NULL DEFAULT (unixepoch()),
    downloaded_at INTEGER,
    played_at INTEGER,
    ip_hash TEXT
);

CREATE INDEX IF NOT EXISTS idx_song_requests_status ON song_requests(status);
CREATE INDEX IF NOT EXISTS idx_song_requests_session ON song_requests(session_id);

SELECT 'Migration 022 applied: song_requests table' as status;
```

### Config: `config.toml` Erweiterung

```toml
[song_requests]
enabled = true
max_per_session = 5
max_total_pending = 50
require_access_code = false
access_code = ""

# Hetzner Download-Service
download_service_url = "http://hetzner:8000"   # über Tailscale
```

### API Endpoints (MacBook)

#### `POST /api/requests/search`

```json
// Request
{ "query": "ABBA Dancing Queen", "limit": 10 }

// Response (nur Spotify, da wir nur Spotify-Suche brauchen für Gäste)
{ "data": { "results": [
  { "id": "spotify:track:xxx", "title": "Dancing Queen",
    "artist": "ABBA", "album": "Arrival", "coverUrl": "...",
    "durationMs": 232000 }
]}}
```

**Implementierung**: Nutzt bestehenden `SpotifyClient` (OAuth) zur Suche.
Keine Deezer-Suche nötig — der Hetzner-Service resolved den Download separat.

#### `POST /api/requests`

```json
// Request
{ "sourceId": "spotify:track:xxx", "guestName": "Anna" }

// Response
{ "data": { "id": 42, "status": "downloading" }}
```

**Handler-Logik**:

1. Rate-Limiting + Access-Code prüfen
2. INSERT in `song_requests` mit `status='pending'`
3. `POST http://hetzner:8000/download {"url": "spotify:track:xxx"}` → `download_task_id`
4. Status auf `'downloading'` setzen

#### `GET /api/requests` — DJ-Dashboard

```
?status=pending,downloading,ready&sort=priority&order=desc
```

#### `PUT /api/requests/{id}` — DJ-Aktionen

```json
{ "status": "played" }
// oder: { "status": "rejected", "djNotes": "Nicht mein Style" }
```

#### `POST /api/requests/sync` — Status-Sync (Maintainer oder manuell)

Pollt alle `status='downloading'` Requests gegen den Hetzner-Service:

```
GET http://hetzner:8000/download/{download_task_id}
```

Wenn `status='ready'`: Update auf `ready`, `downloaded_at` setzen.
Wenn `status='failed'`: Update auf `failed`.

#### `GET /api/requests/stats` + `GET /api/requests/config`

Wie in Plan v1.

### Frontend

**Gäste-Seite (`#request`)**:

- Spotify-Suche (Typeahead, Debounce)
- Ergebnisse mit Cover-Art
- "Request" Button → `POST /api/requests`
- Counter "Noch X von 5"
- Session-ID via `localStorage`

**DJ-Dashboard (`#requests`)**:

- Queue mit Status-Filtern
- Drag/Priority
- Mark as played / reject
- QR-Code Modal
- Stats-Bar

→ Details wie in Plan v1, nur ohne Download-Fortschritt (der läuft remote).

---

## Sync vom Hetzner zum MacBook

### Empfohlen: Mutagen (Continuous Sync)

```bash
# Einmalig auf dem MacBook:
mutagen sync create \
  --name=dj-requests \
  --mode=one-way-safe \
  --ignore-vcs \
  user@hetzner:/opt/download-service/downloads/tracks \
  ~/Music/DJ/Incoming
```

**Vorteile gegenüber rsync-Cron**:

- ~1-2s Latenz statt 0-60s
- Persistente SSH-Verbindung (kein Overhead)
- Filesystem-Watching (erkennt neue Dateien sofort)
- Funktioniert über Tailscale/WireGuard

**Alternative (einfacher, kein extra Tool)**: rsync-Cron alle 30s

```bash
* * * * * rsync -az user@hetzner:/opt/download-service/downloads/tracks/ ~/Music/DJ/Incoming/
* * * * * sleep 30 && rsync -az user@hetzner:/opt/download-service/downloads/tracks/ ~/Music/DJ/Incoming/
```

→ Bis zu 30s Latenz, aber keine zusätzliche Dependency.

### Integration mit Folder Watcher

Der bestehende Folder Watcher (alle 5 Min) erkennt neue Dateien in `~/Music/DJ/Incoming/`
→ Extraktion von BPM/Key → in Traktor sichtbar.

---

## Implementierungs-Reihenfolge

### Phase 1: Download-Service (Hetzner, Python)

1. FastAPI-Gerüst + `/health` Endpoint
2. `spotify_client.py`: Client Credentials → Track Metadata (Stage 1)
3. `deemix_client.py`: ISRC-Lookup + deemix CLI-Wrapper (Stage 2)
4. `spotdl_client.py`: spotDL CLI-Wrapper (Stage 3)
5. `tagger.py`: ID3-Tagging mit mutagen (Stage 4)
6. `pipeline.py`: Orchestrator Stage 1→2→3→4
7. `POST /download` + `GET /download/{id}` Endpoints
8. Tests mit echten Spotify-URLs

### Phase 2: Sync (Hetzner → MacBook)

9. Mutagen installieren + Sync-Session anlegen
10. Verifizieren: Datei auf Hetzner erscheint auf MacBook in <5s

### Phase 3: Momos-Music-Manager Backend

11. Migration `022_song_requests.sql`
12. `src/db/requests.rs`: CRUD-Funktionen
13. `src/api/requests.rs`: Alle Handler + Router
14. `POST /api/requests/search` (Spotify via bestehenden Client)
15. `POST /api/requests` (Delegation an Hetzner)
16. `POST /api/requests/sync` (Status-Poll)
17. `src/config.rs`: `SongRequestsConfig`
18. `src/maintainer.rs`: Auto-Sync als Maintainer-Check
19. Integration-Tests

### Phase 4: Frontend

20. `frontend/pages/request.js` — Gäste-Seite (Spotify-Suche, Request)
21. `frontend/pages/requests.js` — DJ-Dashboard (Queue, Aktionen)
22. Nav-Registrierung + Styles
23. Playwright-Tests

### Phase 5: Polish

24. Access-Code-Feature
25. QR-Code-Generator
26. Audio-Previews
27. YouTube/SoundCloud-Suche als zusätzliche Quellen
28. Traktor-History-Integration

---

## Dateien-Übersicht

### Neue Dateien (gesamt)

| Pfad                               | Typ    | Inhalt                                         |
| ---------------------------------- | ------ | ---------------------------------------------- |
| `migrations/022_song_requests.sql` | SQL    | `song_requests` Tabelle                        |
| `src/db/requests.rs`               | Rust   | DB-Funktionen für Requests                     |
| `src/api/requests.rs`              | Rust   | Request-API-Handler + Router                   |
| `frontend/pages/request.js`        | JS     | Gäste-Seite (Suche + Request)                  |
| `frontend/pages/requests.js`       | JS     | DJ-Dashboard                                   |
| `frontend/tests/request.spec.js`   | JS     | Playwright Gäste-Seite                         |
| `frontend/tests/requests.spec.js`  | JS     | Playwright DJ-Dashboard                        |
| `tests/api_requests.rs`            | Rust   | Integration-Tests                              |
| `/opt/download-service/`           | Python | **Hetzner Download-Service (separates Repo!)** |

### Zu modifizierende Dateien

| Datei                    | Änderung                               |
| ------------------------ | -------------------------------------- |
| `src/config.rs`          | `SongRequestsConfig`                   |
| `src/api/mod.rs`         | `pub mod requests;` + merge            |
| `src/main.rs`            | Router merge                           |
| `src/maintainer.rs`      | Status-Sync als Check                  |
| `frontend/app.js`        | `"request"` + `"requests"` in PAGE_MAP |
| `frontend/shared/nav.js` | `"Requests"` in TOOLS_ITEMS            |
| `frontend/style.css`     | Request-Seiten Styles                  |

---

## Offene Fragen

1. **Mutagen vs. rsync-Cron**: Mutagen ist schöner, aber ein extra Tool. rsync-Cron ist simpler, aber 30-60s Latenz. Was ist dir wichtiger für den DJ-Flow?
2. **Download-Format**: 320kbps MP3 (deemix) oder FLAC? Für Hochzeiten reicht MP3 meistens.
3. **Fail-Handling**: Wenn das MacBook offline ist (kein Sync), aber der Hetzner schon fertig — wie erfährt der DJ davon? Polling mit Timeout?
4. **Access Code**: Soll der defaultmäßig an sein? Ich würde sagen: optional, per Config.
5. **Separate Repos?**: Download-Service als eigenes Git-Repo, oder in `momos-music-manager/` als `download-service/` Subdir?

---

**References**: Siehe [layerd_download_reasearch.md](./layerd_download_reasearch.md) für ausführliche Quellenangaben.
