# Plan: Hub Tagging-System + Spelunking-Engine

**Status**: approved
**Owner**: next agent
**Branch**: (per leaf issue) `feat/hub-…`
**Depends on**: existing tag layer (`plans/mmm-hub/*`, ADR-073), MMM parent/energy import, group weights + ranked groups
**Migration needed**: yes (several; consolidate per release)

> Companion to [`AGENT.md`](../../mmm-hub/AGENT.md). One issue = one branch = one PR
> (`Closes #<n>`), Conventional-Commit titles, never commit to `main`.
> IDs like `T1-1` are planning-local; GitHub numbers are assigned on creation.

---

## 0. GitHub-Artefakte

**Epic**: #203 · **Milestones**: #15 `hub-tags-0.10.0`, #16 `hub-insights-0.11.0`,
#17 `hub-traktor-0.13.0`, #18 `hub-scoring-0.12.0`, #19 `hub-tasks-0.14.0`,
#20 `hub-sim-0.15.0`, #21 `hub-history-0.16.0`.

| Plan-ID | Issue | Titel                                                    |
| ------- | ----- | -------------------------------------------------------- |
| T1-1    | #204  | group kind/role columns + backfill                       |
| T1-2    | #205  | tag-page group filter (multi)                            |
| T1-3    | #206  | create tag from tag page                                 |
| T1-4    | #207  | genre variation / main direction                         |
| T2-1    | #208  | tag insights — top artists                               |
| T2-2    | #209  | tag co-occurrence (same + cross group)                   |
| T2-3    | #210  | jump/discover from co-tag                                |
| T3-1    | #211  | traktor import CLI (collection.nml)                      |
| T3-2    | #212  | traktor playlists + sessions                             |
| T3-3    | #213  | traktor meta views for scoring                           |
| T4-1    | #214  | ripeness score (meta+tags, human>meta)                   |
| T4-2    | #215  | traktor signal in ripeness                               |
| T4-3    | #216  | scoring settings in UI                                   |
| T4-4    | #217  | engine params — all weights + distributions configurable |
| T5-1    | #218  | track tagged-complete flag                               |
| T5-2    | #219  | daily/weekly tagging task                                |
| T5-3    | #220  | tag-task UI                                              |
| T6-1    | #221  | similarity v2 — tag overlap cross-group                  |
| T6-2    | #222  | rumpelkiste exception in similarity                      |
| T6-3    | #223  | wire sim into digging/overlap                            |
| H1      | #224  | import-run ledger (all sources)                          |
| H2      | #225  | import diff / change events                              |
| H3      | #226  | import history UI                                        |

---

## 1. Anforderungen (sortiert)

### A — Tag-Verwaltung & Gruppen-Typen

- **A1** Tag-Seite (`/tags`): **Filter nach Tag-Gruppen** (mehrere gleichzeitig).
- **A2** Tag-Seite: **direkt neue Tags anlegen** (ohne Umweg über eine Playlist).
- **A3** Gruppen bekommen einen **Typ**:
  - _Klassifizierend_: **Mood, Vibe, Phase/Energy, Genre, Attribute, Merkmal** (Attribute ist eine **eigene** Gruppe _zusätzlich_ zu Merkmal — beide eigenständig; später im UI anpassbar).
  - _Sortierend_: **Rumpelkiste, Setlist**.
  - → `hub_tag_groups.kind` (`class` | `sort`), plus semantische Rollen `rumpelkiste`, `setlist`, `genre`, `phase`.
  - **„Vollständig getaggt" bezieht sich auf die _aktuell konfigurierten_ klassifizierenden Gruppen — nicht hart auf 5** (siehe F1).
- **A4** **Genre-Sonderfall**: Genre-Tags haben **Variation + Hauptrichtung** (z. B. Variation `Psy` → Haupt `Trance`; Variation `Tech` → Haupt `House`). → als Parent-Relation (Variation = Tag, Hauptrichtung = Parent) im Genre-Kontext, genutzt fürs Scoring/Ähnlichkeit.

