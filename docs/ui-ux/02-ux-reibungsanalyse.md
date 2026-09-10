# 02 · UX-Reibungsanalyse

> Methodik: Nielsen-Heuristiken pro Kern-Flow, Klickpfad-Analysen („Schritte bis zum Ziel“), visueller Konsistenz-Audit.
> Jeder Befund hat einen Screenshot-Beleg aus [01-ist-zustand.md](01-ist-zustand.md); Impact/Aufwand sind grobe Schätzwerte für die [Roadmap](04-roadmap.md).

## Befund-Übersicht (priorisiert)

| ID | Befund | Flow | Heuristik | Impact | Aufwand | Screenshot-Beleg |
|----|--------|------|-----------|--------|---------|------------------|
| R01 | Import-Ergebnis/Consistency nirgends sichtbar (nur Task-Log) | F4 | Visibility of system status | hoch | S | [traktor-imported](01-ist-zustand.md#nach-erfolgreichem-import) |
| R02 | Kommentar-Diff nur als roher Code inline in der Tabelle; kein Review-dann-Anwenden-Schritt | F2 | Recognition / User control | hoch | M | [files-comment-diff](01-ist-zustand.md#files-liste-mit-diff) |
| R03 | „Write“-Aktion an 3 Orten mit 3 Granularitäten, kein Bulk-Review | F2 | Consistency | hoch | S | [files-comment-diff](01-ist-zustand.md#files-liste-mit-diff) |
| R04 | Deemix: Fortschritt widersprüchlich (Progress 0 % vs. 12/50), Fehlertext sprengt Zeile, 12 Spalten | F1 | Visibility / Aesthetics | mittel | M | [deemix-queue](01-ist-zustand.md#f1-download-queue-deemix) |
| R05 | Digging: nur Drag & Drop in die Ladder; Waveform-Platzhalter; unter Tools versteckt | F2 | Affordance / Findability | mittel | M | [digging](01-ist-zustand.md#digging-curator) |
| R06 | Manueller Traktor-Pfad wird nur per Enter gespeichert (unsichtbarer Trigger) | F4 | Error prevention | mittel | S | [traktor-manual](01-ist-zustand.md#manueller-modus) |
| R07 | Fehlgeschlagene Tasks zeigen rohes JSON (`{"custom_path":null}`), 0 %-Balken auch bei Completed | F4 | Visibility / Aesthetics | mittel | S | [tasks](01-ist-zustand.md#f4-traktor-import-consistency) |
| R08 | „Alarm“-Rot ohne Alarm („On Disk ✗ No“ bei vorhandener Datei) | F2/F3 | Aesthetics, falsche Affordanz | niedrig | S | [file-detail-40](01-ist-zustand.md#file-detail) |
| R09 | Files-Filterpanel sehr hoch & default offen; zwei Filter-Muster (Files vs. Tracks) | F3 | Consistency / Density | mittel | M | [files-browse](01-ist-zustand.md#files-ungefiltert) |
| R10 | 30 Views, Tools-Dropdown mit 10 Einträgen — Hauptwege 2 Klicks tief, keine Suche | alle | Recognition / Navigation | mittel | L | [dashboard](01-ist-zustand.md#kontext-screens) |
| R11 | Status-Vokabular driftet („running/pending“, Badge vs. Chip vs. Punkt; „—“ überall) | alle | Consistency | niedrig | S | [tasks](01-ist-zustand.md#f4-traktor-import-consistency) |

Legende Impact: hoch = blockiert/täuscht den Kern-Workflow · mittel = kostet regelmäßig Zeit oder Verständnis · niedrig = kosmetisch/vereinzelt.
Aufwand: S < 1 Tag · M 1–3 Tage · L > 3 Tage (jeweils inkl. Frontend-Tests).

---

## F1 · Download-Queue

### Heuristik-Check

- **Sichtbarkeit des Systemstatus (schwach):** Der einzige echte Fortschritt steckt in `downloaded/total` (12/50); die Fortschritts-Spalte zeigt dennoch 0 %. Zwei konkurrierende Metriken pro Zeile, keine davon als Balken visualisiert. Bei mehreren parallelen Downloads (im Test 2 downloading, 2 queued) gibt es keine Gesamtlage („2 aktiv, 1 Fehler, 2 warten“), keine ETA.
- **Fehlertoleranz (mittel):** Fehlermeldungen erscheinen als Roh-Text in der Detail-Spalte und kippen die Zeilenhöhe; Retry ist ein unbenannter Icon-Button. Kein „Alle fehlgeschlagenen erneut versuchen“, kein Verlauf „was ist wann schiefgelaufen“.
- **Ästhetik & Minimalismus (schwach):** 12 Spalten, von denen zwei (Title/Artist) ohne verbundenen Deemix-Server nur „—“ sind. Header-Labels brechen um („Downloaded“), die Tabelle scrollt horizontal an ihrer Grenze.
- **Erkennung statt Erinnerung (mittel):** Status-Badges sind ok, aber die Filter (Queued/Downloading/…) liegen hinter dem einklappbaren Filterpanel — der Standardpfad „was läuft gerade?“ braucht erst einen Klick auf den Filter-Toggle.

### Klickpfad heute

| Ziel | Schritte heute | Bewertung |
|------|----------------|-----------|
| „Was läuft gerade / ist fehlgeschlagen?“ | Services ▸ Deemix Queue → Filterpanel öffnen → Status wählen → Zeile scannen | 3+ Schritte, Status nicht gruppiert |
| Fehlgeschlagenen Download erneut versuchen | Zeile finden → Icon-Button Retry (unbeschriftet) | ok, aber keine Bulk-Aktion, kein Feedback-Pfad bei erneutem Fehlschlag |

### Top-Friction F1

Die Queue zeigt **Status, aber keinen Zustand**: keine Fortschrittsbalken, keine Gruppierung nach Status, keine Zusammenfassung, rohe Fehlertexte. Als „Daily-Driver“-Screen (Downloads laufen dauerhaft im Tray-Betrieb) fehlt die Blick-erfassende Struktur.

---

## F2 · Tag-Kommentar / PMV-Roundtrip

### Heuristik-Check

- **Erkennung statt Erinnerung (schwach):** Das Zielformat `[_M_] groovy sp:spotify:track:…` ist eine Fachsprache ohne Legende. „Needs Update“ sagt nicht, *warum* (Kommentar fehlt? Target geändert? nur Backup?). Der Diff ist technisch korrekt, aber nicht lesbar gemacht (kein Klartext „PMV-Kategorie: Groovy · verknüpft mit Spotify-Track“, kein Hervorheben *einer* geänderten Stelle).
- **Benutzerkontrolle & Freiheit (schwach):** Anwenden ist Alles-oder-nichts über die Sidebar („WRITE COMMENTS“) oder einzeln per Stift-Icon mitten in der Tabelle. Der natürliche Review-Flow „Diff ansehen → einzeln akzeptieren → Rest bulk“ existiert nicht; versehentlicher Massen-Write ist möglich.
- **Konsistenz (schwach):** „Write comments“ existiert als Stift pro Zeile (Files), Sidebar-Button (Files), Karte (Dashboard) — drei Orte, drei Skopoi, gleiche Aktion. Comment-Status erscheint als Diff-Spalte, Badges (Dashboard) und Filter-Chips (Files) mit unterschiedlichen Vokabeln (needs_update / Needs Update / Comment Diffs).
- **Ästhetik (schwach):** Die Diff-Zeilen (2 pro Datei, rot/grün, Monospace-Code) blähen die Tabelle; die Spalte mischt Diff-Code, „Backup only“-Text und Badges. Die Sidebar-Primaries (Indigo/Violett) konkurrieren mit dem Tabelleninhalt.

### Klickpfad heute

| Ziel | Schritte heute | Bewertung |
|------|----------------|-----------|
| „Welche Dateien brauchen einen neuen PMV-Kommentar?“ | Files → Filterpanel → Comment: Needs Update + Local: Yes | 3–4 Klicks, Panel hoch |
| „Warum genau? Was ist anders?“ | Zeile lesen (Code-Diff, keine Erklärung) | unklar ohne PMV-Wissen |
| „Einzeln reviewen & anwenden“ | geht nicht granular; nur Stift/Alles | Lücke → R02/R03 |
| Neue PMV-Kategorie vergeben | Tags/Tag Categories (eigene Views) | kein Kontext-Bezug zur Datei |

### Top-Friction F2

Der **PMV-Roundtrip ist der wertvollste Workflow der App** (PR #1 baut die Inbox dazu) — aber der Ist-Zustand macht Review zur Detektivarbeit: roher Code-Diff in einer überladenen Tabelle, kein „Akzeptieren“-Zwischenschritt, drei Write-Orte. Genau hier setzt Mockup [M2](03-redesign-mockups.md#m2-files-pmv-review-diff-verstehen-einzeln-anwenden) an.

---

## F3 · Files / Tracks-Browse

### Heuristik-Check

- **Konsistenz (mittel):** Files nutzt ein horizontales Filterpanel über der Tabelle, Tracks eine vertikale Filter-Sidebar links; Controls wechseln zwischen Chips, Segment-Buttons, Slider, Key-Grid. Jede CRUD-Seite ist ein leicht anderes CRUD — Muster-Drift bei gleicher Aufgabe (filtern/sortieren).
- **Ästhetik & Dichte (mittel):** Files zeigt 15+ Spalten; „Last Played“ bricht um, Diff-Zeilen verdoppeln Zeilenhöhen, Prune-Icons rot ohne Label. Detailseiten zeigen viele „—“-Felder (ehrlich, aber rauschig — besser weglassen oder kompakt als „nicht gesetzt“-Gruppe).
- **Flexibilität & Effizienz (gut):** Filter-/Sortier-/Paginierungs-Zustand steckt in URL-Hash-Parametern (`#files?commentStatuses=…`) — teilbar und bookmarkbar. Das ist ein starkes Fundament, das die UI nur nicht zeigt (kein „gefilterte Ansicht teilen“-Affordanz).

### Klickpfad heute

| Ziel | Schritte heute | Bewertung |
|------|----------------|-----------|
| „Alle Stems zu Artist – Title“ | Files → Title-Suche → Zeile → File-Detail → Variants | ok, aber erst nach Wissen, dass Variants dort liegen |
| „Was ist ungelinkt / nur Backup?“ | Files → Filterpanel → Local: No (+ Backup-Spalte lesen) | Filter „Local“ gut; „Backup only“-Semantik klebt an der Diff-Spalte |

### Top-Friction F3

**Dichte statt Hierarchie:** Eine Tabelle versucht, Browse + Diff-Review + Aktionen gleichzeitig zu sein. Der Files-Screen wäre mit einem expliziten „Review“-Modus (nur Diff-relevante Spalten) und einem „Browse“-Modus (kompakte Tabelle) für beide Aufgaben besser.

---

## F4 · Traktor-Import / Consistency

### Heuristik-Check

- **Sichtbarkeit des Systemstatus (schwach, R01):** Nach „Import Now“ passiert auf der Import-Seite fast nichts Sichtbares — kein Ergebnis-Summary („4 von 5 Tracks gematcht, 4 Ratings übernommen“), keine Abweichungsliste. Das Ergebnis versteckt sich im Task-Log auf der Tasks-Seite. Für „Consistency“ (BPM/Key/Rating zwischen Traktor und Library) gibt es keine UI — der Wert des Imports ist damit unsichtbar.
- **Fehlervermeidung (mittel, R06):** Der manuelle Pfad wird nur per **Enter** übernommen (im Testlauf real reproduziert: Pfad eingetippt, Import gestartet → Task lief mit `custom_path: null` und schlug fehl). Kein Hinweis, kein Save-Button, kein Datei-Browse.
- **Fehlermeldungen (schwach, R07):** Fehlgeschlagene Tasks zeigen ihr Argument als Roh-JSON in der Tabellenzelle; der „failed“-Zustand erklärt keine nächste Aktion. Completed-Tasks zeigen 0 %-Fortschritt (Balken ohne Bedeutung).
- **Erkennung (mittel):** Auto-Detect ohne collection.nml zeigt „Collection —“ — ein Empty-State, der nicht sagt, *wo* Traktor normalerweise liegt oder wie der Auto-Import zu konfigurieren ist (die Erklär-Karte unten ist klein und statisch).

### Klickpfad heute

| Ziel | Schritte heute | Bewertung |
|------|----------------|-----------|
| „Importiere meine Traktor-Stats“ | Traktor Import → ggf. Manual path → Pfad eintippen → **Enter** → Import Now → … Ergebnis auf Tasks suchen | 4+ Schritte, Ergebnis an anderem Ort, Save-Trigger versteckt |
| „Was hat der Import geändert?“ | Tasks → Zeile → Logs → Log-Modal lesen | einziges Fenster; keine Konsistenz-Darstellung |

### Top-Friction F4

Import ist **Aktion ohne Ergebnis-Sicht**: Die Import-Seite sollte nach dem Lauf selbst das Ergebnis zeigen (matched/unmatched, übernommene Felder, Abweichungen zu Library-Werten) — das ist gleichzeitig die „Consistency“-Story, die heute ganz fehlt. Mockup [M5](03-redesign-mockups.md#m5-traktor-import-consistency-import-zeigt-sein-ergebnis) zeigt das Zielbild.

---

## Querschnitt: Navigation, Shell, Konsistenz

### Shell & Navigation (R10)

- 30 Views in Top-Nav + Tools-Dropdown: Die vier sichtbaren Gruppen (Overview/Library/Services/System) sind sinnvoll, aber **Digging, Storage, Backpack, Daily, Bundles, Key Comparison** liegen unsichtbar im Tools-Dropdown — Auffindbarkeit = Erinnerung an die Dropdown-Position.
- Keine globale Suche („zu Datei/Playlist/Tag springen“), kein globaler Aktivitäts-Indikator (laufende Tasks/Downloads sind nur auf ihren Seiten sichtbar), keine Kontext-Hilfe.
- Der Versions-Stempel (v1.3.0) ist bewusst dezent — gut; Branding (Logo-Text) in der Nav ist ok, verbraucht aber Breite, die späteren Hauptwegen fehlt.

### Design-Token-Audit

Positiv: `:root`-Tokens sind sauber definiert (Farben/Räume/Radien/Transition), dark-first, Akzentpalette vorhanden — das Redesign kann auf denselben Tokens aufbauen und muss nichts neu erfinden.
Drift: Radien 4–8 px wirken hart neben 12 px; Schatten fehlen fast komplett (flache Karten); Status-Vokabular variiert (Badge/Chip/Punkt/Text); „—“ als Universal-Platzhalter ohne Unterscheidung „unbekannt/nicht gesetzt/nicht anwendbar“.

---

## Die 5 wichtigsten Baustellen (für die Roadmap)

1. **Ergebnisse & Fortschritt sichtbar machen** (F4-Import-Summary, F1-Queue-Zustand, globale Task-/Download-Aktivität) — R01, R04, R07.
2. **PMV-Review zu einem echten Flow machen** (lesbarer Diff, einzeln anwenden, Bulk-Bar) — R02, R03 → Mockup M2.
3. **Einen Navigations- und Shell-Standard** (Hauptwege sichtbar, Tools gruppiert, Suche später) — R10.
4. **Ein konsistentes Komponenten-Vokabular** (Badges, Chips, Fortschritt, Empty-States, „—“) — R08, R09, R11.
5. **Digging & Telemetry zugänglich machen** (Affordanz in der Ladder, Einstieg + Status der Telemetry sichtbar) — R05 → Mockups M4/M6.

**Weiter:** [03 · Redesign-Mockups](03-redesign-mockups.md) · [04 · Roadmap](04-roadmap.md) · [Start](README.md)
