# Plan: hub track page — download UX (toast + coherent format buttons)

**Status**: proposed
**Branch**: `feat/hub-track-download-ux`
**Depends on**: —
**Migration needed**: no
**Issue**: (see GitHub issue)

### Problem

- **Roh-Fehler statt Feedback.** Die Buttons auf der Track-Page sind bare Links
  (`<a href="/track/{id}/download?format=…">`). Der Handler `pages.rs::track_download`
  streamt im Erfolgsfall die Datei, gibt bei Fehler aber **plain-text 502** zurück
  (`music-api: nicht verfügbar (erst ordern): … → 404`). Der Browser navigiert hin und
  zeigt das als nackte Fehlerseite in einem neuen Tab — ohne Kontext, ohne Weg zurück.
- **Tote Buttons.** Beide Links rendern bei `music_api && has_isrc` — obwohl der
  Page-Load mit `vault.download_state` / `vault.download_formats` **schon weiß**, ob
  überhaupt etwas bereit ist. `download_ready` wird heute nur für den Player-Fallback
  (`fetchable`) benutzt, nicht für die Buttons.
- **Uneinheitliche Formate.** Der music-api-Contract liefert **drei** Varianten —
  `flac`, `320`, `128` (`GET /isrc/{isrc}/{format}`) — die Page zeigt aber nur `flac`
  und ein `mp3` (das auf `320` mappt). `128` ist unerreichbar; die Beschriftung ist
  inkohärent.

### Vorhandene Bausteine (wiederverwenden, nicht neu erfinden)

- **Toast-System:** `#hub-toasts` + `window.hubToast(msg, kind)` und das htmx-`toast`-
  Event, gespeist über `HX-Trigger: {"toast":{"msg":…,"kind":…}}` (in `base.html`).
  Server-Helfer: `web.rs::with_toast`, `music_ops.rs::action_response` / `toast_header`,
  `hub_ops.rs::toast_header`.
- **Ordern ist automatisch** (ADR-068: Demand = ganze Bibliothek, Backpacks zuerst,
  „nach und nach") — ein manueller Holen-Button ist damit **redundant**. `POST
/track/{id}/fetch` existiert, hat aber nur Restwert als **Retry für `failed`/`stranded`**
  Tracks (die die Demand wegen vorhandener Ledger-Zeile überspringt). Auf der
  Download-Zeile wird er **nicht** benutzt.
- **Format-Mapping:** `valid_format` + `music_api::file_bytes_any` (mp3 → `320`).

### Design

1. **Preflight-Download (keine Roh-Fehler).** Neuer htmx-Endpoint
   `POST /track/{id}/download/check?format=X`:
   - bereit → `204` + Info-Toast + `HX-Redirect: /track/{id}/download?format=X`
     (der Streaming-Endpoint bleibt für die Bytes unverändert).
   - nicht bereit / music-api weg → `204` + **Err-Toast** („music-api nicht verfügbar —
     erst ordern"), **keine** Navigation.
   - Die Buttons werden `hx-post` mit `hx-target="this"`.
2. **Verfügbarkeits-getriebene Buttons.** Formatset aus `vault.download_formats`
   (`flac`/`320`/`128`) + `download_state` ableiten:
   - bereit → **je ein** Download-Button pro vorhandenem Format („FLAC", „MP3 320",
     „MP3 128") — kohärente Labels.
   - nicht bereit → **kein Button**, nur eine passive Zustandsanzeige („wird geholt … ·
     <state>"). Kein manuelles Ordern — die Demand deckt die Bibliothek ohnehin ab
     (ADR-068).
   - keine ISRC → nichts (wie heute).
3. **Konsistente Platzierung.** Die Download-Controls wandern aus der losen oberen
   Link-Zeile in die bestehende „Download — Objekt-Store"-`hub-bucket`-Sektion (dort,
   wo der Vault-Download-Satellit schon rendert), damit Zustand + Aktion zusammen liegen.
   Die obere Zeile behält nur Navigations-Links (Digging, digdeeper).
4. **Gleiches für Playlist/Tag-ZIP** — `zip_for`s 502 → Toast, im selben Milestone.

### Acceptance

- [ ] Ein Download-Klick landet **nie** auf einer Roh-Fehlerseite; Fehler sind Toasts.
- [ ] Buttons spiegeln `vault.download_state`/`download_formats`: nur **bereite** Formate
      sind Buttons; nicht bereit → passive Zustandsanzeige, **kein** Order-Button.
- [ ] `flac` / `mp3-320` / `mp3-128` sind je erreichbar, wenn vorhanden; Labels kohärent.
- [ ] Download-Controls liegen in der Download-Sektion, nicht in der oberen Link-Zeile.
- [ ] Integration-Tests: bereit → `HX-Redirect` + **kein** 502-Body; nicht bereit →
      Err-Toast-Header, kein Redirect; Formatset aus State abgeleitet.
- [ ] `cargo test` (music-hub) grün.

### Out of scope

- Toasts auf anderen Pages (search/overlap/digging/playlist/tag) — eigenes Folge-Issue
  („Kohärenz der Page-Elemente").
- Der bestehende Player-Fallback „Jetzt holen" (`POST /track/{id}/fetch`) bleibt vorerst
  unangetastet — für den Normalfall redundant (ADR-068), nützlich nur als Retry für
  `failed`/`stranded`. Eigene Entscheidung.
