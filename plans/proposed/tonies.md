# Plan: tonies

**Status**: proposed
**Branch**: `feat/hub-tonies`
**Ready for review**: yes
**Depends on**: `music-api` Vault (`/isrc/{isrc}/{format}`), Hub-Worker (`worker.rs`)
**Migration needed**: yes — additiv, `NNN_hub_tonies.sql`

> **Kurzfassung**: Eine Spotify-Playlist (später auch Backpack/Tag) wird mit einem
> **Kreativ-Tonie** verknüpft. Ein Klick — oder später automatisch, wenn Spotify eine
> Playlist-Änderung meldet — schreibt die passenden Tracks als Kapitel auf den Tonie:
> **greedy von oben bis 90 min**, was nicht mehr passt bleibt draußen.

## Ausgangslage (verifiziert 2026-10-17)

**Fertiger Client existiert**: `~/dev/tonie` v0.2.1 (eigenes Repo `momokli/tonie`) spricht die
inoffizielle Tonies-API bereits vollständig:

| Baustein | Fundstelle | Nutzen |
| --- | --- | --- |
| Auth: Keycloak `login.tonies.com/auth/realms/tonies/…/token` (password grant + refresh on 401) | `src/tonies.rs:24`, `:45` | Account-Verbindung |
| `GET /v2/me`, `/v2/households`, `/v2/households/{hid}/creativetonies[/{ctid}]` | `src/tonies.rs:227–258` | Haushalt, Tonies, Ist-Kapitel |
| `POST /v2/file` → **presigned S3** (`fileId` + URL + Felder) | `src/tonies.rs:259–262` | Hub lädt die Bytes selbst hoch → **keine öffentlichen Track-URLs** |
| Multipart-PUT zu S3, `Content-Type: audio/mpeg`, Felder zuerst, Datei zuletzt | `src/tonies.rs:269–330` | MP3-Upload |
| `POST …/chapters` `{title, file}` | `src/tonies.rs:340` | Kapitel anhängen |
| `PATCH …/creativetonies/{ctid}` `{chapters:[…]}` — leeres Array löscht alles | `src/tonies.rs:363–378` | Der Sync-Primitiv |
| `Chapter { id, title, file, seconds, transcoding, type }` ist `Serialize` | `src/tonies.rs:81–91` | Bestehende Kapitel im PATCH **wiederverwenden** → kein Re-Upload |
| Kapazität `secondsPresent` + `secondsRemaining` (≈ 5400 s = 90 min), `transcoding`, `live` | `src/tonies.rs:58–80`, `:382`, `:393` | Fit-Budget + „fertig getranscodiert?" |

**Hub-Seite bringt den Rest schon mit**:

- `hub_tracks.duration_ms` (`migrations/001_hub_schema.sql:41`) → Fit rechnet **rein lokal**.
- `hub_playlists.snapshot_id` / `last_synced_snapshot` und `worker.rs:457–470`: Items werden
  **nur** geholt, wenn `items_available = 0` oder Spotifys `snapshot_id` sich geändert hat;
  danach setzt `worker.rs:502–509` `last_synced_snapshot = snapshot_id`.
- `music_api::file_bytes(cfg, isrc, format)` / `file_bytes_any(…)`
  (`music-hub/src/music_api.rs:126`, `:151`) → Audio-Bytes aus music-api, inkl. Fallback.
- `music_api::status()/ready()` → „liegt der Track im Vault?"; `order()` existiert für später.

## Entscheidungen (genommen, im Review änderbar)

| # | Entscheidung | Begründung / Alternative |
| --- | --- | --- |
| D1 | **Credentials im Hub** (Login-Formular, Passwort bzw. Refresh-Token **verschlüsselt at rest**, wie bei Spotify) | Nur so funktionieren „URL + Button" **und** späterer Autosync ohne laufenden Rechner. Alternative (kellner macht den Upload) bleibt möglich, weil der Client als eigenes Modul geschnitten wird. |
| D2 | **Format `mp3-128` als Default**, `mp3-320` pro Binding umschaltbar | 90 min ≈ 86 MB @128 vs. ≈ 216 MB @320; Toniebox ist ein Kinder-Lautsprecher. Musik-API hält beides. |
| D3 | **Quelle**: M1 nur Spotify-Playlist, M2 zusätzlich **Backpack/Tag** (gleicher Sync-Kern, anderer Resolver) | Playlist ist der beschriebene Flow; Backpack/Tag ist die Hub-Logik und fällt fast gratis ab. |
| D4 | **Code-Ort**: Modul `music-hub/src/tonies/{mod,client,sync}.rs` — kein neues Release-Artefakt | Kein neuer Komponenten-Tag/Versionierungsstrang; lässt sich später als Crate herausziehen, wenn kellner es braucht. |

## Datenmodell (eine additive Migration)

```sql
hub_tonies_accounts(user_id PK, household_id, tonie_email, secret_enc, last_login_at, last_error)
hub_tonie_bindings(id PK, user_id, tonie_id, tonie_name,
                   source_kind ('playlist'|'backpack'|'tag'), source_id,
                   format ('mp3-128'|'mp3-320') DEFAULT 'mp3-128',
                   cap_seconds DEFAULT 5400, mode ('manual'|'auto') DEFAULT 'manual',
                   dirty_since, last_sync_at, last_sync_status,
                   UNIQUE(user_id, tonie_id))
hub_tonie_syncs(id, binding_id, started_at, finished_at, status,
                chapters_before, chapters_after, seconds, added, removed,
                skipped_json, error)
```

## Sync-Kern (deterministisch, ohne Netz testbar)

1. **Desired list**: Tracks der Quelle in Reihenfolge → `(isrc, title, duration_ms)`.
   `ready()` prüfen: Tracks ohne Vault-Datei werden **übersprungen und protokolliert**
   (später optional `music_api::order()`).
