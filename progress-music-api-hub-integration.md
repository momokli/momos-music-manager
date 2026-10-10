# Progress: music-api ↔ mmm-hub Integration (YouTube/SoundCloud + Downloads)

- Repo: momokli/momos-music-manager
- Datum: 2026-10-10
- Branches: `feat/music-api` (music-api), `feat/mmm-hub` (hub)
- Deployed: music-api + mmm-hub auf `music-catalog` (192.168.178.200)

## Scope

Metadaten-Import (YouTube/SoundCloud) in den mmm-hub + music-api als
universeller Media-Acquisition-Service (Deezer/YouTube/SoundCloud) mit
Fallback-Kette, Queue-Steuerung und Status-Sichtbarkeit im Hub.

## Issues

| # | Titel | Status |
|---|---|---|
| #233 | fix(music-api): FLAC-Download wird nicht durchgeführt | offen (ARL war Free → jetzt Premium) |
| #234 | feat(music-api): URL-Orders für YouTube/SoundCloud (yt-dlp) | ✅ closed |
| #235 | feat(music-api): Metadaten-API für alle Provider | ✅ closed |
| #236 | feat(music-api): Queue-Steuerung von außen | ✅ closed |
| #237 | feat(hub): SoundCloud Scraping (Sets + Likes) | ✅ closed |
| #238 | feat(hub): YouTube-Import vervollständigen | ✅ closed |
| #243 | feat(music-api): Fallback auf yt-dlp/spotDL | ✅ closed |
| #246 | feat(hub): Downloads-Tabelle | ✅ closed |
| #248 | feat(hub): Playlist priorisieren + Fehlergrund | ✅ closed |

## PRs (alle gemergt)

| PR | Branch | Inhalt |
|---|---|---|
| #239 | `feat/music-api-url-orders` | URL-Orders, Metadaten-API, Queue-Steuerung, ARL-Tier-Check |
| #240 | `feat/hub-soundcloud-scrape` | SoundCloud api-v2 Ingest (Sets + Likes) |
| #241 | `feat/hub-import-ui` | Import-UI `/import` + YouTube komplett |
| #242 | `feat/hub-music-status` | Format pro Track + `/downloads` Queue-View |
| #244 | `feat/music-api-fallback` | yt-dlp-Fallback + Deezer-Lookup-Fix |
| #245 | `feat/music-api-spotdl` | spotDL-Fallback-Provider |
| #247 | `feat/hub-downloads-table` | Downloads-Tabelle (alle Tracks, Filter, Pagination) |
| #249 | `feat/music-api-order-priority` | `POST /orders` mit `priority` |
| #250 | `feat/hub-prioritize` | Playlist priorisieren + Fehlergrund pro Track |

## music-api — neue Features (deployed)

- **URL-Orders**: `POST /url-orders`, `GET /url-orders/{id}` (YouTube/SoundCloud via yt-dlp)
- **Metadaten-API**: `GET /metadata/track`, `GET /metadata/playlist`
- **Queue-Steuerung**: `POST /orders/{id}/prioritize`, `POST /orders/{id}/status`, `GET /queue`
- **Order-Priority**: `POST /orders` akzeptiert `priority`
- **ARL-Tier-Check** beim Start (`deezer::check_arl_tier`)
- **Fallback-Kette**: Deezer → yt-dlp (`ytsearch1:`) → spotDL
- **Deezer-Lookup-Fix**: `DeezerArtist.name` optional (`missing field 'name'`)
- Neue Tabellen: `url_tracks`, `url_orders`, `url_order_items`; `tracks.priority`, `orders.priority`

## mmm-hub — neue Features (deployed)

- **SoundCloud-Ingest** (`src/soundcloud.rs`): api-v2 mit gescrapeter `client_id`
  (Sets, Likes, alle Sets)
- **Import-UI** `/import`: Service + URL, Flash-Message, Playlist-Liste
- **`/downloads`**: Cache-Übersicht + Live-Queue + **Tabelle aller Hub-Tracks**
  (78512) mit Filter (Status/Format/Suche), Pagination, „Alles ordern"
- **Format pro Track**: `hub_music_state.source_format` (flac/mp3-320/mp3-128)
- **Fehlergrund pro Track**: `hub_music_state.error` (Migration 033)
- **Playlist priorisieren**: `POST /playlist/{id}/prioritize` (Priorität 10)
- Migrationen: 028 (format), 033 (error)

## FLAC-Problem (#233) — gelöst

**Ursache:** Der ARL in `/opt/music-api/music-api.env` war **Deezer Free**
(`OFFER: Deezer Free`, `lossless: false`). deemix fiel auf 128 kbps zurück.

**Fix:** Premium-ARL eingetragen (`Deezer Student`, `lossless: true`) + music-api
neu gestartet. Verifiziert: `USQX91201487` → `ready`, `sourceFormat: flac`,
`formats: [flac, 320, 128]`.

## Deploy-Erkenntnisse

- Der `tar`-Deploy aus `mmm-hub/AGENT.md` ist **unzuverlässig** (kopiert nur
  einen Teil der Dateien). **`rsync`** verwenden:
  ```bash
  rsync -az --exclude target --exclude 'hub.db*' --exclude .env \
    --exclude 'datasette-*' --exclude 'make-public*' --exclude docs \
    --exclude models --exclude tmp --exclude .git \
    mmm-hub/ music-catalog:~/mmm-hub/
  ```
- **`rsync` ohne `--delete`** lässt veraltete Dateien liegen → Migrations-Konflikt
  (`migration 29 was previously applied but has been modified`). Empfehlung:
  `--delete` (mit Ausnahmen) + `cargo build` **immer** nach dem Sync.
- SQLx **embedded** die Migrationen ins Binary → nach Migrations-Änderungen
  **neu bauen**, sonst liest SQLx die alte Version.
- Der Server hat Migrationen **029–032** (von einem anderen Feature), die
  `feat/mmm-hub` nicht hat. Migration 029 war belegt → `033_music_state_error.sql`.

## Offene Punkte

- **yt-dlp 403 Forbidden** beim Download (YouTube-Bot-Erkennung). Suche
  funktioniert, Download nicht. Braucht `YTDLP_COOKIES` (cookies.txt) oder Proxy.
- **spotDL-Pfad** gesetzt (`SPOTDL=/home/momo/.local/bin/spotdl`), aber noch nicht
  end-to-end verifiziert (wartet auf einen `not streamable`-Track).
- **Re-Order läuft**: 24096 Tracks (`failed` + `absent: not streamable`) auf
  `pending` zurückgesetzt. ~300 Tracks/Minute → ~80 Minuten.
- **`absent: no data` (3467)**: Deezer kennt den ISRC nicht → Fallback greift
  nicht (keine Metadaten). Bräuchte ISRC→Metadaten-Quelle (MusicBrainz).
- **Alte deemix-Queue**: 40938 `completed`-Dateien könnten aufgeräumt werden.
- **AGENT.md Deploy-Loop** auf `rsync` umstellen.

## Verifikation

- `music-api`: `cargo build` + `cargo test` grün (18 unit + 9 + 11 integration)
- `mmm-hub`: `cargo build` + `cargo test` grün (25 lib + 105 integration)
- Live: `https://hub.zukkafabrik.de/downloads` (HTTP 200), `/import` (HTTP 200),
  Playlist „Priorisieren"-Button vorhanden
- music-api-Log: `ARL tier: Deezer Student (lossless available)`,
  `DEEC31850133: ready (flac + 320 + 128)`
