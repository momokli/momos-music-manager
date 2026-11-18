# Plan: Tonie-Album-Queue (Deezer-Alben als Quelle)

**Status**: proposed
**Branch**: `feat/tonie-album-queue`
**Ready for review**: no
**Depends on**: [tonies](tonies.md) (Sync-Runner, shipped), [native-deezer-download](native-deezer-download.md)
**Migration needed**: yes (`062_tonie_albums.sql`, 061 ist vergeben)

## Problem (verifiziert 2026-11-18)

**ISRC ist editionsgebunden, nicht werkgebunden.** Dieselbe Folge kann in zwei Katalogen
unterschiedlich geschnitten sein und trägt dann unterschiedliche ISRCs.

Beispiel „PAW Patrol Folge 459: Die Robo-Ente":

| | Deezer (Album 1137915301) | Spotify (in `hub_playlist_tracks`) |
| --- | --- | --- |
| Titelsong | 40 s, `CH7812531303` | 40 s, `CH7812531303` (identisch) |
| Kapitel | 11 Kapitel à 95–112 s | 6 Kapitel à 96–214 s |
| ISRCs | `CH7812690465` … `CH7812690475` | `CH7812690476` … `CH7812690481` |

`api.deezer.com/track/isrc:CH7812690476` → `error 800: no data`. Ein Sync kann diese Tracks
per ISRC grundsätzlich nicht finden — es ist kein Fetch- und kein Vault-Problem.

Messung der ersten 40 Positionen von Playlist 278 gegen music-api:

```
ready:   6    (ausschliesslich der Titelsong, 6x dieselbe ISRC)
absent: 34    (alle mit "deezer error 800: no data")
```

Betroffen sind **Folge 456–459 vollständig**. Ältere Folgen (`DED7922…`) matchen sauber.

Datenlage auf der Zielseite: Deezer-Artist `15167501` (Paw Patrol) hat **417 Alben**,
neueste zuerst, wöchentlicher Rhythmus, `record_type: album`, je Folge ~12 Tracks ≈ 20 min.
**Deezer-Album-Tracklists tragen ihre ISRC** — der bestehende Vault-/Order-/Upload-Pfad
bleibt damit unverändert nutzbar.

## Ziel

Der Tonie wird direkt aus Deezer-Alben gefüllt, nicht aus Spotify-Playlists:

1. **Artist-Abo**: „immer die neuesten Folgen" — ganze Alben, neueste zuerst.
2. **Aufwärts**: „fang bei Folge 1 an und arbeite dich nach vorn" — ganze Alben, älteste zuerst.
3. **Warenkorb**: beliebige Alben explizit auswählen (Cover + Dauer) und in Reihenfolge legen.
4. **Listened-Ledger**: was einmal auf einem Tonie war, gilt als gehört und wird übersprungen;
   Alben lassen sich wieder auf „unlistened" setzen.

Invariante in allen Modi: **ein Album wird nur aufgenommen, wenn es ganz in die verbleibende
Kapazität passt.** Nie eine halbe Folge.

## Modell

Neue Quellenart `albums` neben `playlist` / `tag` / `backpack`:

```
hub_tonie_bindings.source_kind = 'albums'
hub_tonie_bindings.source_id   = Deezer-Artist-ID (NULL im Warenkorb-Modus)
hub_tonie_bindings.album_mode  = 'newest' | 'oldest' | 'basket'
```

Neue Tabellen:

```sql
-- Warenkorb: explizite Album-Reihenfolge pro Binding
hub_tonie_album_items(
  binding_id, position, album_id, album_title, track_count, seconds, added_at
)

-- Ledger: ein Album gilt als gehoert, sobald es einmal Teil eines ok-Runs war
hub_tonie_albums_seen(
  user_id, album_id, album_title, first_used_at, last_used_at
)

-- Cache der Deezer-Album-Metadaten (Dauer! die Artist-Liste liefert kein nb_tracks)
hub_deezer_albums(
  album_id, artist_id, title, release_date, record_type, cover_url,
  track_count, seconds, fetched_at
)
```

