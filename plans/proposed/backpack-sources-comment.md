# Plan: Backpacks bedienbar machen + Comment-Template

**Status**: approved (Entscheidung 2026-11-20, ADR-084)
**Branch**: `feat/backpack-sources`
**Ready for review**: no
**Depends on**: nichts (nutzt `hub_backpack_items` aus Migration 044 und den kellner-Backpack-Sync aus #367)
**Migration needed**: ja — additiv (`max_tracks` an `hub_backpack_items`, Default-Template in `hub_device_settings`)

## Problem (verifiziert 2026-11-20)

Der Weg „Backpack definieren → Mac synchronisieren" ist **fast** fertig, aber an der
entscheidenden Stelle nicht bedienbar:

| Schritt | Ist |
|---|---|
| Backpack anlegen/umbenennen/löschen/aktivieren | ✅ `pages.rs:29-32` |
| **Quellen hinzufügen** (Tag/Playlist/Lens/Track) | ❌ `backpacks::add_source` / `remove_source` / `clear_sources` existieren (`backpacks.rs:104/263/285`), haben **null Aufrufer** — keine Route, kein Formular |
| Smart Lens | ⚠️ `curate::LENSES` = `newest`, `liked`, `unheard` — **ohne Parameter** („neueste 100" nicht sagbar) |
| Backpack dem Gerät zuweisen | ✅ `/kellner`, `hub_device_backpacks` |
| Sync in einen Ordner | ✅ `kellner backpack`: Union aller aktiven Backpacks per ISRC, flach nach `collection_dir` |
| Comment schreiben | ⚠️ fester Code (`render_comment`), `hub_device_settings.comment_template` existiert, wird gespeichert/validiert — aber **nicht exportiert** und von kellner **nicht gelesen** |
| Re-Tagging vorhandener Dateien | ❌ nur frisch geladene Dateien werden getaggt (`backpack.rs:156` filtert vorhandene Dateien aus) |

## Entscheidungen (ADR-084)

### 1 · Ein Zielordner für alle Backpacks — ergänzend, nie löschend

- Alle aktiven Backpacks werden **per ISRC vereinigt** (`desired`), geschrieben flach
  als `<isrc>.<ext>` in **einen** Zielordner. Zwei Backpacks mit derselben ISRC
  kollidieren nicht, sie ergänzen sich.
- Aufräumen: was in **keinem** aktiven Backpack mehr steckt, wandert nach
  `<root>/_trash` (Soft-Prune, bereits implementiert: `backpack.rs:219`). Dateien
  werden **nie** gelöscht.
- Nicht von kellner verwaltete Dateien im Ordner bleiben unangetastet (Prune arbeitet
  nur über `state.json`-Mitgliedschaft).

### 2 · Comment-Template mit Platzhaltern

Der Comment wird pro Gerät aus einem Template gerendert (`comment_template` in
`hub_device_settings`, heute tot). Whitelist:

| Platzhalter | Rendert | Beispiel |
|---|---|---|
| `{tags}` | aktive Tag-Namen, alphabetisch, slugged, mit Leerzeichen getrennt | `build jazzy warehouse` |
| `{bpm}` | `BPM-<ganzzahl>` | `BPM-128` |
| `{key}` | `KEY-<camelot>` | `KEY-8A` |
| `{energy}` | `ENERGY-<0..10>` | `ENERGY-7` |

Regeln:

- **Kein Leerzeichen innerhalb eines Tokens.** Trennzeichen im Template sind
  gewollt (Leerzeichen zwischen Tags), Werte sind immer ein Token: `BPM-128`,
  nicht `BPM 128`.
- **Tag-Namen werden geslugged**, weil Leerzeichen das Trennsymbol sind: jedes Zeichen
  außer `[A-Za-z0-9_-]` wird zu `-`, Folgen zusammengefasst, Ränder getrimmt.
  Empirischer Grund: **166 von 609** Tags im Produktions-Hub enthalten Leerzeichen,
  die längsten sind abgeleitete Playlist-Namen
  (`Scratchytunes 2013 : Flight facilities, Metronomy, … & more`).
- **Token-Deckel 48 Zeichen, Comment-Deckel 512 Zeichen** (mit `…`-Marker). Sonst
  kann ein einziger Playlist-Name das Comment-Feld sprengen (ID3-Kommentar, MP4-
  Freeform). Gekürzt wird *am Token*, nicht mitten im Template.
- Leere Platzhalter werden **entfernt**, danach werden Mehrfach-Leerzeichen
  zusammengefasst und Ränder getrimmt — kein `double  space`, kein hängendes `|`.
- **Default** (wenn kein Template gesetzt ist): `{tags} {bpm} {key} {energy}`. Das
  reproduziert den heutigen Inhalt in der neuen Form.
- Unbekannter Platzhalter → **400 beim Speichern im Hub** (Meldung listet die
  erlaubten). kellner fällt bei unbekanntem Platzhalter auf den Default zurück und
  warnt, statt zu scheitern.

### 3 · Kellner taggt auch vorhandene Dateien

Der `missing`-Filter entscheidet nur, was **geladen** wird. Das Tagging läuft danach
über **alle** `desired`-ISRCs: liegt die Datei schon flach im Zielordner, wird nur der
Comment geprüft und bei Abweichung geschrieben (`write_comment` schreibt bereits nur
bei Änderung — mtime bleibt unberührt).

## Slices

| Slice | Inhalt |
|---|---|
| **B1** | Hub: Quellen bedienbar — Routen `POST /backpacks/{id}/sources` (tag/playlist/lens/track), `.../sources/{item}/delete`, `.../clear`, Formulare in `backpacks.html`, Tests für alle vier Arten + Eigentumsprüfung |
| **B2** | Hub: Template wirklich verdrahten — Platzhalter-Validierung beim Speichern, Default setzen, `comment_template` im Device-Export-Payload mitliefern, Tests |
| **B3** | kellner: Renderer mit Platzhaltern, Slugging, Deckeln, Fallback; Re-Tagging vorhandener Dateien; Tests (leere Werte, kein Doppel-Space, Umlaute, langer Tag) |
| **B4** | Hub: Lens-Limit — `max_tracks` (NULL = unbegrenzt) an `hub_backpack_items`, im Resolver anwenden, im Formular setzbar |
| **B5** | Docs: ADR-084, `kellner/AGENT.md` (Zielordner-Semantik, Template), `plans/README.md` |

Reihenfolge: **B1 → B2 → B3 → B4** (B3 braucht das Feld aus B2 im Export).

## Acceptance Criteria

- [ ] In der Hub-UI lässt sich ein Backpack mit allen vier Quellenarten befüllen und wieder leeren; fremde Backpacks sind nicht erreichbar (Test: 404).
- [ ] Ein Backpack mit Tag + Playlist + Lens löst zu einer korrekten ISRC-Menge auf (Test mit fixierten Daten).
- [ ] `GET /api/hub/backpacks/{id}/export` liefert `comment_template` mit.
- [ ] Unbekannter Platzhalter → 400 im Hub, Warnung + Default in kellner.
- [ ] `{bpm}` rendert `BPM-128`; ein Tag `Hard Techno` rendert `Hard-Techno`; nie ein Leerzeichen im Token.
- [ ] Leere Werte erzeugen weder Doppel-Leerzeichen noch hängende Separatoren.
- [ ] Comment > 512 Zeichen wird mit `…` gekürzt, Datei bleibt lesbar.
- [ ] `kellner backpack` taggt auch eine bereits vorhandene Datei neu, wenn das gerenderte Comment abweicht; unveränderte Dateien behalten ihre mtime.
- [ ] Prune verschiebt nach `<root>/_trash`, löscht nie; Dateien außerhalb der Backpack-Mitgliedschaft bleiben liegen.
- [ ] `cargo test` in `music-hub` und `kellner` grün.

## Nicht-Ziele

- Kein Layout jenseits von flach (`<isrc>.<ext>`) — Design-Entscheidung 2026-11-20.
- Keine automatische Übernahme von `collection.nml`-Pfaden aus dem Hub in kellner.
- Kein neuer „Library-Export"-Endpoint: Backpacks **sind** der Export (siehe ADR-084).
- Keine Änderung an der `[PMV]`-Legacy-Formatfamilie; `docs/COMMENT_SYSTEM.md` bleibt MMM-Historie.
