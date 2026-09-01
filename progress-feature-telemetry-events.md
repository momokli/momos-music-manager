# Progress: feature/telemetry-events

**Repo:** https://github.com/momokli/momos-music-manager
**Branch:** feature/telemetry-events (Worktree: /home/node/repos/mmm-telemetry-events, Basis: origin/main @ 25f15e2 / PR #16)
**Ziel:** PR gegen main — Telemetry-Feature (Core-Events, Multi-Client, HTTPS-Batch-Push, Server-Ingest mit eigener telemetry.db, SQL-Views, Konzept-Doc, Tests). KEIN Versions-Bump; CHANGELOG-Eintrag unter Unreleased gemäß docs/versioning.md.
**Auslieferung:** später v1.2.0 (nicht Teil dieses PRs).

## Anforderungen (fixiert)
1. Core-Events: Tasks (Start/Ende/Fehler/Dauer), Scan-Läufe (Dauer, Files, Quelle), Downloads (Deemix/Spotify), Fehler. KEINE UI-Actions, KEINE Heartbeats.
2. Multi-Client: stabile Client-ID (einmalig generiert, persistiert), app_version + os je Event.
3. Transport: HTTPS-POST `POST /api/telemetry` (Batch), Bearer-Token, client-seitiger Ringbuffer + Async-Flusher, Retry/Exponential-Backoff, offline-sicher (Events überleben Neustart). Konfigurierbarer Endpoint + Disable-Flag.
4. Server-Ingest: Auth/Validierung, Dedup via event_id (Idempotenz), Retention/Prune, eigene `telemetry.db` (SQLite, sqlx-konform zu migrations/).
5. SQL-Views: Tasks/Stunde je Client, Fehlerrate, Downloads je Quelle, Scan-Dauer-Trend, Versionen je Client, zuletzt gesehen je Client.
6. Sicherheit: keine Secrets/Dateinamen-PII in Payloads; TLS über Caddy; Token aus Config/Env.
7. Konzept-Doc: `plans/proposed/telemetry-events.md` (Format wie `plans/_TEMPLATE.md` / `battery-friendly-tray.md`) inkl. CRITIQUE-Sektion.
8. Tests: Event-Serialisierung, Ingest+Dedup+SQL (Integration), Backoff, Ringbuffer.

**Repo-Kontext:** Rust-Backend, SQLite via sqlx mit migrations/-Nummernschema, Task-History-Persistenz existiert (andocken!), Config in src/config.rs, Autoupdater in src/autoupdate/ (Update-Apply = Versionswechsel-Event), deploy/ hat systemd+Caddy für LAN. ACHTUNG: Workspace-Clone (/home/node/.openclaw/workspace/momos-music-manager) hat uncommitted Arbeit (feat/battery-friendly-tray) — NIE anfassen. Worktree /home/node/repos/mmm-telemetry-events ist die einzige Arbeitsfläche.

## Pipeline-Status

| # | Stage | Status | Notizen |
|---|-------|--------|---------|
| 1 | feature-dev-planner | DONE | 9 User Stories + Konzept-Doc-Entwurf, siehe Stage-1-Sektion |
| 2 | feature-dev-setup | DONE | Merge origin/main (PR #17) → e49eff6, keine Konflikte; Baseline grün (cargo check/build/test EXIT 0, 487 Unit + Integration + migration_integrity ok); höchste Migration 023; Cargo.lock unverändert; Toolchain-Hinweis: ~/.cargo/bin/cargo 1.98.0 nötig (System-cargo zu alt für edition 2024) |
| 3 | feature-dev-developer | IN PROGRESS (Run 3) | Code + Tests; INKREMENTELL committen (nach jedem Code-Block), damit Arbeit Run-Abstürze überlebt |
| 4 | feature-dev-verifier | PENDING | Quality Gate |
| 5 | feature-dev-tester | PENDING | Integration/E2E |
| 6 | feature-dev-developer | PENDING | PR erstellen |
| 7 | feature-dev-reviewer | PENDING | Final Review |

## Stage 1 — Planner (feature-dev-planner, 2026-09-01)

### Wichtigste Analyse-Erkenntnisse (Ground Truth)

- **Bestehende Telemetrie (snapshot-basiert, `feat/telemetry-analytics`, DONE) bleibt unangetastet**: `src/telemetry/` mit `mod.rs` (Push: `VACUUM INTO`-db.sqlite + Meta), `receiver.rs` (axum-Router, Datei-Ablage `base_dir/<instance>/<ts>/`, Bearer-Auth, konstantzeitiger Vergleich), `metrics.rs`. Receiver ist eigenständige Binary-Invokation `telemetry receive` (systemd `momos-telemetry.service`, Loopback 127.0.0.1:8330, Caddy `telemetry.music.klimk.es` → TLS).
- **Event-Telemetrie ist additiv**: neue Endpoints auf demselben Receiver (`POST /api/telemetry`), neue Client-Module `src/telemetry/events.rs` (+ `spool.rs`, `flusher.rs`, `emit.rs`, `client_id.rs`). Bestehende Routen `/api/telemetry/{instance}/db/{ts}` (PUT) und `/api/telemetry/{instance}/meta/{ts}` (POST) bleiben unverändert — kein Pfadkonflikt mit `POST /api/telemetry`.
- **Task-History (Migration 022, `task_history`-Tabelle)** ist der zentrale Andockpunkt für Task-Events: `TaskManager` in `src/tasks/mod.rs` (3899 Zeilen), Persistenz via `maybe_persist` → `persist_task_to_db` (Zeile ~694/958). TaskType-Enum hat bereits `TelemetryPush`.
- **Scan-Läufe**: `src/db/folders.rs::scan_folder` (Zeile 280) + `scan_cache.rs`; ScanFolder-Task läuft über TaskManager (Task-Events decken Start/Ende/Dauer ab, Scan-Payload ergänzt Files/Quelle).
- **Downloads**: `src/download_guarantor.rs` (Deemix-Queue-Sync + Gap-Remediation mit spotDL-Fallback), `src/deemix/models.rs` (Download-Status), Tabelle `deemix_downloads`.
- **Autoupdater**: `src/autoupdate/verify.rs::apply_update` → `ApplyOutcome::Installed { .. }` (Zeile ~360) = Versionswechsel-Event `app.updated` (from→to). `env!("MMM_VERSION")` überall verfügbar.
- **Migrations**: sqlx `migrate!()` eingebettet zur Compile-Zeit (`src/db/connection.rs::init_db`); Hauptkette endet bei `023` → nächste Nummer wäre `024` (Hauptkette bleibt aber UNVERÄNDERT). Kein `.sqlx`-Offline-Cache im Repo; es werden nur Laufzeit-`sqlx::query` verwendet (keine `query!`-Makros) → kein Offline-Modus nötig.
- **Migration-Integrität**: `tests/migration_integrity.rs` prüft die Hauptkette (bleibt grün, da Hauptkette unverändert). Für telemetry.db: **eigene Migrationskette** `migrations/telemetry/001_events.sql`, eingebettet via `sqlx::migrate!("migrations/telemetry")` — `_sqlx_migrations`-Tabelle ist pro DB, kein Konflikt.
- **Config**: `[telemetry]`/`[telemetry_receiver]` existieren (`src/config.rs`); Env-Priorität Env > TOML > Default; Default `enabled=false`. Neue Keys: `telemetry.events_endpoint` (+ Env `MOMOS_TELEMETRY_EVENTS_ENDPOINT`) und `telemetry_receiver.retention_days` (+ Env) + `telemetry_receiver.db_path`.
- **Versioning**: CHANGELOG-Eintrag unter `## [Unreleased]` → `### Added`; KEIN Versions-Bump (Runbook in docs/versioning.md, Release erst beim Tag).
- **Repo-Konventionen**: Migrations additiv (nie 001 editieren), same-release-Migrationen vor Release konsolidieren; `cargo test` ist Single Source of Truth; Coverage-Kultur (100%-Pläne existieren); `tests/common/mod.rs`-Helper; tower-`oneshot`-Testpattern (siehe `receiver.rs`-Tests).

### Empfohlenes Event-Schema (Vorschlag, nicht final)

Batch-Envelope, HTTPS-POST auf `POST /api/telemetry`:

```json
{
  "client_id": "3f2a…-uuid",
  "sent_at": "2026-09-01T00:42:00Z",
  "events": [
    {
      "event_id": "8f14e45f-…uuid-v4…",      // Dedup-Key (Server-Idempotenz)
      "client_id": "3f2a…-uuid",             // stabil, einmalig generiert, persistiert
      "app_version": "1.1.0-dev+4eaa1d93",   // env!("MMM_VERSION")
      "os": "macos",                          // macos | linux | windows (+ "arch" im Payload optional)
      "ts": "2026-09-01T00:41:59Z",          // ISO8601 UTC, Client-Uhr
      "type": "task.completed",               // Punkt-Notation, Allowlist
      "payload": { "task_type": "ScanFolder", "status": "completed", "duration_ms": 12400 }
    }
  ]
}
```

Antwort: `202 Accepted` `{"accepted": n, "duplicates": m}` · `400` Validierungsfehler · `401` Auth.
Event-Typen (Initial, keine UI-Actions/Heartbeats): `task.started`, `task.completed`, `task.failed`, `scan.completed`, `download.started`, `download.completed`, `download.failed`, `app.updated`, `error.reported`.

### Empfohlenes SQL-Schema (Server, telemetry.db — Vorschlag)

```sql
CREATE TABLE clients (
    client_id       TEXT PRIMARY KEY,
    first_seen_at   INTEGER NOT NULL,   -- unixepoch
    last_seen_at    INTEGER NOT NULL,
    last_app_version TEXT,
    last_os         TEXT
);
CREATE TABLE events (
    event_id    TEXT PRIMARY KEY,       -- Dedup via INSERT OR IGNORE
    client_id   TEXT NOT NULL REFERENCES clients(client_id),
    type        TEXT NOT NULL,
    ts          INTEGER NOT NULL,       -- unixepoch (Client-Zeit)
    received_at INTEGER NOT NULL,       -- unixepoch (Server-Zeit, Basis Retention)
    app_version TEXT NOT NULL,
    os          TEXT NOT NULL,
    payload     TEXT NOT NULL           -- JSON-Objekt
);
CREATE INDEX idx_events_ts         ON events(ts);
CREATE INDEX idx_events_client_ts  ON events(client_id, ts);
CREATE INDEX idx_events_type_ts    ON events(type, ts);
```

Retention: `DELETE FROM events WHERE received_at < unixepoch() - retention_days*86400` (default 30 Tage), Prune bei Ingest + periodisch. Clients werden implizit via `INSERT OR IGNORE` + `UPDATE last_seen_at` registriert.

### User Stories (Implementierungs-Reihenfolge)

1. **Event-Modell + Serialisierung** (`src/telemetry/events.rs` neu)
   - AC: Typen `TelemetryEvent` + `EventBatch` (serde, wie oben); `event_id` = uuid v4, Unique in Batch; `ts` ISO8601-UTC parsebar; Typ-Allowlist + Payload-Größen-Cap; Roundtrip-Serialisierung; keine Secrets/Pfade im Payload (Sanity-Check); Unit-Tests.
2. **Client-ID-Persistenz + Config** (`src/telemetry/client_id.rs` neu, `src/config.rs`, `src/lib.rs`)
   - AC: client_id einmalig generiert, in Data-Dir persistiert (`~/.local/share/momos-music-manager/telemetry-client-id`), stabil über Neustarts; neue Config `telemetry.events_endpoint` (Default: `base_url` + `/api/telemetry`) + Env `MOMOS_TELEMETRY_EVENTS_ENDPOINT`; Disable-Flag = bestehendes `telemetry.enabled` (Default false → kein Verhaltenswechsel); Env > TOML > Default getestet.
3. **Ringbuffer + Spool + Async-Flusher + Backoff** (`src/telemetry/buffer.rs`/`spool.rs`/`flusher.rs` neu, `src/main.rs` Spawn in `serve()`)
   - AC: begrenzter Ringbuffer (z. B. 10k, drop-oldest + warn); JSONL-Spool-Datei (append-only, crash-sicher) im Data-Dir; beim Start Spool → Buffer laden; Flusher zieht Batches (≤ 200 Events/~1 MB), erfolgreiche Flush → Spool-Truncate; Fehler → Exponential-Backoff (30s → 60s → … Cap 1h, mit Jitter); 4xx vs 5xx (4xx: nach N Versuchen droppen); Shutdown-Drain; Offline-Überleben (Neustart) per Test.
4. **Event-Emitters andocken** (`src/telemetry/emit.rs` neu, `src/tasks/mod.rs`, `src/db/folders.rs`, `src/download_guarantor.rs`, `src/deemix/*`, `src/autoupdate/verify.rs`)
   - AC: nicht-blockierendes `telemetry::emit(...)` (mpsc, nie Panic, nie Task-Flow blockieren); Task-Events in `TaskManager` (start/completed/failed inkl. Dauer via `maybe_persist`); `scan.completed` in `scan_folder` (Dauer, Files, Quelle); `download.*` in DownloadGuarantor/Deemix (Quelle deemix|spotdl); `app.updated` bei `ApplyOutcome::Installed`; Fehler via `task.failed` + `error.reported`-Hook an Catch-Punkten; Payload-Hygiene: error_message sanitieren (Home-Prefix strippen, truncaten), keine Dateinamen; disabled → Null-Overhead.
5. **Server-Ingest: telemetry.db + Auth + Dedup + Retention** (`migrations/telemetry/001_events.sql` neu, `src/telemetry/receiver.rs`, `src/config.rs`, `src/main.rs`, `deploy/momos-telemetry.service`)
   - AC: Receiver öffnet eigene telemetry.db (Pfad via Config/Env, Default im Analytics-Dir) und führt `sqlx::migrate!("migrations/telemetry")` aus; `POST /api/telemetry` mit Bearer-Auth (konstantzeitiger Vergleich wie bisher), Validierung (Schema, event_id/client_id-Format, Typ-Allowlist, Batch-Größe); Dedup via event_id (INSERT OR IGNORE, Zähler Duplikate); Clients-Upsert; Retention-Prune (on ingest + periodisch, `retention_days` default 30); Antwort `202 {accepted, duplicates}`; alte Snapshot-Routen funktionieren weiter; Integrationstest (tower oneshot + temp telemetry.db).
6. **SQL-Views** (`migrations/telemetry/001_events.sql` erweitern oder `002_views.sql`)
   - AC: Views `v_tasks_per_hour` (Tasks/Stunde je Client), `v_error_rate` (Fehlerrate je Client/Tag), `v_downloads_by_source`, `v_scan_duration_trend`, `v_client_versions`, `v_clients_last_seen`; Integrationstest mit geseedeter telemetry.db prüft Aggregationen; Konsolidierungs-Regel (AGENT.md): same-release-Migrationen → bevorzugt EINE `001_events.sql` (Tabellen + Indexe + Views).
7. **Tests (Integration)** (`tests/telemetry_events.rs` neu, ggf. `tests/common/mod.rs`)
   - AC: E2E emit → spool → flush → ingest → dedup → views mit temp Dirs + in-memory/telemetry.db; Serialisierung-, Backoff-, Ringbuffer-Tests (aus US1/US3 verankert); `cargo test` grün; neue Module nach Coverage-Kultur getestet.
8. **Konzept-Doc** (`plans/proposed/telemetry-events.md`, `plans/README.md`)
   - AC: Format nach `plans/_TEMPLATE.md` + Stil von `plans/done/telemetry-analytics.md`; Sektionen: Description, Topologie, Event-/SQL-Schema, Config, Endpoints, Views, Retention, Phasen, Acceptance Criteria, Out of Scope, **CRITIQUE**; Status proposed, Branch `feat/telemetry-events`.
9. **CHANGELOG + Doku** (`CHANGELOG.md`, optional `docs/DECISIONS.md` ADR, `.env.example`, `deploy/config.toml`)
   - AC: Eintrag unter `## [Unreleased]` → `### Added` (Event-Telemetrie: Events, Client-ID, Batch-Push, Server-Ingest, Views, Doc); KEIN Versions-Bump (docs/versioning.md); Doku-Kommentare für neue Env/TOML-Keys.

### Abhängigkeiten

```
US1 (Event-Modell) ──► US2 (Client-ID+Config) ──► US3 (Buffer/Spool/Flusher) ──► US4 (Emitters)
   │                        │                                                │
   └──────── US5 (Server-Ingest; braucht nur das Wire-Schema aus US1) ◄───────┘
                     │
                     ▼
                  US6 (Views) ──► US7 (Integrationstests) ──► US9 (CHANGELOG/Doku)
                                   US8 (Konzept-Doc, ab US1 skizzierbar, nach US6 finalisieren)
```

- Sequenziell: US1 → US2 → US3 → US4 (Client-Pipeline), US5 → US6 → US7 (Server). US5 kann nach US1 parallel zu US2–US4 laufen (Wire-Schema fixiert); US8 kann früh entworfen, muss aber nach US6 finalisiert werden; US9 ans Ende.
- Empfohlene Aufteilung für Developer: Agent A = Client (US1–US4), Agent B = Server (US5–US6), Agent C = Tests+Doc (US7–US9) — disjunkt bis auf `src/config.rs` (A und B): Config-Änderungen zuerst (US2) committen oder Teilung vorab abstimmen.

### Risiken / Hinweise für Developer

- **sqlx-Offline-Modus**: nicht nötig — Repo nutzt nur Laufzeit-`sqlx::query`, keine `query!`-Makros; kein `.sqlx`-Cache. Aber: `sqlx::migrate!()` (bzw. `sqlx::migrate!("migrations/telemetry")`) embeddet zur **Compile-Zeit** → neue Migrationsdatei MUSS vor `cargo build` existieren.
- **Migrations-Numbering**: Hauptkette bleibt unverändert (nächste wäre 024 — NICHT anlegen). Telemetry-Kette eigenes Verzeichnis `migrations/telemetry/` mit eigener `001_…`-Nummerierung; `_sqlx_migrations` ist pro DB → kein Konflikt. `tests/migration_integrity.rs` bleibt damit grün; optional Telemetry-Kette dort mitprüfen.
- **init_db unangetastet**: `src/db/connection.rs::init_db` migriert nur die Haupt-DB; der Receiver migriert telemetry.db separat (eigene Funktion, z. B. `telemetry::receiver::init_telemetry_db`).
- **Routing/axum 0.8**: neue Route `POST /api/telemetry` auf dem Receiver-Router; bestehende Routen behalten; Pfadsyntax `{param}`; Body-Limit explizit setzen (DefaultBodyLimit; Client hält Batches ≤ ~1 MB).
- **Emit nie blockierend**: mpsc-Kanal statt direkter I/O; `emit` darf nicht panicken (try + warn); Task-Flow (TaskManager) darf durch Telemetrie nie gestört werden.
- **Payload-Hygiene**: `task_history.error_message` kann absolute Pfade enthalten → sanitieren (Home-Dir strippen, Kürzen auf z. B. 500 Zeichen); niemals Tokens/Secrets loggen oder senden (Redaction-Kultur aus `RedactedConfig` übernehmen); kein `instance`-Hostname-Folder nötig (client_id ersetzt ihn).
- **Clock Skew**: `ts` = Client-Zeit (ISO8601); Retention basiert auf `received_at` (Server-Zeit). Keine Heartbeats — "zuletzt gesehen" aus Events ableiten.
- **Backoff-Disziplin**: Flusher muss Batch erst nach 2xx-ACK verwerfen; bei 4xx (z. B. 400) nach N Versuchen droppen (sonst Endlos-Retry); Caddy/gzip ist transparent für den Batch-Endpoint.
- **Config-Defaults**: alles aus (events via `telemetry.enabled` false default) → kein Verhaltenswechsel für Bestandsnutzer; Snapshot-Telemetrie (alt) bleibt unberührt.
- **Deploy**: `momos-telemetry.service` braucht ggf. Env für `MOMOS_TELEMETRY_RECEIVER_DB_PATH` (Drop-in, kein Secret); Caddy-Snippet deckt den neuen Endpoint automatisch ab (gleiche Origin).
- **Konventionen**: `cargo test` = Source of Truth (test.sh); tower-`oneshot`-Pattern aus `receiver.rs`-Tests wiederverwenden; tempfile + temp Dirs; Coverage-Kultur des Repos (100%-Pläne) → neue Module vollständig testen.
- **Arbeitsfläche**: nur `/home/node/repos/mmm-telemetry-events`; Workspace-Clone (`/home/node/.openclaw/workspace/momos-music-manager`) NIE anfassen.

### Referenzen (gelesen)

`progress-feature-telemetry-events.md`, `plans/_TEMPLATE.md`, `plans/done/telemetry-analytics.md` (Format-Vorbild, da `battery-friendly-tray.md` nur im Workspace-Clone liegt), `docs/versioning.md`, `src/config.rs`, `src/tasks/mod.rs` (TaskManager, persist_task_to_db), `src/telemetry/{mod,receiver,metrics}.rs`, `src/db/connection.rs`, `src/autoupdate/{mod,swap,verify}.rs`, `src/download_guarantor.rs`, `src/deemix/models.rs`, `src/db/folders.rs`, `migrations/022_task_history.sql`, `CHANGELOG.md`, `deploy/*`, `Cargo.toml`, `tests/migration_integrity.rs`, `AGENT.md`.

## Stage 2 — Setup (feature-dev-setup, 2026-09-01)

- Merge origin/main @ 03ed42e (PR #17) via `--no-ff` → HEAD `e49eff6`; betraf nur src/deemix/client.rs + src/poller.rs; keine Konflikte.
- Baseline: `cargo check --all-targets` ✅ (nur bestehende Warnings), `cargo build` Exit 0, `cargo test` Exit 0 (alle Suiten, 0 failed/panics, migration_integrity ok).
- Migrations-Hauptkette 001–023 (höchste: 023); `tests/migration_integrity.rs` existiert; `src/telemetry/` existiert (mod.rs, receiver.rs, metrics.rs).
- Worktree clean außer ungetracktem `plans/proposed/telemetry-events.md`. Kein Push.
- **Toolchain-Warnung für alle Folgestages:** `~/.cargo/bin/cargo` (1.98.0) verwenden; System-cargo (`/usr/bin/cargo`) ist zu alt für edition 2024.

## Stage-Ergebnisse

## Retry-Zähler
- Developer-Retries: 0/2