### B — Tag-Insights (Tag-Detailseite `/tag/{id}`)

- **B1** **Top-Artists** dieses Tags (Häufigkeit absteigend).
- **B2** **Co-Occurrence-Tags**: welche Tags kommen besonders oft mit diesem Tag zusammen vor — **innerhalb derselben Gruppe** _und_ **über Gruppen hinweg** (z. B. `Mood Dark` ↔ `Mood Melancholisch`, `Mood Dark` ↔ `Vibe Warehouse`).
- **B3** **Sprung/Discovery**: von einem Co-Tag direkt auf den Tag springen bzw. ins Digging/Overlap zu den gemeinsamen Tracks.
- **B4** Metric: Co-Occurrence via Jaccard/Lift über `hub_track_resolved_tags`, gruppiert je Gruppe.

### C — Similarity zwischen zwei Tracks

- **C1** **Tag-Overlap-Boost**: mehrere gemeinsame Tags erhöhen die Ähnlichkeit — **stärker, wenn sie in unterschiedlichen Gruppen liegen** (Cross-Group zählt mehr als Same-Group).
- **C2** **Rumpelkiste-Ausnahme**: ist ein Track über eine **Rumpelkiste-Playlist** verknüpft, wird diese Playlist **nicht** für den Ähnlichkeitsvergleich herangezogen — nur **andere** Playlists oder die **Tags**.

### D — Scoring-System

- **D1** **Ripeness Score** ("wie gut sind die Daten _atomar_ für diesen Track?"):
  - **Track-Meta**: BPM, Key, Album, Genre, Cover, Artist, Title.
  - **Human-Tags**: Tags in Gruppen.
  - **Traktor-Meta**: play count, last played, rating (none|1-5), Vorkommen in Traktor-Playlists/Collection/Session-History.
  - **Regel: HUMAN TAGS > META TAGS** (Tags wiegen mehr als Metafelder).
- **D2** **Tag-Punkte je Gruppe** (positionsbasiert): 1. Tag = **100**, 2. = **50**, 3. = **25**, 4. = **10**, 5. = **5**, ab 6. = **1**.
  - Die Punkte hängen an der **Gruppe** (Multiset), **nicht** am einzelnen Tag: Löscht man den „100er"-Tag, verschwinden nicht 100, sondern die **niedrigste** Stufe der Gruppe. Beispiel: 2 Vibe-Tags = 150; entfernt man _einen_ (egal welchen) → −50 (die „billigste" Stufe).
  - Formel: `group_points(k) = Σ_{i=1..k} w_i` mit `w = [100,50,25,10,5,1,1,…]`.
- **D3** **Similarity Score** (zwischen zwei Tracks) separat vom Ripeness (siehe C).
- **D4** **Score-Schwelle ist ein UI-Filter, kein Hard-Cutoff**: In jeder Ansicht (Tag-Aufgabe, Digging, Overlap, Track-Liste, …) gibt es einen **Score-Filter** (min/max). Der Ripeness-Score wird **on the fly** berechnet und ist **unabhängig von fixen Werten**; es gibt keinen eingebauten Ausschluss. Default des Filters ist offen (kein Ausschluss).

### H — Historisierung aller Import-Quellen

- **H1** **Run-Ledger**: Jeder Import (Spotify-Sync, Traktor-Upload, später SoundCloud/YouTube) schreibt einen Lauf mit Zeitstempel, Quelle, Status und Statistiken in `hub_import_runs`.
- **H2** **Change-Events / Diff**: Zwischen zwei Importen wird erfasst, **was sich über die Zeit ändert** — `hub_import_events` (added/removed/changed) für Playlist-Mitgliedschaft, Track-Meta-Deltas, Traktor-playcount/rating/last-played usw. Querschnitt über alle Quellen.
- **H3** **History-UI**: Läufe je Quelle/User + Timeline der Änderungen (pro Track, pro Playlist), filterbar wie die übrigen Tabellen.
- **H4** **Traktor-Quelle**: **jeder User lädt seine eigene `collection.nml` hoch** (siehe E1).

