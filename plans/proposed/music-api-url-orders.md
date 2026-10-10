# Plan: music-api URL-Orders + Metadaten (YouTube/SoundCloud)

**Status**: approved
**Branch**: `feat/music-api-url-orders`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: yes (additive: `provider`, `provider_id`, `url`, `playlist_id`, `priority`)

### Description

Erweitert `music-api` von einem reinen ISRC/Deezer-Service zu einem
**universellen Media-Acquisition-Service**: URL-basierte Orders für YouTube und
SoundCloud via `yt-dlp`, eine Metadaten-API für alle Provider, und eine von außen
steuerbare Queue (Priorisierung, Pause/Resume). Behebt außerdem das FLAC-Problem.

Issues: #233 (FLAC), #234 (URL-Orders), #235 (Metadaten), #236 (Queue).

### Interface

#### `POST /url-orders`
```json
{ "items": [{ "url": "https://soundcloud.com/momokli/sets/discover" }] }
```
→ `{ "orderId": "...", "status": "open", "count": 1 }`

#### `GET /url-orders/{id}`
```json
{
  "orderId": "...",
  "status": "open",
  "items": [
    { "url": "...", "provider": "soundcloud", "providerId": "...",
      "state": "ready", "title": "...", "artist": "...",
      "formats": ["flac","320","128"] }
  ]
}
```

#### `GET /url-orders?status=open|done|partial&limit=50`

#### `GET /metadata/track?url=...`
```json
{ "provider": "youtube", "providerId": "...", "title": "...",
  "artist": "...", "durationMs": 300000, "url": "...", "isrc": null }
```

#### `GET /metadata/playlist?url=...`
```json
{ "provider": "soundcloud", "providerId": "sets/123", "name": "discover",
  "trackCount": 42, "tracks": [ { "providerId": "...", "title": "...",
  "artist": "...", "durationMs": 300000 } ] }
```

#### `POST /orders/{id}/prioritize` → `{ "priority": 10 }`
#### `PATCH /orders/{id}` → `{ "status": "paused" | "open" | "cancelled" }`
#### `GET /queue` → aktuelle Reihenfolge

### Schema (additiv)

`tracks` erweitern um:
- `provider` TEXT CHECK(provider IN ('deezer','youtube','soundcloud'))
- `provider_id` TEXT
- `url` TEXT
- `playlist_id` TEXT
- `priority` INTEGER NOT NULL DEFAULT 0

`orders` erweitern um:
- `priority` INTEGER NOT NULL DEFAULT 0
- `status` erweitern um `paused`, `cancelled`

### Files to modify

| File | Change |
|------|--------|
| `music-api/src/db.rs` | Schema + Queries (URL-Orders, Priorisierung) |
| `music-api/src/models.rs` | `UrlOrder`, `UrlOrderItem`, `MetadataTrack`, `MetadataPlaylist` |
| `music-api/src/api.rs` | Neue Routen |
| `music-api/src/ytdlp.rs` | **NEU**: yt-dlp-Integration (Metadaten + Download) |
| `music-api/src/worker.rs` | URL-Resolution + yt-dlp-Download-Phase |
| `music-api/src/config.rs` | `YTDLP`, `YTDLP_COOKIES`, `YTDLP_FORMAT` |
| `music-api/tests/` | Integrationstests |

### Acceptance Criteria

- [ ] `cargo build` passes
- [ ] `cargo test` passes
- [ ] `POST /url-orders` mit SoundCloud-Set-URL → Tracks geladen
- [ ] `POST /url-orders` mit YouTube-Playlist-URL → Tracks geladen
- [ ] `GET /metadata/playlist?url=...` liefert Tracks
- [ ] Priorisierung beeinflusst Download-Reihenfolge
- [ ] FLAC-Problem dokumentiert/behoben