`DesiredTrack` bleibt unverändert (ISRC + Titel + Dauer). Der Album-Resolve erzeugt
**Gruppen** (Alben), der Fit wählt ganze Gruppen, erst danach wird flach geklopft — so kennt
der Run weiterhin nur ISRCs, aber der Planer kennt die Album-Grenzen.

## Bausteine

| Teil | Ort | Änderung |
| --- | --- | --- |
| Deezer-Client | `music-api/src/deezer.rs` | `artist_albums(artist_id)`, `album_tracks(album_id)`, `search_artist(q)`; Token-pflichtig wie die übrigen Endpunkte |
| API | `music-api/src/api.rs` | `GET /deezer/artist/{id}/albums`, `GET /deezer/album/{id}`, `GET /deezer/search/artist?q=` |
| Hub-Client | `music-hub/src/music_api.rs` | drei dünne Wrapper (Bearer wie `status`) |
| Resolver | `music-hub/src/tonies/albums.rs` (neu) | `resolve(binding, seen) -> Vec<AlbumPlan>`: Artist-Alben bzw. Warenkorb, Liste filtern, Reihenfolge nach Modus |
| Fit | `music-hub/src/tonies/sync.rs` | `greedy_fit_albums(groups, cap, ready)` — ganze Gruppen, sonst `AlbumTooLong`/`CapReached` |
| Run | `music-hub/src/tonies/run.rs` | `execute_binding` nutzt den Album-Resolver, markiert nach `ok` die benutzten Alben in `hub_tonie_albums_seen` |
| Bindings | `music-hub/src/tonies/bindings.rs` | `album_mode`, `source_kind='albums'`, Warenkorb-CRUD |
| UI | `music-hub/templates/tonies.html` + `_partials/tonie_album_picker.html` | Modus-Auswahl, Album-Grid mit Cover/Dauer/Häkchen, Warenkorb-Reihenfolge, „listened"-Liste mit Reset |
| Migration | `music-hub/migrations/062_tonie_albums.sql` | drei Tabellen + `album_mode` (additiv) |

## Slices

**M1 — Artist-Abo (Kernwert)**
Deezer-Endpunkte in music-api, `albums`-Quellenart, Modi `newest`/`oldest`, Fit auf
Album-Ebene, Dauer-Cache. Ergebnis: Tonie bekommt die neuesten N Folgen resp. ab Folge 1
aufwärts. Kein Warenkorb, kein Ledger.

**M2 — Listened-Ledger**
`hub_tonie_albums_seen`, Skip im Resolver, Markierung nach erfolgreichem Run. Damit wird
`oldest` zu „arbeite dich nach vorn": jede Woche rückt der Tonie automatisch weiter.

**M3 — Warenkorb-UI**
Album-Grid mit Covern und Dauer, Mehrfachauswahl, Reihenfolge, Modus `basket`. Manuelles
Beladen mit beliebigen Alben eines beliebigen Artists.

**M4 — Ledger-Verwaltung**
Liste der gehörten Alben, „unlistened" pro Album, optional „alle zurücksetzen".

## Acceptance Criteria

- [ ] `cargo build` + `cargo test` (music-hub **und** music-api) grün
- [ ] Album-Fit nimmt nie ein Teil-Album (Test: 20-min-Album bei 10 min Rest → übersprungen)
- [ ] `newest` und `oldest` liefern spiegelbildliche Reihenfolgen (Test mit fixierten `release_date`)
- [ ] Gehörte Alben werden übersprungen und nach `unlistened` wieder angeboten (Test)
- [ ] Warenkorb-Reihenfolge bestimmt die Kapitel-Reihenfolge (Test)
- [ ] Kein Spotify-Request im Album-Pfad (Test analog `autosync_makes_no_spotify_requests`)
- [ ] UI: Album-Grid rendert Cover-URLs und Dauern, Auswahl überlebt ein Speichern

## UX/IA (beschlossen 2026-11-18)

Drei Ebenen: **Entdecken → Sammeln → Zuweisen**. Der Tonie ist das *Ziel*, nicht
der Startpunkt. User ohne verbundenes Tonie-Konto sehen weder Tonies noch Korb.

### Korb: pro User **und** pro Figur

