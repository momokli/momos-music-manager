# Momo's Music Manager — Architecture Overview

> POC Phase — Rust backend (Axum/SQLx/SQLite) + pure HTML/JS frontend.

---

## Tech Stack

### Backend

| Layer          | Choice                                 |
| -------------- | -------------------------------------- |
| Runtime        | Rust (2024 edition) + Tokio            |
| Web Framework  | Axum 0.8.x                             |
| Database       | SQLite 3.x mit SQLx                    |
| Auth           | OAuth 2.0 (Spotify)                    |
| Logging        | `tracing` crate                        |
| Audio Metadata | exiftool (via `std::process::Command`) |

### Frontend (POC)

- **Pure HTML/CSS/JS** in `frontend/`
- Kein Build-Tool, kein Framework
- Dev-Server: `python3 -m http.server 8000`
- Kommunikation mit Backend via `fetch()` + REST API

---

## Database Schema

**Single Migration**: `migrations/001_initial_schema.sql`

| Table                     | Purpose                                  | BPM/Key? |
| ------------------------- | ---------------------------------------- | -------- |
| `tag_categories`          | Setlist, Phase, Mood, Vibe, Merkmal      | —        |
| `tags`                    | Tag-Katalog, UNIQUE name                 | —        |
| `service_tracks`          | Tracks aus Spotify/SoundCloud/YouTube    | ❌ Nein  |
| `service_playlists`       | Playlists von Services                   | —        |
| `service_playlist_tracks` | Many-to-Many Playlist ↔ Track            | —        |
| `files`                   | **Lokale Dateien** mit allen Metadaten   | ✅ Ja    |
| `service_config`          | OAuth-Tokens (keine Sync-Felder)         | —        |
| `folders`                 | Überwachte Ordner mit Scan-Konfiguration | —        |

### Patterns

- **File vs ServiceTrack**: Zwei getrennte Typen. `files` haben BPM/Key, `service_tracks` nicht. Keine Junction-Tables.
- **Service IDs direkt auf `files`**: `spotify_id`, `soundcloud_id`, `youtube_id` als direkte Spalten (keine Matches-Tabelle).
- **Tags = Playlists**: Tags werden per Name-Matching (case-insensitive) mit Playlist-Namen verknüpft.
- **Sync State in-memory**: Keine Sync-Felder in der DB — alles über `TaskManager` im RAM.

---

## API Struktur

**Kein `/api/v2/` Prefix mehr** — alle Endpoints unter `/api/`.

### Files

```
GET    /api/files                    # Paginierte Liste (50/Seite)
GET    /api/files/count              # Gesamtanzahl
GET    /api/files/{id}               # Detail
POST   /api/files/{id}/sync-comment  # WriteComment Task starten
POST   /api/files/bulk-sync          # Bulk WriteComment Task
```

### Tracks (Service-Tracks, keine lokalen Dateien)

```
GET    /api/tracks
GET    /api/tracks/count
GET    /api/tracks/{id}
```

### Playlists + Tags

```
GET    /api/playlists
GET    /api/playlists/{id}/tracks

GET/POST   /api/tags
PUT/DELETE /api/tags/{id}

GET/POST   /api/tag-categories
PUT/DELETE /api/tag-categories/{id}

GET    /api/tags/from-playlists       # Wizard: Playlists ohne Tags
POST   /api/tags/create-from-playlists
```

### Services (Spotify)

```
GET  /api/services
POST /api/services/{service}/auth
GET  /api/services/{service}/callback
POST /api/services/{service}/sync
GET  /api/services/{service}/sync/{task_id}
```

### Folders

```
GET/POST   /api/folders
GET/PUT/DELETE /api/folders/{id}
POST /api/folders/{id}/watch    # Toggle active
POST /api/folders/{id}/scan     # Scan-Job starten
```

### Tasks

```
GET    /api/tasks (paginated)
GET    /api/tasks/{id}
DELETE /api/tasks/{id}
```

### Sonstige

```
GET /api/health
GET /api/services/{service}/reset
```

---

## Frontend Pages

| Route                       | Beschreibung                             |
| --------------------------- | ---------------------------------------- |
| `/index.html`               | Services Dashboard                       |
| `/files.html`               | Lokale Dateien (BPM/Key, Comment-Status) |
| `/tracks.html`              | Service-Tracks                           |
| `/playlists.html`           | Alle Playlists                           |
| `/folders.html`             | Ordner-Verwaltung mit Scan-Konfiguration |
| `/tags.html`                | Tags (paginiert, suchbar)                |
| `/tag-categories.html`      | Tag-Kategorien                           |
| `/tags-from-playlists.html` | Wizard: Playlists → Tags                 |
| `/tasks.html`               | Task-Manager WebUI                       |

