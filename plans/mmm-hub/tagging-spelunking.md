# Plan: Hub Tagging-System + Spelunking-Engine

**Status**: proposed
**Owner**: next agent
**Branch**: (per leaf issue) `feat/hub-…`
**Depends on**: existing tag layer (`plans/mmm-hub/*`, ADR-073), MMM parent/energy import, group weights + ranked groups
**Migration needed**: yes (several; consolidate per release)

> Companion to [`AGENT.md`](../../mmm-hub/AGENT.md). One issue = one branch = one PR
> (`Closes #<n>`), Conventional-Commit titles, never commit to `main`.
> IDs like `T1-1` are planning-local; GitHub numbers are assigned on creation.

---

## 1. Anforderungen (sortiert)

### A — Tag-Verwaltung & Gruppen-Typen
- **A1** Tag-Seite (`/tags`): **Filter nach Tag-Gruppen** (mehrere gleichzeitig).
- **A2** Tag-Seite: **direkt neue Tags anlegen** (ohne Umweg über eine Playlist).
- **A3** Gruppen bekommen einen **Typ**:
  - *Klassifizierend* (5): **Mood, Vibe, Phase/Energy, Genre, Attribute** (a.k.a. Merkmal).
  - *Sortierend*: **Rumpelkiste, Setlist**.
  - → `hub_tag_groups.kind` (`class` | `sort`), plus semantische Rollen `rumpelkiste`, `setlist`, `genre`, `phase`.
- **A4** **Genre-Sonderfall**: Genre-Tags haben **Variation + Hauptrichtung** (z. B. Variation `Psy` → Haupt `Trance`; Variation `Tech` → Haupt `House`). → als Parent-Relation (Variation = Tag, Hauptrichtung = Parent) im Genre-Kontext, genutzt fürs Scoring/Ähnlichkeit.

### B — Tag-Insights (Tag-Detailseite `/tag/{id}`)
- **B1** **Top-Artists** dieses Tags (Häufigkeit absteigend).
- **B2** **Co-Occurrence-Tags**: welche Tags kommen besonders oft mit diesem Tag zusammen vor — **innerhalb derselben Gruppe** *und* **über Gruppen hinweg** (z. B. `Mood Dark` ↔ `Mood Melancholisch`, `Mood Dark` ↔ `Vibe Warehouse`).
- **B3** **Sprung/Discovery**: von einem Co-Tag direkt auf den Tag springen bzw. ins Digging/Overlap zu den gemeinsamen Tracks.
- **B4** Metric: Co-Occurrence via Jaccard/Lift über `hub_track_resolved_tags`, gruppiert je Gruppe.

### C — Similarity zwischen zwei Tracks
- **C1** **Tag-Overlap-Boost**: mehrere gemeinsame Tags erhöhen die Ähnlichkeit — **stärker, wenn sie in unterschiedlichen Gruppen liegen** (Cross-Group zählt mehr als Same-Group).
- **C2** **Rumpelkiste-Ausnahme**: ist ein Track über eine **Rumpelkiste-Playlist** verknüpft, wird diese Playlist **nicht** für den Ähnlichkeitsvergleich herangezogen — nur **andere** Playlists oder die **Tags**.

### D — Scoring-System
- **D1** **Ripeness Score** ("wie gut sind die Daten *atomar* für diesen Track?"):
  - **Track-Meta**: BPM, Key, Album, Genre, Cover, Artist, Title.
  - **Human-Tags**: Tags in Gruppen.
  - **Traktor-Meta**: play count, last played, rating (none|1-5), Vorkommen in Traktor-Playlists/Collection/Session-History.
  - **Regel: HUMAN TAGS > META TAGS** (Tags wiegen mehr als Metafelder).
- **D2** **Tag-Punkte je Gruppe** (positionsbasiert): 1. Tag = **100**, 2. = **50**, 3. = **25**, 4. = **10**, 5. = **5**, ab 6. = **1**.
  - Die Punkte hängen an der **Gruppe** (Multiset), **nicht** am einzelnen Tag: Löscht man den „100er"-Tag, verschwinden nicht 100, sondern die **niedrigste** Stufe der Gruppe. Beispiel: 2 Vibe-Tags = 150; entfernt man *einen* (egal welchen) → −50 (die „billigste" Stufe).
  - Formel: `group_points(k) = Σ_{i=1..k} w_i` mit `w = [100,50,25,10,5,1,1,…]`.
- **D3** **Similarity Score** (zwischen zwei Tracks) separat vom Ripeness (siehe C).
- **D4** **Schwellwert-Option**: Tracks mit **Score > 500** in der Tag-Aufgabe **auslassen**.