- Neue Tabelle `hub_tonie_cart_items(id, user_id, tonie_id, position, kind, ref_id,
  title, cover_url, seconds, added_at)` mit `kind` in `album` | `track`.
- **Ein Korb pro (User, Figur)**, mehrere parallel, persistent (Handy anfangen,
  Desktop weitermachen).
- Ein Album ist im Korb ein **Paket**: als Ganzes verschiebbar, aufklappbar →
  Tracks intern sortierbar. Ein einzelner Track ist ein eigener Posten.
- Der Korb ist der Entwurf der Figur. `album_mode = basket` liest künftig den
  Korb statt `hub_tonie_album_items` (061). Die alte Tabelle bleibt vorerst
  bestehen (additive Regel); ihre Zeilen werden einmalig migriert.
- Abo (`newest`/`oldest`, Artist) und Korb (`basket`) schließen sich pro Figur
  aus — das ist bereits so modelliert.

### Entdecken

- `/artist/{id}` bekommt eine **Alben-Sektion** (Deezer-Alben, weil Tonies daraus
  gefüllt werden): Cover, Release-Datum, Dauer, Badges `auf Tonie` / `gehört`.
  Toggle „Alben / Statistik", damit die bestehende Meistgespielt-Ansicht bleibt.
- `/search` bekommt neben Tracks die Gruppen **Künstler** und **Alben** (Deezer).
- Beim Öffnen/Suchen eines Artists werden die Album-Metadaten in
  `hub_deezer_albums` gecacht (existiert seit 062).
- Aktionspaar pro Artist: „Neueste ungehörte auf Tonie …" (Abo) und
  „In den Korb" pro Album.
- Abgrenzung: die bestehende `/album/{name}`-Seite (abgeleitet aus
  `hub_tracks.album`, Spotify) bleibt unverändert. Tonie-Alben sind Deezer-Alben;
  beide werden nicht vermischt.

### Zuweisen

- Im Korb: „Auf Tonie …" ist die Figur schon gewählt (Korb hängt an ihr) →
  Aktion „Als Quelle setzen" (`album_mode = basket`) + direkt „Sync".

### Verlauf

- Global pro User: gehörte Alben (`hub_tonie_albums_seen`), inkl. `unlistened`.
- Pro Figur: was aktuell drauf liegt (letzter `ok`-Run) und was vorher lief
  (`hub_tonie_syncs`).

### Slices nach diesem Bild

