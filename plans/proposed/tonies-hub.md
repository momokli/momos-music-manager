# Plan: Tonies-Hub als eigener Dienst

**Status**: approved (Entscheidung 2026-11-19, ADR-083)
**Branch**: `feat/tonies-hub`
**Ready for review**: no
**Depends on**: music-api (`/deezer/*`, `/orders`, `/status`), Deploy-Host `music-catalog`
**Migration needed**: ja — **eigene** DB `tonies.db`, dazu ein einmaliger Import aus `hub.db`

## Beschluss

Drei Dienste, `music-api` als gemeinsamer Unterbau:

```
                ┌──────────────┐
   music-hub ──▶│  music-api   │◀── tonies-hub
   (DJ-Stack)   │ Katalog/Order│    (Eltern/Tonies)
                └──────────────┘
```

`music-hub` und `tonies-hub` sind **Geschwister**: keiner ruft den anderen. Beide
sprechen nur `music-api`. Tonies werden aus **Deezer-Alben** gefüllt — dafür
braucht es keinen Hub-Zustand.

## Warum

Gemessen am Code vom 2026-11-19:

| Befund | Zahl |
|---|---|
| Tonie-Domäne in `music-hub/src/tonies/` + `tonie_history.rs` | 4.616 Zeilen |
| Treffer auf „tonie" in `music-hub/src/web.rs` (5.138 Zeilen) | 272 |
| Tonie-Routen (`/tonies…`) in `web.rs` | 15 (+ `/artist/{key}/cart` in `artists.rs`)<br>_gezaehlt ueber alle `.route(...)`-Registrierungen, auch mehrzeilig (Korrektur 2026-11-20)_ |
| Tonie-Templates | 3 Seiten + 3 Partials, Shell aus `base.html` |
| Tabellen | 6 × `hub_tonie_*` (+ stillgelegtes `hub_deezer_albums`); `hub_tonie_bindings_new` ist nur der Rebuild-Zwischenschritt aus 060 |

Die **Domäne ist schon gekapselt** (Abhängigkeiten nach außen: `music_api` 5×,
`ui` 4×, `config` 4×, `backpacks` 3×, `db` 2×). Was mischt, ist die **Fläche**:
272 Treffer in einer 5k-Zeilen-Datei, ein Nav-Eintrag zwischen Backpacks und
Tags, und eine Zielgruppe (Eltern auf dem Handy), die durch ein DJ-Werkzeug
navigiert. Dazu kommt der Betrieb: ein Deploy, ein Release-Takt, ein
Fehlerradius für zwei Produkte.

## Zielbild

| | music-hub | tonies-hub |
|---|---|---|
| Zielgruppe | DJ / Bibliothek | Eltern, Kind |
| Daten | Spotify-Ingest, Tags, Backpacks, Playlists | Figuren, Bindings, Korb, Verlauf |
| Quellen | Playlists, Backpacks, Tags | **nur** Deezer-Alben (Artist-Abo, Korb) |
| DB | `hub.db` | **`tonies.db`** (eigene Datei) |
| Konten | `hub_users` | **eigene `users`** (SSO später möglich) |
| Session-Cookie | `hub_session` | `tonies_session` |
| Port (Default) | 8080 | 8090 |
| Deploy | `hub-deploy.timer` | `tonies-hub-deploy.timer` (5 min, main-basiert) |
| Host | `hub.zukkafabrik.de` | `tonies.zukkafabrik.de` |

### Geteilter Code — so wenig wie möglich

Eine neue Lib-Crate **`music-api-client`** (path-dependency, kein Workspace nötig):

- `deezer_artist_albums`, `deezer_album`, `deezer_search_artist`
- `file_bytes`, `order` (mit `intent`/`target`), `readiness`/`status`
- Konfiguration als `ApiConfig { base, token }` statt `&music_hub::config::Config`

Umgestellt werden **beide** Seiten (`music-hub/src/music_api.rs` wird ein
dünner Re-Export), damit es genau eine Implementierung gibt.

