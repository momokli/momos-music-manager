# Plan: Comment-Vorlagen — Bibliothek im Hub, Vorschau, ein Renderer

**Status**: proposed (wartet auf Freigabe)
**Branch**: `feat/comment-templates`
**Depends on**: #652/#653 (Template-Feld, Export, kellner-Renderer — in v0.33.0)
**Migration needed**: ja — additiv (`hub_comment_templates`, `hub_device_settings.comment_template_id`)

## Problem (verifiziert 2026-11-21)

Das Template ist heute **ein Freitextfeld pro Gerät** (`hub_device_settings.comment_template`,
Migration 052). Drei Folgen, die alle drei weh tun:

1. **Nichts ist wiederverwendbar.** Wer zwei Kellner hat, tippt den String zweimal.
2. **Nichts ist teilbar.** „Meine Vorlage" gibt es nicht — nur „die Vorlage dieses Geräts".
3. **Niemand sieht das Ergebnis.** Man tippt `{tags} {bpm} {key} {energy}`, speichert und
   erfährt erst beim nächsten Sync, was daraus wird (`--dry-run` zählt nur).

Dazu eine stille Doppelung der Semantik in zwei Crates: `ALLOWED_PLACEHOLDERS` und
`DEFAULT_TEMPLATE` stehen in `music-hub/src/device_settings.rs` **und** in
`kellner/src/tag/comment.rs`; Slugging, Deckel (48/512) und Fallback nur in kellner.

## Zielbild

Vorlagen sind **benannte Objekte im Hub**, nicht Strings in einem Formular:

```
Vorlagen-Bibliothek
├── System          „Standard"        {tags} {bpm} {key} {energy}     (read-only)
│                   „Nur Tags"        {tags}                          (read-only)
├── Geteilt         „momo: Traktor"   {tags} {bpm} {key}              (nutzbar für alle)
└── Meine           „Tonies kurz"     {tags}                          (privat)
                                   ↓ auswählen
                              Gerät (Kellner)
                                   ↓ beim Sync
                         Datei-Comment (gerendert)
```

Ein Gerät **referenziert** eine Vorlage. Auflösung passiert an genau einer Stelle:

1. `comment_template_id` → Body der Vorlage (wenn die Vorlage für den User sichtbar ist)
2. Legacy: `hub_device_settings.comment_template` (Freitext aus v0.33.0), wenn nicht leer
3. Built-in `DEFAULT_TEMPLATE`

Punkt 2 ist die Kompatibilitätszeile: bestehende Geräte funktionieren unverändert weiter.

## Datenmodell (additiv)

```sql
CREATE TABLE hub_comment_templates (
    id            INTEGER PRIMARY KEY,
    owner_user_id INTEGER REFERENCES hub_users(id) ON DELETE CASCADE,  -- NULL = System
    name          TEXT NOT NULL,
    body          TEXT NOT NULL,
    is_shared     INTEGER NOT NULL DEFAULT 0,     -- sichtbar für alle User
    created_at    TEXT NOT NULL,
    updated_at    TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_comment_templates_owner_name
    ON hub_comment_templates(owner_user_id, name COLLATE NOCASE);
CREATE UNIQUE INDEX idx_comment_templates_system_name
    ON hub_comment_templates(name COLLATE NOCASE) WHERE owner_user_id IS NULL;

ALTER TABLE hub_device_settings
    ADD COLUMN comment_template_id INTEGER REFERENCES hub_comment_templates(id) ON DELETE SET NULL;
```

Der partielle Index ist nötig, weil SQLite `NULL` in Unique-Indizes als „verschieden"
behandelt — ohne ihn ließen sich zwei System-Vorlagen „Standard" anlegen.

## Rechte und Teilen

| Vorlage                          | sehen | nutzen | ändern                               |
| -------------------------------- | ----- | ------ | ------------------------------------ |
| eigene                           | ✓     | ✓      | ✓ (auch umbenennen, teilen, löschen) |
| `is_shared = 1` von anderen      | ✓     | ✓      | ✗ — aber „Kopie erstellen"           |
| System (`owner_user_id IS NULL`) | ✓     | ✓      | ✗ (Admin darf)                       |

**Referenz statt Kopie** ist der Default: ändert der Owner eine geteilte Vorlage, greift das
beim nächsten Sync bei allen, die sie nutzen. Das ist gewollt (Kuratieren) und ungefährlich,
weil der Export immer den **aufgelösten Body** mitschickt — ein laufender Sync wird nie
halb umgestellt. Wer seine Variante will, nimmt „Kopie erstellen".

## Vorschau

Auf `/kellner` unter der Vorlagen-Auswahl und auf `/comment-templates/{id}` beim Bearbeiten:

```
Vorschau (10 Tracks aus deinem aktiven Backpack)
  Intro                        build jazzy warehouse BPM-128 KEY-8A ENERGY-7
  I Want Your Soul             house BPM-124 ENERGY-6
  Folge 459 Kapitel 01         Hard-Techno BPM-138 KEY-11B
  · 10 von 93 Tracks · 34/512 Zeichen · Kürzung sichtbar als …
```

- Live (htmx, 400 ms Delay), Daten **nur** aus dem Hub: `hub_track_resolved_tags` +
  `hub_track_audio` (dieselben Abfragen wie der Backpack-Export, `backpack_api.rs:122/137`).