| Slice | Inhalt | Issue |
| --- | --- | --- |
| U1 | Artist-Seite: Alben-Sektion mit Covern, Badges, „In den Korb" | #601 |
| U2 | Korb pro Figur (Migration 063, `/cart`, Drag & Drop, „als Quelle setzen") | #602 |
| U3 | Nav/Visibility: Tonies + Korb nur mit Tonie-Konto; Binding-Editor aufräumen | #603 |
| U4 | Verlauf pro Figur und global | #604 |
| U5 | 128k-Ziel für Tonie-Orders in music-api | #605 |

## Katalog und Qualitaetsstufen (beschlossen 2026-11-18)

Der Konflikt: heute gibt es **eine** Erwerbsstrasse — `POST /orders` laedt in
voller Qualitaet (`formats: flac,320,128`) und analysiert alles auf BPM/Key.
Fuer einen Tonie ist das ~10x zu viel Speicher und komplett unnoetige Analyse,
aber dieselbe ISRC kann morgen in der DJ-Bibliothek landen.

### Zwei Achsen, getrennt halten

| Achse | Bedeutung |
| --- | --- |
| **Katalog** | „was gibt es?" — Deezer-Artists/Alben/Tracks, gecacht, TTL |
| **Vault** | „was habe ich?" — ISRC → Datei(en) + Analyse |
| **Zweck** | `library` (DJ, Spotify-getrieben, BPM/Key) vs. `toys` (Tonies, Deezer-getrieben) |
| **Qualitaet** | Folge des Zwecks: `library → flac + Analyse`, `toys → mp3-128, keine Analyse` |

### Katalog gehoert music-api

```sql
catalogue_artists(id, name, cover_url, fetched_at)
catalogue_albums(id, artist_id, title, release_date, record_type, cover_url,
                 seconds, tracks_fetched_at, fetched_at)
catalogue_tracks(album_id, position, isrc, title, duration_ms)
```

- TTL: Artist 24 h, Album-Tracks 7 Tage.
- **Nur** music-api schreibt (einziger Deezer-Client mit Rate-Limit-Handling).
- Hub liest ueber die `/deezer/*`-Endpunkte; `hub_deezer_albums` (062) wird
  stillgelegt (Tabelle bleibt liegen, additive Regel).
- Kein Vorcrawl der Diskografie: Metadaten fuer das, was angeschaut wird;
  Tracklisten nur fuer Alben, die in Korb oder Binding landen.

### Vault: Qualitaetsstufe + Zweck

```sql
vault_tracks: ... , quality_tier TEXT,   -- 'mp3-128' | 'mp3-320' | 'flac'
                analyzed_at TEXT
```

`POST /orders` bekommt zwei optionale Felder:

```json
{ "items": [{"isrc": "CH7812690465"}], "priority": 5,
  "intent": "toys", "target": "mp3-128" }
```

- `intent: toys` → nur den 128k-Stream holen, keine Analyse. 20-min-Folge: ~19 MB
  statt ~120 MB FLAC.
- `intent: library` → wie heute.
- Default ohne Felder = heutiges Verhalten (kein Breaking Change).

### Upgrade statt Duplikat

Wird eine ISRC spaeter fuer die Bibliothek gebraucht, ordert der Hub sie mit
`target: flac, intent: library`; music-api sieht „nur 128k vorhanden", laedt
hoeher, ergaenzt 320/flac und analysiert **dann**. Umgekehrt gilt: was einmal
FLAC ist, bleibt FLAC — ein Tonie-Order darauf laedt nichts nach.

### Konsequenz fuer die Reihenfolge

U5 ist damit **Voraussetzung** fuer U1/U2, nicht Nacharbeit: ohne
`intent`/`target` gibt es keinen sauberen Begriff davon, welche ISRC „nur Kind"
ist. Neue Slices:

| Slice | Inhalt | Issue |
| --- | --- | --- |
| U0a | Katalog-Store in music-api (Tabellen, TTL, Write-through, Lese-Endpunkte) | #608 |
| U0b | `intent`/`target` in `/orders`, `quality_tier`, Upgrade-Pfad, Analyse-Skip fuer toys | #609 |
| U0c | Hub liest Katalog ueber music-api, `hub_deezer_albums` stilllegen | #610 |

Danach U1–U4 wie gehabt.

## Offene Entscheidungen

1. **Dauer pro Album**: die Artist-Album-Liste liefert kein `nb_tracks`/`duration`.
   Tracklist-Call pro Album (417 Calls ≈ 42 s bei 50 req/5 s) plus Cache in
   `hub_deezer_albums`. Alternative: Dauer erst beim Anzeigen/Benutzen nachladen.
2. **Album länger als die Kapazität** (bei Hörspielen selten, bei Konzerten normal):
   überspringen und im Run-Log melden — oder doch anschneiden? Vorschlag: überspringen.
3. **Ledger-Granularität**: pro User (einmal gehört = überall gehört) oder pro Tonie?
   Vorschlag: pro User, weil „mein Kind hat das gehört" unabhängig vom Tonie gilt.
4. **Wohin mit dem Deezer-Client**: entschieden — Proxy in music-api (#594, #595).
5. **Korb-Scope**: entschieden — persistent pro (User, Figur).
6. **Album-Identitaet**: entschieden — Tonie-Alben sind Deezer-Alben; die
   Hub-Album-Seite bleibt davon unberuehrt.
7. **128k fuer Tonie-Orders**: entschieden — `intent`/`target` in `/orders` (U0b).
   Heute laedt music-api weiter FLAC und transkodiert; `binding.format` wirkt nur
   beim Download Vault → Tonie.
8. **Katalog-Ort**: entschieden — music-api (U0a), der Hub liest nur noch.
9. **Upgrade-Regel**: entschieden — ein hoeherer `target` loest ein Upgrade aus,
   nie ein Duplikat; Analyse laeuft erst beim Upgrade auf `library`.
