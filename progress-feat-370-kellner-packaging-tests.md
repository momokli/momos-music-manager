# Progress — Issue #370 `test(kellner): packaging + tests`

- Repo: momokli/momos-music-manager
- Branch: `feat/370-kellner-packaging-tests` (off `origin/main` @ 29722b1)
- Worktree: `/home/momo/repos/momos-music-manager-370`
- PR target: `main` (independent, no stack-depends)

## Dispatch check (done)

Predecessor check: milestone `kellner-0.1.0` — #361–#369 are **CLOSED**.
Previous `[BLOCKED]` comment on #370 is outdated. **#370 is the last open leaf → build it.**

## Spec (verbatim from plans/kellner/issues.md)

#370 = `test(kellner): packaging + tests` ← plan phases **P5-2, P6-1, P6-2**:
- **P5-2** tray/menu-bar widget — **already landed in #368** (`src/tray.rs`, MENU_LABELS test). No work needed.
- **P6-1** `test(kellner): end-to-end harness (mock hub + music-api)` → `kellner/tests/e2e.rs`.
  AC: full loop (upload → export → order → fetch → tag → backup) passes deterministically.
- **P6-2** `docs(kellner): README, AGENT, roadmap, CHANGELOG close-out` →
  `kellner/README.md`, `kellner/AGENT.md`, `docs/ROADMAP.md`, `plans/README.md`,
  `kellner/CHANGELOG.md`; move plan to `plans/done/`.
- **Packaging** (issue body: "Install + smoke tests"): install/packaging story + smoke test.

## Current gap (verified on origin/main)

- `kellner/tests/e2e.rs` — MISSING
- `kellner/CHANGELOG.md` — MISSING
- `kellner/deploy/**` / install script — MISSING
- `plans/kellner/*` still under `plans/` (not moved to `plans/done/`)

## Stage status

| Stage   | Status  | Result |
| ------- | ------- | ------ |
| plan    | done    | plan in "## Plan" (verbatim below) |
| dev     | done    | US1–US4 implemented; `cargo build --locked && cargo test --locked` green (87 lib + 5 int suites = 119 tests, 0 fail) |
| verify  | done    | PASS — all 5 checkpoints green (see below); `cargo build+test --locked` EXIT=0 (119 tests), `smoke.sh` EXIT=0, e2e 3× stable, install.sh idempotent |
| test    | pending | |
| pr      | pending | |
| review  | pending | |

### dev — done (2026-10-15)

- **US1+US2** `test(kellner): end-to-end harness (mock hub + music-api)` — **da95146**
  `kellner/tests/e2e.rs`: one in-process mock on 127.0.0.1:0 serves hub (`/api/hub/…`,
  device token) + music-api (orders/tracks/isrc/objects, music-api token) with
  per-role `401`; 30 s hang guard; `KELLNER_CONFIG`/`KELLNER_MUSIC_API_URL`/`KELLNER_MUSIC_API_TOKEN`.
  Full loop (upload → export → order → fetch → tag → backup) deterministically 
  (fixed ISRC/tags/BPM/key): one Traktor import (device Bearer + NML body); one
  order + one fetch; flat `coll/DEABC1234567.flac` with Comment `a, b  BPM 128 | A-Minor | Energy 7`;
  `backpack_membership` set; one `POST /objects/check` + one `PUT /objects/{sha256}`;
  mock store holds the **canonical** (Comment-cleared) hash == membership `content_hash`;
  second pass orders/fetches/uploads nothing. 4 consecutive runs stable.
- **US3** `chore(kellner): install script + offline smoke test` — **4eb34da**
  `kellner/scripts/smoke.sh` (offline: `--version`, `--help` lists all 7 subcommands,
  `service install --print --dry-run` prints a unit and writes nothing; `KELLNER_SMOKE_RUN=1`
  optional paired pass), `kellner/scripts/install.sh` (release build w/o tray →
  `${PREFIX:-$HOME/.local}/bin/kellner`, `--service`), `kellner/deploy/{README.md,
  systemd/kellner.service, launchd/de.zukkafabrik.kellner.plist}`, CI step
  `bash scripts/smoke.sh` after the kellner build. Smoke PASS locally.
- **US4** `docs(kellner): README/AGENT/ROADMAP/CHANGELOG close-out` — **3a31e6d**
  new `kellner/CHANGELOG.md` (`## [0.1.0]`); README status → 0.1.0 shipped + Install/Smoke/Tests;
  AGENT all commands implemented + e2e/smoke refs; `docs/ROADMAP.md` kellner `new`→`active`
  + milestone shipped; `plans/README.md` 57 done · 24 proposed, kellner-0.1.0 done;
  plan-lifecycle moves `plans/kellner/` → `plans/done/kellner/`, `plans/proposed/kellner-0.1.0.md`
  → `plans/done/kellner-0.1.0.md` (stale relative links fixed).

