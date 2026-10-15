# Progress — #371 feat(mmm): feature freeze

Branch: `feat/371-mmm-feature-freeze` (worktree `~/repos/momos-music-manager-371`, from `origin/main` @ `42b72fc`)
Repo: `momokli/momos-music-manager` · PR target: `main` · Milestone: `mmm-sunset-1.19.0`
Issue: "Feature freeze MMM" — body: *No new features in mmm/.*

## Dispatch validity (checked 2026-10-15)
- Milestone `mmm-sunset-1.19.0` (id 38): leaves #371 (freeze), #372 (data migration), #373 (deprecation docs). Order #371 → #372 → #373.
- No open/merged PR references #371; no branch exists. → **build it** (not already done).
- MMM already declared *sunset* in `README.md` / `AGENT.md` (scope-change 2026-10-12), but the freeze is
  **not enforced anywhere** — no guard, no marker. That is what this leaf adds.

## Spec (candidate — planner to confirm/refine)
Goal: make the "no new features in `legacy/mmm/`" freeze **explicit and enforced**.
1. `legacy/mmm/FROZEN.md` — freeze marker: MMM is frozen/sunset; keep it running through the
   migration only (bug fixes), no new features; point to Hub + Kellner (`docs/`).
2. `.github/workflows/mmm-freeze.yml` — job `mmm-freeze-guard` on `pull_request`, **always runs**
   (repo convention: no `paths:` filter, a skipped check is not green). Fails when the PR diff
   touches the MMM **feature surface** (`legacy/mmm/src/**`, `legacy/mmm/frontend/**`,
   `legacy/mmm/migrations/**`, `legacy/mmm/Cargo.toml`) unless the PR carries the escape label
   `mmm:unfreeze` (migration-critical fix). Changelog / docs changes under `legacy/mmm/` are allowed.
   Diff via `git diff --name-only origin/${{ github.base_ref }}...HEAD` (checkout fetch-depth 0).
3. Link the marker from `README.md` (MMM row / Legacy section) — keep edits minimal.

## Constraints
- Heavy Rust builds/tests → `planet` (exec host=node). This change is workflow-YAML + docs only → no Rust build expected.
- `gh`/`git` only via `clanker-gh` / `clanker-git`.
- PR body MUST contain `Closes #371`.
- No review, no merge — that is `mmm-pr-gate`.

## Pipeline stages
- [x] triage/recon
- [x] planner — Ansatz: FROZEN.md + CI-Guard + README/AGENT-Links
- [x] setup (branch exists; build baseline)

