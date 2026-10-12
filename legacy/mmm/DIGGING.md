# Digging / Curator — Design & Planning

> **Status**: brainstorming / design phase
> **Created**: 2026-05-22
> **Goal**: Werkzeuge um ausgehend von einer Seed-Menge an Tracks (z.B. alle in einem Tag/Playlist) ähnliche Tracks zu finden — erstmal nur lokal vorhandene Files (mit BPM/Key), später ggf. über Spotify API.

---

## Use Case: "collapse-capital"

Tag `Collapse-capital` (id 434) enthält 8 Files (4 unique Tracks, je 2 Versionen):

| File ID | ISRC         | Title                          | Artist                    | BPM   | Key |
| ------- | ------------ | ------------------------------ | ------------------------- | ----- | --- |
| 4042    | US7NS2500009 | Games People Play (Radio Edit) | Paula van Klar            | 140.0 | 3m  |
| 4362    | US7NS2500009 | Games People Play (Radio Edit) | Paula van Klar            | 139.0 | 3m  |
| 4196    | QZ5FN2650988 | The Void                       | Maite Dedecker            | 141.0 | 8m  |
| 4428    | QZ5FN2650988 | The Void                       | Maite Dedecker            | 140.0 | 8m  |
| 5757    | DGA0H2483973 | This Summer                    | Anna Reusch               | 140.0 | 6m  |
| 5769    | DGA0H2483973 | This Summer                    | Anna Reusch               | 139.0 | 6m  |
| 3904    | ?            | Mean One                       | Elon Bass Luciano Bradini | 160.0 | 1m  |
| 4538    | ?            | Mean One                       | Elon Bass                 | 160.0 | 1m  |

**BPM-Cluster der 3 Ziel-Tracks**: 139–141 ("Mean One" bei 160 ist ein Ausreißer)
**Key-Spread**: 3m, 6m, 8m — alle Minor, moderate Sprünge (+3, +2, -2 auf dem Camelot Wheel)
**Genre**: alle "Electro" / "Electro Dance"

Dazu 3 Spotify Service-Tracks (ids 213349, 211699, 213353) mit gleichen ISRCs.

**DB Stats**: 2184 Files mit BPM+Key, 1728 unique ISRCs — genug Pool zum Diggen.

**Ziel**: ~10 Kandidaten-Tracks vorschlagen, per **Embedded Player** anhörbar, dann ggf. ins "collapse-capital" Tag mit aufnehmen.

**Nebenziel**: besseres Tooling zum schnellen Taggen der existierenden Tracks (P/M/V/E vervollständigen).

---

## Bestandsaufnahme: Was haben wir schon?

### Datenbank (relevant)

| Was                         | Wo                                                               |
| --------------------------- | ---------------------------------------------------------------- |
| Lokale Files mit BPM & Key  | `files` Tabelle (`bpm`, `musical_key` als Camelot z.B. "8A")     |
| ISRC auf Files              | `files.isrc` (wird beim Scan aus Audio-Metadaten extrahiert)     |
| ISRC auf Service-Tracks     | `service_tracks.isrc`                                            |
| File ↔ Track Link           | `v_file_track_link` (per ISRC oder direct service_id)            |
| Tag ↔ Playlist Link         | `v_tag_playlist` (case-insensitive name match)                   |
| File → Tags                 | `v_file_tags` / `v_file_resolved_tags`                           |
| Semantische Tag-Ähnlichkeit | `tag_similarities` Tabelle (Embedding-basiert)                   |
| Tag Energy Levels           | `tag_energy_levels` (Energy 0-10 für Phase-Tags)                 |
| Unified Tracks View         | `unified_tracks` (union files + service_tracks)                  |
| Explorer Seeds Tabelle      | `explorer_seeds` (existiert, aber keine CREATE TABLE migration!) |

### Backend — `src/digging.rs` (vorhanden, funktionsfähig)

| Funktion                | Beschreibung                                                          |
| ----------------------- | --------------------------------------------------------------------- |
| `CamelotKey` + `parse`  | Camelot Key Parsing ("8A" → `{pos:8, mode:'A'}`)                      |
| `are_keys_compatible()` | Harmonic Mixing: +1, -1, +2, -2, +7, -7, A↔B, same                    |
| `get_seeds()`           | Files filtern nach BPM, Key, Genre, Play Count, Last Played           |
| `get_suggestions()`     | **Single-seed** Vorschläge: BPM ± range, Camelot match, Scoring       |
| `save_chain()`          | Entdeckte Chain als Tag speichern                                     |
| Tag Energy CRUD         | `get_tag_energy_levels`, `set_tag_energy_level`, `reorder_tags_batch` |

