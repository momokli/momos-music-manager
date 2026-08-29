# Download Service

4-stage download pipeline for the Wedding DJ Request Flow.

## Quick Start

```bash
cd download-service

# 1. Credentials (Priority: Env-Vars > ~/.config/ > local TOML)
#
# Variante A: TOML-Datei (empfohlen, ausserhalb des Repos)
mkdir -p ~/.config/download-service
cp config.example.toml ~/.config/download-service/config.toml
vim ~/.config/download-service/config.toml  # Creds eintragen
#
# Variante B: Environment Variables (gut fuer Docker/systemd)
# export DOWNLOAD_DEEMIX_ARL="..."
# export DOWNLOAD_SPOTIFY_CLIENT_ID="..."
# export DOWNLOAD_SPOTIFY_CLIENT_SECRET="..."

# 2. Dependencies installieren
pip install -r requirements.txt
pip install deemix spotdl

# 3. Starten
uvicorn main:app --host 0.0.0.0 --port 8000

# 4. Testen
curl http://localhost:8000/health
curl -X POST http://localhost:8000/download -H 'Content-Type: application/json' \
  -d '{"url": "spotify:track:4cOdK2wGLETKBW3PvgPWqT"}'
```

## Credentials (wo werden sie gespeichert?)

**Nie im Repo.** Drei Optionen, Prioritaet von oben nach unten:

| Priority     | Ort                                                                | Gut fuer                                                  |
| ------------ | ------------------------------------------------------------------ | --------------------------------------------------------- |
| 1 (hoechste) | Env-Vars: `DOWNLOAD_DEEMIX_ARL`, `DOWNLOAD_SPOTIFY_CLIENT_ID`, ... | Docker, systemd, CI                                       |
| 2            | `~/.config/download-service/config.toml`                           | **Empfohlen** -- ausserhalb des Repos, wird nie committed |
| 3            | `download-service/config.toml`                                     | Dev/Local -- in `.gitignore`                              |

### ARL (Deezer)

Im Browser bei Deezer einloggen -> DevTools -> Application -> Cookies -> `arl`-Wert kopieren.

### Spotify Client Credentials

[Developer Dashboard](https://developer.spotify.com/dashboard) -> Create App -> Client ID + Secret.

## Pipeline Stages

| Stage | Name     | Tool                             | Beschreibung                           |
| ----- | -------- | -------------------------------- | -------------------------------------- |
| 1     | Metadata | Spotify API (Client Credentials) | ISRC, Title, Artist, Cover extrahieren |
| 2     | Deezer   | deemix CLI (320kbps)             | ISRC-Lookup auf Deezer -> Download     |
| 3     | YouTube  | spotDL CLI (128kbps)             | Fallback wenn nicht auf Deezer         |
| 4     | Tagging  | mutagen                          | ID3-Tags + Cover-Art einbetten         |

## API

### `POST /download`

```json
{"url": "spotify:track:xxx"}
-> {"id": "uuid", "status": "pending"}
```

### `POST /download/playlist`

```json
{"url": "https://open.spotify.com/playlist/..."}
-> {"playlist_id": "...", "track_count": 42, "tasks": [...]}
```

### `POST /download/batch`

```json
{"urls": ["spotify:track:xxx", "spotify:track:yyy"]}
-> {"total": 2, "tasks": [...]}
```

### `GET /download/{id}`

```json
-> {"id": "uuid", "status": "ready", "title": "...", "artist": "...", "file_path": "/opt/..."}
```

Status-Werte: `pending` -> `stage1_metadata` -> `stage2_deemix` -> `stage3_spotdl` -> `stage4_tagging` -> `ready` | `failed`

### `GET /queue`

```json
-> {"total": 15, "tasks": [...]}
```

### `GET /health`

```json
-> {"status": "ok", "deemix_arl_configured": true, ...}
```

## Deployment

```bash
# Auf dem Server:
cd /srv/momos-music-manager/download-service

# Credentials anlegen
mkdir -p ~/.config/download-service
cp config.example.toml ~/.config/download-service/config.toml
vim ~/.config/download-service/config.toml

# Deploy
bash deploy.sh

# Starten
sudo systemctl start download-service
sudo systemctl start dufs-downloads  # optional
```