### E — Traktor-Meta-Ingest (Voraussetzung für D1/E)

- **E1** Pro User **`collection.nml`** importieren: play count, last played, rating, Playlists/Collection/Session-History. **Upload über die UI** (jeder User lädt seine eigene Datei hoch).
- **E2** Speicherung + Views fürs Scoring/„am häufigsten gespielt".

### F — Tägliche/wöchentliche Tag-Aufgabe

- **F1** Ein Track gilt als **vollständig getaggt**, wenn **alle _konfigurierten_ klassifizierenden Gruppen** getaggt sind **oder** der User ihn als **fertig** markiert. (Kein hartes „5" — die Menge der Klassifizierungsgruppen ist Teil der Konfiguration.)
- **F2** Die Aufgabe zeigt **am häufigsten gespielte Traktor-Tracks zuerst**, gemäß Scoring.
- **F3** **Cadence**: täglich **und** wöchentlich (Reset/Rollover, Historie). **Reset-Zeiten sind konfigurierbar** (Tages-Uhrzeit + Wochen-Wochentag, Zeitzone) über das Setting-Registry.
- **F4** Der Score-Filter (D4) ist hier wie überall ein **UI-Filter**, kein eingebauter Skip.

### G — Engine-Integration

- **G1** Ripeness + Similarity in **Digging** (Suche/Ähnlichkeit), **Overlap** und **Tag-Insights** nutzen.
- **G2** Faktoren/Schwellen **konfigurierbar im Web-UI** (admin-Settings + pro Collective).
- **G3** **Alle Gewichtungen, Verteilungen, Koeffizienten, Schwellen und Zeitpunkte** sind als **Werte** im Hub hinterlegt (kein Hardcoding) und im UI **fine-tunebar**: Tag-Punkt-Vektor, Cross-Group-Bonus, Gewichte je Gruppe bzw. Tag-Typ (z. B. Rumpelkiste-Match zählt wenig, Mood-Match viel), **Meta-Gewichte** (BPM/Key/Album/Genre/Cover/Artist/Title), **Traktor-Gewicht**, `shared_weight` vs. `candidate_weight`, Similarity-Koeffizienten, Score-Filter-Defaults, **Cadence-Reset-Zeiten** (Tages-/Wochen-Rollover), Co-Occurrence-Metrik-Wahl. Typed Setting-Keys mit Default + Range/Validierung; editierbar im UI, pro Collective überschreibbar. → **T4-4** (#217).
- **G4** **Nichts ist hart im Code** — jede Konstante der Engine/Aufgaben wird über das Setting-Registry gelesen (Default → Setting → Collective-Override).

---

## 2. Scoring-Spec (verbindlich)

```
w            = [100, 50, 25, 10, 5, 1, 1, …]          # Punkte je Position in einer Gruppe
group_points(k) = sum(w[0..k])                         # k = Anzahl Tags in dieser Gruppe

TAG_SCORE    = Σ_group group_points(k_group)           # Human-Tags (aufgewertet, HUMAN > META)
META_SCORE   = meta_fields_present(track)             # bpm,key,album,genre,cover,artist,title
TRAKTOR_SCORE= f(play_count, last_played, rating, session_occurrence)

RIPENESS     = TAG_WEIGHT   * TAG_SCORE
             + META_WEIGHT  * META_SCORE
             + TRAK_WEIGHT  * TRAKTOR_SCORE            # TAG_WEIGHT > META_WEIGHT (Default z. B. 3 : 1)

Rumpelkiste-Regel (Similarity): Playlist-Kanten, deren Playlist zu einem Rumpelkiste-Tag
  gehört, werden beim Track↔Track-Vergleich ignoriert; nur Nicht-Rumpelkiste-Playlists + Tags zählen.

SIM(a,b)     = Σ_over shared group g  w_g * cross_group_bonus
             + tag_overlap_count_bonus                  # mehrere gemeinsame Tags, cross-group stärker
```

- **Rumpelkiste-Erkennung**: eine Gruppe mit `kind='rumpelkiste'`; ein Track „ist in der Rumpelkiste", wenn er über `hub_group_tags` → `hub_tag_sources` an eine Playlist eines Rumpelkiste-Tags gebunden ist.
- **Genre Variation/Haupt**: Variation-Tag hat Parent = Hauptrichtung (wie in §1 A4); fürs Scoring zählt optional die Hauptrichtung als zusätzlicher Tag.

---

## 3. Datenmodell-Deltas (Skizze)

| Migration                              | Inhalt                                                                                                                                                                     |
| -------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------- |
| `hub_tag_groups.kind` + `role`         | `ALTER TABLE hub_tag_groups ADD COLUMN kind TEXT NOT NULL DEFAULT 'class'; ADD COLUMN role TEXT NOT NULL DEFAULT '';` (Rollen: `rumpelkiste`, `setlist`, `genre`, `phase`) |
| `hub_traktor_tracks`                   | `(user_id, track_id, play_count INT, last_played TEXT, rating INT, imported_at)` PK `(user_id, track_id)`                                                                  |
| `hub_traktor_playlists` / `_tracks`    | Traktor-Playlists + Mitgliedschaft (für „Vorkommen in …")                                                                                                                  |
| `hub_traktor_sessions` / `_tracks`     | Session-History-Vorkommen                                                                                                                                                  |
| `hub_track_tag_done`                   | `(user_id, track_id, marked_at)` – manuelles „fertig getaggt"                                                                                                              |
| `hub_tag_tasks`                        | `(user_id, cadence daily                                                                                                                                                   | weekly, period, track_id, status, created_at)` – Aufgabe/Cadence |
| `hub_track_ripeness` (optional, cache) | `(track_id, ripeness REAL, computed_at)` – wenn On-the-fly zu teuer                                                                                                        |
| `hub_import_runs`                      | `(id, user_id, source, started_at, finished_at, status, stats)` – Run-Ledger für alle Quellen                                                                              |
| `hub_import_events`                    | `(id, run_id, user_id, source, entity_type, entity_ref, change, before, after, at)` – Änderungen über Zeit                                                                 |

Genre Variation/Haupt: **kein** neues Schema — nutzt `hub_tag_parents` (Variation → Haupt) im Genre-Kontext.

---

## 4. Milestones (Vorschlag)

| Milestone             | Outcome                                                                               | Migration |
| --------------------- | ------------------------------------------------------------------------------------- | --------- |
| `hub-tags-0.10.0`     | Gruppen-Typen/Rollen, Genre Variation/Haupt, Tag-Seite: Gruppen-Filter + Tags anlegen | 026       |
| `hub-insights-0.11.0` | Tag-Insights: Top-Artists, Co-Occurrence (same- & cross-group), Sprung/Discovery      | —         |
| `hub-traktor-0.13.0`  | Traktor-Meta-Ingest (playcount/rating/sessions) ins Hub                               | 027       |
| `hub-scoring-0.12.0`  | Ripeness-Score (Meta+Tags+Traktor), Tag-Punkte je Gruppe, konfigurierbar              | 028       |
| `hub-tasks-0.14.0`    | Tägliche/wöchentliche Tag-Aufgabe + „fertig"-Markierung                               | 029       |
| `hub-sim-0.15.0`      | Similarity v2 (Tag-Overlap cross-group, Rumpelkiste-Ausnahme), in Digging/Overlap     | —         |
| `hub-history-0.16.0`  | Historisierung aller Import-Quellen (Run-Ledger, Change-Events, History-UI)           | 030       |

> Reihenfolge: `0.10 → 0.11 → 0.13 → 0.12 → 0.14 → 0.15` (Traktor vor Scoring, weil Scoring Traktor-Meta braucht). `0.16` (History) kann ab `0.13` parallel laufen.
> `0.11` und `0.15` können parallel zu `0.13` laufen.

---

## 5. Leaf-Issues (Backlog)

### Milestone `hub-tags-0.10.0`

- **T1-1** `feat(hub): group kind/role columns + backfill` — Migration + `hub_tag_groups.kind/role`, Rumpelkiste/Setlist/Genre/Phase rollen. _(AC: Migration läuft frisch; `kind`/`role` in `list_groups_for`/`group_detail`; Test.)_
- **T1-2** `feat(hub): tag-page group filter (multi)` — `/tags?groups=` mehrfach; server-side. _(AC: Filter kombiniert sich mit q/mine/owner/collective; Test.)_
- **T1-3** `feat(hub): create tag from tag page` — Formular + `POST /tags/create` (Owner = aktueller User). _(AC: Tag erscheint, Group optional; Test.)_
- **T1-4** `feat(hub): genre variation / main direction` — UI zum Setzen (Variation→Parent) + Anzeige; Import aus MMM falls vorhanden. _(AC: Relation persistiert; in Genre-Gruppe sichtbar; Test.)_

### Milestone `hub-insights-0.11.0`

- **T2-1** `feat(hub): tag insights — top artists` — aggregierte Artists je Tag auf `/tag/{id}`. _(AC: sortiert; Test.)_
- **T2-2** `feat(hub): tag co-occurrence (same + cross group)` — Lift/Jaccard je Gruppe; Anzeige „kommt oft mit …". _(AC: Beispiel Mood Dark↔Vibe Warehouse sichtbar; Test.)_
- **T2-3** `feat(hub): jump/discover from co-tag` — Links Tag→Tag sowie → `/overlap?tag=`/`/digging`. _(AC: Links erzeugen korrekte Auswahl; Test.)_

### Milestone `hub-traktor-0.13.0`

- **T3-1** `feat(hub): traktor import CLI (collection.nml)` — Parse + Store playcount/lastplayed/rating. _(AC: Test mit Beispiel-NML.)_
- **T3-2** `feat(hub): traktor playlists + sessions` — Membership + Session-History. _(AC: Views; Test.)_
- **T3-3** `feat(hub): traktor meta views for scoring` — `v_track_traktor(track,playcount,lastplayed,rating,sessions)`. _(AC: View liefert Werte; Test.)_

### Milestone `hub-scoring-0.12.0`

- **T4-1** `feat(hub): ripeness score (meta+tags, human>meta)` — Kern-Algo + `hub_track_ripeness`-Cache/Endpoint. _(AC: `group_points` exakt lt. §2; Unit-Tests inkl. „Löschen entfernt niedrigste Stufe".)_
- **T4-2** `feat(hub): traktor signal in ripeness` — playcount/lastplayed/rating/sessions einfließen. _(AC: Gewicht konfigurierbar; Test.)_
- **T4-3** `feat(hub): scoring settings in UI` — `engine_*`-Keys erweitert (Gewichte etc.). Score selbst wird on the fly berechnet; kein eingebauter Schwellwert. _(AC: `/admin` editierbar; Test.)_
- **T4-4** `feat(hub): engine params — all weights + distributions configurable` — zentrales Setting-Registry für **jeden** Engine/Aufgaben-Wert (Tag-Punkt-Vektor, Cross-Group-Bonus, Gruppen-/Tag-Typ-Gewichte, **Meta-Gewichte**, Traktor-Gewicht, `shared_weight`/`candidate_weight`, Similarity-Koeffizienten, Score-Filter-Defaults, **Cadence-Reset-Zeiten**, Co-Occurrence-Metrik) mit Default/Range/Validierung, editierbar im UI, pro Collective überschreibbar. Kein Wert hardcodiert. _(AC: Registry listet jeden Parameter; Wertänderung wirkt; Test.)_

### Milestone `hub-tasks-0.14.0`

- **T5-1** `feat(hub): track tagged-complete flag` — `hub_track_tag_done` + „fertig"-Button; Auto-Regel (alle _konfigurierten_ klassifizierenden Gruppen, nicht hart 5). _(AC: Zustand + Anzeige; Test.)_
- **T5-2** `feat(hub): daily/weekly tagging task` — Queue (Traktor-playcount-first), Cadence + Historie. Score ist ein **UI-Filter**, kein eingebauter Skip. _(AC: tägl./wöchentl. Liste; Test.)_
- **T5-3** `feat(hub): tag-task UI` — Seite/Widget „Tagge jetzt" mit Track + Tag-Eingabe. _(AC: Aktion setzt Tags; Test.)_

### Milestone `hub-sim-0.15.0`

- **T6-1** `feat(hub): similarity v2 — tag overlap cross-group` — Boost je gemeinsamer Gruppe, cross-group stärker. _(AC: Tests mit Fixtures.)_
- **T6-2** `feat(hub): rumpelkiste exception in similarity` — Rumpelkiste-Playlist-Kanten beim Vergleich ignorieren. _(AC: Test: Track nur via Rumpelkiste verknüpft zählt nicht.)_
- **T6-3** `feat(hub): wire sim into digging/overlap` — Ripeness/Similarity in Digging-Score + Overlap-Ranking; konfigurierbar. _(AC: bestehende Tests grün + neue.)_

### Milestone `hub-history-0.16.0`

- **H1** `feat(hub): import-run ledger (all sources)` — Migration + `hub_import_runs`; jeder Importer schreibt einen Lauf. _(AC: Run-Row je Import; Test.)_
- **H2** `feat(hub): import diff / change events` — `hub_import_events`, Diff zwischen Importen (Mitgliedschaft, Meta, playcount/rating). _(AC: Spotify- und Traktor-Delta-Tests.)_
- **H3** `feat(hub): import history UI` — Lauf-Liste + Change-Timeline, filterbar. _(AC: Runs + Diffs sichtbar; Test.)_

---

## 6. Entscheidungen

**Entschieden** (2026-10-10):

1. **„Attribute" = eigene Gruppe _zusätzlich_ zu Merkmal** — beide eigenständig, später im UI anpassbar. (§1 A3)
2. **Genre Hauptrichtung**: Relation Variation→Hauptrichtung (Parent) — dient Anzeige **und** optional dem Scoring/Similarity. (§1 A4)
3. **Score-Schwelle**: **kein Hard-Cutoff** — Score **on the fly**, in **jeder** UI als **Filter** (min/max). (§1 D4/F4)
4. **Traktor-Quelle**: **jeder User lädt seine eigene `collection.nml` hoch**. (§1 E1/H4)
5. **Historisierung**: Importe **aller Quellen** werden historisiert (Run-Ledger + Change-Events + UI) — was ändert sich über die Zeit. (§1 H)

**Alle Engine-Parameter sind konfigurierbar** (Setting-Registry, T4-4/#217) — es gibt keine offenen Entscheidungen mehr, die als Konstante festgezurrt werden müssten:

4. **Meta-Gewichte** (inkl. Verhältnis HUMAN:META und Traktor-Anteil) → Setting-Keys, Defaults nur als Startwert.
5. **Cadence**: Reset-Zeiten (Tages-Uhrzeit, Wochen-Wochentag, Zeitzone) + Streak-Toggle → Setting-Keys.
6. **Co-Occurrence-Metrik** (Jaccard/Lift/Konfidenz) → Setting-Key.

> Gültige Defaults werden beim Bau der Engine festgelegt, sind aber **jederzeit im Web-UI änderbar**.

---

## 7. Reihenfolge / Abhängigkeiten

```
T1-1 ─┬─ T1-2, T1-3, T1-4
      └─ (Roles) ─→ T6-2 (Rumpelkiste)
T2-1..3 (unabhängig)
T3-1..3 ─→ T4-1..3 ─→ T5-1..3
T4-1 ─→ T5-2, T6-3 (Engine-Integration)
T3-1 ─→ H1 ─→ H2 ─→ H3 (History; jede Quelle schreibt Runs)
```

Blockt nichts außerhalb: alles baut auf der bestehenden Tag-Schicht auf.