---

## Comment System

### Format

```
[{phase_char}{mood_char}{vibe_char}] {tags} {source_id}
```

Beispiel: `[PMV] build jazzy warehouse sp:1WSF0LJGwJkYejuMtyJVuA`

- **PMV**: Phase/Mood/Vibe — `P`, `M`, `V` oder `_` wenn kein Tag der Kategorie
- **Tags**: Leerzeichen-getrennt, sortiert nach Category-Priorität
- **Source IDs**: `sp:xxx`, `sc:xxx`, `yt:xxx`

### Target-Comment Computation

Für jede Datei wird berechnet, was der Comment **sein sollte**:

1. Matching Service-Tracks finden (via ISRC + Service-IDs)
2. Playlists dieser Tracks finden
3. Tags via Name-Matching finden
4. PMV-Chars aus den Category-Namen der Tags ableiten
5. Tags sortieren (Phase > Mood > Vibe > Merkmal > Setlist)
6. Service-IDs aus `files.spotify_id`/`soundcloud_id`/`youtube_id` sammeln
7. Formatieren → Target Comment

Die API liefert `comment_current`, `comment_target`, `comment_needs_update`.

---

## Task Manager

In-memory Task-Tracking, ersetzt den alten SyncManager.

```
TaskType::SpotifySync(SyncConfig)   → 4 Sync-Arten (playlists/tracks/full)
TaskType::WriteComment { file_ids }  → Single/Batch Write
```

- Tasks bleiben bis Server-Neustart im RAM
- API: `GET /api/tasks`, `GET /api/tasks/{id}`, `DELETE /api/tasks/{id}`
- Frontend pollt Task-Status und zeigt Spinner/Progress

---

## DJ Workflow (aktuell)

```
Spotify URL → Download-Tool → FLACs → NUO-STEMS → STEMs → Taggen → Traktor
```

1. **Download**: Playlist-URL in Download-Queue (`http://localhost:6596`)
2. **Konvertieren**: NUO-STEMS wandelt FLACs in STEM-Dateien
3. **Taggen**: Backend schreibt Playlist-Infos in ID3-Comment (via exiftool)
4. **Traktor**: Consistency Check liest Comments ein → Smart-Lists möglich

---

## Key Matching (Rust-only)

Camelot Wheel — **keine DB-Tabelle** dafür.

- Nur Nummer zählt (±1, wrapping 12↔1)
- A/B (Major/Minor) wird ignoriert
- Implementiert in `db.rs::check_key_compatibility`

---

## Wichtige Prinzipien

1. **Single Migration**: Nur `migrations/001_initial_schema.sql`. Bei Schema-Änderung: File ersetzen + DBs löschen.
2. **Keine Junction-Tables**: Tags ↔ Files wird computed, nicht stored.
3. **Tags = Single Source of Truth**: Keine direkte File-Tag-Zuordnung.
4. **.env Only**: Service-Credentials NUR in `.env`, nie in DB oder UI.
5. **POC Phase**: Keine Production Data, kein Backward-Compatibility, keine Migration-Paths.

---

## Verzeichnisstruktur

```
src/
├── main.rs              # CLI, Router, Server starten
├── api.rs               # Alle API-Endpoints
├── db.rs                # Datenbank-Funktionen (Queries, Scanning, Comment-Computation)
├── comment.rs           # Comment-Parsing/-Generierung
├── audio_extensions.rs  # AudioExtension Enum
├── config.rs            # AppConfig + ServiceCredentials (.env)
├── watch.rs             # Folder Watcher (optional, nicht auto-gestartet)
├── spotify/
│   ├── mod.rs
│   ├── client.rs
│   ├── models.rs
│   └── sync_worker.rs
├── sync/
│   └── mod.rs           # Sync-Typen (4 Sync-Operationen)
└── tasks/
    └── mod.rs           # TaskManager (generisch)

frontend/
├── index.html           # Hauptseite
├── files.html/js        # Dateien-Page
├── tracks.html/js       # Service-Tracks
├── playlists.html/js    # Playlists
├── folders.html/js      # Ordner
├── tags.html/js         # Tags
├── tag-categories.html/js
├── tags-from-playlists.html/js  # Wizard
├── tasks.html/js        # Task-Manager UI
└── style.css            # Styles
```

---

## Nächstes (nicht implementiert)

- SoundCloud/YouTube OAuth (Framework bereit, Flow fehlt)
- Docker Compose (wird später neu erarbeitet)
- Advanced Harmonic Matching (relative keys, extended intervals)
- Preset-Management
- Explorer-Page (Similarity-Feature)

---

_Last Updated: 2026-04-25_
