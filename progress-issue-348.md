# Progress — Issue #348: Per-type "describes the song" toggle

Repo: momokli/momos-music-manager  Branch: feature/issue-348-describes-toggle (base origin/main 403e4b8)
PR target: main (independent)  Milestone: hub-tagging-0.27.0

## Issue
Title: Per-type "describes the song" toggle. Body: "Sort/describing semantics per type."
NOT already done: kein PR/Branch referenziert #348.

## Recon (origin/main)
- TagType::describes() (music-hub/src/tagtypes.rs:117) hard-wired: SETLIST/RUMPELKISTE=false, sonst true.
- TagType::describe_family() (159) Typ->Familie (PHASE_1..5->"PHASE"; SETLIST/RUMPELKISTE->None).
- DESCRIBE_FAMILIES=["MOOD","GENRE","VIBE","PHASE","MERKMAL"].
- Nutzer: artists.rs:789 (`if !t.describes()`); tagdone.rs nutzt describe_family + Setting engine_tag_done_types (CSV).
- Admin-UI /admin rendert settings::SETTINGS-Registry (bool->checkbox), Persistenz hub_settings.

## Ziel
"describes"-Semantik pro Tag-Typ konfigurierbar machen (Toggle) statt hard-wired, sichtbar/toggelbar unter /admin.

## Stage-Status
| # | Stage | Status | Ergebnis |
|---|-------|--------|----------|
| 1 | planner | done | Plan erstellt (S1–S6) |
| 2 | setup | done | Baseline grün (build + test --no-run) |
| 3 | developer | done | S1–S6 implementiert, build+test grün (11 neue Tests) |
| 4 | verifier | done | PASS — Diff/AK geprüft, `cargo test` grün (alle Targets 0 failed, Lib 75 passed) |
| 5 | tester | pending | |
| 6 | developer (PR) | pending | |
| 7 | reviewer | pending | |

