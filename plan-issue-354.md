# Plan — Issue #354: Metadata authority BPM/key backfill for the library

Repo: `momokli/momos-music-manager` · Branch: `feat/music-api-metadata-backfill` · PR target: `main`
Fokus-Milestone: **music-api-0.3.0 (#35)** · Order: #357 → **#354** → #355 → #356

> Kein Code. Reine Spec/Plan. Grundlage: verifizierter Ist-Stand (siehe `progress-issue-354.md`).

---

## 0. Kontext & Abgrenzung (verifiziert)

- **music-api ist die ISRC→BPM/key-Autorität.** `POST /metadata` upsertet
  (`music-api/src/api.rs::post_metadata` → `db::upsert_metadata`), `GET /metadata?isrcs=a,b,c`
  liefert `{items, missing}` (`db::metadata_bulk`), `GET /metadata/{isrc}` (`db::get_metadata`).
- `TrackMetadata` (`music-api/src/models.rs`) serialisiert camelCase:
  `{ isrc, bpm, musicalKey, camelot, title, artist, album, genre, source, updatedAt, key }`
  — `key` ist nur ein **Output-Alias** von `musical_key` (`with_key_alias`).
- Der Hub hat **bereits die Push-Richtung** hub→api (`music-hub/src/music_api.rs`:
  `analysis_metadata`, `push_metadata`, `push_analysis`; CLI `PushMetadata`;
  Console-Button `POST /music-api/push-metadata` in `music_ops.rs`).
- Dem Hub fehlt der **Range-Read** api→hub und der **Backfill** in `hub_track_features`.
- `hub_track_features` (`music-hub/migrations/009_track_features.sql`) wird bisher **nur** von
  `features.rs` (ReccoBeats) und `freqblog.rs` (FreqBlog) gefüllt. Spalten: `track_id PK`,
  `found`, `reccobeats_id`, `isrc`, `bpm REAL`, `key_pitch INTEGER (0..11)`, `key_mode INTEGER (0=minor,1=major)`,
  `camelot TEXT`, `energy`, `danceability`, `valence`, `acousticness`, `instrumentalness`,
  `liveness`, `loudness`, `speechiness`, `source TEXT NOT NULL DEFAULT 'reccobeats'`, `fetched_at TEXT`.
- **Muster für gechunkten, TTL-basierten, fehlerrobusten Pull:**
  `music-hub/src/analysis.rs::refresh_from_vault` (liest `/vault/tracks` → `hub_track_audio`).
  Zentral: `let Some(items) = ...vault_tracks(...).await else { continue };` — ein transienter
  Fehler **skippt** den Chunk, statt „no data" zu schreiben. Dieses Muster wird 1:1 übernommen.
- **`db::normalize_key` (music-api) normalisiert ISRCs / `<source>:<id>`-Track-Keys — NICHT den
  musikalischen Key.** Die im Auftrag vermutete Verbindung ist eine Fehlspur; die relevante
  Ableitung ist unten in §Z dokumentiert.

---

## Z. Mapping-Entscheidung — `musicalKey` ⇒ (`key_pitch`,`key_mode`) ⇒ `camelot`

### Z.1 Warum eine eigene Ableitung nötig ist
`hub_track_features` ist intern konsistent gehalten: `camelot` ist **immer** das Ergebnis von
`features::camelot(key_pitch, key_mode)` (siehe `features.rs::store`). Ein `musicalKey`, das
music-api roh als TEXT hält, ist dagegen **nicht** kanonisch — beobachtete Formen:
Camelot (`"8A"`), Traktor (`"8m"`/`"8d"`), Notennamen (`"Am"`, `"A-Minor"`, `"C"`, `"F#m"`),
Essentia-artig (`"11m"`). `music-api` validiert/kanonisiert es nicht.

### Z.2 Exakte Ableitung (Implementierungs-Spec)
Ziel: aus einem `TrackMetadata`-Item `(bpm, camelot, musicalKey)` `(bpm, camelot, key_pitch, key_mode)`
für `hub_track_features` bestimmen.

1. **Camelot bevorzugen.** Wenn `camelot` gesetzt und wohlgeformt ist (`"^([1-9]|1[0-2])[ABab]$"`,
   getrimmt, upper-cased) → direkt als `camelot` verwenden.
2. **Sonst aus `musicalKey` parsen** (`parse_musical_key(&str) -> Option<String /*camelot*/>`),
   in dieser Priorität:
   - **Camelot-Notation**: `^(1..12)[AB]$` (case-insensitive) → direkt.
   - **Traktor**: `^(1..12)[md]$`, `m`→`A` (minor), `d`→`B` (major) — identisch zu
     `traktor.rs::traktor_key_to_camelot`.
   - **Notenname**: Root-Note (`C,C#,Db,D,D#,Eb,E,F,F#,Gb,G,G#,Ab,A,A#,Bb,B`) + optionale
     Mode-Endung. Endung leer/`major`/`maj`/`d`/`dur` ⇒ major; `m`/`min`/`minor`/`-` ⇒ minor.
     Root-Pitch-Class (Sharps, C=0): `C=0,C#=1,D=2,D#=3,E=4,F=5,F#=6,G=7,G#=8,A=9,A#=10,B=11`;
     Flats enharmonisch: `Db=1,Eb=3,Gb=6,Ab=8,Bb=10`.
     Dann `camelot(pitch, mode)` mit `mode=1` (major) / `mode=0` (minor).
   - Nicht parsebar ⇒ `None` (kein Key, aber BPM ggf. trotzdem schreiben).
3. **`key_pitch`/`key_mode` aus dem finalen `camelot` invertieren** (die Camelot-Tabellen in
   `features.rs::camelot` sind bijektiv). Inverse Tabelle (statt Neuberechnung — hält
   `camelot == camelot(key_pitch,key_mode)` garantiert konsistent):
   - `pitch = Index von camelot in table_minor` falls Endbuchstabe `A`, `key_mode=0`;
   - `pitch = Index von camelot in table_major` falls Endbuchstabe `B`, `key_mode=1`.
   - `table_major = ["8B","3B","10B","5B","12B","7B","2B","9B","4B","11B","6B","1B"]`
     `table_minor = ["5A","12A","7A","2A","9A","4A","11A","6A","1A","8A","3A","10A"]`
   - Referenz-Checks: `8A`→pitch 9,mode 0 (A minor); `8B`→pitch 0,mode 1 (C major);
     `8m`→`8A`; `11m`→`11A`; `"C"`→`8B`; `"Am"`→`8A`.
4. **BPM** wird direkt aus `item.bpm` übernommen (falls `> 0`; `None` sonst).
5. `energy` bleibt unberührt (`hub_track_features.energy` ist ReccoBeats' Domäne; music-api`s
   `/metadata` liefert kein energy im Range-Read — nur `/vault/tracks` tut das).

> Diese Funktion gehört als reine, unit-testbare Helper `music_api::parse_musical_key` (oder
> `features::key_to_camelot`) neben `features::camelot`. Keine Netz-/DB-Abhängigkeit.

---

## 1. User Stories (in Reihenfolge)

### US-1 — Hub-Client: `/metadata`-Range-Read
**Als** Hub **will ich** BPM/key für eine ISRC-Liste gebündelt von music-api lesen,
**damit** ich die Bibliothek ohne N Einzel-Requests anreichern kann.

- **Akzeptanzkriterien**
  - Neue async fn in `music-hub/src/music_api.rs`:
    `pub async fn metadata_range(cfg: &Config, isrcs: &[String]) -> Option<Vec<MetadataItem>>`
    (`tracks` als Namens-Vorbild; `Option` für Transient-vs-Leer).
  - Antwort-Shape: `GET {music_api_base}/metadata?isrcs=a,b,c` (Bearer `MUSIC_API_TOKEN`),
    Response `{items:[TrackMetadata], missing:[isrc]}`.
  - Deserialisierung über ein `#[derive(Deserialize)] #[serde(rename_all="camelCase")]`-Struct
    `MetadataRow { isrc, bpm: Option<f64>, musical_key: Option<String> (alias "key"),
    camelot: Option<String>, source: Option<String>, updated_at: i64 }`. **Robust gegen fehlende
    Felder** (`#[serde(default)]`) und camelCase wie die API.
  - `None` bei: fehlendem Token, `!status.is_success()`, Netz-/JSON-Fehler (identisch
    `vault_tracks`). `Some(vec![])` bei leerer Eingabe.
  - `missing` wird mitgeliefert (Rückgabe `(Vec<MetadataRow>, Vec<String>)` **oder** ein
    `RangeReply{items, missing}`) — der Backfill braucht `missing` für TTL-Marker.
- **Betroffene Dateien:** `music-hub/src/music_api.rs`
- **Geschätzte LOC:** ~45 (+ Tests)

### US-2 — Backfill-Loop `hub_track_features` aus music-api (source='music-api')
**Als** Betreiber **will ich**, dass fehlende BPM/key aus der ISRC-Autorität die Bibliothek
füllen, **damit** ReccoBeats-Lücken geschlossen werden.

- **Akzeptanzkriterien**
  - Neue fn (z. B. `music-hub/src/features.rs::backfill_from_music_api(pool, cfg, limit) -> Result<usize>`)
    nach dem Muster `analysis::refresh_from_vault`.
  - **Selektion (bounded):** Tracks mit `hub_tracks.isrc` nicht-leer und **ohne brauchbares
    BPM/key** in `hub_track_features` — d. h. `f.track_id IS NULL OR (f.bpm IS NULL AND f.camelot IS NULL)`.
  - **TTL:** ein „geprüft, keine Daten"-Marker wird erst nach `RECHECK_DAYS` (7, wie `analysis.rs`)
    erneut abgefragt. Dafür **Migration 050** `050_track_features_metadata_check.sql`:
    `ALTER TABLE hub_track_features ADD COLUMN metadata_checked_at TEXT;` (nullable).
    Selektion: `AND (f.metadata_checked_at IS NULL OR f.metadata_checked_at < ?ttl)`.
  - **Chunking:** ISRCs in Chunks (z. B. `API_BATCH = 100`) → `metadata_range`.
  - **Kein „no data"-Poisoning:** Ist `metadata_range` `None` (transienter Fehler) → `continue`
    (Chunk komplett überspringen; weder Daten noch `metadata_checked_at` schreiben).
  - **Hit** (Item mit `bpm` oder `camelot`/parsebarem `musicalKey`): UPSERT in
    `hub_track_features` mit `bpm` (**COALESCE**, vorhandenes nicht überschreiben),
    `camelot` (COALESCE), `key_pitch`/`key_mode` (aus §Z), `isrc`, `found=1`,
    `source='music-api'`, `fetched_at=now`, `metadata_checked_at=now`.
  - **Miss** (ISRC in `missing` oder Item ohne verwertbare Werte): **nur** `metadata_checked_at=now`
    setzen (+ ggf. Row mit `found=0` anlegen, **ohne** `source` zu verändern), niemals vorhandene
    Werte löschen.
  - Rückgabe = Anzahl tatsächlich geschriebener Daten-Zeilen (`written`).
  - `cfg.music_api_token.is_none() || limit == 0` ⇒ `Ok(0)`.
- **Betroffene Dateien:** `music-hub/src/features.rs`, `music-hub/migrations/050_*.sql`
- **Geschätzte LOC:** ~90 (+ Migration)

### US-3 — CLI-Subcommand
- **Akzeptanzkriterien**
  - Neuer Subcommand in `music-hub/src/main.rs`:
    `MetadataBackfill { #[arg(long, default_value_t = 2000)] limit: usize }`
    (Name analog `Firebase`/`Features`; Doku-Kommentar „Backfill BPM/key from music-api (ISRC authority)").
  - Handler `cmd_metadata_backfill(cfg, limit)`: `db::connect` → `settings::overlay_config`
    → Token-Check (`bail!` wie `cmd_push_metadata`) → Loop bis `limit` erreicht/erschöpft
    (wie `features::backfill`) → Abschluss-Print (`✓ N Tracks aus music-api angereichert`).
  - `--help` nennt Zweck + `limit`.
- **Betroffene Dateien:** `music-hub/src/main.rs`
- **Geschätzte LOC:** ~30

### US-4 — Optional: Console-Button
- **Akzeptanzkriterien**
  - Route `POST /music-api/backfill-metadata` in `music-hub/src/music_ops.rs` (Muster
    `push_metadata_action`: `crate::ui::nav(...).await.is_none()` ⇒ `/login`; Token-Check;
    `action_response` mit htmx-Toast).
  - Button in `music-hub/templates/music_api.html` neben „BPM/Key pushen":
    `<button hx-post="/music-api/backfill-metadata" hx-swap="none" hx-disabled-elt="this" class="secondary">BPM/Key holen</button>`.
  - Antwort: `Ok(0)` ⇒ „Keine offenen Tracks", `Ok(n)` ⇒ „{n} Tracks angereichert", `Err` ⇒ Fehler-Toast.
- **Betroffene Dateien:** `music-hub/src/music_ops.rs`, `music-hub/templates/music_api.html`
- **Geschätzte LOC:** ~25

### US-5 — Tests
- **Akzeptanzkriterien**
  - `music_api.rs` Tests: (a) `MetadataRow` deserialisiert camelCase + fehlende Felder;
    (b) `parse_musical_key`: `"8A"→"8A"`, `"8m"→"8A"`, `"8d"→"8B"`, `"Am"→"8A"`, `"C"→"8B"`,
    `"F#m"`→correkt, `""`/`"xx"`→`None`; (c) inverse Camelot-Ableitung `camelot→(pitch,mode)`
    stimmt mit `features::camelot` überein (Round-Trip-Test über alle 24 Keys).
  - `features.rs` Tests: `backfill_from_music_api` Selektions-/UPSERT-Semantik.
    (DB-abhängige Tests nur falls bestehendes Muster vorhanden; sonst reine Helper-Tests +
    ein `#[sqlx::test]`/In-Memory-Fall analog vorhandener DB-Tests — **prüfen**.)
  - Kein neuer Test auf music-api-Seite (Route unverändert, #357 deckt sie ab).
- **Betroffene Dateien:** `music-hub/src/music_api.rs` (`mod tests`), `music-hub/src/features.rs` (`mod tests`)
- **Geschätzte LOC:** ~70

### US-6 — Docs
- **Akzeptanzkriterien**
  - `music-hub/AGENT.md`: Backfill-Pfad + „music-api ist ISRC-Autorität" dokumentieren.
  - `music-hub/README.md`: CLI-Beispiel `cargo run -- metadata-backfill --limit 2000`.
  - `music-hub/CHANGELOG.md`: Eintrag „Added: BPM/key backfill from music-api into
    hub_track_features (source='music-api')".
  - Optional `docs/DECISIONS.md`: ADR-Notiz „musicalKey→camelot Ableitung (§Z)".
- **Betroffene Dateien:** `music-hub/AGENT.md`, `music-hub/README.md`, `music-hub/CHANGELOG.md`,
  ggf. `docs/DECISIONS.md`
- **Geschätzte LOC:** ~25

---

## 2. Betroffene Dateien (Gesamt)

| Datei | Änderung |
|---|---|
| `music-hub/src/music_api.rs` | `MetadataRow`, `metadata_range`, `parse_musical_key`, inverse Camelot-Helper + Tests |
| `music-hub/src/features.rs` | `backfill_from_music_api`, UPSERT-Helper für metadata-source, Tests |
| `music-hub/migrations/050_track_features_metadata_check.sql` | `metadata_checked_at TEXT` |
| `music-hub/src/main.rs` | `MetadataBackfill` + `cmd_metadata_backfill` |
| `music-hub/src/music_ops.rs` | `POST /music-api/backfill-metadata` |
| `music-hub/templates/music_api.html` | Button |
| `music-hub/AGENT.md`, `music-hub/README.md`, `music-hub/CHANGELOG.md` | Docs |
| `docs/DECISIONS.md` (optional) | ADR-Notiz Mapping |
| **unverändert** | `music-api/*` (#357 deckt die Route ab) |

---

## 3. Implementierungs-Reihenfolge

1. **US-1** Client `metadata_range` + `MetadataRow` (Basis für alles).
2. **§Z-Helper** `parse_musical_key` + inverse Camelot-Ableitung (pure, testbar) → round-trip-Test.
3. **Migration 050** `metadata_checked_at`.
4. **US-2** `backfill_from_music_api` (Selektion/TTL/Chunking/Poisoning-Schutz/UPSERT).
5. **US-3** CLI-Subcommand.
6. **US-4** Console-Button.
7. **US-5** Tests komplettieren (Rust-Tests, Mapping-Round-Trip, Selektions-Semantik).
8. **US-6** Docs + CHANGELOG.

**Abhängigkeiten:** US-1 → US-2/3/4; §Z → US-2; US-2 → US-3/4; Tests (US-5) begleiten 1–4,
Docs (US-6) zuletzt.

---

## 4. Risiken / offene Punkte

- **`musicalKey`-Formate** sind nicht kanonisch; §Z deckt die beobachteten Notationen ab, ein
  unbekanntes Format führt bewusst zu „BPM bleibt, Key leer" statt zu Fehlmapping.
- **`source`-Spalte ist einwertig:** ReccoBeats- und music-api-Zeilen teilen sich eine Row.
  Regel: `source='music-api'` nur bei daten-beitragendem Write; No-Data-Marker ändern `source`
  nicht (kein Clobbern der Herkunft).
- **TTL-Trennung:** eigenes `metadata_checked_at` statt `fetched_at`, damit ein music-api-Miss
  den ReccoBeats-Recheck-Zyklus nicht verkürzt.
- **DB-abhängige Tests:** vor US-5 prüfen, welches Test-Setup der Hub nutzt (sqlx::test vs.
  In-Memory), um keine neue Infrastruktur zu erfinden.
