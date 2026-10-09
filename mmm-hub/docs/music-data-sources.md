# Musik-Datenquellen für den Hub (Recherche)

> Stand: 2026-10-09 · Recherche über Kagi/Web · Zweck: Track-Metadaten (Genre/Stil, BPM, Key,
> Mood/Energy) und Library-Ingest jenseits von Spotify.

## Ausgangslage

Spotify hat am **27.11.2024** `audio-features`, `audio-analysis`, `recommendations`,
`related-artists` und featured-playlists **abgeschaltet**. Für **neu angelegte Apps** gibt es
dafür **403** — ein offizielles Replacement gibt es bis heute nicht. Der Hub kann also über
Spotify **keine BPM/Key/Energy/Genre** mehr beziehen. Genau deshalb brauchen wir eine eigene
„Analyse-Schicht" dazwischen.

## Empfehlung in einem Satz

**ReccoBeats** als kostenlose Primärquelle für Audio-Features (BPM/Key/Energy/…), **MusicBrainz**
für Genre/Tags, **eigene On-Disk-Analyse** (Essentia) als Fallback — und für Ingest zusätzlich
**SoundCloud OAuth** + **YouTube Data API v3**.

## 1. Audio-Features (BPM, Key, Energy, Danceability, …) — Spotify-Ersatz

| Anbieter | Daten | Auth | Kosten | Coverage | Bewertung |
| --- | --- | --- | --- | --- | --- |
| **ReccoBeats** | acousticness, danceability, energy, instrumentalness, key, liveness, loudness, speechiness, **tempo/BPM**, valence (+ Recommendation) | **keine** (public) | **frei** | Spotify-ID-keyed, groß | Primaerwahl — Schema spiegelt das alte Spotify-Endpoint, quasi drop-in |
| **FreqBlog** | BPM, Key, Camelot, Mood, 44 Felder **per Track-Name** (kein Spotify-ID nötig) | API-Key | Free-Tier, sonst guenstig | gut, name-matching | Guter Fallback, wenn kein Spotify-ID/ISRC vorhanden |
| **Musicae / "Spotify Extended Audio Features"** | Audio-Features + DJ-Scores, drop-in | API-Key (RapidAPI) | **paid** | >250 Mio | Wenn man DJ-Scores/Beatgrids braucht |
| **AcousticBrainz** | BPM, Key, Mood-Modelle | keine | frei | nur Bestand (Sammlung **2022 eingestellt**) | read-only, lueckenhaft — nur als Alt-Daten |
| **Essentia / librosa (self-hosted)** | alles, aus der Audiodatei | — | nur Compute | eigene Library | Fallback/Offline; braucht die Dateien (teils via `music-api`) |

**Fazit:** ReccoBeats zuerst (kostenlos, kein Auth!), FreqBlog als Name-Fallback. Fuer Tracks ohne
Match -> self-hosted Essentia ODER als "unbekannt" markieren.

## 2. Genre / Stil ("ist das Techno?")

Spotify liefert kein verlaessliches Genre pro Track mehr. Optionen:

| Quelle | Was | Auth | Kosten |
| --- | --- | --- | --- |
| **MusicBrainz** | Genres/Tags pro Recording (community), + ISRC<->MBID | keine (User-Agent Pflicht) | frei |
| **Last.fm** | crowd tags, `track.getTopTags` | API-Key | frei |
| **Discogs** | Genre/Style (Vinyl/Release-Ebene) | API-Key/OAuth | frei (rate-limited) |
| **Beatport** | sehr genaue elektronische Genres, BPM, Key — aber **offiziell keine offene API** | — | Scraping/Partner |

**Fazit fuer "Techno-Filter":** MusicBrainz-Tags + Last.fm-Tags kombinieren; Beatport ist fuer
elektronische Musik am praezisesten, aber nicht offiziell angebunden (rechtlich heikel).

## 3. Library-Ingest (mehrere Quellen pro User)

Ziel: ein Track kann **mehrere Playlist-Quellen** haben (Spotify / SoundCloud / YouTube), die
denselben **Tag** fuellen.

| Dienst | Endpoints | Auth | Huerden |
| --- | --- | --- | --- |
| **Spotify** | `/me/playlists`, `/me/tracks`, `/playlists/{id}/items` | OAuth (haben wir) | dev-mode Quota |
| **SoundCloud** | `/users/{urn}/likes/tracks`, `/users/{urn}/likes/playlists`, `/me/...` | **OAuth 2.1 + PKCE** | Neuregistrierung oeffentlich **stark eingeschraenkt**; client_id oft nur "intern" (api-v2) |
| **YouTube (Music)** | `playlistItems.list`, `playlists.list`, liked videos | OAuth (Data API v3) | **10.000 Quota-Einheiten/Tag** (`playlistItems` = 1, `search` = 100) |

**Fazit:** Spotify haben wir. SoundCloud ist machbar, aber die Developer-Registrierung ist
zickig -> ggf. bestehender/interner client_id. YouTube ist sauber dokumentiert, Quota beachten
(kein `search`, nur `playlistItems`).

## 4. Konsequenz fuer die Architektur (Tag-Layer)

Wie im Haupt-Repo (Momo's Music Manager): **nicht Playlist und Tag gleichsetzen**, sondern:

```
service_playlist  --(resolve)-->  tag  <--(resolve)-- service_playlist (andere Quelle)
        |                            ^
   playlist_tracks                   |  track_resolved_tags
        v                            |
      track -------------------------+
```

- **Playlists sind Quellen**, Tags sind das normalisierte, quellenuebergreifende Konzept.
- Eine **Meta-Playlist** (z. B. "Alle Likes", "Fusion 2025 | My liked Artists") wird beim Resolve
  **ausgeschlossen**.
- Ein Tag kann von **mehreren Quellen** gefuellt werden (Spotify-Playlist + SoundCloud-Set + YT).
- Das **Backend macht das Resolve** (nie das Frontend) und **cached** das Ergebnis
  (materialisiertes `track_resolved_tags`, wie im Haupt-Repo).

## 5. Konkrete naechste Schritte (Vorschlag)

1. **ReccoBeats-Adapter** (kein Auth): `GET /v1/track/{id}/audio-features` keyed by Spotify-ID;
   Ergebnis in `hub_track_features` cachen. -> liefert BPM/Key/Energy fuer Track-Filter.
2. **MusicBrainz + Last.fm Genre-Fetch** (ISRC->MBID->Tags) in `hub_track_genres` cachen.
3. **Tag-Layer** (Migration + Resolve-Job + Views) nach MMM-Vorbild.
4. **SoundCloud + YouTube** Ingest pro User (OAuth), gleiche Playlist/Track-Tabellen.
5. **Filter-Framework**: jedes Panel = Filter-Bar (Top) -> SQL -> Tabelle; Sortierung/Filter
   serverseitig, gecacht wo teuer.

## Quellen (Auswahl)

- Spotify changelog / community: audio-features deprecated 2024-11-27
- ReccoBeats docs: https://reccobeats.com/docs/apis/get-track-audio-features
- FreqBlog: https://freqblog.com/
- AcousticBrainz (shutdown 2022): https://acousticbrainz.org/
- SoundCloud API guide: https://developers.soundcloud.com/docs/api/guide
- YouTube Data API v3 Quota (10k/day): Google docs / 2026 guides
- MusicBrainz: https://musicbrainz.org/doc/Genre