## Setup Baseline (2026-10-15)
- Branch `feat/371-mmm-feature-freeze`, HEAD == origin/main @ `42b72fc` (`rev-list --left-right --count origin/main...HEAD` = `0	0`). Kein Reset noetig.
- Change = YAML+Docs only -> kein Rust/cargo-Build. `python3 -c "import yaml"` OK (PyYAML 6.0.1).
- Label `mmm:unfreeze` (#B60205) angelegt + verifiziert.
- [x] developer — Commit 9e84c7b (4 Dateien), gepusht
- [x] verifier — PASS
- [x] tester — PASS (Guard-E2E S1–S6/SX, YAML ok)
- [x] developer (PR) — PR #463, Body mit `Closes #371`
- [x] reviewer — PASS

## Ergebnis
PR #463 (offen, gegen main, MERGEABLE/BLOCKED bis Checks grün): https://github.com/momokli/momos-music-manager/pull/463
Checks: Conventional-Commit-Titel ✓, Issue-Referenz ✓, MMM freeze guard ✓; Build+Test/clippy noch pending.
Kein Merge — Aufgabe des mmm-pr-gate.

## Plan

Gewählter Ansatz: **Freeze-Marker + Repo-Doku-Link + durchsetzender CI-Guard** (3 Artefakte).
Das ist die minimale, *durchsetzbare* Variante: Der Marker/die Doku macht den Freeze für Menschen
sichtbar, der Workflow erzwingt ihn maschinell. Kein Scope-Creep (keine Branch-Protection-Änderung,
kein Rust-Build, keine „policy engine").

### Betroffene Dateien
- `legacy/mmm/FROZEN.md` — **neu**: Freeze-Marker.
- `.github/workflows/mmm-freeze.yml` — **neu**: Guard-Workflow.
- `README.md` — **edit** (2 kleine Stellen: MMM-Tabellenzeile + Legacy-Abschnitt).
- optional: `AGENT.md` — **edit** (1 Satz zum `mmm:unfreeze`-Label). Empfohlen, weil der Gate-Runner
  die Labels aus AGENT.md liest; ohne Notiz ist das Escape-Label nicht entdeckbar.

### Verfeinerungen gegenüber der Kandidaten-Spec (gegen Repo geprüft)

**1. Feature-Oberfläche: Blocklist statt Allowlist.** Eine Allowlist („nur Docs erlaubt") überrascht
bei jedem legitimen Nicht-Feature-Diff (z. B. `deploy/`, `test.sh`, `vendor/`-Updates). Die Spec nennt
4 Pfade — bestätigt und um `Cargo.lock` + `build.rs` bereinigt:

Blocklist (jede Änderung ⇒ Fail ohne Escape-Label):
```
legacy/mmm/src/**
legacy/mmm/frontend/**
legacy/mmm/migrations/**
legacy/mmm/Cargo.toml
legacy/mmm/Cargo.lock
legacy/mmm/build.rs
```
Ausdrücklich **erlaubt** (nicht auf Blocklist): `legacy/mmm/CHANGELOG.md`, `legacy/mmm/*.md`,
`legacy/mmm/DIGGING.md`, `legacy/mmm/site/**`, `legacy/mmm/deploy/**`, `legacy/mmm/scripts/**`,
`legacy/mmm/tests/**`, `legacy/mmm/examples/**`, `legacy/mmm/resources/**`.
Begründung: `Cargo.lock` allein ist Churn, aber ein Feature, das `Cargo.toml` ändert, zieht ihn mit —
beide in der Blocklist hält die Regel einfach und schließt „nur Lockfile"-Schlupflöcher aus.
`build.rs` ist Code, kein Doc.

**2. Release-PR-Ausnahme (neu, sonst Fehlalarm).** Der Release-Prozess (AGENT.md §Release Process
Schritt 8) bumpt `legacy/mmm/Cargo.toml` — liegt auf der Blocklist und würde den Guard **rot** machen.
Deshalb dieselbe Release-Erkennung wie `pr-quality.yml`: exempt wenn PR-Titel `/^chore\(release\)/i`
**oder** ein Label `release:*`. Logik 1:1 aus `pr-quality.yml` übernehmen (isRelease).

**3. Trigger inkl. Label-Events.** `on: pull_request: types: [opened, synchronize, reopened, labeled, unlabeled]`.
Nötig, damit das nachträgliche Setzen/Entfernen von `mmm:unfreeze` neu auswertet — sonst bliebe ein
guarded PR rot, bis jemand einen Commit pusht.

**4. Konvention: kein `paths:`-Filter (Required-Job-Konvention).** `ci-pr.yml` sagt explizit: kein
`paths:` auf Required-Jobs, weil ein **geskippter Check nicht „grün" ist**. Der Guard muss also auf
*fremden* PRs mitlaufen und dort grün werden (leerer MMM-Diff ⇒ pass). `hub-ci.yml` benutzt einen
`paths:`-Filter, ist aber bewusst **nicht** required.

**5. Exakte Diff-Ermittlung:**
```yaml
- uses: actions/checkout@v4
  with:
    fetch-depth: 0          # volle History nötig für merge-base + origin/<base_ref>
- name: MMM freeze guard
  env:
    BASE_REF: ${{ github.base_ref }}          # z. B. main (Zielbranch des PR)
    PR_LABELS: ${{ join(github.event.pull_request.labels.*.name, ',') }}
    PR_TITLE:  ${{ github.event.pull_request.title }}
  run: bash .github/scripts/mmm-freeze-guard.sh   # oder inline
```
Diff: `git diff --name-only "origin/${BASE_REF}...HEAD"`.
Drei Punkte (`...`) = Diff gegen den **Merge-Base** — korrekt auch für gestackte PRs (deren Base ein
Vorgänger-Branch ist, vgl. ci-pr.yml-Kommentar). `fetch-depth: 0` liefert die Refs, damit
`origin/${BASE_REF}` existiert; bei `pull_request` ist `HEAD` der Merge-Commit, der Diff ist der PR-Diff.

**6. Fallback ohne DSL-Abhängigkeit.** Um keine Action-Abhängigkeit (z. B. dorny/paths-filter)
einzuführen: rein in Bash (`grep -E` gegen die Blocklist-Regex). Der Repo-Stil nutzt für
Label/Script-Logik `actions/github-script@v7` (pr-quality.yml) — Bash ist hier gleichwertig und
schlanker; Wahl in Story 3 als offene Variante vermerkt.

**7. Required-Check? — Nein, nicht durch uns.** Branch-Protection/Required-Checks setzt die
`mmm-pr-gate` (AGENT.md: `Build + Test (cargo)`, `Conventional-Commit-Titel`, `Issue-Referenz im PR`).
Der Gate merged ohnehin nur bei **allen Checks grün** („Zero check runs do not count as green"), d. h.
ein immer laufender, grüner Guard wirkt faktisch mit. Empfehlung an den Gate: Job-Name
`MMM freeze guard` in die Required-Liste aufnehmen — **das ist eine Gate-Aufgabe, kein Deliverable
dieses Issues.**

### User Stories (in Reihenfolge)

1. **Freeze-Marker `legacy/mmm/FROZEN.md` anlegen**
   - Akzeptanzkriterien: Datei existiert; nennt (a) Status „frozen / sunset", (b) erlaubt nur
     Bugfixes, die MMM bis Abschluss der Migration lauffähig halten, (c) verbietet neue Features in
     `src/ frontend/ migrations/`, (d) verweist auf Migrationsziele `music-hub` + `music-api` und
     `docs/`, (e) dokumentiert das Escape-Label `mmm:unfreeze` für migrationskritische Fixes.
   - Betroffene Dateien: `legacy/mmm/FROZEN.md` (neu)
   - LOC: ~25

2. **README auf den Marker verlinken**
   - Akzeptanzkriterien: MMM-Tabellenzeile („Docs"-Spalte) und/oder Legacy-Abschnitt verlinken
     `legacy/mmm/FROZEN.md`; Satz „Treat MMM as read-only unless a fix is needed…" mit dem Marker
     verknüpft; keine weitere README-Umschreibung.
   - Betroffene Dateien: `README.md` (2 Zellen)
   - LOC: ~2 (±1)

3. **CI-Guard `mmm-freeze.yml`** (Kern)
   - Akzeptanzkriterien:
     - Workflow `name: MMM freeze`, Job `mmm-freeze-guard` (`name: MMM freeze guard`), `on: pull_request`
       mit `types: [opened, synchronize, reopened, labeled, unlabeled]`, **kein `paths:`-Filter**,
       `permissions: contents: read`.
     - `checkout@v4` mit `fetch-depth: 0`.
     - Exempt, wenn Titel `/^chore\(release\)/i` oder Label `release:*` (Logik aus `pr-quality.yml`).
     - Exempt, wenn `mmm:unfreeze` in den PR-Labels.
     - Sonst: `git diff --name-only "origin/${BASE_REF}...HEAD"`, Fail (Exit 1) mit Listing der
       getroffenen Blocklist-Pfade, wenn ein Pfad der Blocklist (Story-1-Konvention, Punkt 1) matched.
     - Leerer/ fremder Diff ⇒ grün (Check läuft, kein Skip).
     - Fehlermeldung nennt das Escape-Label `mmm:unfreeze` und den Marker `legacy/mmm/FROZEN.md`.
   - Betroffene Dateien: `.github/workflows/mmm-freeze.yml` (neu); optional `.github/scripts/mmm-freeze-guard.sh`
   - LOC: ~45 (YAML) bzw. +~30 (bash), wenn ausgelagert

4. **(empfohlen) AGENT.md-Hinweis + Label-Provisionierung**
   - Akzeptanzkriterien: AGENT.md nennt im Legacy-/PR-Gate-Abschnitt das Label `mmm:unfreeze` als
     einziges Escape für den Freeze-Guard; Repo-Label `mmm:unfreeze` existiert
     (`gh label create mmm:unfreeze --repo momokli/momos-music-manager` — Setup-Schritt, kein Code).
   - Betroffene Dateien: `AGENT.md` (1 Satz)
   - LOC: ~2 (Label-Anlage = 1 `gh`-Befehl)

### Implementierungs-Reihenfolge
1 → 2 → 3 (→ 4). Story 3 ist der eigentliche Enforcement; 1/2 liefern die Quelle, auf die 3 verweist.
Story 4 nur, wenn der Gate das Label tatsächlich nutzt (Discoverability), sonst optional.

### Blocker/Abhängigkeiten
- Kein Rust-Build nötig (YAML + Docs) → verifizierbar ohne `planet`.
- `Closes #371` muss in den PR-Body (pr-quality.yml, Hard-Fail). Titel Conventional Commit, z. B.
  `ci(mmm): feature-freeze guard for legacy/mmm (#371)`.
- Required-Check-Eintrag: **nicht** hier — Aufgabe der `mmm-pr-gate`.

### Offene Entscheidungen (für Reviewer/Gate)
- `Cargo.lock`/`build.rs` in der Blocklist belassen (empfohlen) oder auf die 4 Spec-Pfade reduzieren?
- Release-Ausnahme per Titel **und** Label `release:*` (empfohlen, wie pr-quality.yml) — reicht Titel?
- Guard-Logik inline im Workflow vs. `.github/scripts/*.sh` (Repo hat noch keine Script-Konvention).
- Soll der Gate den Check in die Required-Liste heben (empfohlen) oder reicht „alle Checks grün"?

## Verify

**Verdikt: PASS** (2026-10-15, Verifier-Stage, HEAD 9e84c7b)

Scope (4 Dateien, exakt geplant): `.github/workflows/mmm-freeze.yml` (neu), `legacy/mmm/FROZEN.md` (neu),
`README.md`, `AGENT.md`. Kein progress-/plan-File committed (`progress-issue-371.md` ist untracked `??`).
Keine Fremdänderungen. → PASS.

Guard-Korrektheit (`mmm-freeze.yml`, 71 Zeilen): kein `paths:`-Filter ✓; `types: [opened, synchronize,
reopened, labeled, unlabeled]` ✓; `fetch-depth: 0` ✓; `permissions: contents: read` ✓;
Release-Exempt (Titel `^chore\(release\)` / Label `(^|,)release`) **vor** Blocklist ✓; `mmm:unfreeze`-Exempt
**vor** Blocklist ✓; Diff `origin/${BASE_REF}...HEAD` (Merge-Base, 3 Punkte) ✓; Blocklist-Regex deckt
`legacy/mmm/src/`, `frontend/`, `migrations/`, `Cargo.toml$`, `Cargo.lock$`, `build.rs$` ✓; leerer/foreign
Diff ⇒ grün (`grep ... || true`, `hits` leer) ✓; `set -euo pipefail` ✓.

Live-Selbsttest Guard-Bash lokal reproduziert:
- (a) realer Diff `origin/main...HEAD` (nur YAML/Docs) ⇒ **kein Hit** → Guard grün, wie erwartet.
- (b) Pfad-Regex: HIT für `legacy/mmm/src/main.rs`, `frontend/app.ts`, `migrations/001.sql`, `Cargo.toml`,
  `Cargo.lock`, `build.rs`; miss für `legacy/mmm/CHANGELOG.md`, `legacy/mmm/FROZEN.md`,
  `legacy/mmm/site/index.html`, `legacy/mmm/tests/x.rs`, `docs/x.md`, `README.md`, `srcfoo.rs`,
  `xlegacy/mmm/src/a.rs` (Anker korrekt). Alles korrekt.
- Label-Regex `mmm:unfreeze`: HIT für `mmm:unfreeze`, `foo,mmm:unfreeze`, `mmm:unfreeze,other`;
  miss für `release:1.0`, `foo`, `""`, `xmmm:unfreeze`, `mmm:unfreezeX` (Teilstring ausgeschlossen) ✓.
- Release-Label: HIT für `release:1.0`, `foo,release:2`; miss für `myrelease` ✓ (konsistent mit
  pr-quality.yml `startsWith('release')`).

YAML-Syntax: `python3 -c "import yaml; yaml.safe_load(...)"` → OK.
Security: keine Secrets/Token/PII im Diff (`grep` clean); `contents: read` ✓. Keine debug-prints
(echo = bewusste `::notice::/::error::`-Meldungen).
Doku-Konsistenz: FROZEN.md nennt Blocklist `src/ frontend/ migrations/ Cargo.toml Cargo.lock build.rs` +
Label `mmm:unfreeze`; README verlinkt `legacy/mmm/FROZEN.md` in MMM-Zeile + Legacy-Abschnitt (Label genannt);
AGENT.md nennt Job `MMM freeze guard` + Label `mmm:unfreeze` als **einzigen** Escape. → einheitlich.

### Findings (nicht blockierend)
- `.github/workflows/mmm-freeze.yml:39` — Release-Titel-Exempt nutzt Bash `[[ =~ ]]` (case-**sensitive**),
  Plan/`pr-quality.yml` nutzen `/^chore\(release\)/i`. Titel `Chore(release): …` würde nicht exempt.
  Impact gering (Release-Titel sind lowercase); bei Gelegenheit `shopt -s nocasematch` oder `grep -qiE` erwägen.
- `.github/workflows/mmm-freeze.yml:39` — `(^|,)release` matcht jedes Label mit Präfix `release`
  (z. B. `released`); identisch zu pr-quality.yml `startsWith('release')` → bewusst konsistent, kein Fix.

### Empfehlung
Weiter zu **Test**. Keine Blocker. Required-Check-Eintrag `MMM freeze guard` bleibt Gate-Aufgabe
(`mmm-pr-gate`), wie im Plan vermerkt.

## Test

**Verdikt: PASS** (2026-10-15, HEAD 9e84c7b). Guard-`run`-Block per `yaml.safe_load` extrahiert, isoliert in Wegwerf-Repo `/tmp/mmm-freeze-e2e` ausgeführt (echtes `git diff --name-only origin/main...HEAD`; keine Branch-Änderung, nichts committet).

| # | Szenario | Exit | Soll |
|---|---|---|---|
| S1 | nur README.md | 0 | 0 ✓ |
| S2 | legacy/mmm/src/main.rs | 1 | 1 ✓ (nennt `mmm:unfreeze`+`FROZEN.md`) |
| S3 | S2 + Label `mmm:unfreeze` | 0 | 0 ✓ |
| S4 | Titel `chore(release): v1.19.0` + Cargo.toml | 0 | 0 ✓ |
| S5 | Label `release:1.19.0` + Cargo.toml | 0 | 0 ✓ |
| S6 | leerer Diff | 0 | 0 ✓ |
| SX | frontend/app.ts + Label `bug,mmm:unfreeze,other` | 0 | 0 ✓ |

YAML/Lint: `actionlint` fehlt (`command -v` leer; `npx --yes actionlint` scheitert); Fallback `python3 yaml.safe_load` → OK. Rust: Branch-Diff nur YAML/Docs/`FROZEN.md` ⇒ `cargo test` nicht anwendbar. Keine Blocker.

## Review

**Verdikt: PASS** (2026-10-15, Reviewer-Stage, HEAD 9e84c7b, PR #463)

Diff `origin/main...HEAD` = 4 Dateien, exakt wie geplant: `mmm-freeze.yml` (neu, 71 Z.), `legacy/mmm/FROZEN.md` (neu, 31 Z.), `README.md`, `AGENT.md`. Kein progress-/plan-File committed (`progress-issue-371.md` untracked `??`).

1. **Issue-Erfüllung (`No new features in mmm/`)**: ✓ Freeze ist *maschinell durchsetzbar* — Required-Job läuft auf jedem PR (kein `paths:`-Filter, „geskippter Check ≠ grün"), failt bei Blocklist-Treffern mit `::error::` + Escape-Hinweis. Ausnahmen eng: `mmm:unfreeze` (deliberate, label-gated) + Release-PR (`chore(release)`/`release:*`, nötig gegen Fehlalarm beim Cargo.toml-Bump). Beide vor der Blocklist ausgewertet.
2. **Lesbarkeit/Konsistenz**: ✓ YAML idiomatisch, `set -euo pipefail`, `base_ref`-Parametrisierung, Least-Privilege `contents: read`. FROZEN.md ↔ Workflow ↔ README ↔ AGENT.md konsistent (Blocklist, Label, Migrationsziele, Links: `docs/ROADMAP.md`, `docs/ARCHITECTURE.md`, `docs/legacy/mmm-README.md` alle vorhanden). Diff-Spec `origin/${BASE_REF}...HEAD` (Merge-Base) korrekt.
3. **PR-Konvention**: ✓ Titel `ci(mmm): feature-freeze guard for legacy/mmm` conventional; Body enthält `Closes #371`; keine progress-/plan-Dateien im Commit.
4. Unabhängig reproduziert: Blocklist-Regex korrekt verankert (BLOCK für `src/ frontend/ migrations/ Cargo.toml Cargo.lock build.rs`; allow für `CHANGELOG.md`, `FROZEN.md`, `site/`, `tests/`, `srcfoo.rs`, `xlegacy/...`); Release-/Unfreeze-Label-Regex wie dokumentiert.

### Findings (nicht blockierend, nicht gefixt)
- `mmm-freeze.yml:39` — Release-Titel-Exempt `[[ "$PR_TITLE" =~ ^chore\(release\) ]]` ist case-**sensitive**; `pr-quality.yml` nutzt `/^chore\(release\)/i`. Titel `Chore(release): …` würde nicht exempt → Guard rot beim Release. Impact gering (Release-Titel lowercase); bei Gelegenheit `grep -qiE`/`shopt -s nocasematch`. (Von Verify übernommen, bestätigt.)
- `mmm-freeze.yml:39` — `(^|,)release` matcht jedes Label mit Präfix `release` (z. B. `released`); identisch zu `pr-quality.yml` `startsWith('release')` → bewusst konsistent, kein Fix.
- `AGENT.md:71` — „the label `mmm:unfreeze` is the **only** escape" überzeichnet leicht: die Release-PR-Ausnahme ist ein zweiter (nutzungsbedingter) Bypass. Reine Doku-Nuance, kein Enforcement-Problem.

### Empfehlung
Merge-fähig (Squash). Required-Check-Eintrag `MMM freeze guard` bleibt Aufgabe des `mmm-pr-gate` (nicht Teil dieses Issues). Keine Blocker.
