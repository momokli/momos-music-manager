# Dev: Screenshots vom Hub

Ein Weg zu echten Screenshots der Hub-Oberfläche, ohne Keycloak, ohne
Tonies-Konto, ohne die Produktions-`music-api` und ohne Netzwerk.

```bash
scripts/dev/screenshot.sh                 # -> screenshots/*.png
scripts/dev/screenshot.sh /tmp/shots      # eigener Zielordner
```

## Was passiert

1. `music-hub` wird gebaut (Debug) und mit einer **temporären SQLite-DB** auf
   `127.0.0.1:18080` gestartet — Migrationen laufen beim Start, wie überall.
2. `scripts/dev/mock-music-api.py` liefert die Deezer-Routen, die die
   Album-Seiten lesen (`/deezer/artist/{id}/albums`, `/deezer/album/{id}`,
   `/deezer/search/artist`) mit einem festen PAW-Patrol-Fixtureset. Cover sind
   generierte SVGs, damit kein Screenshot ein kaputtes Bild zeigt.
3. Ein User `demo`/`demo` wird angelegt. **Login ist lokal**: `POST /login` prüft
   `hub_users.password_hash` per bcrypt (`music-hub/src/web.rs`), Keycloak ist
   nur der SSO-Pfad der Produktion. Die Registrierung wird dafür in der
   Wegwerf-DB kurz geöffnet (`hub_settings.registration_open`).
4. Eine Bindung (`source_kind = albums`, `album_mode = basket`) und zwei
   Korb-Zeilen werden direkt eingefügt — der Korb hängt an (User, Figur), eine
   echte Tonie-Figur ist nicht nötig.
5. `scripts/dev/screenshot-hub.cjs` (Playwright, **System-Chrome** über
   `channel: "chrome"`, also kein Browser-Download) setzt das Session-Cookie und
   schießt Desktop (1280×900) und Mobil (390×844). Es prüft vorher, dass das
   Album-Grid nicht leer ist, und schlägt bei `pageerror` fehl.

Alles läuft in einem `mktemp`-Verzeichnis und wird beim Beenden abgeräumt.

## Grenzen

- **Artist-Seite** (`/artist/{id}`) wird nicht geschossen: sie 404t, solange der
  Artist keine Tracks im User-Scope hat (`music-hub/src/artists.rs`, leerer
  Track-Filter → `not_found`). Dafür müsste der Seed Katalog-Tracks anlegen.
- Die Dauer pro Kachel kommt in der Produktion aus dem Detail-Call je Album
  (`GET /deezer/album/{id}`), weil die Artist-Liste kein `seconds` liefert
  (#621). Der Mock liefert beides.
- Der Hub muss portfrei sein: die Skripte belegen `18080` (Hub) und `18710`
  (Mock), überschreibbar per `HUB_PORT` / `MUSIC_API_PORT`.

## Warum das existiert

Der PR-Review verlangt bei Frontend-Änderungen Screenshots. Ohne diese
Umgebung hieß das: Produktions-Login, verbundenes Tonie-Konto und echte
music-api — also nichts, was im CI oder auf einem Build-Rechner geht.