**`get_suggestions()` Scoring-Logik (niedriger = besser):**

- Play Count (max 100) × 2.0
- Last Played Recency (nie gespielt = Bonus -50)
- BPM Diff × 1.5
- Camelot "perfect" match = Bonus -30, "good" = -15
- Nur Tracks die Camelot-kompatibel sind werden zurückgegeben

### Backend — `src/api.rs` (vorhanden, teilweise Stubs)

| Endpoint                                   | Status          |
| ------------------------------------------ | --------------- |
| `GET /api/files`                           | ✅ voll         |
| `GET /api/files/{id}/similar-tracks`       | ✅ voll         |
| `GET /api/explorer/seeds`                  | ✅ funktioniert |
| `POST /api/explorer/seeds`                 | ❌ stub         |
| `DELETE /api/explorer/seeds/{id}`          | ✅ funktioniert |
| `GET /api/explorer/matches`                | ❌ disabled     |
| `POST /api/explorer/matches`               | ❌ stub         |
| `GET/POST/PUT/DEL /api/explorer/presets/*` | ❌ alle stubs   |

### Spotify Client (`src/spotify/client.rs`)

| Methode                 | Vorhanden |
| ----------------------- | --------- |
| `get_playlist()`        | ✅        |
| `get_playlist_tracks()` | ✅        |
| `get_track()`           | ✅        |
| `get_user_playlists()`  | ✅        |
| **Recommendations**     | ❌ fehlt  |

rspotify hat `client.recommendations()` — müsste nur gewrappt werden.
Spotify Recommendations API: `GET https://api.spotify.com/v1/recommendations?seed_tracks=...&limit=20`
Parameter: `seed_artists`, `seed_genres`, `seed_tracks`, `limit`, sowie Ziel-Attribute (`target_energy`, `target_tempo`, etc.)

### Frontend

| Seite            | Status             |
| ---------------- | ------------------ |
| `#digging` Route | ❌ existiert nicht |
| `digging.html`   | ❌ existiert nicht |

---

## Mögliche Ähnlichkeits-Dimensionen

### 1. Camelot Wheel (Harmonic Mixing) ✅ schon da

- Gleiche Key = "perfect"
- Gleiche Position, anderer Mode (A↔B) = "good"
- +1, -1, +2, -2, +7, -7 = "ok"
- **Limit**: nur 1 Seed-Track. Multi-Seed: müsste Kandidaten finden die mit MINDESTENS einem Seed kompatibel sind.

### 2. BPM-Nähe ✅ schon da

- ± N BPM vom Seed (default ±8)
- **Limit**: nur 1 Seed. Multi-Seed: BPM-Range-Union aller Seeds.

### 3. Semantische Tag-Ähnlichkeit ✅ schon da

- `GET /api/files/{id}/similar-tracks` nutzt `tag_similarities` Tabelle
- Findet Files die Tags haben, die den Tags des Seed-Files semantisch ähnlich sind
- **Limit**: single-seed, nur file-basiert

### 4. Shared Tags (gleiche Tags)

- Einfachster Ansatz: Files die in den gleichen Tags/Playlists sind wie die Seeds
- "collapse-capital" selbst ist ein Tag → andere Files im gleichen Tag sind schon bekannt
- Aber: Files die in Tags sind, die OFT ZUSAMMEN mit "collapse-capital" vorkommen
- → das ist quasi was `tag_similarities` macht, aber auf File-Ebene aggregiert

### 5. Spotify Recommendations API ❌ fehlt

- `seed_tracks` (Spotify IDs, aus ISRCs ableitbar)
- `target_*` Attribute: energy, danceability, valence, tempo, ...
- Spotify hat Audio-Features für jeden Track → kann "ähnlich klingende" Tracks empfehlen
- **Cross-Reference**: Spotify-Empfehlungen mit lokalen Files per ISRC matchen
- **Vorteil**: "klingt ähnlich" ist was völlig anderes als Camelot/BPM/Tag-Logik

### 6. Genre / Artist / Album

- Gleicher Artist, gleiches Album, gleiches Genre
- Trivial aber effektiv als Basis-Filter

### 7. Play Count / Last Played

- Schon im Scoring von `get_suggestions()` drin
- "Ungespielte" / "selten gespielte" Tracks bevorzugen → Digging-Charakter

---

## Architektur-Vorschlag

