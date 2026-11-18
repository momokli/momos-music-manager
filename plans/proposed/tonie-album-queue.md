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

## Offene Entscheidungen

1. **Dauer pro Album**: die Artist-Album-Liste liefert kein `nb_tracks`/`duration`.
   Tracklist-Call pro Album (417 Calls ≈ 42 s bei 50 req/5 s) plus Cache in
   `hub_deezer_albums`. Alternative: Dauer erst beim Anzeigen/Benutzen nachladen.
2. **Album länger als die Kapazität** (bei Hörspielen selten, bei Konzerten normal):
   überspringen und im Run-Log melden — oder doch anschneiden? Vorschlag: überspringen.
3. **Ledger-Granularität**: pro User (einmal gehört = überall gehört) oder pro Tonie?
   Vorschlag: pro User, weil „mein Kind hat das gehört" unabhängig vom Tonie gilt.
4. **Wohin mit dem Deezer-Client**: eigener Client im Hub (2 Endpunkte, kein Token) oder
   Proxy in music-api (ein Client mit Rate-Limit-Handling, wie hier angenommen)?
