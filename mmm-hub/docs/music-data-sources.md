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

| Anbieter                                        | Daten                                                                                                                                 | Auth               | Kosten                    | Coverage                                    | Bewertung                                                              |
| ----------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------- | ------------------ | ------------------------- | ------------------------------------------- | ---------------------------------------------------------------------- |
| **ReccoBeats**                                  | acousticness, danceability, energy, instrumentalness, key, liveness, loudness, speechiness, **tempo/BPM**, valence (+ Recommendation) | **keine** (public) | **frei**                  | Spotify-ID-keyed, groß                      | Primaerwahl — Schema spiegelt das alte Spotify-Endpoint, quasi drop-in |
| **FreqBlog**                                    | BPM, Key, Camelot, Mood, 44 Felder **per Track-Name** (kein Spotify-ID nötig)                                                         | API-Key            | Free-Tier, sonst guenstig | gut, name-matching                          | Guter Fallback, wenn kein Spotify-ID/ISRC vorhanden                    |
| **Musicae / "Spotify Extended Audio Features"** | Audio-Features + DJ-Scores, drop-in                                                                                                   | API-Key (RapidAPI) | **paid**                  | >250 Mio                                    | Wenn man DJ-Scores/Beatgrids braucht                                   |
| **AcousticBrainz**                              | BPM, Key, Mood-Modelle                                                                                                                | keine              | frei                      | nur Bestand (Sammlung **2022 eingestellt**) | read-only, lueckenhaft — nur als Alt-Daten                             |
| **Essentia / librosa (self-hosted)**            | alles, aus der Audiodatei                                                                                                             | —                  | nur Compute               | eigene Library                              | Fallback/Offline; braucht die Dateien (teils via `music-api`)          |

**Fazit:** ReccoBeats zuerst (kostenlos, kein Auth!), FreqBlog als Name-Fallback. Fuer Tracks ohne
Match -> self-hosted Essentia ODER als "unbekannt" markieren.

## 2. Genre / Stil ("ist das Techno?")

Spotify liefert kein verlaessliches Genre pro Track mehr. Optionen:

| Quelle          | Was                                                                              | Auth                       | Kosten              |
| --------------- | -------------------------------------------------------------------------------- | -------------------------- | ------------------- |
| **MusicBrainz** | Genres/Tags pro Recording (community), + ISRC<->MBID                             | keine (User-Agent Pflicht) | frei                |
| **Last.fm**     | crowd tags, `track.getTopTags`                                                   | API-Key                    | frei                |
| **Discogs**     | Genre/Style (Vinyl/Release-Ebene)                                                | API-Key/OAuth              | frei (rate-limited) |
| **Beatport**    | sehr genaue elektronische Genres, BPM, Key — aber **offiziell keine offene API** | —                          | Scraping/Partner    |

**Fazit fuer "Techno-Filter":** MusicBrainz-Tags + Last.fm-Tags kombinieren; Beatport ist fuer
elektronische Musik am praezisesten, aber nicht offiziell angebunden (rechtlich heikel).

## 3. Library-Ingest (mehrere Quellen pro User)

Ziel: ein Track kann **mehrere Playlist-Quellen** haben (Spotify / SoundCloud / YouTube), die
denselben **Tag** fuellen.

| Dienst              | Endpoints                                                              | Auth                 | Huerden                                                                                    |
| ------------------- | ---------------------------------------------------------------------- | -------------------- | ------------------------------------------------------------------------------------------ |
| **Spotify**         | `/me/playlists`, `/me/tracks`, `/playlists/{id}/items`                 | OAuth (haben wir)    | dev-mode Quota                                                                             |
| **SoundCloud**      | `/users/{urn}/likes/tracks`, `/users/{urn}/likes/playlists`, `/me/...` | **OAuth 2.1 + PKCE** | Neuregistrierung oeffentlich **stark eingeschraenkt**; client_id oft nur "intern" (api-v2) |
| **YouTube (Music)** | `playlistItems.list`, `playlists.list`, liked videos                   | OAuth (Data API v3)  | **10.000 Quota-Einheiten/Tag** (`playlistItems` = 1, `search` = 100)                       |

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

## 6. Discovery / Similarity-Quellen (fuer das Hub-Digging)

Ziel: wie das MMM-Digging (`#digging`) — aus einem Seed (Track/Playlist/Tag) aehnliche Tracks
vorschlagen — aber im Hub, auf der geteilten DB, und mit **mehreren externen Quellen**.

