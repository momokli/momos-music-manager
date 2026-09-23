# UI/UX-Redesign-Proposal — momo's music manager

> Vorschlag, **kein fertiges Redesign**: Ist-Zustand mit Screenshots, UX-Reibungsanalyse, klickbare Mockups und eine priorisierte Roadmap.
> Nur Docs/Mockups/Screenshots — **keine App-Code-Änderungen** in diesem PR.

## Executive Summary

momo's music manager hat eine **solide, token-basierte Dark-UI** mit 26 klar getrennten Views und URL-basierten Filterzuständen — die Reibung liegt nicht im Fundament, sondern in **Dichte, Auffindbarkeit und fehlenden Zwischenzuständen**:

1. **Ergebnisse & Fortschritt sind unsichtbar** — der Traktor-Import zeigt sein Ergebnis nur im Task-Log, die Deemix-Queue zeigt 0 % trotz 12/50 Tracks, Fehlertexte sprengen Zeilen, rohes JSON steht in Task-Zellen.
2. **Der PMV-Roundtrip (Kern-Flow!) ist Detektivarbeit** — Diff als roher Code in einer Megatabelle, „Anwenden“ nur als Alles-oder-nichts, PMV-Syntax ohne Legende.
3. **30 Views ohne Hierarchie** — Digging (will er mehr nutzen) hängt im Tools-Dropdown, Folders (wichtig) 2 Klicks tief, Telemetry (sehr wichtig) ist „Disabled (default)“ ohne Status.

Die **6 Mockups** (klickbar unter [`mockups/`](mockups/)) zeigen das Zielbild je Baustelle; die **Roadmap** sortiert Quick Wins (≤ 1 Tag) vor die Umbauten und ist mit den laufenden PRs (#1 Tag-Roundtrip-Inbox, #2 STEMS-Filter) verzahnt.

## Inhalt

| Dokument | Inhalt |
|---|---|
| [01 · Ist-Zustand](01-ist-zustand.md) | Die App heute entlang der 4 Kern-Flows (Download-Queue → Tag-Kommentar/PMV → Files/Tracks → Traktor-Import/Consistency), 13 Screenshots, Methodik |
| [02 · UX-Reibungsanalyse](02-ux-reibungsanalyse.md) | Nielsen-Heuristiken, Klickpfad-Analysen, Konsistenz-Audit; 11 priorisierte Befunde (R01–R11) mit Belegen |
| [03 · Redesign-Mockups](03-redesign-mockups.md) | 6 klickbare HTML-Mockups (M1–M6) mit Screenshots, Friction-Mapping, Design-Entscheidungen |
| [04 · Roadmap](04-roadmap.md) | Quick Wins → große Umbauten, verknüpft mit Momos Prioritäten (Folders, Telemetry, Tag-Roundtrip, Digging, Tray) |
| [mockups/](mockups/) | Die klickbaren HTML-Dateien (lokal im Browser öffnen) |
| [screenshots/](screenshots/) | Alle PNGs (Ist-Zustand + Mockup-Vorschauen) |

## Wie die Screenshots entstanden sind (Kurzfassung)

App im Debug-Build mit Test-DB (`DATABASE_URL=sqlite:test-playwright.db … serve --no-autoupdate --port 3001`), Testdaten über `POST /api/testing/seed` (Szenarien `basic`, `comment_diff`, `digging`, `wav_variants`, `files_filter`), Deemix-Queue per direkten DB-Inserts, Traktor-Import mit selbst erstellter `collection.nml` über die echte UI. Capture: Chromium/Playwright, Viewport 1440×900 (Retina-Faktor 2), full-page PNG. Details: [01-ist-zustand.md → Methodik](01-ist-zustand.md#wie-die-screenshots-entstanden-sind).

## Schnellster Weg durch das Proposal

1. [01](01-ist-zustand.md) überfliegen (Screenshots sagen mehr als Text) →
2. [02](02-ux-reibungsanalyse.md) Tabelle „Befund-Übersicht“ →
3. [03](03-redesign-mockups.md) Mockup-Bilder ansehen, je 1–2 klickbare Mockups lokal öffnen →
4. [04](04-roadmap.md) Phasen 0–1 lesen (Quick Wins + PMV-Review).
