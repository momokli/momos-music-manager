# Momo's Music Manager

Multi-service music library management for DJs — local files + streaming services (Spotify) mit harmonischem Matching, Tag-Organisation und Traktor-Integration.

> **Status**: RC — Funktional, single-binary, served from one port
> **Branch**: `momos-music-manager`

---

## Quick Start

```bash
# Ein Prozess, ein Port
cargo run -- serve

# Browser öffnen
open http://localhost:3000
```

### Voraussetzungen

- Rust 1.80+ (und Cargo)
- SQLite 3.x
- Spotify Developer Account (für OAuth — optional, nur für Sync)

**Kein Python, kein separater Dev-Server, kein Node.js nötig.**  
Der gesamte Frontend-Code (HTML/JS/CSS/Images/Fonts) ist im Rust-Binary eingebettet.

### Konfiguration

```bash
# Optional: .env für Spotify Credentials
cp example.env .env
# Oder persistenter: ~/.config/momos-music-manager/config.toml
```

---

## DJ Workflow

```mermaid
flowchart LR
    Spotify -->|Download| FLAC
    FLAC -->|NUO-STEMS| STEMS
    STEMS -->|Taggen| Kommentiert
    Kommentiert -->|Consistency Check| Traktor
```

### 1. Spotify → FLAC downloaden

Playlist-URL von Spotify kopieren → in Download-Queue einfügen → FLACs landen in `~/Music/flacs/`

### 2. FLAC → STEMS konvertieren

NUO-STEMS öffnen → Ordner `~/Music/flacs` hinzufügen → "Skip duplicates?" → Start → STEM-Dateien erscheinen in `~/Music/stems/`

### 3. Metadata taggen (Comment schreiben)

Der Server schreibt beim Sync automatisch Playlist-Informationen in den ID3-Comment:

```
[{Phase}{Mood}{Vibe}] {tags} {source_id}
```

Beispiel: `[PMV] build jazzy warehouse sp:1WSF0LJGwJkYejuMtyJVuA`

### 4. In Traktor importieren

Traktor öffnen → "Consistency Check" über alle Tracks → Comments sichtbar → Smart-Lists möglich

---

## SPA Pages

| Route              | Beschreibung                               |
| ------------------ | ------------------------------------------ |
| `#dashboard`       | Stats + Services-Dashboard                 |
| `#files`           | Lokale Dateien mit BPM/Key, Comment-Status |
| `#tracks`          | Service-Tracks (Spotify)                   |
| `#playlists`       | Alle Service-Playlists                     |
| `#tags`            | Tags (paginierte Liste)                    |
| `#tag-categories`  | Tag-Kategorien                             |
| `#services`        | Service-Status + OAuth/Sync                |
| `#folders`         | Überwachte Ordner verwalten                |
| `#tasks`           | Task-Manager (Sync/Write-Comment Jobs)     |
| `#auto-categorize` | AI Tag Categorization Wizard               |
| `#traktor-import`  | Traktor Collection Import                  |
| `#digging`         | Digging Curator — Chain-basierte Sessions  |

---

## CLI

```bash
# Server starten (embedded Frontend + API)
cargo run -- serve --host 127.0.0.1 --port 3000

# Version anzeigen
cargo run -- --version

# Database status
cargo run -- db-status

# Single File scannen (Debug)
cargo run -- scan-file ~/Music/stems/example.stem.m4a

# Directory scannen und importieren
cargo run -- scan ~/Music/stems/

# DB Dump/Restore (JSON)
cargo run -- dump
cargo run -- restore
```

---

## Dev Commands

```bash
# Clean DB (bei Schema-Änderungen)
rm -f app.db && cargo run -- serve

# Spotify API Cache (record/replay)
SPOTIFY_API_CACHE=record cargo run -- serve
SPOTIFY_API_CACHE=replay cargo run -- serve

# Scan Cache (record/replay)
SCAN_CACHE=record cargo run -- serve
SCAN_CACHE=replay cargo run -- serve

# Cache löschen
rm -rf dev-data/spotify-api
rm -rf dev-data/scan-cache
```

---

## Projekt-Struktur

```
momos-music-manager/
├── src/                     # Rust Backend
│   ├── main.rs              # CLI, Router, embedded Frontend
│   ├── api.rs               # Alle API Endpoints
│   ├── db.rs                # Datenbank-Queries, Scanning, Comment-Computation
│   ├── comment.rs           # Comment-Parsing/-Generierung
│   ├── config.rs            # Config.toml + Env-Var Loading
│   ├── embeddings.rs        # Semantische Tag-Embeddings (candle/ML)
│   ├── digging.rs           # Curator/Session-Builder
│   ├── audio_extensions.rs  # AudioExtension Enum
│   ├── scan_cache.rs        # File Scan Caching
│   ├── dump.rs              # DB Dump/Restore (JSON)
│   ├── poller.rs            # Playlist Subscription Poller
│   ├── traktor.rs           # Traktor collection.nml Parser
│   ├── watch.rs             # Folder Watcher (optional)
│   ├── spotify/             # Spotify OAuth + Sync
│   │   ├── client.rs
│   │   ├── models.rs
│   │   ├── replay.rs
│   │   └── sync_worker.rs
│   └── tasks/               # TaskManager + Worker
│       └── mod.rs
├── frontend/                # SPA (embedded via rust-embed)
│   ├── index.html           # Shell
│   ├── app.js               # Hash-Router
│   ├── style.css            # Styles
│   ├── fontawesome/         # Local Font Awesome Bundle
│   ├── shared/              # Wiederverwendbare Module
│   │   ├── api.js
│   │   ├── components.js
│   │   ├── format.js
│   │   ├── nav.js
│   │   └── search-filter.js
│   └── pages/               # Eine JS-Modul pro Route
│       ├── dashboard.js
│       ├── files.js
│       ├── tracks.js
│       ├── playlists.js
│       ├── tags.js
│       ├── tag-categories.js
│       ├── services.js
│       ├── tasks.js
│       ├── folders.js
│       ├── auto-categorize.js
│       ├── digging.js
│       └── traktor-import.js
├── migrations/
│   └── 001_initial_schema.sql  # Single Migration
└── docs/
    ├── ARCHITECTURE.md      # System-Architektur
    ├── COMMENT_SYSTEM.md    # Comment-Format Spezifikation
    ├── DECISIONS.md         # Architectural Decision Records
    └── TASK_MANAGER.md      # Task-Manager Design
```

---

## API Endpoints

| Gruppe    | Base Path                               |
| --------- | --------------------------------------- |
| Files     | `GET/POST /api/files`                   |
| Tracks    | `GET /api/tracks`                       |
| Playlists | `GET /api/playlists`                    |
| Tags      | `CRUD /api/tags`, `/api/tag-categories` |
| Folders   | `CRUD /api/folders`                     |
| Services  | `GET/POST /api/services/{service}/...`  |
| Tasks     | `GET/DELETE /api/tasks`                 |
| Digging   | `GET/POST /api/digging/...`             |
| Health    | `GET /api/health`                       |

Detail siehe [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md).

---

## Docs

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) — System-Design
- [`docs/DECISIONS.md`](docs/DECISIONS.md) — Technische Entscheidungen
- [`docs/COMMENT_SYSTEM.md`](docs/COMMENT_SYSTEM.md) — Comment-Format
- [`docs/TASK_MANAGER.md`](docs/TASK_MANAGER.md) — Task-Manager Architektur

---

## License & Repo

Fork von Spotify Mirror.  
`git@git.sr.ht:~momoy/momos-music-manager`