| Quelle                                               | Was                                                                                               | Auth                       | Kosten        | Integrierbar?                                   |
| ---------------------------------------------------- | ------------------------------------------------------------------------------------------------- | -------------------------- | ------------- | ----------------------------------------------- |
| **Hub-intern**                                       | Co-Occurrence: Tracks, die mit dem Seed dieselben Playlists/Tags teilen; "wer hat das sonst noch" | —                          | —             | ✅ selber rechnen (SQL)                         |
| **Last.fm `track.getSimilar` / `artist.getSimilar`** | aehnliche Tracks/Artists aus Hoer-Daten                                                           | API-Key (frei)             | frei          | ✅ sauber                                       |
| **ListenBrainz**                                     | Similar/Labs + Last.fm-kompatibel                                                                 | User-Token (frei)          | frei          | ✅                                              |
| **ReccoBeats**                                       | Track-Recommendations (kein Auth)                                                                 | keine                      | frei          | ✅                                              |
| **DigDeeper.fm**                                     | Audio-Aehnlichkeit (Referenz-Track -> 100 aehnliche), elektronisch                                | **keine oeffentliche API** | Pro 5,49€/Mon | ⚠️ nur **Deep-Link** (Handoff), kein Auto-Query |
| **Spotify Recommendations**                          | —                                                                                                 | —                          | —             | ❌ am 27.11.2024 abgeschaltet                   |

**Fazit:** Das Digging-View aggregiert **Last.fm + ListenBrainz + ReccoBeats + hub-interne
Co-Occurrence** automatisch; **DigDeeper.fm** wird als Deep-Link/Handoff pro Track angeboten
("auf digdeeper.fm oeffnen"). Ergebnisdarstellung: Zeilen = Vorschlaege, **eine Spalte je
Quelle** + Score — konsistent zum Overlap-/Similar-Layout.

## 7. Paid / kommerzielle Datenquellen (Recherche-Update)

**Last.fm: nicht verlassen (Stand 2026-10 verifiziert).** Die API-Doku sagt „available to anyone“.
`/api/account/create` **leitet aber auf die Login-Seite** — man braucht also ein (eingeloggtes)
Last.fm-Konto, um einen Key anzulegen; ein offener Für-sich-Signup existiert nicht. In der
Vergangenheit war die Key-Anlage zeitweise ganz abgeschaltet (SO-Thread 2015, diverse „can’t create
key“-Berichte). Read-only braucht nur den Key, kommerzielle Nutzung braucht extra Vertrag.
**Fazit:** Wer ein Last.fm-Konto + Key hat: nutzen (Top-Tags als Genre-Fallback, ähnliche Tracks).
Nicht als tragende Säule planen — der Adapter ist im Hub bereits key-gated und optional.

Spotify `audio-features` ist tot (Nov 2024). Kommerzielle Ersatzquellen (Preise verifiziert ~2026-09;
Quelle: freqblog.com/compare — **Vendor-Seite, entsprechend parteiisch**):

| Anbieter                      | Felder                                                                                                              | Lookup                                                                  | Preis                                                               |
| ----------------------------- | ------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------- | ------------------------------------------------------------------- |
| **FreqBlog**                  | BPM, Key + **Camelot**, Energy, Danceability, Valence, Loudness, TimeSig, **Mood, Genre**, 44 Felder, `mood_vector` | **Name + ISRC** (kein Spotify-ID nötig), `POST /identify` (Fingerprint) | Free 1.000/mo; Hobbyist £9.99/mo; Pro £129/mo (750k) ≈ **£0.17/1k** |
| **MeloData**                  | BPM + Key (gut), sonst wenig                                                                                        | match→ISRC                                                              | Free 1.000/mo; ab $19/mo; Scale $299/mo (1M) ≈ $0.30/1k             |
| **Musicae** (api.musicae.io)  | BPM, Key, **Camelot**, 9 DJ-Scores                                                                                  | Spotify-ID oder ISRC                                                    | paid (RapidAPI) — DJ-fokussiert                                     |
| **Cyanite**                   | sehr gutes **Mood/Genre**, BPM, Key                                                                                 | **upload-only**                                                         | ab **€290/mo + per track**                                          |
| **Soundcharts**               | Audio-Features + Industry-Intel                                                                                     | —                                                                       | ab $50/mo (10k Queries) ≈ $5/1k                                     |
| **GetSongBPM**                | **nur** BPM + Key                                                                                                   | Name                                                                    | frei (Backlink-Pflicht)                                             |
| **AudD**                      | Recognition, Basis-Meta                                                                                             | Fingerprint                                                             | $5/1k (Entry), $3.60/1k (500k)                                      |
| **MusicAPI.com**              | Streaming-Aggregator (User-Libraries)                                                                               | OAuth                                                                   | €0.60/1k, **€500/mo Minimum**                                       |
| **Describe Music / TrackTag** | AI-Tagging (Genre/Mood/BPM/Key)                                                                                     | upload/API                                                              | Credits bzw. günstiger als Cyanite                                  |
| **SonoVault** (sonovault.now) | **ISRC/ISWC**, Genre, Label, Cross-Platform-IDs (90M+ Tracks, aus Discogs/MusicBrainz/… aggregiert)                 | **ISRC + Reverse-ISRC**                                                 | Free-Tier; **€0–249/mo** (offene Beta)                              |
| **audiometa.io**              | BPM, Key, Play-Counts (Spotify/YT/SC), Social-Follower                                                              | Name                                                                    | **frei**                                                            |

