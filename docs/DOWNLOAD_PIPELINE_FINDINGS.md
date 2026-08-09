# Download Pipeline — Erkenntnisse & Integration

> Stand: 2026-07-17
> Alle Fixes und Learnings vom End-to-End-Test auf `projectmellon.de`

---

## 1. Server-Setup

| Aspekt          | Erkenntnis                                              |
| --------------- | ------------------------------------------------------- |
| **Python venv** | Debian/Ubuntu braucht `apt install python3.12-venv`     |
| **Port**        | 8000 von Docker belegt → Ausgewichen auf **8700**       |
| **dufs**        | Liegt unter `~/.cargo/bin/dufs`, Port **8321** ist frei |
| **Config-Pfad** | `~/.config/download-service/config.toml` (nie im Repo)  |
| **deemix ARL**  | `~/.config/deemix/.arl` (nicht via `--arl` Flag)        |
| **Output**      | `/opt/download-service/downloads/tracks/`               |

### systemd Unit

```ini
[Service]
User=momo
WorkingDirectory=/srv/download-service
Environment="DOWNLOAD_SERVICE_CONFIG=/home/momo/.config/download-service/config.toml"
Environment="PATH=/srv/download-service/.venv/bin:/usr/local/bin:/usr/bin:/bin"
ExecStart=/srv/download-service/.venv/bin/uvicorn main:app --host 0.0.0.0 --port 8700
```

---

## 2. deemix CLI — Gepatchte Bugs

### 2.1 `--arl` Flag existiert nicht mehr

**Vorher (falsch):**

```bash
python3 -m deemix --bitrate 320 --arl $ARL $URL
```

**Jetzt (korrekt):**

```bash
python3 -m deemix --bitrate 320 --path $DIR $URL
```

ARL wird aus `~/.config/deemix/.arl` gelesen. Das File muss vorher angelegt werden:

```bash
echo "$ARL" > ~/.config/deemix/.arl
```

### 2.2 `deezer/utils.py:373` — `MEDIA[0]` KeyError

Manche Tracks haben kein `MEDIA`-Array → Crash bei `track['MEDIA'][0]['HREF']`.

**Fix:**

```python
# deezer/utils.py line 373
# Vorher:
result['preview'] = track['MEDIA'][0]['HREF']
# Nachher:
result['preview'] = track.get('MEDIA', [{}])[0].get('HREF', '') if track.get('MEDIA') else ''
```

### 2.3 `deemix/utils/pathtemplates.py:158` — `track.album.barcode` ist None

**Fix:**

```python
# Vorher:
filename = filename.replace("%upc%", track.album.barcode)
# Nachher:
filename = filename.replace("%upc%", track.album.barcode or "")
```

### 2.4 `deemix/tagger.py:69` — `TXXX("BARCODE", None)` → ValueError

**Fix:**

```python
# Vorher:
tag.add(TXXX(desc="BARCODE", text=track.album.barcode))
# Nachher:
tag.add(TXXX(desc="BARCODE", text=track.album.barcode or ""))
```

### 2.5 deemix Config (`~/.config/deemix/config.json`)

Wichtige Settings für flache Output-Struktur (keine Artist/Album-Ordner):

```json
{
  "downloadLocation": "/opt/download-service/downloads/tracks",
  "createArtistFolder": false,
  "createAlbumFolder": false,
  "createCDFolder": false,
  "createSingleFolder": false,
  "maxBitrate": "3",
  "fallbackBitrate": true,
  "overwriteFile": "y"
}
```

---

## 3. spotDL CLI — Korrekte Syntax

spotDL 4.5.0 hat geänderte CLI-Syntax:

```bash
# FALSCH (v3):
spotdl download <url> --output <path> --bitrate 320 --format mp3

# KORREKT (v4.5.0):
spotdl download <url> --output <path> --bitrate 320k --format mp3
```

**Wichtig:** Bitrate mit `k`-Suffix (`320k`, nicht `320`). Ohne YouTube Premium max ~128kbps.

---

## 4. File-Dedup im Tagger

Vorher produzierten deemix und unser Stage-4-Tagger Duplikate (`Artist - Title (1).mp3`).

**Fix:** Wenn die kanonische Datei schon existiert, wird die Dateigröße verglichen.
Das größere File (bessere Qualität) bleibt, das kleinere wird gelöscht.

```python
if new_path.exists():
    existing_size = new_path.stat().st_size
    current_size = path.stat().st_size
    if current_size > existing_size:
        new_path.unlink()
        os.rename(path, new_path)
    else:
        path.unlink()
```

**Ergebnis:** 60 Tracks → 47 unique Files, 0 Duplikate, 0 Fehler.

---

## 5. Playlist-Download — Pipeline-Optimierung

Bei Playlists werden Stage 1 (Spotify Metadata) für ALLE Tracks in einem API-Call
geholt, dann wird Stage 1 pro Track geskippt. Spart 60 API-Calls bei 60 Tracks.

```
POST /download/playlist {"url": "..."}
  → Spotify API: alle Tracks + ISRCs (1 Call)
  → Pro Track: Stage 2 (deemix) parallel in Threads
  → Stage 4 (tagging + dedup)
```

---

## 6. Relevanz für momos-music-manager

### 6.1 Lücken in bestehenden Downloads

Der Rust `DeemixClient` (`src/deemix/client.rs`) nutzt die deemix-pyweb HTTP-API
(`POST /api/addToQueue`). Das ist ein anderer Pfad als unser CLI-Wrapper. Mögliche
Probleme:

| Problem                                        | Lösung aus der Pipeline                                                       |
| ---------------------------------------------- | ----------------------------------------------------------------------------- |
| ARL expired → `NotLoggedIn`                    | Der `ensure_action_success` retry-Mechanismus im Rust-Client macht das schon. |
| Download schlägt fehl (Track nicht auf Deezer) | **Kein Fallback** im Rust-Client. Könnte spotDL-Fallback übernehmen.          |
| Fehlende Metadaten (BPM, Key)                  | Pipeline taggt ISRC + Cover. BPM/Key muss Traktor/Folder-Watcher extrahieren. |
| Datei nicht gefunden nach Download             | `_parse_downloaded_path` scannt Output-Dir nach neuestem File.                |
| Deezer-URL-Format geändert                     | ISRC-Lookup via `api.deezer.com/track/isrc:{ISRC}` ist stabiler.              |

### 6.2 Was der Rust-Code übernehmen könnte

1. **ISRC-basierter Deezer-Lookup** statt URL-basiertem Download

   ```rust
   // Statt: POST /api/addToQueue mit Spotify-URL
   // Besser: GET https://api.deezer.com/track/isrc:{ISRC} → Deezer-URL → Download
   ```

2. **spotDL-Fallback** wenn Deezer nicht hat
   - Via `std::process::Command` (gleiches Pattern wie Python)
   - Oder via HTTP-Client zu unserem Download-Service

3. **File-Dedup** nach Download
   - Kanonischer Name: `{artist} - {title}.mp3`
   - Größeres File behalten bei Konflikt

4. **Stage-4-Tagging** (ID3 + Cover) als Rust-Funktion
   - `id3` crate statt Python `mutagen`

### 6.3 Integration: momos-music-manager → Download-Service

Statt deemix-pyweb direkt zu nutzen, könnte der Rust-Code den Download-Service
via HTTP aufrufen:

```rust
// src/api/requests.rs
let resp = reqwest::Client::new()
    .post("http://projectmellon.de:8700/download/playlist")
    .json(&json!({"url": spotify_playlist_url}))
    .send().await?;
```

Damit würde der Rust-Code den kompletten 4-Stage-Pipeline-Prozess nutzen
(deemix 320k → spotDL Fallback → Tagging → Dedup).

---

## 7. Gepatchte Dateien (Pfade auf dem Server)

| Datei                                                                                            | Patch                                                 |
| ------------------------------------------------------------------------------------------------ | ----------------------------------------------------- |
| `/srv/download-service/.venv/lib/python3.12/site-packages/deezer/utils.py:373`                   | `MEDIA[0]` → `.get('MEDIA', [{}])[0].get('HREF', '')` |
| `/srv/download-service/.venv/lib/python3.12/site-packages/deemix/utils/pathtemplates.py:158,197` | `track.album.barcode` → `track.album.barcode or ""`   |
| `/srv/download-service/.venv/lib/python3.12/site-packages/deemix/tagger.py:69`                   | `track.album.barcode` → `track.album.barcode or ""`   |
| `/srv/download-service/tagger.py`                                                                | Dedup-Logik (größeres File behalten)                  |

**Achtung:** Die venv-Patches gehen bei `pip install --upgrade` verloren!

---

## 8. Quick-Reference: Server-Kommandos

```bash
# Service
sudo systemctl restart download-service
sudo journalctl -u download-service -f

# API
curl http://localhost:8700/health | jq
curl http://localhost:8700/queue | jq
curl -X POST http://localhost:8700/download/playlist \
  -H 'Content-Type: application/json' \
  -d '{"url": "https://open.spotify.com/playlist/..."}'

# File-Browser
# http://projectmellon.de:8321

# Output leeren
rm -f /opt/download-service/downloads/tracks/*.mp3

# Nach pip-Upgrade: Patches neu anwenden
cd /srv/download-service
source .venv/bin/activate
# ... (siehe Section 2)
```

### 6.4 Konkrete Rust-Luecken & Fixes

#### `DeemixClient::ensure_queued()` - kein Einzeltrack-Download

Aktuell (`src/deemix/client.rs:196`):
```rust
pub async fn ensure_queued(&self, spotify_url: &str) -> Result<()> {
    // Sucht in der Queue nach playlist-URL
    let item_url = format!("https://open.spotify.com/playlist/{}", item.id);
```

Problem: Funktioniert nur fuer Playlists, nicht fuer einzelne Tracks.
Ein `spotify:track:xxx` wird nicht erkannt.

**Fix-Idee:** Vor dem Queue-Add den Track via ISRC auf Deezer lookupen und
Deezer-Track-URL an deemix schicken:
```rust
// 1. ISRC via Spotify API holen (Client Credentials)
// 2. GET https://api.deezer.com/track/isrc:{ISRC}
// 3. Deezer-URL an deemix: POST /api/addToQueue {"url": "https://www.deezer.com/track/..."}
```

#### `DeemixQueueItem` Deserialisierung - kann bei null-Feldern brechen

Aktuell (`src/deemix/models.rs`): `deserialize_nullable_string` existiert, aber
nur fuer `item_type`, `uuid`, `title`, `artist`, `collection_type`, `status`.

Wenn deemix Felder als `null` statt leer schickt (passiert bei neueren Versionen),
crasht die Deserialisierung. Gleiches Problem wie Section 2.2 (`MEDIA[0]` KeyError).

**Fix:** `#[serde(default)]` auf ALLEN nicht-optionalen Feldern.

#### `DeemixDownloadedFile` - keine Existenz-Pruefung

Die Rust-Modelle tracken `download_path`, aber es gibt keine Verifikation ob die
Datei wirklich auf Disk existiert. Der Folder-Watcher findet sie irgendwann, aber
es gibt kein direktes Feedback nach dem Download.

**Fix-Idee:** Nach Queue-Poll (wenn `status == "finished"`) den `download_path`
mit `std::fs::metadata()` pruefen und Dateigroesse verifizieren.