### Neuer Endpoint: `POST /api/digging/suggest`

**Input:**

```json
{
  "seedFileIds": [1, 2, 3],
  "bpmRange": 8.0,
  "camelotJumps": ["+1", "-1", "+2", "-2", "+7", "-7", "a_to_b", "same"],
  "useTagSimilarity": true,
  "useSpotifyRecommendations": false,
  "limit": 10,
  "excludeFileIds": [1, 2, 3]
}
```

**Ablauf:**

1. Lade alle Seed-Files (BPM, Key, Tags)
2. Berechne BPM-Range: [min(bpm) - range, max(bpm) + range]
3. Query alle Files in BPM-Range (ohne die Seeds selbst), die lokale Files sind
4. Für jeden Kandidaten: prüfe Camelot-Kompatibilität mit MINDESTENS einem Seed
5. Score berechnen (multi-seed scoring: best-match pro Dimension)
6. Optional: Tag-Similarity Boosts
7. Optional: Spotify Recommendations quer-checken (ISRC-Abgleich)
8. Sortieren, limitieren, zurückgeben

**Output:**

```json
{
  "seeds": [{ "id": 1, "title": "...", "artist": "...", "bpm": 128.0, "key": "8A" }],
  "suggestions": [
    {
      "id": 42,
      "title": "...",
      "artist": "...",
      "bpm": 130.0,
      "bpmDiff": 2.0,
      "key": "9A",
      "camelotCompatibility": "ok",
      "matchingSeedId": 1,
      "sharedTags": ["dark", "techno"],
      "playCount": 3,
      "lastPlayed": null,
      "hasLocalFile": true,
      "score": 12.5
    }
  ]
}
```

### Frontend: `#digging` SPA-Seite

**Layout-Idee (split-view):**

```
┌──────────────────────────────────────────────────────────┐
│  DIGGING                           [Presets ▼] [Config] │
├──────────────────────────┬───────────────────────────────┤
│  SEEDS (linke Spalte)    │  SUGGESTIONS (rechte Spalte)  │
│                          │                               │
│  Tag/Playlist auswählen: │  ┌─────────────────────────┐  │
│  [collapse-capital  ✕]  │  │ #1 Artist - Title       │  │
│                          │  │    128 BPM · 8A · ⭐3   │  │
│  ┌────────────────────┐  │  │    Tags: dark, techno   │  │
│  │ Track 1            │  │  │    [▶ Preview] [✓ Save] │  │
│  │ Artist · 128 8A    │  │  └─────────────────────────┘  │
│  │ Tags: [P][M][V]    │  │                               │
│  │ [edit tags]        │  │  ┌─────────────────────────┐  │
│  ├────────────────────┤  │  │ #2 Artist - Title       │  │
│  │ Track 2            │  │  │    130 BPM · 9A · ⭐0   │  │
│  │ ...                │  │  │    ...                  │  │
│  └────────────────────┘  │  └─────────────────────────┘  │
│                          │                               │
│  [+ Add seed track]      │  [Load More]                  │
│                          │                               │
│  Filter-Optionen:        │  Sort: [Score ▼] [BPM] [Key] │
│  BPM Range: [±8  ]       │                               │
│  Jumps: [+1][-1][+2]...  │  Batch-Actions:               │
│  [☐] nur lokale Files    │  [Save all as tag...]         │
│  [☐] Spotify einbeziehen │  [Add all to Setlist]         │
│                          │                               │
└──────────────────────────┴───────────────────────────────┘
```

### Workflow

1. User öffnet `#digging`
2. Wählt einen Tag/eine Playlist als Seed-Quelle (z.B. "collapse-capital")
3. Alle Files in diesem Tag werden als Seeds geladen
4. Kann einzelne Seeds de-/selektieren
5. Klick auf "Find Similar" → lädt Vorschläge
6. Vorschläge erscheinen rechts
7. Kann einzelne Vorschläge anhören (wie? ggf. File-Path öffnen oder Vorschau-URL)
8. Kann Vorschläge in den Seed-Tag übernehmen oder einen neuen Tag erstellen
9. Bonus: Side-by-Side Tag-Editing für Seeds (P/M/V/E vervollständigen)

---

## Quick-Tagging Feature (Nebenziel)

"Die existierenden drei Tracks besser taggen" → ein Workflow um P/M/V/E Tags schnell zu setzen.

**Idee: Inline-Tag-Editor in der Seed-Liste**

Jeder Seed-Track zeigt seine aktuellen Tags mit Kategorie-Badges:

