Ich habe jetzt alle notwendigen Informationen zusammengetragen. Hier ist die umfassende Evaluierung aller fünf Forschungsaspekte:

---

# Research Report: Layered Download Pipeline

## 1. Deemix Headless Integration

### Empfehlung: CLI-Wrapper statt direktem Modul-Import

**Direkter Python-Import ist möglich, aber fragil.** Das `deemix`-Paket kann als Python-Bibliothek importiert werden — das `deempy`-Projekt zeigt ein funktionierendes Beispiel mit `from deemix import generateDownloadObject`, `from deemix.downloader import Downloader` und `from deemix.plugins.spotify import Spotify` [^1]. Die Kernlogik: `generateDownloadObject` parst URLs/ISRCs in Download-Objekte, die dann vom `Downloader` verarbeitet werden [^1].

**Aber: CLI-Wrapper ist robuster aus folgenden Gründen:**

- **Projekt-Fragmentierung**: Das originale `deemix` von RemixDev ist praktisch inaktiv. Der aktiv gepflegte Fork ist `bambanah/deemix` ("monorepo for the revived Deemix project") [^10]. Die API-Oberfläche kann sich zwischen Forks verschieben — ein CLI-Wrapper isoliert dich davon.
- **ARL-Authentifizierung**: Deemix nutzt einen ARL-Cookie (aus dem Deezer-Browser-Login) zur Authentifizierung. Der revived Fork hat die ARL-Login-Logik gehärtet (Whitespace-Stripping, Validierung auf non-hex characters) [^5]. Der ARL kann periodisch ablaufen — ein CLI-Wrapper mit klarer Fehler-Ausgabe macht das Debugging einfacher als ein Bibliotheks-Import, der Exceptions wirft.
- **CLI existiert und ist stabil**: `python3 -m deemix <url>` funktioniert direkt als CLI [^17]. Die PyPI-Beschreibung bestätigt: "The base library (or core) can be used as a stand alone CLI app or implemented in an UI using the API" [^12].

**ISRC-Suchpräzision**: Deemix/Deezer unterstützt ISRC-basierte Suche. Die `generateDownloadObject`-Funktion kann Deezer-Track-URLs verarbeiten [^1]. Für die ISRC-basierte Suche auf Deezer empfiehlt sich aber, direkt die Deezer-API (`api.deezer.com/track/isrc:{ISRC}`) zu查询 — das gibt dir einen klaren 404/200-Status für die Fallback-Entscheidung in Stage 2→3, unabhängig von deemix-Interna.

**Konkreter Implementierungsvorschlag für Stage 2:**
```python
# 1. ISRC-Lookup via Deezer API (unabhängig von deemix)
import requests
def deezer_isrc_lookup(isrc: str) -> dict | None:
    r = requests.get(f"https://api.deezer.com/track/isrc:{isrc}")
    data = r.json()
    if "error" in data:
        return None  # → Stage 3
    return data  # enthält deezer track_id

# 2. deemix CLI als Subprocess (mit ARL)
import subprocess
def deemix_download(deezer_track_url: str, arl: str, output_dir: str) -> bool:
    result = subprocess.run([
        "python3", "-m", "deemix",
        "--bitrate", "320",
        "--path", output_dir,
        "--arl", arl,
        deezer_track_url
    ], capture_output=True, text=True, timeout=120)
    return result.returncode == 0
```

---

## 2. spotDL vs. nacktes yt-dlp

### Empfehlung: spotDL mit eigenen Spotify-API-Credentials und Cookie-File

**spotDL ist zuverlässiger als ein selbstgebauter yt-dlp-Wrapper**, hat aber spezifische Konfigurationsanforderungen:

**Vorteile von spotDL gegenüber DIY yt-dlp:**
- spotDL nutzt einen ausgeklügelten Matching-Algorithmus, der YouTube-Suchergebnisse gegen Spotify-Metadaten (Dauer, Titel, Künstler) validiert — nicht nur eine naive `"Artist - Title - Audio"`-Suche [^8].
- Es behandelt Edge-Cases wie "YouTube hat einen Remix, keinen Original-Track" durch Score-basiertes Matching.
- Eingebautes ID3-Tagging mit Spotify-Metadaten + Cover-Art.

**Kritische Konfigurationspunkte (aus den Issues):**