Validation: `cd kellner && cargo build --locked` (exit 0) and `cargo test --locked`
(exit 0; `grep "^test result"`: 87 + 3 + 4 + 9 + 1 + 7 + 4 + 4 = 119 passed, 0 failed).
No tray feature. No other crates touched; no migrations.

### verify — done (2026-10-15)

Independent re-run on `planet`, diff `origin/main..HEAD`:

1. **Scope — PASS.** Touched files: `kellner/**`, `plans/**` (incl. moves
   `plans/{=>done}/kellner/issues.md`, `plans/{proposed=>done}/kellner-0.1.0.md`),
   `docs/ROADMAP.md`, `.github/workflows/ci-pr.yml`, progress file only. No other
   crates, no migrations; `kellner/src/tray.rs` untouched (empty diff).
2. **e2e harness — PASS.** `kellner/tests/e2e.rs`: one `TcpListener` on
   `127.0.0.1:0` serves `/api/hub/…` (device token) + music-api (music-api token)
   with per-role `401`; no external network (only loopback); fixed
   ISRC/tags/BPM/key; full loop + idempotency (2nd pass orders/fetches/uploads
   nothing); canonical Comment-cleared hash is the dedup key. 3 consecutive runs
   stable (0.10–0.11s).
3. **Packaging — PASS.** `smoke.sh` offline (version/help/`service install
   --print --dry-run` writes nothing) → `smoke: PASS` EXIT=0; `install.sh` run 2×
   → EXIT=0 both, binary at prefix, no network; deploy templates consistent with
   `src/service.rs` (`systemd_user_unit`/`launchd_plist`); CI smoke step added
   after the kellner build (no required-check broken).
4. **Security — PASS.** No secrets/PII (test tokens only); both scripts
   `set -euo pipefail`, quoted expansions, no path escapes.
5. **Docs — PASS.** README/AGENT/CHANGELOG/ROADMAP/plans-index updated; plan +
   issues moved to `plans/done/`; relative links fixed; counter 56→57 done ·
   25→24 proposed consistent.

**Overall: PASS.**

## Guardrails

- Never commit to `main`. Commit style: `test(kellner): …` / `docs(kellner): …` (Conventional Commits, English).
- `gh`/`git` only via `clanker-gh` / `clanker-git` (bot `momo-clanker[bot]`).
- Heavy Rust builds/tests on `planet` (exec host=node), not the gateway host.
- PR body MUST carry `Closes #370`.
- Do not restructure the tray (P5-2 done). Do not touch other crates.

## Plan