```
┌────────────────────────────────────────┐
│ Track: Artist - Title                  │
│ BPM: 128 · Key: 8A                     │
│                                        │
│ Phase:  [peak time        ▼]  ⚡5      │
│ Mood:   [dark             ▼]           │
│ Vibe:   [warehouse        ▼]           │
│ Merkmal:[driving          ▼]  [+add]   │
│                                        │
│ [💾 save tags]                         │
└────────────────────────────────────────┘
```

- Jede Kategorie hat ein Typeahead-Dropdown mit existierenden Tags
- Energy-Slider (0-10) für Phase-Tags
- "Save tags" schreibt Tags via API + updated File-Comment

**API dafür**: existiert schon teilweise:

- `GET /api/tags?category_id=X` → Tags pro Kategorie laden
- `PUT /api/files/{id}/write-comment` → File-Comment neu schreiben
- Brauchen wir noch: `POST /api/files/{id}/tags` um Tags on-the-fly zu setzen

---

## Implementierungs-Plan

### Phase 1: Multi-Seed Suggestion Engine (Backend)

1. **Neuer Endpoint** `POST /api/digging/suggest` in `src/api.rs`
2. **Neue Funktion** `get_multi_seed_suggestions()` in `src/digging.rs`
   - Nimmt mehrere File-IDs
   - Union BPM-Range
   - Camelot-Kompatibilität gegen ALLE Seeds (OR-Logik)
   - Scoring aggregiert über Seeds
   - Optionaler Tag-Similarity Boost
3. **Test mit curl**: `curl -X POST localhost:3000/api/digging/suggest -H 'Content-Type: application/json' -d '{"seedFileIds":[1,2,3],"limit":10}'`

### Phase 2: Frontend `#digging` Seite

1. Neue Page-Module `frontend/pages/digging.js`
2. Register in `app.js` PAGE_MAP
3. Nav-Eintrag in `nav.js`
4. Layout: 2-Spalten (Seeds | Suggestions)
5. Tag-Auswahl als Seed-Quelle (Typeahead wie Tracks-Filter)
6. Konfiguration: BPM Range, Camelot Jumps, Spotify toggle
7. Vorschlags-Liste mit Scoring-Metadaten
8. Actions: Save as tag, Quick-tag seeds

### Phase 3: Spotify Recommendations Integration

1. `src/spotify/client.rs`: `get_recommendations()` wrapper
2. ISRC → Spotify-ID Lookup (Spotify `search?q=isrc:...`)
3. Recommendations abrufen, mit lokalen Files per ISRC cross-referencen
4. In `POST /api/digging/suggest` integrieren (optionaler Parameter)

### Phase 4: Quick-Tag Editor

1. `GET /api/files/{id}/resolved-tags` → alle Tags eines Files mit Kategorien
2. `PUT /api/files/{id}/tags` → Tags setzen (ersetzt aktuelle Tags)
3. Frontend: Inline-Editor mit Typeahead pro Kategorie
4. Integration in `#digging` Seed-Panel UND als standalone Verbesserung

### Phase 5: Tag-Bulk-Editor (Files Page Enhancement)

1. Auf der Files-Page: multi-select + "Quick Tag" Button im Actions Panel
2. Öffnet Modal mit Kategorie-Editoren für alle ausgewählten Files
3. Batch-setzt P/M/V/E Tags + schreibt Comments neu

---

## Entscheidungen (2026-05-22)

1. **Embedded Player**: ✅ Ja — Audio-Player direkt im Frontend, lokale Files über Backend streamen.
   - Backend: `GET /api/files/{id}/stream` → Range-Request-fähiges Audio-Streaming
   - Frontend: `<audio>` Element mit Waveform oder simpler Play/Pause

2. **Spotify Audio Features**: ✅ Ja — als zusätzliche Filter-Dimension vorhalten.
   - `GET /api/spotify/audio-features/{track_id}` → cached in `service_tracks.metadata_json`
   - Filter: `target_energy`, `target_danceability`, `target_valence`, `target_tempo`
   - Erstmal nur fetchen + anzeigen, Filter später

3. **Multiple Tags als Seed**: Ja, aber Workflow-Design braucht Praxis-Erfahrung.
   - Phase 1: Single-Tag Seeds
   - Später: User lernt Workflow, dann Multi-Tag Intersection/Union

## Offene Fragen

1. **Audio-Streaming**: Lokale Files über HTTP streamen — reichen simple GET-Range-Requests?
   Oder wollen wir serverseitiges Transcoding (z.B. FLAC → MP3 fürs Web)?