### E — Traktor-Meta-Ingest (Voraussetzung für D1/E)
- **E1** Pro User **`collection.nml`** importieren: play count, last played, rating, Playlists/Collection/Session-History.
- **E2** Speicherung + Views fürs Scoring/„am häufigsten gespielt".

### F — Tägliche/wöchentliche Tag-Aufgabe
- **F1** Ein Track gilt als **vollständig getaggt**, wenn **alle 5 klassifizierenden Gruppen** getaggt sind **oder** der User ihn als **fertig** markiert.
- **F2** Die Aufgabe zeigt **am häufigsten gespielte Traktor-Tracks zuerst**, gemäß Scoring.
- **F3** **Cadence**: täglich **und** wöchentlich (Reset/Rollover, Historie).
- **F4** Option D4 (Score > 500 auslassen) hier anwenden.

### G — Engine-Integration
- **G1** Ripeness + Similarity in **Digging** (Suche/Ähnlichkeit), **Overlap** und **Tag-Insights** nutzen.
- **G2** Faktoren/Schwellen **konfigurierbar im Web-UI** (admin-Settings + pro Collective).

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

| Migration | Inhalt |
| --- | --- |
| `hub_tag_groups.kind` + `role` | `ALTER TABLE hub_tag_groups ADD COLUMN kind TEXT NOT NULL DEFAULT 'class'; ADD COLUMN role TEXT NOT NULL DEFAULT '';` (Rollen: `rumpelkiste`, `setlist`, `genre`, `phase`) |
| `hub_traktor_tracks` | `(user_id, track_id, play_count INT, last_played TEXT, rating INT, imported_at)` PK `(user_id, track_id)` |
| `hub_traktor_playlists` / `_tracks` | Traktor-Playlists + Mitgliedschaft (für „Vorkommen in …") |
| `hub_traktor_sessions` / `_tracks` | Session-History-Vorkommen |
| `hub_track_tag_done` | `(user_id, track_id, marked_at)` – manuelles „fertig getaggt" |
| `hub_tag_tasks` | `(user_id, cadence daily|weekly, period, track_id, status, created_at)` – Aufgabe/Cadence |
| `hub_track_ripeness` (optional, cache) | `(track_id, ripeness REAL, computed_at)` – wenn On-the-fly zu teuer |

Genre Variation/Haupt: **kein** neues Schema — nutzt `hub_tag_parents` (Variation → Haupt) im Genre-Kontext.

---

## 4. Milestones (Vorschlag)

| Milestone | Outcome | Migration |
| --- | --- | --- |
| `hub-tags-0.10.0` | Gruppen-Typen/Rollen, Genre Variation/Haupt, Tag-Seite: Gruppen-Filter + Tags anlegen | 026 |
| `hub-insights-0.11.0` | Tag-Insights: Top-Artists, Co-Occurrence (same- & cross-group), Sprung/Discovery | — |
| `hub-traktor-0.13.0` | Traktor-Meta-Ingest (playcount/rating/sessions) ins Hub | 027 |
| `hub-scoring-0.12.0` | Ripeness-Score (Meta+Tags+Traktor), Tag-Punkte je Gruppe, konfigurierbar | 028 |
| `hub-tasks-0.14.0` | Tägliche/wöchentliche Tag-Aufgabe + „fertig"-Markierung + Score>500-Skip | 029 |
| `hub-sim-0.15.0` | Similarity v2 (Tag-Overlap cross-group, Rumpelkiste-Ausnahme), in Digging/Overlap | — |

> Reihenfolge: `0.10 → 0.11 → 0.13 → 0.12 → 0.14 → 0.15` (Traktor vor Scoring, weil Scoring Traktor-Meta braucht).
> `0.11` und `0.15` können parallel zu `0.13` laufen.

---

## 5. Leaf-Issues (Backlog)

### Milestone `hub-tags-0.10.0`
- **T1-1** `feat(hub): group kind/role columns + backfill` — Migration + `hub_tag_groups.kind/role`, Rumpelkiste/Setlist/Genre/Phase rollen. *(AC: Migration läuft frisch; `kind`/`role` in `list_groups_for`/`group_detail`; Test.)*
- **T1-2** `feat(hub): tag-page group filter (multi)` — `/tags?groups=` mehrfach; server-side. *(AC: Filter kombiniert sich mit q/mine/owner/collective; Test.)*
- **T1-3** `feat(hub): create tag from tag page` — Formular + `POST /tags/create` (Owner = aktueller User). *(AC: Tag erscheint, Group optional; Test.)*
- **T1-4** `feat(hub): genre variation / main direction` — UI zum Setzen (Variation→Parent) + Anzeige; Import aus MMM falls vorhanden. *(AC: Relation persistiert; in Genre-Gruppe sichtbar; Test.)*

### Milestone `hub-insights-0.11.0`
- **T2-1** `feat(hub): tag insights — top artists` — aggregierte Artists je Tag auf `/tag/{id}`. *(AC: sortiert; Test.)*
- **T2-2** `feat(hub): tag co-occurrence (same + cross group)` — Lift/Jaccard je Gruppe; Anzeige „kommt oft mit …". *(AC: Beispiel Mood Dark↔Vibe Warehouse sichtbar; Test.)*
- **T2-3** `feat(hub): jump/discover from co-tag` — Links Tag→Tag sowie → `/overlap?tag=`/`/digging`. *(AC: Links erzeugen korrekte Auswahl; Test.)*

### Milestone `hub-traktor-0.13.0`
- **T3-1** `feat(hub): traktor import CLI (collection.nml)` — Parse + Store playcount/lastplayed/rating. *(AC: Test mit Beispiel-NML.)*
- **T3-2** `feat(hub): traktor playlists + sessions` — Membership + Session-History. *(AC: Views; Test.)*
- **T3-3** `feat(hub): traktor meta views for scoring` — `v_track_traktor(track,playcount,lastplayed,rating,sessions)`. *(AC: View liefert Werte; Test.)*

### Milestone `hub-scoring-0.12.0`
- **T4-1** `feat(hub): ripeness score (meta+tags, human>meta)` — Kern-Algo + `hub_track_ripeness`-Cache/Endpoint. *(AC: `group_points` exakt lt. §2; Unit-Tests inkl. „Löschen entfernt niedrigste Stufe".)*
- **T4-2** `feat(hub): traktor signal in ripeness` — playcount/lastplayed/rating/sessions einfließen. *(AC: Gewicht konfigurierbar; Test.)*
- **T4-3** `feat(hub): scoring settings in UI` — `engine_*`-Keys erweitert (Weights, Threshold 500). *(AC: `/admin` editierbar; Test.)*

### Milestone `hub-tasks-0.14.0`
- **T5-1** `feat(hub): track tagged-complete flag` — `hub_track_tag_done` + „fertig"-Button; Auto-Regel (alle 5 Klassifizierungsgruppen). *(AC: Zustand + Anzeige; Test.)*
- **T5-2** `feat(hub): daily/weekly tagging task` — Queue (Traktor-playcount-first, Score<500), Cadence + Historie. *(AC: tägl./wöchentl. Liste; Test.)*
- **T5-3** `feat(hub): tag-task UI` — Seite/Widget „Tagge jetzt" mit Track + Tag-Eingabe. *(AC: Aktion setzt Tags; Test.)*

### Milestone `hub-sim-0.15.0`
- **T6-1** `feat(hub): similarity v2 — tag overlap cross-group` — Boost je gemeinsamer Gruppe, cross-group stärker. *(AC: Tests mit Fixtures.)*
- **T6-2** `feat(hub): rumpelkiste exception in similarity` — Rumpelkiste-Playlist-Kanten beim Vergleich ignorieren. *(AC: Test: Track nur via Rumpelkiste verknüpft zählt nicht.)*
- **T6-3** `feat(hub): wire sim into digging/overlap` — Ripeness/Similarity in Digging-Score + Overlap-Ranking; konfigurierbar. *(AC: bestehende Tests grün + neue.)*

---

## 6. Offene Entscheidungen (vor Umsetzung klären)

1. **„Attribute" = „Merkmal"?** (§1 A3) — eine Gruppe oder zwei? Vorschlag: identisch, Rollenname `attribute`.
2. **Genre Hauptrichtung**: nur Anzeige oder **zählt als zusätzlicher Tag** im Scoring?
3. **Score > 500**: hart (ausblenden) oder nur Sortierung (nach hinten)?
4. **Meta-Gewichte**: Default-Verhältnis HUMAN:META (Vorschlag 3:1) und Traktor-Anteil.
5. **Traktor-Quelle**: welche `.nml`/Pfade, pro User hochgeladen oder serverseitig gemountet?
6. **Cadence**: Tages-/Wochen-Reset-Zeit (UTC?) + „streak"-Anzeige gewünscht?
7. **Co-Occurrence-Metrik**: Jaccard vs. Lift vs. Konfidenz.

---

## 7. Reihenfolge / Abhängigkeiten

```
T1-1 ─┬─ T1-2, T1-3, T1-4
      └─ (Roles) ─→ T6-2 (Rumpelkiste)
T2-1..3 (unabhängig)
T3-1..3 ─→ T4-1..3 ─→ T5-1..3
T4-1 ─→ T5-2 (Score<500), T6-3 (Engine-Integration)
```

Blockt nichts außerhalb: alles baut auf der bestehenden Tag-Schicht auf.
