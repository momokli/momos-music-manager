# Progress — Issue #354: Metadata authority BPM/key backfill for the library

Repo: momokli/momos-music-manager · Branch: `feat/music-api-metadata-backfill` · PR target: `main`
Fokus-Milestone: music-api-0.3.0 (#35) · Order: #357 → **#354** → #355 → #356

## Issue
- #354 (OPEN, `area:api`, `orchestrator:dispatched`): Titel „Metadata authority BPM/key backfill
  for the library". Body: „ISRC -> BPM/key; backfill hub_track_features."
- Milestone #35 Outcome: „a native range route lets the Hub batch-enrich, and an ISRC→BPM/key
  backfill fills `hub_track_features`."

## ALREADY-DONE-Check (result: NEIN — bauen)
- #357 (Native metadata range route) ist auf `main` vorhanden: `music-api/src/api.rs`
  `GET /metadata?isrcs=a,b,c` → `{items, missing}` (+ `GET /metadata/{isrc}`). Kein PR nötig.
- PR #302 (merged 2026-10-11, `Closes #301`, Commit `0343da5`) baute die **Push**-Richtung
  hub→api (`push_metadata`, `analysis_metadata`, `POST /metadata`) plus die Range-Route.
- Die **Backfill**-Richtung api→hub in `hub_track_features` fehlt: Autoren von
  `hub_track_features` sind nur `features.rs` (ReccoBeats) und `freqblog.rs` (FreqBlog).
  Kein Pfad liest music-api `/metadata` und schreibt `hub_track_features`.
- `hub_track_audio` (migration 048, `analysis.rs::refresh_from_vault`) liest bereits
  `/vault/tracks` → **anderer** Lesemodell-Pfad, nicht `hub_track_features`.

## Ist-Stand (relevante Dateien)
- `music-api/src/api.rs`: `GET/POST /metadata`, `GET /metadata/{isrc}`; `MetadataQuery{isrcs,limit}`.
- `music-api/src/models.rs`: `TrackMetadata` (isrc,bpm,musicalKey,camelot,title,artist,album,genre,source,updatedAt, key-alias).
- `music-api/src/db.rs`: `metadata_bulk`, `upsert_metadata`, `normalize_key`.
- `music-hub/src/music_api.rs`: Client — u.a. `analysis_metadata`, `push_metadata`,
  `push_analysis`, `vault_tracks`; **kein** `/metadata`-Range-**Read**.
- `music-hub/src/features.rs`: `hub_track_features` (ReccoBeats) + `sync_once`, `backfill`.
- `music-hub/src/analysis.rs`: `refresh_from_vault` (vault→`hub_track_audio`) — Muster für Chunking/TTL.
- `music-hub/src/main.rs`: CLI (`Features`, `PushMetadata`, `Backfill*`, …).
- Migrationen: `music-hub/migrations/009_track_features.sql` (hub_track_features, source default 'reccobeats').

## Zu bauen (Vorschlag, Planner bestätigt)
1. Hub-Client: `metadata_range(cfg, isrcs)` → `GET /metadata?isrcs=` lesen (None bei Fehler).
2. Backfill (z. B. `features::backfill_from_music_api` o. neues Modul): Tracks mit ISRC ohne
   brauchbares BPM/Key in `hub_track_features` → api abfragen → UPSERT (source='music-api',
   bpm, camelot; key_pitch/key_mode aus musicalKey ableiten). Bounded/gechunkt, TTL wie analysis.rs,
   transienter Fehler darf **keine** „no data"-Zeile schreiben.
3. CLI-Subcommand + optional Console-Button.
4. Tests (hub: Client-Mapping + UPSERT; api unverändert).
5. Docs (music-hub AGENT/README, evtl. ADR) + CHANGELOG-Notiz.

## Pipeline-Stages
- [x] plan · [x] setup · [x] develop · [x] verify · [x] test · [x] pr · [ ] review

## Setup (done)
- Branch-Stand verifiziert (Host `planet`, lokal ausführbar):
  - `clanker-git fetch origin` → clean, kein Update nötig.
  - `clanker-git status -sb` → `## feat/music-api-metadata-backfill...origin/main`,
    nur untracked `plan-issue-354.md` + `progress-issue-354.md`; keine lokalen Code-Änderungen.
  - `clanker-git log --oneline -3` → `ec1b928 chore(release): v0.28.0`, `40cf555 feat(hub): import-history UI polish`,
    `9cf473a feat(hub): admin/ops console (#350)`. Branch basiert auf aktuellem `origin/main`.
- **Build-Baseline (Rust, cargo 1.97.1, `--locked`):**
  - `music-hub`: `cargo build --locked` → **OK** (39.1s). 1 vorbestehende Warnung
    (`src/web.rs:997` `TrackPage.tags`/`me` never read, dead_code) — nicht blockierend.
  - `music-hub`: `cargo test --locked` → **OK, 419 passed / 0 failed** (alle Binaries: lib 81,
    `tests/*.rs` inkl. `ui.rs` 53, `views.rs` 5; Doc-tests 0).
  - `music-api`: `cargo build --locked` → **OK** (30.2s), unverändert in #354.
- Baseline **grün** → Setup abgeschlossen, keine Blocker. Develop kann starten.

## Plan (fertig)
- `plan-issue-354.md` erstellt: 6 User Stories (Client-Range-Read, Backfill-Loop,
  CLI, Console-Button, Tests, Docs), Mapping-Ableitung `musicalKey → (key_pitch,key_mode)
  → camelot` (§Z), Datei-/Testliste und Implementierungs-Reihenfolge.
- Wichtig: `music-api/src/db.rs::normalize_key` normalisiert **ISRCs/Track-Keys**, NICHT den
  musikalischen Key → die Mapping-Ableitung erfolgt neu über `parse_musical_key` + inverse
  Camelot-Tabellen in `features.rs`.
- Vorgesehene Migration: `050_track_features_metadata_check.sql` (`metadata_checked_at TEXT`).

## Develop (in progress)
Stage 3 umgesetzt nach `plan-issue-354.md` (US-1..US-6, §Z). Kein `git commit` ausgeführt.

**Geänderte/neue Dateien (music-hub/ + docs):**
- `music-hub/src/music_api.rs` (+233): `MetadataRow` (camelCase, `musicalKey`/`key`-Felder,
  serde default), `metadata_range(cfg, isrcs) -> Option<(Vec<MetadataRow>, Vec<String>)>`,
  `parse_musical_key` (Camelot/Traktor/Notennamen → Camelot) + Tests.
- `music-hub/src/features.rs` (+216): `camelot_to_pitch_mode` (inverse, bijektiv zu `camelot`),
  `backfill_from_music_api` (bounded/TTL/Chunking/`continue`-Poisoning-Schutz/UPSERT mit COALESCE),
  Helfer `store_metadata_row`/`mark_metadata_checked`/`canonical_camelot` + Tests.
- `music-hub/migrations/050_track_features_metadata_check.sql` (neu): `metadata_checked_at TEXT`.
- `music-hub/src/main.rs` (+20): Subcommand `MetadataBackfill { --limit, default 2000 }` +
  `cmd_metadata_backfill`.
- `music-hub/src/music_ops.rs` (+30): `POST /music-api/backfill-metadata`.
- `music-hub/templates/music_api.html` (+8): Button „BPM/Key holen“.
- `music-hub/tests/features_metadata.rs` (neu, +4 Tests): Mock `/metadata`, Hit/Miss/TTL/No-Token.
- `music-hub/AGENT.md`, `music-hub/README.md`, `music-hub/CHANGELOG.md`, `docs/DECISIONS.md`
  (ADR-080) — Docs.
- `music-api/` unverändert.

**Verifikation (Host planet, cargo 1.97.1, `--locked`):**
- `cargo build --locked` (music-hub) → OK (17.2s), nur vorbestehende `web.rs:997` dead_code-Warnung.
- `cargo test --locked` (music-hub) → **grün**: lib 88 passed, Integration `features_metadata` 4/4,
  alle übrigen Suites 0 failed.

**Abweichungen/Notizen:**
- `MetadataRow`: getrennte Felder `musical_key` + `key` statt serde-`alias`, weil die API **beide**
  Felder sendet (`musicalKey` und `key`-Alias) — ein Alias erzeugt einen duplicate-field-Fehler.
  Helper `musical_key()` bevorzugt `musicalKey`, fällt auf `key` zurück.
- CLI ruft `backfill_from_music_api` einmal mit `--limit` auf (Funktion ist selbst chunked/bounded);
  kein Loop, da die Signatur nur `written` liefert.
- Ein Miss legt bewusst eine `found=0`-Row an (TTL-Bound); das schließt den ReccoBeats-`f.track_id
  IS NULL`-Pfad für dieses Track künftig aus (in ADR-080 dokumentiert; ggf. später revisiten).

- verify=PASS (Stage 4, 2026-10-15): cargo build/test --locked grün, 430 passed/0 failed (+11 neue Tests); Mapping §Z round-trip-konsistent, kein no-data-Poisoning, COALESCE, parametrisierte SQL, keine Token-Leaks.

- test=PASS (Stage 5, 2026-10-15): `cd music-hub && cargo test --locked` → 437 passed / 0 failed; `features_metadata.rs` 5 → 11 Tests (+6 direkte `metadata_range`-Fälle: Happy/Missing, leere Eingabe, kein Token, 500→None, malformed Body→None, leere Antwort→Some(empty); +1 No-Poisoning bei transientem 500). Kein Commit.

## PR (Stage 6, done)
- Commit: `4a0621f` — `feat(music-api): BPM/key backfill from metadata authority into hub_track_features (#354)`
  (13 Dateien, +1250/-2). Co-authored-by: momo-clanker[bot].
- Branch: `feat/music-api-metadata-backfill` → `origin` gepusht (upstream gesetzt).
- **PR #442**: https://github.com/momokli/momos-music-manager/pull/442 (base `main`).
- PR-Body enthält `Closes #354` (verifiziert via `pr view --json body`).
- Merge-Status: **OPEN**, kein Merge ausgeführt (auftragsgemäß).

## Review (Stage 7, 2026-10-15)
- review=APPROVE — Diff gegen `plan-issue-354.md` vollständig geprüft (13 Dateien, music-api unverändert).
  Lokale Reproduktion: `cd music-hub && cargo test --locked` → alle Suiten 0 failed, `features_metadata` 11/11.
  Mechanisches Gate: Required-Check „Build + Test (cargo)“ (Hub CI / CI (PR)) noch `in_progress` → `mergeStateStatus=BLOCKED`,
  kein Content-Blocker. Kein Merge/Kommentar ausgeführt.

## Rework (Screenshot-Beleg, 2026-10-15)
- Blocker "Frontend ohne Screenshot" adressiert: `/music-api`-Console gerendert (Test-DB + Session, MUSIC_API_TOKEN gesetzt),
  Button „BPM/Key holen" sichtbar. PNG: `docs/screenshots/music-api-metadata-backfill.png`, im PR-Body eingebettet.