2. **Dedup vor dem Scoring**: Wir haben 2 Versionen pro Track (verschiedene Qualitäten/Formate).
   Sollen die Suggestions per ISRC dedupliziert werden, oder darf die gleiche ISRC mehrfach auftauchen?

3. **"Mean One" (160 BPM)**: Ist das wirklich Teil von "collapse-capital" oder ein Tagging-Fehler?
   Sollten wir im Digging-Tool Outlier-Detection haben ("dieser Seed weicht stark vom BPM-Mittel ab")?

---

## Phasen-Plan (konkret)

### Phase 0: ISRC-Check ✅ done

- 3 ISRCs gefunden, 8 Files (4 unique), BPM 139-141 (+ Outlier 160)
- Tag "Collapse-capital" (id 434) existiert
- DB: 2184 Files mit BPM+Key, 1728 unique ISRCs

### Phase 1: Multi-Seed Suggestion Engine (Backend)

**Ziel**: `POST /api/digging/suggest` — nimmt mehrere File-IDs, returned scored + ranked Vorschläge

1. **`src/digging.rs`**: Neue Funktion `get_multi_seed_suggestions()`
   - Input: Vec<file_id>, bpm_range, camelot_jumps, limit
   - BPM-Range-Union aller Seeds
   - Camelot-OR-Matching (Kandidat muss mit mind. 1 Seed kompatibel sein)
   - Multi-Seed Scoring: best-match über Seeds, Play Count Bonus, Recency Bonus
   - Dedup per ISRC (eine ISRC = ein Vorschlag, beste Version nach Bitrate/Qualität)
   - Optional: Genre-Filter (Seed-Genre-Intersection als Bias)

2. **`src/api.rs`**: Neuer Handler + Route
   - `POST /api/digging/suggest` → `DiggingSuggestRequest` → `DiggingSuggestResponse`
   - Seeds als File-IDs, optional auch Tag-Name → Backend resolved Files

3. **Test mit curl**: `curl -X POST localhost:3000/api/digging/suggest -H 'Content-Type: application/json' -d '{"seedFileIds":[4042,4196,5757],"limit":10}'`

### Phase 2: Frontend `#digging` + Embedded Player

1. **`frontend/pages/digging.js`** — neue SPA-Seite
   - Tag-Typeahead als Seed-Quelle
   - Seed-Panel (links): geladene Seeds mit BPM/Key/Tags
   - Suggestions-Panel (rechts): gerankte Vorschläge
   - Konfiguration: BPM Range Slider (±2..±20), Camelot Jump Toggles
   - "Find Similar" Button → API-Call → Rendering

2. **Embedded Audio-Player**
   - Backend: `GET /api/files/{id}/stream` — Range-Request Streaming
   - Frontend: `<audio>` Element pro Suggestion-Zeile
   - Play/Pause, ggf. kleine Waveform

3. **Actions**
   - "Add to Tag" → fügt File zum aktuellen Seed-Tag hinzu
   - "Create new Tag" → speichert Selection als neuen Setlist-Tag

4. **Register**: `app.js` PAGE_MAP, `nav.js` TOOLS_ITEMS

### Phase 3: Spotify Audio Features

1. `src/spotify/client.rs`: `get_audio_features(track_ids: &[String])`
2. Bei Sync: Audio Features mitspeichern in `metadata_json`
3. `src/digging.rs`: Optionaler Filter nach Energy/Danceability/Valence
4. Frontend: Anzeige der Features in Suggestion-Cards

### Phase 4: Quick-Tag Editor

1. `GET /api/files/{id}/resolved-tags` — Tags mit Kategorien
2. `PUT /api/files/{id}/tags` — Batch-Tag-Update + Comment-Rewrite
3. Frontend: Inline-Editor in Seed-Panel und Files-Page

### Phase 5: Spotify Recommendations Cross-Reference

1. `src/spotify/client.rs`: `get_recommendations()`
2. ISRC → Spotify-ID Lookup
3. Cross-Reference mit lokalen Files per ISRC
4. Optionaler Blend: DB-basierte + Spotify-Vorschläge

---

## Nächste Schritte

1. **Offene Fragen klären** — Audio-Streaming Range-Requests vs. Transcoding? ISRC-Dedup? Outlier-Handling?
2. **Phase 1 starten** — `POST /api/digging/suggest` implementieren + mit ISRCs testen
3. **Danach**: Frontend-Seite mit Player