**Fallback-Coverage:** FreqBlog nutzt als 2. Stufe **MusicBrainz → AcousticBrainz** (offener CC0-Datensatz,
7.5M Zeilen) und sonst On-Demand-Analyse. Cyanite/Musiio-artige Modelle analysieren die **Audiodatei**
(am genauesten, teurer).

### Empfehlung

1. **ReccoBeats (frei)** als erster Durchlauf — deckt ~46 %.
2. **FreqBlog** (paid) für den **Rest**: ISRC-Lookup, liefert BPM/Key/**Camelot/Genre/Mood** und passt zum
   Hub (Filter „ist das Techno?“ + harmonisches Digging). Alternativ **MeloData** (billiger bei Volumen,
   aber nur BPM+Key).
3. Für maximale Genauigkeit bei Mood/Genre: **Cyanite** (upload) — nur wenn Genauigkeit > Preis.
4. **Musicae** speziell, wenn DJ-Scores/Camelot im Fokus stehen.

Umsetzung im Hub: Adapter generisch halten (ISRC → features), Reihenfolge **ReccoBeats → bezahlter Fallback
nur bei `found=0`**; Key über `/admin` pflegbar.

## 8. Audio-Ähnlichkeit (DigDeeper-Ersatz) — selbst bauen?

**Kurz: ja.** Wir haben ISRC **und** (via music-api) die **FLAC-Datei** — genau das, was Audio-Similarity
braucht. Es ist ein gut abgetretener Weg, kein Hexenwerk.

### Kommerzielle Alternativen zu DigDeeper

- **cosine.club** — Similarity-Search-Engine (2M+ Tracks, Input per YT/Bandcamp/SoundCloud-Link).
  **Wichtig:** nutzt laut eigener Angabe genau **`discogs-effnet` von Essentia** — bestätigt unseren Bauplan.
- **diggercamp.com** — akustische Ähnlichkeit (techno/house/disco/…), „Scan a record“, findet auf Bandcamp/Revibed/YouTube.
- **Cyanite** — hat Sonic-Similarity-Search (upload-basiert, teuer).
- Chosic (Audio-Feature-Matching), bijou.fm, Sonoteller/Describe Music (Tagging).
- **DigDeeper.fm selbst** bleibt Deep-Link-Only (keine öffentliche API) → nicht integrierbar, nur verlinken.

### Open-Source-Bausteine (GitHub/HF)

| Baustein                                               | Rolle                                                                       |
| ------------------------------------------------------ | --------------------------------------------------------------------------- |
| **Essentia** (MTG-UPF, AGPL)                           | Features: **BPM, Key**(+Camelot), Loudness, Danceability **und** Embeddings |
| **Discogs-EffNet** (Essentia-Model)                    | 1280-dim **Music-Embeddings** — genau das, was cosine.club nutzt            |
| **CLAP** (LAION) / **MuQ-MuLan**                       | audio/Text-Embeddings, sehr gute Perceptual-Alignment (arxiv 2601.19109)    |
| **MERT** (m-a-p/MERT)                                  | Self-supervised Music-Embeddings                                            |
| **OpenL3 / VGGish**                                    | ältere Audio-Embeddings                                                     |
| **Faiss / Qdrant / pgvector / sqlite-vec**             | Vektor-Index (ANN)                                                          |
| **discogs-effnet-onnx** (`Heyian/discogs-effnet-onnx`) | ONNX-Mirror → in **Rust via `ort`** lauffähig, **kein Python nötig**        |
| **Replicate `mtg/effnet-discogs`**                     | Hosted-EffNet (pay-per-call) — Fallback ohne eigenes Hosting                |

Fertige Projekte als Vorlage:

- `andrewbasterfield/apple-music-similarity` — CLAP/MERT-Embeddings → Postgres **pgvector**.
- `TaaroBravo/semantic-audio-search` — **CLAP + Qdrant** (+ FastAPI/Gradio), self-hosted.
- `KonNik88/audio-similarity-tagging-hub` — Precomputed-Embeddings + **Qdrant**.
- `CDrummond/music-similarity` — Essentia-Features + Similarity-API.
- `DadumDeker/MusicAnalysis` — **EffNet-Discogs** + CLAP.
- `msiric/spectune` — **3-Modell-Konsens** (EffNet + MusiCNN + CLAP), audio-first.
- `backblaze-b2-samples/music-tagging-search` — self-hosted: Essentia (BPM/Key/Genre/Mood) + CLAP-Embeddings.
- **DeepCuts** (rlupi.com, jetzt Open Source) — CLAP-**kNN** im Browser/Server, schöne Referenz-UI.
- `rekordcloud/OpenKeyScan` — Offline-**Key-Detection** (AI-Spektrogramm), free/OSS, schreibt in Dateien.

### Unser Plan (im Hub)

1. **FLAC sichern** (music-api: `POST /orders` + `GET /isrc/{isrc}/flac`), nur für Kandidaten/Seeds.
2. **Analyse**: zwei Wege, gleiche Schnittstelle `{bpm, key, camelot, embedding[1280]}`:
   - **(A) ONNX in Rust** — `discogs-effnet-onnx` via `ort`-Crate direkt im Hub-Binary. Lean, kein
     Extra-Container, kein Python. **Bevorzugt** (passt zur Rust-Architektur).
   - **(B) Python-Microservice** (Essentia, AGPL) — mehr Modelle/Features, dafür zweiter Container.
   - Fallback für Tracks ohne FLAC: **Replicate `mtg/effnet-discogs`** (hosted, pay-per-call).
     → löst **gleichzeitig** die BPM/Key-Lücke.
3. **Embeddings speichern** (SQLite-BLOB + Brute-Force-Cosine reicht für ~100k; sonst `sqlite-vec`
   oder Qdrant-Container).
4. **Digging-Source „Audio“**: Seed-Embedding → ANN → Top-N → mit unseren Daten anreichern (wer hat's)
   — genau die bestehende Digging-Enrich-Logik.
5. **Ergebnis:** unser eigenes DigDeeper, auf unserer DB, ohne Per-Track-API-Kosten.

**Lizenz-Hinweis:** Essentia ist **AGPL-3.0** (oder kommerzielle Lizenz von MTG-UPF). Für ein privat
self-hosted Hub unkritisch; bei öffentlichem Betrieb beachten.

**Vorbehalt:** Audio-Analyse braucht die Dateien — für Tracks ohne FLAC keine Ähnlichkeit (oder erst
downloaden). Für elektro/underground ist eigenes Analyisieren aber robuster als Katalog-APIs.

## Quellen (Auswahl)

- Spotify changelog / community: audio-features deprecated 2024-11-27
- ReccoBeats docs: https://reccobeats.com/docs/apis/get-track-audio-features
- FreqBlog: https://freqblog.com/
- AcousticBrainz (shutdown 2022): https://acousticbrainz.org/
- SoundCloud API guide: https://developers.soundcloud.com/docs/api/guide
- YouTube Data API v3 Quota (10k/day): Google docs / 2026 guides
- MusicBrainz: https://musicbrainz.org/doc/Genre
- cosine.club (nutzt Discogs-EffNet): https://cosine.club/
- Essentia-Modelle (Discogs-EffNet): https://essentia.upf.edu/models.html
- EffNet als ONNX: https://huggingface.co/Heyian/discogs-effnet-onnx
- Rust-ONNX-Runtime: https://github.com/pykeio/ort
- SonoVault: https://sonovault.now/ · Musicae: https://api.musicae.io/ · audiometa: https://audiometa.io/