| Problem | Lösung |
|---------|--------|
| Spotify Rate-Limits | **Eigene Client ID/Secret verwenden** — die Shared-Credentials von spotDL werden gedrosselt [^6] |
| YouTube Rate-Limits (429) | `--cookie-file cookies.txt` + PO-Token [^6] |
| Bitrate-Limit | Ohne YouTube Premium: max. **128 kbps MP3**. Mit YTMusic Premium + M4A: bis zu **256 kbps** [^15] |
| Audio Providers | `audio_providers: ["youtube", "youtube-music"]` im Config-JSON [^9] |

**Wann ein eigener yt-dlp-Wrapper besser wäre:** Wenn du volle Kontrolle über die Such-Logik brauchst (z.B. ISRC-basierte YouTube-Suche via `ytsearch`-Filter) oder wenn spotDL zu oft bricht. Aber für deinen Use-Case (Fallback-Stage) ist spotDL die pragmatischere Wahl — es ist aktiv gepflegt und kapselt die komplexe YouTube-Matching-Logik.

**spotDL Config-Beispiel:**
```json
{
  "client_id": "<deine_spotify_client_id>",
  "client_secret": "<deine_spotify_client_secret>",
  "audio_providers": ["youtube", "youtube-music"],
  "format": "mp3",
  "bitrate": "128",
  "cookie_file": "cookies.txt"
}
```

**Aufruf als Subprocess:**
```python
subprocess.run([
    "spotdl", "download", spotify_url,
    "--output", output_dir,
    "--bitrate", "128"
], timeout=180)
```

---

## 3. Alternative Deezer-Downloader: streamrip

### Empfehlung: streamrip ist **nicht** als Drop-in-Ersatz für Stage 1-3 geeignet

**streamrip unterstützt KEINE direkten Spotify-Track-URLs.** Die Spotify-Integration in streamrip funktioniert ausschließlich über **Playlists** und nutzt **Last.fm** als Matching-Mechanismus — nicht ISRC [^2]. Die README sagt explizit: "Supports downloads of Spotify and Apple Music playlists through last.fm" [^2].

**Was streamrip gut kann:**
- Deezer, Qobuz, Tidal, SoundCloud Downloads (direkte URLs) [^2]
- Fallback auf niedrigere Qualität, wenn gewünschte Qualität nicht verfügbar [^14]
- Concurrent Downloads via `aiohttp`, Rate-Limiting, Dedup-Database

**Was streamrip NICHT kann (für deinen Use-Case):**
- ❌ Einzelne Spotify-Track-URLs auflösen (nur Playlists via Last.fm)
- ❌ Automatisches Fallback-Routing zwischen Deezer und YouTube
- ❌ ISRC-basiertes Matching

**Es gibt aber Forks**, die das ergänzen: `justinjohnso/music-downloader` erweitert streamrip um "Download individual Spotify tracks via URL or URI (searches Deezer/Qobuz)" mit "Qobuz fallback if tracks aren't found on Deezer" [^7]. Das ist aber ein Dritt-Fork und kein Feature des Haupt-Projekts.

**Aktueller Issue (Juni 2025):** Deezer hat sein URL-Format geändert, was zu Problemen bei Single-Track-Downloads führt [^16]. Ein Fix wurde zwar released [^14], aber das zeigt, dass streamrip an Deezer-URL-Änderungen gekoppelt ist.

**Fazit**: Behalte deemix als Primary Downloader. streamrip wäre eine interessante Alternative **nur** für die Deezer-Download-Komponente (Stage 2), aber da dein Fallback auf YouTube (nicht Qobuz/Tidal) gehen soll, ist spotDL die bessere Wahl für Stage 3.

---

## 4. Spotify API Credentials

### Setup-Prozess für Headless-Zugriff

Der **Client Credentials Flow** ist genau das, was du brauchst — er ist "server-to-server authentication" und erfordert keinen User-Login [^4].

**Setup-Schritte:**

