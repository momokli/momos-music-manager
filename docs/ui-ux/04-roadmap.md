# 04 · Roadmap — Quick Wins zuerst, Umbauten danach

> Prioritäten-Bezug (aus dem Auftrag): **Folders** (basic, wichtig) · **Telemetry** (sehr wichtig) ·
> **Tag-Roundtrip-Inbox** (PR #1 läuft) · **Digging** (will er mehr nutzen) · **Battery-friendly Tray** (wichtig).
> Offene PRs im Repo: **#1** feat: tag roundtrip inbox · **#2** feat(files): STEMS filter.
> Aufwände: S < 1 Tag · M 1–3 Tage · L > 3 Tage (inkl. Frontend-Tests). Alle Punkte sind UI-/Doku-Arbeit; Backend-APIs existieren für die meisten bereits.

## Phase 0 · Quick Wins (je ≤ 1 Tag, sofortiger Sichtnutzen)

| # | Maßnahme | Behebt | Aufwand | Mockup/Beleg |
|---|----------|--------|---------|--------------|
| Q1 | **Folders in die Top-Nav** (unter Library) + „+ Ordner“-Sprung von Dashboard/Settings | R10; Folders ist die wichtigste „basic“-Funktion und heute 2 Klicks tief | S | M1/M6 |
| Q2 | **Task-/Import-Ergebnis auf der Traktor-Import-Seite** (Summary-Chips aus `ImportStats`, „ohne Match“-Liste) | R01/R07 | S–M | M5 |
| Q3 | **Manual-Path-Feld: sichtbarer „Übernehmen“-Button + Hinweis** (Enter bleibt als Shortcut) | R06 (real reproduzierter Fehler) | S | M5 |
| Q4 | **Deemix-Queue: Fehler aus der Zeile lösen** — Fehlertext als eigene Zeile unter der betroffenen Playlist, Retry-Button daneben | R04 | S | M3 |
| Q5 | **„—“-Semantik bereinigen** (nicht gesetzt vs. unbekannt) + „Last Played“-Spaltenbreite fixen | R08/R11 | S | 01 F3 |
| Q6 | **Telemetry-Status sichtbar machen** („● aktiv, Push vor X min“, „Jetzt pushen“-Knopf in Settings + dezenter Status in der Nav) — nutzt vorhandene Telemetry-API | Telemetry-Priorität; heute nur „Disabled (default)“ | S–M | M6 |
| Q7 | **Tray-Kontext-UI** (Doku + kleiner Einstieg): Tray-Menü zeigt Queue-/Task-Kurzstatus, damit der Laptop im Tray-Betrieb ohne UI-Öffnen informiert (Battery-friendly: kein dauerhaftes Web-UI nötig) | Battery-friendly Tray | M | — (Desktop, außerhalb Web-UI) |

## Phase 1 · PMV-Review-Flow (parallel zu PR #1)

| # | Maßnahme | Behebt | Aufwand | Mockup |
|---|----------|--------|---------|--------|
| P1 | **Review-Modus in Files** (Segment Browse/Review/Ungelinkt); Review-Zeilen mit lesbarem Diff + Klartext-Hinweis (Kategorie-/Service-Chips) | R02 | M | M2 |
| P2 | **Per-Zeile-Anwenden + Bulk-Bar** („N gewählte schreiben“) als Ersatz für Sidebar-„WRITE COMMENTS“-Alles-oder-nichts | R03 | M | M2 |
| P3 | **PMV-Syntax-Legende** in der UI (Tooltip/Info-Karte) statt nur `docs/COMMENT_SYSTEM.md` | R02 | S | M2 |
| — | **Abstimmung mit PR #1:** Wenn die Tag-Roundtrip-Inbox (Inbox-View) merge-t, P1/P2 dort andocken statt doppelt in Files bauen; Files behält dann den Browse-Modus schlank (R09). | — | — | — |

## Phase 2 · Digging zugänglich machen

| # | Maßnahme | Behebt | Aufwand | Mockup |
|---|----------|--------|---------|--------|
| D1 | „+ Ladder“-Button auf Track-Karten (DnD bleibt), Karten-Hover klarer | R05 | S | M4 |
| D2 | Waveform-Platzhalter als echte Preview-Optik + Play-Affordanz (ohne Audio-Engine: aus `durationMs`/BPM generiertes Muster) | R05 | S–M | M4 |
| D3 | Ladder-Summary (BPM-Spanne, Key-Kompatibilität) + Session-Continuity (letzte Session beim Öffnen anbieten) | R05; „mehr nutzen“-Ziel | M | M4 |
| D4 | BPM-/Key-basierte **Vorschlags-Karte** („passt zu deiner Ladder“) — aufbauend auf vorhandener Digging-Filterung | R05 | M–L | M4 |

## Phase 3 · Größere Umbauten (Fundament)

| # | Maßnahme | Behebt | Aufwand | Mockup |
|---|----------|--------|---------|--------|
| U1 | **Deemix-Queue als Status-Gruppen-Ansicht** statt 12-Spalten-Tabelle (Gruppen Aktiv/Fehler/Wartend/Fertig + Summary-Chips + Progress-Balken konsistent zu `downloaded/total`) | R04 | M | M3 |
| U2 | **Consistency-Check nach Traktor-Import** (BPM-Drift, Key-Konflikte, fehlende Ratings; Liste + CSV-Export) — macht Kern-Flow F4 zu einem echten Werkzeug | R01 | M | M5 |
| U3 | **Shell-Veredelung** (Verlauf-Primaries, Schatten, Radien, Fokus-Ringe, einheitliche Badge-/Chip-/Progress-Komponenten als CSS-Tokens) + globaler Aktivitäts-Chip | R08–R11 | M | M1 |
| U4 | **Dashboard als Fokusliste** („Heute“-Karte mit echten nächsten Schritten, Statistik-Füße mit Sprungzielen) | R10 | M | M1 |
| U5 | **Globales Komponenten-Vokabular** (Empty-States, Fehler-Banner, „—“-Regeln) quer durch alle 26 Views; Navigationsebenen evaluieren (Suche als späterer Schritt) | R10/R11 | L | M1/M6 |

## Reihenfolge-Empfehlung

1. **Q1–Q6** (eine UI-Woche, viel Sichtbarkeit — inkl. Telemetry-Status als Signal an Momos wichtigstes Thema).
2. **P1–P3** abgestimmt mit PR #1 (Review-Flow ist der wertvollste Workflow; nicht doppelt bauen).
3. **D1–D3** (Digging-Nutzung steigern) und **Q7** (Tray-Status, battery-friendly).
4. **U1–U4** als kontinuierliche Veredelung; **U5** als Hüter der Konsistenz bei jeder neuen View.

Jeder Umbau behält die bestehenden Design-Tokens (`style.css` `:root`) und das URL-basierte Filter-/Ansichts-Modell (Hash-Parameter) — die Mockups M1–M6 zeigen, wie das Ergebnis aussieht, ohne die Architektur anzufassen.

**Zurück:** [03 · Redesign-Mockups](03-redesign-mockups.md) · [02 · Reibungsanalyse](02-ux-reibungsanalyse.md) · [Start](README.md)