- 10 Tracks = Vereinigung der **aktiven** Backpacks des Geräts, in Export-Reihenfolge.
- Optional: Vergleich zweier Vorlagen nebeneinander (zwei Spalten) — nice-to-have, nicht kritisch.

## Ein Renderer

Die Semantik (Platzhalter, Slugging, Deckel 48/512, Whitespace, Fallback) zieht in eine
Crate **`comment-template/`** (Muster `music-api-client`). Hub (Vorschau, Validierung) und
kellner (Schreiben) benutzen sie. Golden-Fixture `comment-template/tests/fixtures/cases.json`
ist der Vertrag zwischen beiden, plus ein Test „Vorschau == geschriebene Datei".

## Nicht-Ziele

- Kein Template **pro Backpack** — die Vorlage hängt am Gerät.
- Keine Tag-Bearbeitung aus der Vorschau.
- Kein Speichern gerenderter Strings; die Vorschau rechnet je Request.
- Kein neues Feld im Export außer dem aufgelösten Body (+ Name fürs Log).
- Keine Änderung am Format selbst (Platzhalter, Slugging, Deckel).
- **tonies-hub** (eigener Dienst, eigene DB) bekommt seine Bibliothek später selbst — hier
  nicht mitbauen.

## Slices

| Slice  | Inhalt                                                                                                                                                                              |
| ------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **C1** | Crate `comment-template/`: `render`, `validate`, Konstanten, Slugging, Deckel, Fallback; Golden-Fixture; `music-hub` **und** `kellner` stellen um (verhaltensneutral)               |
| **C2** | Hub: Bibliothek — Migration, `hub_comment_templates` CRUD (Seite `/comment-templates`), Sichtbarkeit (eigen/geteilt/System), Teilen, Kopie, Löschen; System-Vorlagen seeden         |
| **C3** | Hub: Gerät referenziert Vorlage — `comment_template_id`, Auflösungskette (Vorlage → Legacy-Freitext → Default), Auswahl im `/kellner`-Formular, Export liefert den aufgelösten Body |
| **C4** | Hub: Vorschau (10 Tracks) auf `/kellner` und auf der Vorlagen-Seite                                                                                                                 |
| **C5** | kellner: `comment --preview [--limit N]` — dieselbe Sicht offline am Mac                                                                                                            |
| **C6** | Docs: ADR-085, `kellner/AGENT.md`, `music-hub/CHANGELOG.md` (dieser PR)                                                                                                             |

Reihenfolge: **C1 → C2 → C3 → C4 → C5**. C4 ohne C1 wäre die dritte Implementierung, C3 ohne
C2 hätte nichts zum Referenzieren.

## Acceptance Criteria

- [ ] `cargo test` in `comment-template`, `music-hub`, `kellner` grün.
- [ ] Golden-Fixture deckt ab: leeres Template, leerer Tag-Satz, fehlende BPM/Key/Energy,
      `Hard Techno` → `Hard-Techno`, Token > 48 Zeichen (am Token gekürzt), Ergebnis > 512
      (`…` am Ende), unbekannter Platzhalter (Fallback), `{{`-Escape.
- [ ] Vorlagen-CRUD: anlegen, umbenennen, teilen, teilen aufheben, Kopie, löschen — je ein Test.
- [ ] Fremde, nicht geteilte Vorlage ist nicht sichtbar (404 / nicht in der Liste) — Test.
- [ ] Geteilte Vorlage ist für andere User nutzbar (Gerät auswählen), aber nicht änderbar — Test.
- [ ] Auflösungskette: Vorlage gesetzt → ihr Body; Vorlage gelöscht → Legacy-Freitext; beides
      leer → `DEFAULT_TEMPLATE` (drei Tests).
- [ ] Bestehende Geräte mit Freitext aus v0.33.0 funktionieren unverändert (Regressionstest).
- [ ] Vorschau macht **keinen** Netzwerk-Call und braucht **keinen** `MUSIC_API_TOKEN` (Test
      mit Mock, der jeden Request scheitern lässt).
- [ ] Vorschau == geschriebene Datei (byte-identisch, Test in `kellner/tests/`).
- [ ] System-Vorlagen sind schreibgeschützt (Test: 403) und bestehen `validate` (Test).
- [ ] Ohne aktives Backpack: Hinweis statt leerer Vorschau; ohne Gerät: `/kellner` wie heute.

## Risiken

| Risiko                                        | Umgang                                                                                 |
| --------------------------------------------- | -------------------------------------------------------------------------------------- |
| Geteilte Vorlage ändert rückwirkend bei allen | Gewollt; Export enthält den aufgelösten Body, Sync bleibt konsistent. Kopie als Ausweg |
| Owner löscht geteilte, genutzte Vorlage       | `ON DELETE SET NULL` → Gerät fällt auf Legacy/Default zurück, kein kaputter Zustand    |
| Drift zwischen Vorschau und Datei             | Eine Crate + „Vorschau == Datei"-Test                                                  |
| Namens-Kollisionen zwischen Usern             | Unique nur pro Owner; Anzeige mit Owner-Präfix (`momo: Traktor`)                       |
| Umzug der Semantik bricht bestehende Dateien  | C1 ist ein Move; bestehende Tests wandern mit                                          |
| Neue Crate bläht CI auf                       | Eigener `cargo test`-Job wie `music-api-client`                                        |
