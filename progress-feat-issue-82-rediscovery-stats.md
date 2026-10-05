# Progress — Issue #82: `GET /api/rediscovery/stats`

Repo: `momokli/momos-music-manager` · Branch: `feat/issue-82-rediscovery-stats`
Base: `origin/main` @ `6da4728` (#113 merged) · PR target: `main` (independent)
Milestone: 1.15.0 (Rediscovery-Query-Engine)

## Stage-Status (Source of Truth)

| Stage | Status | Result |
|-------|--------|--------|
| 1 planner | done | Plan in „## Plan (planner)“ (5 Stories, Design-Entscheidungen 1–5 übernommen) |
| 2 setup | done | Branch `feat/issue-82-rediscovery-stats` @ origin/main `6da4728` (0/0 divergence, nur untracked Progress-Datei). `cargo build` EXIT:0. `TMPDIR=/dev/shm cargo test --test api_rediscovery` EXIT:0 (15 passed / 0 failed). CHANGELOG: `[Unreleased]`-Abschnitt vorhanden, kein Versions-Bump. |
| 3 developer | done | Stories 1–5 umgesetzt. `cargo build` EXIT:0. `TMPDIR=/dev/shm cargo test --test api_rediscovery --test api_rediscovery_stats` EXIT:0 (15 + 7 passed / 0 failed). Commits via clanker-git (momo-clanker[bot]). |
| 4 verifier | pending | |
| 5 tester | pending | |
| 6 developer (PR) | pending | |
| 7 reviewer | pending | |

## Auftrag (aus Issue #82)

Neuer Endpunkt `GET /api/rediscovery/stats` — Kennzahlen zum aktuellen Preset,
über **dieselben Facetten-Parameter** wie `candidates`.

Antwortform:
```jsonc
{
  "matching": 4123,
  "withBpmAndKey": 2870,
  "needsAnalysis": 1253,
  "notOwned": 96,
  "pushedRecently": 240,
  "byBpmBucket": { "120-124": 300, "125-129": 512, "...": 0 },
  "byLiked": { "liked": 3900, "unliked": 223 }
}
```

Definitionen:
- `matching` = Treffer der Facetten (gesamter Kandidaten-Set, ohne Pagination).
- `withBpmAndKey` = Treffer mit verknüpftem File **und** `bpm` **und** `musical_key`.
- `needsAnalysis` = Treffer mit verknüpftem File **ohne** `bpm`/`musical_key` → Arbeitsmenge 1.17.0.
- `notOwned` = Treffer ohne lokale `file_locations` → Arbeitsmenge 1.18.0.
- `pushedRecently` = Treffer mit Push im `excludePushedSinceDays`-Fenster.
- `byBpmBucket` = Verteilung der `withBpmAndKey` über die geteilte Bucket-Definition.
- `byLiked` = liked/unliked-Split der Treffer.

## DoD (aus Issue)

- [ ] 200, alle Zähler konsistent (Summe der Buckets = `withBpmAndKey`)
- [ ] `needsAnalysis` + `withBpmAndKey` + Tracks ganz ohne File = `matching`
- [ ] `notOwned` = Tracks ohne lokale `file_locations` (Definition für order-missing 1.18.0)
- [ ] Facetten-Parameter identisch zu `candidates` (gemeinsame Deserialisierungs-Struct, Test `likedOnly=true`)
- [ ] Bucket-Definition in **genau einer** Konstante

## Vorhandener Stand (recon, origin/main @ 6da4728)

- `src/api/rediscovery.rs` — `candidates_handler`, `RediscoveryCandidatesQuery`
  (serde camelCase), Facetten-Filterung (touched_before_days, max_playlists,
  liked_only, exclude_backpack, exclude_pushed_since_days, play_count_max,
  not_played_since_days, require_bpm/key, bpm_min/max, keys, genres), batched loads.
- `src/db/rediscovery.rs` — `list_forgotten_facts`, `load_file_facts`,
  `owned_track_ids`, `tracks_matching_audio`, `pushed_track_ids_since`,
  `last_push_dates`, `track_metadata`, `AudioFilter`, `FileFacts`, `TrackFacts`.
- `src/api/mod.rs:46` mergt `rediscovery::router()`.
- Tests: `tests/api_rediscovery.rs` (+ `tests/common/mod.rs::seed_rediscovery_data`,
  `src/db/testing.rs::seed_rediscovery_scenario`, anchor `REDISCOVERY_SEED_EPOCH=1_790_000_000`).

Seed-Fixture Tracks 10–18 (+ Basis 1–3): bpm/key 120/1a, 124/4m, 128/8m, 140/12a,
155/1a, NULL/NULL (15), 120/4m (16, Backpack, pcount 1), 124/8m (17, push frisch),
128/12a (18, push alt). Alle Files haben lokale `file_locations`.

## Design-Entscheidungen (Orchestrator, bindend)

1. **Gemeinsame Query-Struct**: `RediscoveryCandidatesQuery` → verschoben/umbenannt
   in eine geteilte Struct (z. B. `RediscoveryFacetsQuery`, weiterhin serde camelCase,
   gleiche Felder/Defaults). `candidates_handler` UND `stats_handler` nutzen sie.
2. **Facetten-Filterung teilen**: die Survivor-Berechnung (last_touched +
   max_playlists + liked_only + exclude_backpack + exclude_pushed + audio) in eine
   gemeinsame Funktion auslösen, damit stats strukturell identisch filtert.
   Pagination/Sortierung bleiben candidates-spezifisch.
3. **Bucket-Konstante**: `pub const BPM_BUCKET_WIDTH: f64 = 5.0;` + genau eine
   Funktion `pub fn bpm_bucket_label(bpm: f64) -> String` in `src/db/rediscovery.rs`
   (Label z. B. `"120-124"`, ganzzahlig, floor auf Bucket-Start, `start..start+4`).
   Alles (stats jetzt, `bpmBuckets`-Preset 1.16.0 später) nutzt diese eine Definition.
   Buckets mit 0 Treffern werden NICHT emittiert (nur belegte Buckets) — Summe = `withBpmAndKey`.
4. **Billig bleiben**: Aggregat/Counts über den batched geladenen Kandidaten-Set,
   kein Laden aller Rows pro Row; keine neue Migration nötig.
5. **Kein Versions-Bump**: CHANGELOG unter `[Unreleased]`.

## Plan (planner)

### Betroffene Dateien
- `src/api/rediscovery.rs` — geteilte Query-Struct, geteilte Facetten-Funktion,
  `stats_handler`, Stats-Response-Typen.
- `src/db/rediscovery.rs` — `BPM_BUCKET_WIDTH` + `bpm_bucket_label`.
- `tests/api_rediscovery_stats.rs` (neu) — Integrationstests des Stats-Endpunkts.
- `CHANGELOG.md` — Abschnitt `[Unreleased]` (kein Versions-Bump).
- `src/api/mod.rs` — **keine Änderung** (Route hängt in `rediscovery::router()`).

### Bindende Design-Entscheidungen (1–5) — Umsetzung
1. `RediscoveryCandidatesQuery` wird zu `RediscoveryFacetsQuery` (serde camelCase,
   Felder/Defaults unverändert) umbenannt; `candidates_handler` **und** `stats_handler`
   binden denselben Typ. `stats_handler` ignoriert `limit`/`offset`/`sort`/`seed`
   (keine Pagination), validiert aber `bpmMin>Max` identisch.
2. Die Survivor-Berechnung (last_touched → max_playlists → liked_only →
   exclude_backpack → exclude_pushed → audio) wird aus `candidates_handler` in eine
   gemeinsame Funktion gezogen. Sie liefert die Survivor-Liste **und** zusätzlich den
   Zähler `pushed_recently` = Tracks, die alle Facetten **außer** `excludePushed`
   erfüllen und am Push-Facet herausfallen. Pagination/Sortierung bleiben in
   `candidates_handler`.
3. `pub const BPM_BUCKET_WIDTH: f64 = 5.0;` und genau eine
   `pub fn bpm_bucket_label(bpm: f64) -> String` in `src/db/rediscovery.rs`
   (floor auf Bucket-Start, Label `"{start}-{start+4}"`, z. B. `120.0→"120-124"`).
   Nur belegte Buckets werden emittiert.
4. Stats arbeitet rein aggregierend über den einen batched geladenen
   Survivor-Set (`list_forgotten_facts` + `load_file_facts` + `owned_track_ids` +
   optional `pushed_track_ids_since`); kein Pro-Row-Laden, keine neue Migration.
5. Kein Versions-Bump; CHANGELOG-Eintrag unter `[Unreleased]`.

### Fachliche Definitionen (umgesetzt im Aggregat)
- `matching` = Survivor-Anzahl (== `candidates.total` bei gleichen Parametern).
- `withBpmAndKey` = Survivor mit gelinktem File **und** `bpm` **und** `musical_key`.
- `needsAnalysis` = Survivor mit gelinktem File **und** fehlendem `bpm` **oder** `key`.
- Tracks ganz ohne File = Survivor, der in der `load_file_facts`-Map fehlt.
  Invariante: `withBpmAndKey + needsAnalysis + noFile == matching`.
- `notOwned` = Survivor ohne lokale `file_locations` (`owned_track_ids`-Semantik;
  Tracks ohne File zählen mit).
- `pushedRecently` = Zähler aus der gemeinsamen Funktion (siehe Entscheidung 2),
  `0` wenn `excludePushedSinceDays` deaktiviert (0/None).
- `byLiked` = liked/unliked-Split der Survivor; Summe == `matching`.
- `byBpmBucket` = Verteilung von `withBpmAndKey`; Summe der Werte == `withBpmAndKey`.

### User Stories (in Reihenfolge)

1. **Geteilte Facetten-Query-Struct**
   - Akzeptanzkriterien: `RediscoveryFacetsQuery` existiert mit denselben Feldern/
     camelCase-Deserialisierung wie bisher; `candidates_handler` nutzt sie; alle
     bestehenden `tests/api_rediscovery.rs` bleiben grün (kein Verhaltenswechsel).
   - Dateien: `src/api/rediscovery.rs`.
   - LOC: ~10 (reine Umbenennung).

2. **Bucket-Definition als Single Source**
   - Akzeptanzkriterien: `BPM_BUCKET_WIDTH` und `bpm_bucket_label` in
     `src/db/rediscovery.rs`; Unit-Tests decken Grenzen ab (`120.0→120-124`,
     `124.9→120-124`, `125.0→125-129`, `128.0→125-129`, `155.0→155-159`).
   - Dateien: `src/db/rediscovery.rs`.
   - LOC: ~20.

3. **Gemeinsame Facetten-Filterfunktion**
   - Akzeptanzkriterien: Survivor-Berechnung liegt in einer Funktion, die von
     `candidates_handler` aufgerufen wird; `candidates`-Tests unverändert grün;
     Funktion liefert Survivor + `pushed_recently`.
   - Dateien: `src/api/rediscovery.rs`.
   - LOC: ~60 (Verschiebung + Rückgabetyp).

4. **`GET /api/rediscovery/stats` (Aggregat)**
   - Akzeptanzkriterien: 200 mit `matching`, `withBpmAndKey`, `needsAnalysis`,
     `notOwned`, `pushedRecently`, `byBpmBucket`, `byLiked`; gleiche Struct wie
     `candidates` deserialisiert Facetten; `bpmMin>Max` → 400; unbekannter `sort`
     wird ignoriert (Stats paginiert nicht).
   - Dateien: `src/api/rediscovery.rs`.
   - LOC: ~90.

5. **Invarianten- und Paritäts-Tests**
   - Akzeptanzkriterien (Fixture 10–18 + Basis 1–3):
     - Bucketsumme == `withBpmAndKey`; `withBpmAndKey+needsAnalysis+ohneFile == matching`.
     - `notOwned` zählt Tracks ohne lokale Location (Basis-Track 2 nur `backup`,
       Basis-Track 3 ohne File) → unscoped `excludePushed=0` `touchedBeforeDays=30`
       ergibt `matching=12`, `withBpmAndKey=10`, `needsAnalysis=1`, `notOwned=2`,
       `byLiked={liked:11,unliked:1}`.
     - Facetten-Parität: `stats.matching == candidates.total` bei identischer Query.
     - `likedOnly=true` → nur gelikte Survivor; gleiche Struct, `unliked=0`.
     - `excludePushedSinceDays=180` senkt `matching` (Track 17 raus) und meldet
       `pushedRecently=1`; `=0` deaktiviert und ergibt `pushedRecently=0`.
   - Dateien: `tests/api_rediscovery_stats.rs`, ggf. `tests/common/mod.rs` unverändert.
   - LOC: ~150.

### Implementierungs-Reihenfolge
1 → 2 → 3 → 4 → 5. Story 3 hält `candidates` grün, bevor Stats (4) darauf aufsetzt.

### Offene Risiken / zu bestätigende Punkte
- **`pushedRecently`-Semantik** mehrdeutig: Issue-Satz „Treffer mit Push im Fenster“
  ergibt bei aktivem Default-Facet (`excludePushedSinceDays=180`) rein logisch `0`.
  Der Plan wählt die einzige mit dem Beispiel (240>0) konsistente Lesart:
  „durch das Push-Facet herausgefallene Sonst-Treffer“. Vor Story 4 bestätigen.
- `stats.matching == candidates.total` gilt nur bei identischen Parametern; bei
  `sort`/`limit`-Abweichung bleibt `matching` paginationsunabhängig.

## Orchestrator-Entscheid (pushedRecently) — bindend

`pushedRecently` = Anzahl der Tracks, die alle übrigen Facetten erfüllen, aber am
Push-Cooldown-Facet (`excludePushedSinceDays`) herausfallen (= `matching` **ohne** diese
Facette minus `matching` **mit** Facette). Bei deaktiviertem Facet (`0`/`null`) ist
der Wert `0`. Lesart (b) des Plans ist bestätigt — nur sie ist mit dem Beispiel
(>0 trotz aktivem Default-Facet) konsistent. Die Implikation „pushedRecently ist
nicht Teilmenge von matching“ ist gewollt.

## PR-Konvention

PR-Body MUSS `Closes #82` enthalten (nicht „Refs #82").

## Blocker

(keine bisher)