## Koordination
- Nur music-hub/**. Kein Merge/main-Push. Commit-Style fix(#348):/feat(#348): <english>.

## Developer-Ergebnis (Stage 3)

Commit: `af6e29f` — `feat(#348): make per-type 'describes the song' semantics configurable.`

### Umgesetzt (S1–S6)
- **S1** `settings.rs`: 7 Keys `ENGINE_DESCRIBES_{MOOD,GENRE,VIBE,PHASE,MERKMAL,SETLIST,RUMPELKISTE}`
  als `flag(...)`/`SettingKind::Bool` in `SETTINGS` registriert; Defaults mood/genre/vibe/phase/
  merkmal=`1`, setlist/rumpelkiste=`0`. Doc-Kommentar ergänzt.
- **S2** `tagtypes.rs`: `describes()` bleibt built-in Default; neu `describe_key(self)->&'static str`
  (PHASE_1..5 → `engine_describes_phase`), `fn parse_bool(Option<&str>, bool)->bool`
  (1/true/on/yes) und `pub async fn describes_map(&SqlitePool)->HashMap<TagType,bool>`
  (Fallback auf `describes()` bei unset).
- **S3** `artists.rs`: `describes_map` einmal vor der AR3-Type-Dist-Schleife geladen; Filter auf
  `!describes.get(&t).copied().unwrap_or_else(|| t.describes())` umgestellt. Leere DB ⇒ identisch.
- **S4** `tagdone.rs`: `family_primary_type(fam)->Option<TagType>` ergänzt; `configured_families`
  retained nur Familien, deren Typ aktuell `describes==true` ist (unbekannte Tokens bleiben erhalten).
- **S5** `admin_save` (pages.rs) iteriert `SETTINGS` und schreibt alle `Bool`-Specs — die 7 neuen
  Checkboxen werden automatisch gerendert/persistiert. Kein Template-/Handler-Edit nötig.
- **S6** Doc-Kommentare in `tagtypes.rs`/`tagdone.rs` korrigiert (kein „inherent/hard-wired" mehr).

### Tests (neu, alle grün)
- `settings::tests::describe_toggles_are_registered` (7 Keys, Defaults, Kind=Bool)
- `tagtypes::tests::describe_key_covers_all_types`
- `tagtypes::tests::describe_key_unique_per_semantic_type`
- `tagtypes::tests::parse_bool_handles_truthy_and_fallback`
- `tagtypes::tests::describes_map_defaults_to_builtin`
- `tagtypes::tests::describes_map_reads_settings_with_builtin_fallback` (sqlx in-memory)
- `tagdone::tests::family_primary_type_maps_canonical_families`
- `tagdone::tests::configured_families_default_to_builtin`
- `tagdone::tests::configured_families_drop_disabled_types`

### Build/Test-Summary
- `cargo build` → **grün** (nur vorbestehende dead_code-Warnung web.rs:997).
- `cargo test` → **grün**: Lib 75 passed / 0 failed; alle Integrations-Targets 0 failed;
  Doc-tests 0.

### Betroffene Dateien
`music-hub/src/settings.rs`, `music-hub/src/tagtypes.rs`, `music-hub/src/artists.rs`,
`music-hub/src/tagdone.rs`. Keine Migration, nur `music-hub/**`.

## Verify-Ergebnis (Stage 4)

**Entscheidung: PASS** — Diff gegen AK S1–S6 geprüft, keine Blocker.

### Test-Summary (`cd music-hub && cargo test`, Host planet)
- Exit-Code **0**; alle ~39 Test-Targets **0 failed**.
- Lib: **75 passed / 0 failed**; UI-Integration: 53 passed; views: 5 passed; Doc-tests: 0.
- Neue #348-Tests vorhanden und grün:
  `settings::tests::describe_toggles_are_registered`,
  `tagtypes::tests::{describe_key_covers_all_types, describe_key_unique_per_semantic_type,
  parse_bool_handles_truthy_and_fallback, describes_map_defaults_to_builtin,
  describes_map_reads_settings_with_builtin_fallback}`,
  `tagdone::tests::{family_primary_type_maps_canonical_families,
  configured_families_default_to_builtin, configured_families_drop_disabled_types}`.
- `cargo build`: grün; nur vorbestehende dead_code-Warnung (`web.rs:997`, unabhängig von #348).

### AK-Check
- **S1** `settings.rs`: 7 `ENGINE_DESCRIBES_*`-Konstanten; alle via `flag(...)` (`SettingKind::Bool`)
  registriert. Defaults mood/genre/vibe/phase/merkmal=`1`, setlist/rumpelkiste=`0` — verifiziert
  (Code + Test). ✅
- **S2** `tagtypes.rs`: `describes()` unverändert (built-in Default); `describe_key()` mappt alle
  Typen, PHASE_1..5 → `ENGINE_DESCRIBES_PHASE`; `parse_bool()` akzeptiert `1/true/on/yes`,
  unset⇒Default; `describes_map()` liest `settings::load_all`, Fallback `describes()`. ✅
- **S3** `artists.rs:1311`: `describes_map` einmal vor der Type-Dist-Schleife geladen;
  Filter `artists.rs:1335` nutzt Setting-Wert mit Fallback. Leerer Store ⇒ identisch zu vorher. ✅
- **S4** `tagdone.rs`: `family_primary_type()` (MOOD/GENRE/VIBE/PHASE→Phase1/MERKMAL);
  `configured_families()` retained nur Familien mit aktivem `describes`; unbekannte Tokens bleiben. ✅
- **S5** Admin-UI: `templates/admin.html:28` rendert `kind=="bool"` generisch (Name=f.key);
  `pages.rs:3919` `admin_save` schreibt **alle** Bool-Specs unbedingt; `checked` aus Store/Default
  (`pages.rs:3859`). Kein Template-/Handler-Edit nötig. ✅
- **S6** Doc-Kommentare in `tagtypes.rs`/`settings.rs`/`tagdone.rs` korrigiert (kein „inherent/
  hard-wired"). ✅
- **Hygiene**: nur `music-hub/**` + Progress; **keine Migration**, keine fremden Module, kein
  `unsafe`, keine Secrets/Debug-Prints, kein `unwrap()` in neuen Produktionspfaden. ✅

### Findings
Keine blockierenden Findings. Randnotiz (nicht #348, aus dem Plan bestätigt):
`SettingKind::Csv` parst nur Zahlen, daher ist `engine_tag_done_types` (Familien-CSV) im Admin
faktisch nicht speicherbar — für #348 irrelevant (Bool-Toggles), ggf. separates Issue.

### Empfehlung
Weiter zu Stage 5 (Tester).

## Setup-Baseline

- **Branch verifiziert:** `feature/issue-348-describes-toggle` @ `4ff8c01` (== origin/main),
  up to date, nur `progress-issue-348.md` untracked. Kein neuer Branch, kein Commit nötig.
- **Toolchain:** cargo 1.97.1 (c980f4866 2026-06-30), rustc 1.97.1 (8bab26f4f 2026-07-14).
- **Build:** `cd music-hub && cargo build` → **grün**, Finished in 48.61s (nur 1 vorbestehende
  dead_code-Warnung in `src/web.rs:997` `TrackPage.tags/me`, irrelevant für #348).
- **Tests kompilieren:** `cargo test --no-run` → **grün**, alle ~39 Test-Targets gelinkt,
  Finished in 45.90s.
- **Fazit:** Baseline grün, Implementierung kann starten. Blocker: keine.

## Plan

### Zielbild
Die harte `describes`-Regel (SETLIST/RUMPELKISTE=false, sonst true) wird zu einer
**pro Typ in `hub_settings` konfigurierbaren** Eigenschaft. Unter `/admin` erscheint
je Typ eine Checkbox (die bestehende bool-Rendering-Logik der SETTINGS-Registry
wird wiederverwendet — kein Template-Umbau). PHASE_1..5 teilen sich wie beim Gewicht
(`engine_w_phase`) **einen** Toggle `engine_describes_phase`.

### Betroffene Dateien
- `music-hub/src/settings.rs` — 7 neue Setting-Keys + Registry-Einträge.
- `music-hub/src/tagtypes.rs` — Default-`describes` bleibt; Key-Mapping + async Resolver.
- `music-hub/src/artists.rs` — Filter `if !t.describes()` auf Setting-Wert umstellen.
- `music-hub/src/tagdone.rs` — `configured_families` mit `describes`-Zustand abgleichen.
- (kein neues Template; `templates/admin.html` rendert `kind=="bool"` generisch.)

### Setting-Design (7 Keys, Muster von `ENGINE_W_*`)
| Key | Label | Default |
|-----|-------|---------|
| `engine_describes_mood` | Beschreibt: Mood | `1` |
| `engine_describes_genre` | Beschreibt: Genre | `1` |
| `engine_describes_vibe` | Beschreibt: Vibe | `1` |
| `engine_describes_phase` | Beschreibt: Phase (alle Stufen) | `1` |
| `engine_describes_merkmal` | Beschreibt: Merkmal | `1` |
| `engine_describes_setlist` | Beschreibt: Setlist | `0` |
| `engine_describes_rumpelkiste` | Beschreibt: Rumpelkiste | `0` |

Alle als `flag(...)` in `SETTINGS` registriert → automatisch als Checkbox unter
`/admin`, Persistenz/Validierung/Overlay über die vorhandene Maschinerie.

### User Stories (in Reihenfolge)

1. **[S1] Settings-Keys + Registry-Einträge**
   - Akzeptanzkriterien: 7 `pub const ENGINE_DESCRIBES_*` in `settings.rs`; alle 7 in
     `SETTINGS` als `SettingKind::Bool` mit o.g. Default; `spec(key)` findet sie;
     Unit-Test `describe_toggles_are_registered` prüft Keys+Defaults+Kind=Bool.
   - Dateien: `music-hub/src/settings.rs`. LOC: ~35.

2. **[S2] Key-Mapping + Default-Resolver in tagtypes.rs**
   - Akzeptanzkriterien:
     - `describes()` bleibt als **built-in Default** erhalten (Doku: Fallback).
     - `TagType::describe_key(self) -> &'static str` mappt jeden Typ auf seinen
       Setting-Key (PHASE_1..5 → `engine_describes_phase`).
     - `pub async fn describes_map(pool: &SqlitePool) -> HashMap<TagType, bool>`:
       liest `settings::load_all`, pro Typ Fallback auf `describes()` wenn unset;
       bool-Parsing `1/true/on/yes`.
     - Helper `fn parse_bool(raw: Option<&str>, default: bool) -> bool`.
     - Tests: `describe_key_covers_all_types` (jeder `TYPE_NAMES`-Typ liefert Key);
       `describes_map_defaults_to_builtin` (leere DB → Mood true, Setlist false).
   - Dateien: `music-hub/src/tagtypes.rs`. LOC: ~45.
   - Abhängigkeit: braucht S1 (Keys).

3. **[S3] artists.rs nutzt Setting-Wert**
   - Akzeptanzkriterien: Vor der Typ-Distributions-Schleife wird
     `let describes = crate::tagtypes::describes_map(&st.pool).await;` einmal geladen;
     das Filter `if !t.describes() { return None; }` wird zu
     `if !describes.get(&t).copied().unwrap_or_else(|| t.describes()) { return None; }`.
     Verhalten mit leerer DB identisch zu vorher (Regression: bestehende Artist-View
     unverändert).
   - Dateien: `music-hub/src/artists.rs` (nur die ~6 Zeilen am Filter, `TypeDist`).
     LOC: ~5.
   - Abhängigkeit: S2.

4. **[S4] tagdone.rs konsistent halten**
   - Akzeptanzkriterien: `configured_families` behält CSV/Default, filtert die
     zurückgegebenen Familien aber auf solche, deren Typ aktuell `describes == true`
     ist (Map aus S2). Dazu `family_primary_type(fam)->TagType`-Hilfsmapping
     (MOOD→Mood, GENRE→Genre, VIBE→Vibe, PHASE→Phase1, MERKMAL→Merkmal). Ein
     abgeschalteter Typ wird damit nicht mehr als „nötige Familie“ verlangt; die
     Familien-Reihenfolge der CSV bleibt erhalten. So driften `describes` und
     „vollständig getaggt“ nicht auseinander.
   - Dateien: `music-hub/src/tagdone.rs`. LOC: ~30.
   - Abhängigkeit: S2. (Falls als zu weitreichend verworfen: dokumentieren, dann nur
     S1–S3 + S5.)

5. **[S5] Admin-UI-Verhalten verifizieren (kein Template-Code)**
   - Akzeptanzkriterien: `/admin` zeigt 7 neue Checkboxen in der Wertespalte;
     `admin_save` schreibt Bool-Flags immer (an/aus) — vorhandene Bool-Schleife
     deckt die neuen Keys automatisch ab (kein `pages.rs`-Edit). Nachweis: manueller/
     manueller HTTP-Test oder Assertion, dass `admin_save`-Bool-Pfad alle Bool-Specs
     persistiert.
   - Dateien: keine (verifizierend). LOC: 0.
   - Abhängigkeit: S1.

6. **[S6] Doku/Doc-Kommentar**
   - Akzeptanzkriterien: Doc-Kommentar oben in `tagtypes.rs` (die Zeile „inherent /
     hard-wired“ ist nach #348 falsch) korrigiert: describes ist pro Typ via
     `engine_describes_*` konfigurierbar, Default = built-in. Hinweis in `settings.rs`
     bei den neuen Keys.
   - Dateien: `music-hub/src/tagtypes.rs`, `music-hub/src/settings.rs`. LOC: ~10.

### Reihenfolge / Abhängigkeiten
S1 → S2 → {S3, S4} ; S5 (nach S1) ; S6 zuletzt. S3 ist der eigentliche
Bug-Fix-Kern; S4 hält das System kohärent, S6 Doku.

### Tests
- `tagtypes.rs` Unit: `describe_key_covers_all_types`, `describes_map_defaults_to_builtin`.
- `settings.rs` Unit: `describe_toggles_are_registered` (7 Keys, Defaults, Bool).
- Optional Integrations-(sqlx in-memory)-Test in `tagdone.rs`: Setlist-Toggle aus →
  Setlist nicht in required families; Merkmal-Toggle aus → MERKMAL nicht gefordert.
- Regressionsanker: leerer Settings-Store ⇒ `describes_map` == altes Hard-wired-Verhalten.

### Risiken / Hinweise
- **CSV-Validierung:** `SettingKind::Csv` parst nur Zahlen; `engine_tag_done_types`
  (CSV aus Familien) ist damit im Admin faktisch nicht speicherbar (bestätigt im Code).
  Für #348 irrelevant, da wir Bool nutzen — aber als Randnotiz/ggf. separater Blocker
  festhalten, falls S4 angefasst wird.
- **Semantische Kopplung:** `describes` (Sichtbarkeit/Sortierung) und `describe_family`
  (Familie, statisch) sind orthogonal. S4 entkoppelt die Completion-Anforderung von
  abgeschalteten Typen, damit kein toter Pflichteintrag entsteht.
- **Phase-Bündelung:** PHASE_1..5 teilen einen Toggle (analog Gewicht). Falls je Stufe
  nötig, wäre das ein bewusstes Scope-Out (11 statt 7 Keys).
- **Cache:** Map pro Request einmal geladen; keine Persistenz-/Migrationsänderung nötig.

### Geschätzte Gesamt-LOC: ~120 (ohne Tests), nur `music-hub/**`.
