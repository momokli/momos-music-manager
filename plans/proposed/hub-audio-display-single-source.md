# Plan: hub — BPM/key-Anzeige aus einer Quelle (live Vault, 1:1)

**Status**: proposed
**Branch**: `fix/hub-audio-display-single-source`
**Depends on**: —
**Migration needed**: no

### Problem

BPM/Key werden an zwei Stellen **aus zwei verschiedenen Quellen** gezeigt und driften:

| Ort | Quelle | Ergebnis |
| --- | --- | --- |
| Track-Page `/track/{id}` | **live** `GET /vault/track/{isrc}` (`web.rs::build_track_page`) | kennt die BPM ✓ |
| Now-Playing `/now-playing` | **Cache** `hub_track_audio` (`web.rs::track_bpm_key` → `analysis::get`) | „—" ✗ |

Der Cache (`hub_track_audio`) hinkt nach: er wird vom Worker `refresh_from_vault(pool, cfg, 200)`
gefüllt — **200 Zeilen/Durchlauf**, `ORDER BY t.id`, und eine einmal geschriebene
„no data"-Zeile wird **7 Tage** (`RECHECK_DAYS`) nicht neu geprüft. Track 16596 (hohe ID)
steht entsprechend hinten. Ergebnis: Page und Leiste widersprechen sich.

### Decision (sauber, kein Wusel)

**Die angezeigte BPM/Key kommt überall 1:1 aus dem Vault — live.** Latenz ist unkritisch.

- **Eine** geteilte Funktion für den aufgelösten Anzeige-Wert, auf der eingefrorenen
  Consumer-Fläche `GET /vault/tracks?isrcs=` (dieselbe `resolved`-Definition, aus der auch
  die Track-Page ihren Wert bekommt — ADR-079/080: „resolved mirrors `/vault/tracks`").
- **Now-Playing liest nicht mehr** `hub_track_audio` für die Anzeige, sondern denselben
  Vault-Wert wie die Track-Page.
- **Kein** Targeted-Refresh/TTL-Gefrickel.

### Abgrenzung

`hub_track_audio` **bleibt** — als server-seitiger Read-Model für **Bulk-Berechnungen**
(Scoring/Digging/Similarity), wo kein HTTP pro Zeile geht. Nur die **Anzeige** liest ihn
nicht mehr. (Dass berechnete Feature-Werte und die Anzeige dadurch unterschiedlich frisch
sein können, ist ein **eigener** Punkt — hier nicht gelöst, bewusst.)

### Umsetzung (Skizze)

- `music_api::resolved_audio(cfg, isrc) -> Option<(bpm, key/camelot)>` — dünner Wrapper um
  `vault_tracks(cfg, &[isrc])`.
- Now-Playing: ISRC des aktuellen Tracks bestimmen (aus `hub_tracks.isrc` über
  `hub_track_id`; optional zusätzlich `external_ids.isrc` aus dem Spotify-Payload, falls der
  Track noch nicht im Hub ist) → `resolved_audio` → Anzeige.
- `web.rs::track_bpm_key` von `analysis::get` auf `resolved_audio` umstellen (bzw. die
  Now-Playing-Stelle direkt).
- Track-Page: unverändert (ihr `resolved` ist per Contract identisch) — oder denselben
  Helper nutzen, damit es nur **eine** Stelle gibt.

### Acceptance

- [ ] Track-Page und Now-Playing zeigen für denselben Track **denselben** BPM/Key.
- [ ] Now-Playing zeigt den Vault-Wert **auch wenn** `hub_track_audio` leer/veraltet ist.
- [ ] Genau **eine** Funktion liefert den Anzeige-Wert (kein zweiter Pfad).
- [ ] `cargo test` (music-hub) grün; Test: leeres Read-Model + gefüllter Vault → Leiste zeigt Wert.

### Out of scope

- Der Read-Model selbst / seine Befüllung (`refresh_from_vault`, TTL) bleibt wie er ist —
  nur die Anzeige hängt nicht mehr daran.