**Nicht** geteilt: Tonies-Domäne (zieht nach `tonies-hub`), Askama-Shell,
Auth, Toast/Flash. Die werden dort klein und eigenständig gebaut; geteilte
UI-Bibliotheken sind der klassische Ort, an dem zwei Produkte wieder
zusammenwachsen.

## Nicht-Ziele

- **Kein Shared-DB.** `tonies-hub` liest/schreibt `hub.db` nicht. Sonst sind
  Migrationen und Deploys wieder gekoppelt — genau das soll weg.
- **Kein zweiter Katalog.** Alben/Tracks/Dauern/Readiness kommen aus
  `music-api` (#621, `/deezer/*`, `/orders`).
- **Keine Playlist/Backpack/Tag-Quellen** in `tonies-hub`. Das sind
  Hub-Konzepte und bleiben dort; die alten Bindings (Migration 057–061) werden
  **nicht** portiert, nur als Historie in `hub.db` belassen.
- **Kein Umzug der Hub-Tabellen.** Sie bleiben liegen (Rollback), bis
  `tonies-hub` nachweislich läuft.

## Datenmodell `tonies.db`

Migrationen `001…` in `tonies-hub/migrations/`:

| Tabelle | Inhalt |
|---|---|
| `users` | `id`, `slug`, `display_name`, `password_hash` (bcrypt), `is_admin`, `created_at` |
| `sessions` | `id` (Token), `user_id`, `created_at`, `expires_at` |
| `tonies_accounts` | wie `hub_tonies_accounts`: `user_id`, `household_id`, `tonie_email`, `secret_enc`, `last_login_at`, `last_error` |
| `bindings` | `user_id`, `tonie_id`, `tonie_name`, `source_kind='albums'`, `artist_id`, `artist_name` (im Hub heissen sie `source_id`/`source_name`, Migration 064), `cap_seconds`, `mode`, `album_mode`, `order_by`, `last_sync_*` |
| `cart_items` | `user_id`, `tonie_id`, `position`, `album_id`, `title`, `cover_url`, `seconds`, `added_at`, `UNIQUE(user_id, tonie_id, album_id)` |
| `albums_seen` | `user_id`, `album_id`, `seen_at` (Ledger, PRIMARY KEY `(user_id, album_id)`) |
| `syncs` | Run-Log: `binding_id`, `status`, `started_at`, `finished_at`, `added`, `removed`, `skipped_json`, `error` + Teil-Index „eine `running`-Zeile pro Binding" |
| `settings` | `key`/`value` (music-api-Base/-Token, `tonies_client_id`) |

Verschlüsselung der Tonies-Zugangsdaten wie im Hub (`secret_enc`, Key aus
`TONIES_SECRET_KEY`), **nie** im Klartext, **nie** im Log.

## Slices

| Slice | Inhalt | Warum zuerst |
|---|---|---|
| **T1** | Lib-Crate `music-api-client`; `music-hub` nutzt sie (Verhalten unverändert) | Entkoppelt, bevor irgendetwas umzieht; testbar ohne neue Oberfläche |
| **T2** | `tonies-hub`-Crate: DB + Migrationen + Sessions + Login/Logout + `/health` | Fundament ohne Domäne |
| **T3** | Tonies-Client (`client.rs`) + Konto verbinden (verschlüsselt) + Figurenliste | Erst danach gibt es etwas zu zeigen |
| **T4** | Katalog: Artist-Suche, Album-Grid mit Dauern, „In den Korb" | Kernnutzen: Folgen finden |
| **T5** | Korb + Binding + Album-Modus (`newest`/`oldest`/`basket`) + Fit auf ganze Alben | Das ist der Sync-Plan |
| **T6** | Sync-Run + Autosync-Loop (Lease über `syncs.running`-Teilindex) + Run-Log/Verlauf | Schreibender Teil |
| **T7** | Betrieb: `deploy/` (systemd, 5-Min-Timer, Caddy-Vhost), CI-Jobs, Import-Skript aus `hub.db` | Ohne Deploy kein echter Test |
| **T8** | `music-hub`: Tonies-Fläche entfernen (Routen, Templates, Nav-Eintrag, `worker.rs`-Hook), Tabellen bleiben | Erst wenn `tonies-hub` stabil läuft |

Reihenfolge ist bindend: T8 **erst** nach einem grünen Live-Lauf von T1–T7.

## Akzeptanzkriterien

- [ ] `cargo build` + `cargo test` in **allen drei** Crates grün (CI prüft jede).
- [ ] `tonies-hub` startet mit leerer DB und legt alle Tabellen über Migrationen an.
- [ ] **Keine** Zeile in `tonies-hub/src` öffnet `hub.db` oder ruft einen Hub-Endpunkt (Test: Grep + Review).
- [ ] Login per bcrypt-Passwort; Session-Cookie `tonies_session`; `/health` ohne Auth.
- [ ] Katalog-Grid zeigt Cover, Dauer und `im Korb` (Screenshot über `scripts/dev/`-Muster).
- [ ] `newest`/`oldest` sind spiegelbildlich (Test mit fixierten `release_date`).
- [ ] Fit nimmt nie ein Teil-Album (20-min-Album bei 10 min Rest → übersprungen, Test).
- [ ] Korb-Reihenfolge wird Kapitel-Reihenfolge (End-to-End-Test gegen Tonies-Mock).
- [ ] Ein `ok`-Run schreibt die benutzten Alben ins Ledger; der nächste Run überspringt sie.
- [ ] Album-Skips stehen mit Grund im `skipped_json`.
- [ ] Kein Spotify-Request im gesamten Pfad (Zähler-Mock, wie `tonies_autosync.rs`).
- [ ] Import-Skript übernimmt Bindings/Korb/Ledger aus `hub.db`, ohne sie dort zu verändern.
- [ ] `tonies-hub` deployt über einen eigenen Timer und antwortet auf `tonies.zukkafabrik.de`.
- [ ] Nach T8: `music-hub` hat keine `/tonies`-Route mehr, `hub.db` behält die Tabellen.

## Risiken

| Risiko | Umgang |
|---|---|
| **Zwei Konten pro Person** (Hub + Tonies-Hub) | Bewusst akzeptiert für die Entkopplung; Ausbaupfad: Magic-Link/SSO (`plans/proposed/accounts-mail.md` liegt schon vor) |
| Dritter Deploy-Block auf dem Host | Muster von `music-hub/deploy/` kopieren; eigener Health-Gate wie dort |
| Import-Fehler bei laufendem Hub | Import liest `hub.db` **read-only**; Hub-Tabellen bleiben unangetastet |
| Zwei Implementierungen des music-api-Clients driften | T1 zieht **beide** auf die Lib, `music-hub/src/music_api.rs` wird Re-Export |
| Tonies-API-Verhalten (429, Token-Rotation) | Client kommt unverändert aus dem Hub (dort erprobt); Retry/Refresh-Logik mitnehmen |
| Runner-Betrieb mit drei Komponenten | CI-Jobs je Crate; Milestone-Slices klein halten (WIP = 1) |

## Offene Punkte (entschieden, aber umkehrbar)

1. **Eigene Konten** in `tonies.db` (Default) statt geteilter `hub_users`.
   Umkehrbar, weil Auth klein ist; SSO wäre ein späterer Slice.
2. **Alben-only.** Die Playlist/Backpack/Tag-Bindings sterben mit dem Umzug.
   Falls sie gebraucht werden, wären sie ein neuer Slice in `tonies-hub` — nicht
   eine Rückkehr in den Hub.
3. **Port 8090 / Domain `tonies.zukkafabrik.de`** sind Vorschläge, per Env/Caddy
   änderbar.
