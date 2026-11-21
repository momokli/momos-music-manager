# Plan: Comment-Template — Vorschau und ein Renderer

**Status**: proposed (wartet auf Freigabe)
**Branch**: `feat/comment-template-preview`
**Depends on**: #652/#653 (Template-Feld, Export, kellner-Renderer — gemergt in v0.33.0)
**Migration needed**: nein

## Problem (verifiziert 2026-11-21)

Das Template lässt sich seit v0.33.0 pro Gerät setzen (`/kellner` → „Kommentar-Vorlage"),
aber **niemand sieht vorher, was daraus wird**. Man tippt `{tags} {bpm} {key} {energy}`,
drückt Speichern und erfährt das Ergebnis erst, wenn kellner eine Datei schreibt.
`--dry-run` hilft nicht: es zählt nur (`X desired, Y missing`), es rendert nichts.

Dazu kommt eine stille Doppelung der Template-Semantik in zwei Crates:

| | `music-hub/src/device_settings.rs` | `kellner/src/tag/comment.rs` |
|---|---|---|
| erlaubte Platzhalter | `ALLOWED_PLACEHOLDERS` | `ALLOWED_PLACEHOLDERS` |
| Default-String | `DEFAULT_TEMPLATE` | `DEFAULT_TEMPLATE` |
| Token-Deckel | – | `TOKEN_MAX_CHARS = 48` |
| Comment-Deckel | – | `COMMENT_MAX_CHARS = 512` |
| Rendering | – | `render_from_template` |

Die Listen sind heute identisch — aber sie sind **zwei** Wahrheiten. Eine Vorschau, die im
Hub nochmal rendert, wäre die dritte.

## Entscheidung (ADR-085)

**Ein Renderer, zwei Nutzer.** Die Template-Semantik (Platzhalter, Slugging, Deckel,
Fallback, Whitespace-Normalisierung) zieht in eine kleine Crate **`comment-template/`**
(Muster wie `music-api-client`). Hub und kellner hängen sie als path-dependency ein.

- `render(template, &TrackFacts) -> String` — reine Funktion, keine I/O.
- `validate(template) -> Result<(), String>` — die Prüfung, die der Hub beim Speichern nutzt.
- `DEFAULT_TEMPLATE`, `ALLOWED`, `TOKEN_MAX_CHARS`, `COMMENT_MAX_CHARS` leben **nur** hier.
- Golden-Fixture `comment-template/tests/fixtures/cases.json` mit Eingabe→Ausgabe-Paaren;
  **beide** Seiten testen dagegen. Damit kann die Vorschau nicht lügen.

## Vorschau im Hub

Auf `/kellner`, direkt unter dem Feld „Kommentar-Vorlage":

```
Vorschau (10 Tracks aus deinem aktiven Backpack)
  Intro                              build jazzy warehouse BPM-128 KEY-8A ENERGY-7
  I Want Your Soul                   house BPM-124 ENERGY-6
  Folge 459 Kapitel 01               Hard-Techno BPM-138 KEY-11B
  …
  leer = Default: {tags} {bpm} {key} {energy} · 10 von 93 Tracks · Zeichen: 34/512
```

- **Live**: htmx-POST auf `/keller/{device}/template/preview` bei `input changed delay:400ms`.
- **Datenquelle ist der Hub selbst** — keine music-api-Calls: Tags aus
  `hub_track_resolved_tags`, Audio aus `hub_track_audio` (dieselben Abfragen, die der
  Backpack-Export benutzt: `backpack_api.rs:122` und `:137`).
- **Welche 10?** Die ersten 10 Tracks aus der Vereinigung der **aktiven** Backpacks des
  Geräts — also genau die, die kellner als nächstes schreiben würde. Sortierung wie im
  Export (Backpack-Reihenfolge, dann Position).
- **Sichtbar machen, was schiefgehen kann**: Zeichenzähler gegen 512, und ein
  `…`-Marker bleibt sichtbar, wenn gekürzt wurde. Bei leerem Template zeigt die Vorschau
  den Default **und** schreibt ihn dazu.

## Nicht-Ziele

- Keine Tag-Bearbeitung aus der Vorschau (Tags pflegt man im Tag-Layer).
- Kein Template pro Backpack — weiterhin **pro Gerät**.
- Gerenderte Strings werden **nicht** gespeichert; die Vorschau rechnet bei jedem Request.
- Kein neues Feld im Backpack-Export (das Template ist schon drin, #652).
- Keine Änderung am Format selbst (Platzhalter, Slugging, Deckel bleiben).

## Slices

| Slice | Inhalt |
|---|---|
| **C1** | Crate `comment-template/`: `render`, `validate`, Konstanten, Slugging, Deckel, Fallback; Golden-Fixture; `music-hub` **und** `kellner` stellen um (Verhalten unverändert, alle bestehenden Tests bleiben) |
| **C2** | Hub: Route `POST /kellner/{device_id}/template/preview` + htmx-Vorschau auf `/kellner` (10 Tracks, Zeichenzähler, Default-Hinweis) |
| **C3** | kellner: `kellner comment --preview [--limit N]` — gibt die gerenderten Kommentare der Backpack-Tracks lokal aus (offline, ohne Sync) |
| **C4** | Docs: ADR-085, `kellner/AGENT.md` (Renderer-Ort), `music-hub/CHANGELOG.md` |

Reihenfolge: **C1 → C2 → C3 → C4**. C2 und C3 sind ohne C1 nicht sinnvoll (sonst dritte
Implementierung).

## Acceptance Criteria

- [ ] `cargo test` in `comment-template`, `music-hub`, `kellner` grün.
- [ ] Golden-Fixture deckt ab: leeres Template, leerer Tag-Satz, fehlende BPM/Key/Energy,
      Tag mit Leerzeichen (`Hard Techno` → `Hard-Techno`), Tag > 48 Zeichen (am Token
      gekürzt), Ergebnis > 512 Zeichen (`…` am Ende), unbekannter Platzhalter
      (Fallback auf Default), `{{`-Escape.
- [ ] **Vorschau == geschriebene Datei**: derselbe Track durch Crate und durch kellners
      Backpack-Pfad ergibt byte-identische Strings (Test in `kellner/tests/`).
- [ ] Vorschau braucht **keinen** music-api-Token und macht **keinen** Netzwerk-Call.
- [ ] Ohne aktives Backpack: Hinweis statt leerer Liste; ohne Gerät: Seite bleibt wie heute.
- [ ] Zeichenzähler zeigt `n/512`; Kürzung ist in der Vorschau sichtbar.

## Risiken

| Risiko | Umgang |
|---|---|
| Drift trotz Crate (z. B. kellner rendert aus alten Zwischenständen) | Genau ein Aufrufpunkt pro Seite; Test „Vorschau == Datei" |
| Vorschau zeigt andere Tracks als der Sync | Dieselbe Vereinigung wie `kellner backpack` (aktive Backpacks, gleiche Ordnung) |
| Umzug bricht bestehende Dateien | C1 ist verhaltensneutral; die bestehenden 20+ Tests in `kellner/src/tag/comment.rs` wandern mit |
| Neue Crate bläht CI auf | Eigener `cargo test`-Job wie `music-api-client` |
