# Progress — Issue #351: feat(hub): import-history UI polish

- Repo: momokli/momos-music-manager
- Branch: `feat/issue-351-import-history-polish` (from `origin/main` @ 9cf473a)
- Worktree: `~/repos/momos-music-manager-351`
- Milestone: hub-ops-0.28.0 (#34) — leaf #351
- PR target: `main` — body MUST contain `Closes #351`
- Host: planet

## Issue
Title: "Import-history UI polish"
Body: "Filters, diffs, change events."
Existing surface: `music-hub/src/history.rs` + `music-hub/templates/history.html`
(plan H1–H3, #224–#226). Runs table + change timeline, filter by q/source/user,
independent sort per table, LIMIT 500.

## Dispatch validity
Checked: no open/merged PR references #351; `main` has no import-history polish
commit. Predecessors #353/#350 merged. → Not ALREADY-DONE. Build.

## Scope (polish, grounded in the issue body)
1. Filters — add change-kind filter (added|removed|changed) and a per-run filter
   (click a run id → events scoped to that run); reset/link hygiene.
2. Diffs — render before→after as a readable diff; pretty-print run `stats` JSON.
3. Change events — per-run change counts (added/removed/changed) as badges in the
   runs table; tighter empty states.

## Stage log
- [x] planner — Plan geschrieben (6 Stories, AC + Testfaelle)
- [x] setup — Baseline build OK, history-Tests 6/6 gruen @ 9cf473a
- [x] developer — implementiert (6 Stories) + Tests, commit 141f0fe
- [x] verifier — [VERDICT: APPROVE] , keine Fixes
- [x] tester — 418 Tests grün, 3 Screenshots, commit 7ad684b
- [x] developer (PR) — PR #440 offen, Closes #351, Screenshots 200
- [x] reviewer — [VERDICT: APPROVE], MERGEABLE, Blocker nur pending Checks
---

## Baseline (setup) — 2026-10-15

- Branch `feat/issue-351-import-history-polish`, HEAD `9cf473a` (from origin/main). Working tree clean (nur `progress-issue-351.md` untracked).
- `music-hub/` (separate crate, kein Workspace-Member) auf Host `planet`.
- `cargo build --locked` → **OK** (36.5s), 1 pre-existing Warnung (`web.rs:997` dead_code: `tags`, `me`).
- `cargo test --locked --test history` → **grün, 6 passed / 0 failed** (0.35s).
- Informativ: `cargo fmt --check` NICHT clean (u. a. `src/analyze.rs:37`); `clippy` ~36+ Warnungen (collapsible_if, assert_eq mit bool-Literal, sort_by_key …). Nicht gefixt.
- Tools vorhanden: flac, ffmpeg, ffprobe, exiftool → Media-Tests lauffähig.

## Plan (planner) — Polish /history

Grundlage: bestehende H3-Seite (`history.rs`, `history.html`, `table.rs`), Migration
`030_import_history.sql`, Tests `tests/history.rs`. Keine neue Migration, keine
Schema-Änderung. Alles serverseitig (QueryBuilder + `push_bind`), LIMIT 500 bleibt.
askama-Regel: **keine Vergleiche/Logik in Attributen** → alle `selected`/`active`/
Klassen/Zustände als vorberechnete Struct-Felder (Muster: `status_class`,
`change_class`, `SortHead`).

### Betroffene Dateien
- `music-hub/src/history.rs` — `HistoryFilter` (+`change`, `run`), Event-Query, Run-Query
  (Aggregat), `RunRow`/`EventRow`-Felder, neue Helfer (`parse_stats`, `diff_*`-Flags,
  Counts).
- `music-hub/templates/history.html` — Filterbalken (change-Select, Run-Chip),
  Diff-Zelle, Stats-Chips, Count-Badges, Empty-States.
- `music-hub/templates/base.html` — CSS: `hub-diff-add`, `hub-diff-rem`, `hub-chip-row`
  (kleine, additive Ergänzungen; bestehende `hub-badge*`/`hub-chip*` wiederverwenden).
- `music-hub/tests/history.rs` — neue Integrationstests (reqwest gegen `/history`).

### User Stories (in Reihenfolge)

1. **Change-Kind-Filter (`change`) im Filterbalken**
   - AC: Filterbalken erhält `<select name="change">` mit Optionen `alle` / `added` /
     `removed` / `changed` (deutsche Anzeige, Wert = roh). Serverseitig: `HistoryFilter`
     bekommt `change: Option<String>`; nur Whitelist `["added","removed","changed"]`
     wird angewandt, sonst ignoriert. Wirkt **nur** auf die Event-Query:
     `eq.push(" AND e.change = ").push_bind(change.clone())`. Läufe-Tabelle unberührt.
     Auswahl als vorberechnetes `selected`-Feld (ChangeOption-Struct wie SourceOption).
     `change` bleibt in Sort-/Reset-Links erhalten (`prefixed_sort_head` konserviert die
     Rohquery automatisch).
   - Betroffen: `history.rs` (Filter + Query + Option-Build), `history.html` (Select).
   - Gecsh. LoC: ~40.

2. **Per-Run-Filter (`run`) + aktiver Filter-Chip + Reset**
   - AC: Die Run-`id`-Zelle in der Läufe-Tabelle ist ein Link auf `/history?run=<id>`,
     der die übrigen Filter (q/source/user/change + Runs-Sort) erhält und `run` setzt
     (href serverseitig in `RunRow` vorberechnen, kein Template-Logik). `HistoryFilter`
     bekommt `run: Option<i64>` via `#[serde(default, deserialize_with = "crate::table::qnum::opt_i64")]`
     (toleriert leeres `run=`). Mit `run=N` Event-Query: `AND e.run_id = ?`. Oberhalb der
     Timeline ein aktiver Filter-Chip „Run #N ×", dessen Link nur `run` entfernt
     (übrige Parameter bleiben). Der globale „Zurücksetzen"-Button leert weiterhin alles.
     Chip-Sichtbarkeit als vorberechnetes Bool `run_filter_active` + `run_chip_href`.
   - Betroffen: `history.rs` (Filter, Query, Struct-Felder), `history.html` (Link + Chip).
   - Gecsh. LoC: ~45.

3. **Diff vorher→nachher lesbar darstellen**
   - AC: Die Event-Tabelle fasst `before`/`after` in **einer** Diff-Zelle zusammen:
     `added` → nur `after`, grün/emphasized (Klasse `hub-diff-add`); `removed` → nur
     `before`, rot + durchgestrichen (`hub-diff-rem`); `changed` → `before → after`
     (before rot/durchgestrichen, Arrow, after grün). Fehlender Wert ⇒ „—". Alle
     Flags/Klassen vorberechnet (`EventRow`: `diff_before`, `diff_after`,
     `before_class`, `after_class`, `has_diff`, `is_added/is_removed`). Kein `==` im
     Template-Attribut. Spalten `vorher`/`nachher` werden zur Spalte „Änderung (Diff)"
     (colspan-Anpassung der Empty-State-Zeile).
   - Betroffen: `history.rs` (EventRow-Felder + Helfer), `history.html` (Zelle),
     `base.html` (CSS `hub-diff-add`/`hub-diff-rem`).
   - Gecsh. LoC: ~50.

4. **`stats`-JSON als key:value-Chips rendern**
   - AC: `RunRow.stats` wird serverseitig geparst (`serde_json::from_str::<Value>`); ist
     es ein Objekt, entsteht `Vec<StatChip{key, value}>` (values via `Display`: Strings
     nackt, Zahlen/Bools wie-is, verschachtelt als kompaktes JSON). Template rendert
     Chips (`hub-chip-row`/`hub-chip`, `key: value`). Leeres `{}`/NULL/ungültiges JSON
     ⇒ Empty-Platzhalter „—" (vorberechnetes `stats_chips.is_empty()`). Roher Blob wird
     nicht mehr ausgegeben (Rückfall nur als `title`-Attribut ok, nicht nötig).
   - Betroffen: `history.rs` (`parse_stats`-Helfer + RunRow-Feld), `history.html`.
   - Gecsh. LoC: ~35.

5. **Änderungs-Zählung als Badges in der Läufe-Tabelle**
   - AC: Die Läufe-Query liefert je Run die Event-Zählungen:
     `LEFT JOIN (SELECT run_id, SUM(change='added') AS added, SUM(change='removed') AS removed,
     SUM(change='changed') AS changed FROM hub_import_events GROUP BY run_id) c ON c.run_id = r.id`
     (alle drei als `COALESCE(...,0)`), `RunRow` erhält `added/removed/changed: i64` +
     vorberechnete Badge-Klassen (added→`hub-badge-ok`, removed→`hub-badge-no`,
     changed→neutral). Template zeigt die drei Badges in einer „Änderungen"-Spalte
     (0 → gedämpft/ausgeblendet, vorberechnetes `has_changes`). Zählung ist unabhängig
     vom aktiven Event-Filter (Gesamtzahlen des Runs).
   - Betroffen: `history.rs` (Query + RunRow), `history.html` (Spalte/Badges).
   - Gecsh. LoC: ~40.

6. **Empty-States verfeinern (filter-bewusst)**
   - AC: Events leer **mit aktivem Filter** (run und/oder change) ⇒ Meldung
     „Keine Änderungen für diesen Filter" + Link zum Entfernen des Filters;
     Events leer **ohne** Filter ⇒ „Keine Änderungen.". Läufe leer ⇒ „Keine Import-Läufe."
     (bleibt), optional zweite Zeile Hinweis. Zustände über vorberechnete
     `events_filtered`/`has_any_filter`-Bools, kein Template-Vergleich.
   - Betroffen: `history.rs` (Bools), `history.html` (Empty-Zeilen).
   - Gecsh. LoC: ~20.

### Implementierungs-Reihenfolge
1 → 2 (Filter-Grundlagen, gemeinsam in `HistoryFilter` + Event-Query) → 5 (Run-Aggregat,
berührt Runs-Query) → 3 (Diff) → 4 (Stats-Chips) → 6 (Empty-States, zuletzt, da alle
Filterzustände voraussetzt).

### Testfälle (zu erweitern in `music-hub/tests/history.rs`)
Seed-Erweiterung nutzen: `seed_history` erzeugt bereits added (srcalpha/alice),
changed (srcbeta/alice), removed (srcalpha/bob) über Runs `a`,`b`,`c`.

- `history_change_filter_scopes_events`: `GET /history?change=added` ⇒ nur `ALPHA-ref`;
  `?change=removed` ⇒ nur `GAMMA-ref`; unbekannter Wert `?change=bogus` wird ignoriert
  (alle Events sichtbar, wie ungefiltert).
- `history_run_filter_scopes_events`: `GET /history?run=<a>` ⇒ nur Events von Run a
  (`ALPHA-ref`), nicht `BETA-ref`/`GAMMA-ref`; der Filter-Chip (z. B. `Run #<a>`) ist im
  HTML; `?run=999999` ⇒ filter-bewusster Empty-State.
- `history_run_link_and_chip_present`: `/history`-HTML enthält `run=<a>`-Link in der
  Läufe-Tabelle und (bei gesetztem `run`) den Reset-Link/Chip, der `run` entfernt,
  übrige Params aber behält (z. B. `run=<a>&source=srcalpha` → Chip-Link enthält `source`).
- `history_change_counts_badges`: Läufe-Tabelle zeigt je Run die Zählung passend zu den
  geseedeten Events (Run a: added=1; Run b: changed=1; Run c: removed=1).
- `history_stats_rendered_as_chips`: Run a (`stats {"marker":"STATALPHA"}`) ⇒ HTML
  enthält `marker` und `STATALPHA` (key + value getrennt, nicht als Roh-`{...}`).
- `history_diff_rendering`: added-Event ⇒ after-Wert mit `hub-diff-add`; removed-Event ⇒
  before-Wert mit `hub-diff-rem`; changed-Event ⇒ beide Werte + Arrow.
- `history_filtered_empty_state`: `?change=added&run=<c>` (Run c hat nur removed) ⇒
  filter-bewusste Empty-Meldung, kein „Keine Änderungen." generisch.
- `history_filters_require_login` bleibt unverändert (Regressionsschutz).

### Konventionen
- Deutsche UI-Labels („Änderung", „hinzugefügt/entfernt/geändert", „Run #N").
- Pico.css + bestehende `hub-*`-Klassen; nur minimal additive CSS.
- LIMIT 500 beibehalten; Whitelist für `change`; `run` numerisch via `qnum::opt_i64`.
- Kein Code, kein Commit durch den Planner.

---

## Rework (Developer) — 2026-10-15

Review-Runde 2: ein Pflicht-Blocker (CHANGELOG) + zwei Hinweise.

- **Blocker — CHANGELOG:** `music-hub/CHANGELOG.md` unter `## [Unreleased]` um
  `### Added` + `### Changed`-Eintrag `(#351, PR #440)` ergänzt (Change-Filter,
  Per-Run-Filter+Chip, Diff-Zelle, Stats-Chips, Change-Count-Badges,
  filterbewusste Empty-States).
- **Hinweis 1 — `count_class("changed", n)`:** lieferte für `changed > 0` einen leeren
  String → `~n`-Badge ungefärbt. Jetzt `"changed" => "hub-badge-warn"`; neues CSS
  `.hub-badge-warn { background:#3a2f12; color:#ffd479; }` in `base.html` nahe
  `.hub-badge-dim`. `n <= 0` bleibt `hub-badge-dim`.
- **Hinweis 2 — `events_filter_active`:** berücksichtigt jetzt zusätzlich
  `!q.is_empty() || !source.is_empty() || !user.is_empty()`; `events_reset_href`
  entfernt via `query_drop` nun auch `source`/`user`/`q` (run/change/source/user/q sind
  mit der Runs-Tabelle geteilt — vollständiges Zurücksetzen gewünscht).
- **Tests:** `tests/history.rs` — neuer Test `history_empty_state_for_freetext_and_source_filters`
  (filterbewusster Empty-State + Reset-Link für `q`/`source`, kombiniert);
  `history_change_counts_badges` prüft zusätzlich `hub-badge-warn`.
- **Ergebnis:** `cargo test --locked` → grün; `--test history` 14 passed / 0 failed.