2. **Greedy Prefix**: von oben aufsummieren, bis `cap_seconds` überschritten würde; der
   auslösende Track fällt raus, danach wird **nicht weiter gesucht** (so gewünscht).
3. **Diff** gegen die Ist-Kapitel (`title` + `seconds` + Reihenfolge):
   - identisch → `unchanged`, kein API-Write;
   - Prefix identisch → nur den Rest anhängen (`POST chapters`);
   - sonst → `PATCH {chapters:[…]}` mit **wiederverwendeten** bestehenden Kapiteln für den
     gleichen Prefix, neu hochgeladen wird nur die Differenz.
4. **Upload** pro neuem Track: `POST /v2/file` → Multipart-PUT nach S3 (Bytes aus
   `file_bytes(isrc, fmt)`) → `POST chapters`.
5. **Warten**: `transcoding` pollen (bounded, mit Timeout) und Ergebnis im Run-Log festhalten.

## Trigger

- **Manuell (M2)**: `POST /tonies/{id}/sync` — htmx, Session-Owner only, pro User rate-limited,
  Antwort als Toast (`12 Kapitel, 61 min — 3 passten nicht mehr, 1 nicht im Vault`).
- **Automatisch (M3)**: Hook **direkt nach** `last_synced_snapshot = snapshot_id`
  (`worker.rs:506`): `UPDATE hub_tonie_bindings SET dirty_since = now()` für die Bindings dieser
  Playlist. Ein neuer Hub-Loop (`LOOP_NAMES` erweitern) konsumiert `dirty`-Bindings.
  **Debounce**: erst nach einer Ruhephase (Default 5 min ohne weitere Änderung) und nur, wenn
  sich der *gepasste Prefix* tatsächlich unterscheidet; Mindestabstand pro Tonie (Default 10 min).

> **Spotify-Budget**: Der Autosync erzeugt **null zusätzliche Spotify-Requests** — er hängt an
> der bestehenden Snapshot-Erkennung. Es wird nichts zusätzlich gepollt.

## UI

`/tonies` (Session-geschützt, pro User):
- Account-Zustand: verbunden / „neu verbinden" (Token abgelaufen).
- Karte pro Kreativ-Tonie: Kapazitätsbalken („63 min / 90 min"), Quelle (Playlist/Backpack/Tag),
  Format, Modus (manual/auto), **„Sync now"**, letzter Sync mit Diff + übersprungenen Tracks.
- Run-Log (letzte N Syncs) mit Dauer und Fehlertext.

## Meilensteine

| M | Inhalt | Akzeptanz |
| --- | --- | --- |
| **M1** | Client-Modul (Auth, Liste, Kapitel lesen, `request_upload`/S3/`chapters`/`PATCH`, `live`), Account-Verbindung, `/tonies` read-only (Tonies + Kapazität) | Tests gegen gemockte Tonies-API; kein Netz in CI; 401 → „neu verbinden" |
| **M2** | Bindings + manueller Sync (Greedy-Fit, Diff, Upload, Transcoding-Poll), Run-Log | Fit-Grenzfälle + alle drei Diff-Fälle getestet; kein Re-Upload bei unverändertem Prefix |
| **M3** | Auto-Sync über den Worker-Hook + Debounce + Loop-Health | Ein Playlist-Refresh markiert dirty; zweiter Refresh ohne Änderung erzeugt keinen Sync |

## Tests

- `music-hub/tests/tonies.rs` gegen eine gemockte Tonies-API (Muster wie `tests/nowplaying.rs`).
- Unit-Tests für den Greedy-Fitter: exakt 5400 s, ein Track zu lang, leere Quelle, doppelte ISRC,
  fehlende Vault-Datei.
- Diff-Fälle: `unchanged` / `append` / `replace` (mit Kapitel-Wiederverwendung).
- Ownership: fremdes Binding → 403/404; Sync-Endpoint rate-limited.
- **Live (erst nach dem Umzug)**: echter Tonie, 90-min-Grenze, `mp3-128` abspielbar,
  Re-Sync nach Playlist-Änderung, `mp3-320`-Variante.

## Offene Punkte, die nur ein Live-Test klärt

- Akzeptiert die Tonies-API **128 kbps** MP3 klaglos? (Der Client sendet überall `audio/mpeg`.)
- Lässt `PATCH` bestehende `file`-IDs wiederverwenden (kein Re-Upload)? Die Serialisierbarkeit
  der `Chapter`-Structs legt es nahe — unbelegt.
- Maximale Kapitelanzahl pro Tonie? (unbekannt)
- Gibt es ein Tageslimit für `POST /v2/file`?

## Non-Goals

- teddyCloud / Toniebox-OTA / `.taf`-Manipulation — wir nutzen die offizielle Cloud-API.
- Automatisches Bestellen fehlender Tracks im Sync (später, über die bestehende „order missing"-Logik).
- Kapitel-Reihenfolge abweichend von der Playlist; Normalisierung/Lautheit pro Kapitel.

## Akzeptanz (Plan)

- [ ] `cargo test` in `music-hub/` grün, neue Tonie-Tests enthalten, kein Netzwerk in CI
- [ ] `/tonies` erreichbar, nur eigene Bindings sichtbar
- [ ] Manueller Sync: Kapazität korrekt gefüllt, Diff ohne Re-Upload, Toast mit Zusammenfassung
- [ ] Auto-Sync erzeugt **keine** zusätzlichen Spotify-Requests
- [ ] Keine Tonies-Credentials in Logs, Templates oder Fehlermeldungen
- [ ] Live-Test dokumentiert (echter Tonie), inkl. der vier offenen Punkte oben