1. **App erstellen**: Im [Spotify Developer Dashboard](https://developer.spotify.com/dashboard) → "Create app" → erhältst du **Client ID** und **Client Secret** [^13].

2. **Token abholen** (Server-to-Server, kein User-Login nötig):
```python
import requests
import base64

def get_spotify_token(client_id: str, client_secret: str) -> str:
    credentials = base64.b64encode(f"{client_id}:{client_secret}".encode()).decode()
    r = requests.post(
        "https://accounts.spotify.com/api/token",
        headers={"Authorization": f"Basic {credentials}"},
        data={"grant_type": "client_credentials"}
    )
    return r.json()["access_token"]  # Gültig für ~1 Stunde
```

3. **Metadaten abrufen** (Stage 1):
```python
def get_track_metadata(token: str, track_id: str) -> dict:
    r = requests.get(
        f"https://api.spotify.com/v1/tracks/{track_id}",
        headers={"Authorization": f"Bearer {token}"}
    )
    data = r.json()
    return {
        "isrc": data["external_ids"]["isrc"],
        "title": data["name"],
        "artist": data["artists"][0]["name"],
        "album": data["album"]["name"],
        "cover_url": data["album"]["images"][0]["url"]
    }
```

**Einschränkung**: Der Client Credentials Flow kann nur **öffentliche** Endpunkte erreichen (Tracks, Albums, Playlists) — keine User-spezifischen Daten (Liked Songs, Private Playlists) [^4]. Für deinen Use-Case (öffentliche Spotify-Track-URLs) reicht das vollständig.

**Für spotDL**: Dieselben Credentials können im spotDL-Config-JSON unter `client_id` und `client_secret` eingetragen werden [^9]. Das löst gleichzeitig die Spotify-Rate-Limit-Problematik von spotDL [^6].

**Token-Management**: Access Tokens gelten ~1 Stunde. Caching mit automatischem Refresh empfohlen — nicht bei jedem Request einen neuen Token anfordern [^11].

---

## 5. Sync-Mechanismus

### Empfehlung: Mutagen für minimale Latenz, rsync-cron als Fallback

**Vergleich der Optionen:**

| Ansatz | Latenz | Komplexität | Bidirektional | Eignung |
|--------|--------|-------------|---------------|---------|
| rsync + Cron (1 Min) | 0–60s | Niedrig | Nein (unidirektional) | Einfach, aber Latenz-Spitzen |
| inotifywait + rsync | ~1–5s | Mittel | Nein | Event-gesteuert, aber separate Tools [^18] |
| **Mutagen** | **~1–2s** | Mittel | **Ja** | **Beste Wahl für DJ-Use-Case** [^3] |
| systemd.path + rsync | ~1–3s | Mittel | Nein | Native systemd-Alternative zu inotifywait [^19] |

**Warum Mutagen die beste Wahl ist:**

Mutagen kombiniert "the performance of the rsync algorithm with bidirectionality and low-latency filesystem watching" [^3]. Für deinen Use-Case (Track vom Hetzner-Server → MacBook für Traktor) ist das ideal:

- **Sofortige Synchronisation**: Filesystem-Watching erkennt neue Dateien nahezu in Echtzeit (~1–2s) [^3].
- **Über Tailscale/WireGuard**: Mutagen nutzt SSH als Transport — funktioniert nativ über dein Tailscale-Setup.
- **Persistente Verbindung**: Im Gegensatz zu rsync-Cron hält Mutagen eine dauerhafte Verbindung offen, kein Connection-Overhead pro Sync.
- **Bidirektional**: Falls du auch ID3-Tag-Änderungen vom Mac zurück pushst.

**Setup auf dem Hetzner-Server + MacBook:**
```bash
# Auf dem Mac (einmalig):
mutagen sync create \
  --name=dj-tracks \
  --mode=one-way-safe \
  --ignore-vcs \
  user@hetzner-server:/opt/downloads/tracks \
  ~/Music/DJ/Incoming
```

**Wenn rsync-Cron als einfachere Alternative gewünscht:**
```bash
# crontab -e auf dem Mac (jede Minute):
* * * * * rsync -az --delete \
  --rsh="ssh -o StrictHostKeyChecking=no" \
  user@hetzner-server:/opt/downloads/tracks/ \
  ~/Music/DJ/Incoming/ 2>> ~/rsync-dj.log
```
Nachteile: bis zu 60s Latenz, SSH-Connection-Overhead pro Durchlauf, keine Event-Benachrichtigung.

---

## Zusammenfassung: Empfohlene Architektur

```
┌─────────────────────────────────────────────────────┐
│  FastAPI Backend (Hetzner)                           │
│                                                     │
│  POST /download {"url": "spotify:track:xxx"}        │
│       │                                             │
│       ▼                                             │
│  Stage 1: Spotify API (Client Credentials Flow)     │
│       → ISRC, Artist, Title, Album, Cover           │
│       │                                             │
│       ▼                                             │
│  Stage 2: Deezer ISRC Lookup → deemix CLI (320k)    │
│       │ Erfolgreich ──────────────────────┐         │
│       │ 404/Fehler                         │         │
│       ▼                                   │         │
│  Stage 3: spotDL (youtube + youtube-music) │         │
│       → --cookie-file, eigene Creds       │         │
│       │                                   │         │
│       ▼                                   ▼         │
│  Stage 4: ID3-Tagging (mutagen/eyeD3)  +  move      │
│       → /opt/downloads/tracks/                       │
└─────────────────────────────────────────────────────┘
                        │
                        ▼ (Mutagen sync, ~1-2s)
┌─────────────────────────────────────────────────────┐
│  MacBook (Traktor)                                   │
│  ~/Music/DJ/Incoming/ (auto-imported by Traktor)    │
└─────────────────────────────────────────────────────┘
```

| Komponente | Empfehlung | Begründung |
|------------|------------|------------|
| **Primary Downloader** | deemix CLI-Wrapper (bambanah/deemix) | Robuster als Bibliotheks-Import, isoliert vor Fork-Fragmentierung |
| **ISRC-Lookup** | Deezer REST API direkt | Klares 404/200 für Fallback-Entscheidung |
| **Fallback Downloader** | spotDL mit eigenen Creds + Cookie-File | Ausgereiftes YouTube-Matching, aktiv gepflegt |
| **streamrip** | Nicht als Primärtool | Keine Spotify-Track-URL-Unterstützung, nur Playlists via Last.fm |
| **Spotify API** | Client Credentials Flow | Headless, kein User-Login, reicht für öffentliche Metadaten |
| **Sync** | Mutagen over Tailscale | ~1–2s Latenz, persistente Verbindung, filesystem-watching |
| **Tagging** | Python `mutagen`-Bibliothek (nicht zu verwechseln mit dem Sync-Tool) | Industriestandard für ID3-Tag-Manipulation |

**References**

[^1]: [dmx.py](https://github.com/naujzed/deempy/blob/main/dmx.py) (15%)
[^2]: [nathom/streamrip GitHub Repo](https://github.com/nathom/streamrip) (9%)
[^3]: [File synchronization - Mutagen](https://mutagen.io/documentation/synchronization/) (8%)
[^4]: [Client Credentials Flow](https://developer.spotify.com/documentation/web-api/tutorials/client-credentials-flow) (8%)
[^5]: [Releases · bambanah/deemix · GitHub](https://github.com/bambanah/deemix/releases) (6%)
[^6]: [FAQ: Rate/request limit or YT-DLP Download Error #2420](https://github.com/spotDL/spotify-downloader/issues/2420) (6%)
[^7]: [GitHub - justinjohnso/music-downloader: Extending streamrip for my...](https://github.com/justinjohnso/music-downloader) (6%)
[^8]: [Architecture | spotDL/spotify-downloader | DeepWiki](https://deepwiki.com/spotDL/spotify-downloader/2-architecture) (5%)
[^9]: [Docker/Web UI: client not found and http/1.1 404 errors · Issue #2467...](https://github.com/spotDL/spotify-downloader/issues/2467) (5%)
[^10]: [bambanah/deemix](https://github.com/bambanah/deemix) (5%)
[^11]: [github.com/jwilsson/spotify-web-api-php/blob/main/docs/examples...](https://github.com/jwilsson/spotify-web-api-php/blob/main/docs/examples/access-token-with-client-credentials-flow.md) (4%)
[^12]: [deemix · PyPI](https://pypi.org/project/deemix/) (4%)
[^13]: [Apps | Spotify for Developers](https://developer.spotify.com/documentation/web-api/concepts/apps) (4%)
[^14]: [Releases · nathom/streamrip - GitHub](https://github.com/nathom/streamrip/releases) (3%)
[^15]: [spotify-downloader/docs/usage.md at master · spotDL/spotify ... - GitHub](https://github.com/spotDL/spotify-downloader/blob/master/docs/usage.md) (3%)
[^16]: [Deezer URL Format Change Prevents Single Track ...](https://github.com/nathom/streamrip/issues/865) (3%)
[^17]: [Is there any way to use Deemix via CLI? I miss using CLI on ... - Reddit](https://www.reddit.com/r/deemix/comments/r9pw53/is_there_any_way_to_use_deemix_via_cli_i_miss/) (2%)
[^18]: [Low-latency continuous rsync - Right Angles](https://www.danplanet.com/blog/2012/05/09/low-latency-continuous-rsync/) (2%)
[^19]: [Self-Hosted Linux Filesystem Event Monitoring: systemd.path vs ...](https://www.pistack.xyz/posts/2026-05-24-self-hosted-linux-filesystem-event-monitoring-systemd-path-vs-inotifywait-vs-incron-guide/) (2%)
