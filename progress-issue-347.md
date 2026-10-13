# Progress — Issue #347: Tag-task UI

Repo: momokli/momos-music-manager · Milestone: hub-tagging-0.27.0 (#33)
Base: main (9c841de) · Branch: feature/issue-347-tag-task-ui · PR target: main
Deliverable: pushed branch + open PR against `main`, body contains `Closes #347`.

## Issue (Spec)
**"Tag-task UI"** — "Card-based tagging queue for the daily/weekly task."

Milestone outcome: a track is *tagged-complete* when the 5 describing types are set
(or user-forced); a daily/weekly tagging task surfaces uncomplete tracks
(most-played first) in a **card-based queue**; per-type "describes the song" toggles
refine the semantics.

Order: #345 → #346 → **#347** → #348.
- #345 (DONE, merged PR #401 / 14fc0eb): `hub_track_tag_done`, `tagdone.rs`.
- #346 (DONE, merged PR #402 / 9c841de): backend task — `046_tag_task.sql`,
  `src/tagtasks.rs` (`Cadence`, `period_start`, `candidates`, `task_for`),
  `GET /api/hub/tag-task?cadence=daily|weekly`, settings `ENGINE_TAG_TASK_CADENCE`,
  `ENGINE_TAG_TASK_LIMIT`.
- **#347 = this issue: the card UI** consuming the #346 task. #348 = per-type toggle.

## Recon (verbindlich)
- Server-rendered askama templates in `music-hub/templates/`, handlers in
  `music-hub/src/*.rs` (axum Router per module, merged in `api.rs`).
- Card/queue precedent: `src/tagqueue.rs` (`/tag-queue`), templates `queue.html`
  + `queue_detail.html`, shared partials `_partials/tag_clouds.html`,
  `_partials/rec_tags.html`, `_partials/track_tag_form.html`.
- Current user: `crate::web::current_user(&st, &headers)`; nav via
  `crate::ui::nav(&st,&headers,"<key>")` (redirect `/login` when None).
  Nav links live in `templates/base.html` (~line 1057 `Tag-Queue`).
- #346 API handler `api.rs::tag_task` shows exactly how to build the card list:
  `tagtasks::task_for(&pool,user_id,cadence,now,limit)` then join
  `hub_tag_task_items` → `hub_tracks` (title, artists), `missing` is a CSV string.
- Manual completion: `tagdone::set_done` / `clear_done` on
  `hub_track_tag_done(user_id,track_id)`; `tagdone::status(...)`.

## Plan

Scope: eine server-gerenderte Seite `/tag-task`, die die persistierte #346-Task als
**Kartenliste** (most-played first) zeigt, pro Karte die **fehlenden describing
families** als Chips rendert und direktes Taggen + `done`/`undo` erlaubt. Kein
#348-Toggle, keine Neuberechnung der Task.

### Domänenregeln (verbindlich)
- Kartenquelle ist **ausschließlich** `crate::tagtasks::task_for(&pool, user_id, cadence,
  now, limit)` plus der Join `hub_tag_task_items` → `hub_tracks` (title, artists). Die
  Task wird **nicht** neu berechnet (kein `candidates()`-Aufruf im Handler).
- Cadence: `cadence`-Query optional; Default aus `ENGINE_TAG_TASK_CADENCE`, sonst
  `Cadence::Daily` (exakt wie `api.rs::tag_task`). Limit aus `ENGINE_TAG_TASK_LIMIT`,
  Default 50, `>0`-Filter.
- Reihenfolge = `rank ASC` der Task (`plays DESC, track_id ASC`); Karten zeigen
  `rank` + `plays`.
- `missing` aus `hub_tag_task_items.missing` ist CSV (Familien-Namen in CRATE-Großschreibung,
  z. B. `MOOD,ENERGY,PUBLIC`). Anzeige über `crate::tagtypes::TagType::parse` → `label()`/
  `icon()` mappen, Fallback Rohname.
- Taggen mutiert über die bestehende Pipeline (`tags::ensure_tag` → `tagtypes::set` →
  `tags::tag_track`), wie `web.rs::track_tag`/`tagqueue::queue_tag`.
- `done` = `crate::tagdone::set_done`, `undo` = `crate::tagdone::clear_done`
  (`hub_track_tag_done(user_id,track_id)`), idempotent.
- Laufzeit-Filter: Beim Rendern Karten unterdrücken, deren Track manuell `done` ist
  (`tagdone::is_done`), damit die Task-Items trotz Persistenz binnen der Periode
  nicht als erledigt stehen bleiben. Auto-complete Tracks sind bereits durch `candidates()`
  ausgeschlossen. Empty-State, wenn keine Karten übrig sind.
- Auth-Gate in jedem Handler: `crate::ui::nav(&st,&headers,"tag-task")`, sonst
  `Redirect::to("/login")`.

### Betroffene Dateien
- `music-hub/src/tagtaskui.rs` (neu) — axum Router, askama Templates, Handler.
- `music-hub/templates/tag_task.html` (neu) — Seiten-Shell (extends `base.html`).
- `music-hub/templates/_partials/tag_task_card.html` (neu) — Karten-Fragment
  (Chips + Tag-Form + done/undo).
- `music-hub/templates/base.html` — Nav-Link „Tag-Task" unter Bibliothek.
- `music-hub/src/api.rs` — `.merge(crate::tagtaskui::router(state.clone()))`.
- `music-hub/src/lib.rs` — `pub mod tagtaskui;`.
- `music-hub/tests/tag_task_ui.rs` (neu) — Integrationstests.

Wiederverwendung statt Neubau: `_partials/track_tag_form.html` (Tag-Eingabe inkl.
`my_types`-Select), `_partials/tag_clouds.html` / `_partials/rec_tags.html` optional für
Karten-Detail; `queue.html` + `ui::nav` als Struktur-Vorbild.

### User Stories (in Reihenfolge)

1. **Route + Seiten-Shell `/tag-task` (Kartenliste aus #346)**
   - Akzeptanzkriterien: `GET /tag-task` ohne Session → 302 `/login`; mit Session → 200
     HTML mit Topbar (`nav.active == "tag-task"`); der Handler ruft `tagtasks::task_for`
     mit Setting-Cadence/-Limit auf und listet die Items in `rank`-Reihenfolge; leere
     Task → Empty-State-Text; `missing` wird als Chip-Liste gerendert.
   - Betroffene Dateien: `src/tagtaskui.rs`, `templates/tag_task.html`, `src/api.rs`, `src/lib.rs`.
   - Geschätzte LoC: ~130 (Rust) + ~50 (Template).

2. **Karten-Fragment mit Track-Kopf, Plays/Rank und fehlenden Familien**
   - Akzeptanzkriterien: pro Item eine Karte mit `rank`, `title`, `artists`, `plays`,
     Link `/track/{id}`; fehlende Familien als lesbare Chips (Icon+Label via
     `tagtypes`); bereits `done`-markierte Tracks werden beim Rendern ausgefiltert.
   - Betroffene Dateien: `templates/_partials/tag_task_card.html`, `templates/tag_task.html`,
     `src/tagtaskui.rs`.
   - Geschätzte LoC: ~35 (Template) + ~40 (Rust-View-Mapping).

3. **Tagging aus der Karte**
   - Akzeptanzkriterien: Karte enthält Tag-Formular (Name-Input + Typ-Select aus
     `tagtypes::options()`); `POST /tag-task/{id}/tag` wendet den Tag über die bestehende
     Pipeline an; danach Redirect zurück auf `/tag-task` (bzw. htmx-Karten-Refresh),
     sodass die verbrauchte Familie bei nächstem Render aus `missing` verschwindet; Scope
     wird nicht benötigt (Kartenkontext).
   - Betroffene Dateien: `src/tagtaskui.rs`, `templates/_partials/tag_task_card.html`.
   - Geschätzte LoC: ~50.

4. **`done` / `undo` pro Karte**
   - Akzeptanzkriterien: „Fertig"-Button → `POST /tag-task/{id}/done` →
     `tagdone::set_done`; „Rückgängig" (bzw. auf erledigten) → `POST /tag-task/{id}/undone`
     → `tagdone::clear_done`; nach `done` verschwindet die Karte (Render-Filter), nach
     `undo` erscheint sie wieder; idempotent.
   - Betroffene Dateien: `src/tagtaskui.rs`, `templates/_partials/tag_task_card.html`.
   - Geschätzte LoC: ~55.

5. **Navigationseintrag + aktiver Zustand**
   - Akzeptanzkriterien: Link „Tag-Task" → `/tag-task` im Bibliotheks-Menü von
     `base.html`; bei aktiver Seite `aria-current="page"` (Key `tag-task`).
   - Betroffene Dateien: `templates/base.html`.
   - Geschätzte LoC: ~3.

6. **Integrationstests**
   - Akzeptanzkriterien: Tests decken Login-Gate, Task-Konsum (Seite reflektiert
     `task_for`-Items, most-played zuerst), Anzeige fehlender Familien, erfolgreiches
     Taggen aus der Karte, `done`-Ausblenden + `undone`-Wiederanzeige sowie Nav-Aktivzustand
     ab; Aufbau wie `tests/tag_tasks.rs` / `tests/tagqueue.rs` (Seed, `common::spawn`,
     `session_cookie`, Hilfe `add_tag`/`set_plays`).
   - Betroffene Dateien: `tests/tag_task_ui.rs`.
   - Geschätzte LoC: ~180.

### Implementierungs-Reihenfolge
`lib.rs`+`api.rs` (Modul/Route scaffold) → US1 → US2 → US3 → US4 → US5 → US6.
US1/US2 sind Voraussetzung für alle weiteren; US3 und US4 sind unabhängig voneinander,
US5/US6 schließen ab.

### Teststrategie
- Domänen-/Unit-nah: Reihenfolge + `missing`-Mapping werden implizit über
  `tagtasks`-Fixtures geprüft (Task bleibt single source).
- Integration (axum + reqwest, wie bestehende Suites): Login-Redirect, HTTP 200 +
  Karteninhalt, Tag-POST + Redirect, done/undone-POSTs, Nav-`aria-current`.
- Kein Rust-Code in dieser Progress-Datei; Tests liegen in `tests/tag_task_ui.rs`.

### Offene Risiken
- **Persistenz vs. Reaktivität:** Da `tagtasks::task_for` die Items für die Periode
  einfriert, muss der Render-Filter (`tagdone::is_done`) die `done`-Karten aktiv
  unterdrücken, sonst wirkt `done` ohne sichtbare Wirkung.
- **Partial-Kopplung:** `track_tag_form.html` hat hartkodierte `/track/...`-Actions;
  falls es nicht parametrisiert wird, wird das Formular im Karten-Partial neu
  geschrieben (bewusste Duplikation statt invasiver Änderung am Track-Home).
- **Chips-Semantik:** `missing` ist Familien-Großschreibung; Mapping auf `TagType`
  muss unbekannte Familien robust als Rohlabel durchlassen.
- **htmx optional:** Scope klein halten — Primary-Flow als klassische Formulare +
  Redirect; htmx-Karten-Refresh nur, falls ohne Mehraufwand möglich.

## Stage status
| Stage     | Status  | Result |
|-----------|---------|--------|
| recon     | done    | siehe oben |
| planner   | done    | Plan siehe oben |
| setup     | done    | Branch `feature/issue-347-tag-task-ui` @ c9b6eef (auf origin/main 9c841de). `cargo test` (music-hub) EXIT 0: 41 Suites, **350 passed / 0 failed**. Log: /tmp/347-baseline.log. Keine `pool timed out`-Hänger (TMPDIR-Workaround nicht nötig). |
| developer | done    | Umgesetzt US1–US6. Neu: `music-hub/src/tagtaskui.rs` (Router `/tag-task`, `/tag-task/{id}/tag|done|undone`), `templates/tag_task.html`, `templates/_partials/tag_task_card.html`; Nav-Link „Tag-Task“ in `base.html` (+ Card-Grid-CSS); Modul in `lib.rs`, Router-Merge in `api.rs`. Karten aus `tagtasks::task_for` (kein `candidates()`), Rank/Plays, `missing`-CSV → Chips via `tagtypes` (PHASE-Fallback), `done`-Filter via `tagdone::is_done`, Laufzeit-Missing via `tagdone::status` (Reaktivität US3). Taggen via `tags::ensure_tag`/`tagtypes::set`/`tags::tag_track`. Tests: `tests/tag_task_ui.rs` (7) — Login-Gate, Order, Chips, Taggen-clears-Missing, done/undone, Empty-State, POST-Gate. `cargo test` EXIT 0: **357 passed / 0 failed** (42 Suites; Baseline 350/41). Log: /tmp/347-developer-full.log. Commits: 395c92e (feat), 7df7ae7 (test) — gepusht. |
| verifier  | pending | |
| tester    | pending | |
| reviewer  | pending | |
