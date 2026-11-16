# Plan — Issue #510 (Meilenstein M2): nativer Download im Worker + ISRC-Tag

Repo: momokli/momos-music-manager · Milestone `music-api-0.5.0`
Basis-Branch: `main` (origin/main = `7ac913e`, M1 `deezer_dl` bereits gemergt via #515)
Gesamtplan: `plans/proposed/native-deezer-download.md` (M2)
Scope/Akzeptanz: `progress-issue-510.md`

> **Kein Produktionscode hier.** Nur der Plan. Schwere Rust-Builds/Tests laufen auf `planet`.
> Nichts committen.

---

## Zielbild M2 (ein Satz)

Der Worker lädt Deezer-Tracks **nativ** über das bereits gemergte `deezer_dl`-Modul (statt
`deemix::add_to_queue` + Queue-Polling), schreibt das **ISRC-Tag** in die Datei, legt sie über die
**vorhandenen** Wege ab (`store_downloaded`/`mark_ready` + `vault_download`) und protokolliert
**Dauer, Qualität, Quelle = native**. Der Sidecar-Pfad (`DEEMIX_URL`) bleibt unverändert funktionsfähig.

## Ausgangslage im Code (verifiziert 2026-11-16)

| Baustein | Fundstelle | Für M2 relevant |
| --- | --- | --- |
| Pending → resolve → dispatch | `worker.rs:resolve_pending` | Hier wird `deezer_id` per `deezer::lookup_isrc` ermittelt und via `db::mark_resolved` persistiert. |
| In-Flight-Polling (deemix) | `worker.rs:advance_downloads` | Pollt `db::downloading_tracks` (nur `source='deezer'`) gegen `deemix::queue`. |
| Datei-Einsortierung | `worker.rs:store_downloaded`, `worker.rs:finalize` | Klassifiziert `flac` vs. mp3 nach Extension + `transcode::probe_bitrate`; schreibt `mark_ready`. |
| ISRC Lesen | `tags.rs:read_tags`/`parse_ffprobe` | Nur Lesen via `ffprobe`. **Schreiben fehlt.** |
| Tag-Lib | `Cargo.toml:34` `lofty = "0.20"`; Vorbild `src/bin/store_import.rs:18-68` | `lofty::read_from_path` / `tag.*` / `save_to_path(dest, WriteOptions::default())`. |
| Native DL-API | `deezer_dl/mod.rs` | `login(arl)->Session`, `download_track(&Session,id,Quality)->Vec<u8>`, `download_track_to_file(...,&Path)->Downloaded`, `Quality::{Mp3_128,Mp3_320,Flac}`. |
| Session-Hygiene | `deezer_dl/mod.rs` (Modul-Doku + Tests) | ARL wird nie gehalten/geloggt; `Error::InvalidArl`, `Error::NoLosslessTier`, `Error::TrackGone`. |
| ARL/Tier | `config.rs:deemix_arl`, `lib.rs:init_arl_tier` | ARL existiert bereits; Tier-Cache in `AppState.arl_tier`. |
| Vault/Store | `vault.rs:load_download`/`load_track_download`, `store.rs` | `sat_track_download` unverändert nutzen. |
| Logs | `AppState.logs` (`logbuf`), `tracing` | Quelle für „Run-Log". |

---

## Kritische Punkte & Entscheidungen

### 1. Session-Beschaffung (ARL aus `config.deemix_arl`, kein neues Feld)
- **ARL-Quelle:** `config.deemix_arl` unverändert. Kein neues Config-Feld in M2 (ein evtl.
  Umbenennen `DEEMIX_ARL` → `DEEZER_ARL` gehört in M3, um den Sidecar nicht zu brechen).
- **Session-Cache:** `login()` ist teuer → **einmal** pro Prozess. Neues Feld in `AppState`:
  `native_session: Arc<tokio::sync::RwLock<Option<Arc<deezer_dl::Session>>>>`.
  Analog zu `ensure_deemix_login` eine `ensure_native_login(state) -> Option<Arc<Session>>`,
  die bei `Error::InvalidArl` die Session verwirft und (mit dem **vorhandenen** `LoginBackoff`,
  neuer zweiter Zähler z. B. `native_login`) 60s→1800s zurückweicht.
- **ARL-Hygiene (zwingend):** ARL nur an `deezer_dl::login` übergeben. Nie in `tracing`, nie in
  `state.logs.push`, nie in `anyhow::bail!(...)`/Fehlermeldungen. Fehlertext von `deezer_dl::Error`
  ist ARL-frei (durch M1-Tests belegt) → nur `{e}` loggen, niemals die ARL-Variable.
- **Leerer ARL:** kein Login-Versuch, genau eine Warnung (Muster `warn_missing_arl`).

### 2. Welche Deezer-Track-ID (kein neuer ISRC→ID-Lookup)
- `resolve_pending` hat `deezer_id` bereits aus `deezer::lookup_isrc` und schreibt es mit
  `db::mark_resolved` in `tracks.deezer_id`. Der native Pfad nutzt **genau diese** `track.deezer_id`.
- Fallback: ist `track.deezer_id` beim Start eines nativen Jobs leer (z. B. Crash-Recovery), wird
  `deezer::lookup_isrc` erneut aufgerufen — **derselbe** Lookup-Pfad, kein zweiter Mechanismus.

### 3. ISRC-Schreiben (MP3-ID3 `TSRC` / FLAC-Vorbis `ISRC`)
- Neue Funktion in `tags.rs`, z. B.
  `pub async fn write_isrc(path: &Path, isrc: &str) -> anyhow::Result<()>` (offloaded via
  `tokio::task::spawn_blocking`).
- **Ein** Codepfad für beide Container über `lofty`:
  `let mut f = lofty::read_from_path(path)?;` → `f.primary_tag_mut()` (bzw. `Tag::new(...)` +
  `insert_tag` falls keiner existiert) → **`tag.insert_text(ItemKey::Isrc, isrc)`** →
  `f.save_to_path(path, WriteOptions::default())?`.
  `ItemKey::Isrc` mappt in lofty auf ID3v2 **`TSRC`** (MP3) bzw. Vorbis-Comment **`ISRC`** (FLAC) —
  eine Implementierung, beide Container. (`insert_text`-Signatur gegen `lofty 0.20` verifizieren;
  Vorbild für Lese/Save-Fluss: `src/bin/store_import.rs`.)
- **Reihenfolge:** ISRC **vor** dem Einsortieren in den Store schreiben (auf der Temp-/Incoming-
  Datei), damit die abgelegten `flac/320/128`-Dateien das Tag tragen; danach optional
  `tags::read_tags` als Verifikation.
- Kein `ffprobe`-Schreibweg — `ffprobe` liest weiterhin nur.

### 4. Sidecar-Fallback erhalten (M3 entkoppelt, M2 darf nicht brechen)
- Neuer Config-Schalter `DOWNLOAD_BACKEND` mit Werten `native | sidecar | auto`
  (`env_parse`-Stil wie die übrigen Flags). **Default `auto`** = native wenn `DEEMIX_ARL` gesetzt,
  sonst sidecar. Achtung: `DEEMIX_URL` hat einen Default (`http://127.0.0.1:6595`) → „gesetzt" ist
  **kein** verlässliches Signal; daher der explizite Schalter.
- **Trennung ohne DB-Migration:** native Jobs erhalten ein UUID-Sentinel
  `deemix_uuid = "native:<isrc>"`. `advance_downloads` überspringt Zeilen mit diesem Prefix
  (native Downloads laufen als Hintergrund-Task wie `spawn_non_deezer` und werden dort finalisiert).
  Sidecar-Zeilen (`track_<id>_1`) werden exakt wie heute gepollt.
- `ensure_deemix_login`/`deemix::*` bleiben unangetastet; `submit_artist_order`/Artist-Orders
  bleiben in M2 auf dem Sidecar-Pfad (Entkopplung ist M3).

### 5. Ablage unverändert (Store + Vault + API)
- Nutze die bestehenden Funktionen: native Bytes → Temp-Datei → `store_downloaded(...)` (klassifiziert
  flac/320/128) → `db::ingest_ready_track` **oder** `db::mark_ready` (je nach bereits vorhandener
  Ledger-Zeile) → `vault_download`/`vault::load_download` (schreibt `sat_track_download`).
- `/isrc/{isrc}/{format}`, `store_objects`, `sat_track_download` bleiben **unverändert**.

### 6. Run-Log (Dauer, Qualität, Quelle=native)
- `std::time::Instant` um den Download; Ergebnis als strukturierte Zeile
  `state.logs.push("info", format!("{isrc}: ready (native, {qual}, {secs:.1}s)"))`
  plus `info!(isrc, quality, source = "native", elapsed_ms, "native download complete")`.
- Keine ARL/Token in der Zeile.

---

## User Stories (in Reihenfolge)

### US-1 — `DownloadBackend`-Schalter + Konfiguration
- **Akzeptanz:** `Config.download_backend: DownloadBackend` (enum `Native|Sidecar|Auto`) aus
  `DOWNLOAD_BACKEND` (`auto` default); `auto` wählt native, wenn `deemix_arl` nicht leer ist, sonst
  sidecar. Unbekannter Wert → Warnung + `auto`.
- **Dateien:** `src/config.rs`, `src/lib.rs` (evtl. Hilfsmethode `AppState::download_backend()`).
- **Tests:** Unit in `config.rs` (Env-Parsing, Default, `auto`-Auflösung).
- **LOC:** ~40.

### US-2 — Native Session + Login-Backoff (ARL-Hygiene)
- **Akzeptanz:** `AppState.native_session` (`RwLock<Option<Arc<deezer_dl::Session>>>`);
  `ensure_native_login` liefert eine gültige Session, backofft `InvalidArl` (60→1800s), warnt bei
  leerem ARL **genau einmal**. Kein Log/Panic/Fehlertext enthält die ARL (Test).
- **Dateien:** `src/lib.rs` (AppState), `src/worker.rs` (Backoff + ensure-Fn).
- **Tests:** Unit: Backoff-Wachstum/Reset (Muster `login_backoff_grows_and_resets`); ARL-Leak-Test
  analog `deezer_dl`-Inline-Tests (Session-Debug, Fehler-Rendering).
- **LOC:** ~70.

### US-3 — `Quality`-Mapping + Tier-basierter Fallback
- **Akzeptanz:** Mapping `deemix_bitrate` (9/3/1) → `Quality::{Flac,Mp3_320,Mp3_128}`; gewünschte
  Qualität zuerst, bei `Error::NoLosslessTier` Fallback 320 → 128 (spiegelt heutiges deemix-Verhalten).
  Rückgabe enthält die **tatsächlich** verwendete `Quality` (für Log/Format).
- **Dateien:** `src/worker.rs` (Helfer `wanted_quality`/`download_native`), `src/config.rs` (Mapping).
- **Tests:** Unit für Mapping + Fallback-Kette (ohne Netz, über Error-Injektion/Trait).
- **LOC:** ~50.

### US-4 — ISRC-Tag schreiben (`tags.rs`)
- **Akzeptanz:** `tags::write_isrc(path, isrc)` setzt MP3-ID3 `TSRC` bzw. FLAC-Vorbis `ISRC`;
  `tags::read_tags` liest den Wert danach zurück. Idempotent; vorhandene Tags/ISRC unverändert.
- **Dateien:** `src/tags.rs`.
- **Tests:** Unit mit Mini-Fixtures (kleine MP3-/FLAC-Dateien unter `tests/fixtures/`), Round-Trip
  write→read; `insert_text`-API gegen lofty 0.20 absichern.
- **LOC:** ~50.

### US-5 — Nativer Download-Job im Worker
- **Akzeptanz:** In `resolve_pending` wird bei `backend=native` statt `deemix::add_to_queue` ein
  Hintergrund-Task (`spawn_native_download`, Muster `spawn_non_deezer`) gestartet: Temp-Datei
  via `download_track_to_file` (bzw. Bytes → Temp), `write_isrc`, `store_downloaded`, `mark_ready`,
  `vault_download`, `enqueue_transcode`. Row wird mit `deemix_uuid="native:<isrc>"` +
  Lease `mark_downloading`. `deezer_id` aus `track.deezer_id` (Fallback neuer Lookup).
- **Akzeptanz:** `advance_downloads` überspringt `native:`-Zeilen (kein deemix-Polling).
- **Dateien:** `src/worker.rs` (`resolve_pending`, `advance_downloads`, neue Fn).
- **Tests:** Integration in `tests/api.rs` (siehe Teststrategie).
- **LOC:** ~150.

### US-6 — Sidecar-Pfad unverändert (Regression)
- **Akzeptanz:** Mit `DOWNLOAD_BACKEND=sidecar` (oder `auto` ohne ARL) läuft `resolve_pending` →
  `deemix::add_to_queue` + `advance_downloads`-Polling **exakt wie heute**; native Jobs werden dann
  nicht gestartet. Bestehende Worker-Tests bleiben grün.
- **Dateien:** `src/worker.rs` (Verzweigung), `tests/api.rs`.
- **LOC:** ~30.

### US-7 — Run-Log: Dauer, Qualität, Quelle=native
- **Akzeptanz:** Nach erfolgreichem nativem Download erscheint genau eine `logs`-Zeile
  `"{isrc}: ready (native, {flac|mp3-320|mp3-128}, {secs:.1}s)"` (kein ARL). `source_format`
  entspricht der tatsächlich gelieferten `Quality`.
- **Dateien:** `src/worker.rs`.
- **LOC:** ~25.

---

## Betroffene Dateien (Zusammenfassung)

- `music-api/src/config.rs` — `DownloadBackend`-Enum + Env-Parsing.
- `music-api/src/lib.rs` — `AppState.native_session`.
- `music-api/src/worker.rs` — `ensure_native_login`, `download_native`/`spawn_native_download`,
  Verzweigung in `resolve_pending`, Skip in `advance_downloads`, Run-Log.
- `music-api/src/tags.rs` — `write_isrc`.
- `music-api/Cargo.toml` — unverändert (lofty 0.20 vorhanden).
- `music-api/tests/api.rs` — neue Integrationstests.
- `music-api/tests/fixtures/` — Mini-MP3/-FLAC für den Tag-Round-Trip.
- **Nicht** angefasst: `store.rs`, `vault.rs` (nur Aufruf), `deemix.rs`, `deezer_dl/**`,
  `api.rs`-Routen, DB-Schema.

---

## Teststrategie

**Unit (in-module, CI-fähig, kein Netz):**
- `config.rs`: `DownloadBackend`-Parsing/Default/`auto`-Auflösung.
- `worker.rs`: zweiter LoginBackoff (Wachstum/Reset), `wanted_quality`-Mapping, Fallback-Kette,
  `native:`-Sentinel-Skip-Logik.
- `tags.rs`: `write_isrc`→`read_tags` Round-Trip für MP3 (`TSRC`) und FLAC (`ISRC`) auf Fixtures.
- ARL-Hygiene: Fehlertext/Log/Debug der neuen Worker-Pfade enthält die ARL nicht.

**Integration (`tests/api.rs`, bestehende Harness-Struktur):**
- `resolve_pending` mit `backend=native` und **injiziertem Fake-Downloader** (Trait/Object statt
  echtem Netz) → Track landet `ready`, Datei im Store, `sat_track_download` geschrieben,
  `/isrc/{isrc}/{format}` liefert die Datei, gelesenes ISRC == erwartet.
- `advance_downloads` ignoriert `native:`-Zeilen und pollt nur Sidecar-Zeilen.
- Regression: `backend=sidecar` reproduziert das vorhandene Verhalten
  (`deemix::add_to_queue`-Aufruf, Polling) — bestehende Tests unverändert grün.
- Bestehende Tests, die geprüft bleiben müssen: `resolve_pending_respects_inflight_cap`,
  `worker_status_reports_retry_cap`, `transcode_item`-Tests, Vault-Analyse-Tests.
- **Netz-Tests** (echter ARL, 128/320/flac) bleiben **manuell** auf `planet` (nicht CI) — Matrix wie
  im Gesamtplan; Ergebnis dokumentieren.

**Nicht-Ziele der Tests:** echte Deezer-Calls in CI, echte ffmpeg-Transcodes in CI.

---

## Risiken

1. **lofty-0.20-API-Drift** (`insert_text(ItemKey::Isrc)`): Signatur exakt verifizieren; falls nötig
   `Tag::insert_text` vs. `push`/`insert` wählen. Abgefedert durch Round-Trip-Test.
2. **Qualitäts-Fallback**: `deezer_dl` kennt bewusst keinen Fallback (M1) → M2 muss ihn explizit
   implementieren, sonst bleiben Tracks ohne Lossless-Tier hängen.
3. **ARL-Leak**: jeder neue Log-/Fehlerpfad ist eine potenzielle Leckstelle → dedizierter Test.
4. **Sidecar-Regression**: `DEEMIX_URL`-Default macht „gesetzt" unzuverlässig → expliziter
   `DOWNLOAD_BACKEND`; Sidecar-Regressionstest Pflicht.
5. **Row-Zustand bei nativen Jobs**: Without Polling darf die Lease nicht ablaufen, bevor der
   Hintergrund-Task fertig ist → Lease beim Start + Refresh im Task (Muster `spawn_non_deezer`),
   Reaper-Frist `WORK_STALE_SECS` beachten.
6. **Doppelte Finalisierung**: `finalizing`-Guard (vorhanden) auch für native Jobs nutzen.
7. **Temp-Datei-Hygiene**: Temp nach Copy löschen (Muster `finalize`).

---

## Definition of Done (M2)

- [ ] `DOWNLOAD_BACKEND` (`native|sidecar|auto`, default `auto`) implementiert + getestet.
- [ ] Native Session mit Login-Backoff; **ARL nie geloggt** (Test grün).
- [ ] `Quality`-Mapping + `NoLosslessTier`-Fallback (320→128).
- [ ] `tags::write_isrc` setzt `TSRC` (MP3) / `ISRC` (FLAC); Round-Trip-Test grün.
- [ ] Ein Deezer-Track landet nativ mit korrektem ISRC im Store und ist über
      `/isrc/{isrc}/{format}` abrufbar (Integrationstest mit injiziertem Downloader).
- [ ] `advance_downloads` pollt keine `native:`-Zeilen; Sidecar-Pfad unverändert.
- [ ] Run-Log zeigt Dauer + Qualität + Quelle=`native` (ohne ARL).
- [ ] **Bestehende Worker-Tests grün** (`resolve_pending_respects_inflight_cap` u. a.).
- [ ] Manuelle Live-Matrix (128/320/flac, echter ARL) auf `planet` dokumentiert.
- [ ] Kein Produktionscode außerhalb des Planscopes; keine DB-Migration.

---

## Offene Fragen

1. **`DOWNLOAD_BACKEND`-Default**: `auto` (native bevorzugt) vs. konservativ `sidecar`?
   Vorschlag `auto`, damit #510 den Betrieb sofort ohne Docker ermöglicht.
2. **`deemix_bitrate` als Qualitätsquelle** beibehalten oder eigenes `DOWNLOAD_QUALITY` einführen?
   Vorschlag beibehalten (ein Schalter, weniger Verwirrung); Umbenennung in M3.
3. **`native:`-Sentinel** vs. dediziertes `tracks.backend`-Feld — Sentinel gewählt (keine Migration);
   falls M3 ein Feld braucht, dort migrieren.
4. **Installierte Asset-Dateien (Cover)** von Rusteer ignorieren (Store ist ISRC-keyed) — bestätigen?
5. **Deployment-Hinweis für #473** (Tier-Gate) ist im Gesamtplan als Nebenbefund notiert, aber
   nicht Teil von #510 — separates Ticket nötig?
