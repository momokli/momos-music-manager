# Progress — Issue #346: Daily/weekly tagging task

Repo: momokli/momos-music-manager · Milestone: hub-tagging-0.27.0 (#33)
Base: main (14fc0eb) · Branch: feature/issue-346-tag-task · PR target: main
Deliverable: pushed branch + open PR against `main`, body contains `Closes #346`.

## Issue (Spec)
**"Daily/weekly tagging task"** — "Uncomplete tracks, most-played first."

Milestone outcome: a track is *tagged-complete* when the 5 describing types are set
(or user-forced); a **daily/weekly tagging task surfaces uncomplete tracks
(most-played first)** in a card-based queue. Per-type "describes the song" toggles
refine the semantics.

Order: #345 → **#346** → #347 → #348.
- #345 (DONE, merged PR #401 / commit 14fc0eb) added:
  - `hub_track_tag_done` (per-user manual "done" mark).
  - `src/tagdone.rs`: `configured_families(pool)`, `status(pool,user,track,types)`
    → `TagDoneStatus { complete, auto, manual, missing }`; `set_done`/`clear_done`.
  - `src/tagtypes.rs`: `DESCRIBE_FAMILIES = [MOOD, GENRE, VIBE, PHASE, MERKMAL]`,
    `TagType::describe_family()` (PHASE_1..5 → "PHASE").
  - setting `ENGINE_TAG_DONE_TYPES` (CSV, default `MOOD,GENRE,VIBE,PHASE,MERKMAL`).
- **#346 = the backend task** (this issue). #347 = card UI, #348 = per-type toggle.

## Scope of #346 (backend only — no UI, that is #347)
Provide a server-side "tagging task": the list of **uncomplete tracks** for a user,
**most-played first**, refreshed on a **daily** or **weekly** cadence (persisted per
period so it is stable within a period). Expose it via the JSON API
(`/api/hub/...`) and a crate-level function the UI (#347) can consume.

## Design (baseline — planner may refine)
- Migration `046_tag_task.sql`:
  - `hub_tag_tasks(id, user_id, cadence, period_start, generated_at, UNIQUE(user_id,cadence,period_start))`.
  - `hub_tag_task_items(task_id, track_id, rank, plays, missing, PRIMARY KEY(task_id,track_id))`.
- `src/tagtasks.rs`:
  - `Cadence { Daily, Weekly }` parse/as_str; `period_start(cadence, now) -> String` (date).
  - `task_for(pool, user_id, cadence, limit)` → current period's task (generate+persist if absent),
    items = uncomplete tracks (per `tagdone` families) not manually done, sorted plays DESC.
  - `generate(...)` internals: single SQL CTE computing per-user plays
    (`hub_traktor_tracks.play_count` + `hub_listening_history` count) and covered
    describing families; filter `covered < required` and not in `hub_track_tag_done`.
- API: `GET /api/hub/tag-task?cadence=daily|weekly` (login required) → JSON task + items.
- Settings: optional cadence/limit knobs (keep minimal).
- Tests: `tests/tag_tasks.rs` — most-played-first ordering, excludes complete/manual-done,
  period stability (same task within period), re-generation on new period, login required.

## Stage status
| Stage     | Status  | Result |
|-----------|---------|--------|
| planner   | done    | Plan below (5 backend user stories, US1–US5) |
| setup     | done    | Branch `feature/issue-346-tag-task` von origin/main (14fc0eb); Baseline `cargo test` grün: 337 passed / 0 failed (40 Suites), Log /tmp/346-baseline.log |
| developer | done    | US1–US5 implementiert + getestet; `cargo test` 350 passed / 0 failed (41 Suites); Commits cf8c1f8, ab89c46, e330750, 0c5eac8, 99fb0fe |
| verifier  | pending | |
| tester    | pending | |
| reviewer  | pending | |

## Plan

Backend-only. Order is dependency-driven: schema → pure period logic → item
computation → persistence/stability → HTTP surface. UI consumption (#347) is
explicitly out of scope; only the crate-level `task_for(...)` is exposed for it.

### Betroffene Dateien
- `music-hub/migrations/046_tag_task.sql` (neu)
- `music-hub/src/tagtasks.rs` (neu)
- `music-hub/src/lib.rs` (`pub mod tagtasks;`)
- `music-hub/src/tagdone.rs` (Familien-Helper für die Batch-Abfrage ergänzen)
- `music-hub/src/settings.rs` (optionale Knobs)
- `music-hub/src/api.rs` (Route + Handler)
- `music-hub/tests/tag_tasks.rs` (neu)

### Domänenregeln (verbindlich für alle Stories)
- **most-played first**: per-User Plays = `hub_traktor_tracks.play_count` (für
  `user_id`) **+** Anzahl `hub_listening_history`-Zeilen (für `user_id`), absteigend.
  Tie-Break deterministisch: `plays DESC, track_id ASC`.
- **tagged-complete** (siehe `tagdone`): `auto` = jede konfigurierte Familie in
  `ENGINE_TAG_DONE_TYPES` hat mind. einen resolved Tag auf dem Track
  (`hub_track_resolved_tags` JOIN `hub_tags`, `PHASE_1..5` → Familie `PHASE`);
  `manual` = Zeile in `hub_track_tag_done(user_id, track_id)`.
- Die Task listet **NUR** Tracks mit `covered < required` **und** ohne
  `hub_track_tag_done`-Zeile (also weder auto- noch manuell-fertig).
- **Kadenz**: pro `(user_id, cadence, period_start)` wird genau ein Task
  persistiert. Innerhalb der Periode stabil (identische `task_id`/Items),
  Neugenerierung erst an der Periodengrenze. `period_start`:
  - daily → `YYYY-MM-DD` (UTC-Kalendertag).
  - weekly → Montag der ISO-Woche als `YYYY-MM-DD`.

### User Stories (in Reihenfolge)

1. **US1 — Migration: Task-Persistenz-Schema**
   - Akzeptanzkriterien:
     - `046_tag_task.sql` legt `hub_tag_tasks` und `hub_tag_task_items` an.
     - `hub_tag_tasks(id INTEGER PRIMARY KEY, user_id, cadence, period_start,
       generated_at, UNIQUE(user_id, cadence, period_start))`, FKs auf
       `hub_users(id)` mit `ON DELETE CASCADE`.
     - `hub_tag_task_items(task_id, track_id, rank, plays, missing,
       PRIMARY KEY(task_id, track_id))`, FKs auf `hub_tag_tasks(id)` und
       `hub_tracks(id)` mit `ON DELETE CASCADE`.
     - Index `idx_hub_tag_task_items_task ON hub_tag_task_items(task_id, rank)`.
     - `cargo test` (Migrationen laufen bei jedem Test-Boot) bleibt grün.
   - Betroffene Dateien: `migrations/046_tag_task.sql`
   - Geschätzte LoC: ~30

2. **US2 — `Cadence` + `period_start` (reine Logik)**
   - Akzeptanzkriterien:
     - `src/tagtasks.rs` mit `pub enum Cadence { Daily, Weekly }`,
       `parse(&str) -> Option<Cadence>` (case-insensitive; `"daily"`/`"weekly"`)
       und `as_str() -> &'static str`.
     - `period_start(cadence, now: DateTime<Utc>) -> String` liefert
       `YYYY-MM-DD`; daily = UTC-Datum, weekly = Montag der ISO-Woche.
     - `lib.rs` registriert `pub mod tagtasks;`.
     - Unit-Tests: Parse-Roundtrip, ungültiger Wert → `None`; daily stabil über
       einen Tag, wechselt bei Tagesgrenze; weekly stabil Mo–So, wechselt am
       Montag; Wochenend-/Jahreswechsel-Fall.
   - Betroffene Dateien: `src/tagtasks.rs`, `src/lib.rs`
   - Geschätzte LoC: ~120

3. **US3 — Item-Berechnung: unvollständig, most-played first**
   - Akzeptanzkriterien:
     - Öffentliche Funktion `candidates(pool, user_id, families: &[String], limit)`
       (oder `generate_items`) liefert geordnete Kandidaten
       `Vec<TagTaskItem { track_id, rank, plays, missing }>`.
     - Per-User-Plays werden per CTE/Subquery aus `hub_traktor_tracks.play_count`
       (+ `hub_listening_history`-Count) für `user_id` berechnet; Tracks ohne
       Traktor-/History-Daten haben `plays = 0` und werden **nicht** ausgeschlossen.
     - Covered-Familien werden in SQL ermittelt (`hub_track_resolved_tags` JOIN
       `hub_tags`, `CASE WHEN t.type LIKE 'PHASE_%' THEN 'PHASE' ELSE t.type END`);
       nur Tracks mit `covered_families < required_families` erscheinen.
     - Tracks mit `hub_track_tag_done`-Zeile (user) sind ausgeschlossen.
     - Reihenfolge `plays DESC, track_id ASC`; `rank` beginnt bei 1.
     - `missing` enthält die fehlenden Familien in kanonischer Reihenfolge
       (`ENGINE_TAG_DONE_TYPES`-Ordnung).
     - `limit` wird respektiert (Default aus Setting, siehe US5).
     - `tagdone`-Familienlogik wird wiederverwendet (Helper `configured_families`
       bzw. ein geteilter `family_of(type)`), keine Zweitimplementierung.
   - Betroffene Dateien: `src/tagtasks.rs`, `src/tagdone.rs` (Helper)
   - Geschätzte LoC: ~180

4. **US4 — `task_for`: Persistenz + Periodenstabilität**
   - Akzeptanzkriterien:
     - `task_for(pool, user_id, cadence, now, limit) -> TagTask` (mit injiziertem
       `now` für deterministische Tests) gibt den aktuellen Perioden-Task zurück.
     - Existiert für `(user_id, cadence, period_start)` bereits ein Task, wird er
       **unverändert** gelesen (keine Neuberechnung, identische `task_id` + Items).
     - Andernfalls: Items via US3 berechnen, `hub_tag_tasks`-Zeile + Items in einer
       Transaktion persistieren (`generated_at = now`).
     - `INSERT ... ON CONFLICT(user_id, cadence, period_start) DO NOTHING` +
       Re-Read, damit ein Parallelaufruf keine Duplikate/Fehler erzeugt.
     - Neue Periode → neue `task_id`, neu berechnete Items.
     - Rückgabetyp enthält `id, cadence, period_start, generated_at, items`.
     - Tests: gleiche Periode zweimal → gleiche `task_id` und gleiche Item-Menge;
       neue Periode (verschobenes `now`) → andere `task_id`.
   - Betroffene Dateien: `src/tagtasks.rs`
   - Geschätzte LoC: ~90

5. **US5 — JSON-API + Settings-Knobs**
   - Akzeptanzkriterien:
     - Route `GET /api/hub/tag-task?cadence=daily|weekly` in `api.rs`, an den
       bestehenden Router gemerged.
     - Login-Pflicht via `crate::web::current_user(&st, &headers)`; ohne Session
       → `401 {"error":"login required"}`.
     - Ungültiges `cadence` → `400`; fehlendes `cadence` → Default (Setting,
       Fallback `daily`).
     - Antwort `{"data": { id, cadence, periodStart, generatedAt, items: [
       { trackId, rank, plays, missing, title, artists } ] }}`; Titel/Artists via
       `hub_tracks`-Join.
     - Optionale Settings (minimal, mit Defaults im `SETTINGS`-Registry):
       `ENGINE_TAG_TASK_LIMIT` (`num`, Default `50`, min 1) und
       `ENGINE_TAG_TASK_CADENCE` (`one_of ["daily","weekly"]`, Default `daily`).
     - Tests (`tests/tag_tasks.rs`, Harness `common::spawn()`):
       - Login erforderlich (401 ohne Cookie).
       - most-played-first-Reihenfolge (Traktor-`play_count` + History-Zeilen).
       - vollständig getaggte Tracks (`auto`) ausgeschlossen.
       - manuell erledigte Tracks (`hub_track_tag_done`) ausgeschlossen.
       - Periodenstabilität: zwei Aufrufe in derselben Periode → gleiche Items;
         per direktem `task_for`-Aufruf mit verschobenem `now` Neugenerierung.
       - `missing` je Item korrekt; ungültiges `cadence` → 400.
   - Betroffene Dateien: `src/api.rs`, `src/settings.rs`, `tests/tag_tasks.rs`
   - Geschätzte LoC: ~150 (Handler ~60, Settings ~20, Tests ~200)

### Reihenfolge
US1 → US2 → US3 → US4 → US5 (US3 hängt an US2s Familien-/Plays-Logik; US4 an
US1+US3; US5 an US4).