**Feature**: kellner packaging + tests (Issue #370) — deliverables P6-1, Packaging/Install+Smoke, P6-2.
**Scope guardrail**: only `kellner/**`, `plans/**`, `docs/ROADMAP.md` and `.github/workflows/ci-pr.yml`. No other crates, no migrations. P5-2 (tray) is done (#368) and is **not** re-planned.

### Ist-Stand (recherchiert im Worktree, Branch `feat/370-kellner-packaging-tests`)

- `kellner/tests/e2e.rs` → **fehlt**. Vorhandene Muster: `pair_flow.rs`, `traktor_upload.rs`, `backpack_sync.rs`, `backup.rs`, `downloads.rs` — jede Datei ist **self-contained**: eigener `TcpListener`-Mock auf `127.0.0.1:0`, eigener `run_kellner`-Helfer (30 s Hang-Guard, `env!(CARGO_BIN_EXE_kellner)`, stdout/stderr-Drain-Threads), `Captured`/`parse_request`/`find_sub`/`content_length`-Helfer. `backpack_sync.rs` vereint bereits Mock-hub **und** Mock-music-api auf **einem** Port (Pfad-Prefix `/api/hub/…` = device-token, sonst music-api-token). `test.flac` + `test.mp3` Fixtures existieren. `state.json` wird über `KELLNER_CONFIG` (Sibling) in das Tempdir umgeleitet.
- CLI-Fluss ist vollständig implementiert (`src/main.rs`, `src/sync/*`): `traktor-upload --collection-nml <nml> --once`; `backpack --root <dir> --poll-secs 1`; `backup --root <dir> --once`. Tag-Render-Format (Beleg aus `backpack_sync.rs`): `"a, b  BPM 128 | A-Minor | Energy 7"`. Backup canonicalisiert (Comment geleert) → SHA-256-Objektkey.
- `kellner/deploy/**` / `scripts/**` → **fehlt**. P5-1 hatte `kellner/deploy/**` als Write-Set vorgesehen, aber die Unit-Templates landeten als reine Builder in `src/service.rs` (snapshot-getestet). `.github/workflows/ci-pr.yml` hat einen kellner-Job (`cargo build --locked && cargo test --locked`, workspace-cache `kellner -> target`), **keinen** Smoke-Step.
- `kellner/CHANGELOG.md` → **fehlt**. `plans/kellner/issues.md` liegt noch unter `plans/` (nicht `plans/done/`); `plans/README.md` zählt `56 done · 0 in progress · 25 proposed` und führt `kellner-0.1.0` unter **Proposed**. `docs/ROADMAP.md` führt kellner als Status `**new**`.

### Betroffene Dateien

- `kellner/tests/e2e.rs` — **neu** (P6-1, kombiniertes Mock-hub+music-api, voller Loop)
- `kellner/deploy/README.md` + `kellner/deploy/systemd/kellner.service` + `kellner/deploy/launchd/de.zukkafabrik.kellner.plist` — **neu** (Packaging-Assets; Template-Doku; Build-Setting bleibt in `src/service.rs`)
- `kellner/scripts/install.sh` + `kellner/scripts/smoke.sh` — **neu** (Install + Smoke, offline/CI-tauglich)
- `.github/workflows/ci-pr.yml` — Smoke-Step im kellner-Job
- `kellner/README.md`, `kellner/AGENT.md`, `kellner/CHANGELOG.md` (neu), `docs/ROADMAP.md`, `plans/README.md`
- `plans/kellner/issues.md` → `plans/done/kellner/issues.md` (Verschiebung)

### User Stories (in Reihenfolge)

1. **E2E-Harness: kombinierter In-Process-Mock (P6-1 Setup)**
   - Akzeptanzkriterien:
     - `kellner/tests/e2e.rs` existiert und kompiliert unter `cargo test --locked` (kein GTK/tray-Feature).
     - Ein `start_mock()` startet **einen** `tokio::net::TcpListener` auf `127.0.0.1:0`, der beide Rollen bedient: `/api/hub/backpacks`, `/api/hub/backpacks/{id}/export`, `POST /api/hub/traktor/import` (Bearer *device_token*) **und** `POST /orders`, `GET /tracks?isrcs=…`, `GET /isrc/{isrc}/flac`, `POST /objects/check`, `PUT /objects/{sha256}` (Bearer *music_api_token*). Falsches Token → `401`.
     - `run_kellner`-Helfer mit 30 s Hang-Guard, `KELLNER_CONFIG`/`KELLNER_MUSIC_API_URL`/`KELLNER_MUSIC_API_TOKEN` gesetzt, `env_remove` der anderen `KELLNER_*`; Muster identisch zu `backpack_sync.rs`.
     - Kein Socket verlässt `127.0.0.1`; keine realen Server.
   - Betroffene Dateien: `kellner/tests/e2e.rs`
   - Geschätzte LOC: ~300 (Helfer + Mock-Handler)

2. **E2E-Full-Loop + Determinismus (P6-1 Kern-AC)**
   - Akzeptanzkriterien:
     - **Ein** Test fährt den vollen Loop in Prozess-Schritten: NML-Upload → backpack-Export → Order fehlender ISRCs → Fetch/flat file → Comment-Tag schreiben → Backup (object store).
     - Assertions je Etappe: (a) genau **ein** `POST /api/hub/traktor/import` mit Bearer device-token und NML-Body; (b) Export liefert Track → genau **ein** `POST /orders` + **ein** `GET /isrc/{isrc}/flac`; Datei liegt flat als `coll/<isrc>.flac`; Comment-Tag == erwarteter Render (`"a, b  BPM 128 | A-Minor | Energy 7"`) via `lofty::read_from_path`; `state.json` `backpack_membership` gesetzt; (c) `backup` erzeugt `POST /objects/check` + **ein** `PUT /objects/{sha256}` und der Mock-Store enthält den **canonical**-Hash (Comment geleert) — der Tag-Write darf den Dedup-Key nicht verfälschen.
     - Determinismus: feste ISRCs/Tags/BPM/Key, `#[tokio::test(flavor="multi_thread", worker_threads=2)]`, 30 s Guard; wiederholte Läufe stabil. Ein zweiter Lauf (Idempotenz) ordert/fetcht/uploaded nichts erneut.
   - Betroffene Dateien: `kellner/tests/e2e.rs`
   - Geschätzte LOC: ~140

3. **Packaging/Install + Smoke-Test (Issue „Install + smoke tests“)**
   - Akzeptanzkriterien:
     - `kellner/scripts/smoke.sh` baut/läuft das Binary und prüft **offline**: `kellner --version` (exit 0, enthält Version), `kellner --help` (listet `pair|backpack|backup|traktor-upload|downloads|run|service`), `kellner service install --print --dry-run` (exit 0, gibt Unit-Text aus, schreibt nichts). Kein Netz, kein Pairing, kein GTK/tray → CI-tauglich.
     - `run --once` wird **nicht** im Smoke genutzt (benötigt Pairing); stattdessen dokumentiert das Skript den gepaarten Trockenlauf als optionalen `KELLNER_SMOKE_RUN=1`-Pfad.
     - `kellner/scripts/install.sh` baut `cargo build --release` (ohne tray) und installiert das Binary nach `${PREFIX:-$HOME/.local}/bin/kellner`; optionaler `--service`-Schalter ruft `kellner service install`. Idempotent, kein Netz.
     - `kellner/deploy/` enthält die Referenz-Templates (`systemd/kellner.service`, `launchd/de.zukkafabrik.kellner.plist`) + `deploy/README.md`; Templates stimmen inhaltlich mit `src/service.rs` überein (Kommentar verweist dorthin).
     - `.github/workflows/ci-pr.yml` kellner-Job erhält einen Step `bash scripts/smoke.sh` **nach** dem Build.
   - Betroffene Dateien: `kellner/scripts/install.sh`, `kellner/scripts/smoke.sh`, `kellner/deploy/README.md`, `kellner/deploy/systemd/kellner.service`, `kellner/deploy/launchd/de.zukkafabrik.kellner.plist`, `.github/workflows/ci-pr.yml`
   - Geschätzte LOC: ~120 (Scripts+deploy) + ~5 (CI)

4. **Docs close-out + Plan-Lifecycle (P6-2)**
   - Akzeptanzkriterien:
     - `kellner/CHANGELOG.md` **neu** angelegt (Keep-a-Changelog-Kurzform, `## [0.1.0]` mit Pairing/Traktor/Backpack/Backup/Downloads/Service/Tray/Tests/Docs).
     - `kellner/README.md`: Status „sync loop + service/tray implemented (#368)“ → „0.1.0 shipped (all P0–P6)“; Smoke/Install-Abschnitt ergänzt; Stub-Sprache entfernt.
     - `kellner/AGENT.md`: „nur `backpack` ist noch Stub“ → alle Kommandos implementiert; Verweis auf `tests/e2e.rs` + `scripts/smoke.sh`; „Last updated“ aktualisiert.
     - `docs/ROADMAP.md`: kellner-Zeile Status `**new**` → `active`; Milestone-Notiz `kellner-0.1.0` als shipped.
     - `plans/README.md`: `plans/kellner/issues.md` → `plans/done/kellner/issues.md` (Dir-Move beider Links/Index); Zähler `56 done · 25 proposed` → `57 done · 24 proposed`; `kellner-0.1.0` von Proposed nach Done.
     - Verschiebung `plans/kellner/` → `plans/done/kellner/` (nur `issues.md`); `plans/proposed/kellner-0.1.0.md` → `plans/done/kellner-0.1.0.md` (Lifecycle-Konsistenz zum Task „plan to `plans/done/`“).
     - Docs-only: `cd kellner && cargo build` bleibt grün.
   - Betroffene Dateien: `kellner/CHANGELOG.md`, `kellner/README.md`, `kellner/AGENT.md`, `docs/ROADMAP.md`, `plans/README.md`, `plans/kellner/issues.md` (Move), `plans/proposed/kellner-0.1.0.md` (Move)
   - Geschätzte LOC: ~150 (Docs-Änderungen, keine Code-LOC)

### Reihenfolge / Abhängigkeiten

1. US1 (Harness-Scaffold) → 2. US2 (Full-Loop nutzt US1-Helfer) → 3. US3 (Packaging/Smoke, unabhängig vom Harness; baut auf vorhandenem CLI auf) → 4. US4 (Docs zuletzt, beschreibt den Endstand).

- US1‖US3 sind file-disjoint und parallelisierbar; US2 hängt an US1; US4 hängt an allen (beschreibt das Ergebnis).
- Commit-Schnitt: `test(kellner): end-to-end harness …` (US1+US2), `chore(kellner): install script + offline smoke test` (US3), `docs(kellner): README/AGENT/ROADMAP/CHANGELOG close-out` (US4) — je Conventional Commit, English; PR-Body `Closes #370`.
- Validierung überall: `cd kellner && cargo test --locked` (Planet/node) + `bash kellner/scripts/smoke.sh`.
