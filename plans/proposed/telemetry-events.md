# Plan: telemetry-events

**Status**: proposed
**Branch**: `feat/telemetry-events`
**Ready for review**: no
**Depends on**: bestehende Telemetrie (`feat/telemetry-analytics`, done) — additiv, kein Umbau; `task_history` (Migration 022), `MMM_VERSION` (build.rs), axum + reqwest + sqlx vorhanden
**Migration needed**: ja — **neue eigene Kette** `migrations/telemetry/001_events.sql` für die Server-`telemetry.db` (Hauptkette bleibt unverändert; kein `024`)

### Description

Ereignisbasierte Telemetrie ergänzend zum bestehenden Snapshot-Push: Clients (eine
oder mehrere Instanzen) senden **strukturierte Core-Events** (Tasks, Scan-Läufe,
Downloads, Fehler, App-Updates) als **HTTPS-Batch** an einen Server-Ingest, der sie
in einer eigenen `telemetry.db` (SQLite, sqlx) ablegt — mit Bearer-Auth,
Idempotenz-Dedup über `event_id` und Retention. SQL-Views liefern die Auswertung
(Tasks/Stunde, Fehlerrate, Downloads je Quelle, Scan-Dauer-Trend, Versionen,
zuletzt gesehen). Kein Dashboard in diesem PR; Views + Konzept-Doc sind die
Auswertungsfläche. Strikte Grenzen: **keine UI-Actions, keine Heartbeats, keine
Secrets/Dateinamen-PII in Payloads**.

### Topologie

```
momos-music-manager (1..n Instanzen)         LAN Server / Collector
──────────────────────────────────            ──────────────────────────────
[telemetry] enabled = true                    Caddy: telemetry.music.klimk.es
  Event-Emitters (Tasks, Scan, …)              └─ reverse_proxy 127.0.0.1:8330
    │ ringbuffer (10k, drop-oldest)                 │
    ▼ spool (JSONL, data-dir, crash-safe)           ▼ momos-telemetry (systemd)
  Async-Flusher ──POST /api/telemetry──►  Receiver (axum)
  Batch ≤200/≤1 MB, Bearer-Token,               ├─ Auth (constant-time Bearer)
  Retry + Exponential-Backoff                   ├─ Validierung (Schema/Allowlist)
  (offline-sicher über Neustart)                ├─ Dedup: events.event_id PK
                                                ├─ telemetry.db (sqlx-Migration)
                                                └─ Retention: received_at < now - retention_days
```

Die bestehenden Snapshot-Routen (`PUT /api/telemetry/{instance}/db/{ts}`,
`POST /api/telemetry/{instance}/meta/{ts}`) bleiben unverändert funktionsfähig;
`POST /api/telemetry` ist additiv. `client_id` ersetzt das bisherige
`instance`-String-Konzept für Events (pro-Installation-stabil, kein Hostname-PII).

### Event-Schema (Vorschlag, wird in Stage 3 finalisiert)

Batch-Envelope:

```json
{
  "client_id": "3f2a…-uuid-v4",
  "sent_at": "2026-09-01T00:42:00Z",
  "events": [
    {
      "event_id": "8f14e45f-…uuid-v4…",
      "client_id": "3f2a…-uuid-v4",
      "app_version": "1.1.0-dev+4eaa1d93",
      "os": "macos",
      "ts": "2026-09-01T00:41:59Z",
      "type": "task.completed",
      "payload": { "task_type": "ScanFolder", "status": "completed", "duration_ms": 12400 }
    }
  ]
}
```

- `event_id`: uuid v4, **Dedup-Key** (Server: `INSERT OR IGNORE` → Idempotenz bei Retry).
- `client_id`: einmalig generiert, persistiert (Data-Dir); stabil über Neustarts/Updates.
- `app_version`: `env!("MMM_VERSION")` — erfüllt „Versionen je Client"-View.
- `os`: `macos | linux | windows` (+ optional `payload.arch`).
- `ts`: ISO8601 UTC (Client-Uhr); Server speichert zusätzlich `received_at`.
- `type`: Punkt-Notation, Server-Allowlist.
- `payload`: typ-spezifisch, **redigiert** (keine Pfade, keine Secrets, `error_message` gesäubert/gekürzt).

**Event-Typen (Initial):** `task.started`, `task.completed`, `task.failed`,
`scan.completed` (Dauer, Files, Quelle), `download.started`, `download.completed`,
`download.failed` (Quelle deemix|spotdl), `app.updated` (from→to, Autoupdater),
`error.reported` (Fehler-Hook an Catch-Punkten). Keine Heartbeats, keine UI-Actions.

### SQL-Schema (Server `telemetry.db`, Vorschlag)

```sql
CREATE TABLE clients (
    client_id        TEXT PRIMARY KEY,
    first_seen_at    INTEGER NOT NULL,
    last_seen_at     INTEGER NOT NULL,
    last_app_version TEXT,
    last_os          TEXT
);

CREATE TABLE events (
    event_id    TEXT PRIMARY KEY,
    client_id   TEXT NOT NULL REFERENCES clients(client_id),
    type        TEXT NOT NULL,
    ts          INTEGER NOT NULL,
    received_at INTEGER NOT NULL,
    app_version TEXT NOT NULL,
    os          TEXT NOT NULL,
    payload     TEXT NOT NULL
);

CREATE INDEX idx_events_ts        ON events(ts);
CREATE INDEX idx_events_client_ts ON events(client_id, ts);
CREATE INDEX idx_events_type_ts   ON events(type, ts);
```

Retention: `DELETE FROM events WHERE received_at < unixepoch() - ?` mit
`retention_days` (Default 30), Prune bei jedem Ingest + periodisch (z. B.
stündlich). Clients: `INSERT OR IGNORE` + `UPDATE last_seen_at/last_app_version/last_os`
(= „zuletzt gesehen" ohne Heartbeat).

### SQL-Views

| View | Aggregation |
|---|---|
| `v_tasks_per_hour` | Events `task.*` je `client_id`, Stunde (`strftime('%Y-%m-%d %H', ts, 'unixepoch')`), je `task_type` |
| `v_error_rate` | je `client_id` + Tag: `task.failed` / `task.completed`-Menge |
| `v_downloads_by_source` | `download.*` je Quelle (deemix/spotdl) |
| `v_scan_duration_trend` | `scan.completed` je Tag: avg/p95 `duration_ms` |
| `v_client_versions` | `clients` (aktuelle `last_app_version` je Client) |
| `v_clients_last_seen` | `clients` (`last_seen_at`) |

### Config (additiv zu bestehender `[telemetry]`/`[telemetry_receiver]`-Sektion)

```toml
[telemetry]
enabled = true                  # Master-Flag (bestehend) — Disable-Flag für Events
events_endpoint = "https://telemetry.music.klimk.es/api/telemetry"  # Default: base_url + /api/telemetry

[telemetry_receiver]
db_path = "/home/momo/momos-analytics/telemetry.db"   # Default: <base_dir>/telemetry.db
retention_days = 30
```

Env: `MOMOS_TELEMETRY_EVENTS_ENDPOINT`, `MOMOS_TELEMETRY_RECEIVER_DB_PATH`,
`MOMOS_TELEMETRY_RECEIVER_RETENTION_DAYS`. Priorität Env > TOML > Default; alles
aus per Default (`enabled=false`) → kein Verhaltenswechsel.

### Endpoints (Receiver)

| Endpoint | Methode | Payload | Effekt |
|---|---|---|---|
| `/health` | GET | — | `ok` (bestehend) |
| `/api/telemetry/{instance}/db/{ts}` | PUT | db.sqlite-Stream | Snapshot-Ablage (bestehend) |
| `/api/telemetry/{instance}/meta/{ts}` | POST | MetaPayload | Meta-JSON (bestehend) |
| `/api/telemetry` | POST | EventBatch (JSON) | Ingest in `telemetry.db`; `202 {accepted, duplicates}` |

Auth: `Authorization: Bearer <token>` (konstantzeitiger Vergleich, bestehende
Implementierung). Ohne/falsch → 401; Validierungsfehler → 400; nicht-2xx vom
Server → Client-Backoff.

### Neue / geänderte Dateien

| Datei | Änderung |
|---|---|
| `src/telemetry/events.rs` | **neu**: `TelemetryEvent`, `EventBatch`, Serialisierung, Validierung, Ringbuffer |
| `src/telemetry/client_id.rs` | **neu**: Client-ID generieren/persistieren/laden |
| `src/telemetry/spool.rs` | **neu**: JSONL-Spool (append, crash-sicher), Laden beim Start |
| `src/telemetry/flusher.rs` | **neu**: Async-Flusher, Batch-Building, Backoff, ACK-Handling |
| `src/telemetry/emit.rs` | **neu**: nicht-blockierende `emit()`-API (mpsc) + Payload-Hygiene-Helfer |
| `src/telemetry/mod.rs` | Submodule re-exportieren; `init_telemetry_db` für Receiver |
| `src/telemetry/receiver.rs` | Route `POST /api/telemetry`, telemetry.db-Ingest, Dedup, Retention |
| `src/config.rs` | `events_endpoint`, `receiver.db_path`, `receiver.retention_days` (+ Env) |
| `src/main.rs` | Flusher-Spawn in `serve()`; Receiver-Init |
| `src/tasks/mod.rs` | Task-Events in `TaskManager`/`persist_task_to_db` (emit, nie blockierend) |
| `src/db/folders.rs` | `scan.completed`-Event (Dauer, Files, Quelle) |
| `src/download_guarantor.rs`, `src/deemix/*` | `download.*`-Events (Quelle) |
| `src/autoupdate/verify.rs` | `app.updated` bei `ApplyOutcome::Installed` |
| `migrations/telemetry/001_events.sql` | **neu**: Tabellen + Indexe + Views (eigene Kette) |
| `tests/telemetry_events.rs` | **neu**: Integrationstests (Ingest+Dedup+Views, Serialisierung, Backoff, Ringbuffer) |
| `deploy/momos-telemetry.service` | Env-Doku für `db_path`/`retention_days` (Drop-in, kein Secret) |
| `CHANGELOG.md` | Eintrag unter `[Unreleased]` → `Added` (KEIN Versions-Bump) |
| `plans/proposed/telemetry-events.md` | dieses Dokument (finalisiert in Stage 3) |

### Phasen (inkrementell, jede testbar)

| Phase | Lieferung | Validierung |
|---|---|---|
| 1. Event-Modell + Client-ID + Config | `events.rs`, `client_id.rs`, Config-Keys | Unit-Tests (Roundtrip, Stabilität, Env>TOML) |
| 2. Buffer/Spool/Flusher | Ringbuffer, JSONL-Spool, Backoff-Flusher | Unit-Tests (Backoff-Folge, Spool-Überleben, Truncate nach ACK) |
| 3. Emitters | Task/Scan/Download/Autoupdater/Fehler andocken | Tests: genau 1 Event je Lifecycle, keine PII |
| 4. Server-Ingest | telemetry.db + Migration + `POST /api/telemetry` | tower-oneshot-Tests (401/400/202, Dedup, Retention) |
| 5. Views | 6 Views in Migration | Seed-Test prüft Aggregationen |
| 6. Doku + CHANGELOG | Konzept-Doc (CRITIQUE), CHANGELOG Unreleased | Review |

### Acceptance Criteria

- [ ] `cargo build` passes
- [ ] `cargo test` passes (bestehende + neue Tests; `tests/migration_integrity.rs` bleibt grün)
- [ ] Client: stabile, persistierte Client-ID; `event_id`-Dedup-Key je Event
- [ ] Events überleben Neustart (Spool) und werden nach 2xx-ACK verworfen; Backoff-Folge korrekt
- [ ] Emitters feuern je Lifecycle genau einmal; Payloads enthalten keine Secrets/Pfade (getestet)
- [ ] `POST /api/telemetry` nimmt Batches an (202), lehnt Auth-/Validierungsfehler ab (401/400)
- [ ] Dedup: doppelter Batch (Retry) erzeugt keine Duplikate; Antwort zählt `duplicates`
- [ ] Retention prunet Events älter als `retention_days`
- [ ] Alle 6 Views liefern korrekte Aggregationen (Seed-Integrationstest)
- [ ] CHANGELOG-Eintrag unter `[Unreleased]`; **kein** Versions-Bump
- [ ] Bestehende Snapshot-Telemetrie (PUT/POST `/api/telemetry/{instance}/…`) unverändert grün

### Out of Scope (erste Iteration)

- Dashboard/UI (Views + Doc sind die Auswertungsfläche; openclaw/`sqlite3` lesen direkt)
- Heartbeats, UI-Actions, Play-/Tag-Statistik (bewusst ausgeschlossen)
- Rate-Limiting / Quota pro Client (Follow-up; Dedup + Auth decken Missbrauch grob ab)
- Event-Telemetrie für den Python-`download-service` (eigene Pipeline, Follow-up)
- Migration der Alt-`instance`-Konzepte (Snapshot-Push bleibt separat)
- gzip im App-Client (Caddy `encode gzip` deckt LAN ab)

### CRITIQUE

**Annahmen & Schwächen (ehrlich):**
1. **Zwei Telemetrie-Systeme parallel** (Snapshots + Events) — doppelte Fläche, doppelte
   Wartung. Trade-off akzeptiert: Events sind leichtgewichtig und granular, Snapshots sind
   Backup; Konvergenz (Events als einzige Quelle, Snapshot nur noch auf Anforderung) ist
   ein bewusster Follow-up, kein Ziel dieses PRs.
2. **Idempotenz hängt an Client-Disziplin**: Dedup funktioniert nur, wenn der Client den
   Batch erst nach 2xx-ACK verwirft und `event_id` bei Retry unverändert lässt. Der Flusher
   muss das strikt umsetzen (Test dafür Pflicht); ein fehlerhafter Client kann trotzdem
   Duplikate erzeugen — abgefedert, nicht verhindert.
3. **Clock Skew**: `ts` ist Client-Zeit (ISO8601); Aggregationen über `ts` können bei
   falsch gestellten Uhren verzerren. Retention läuft über `received_at` (Server-Zeit).
   Akzeptiert; kein NTP-Zwang.
4. **Payload-PII-Risiko**: `task_history.error_message` kann absolute Pfade enthalten.
   Sanitisierung (Home-Prefix strippen, Kürzen) ist Pflicht und muss getestet sein —
   Fehler hier wäre ein Sicherheits-/Privacy-Bug. Redaction-Kultur aus
   `RedactedConfig`/`instance.json` wiederverwenden.
5. **Retention-Granularität**: hartes Löschen nach Tagen; keine Sampling-/Rollup-Stufe.
   Für die geforderten Views ausreichend; Rollups sind Follow-up, wenn die DB wächst.
6. **Kein Rate-Limiting**: ein kompromittierter Client kann viel schreiben. Auth +
   Dedup + Retention begrenzen den Schaden; hartes Limiting ist Out of Scope.
7. **Batch-Größe vs. HTTP**: 200 Events/~1 MB pro Batch; Caddy/`encode gzip` ist
   transparent; Body-Limit am Receiver explizit setzen (DefaultBodyLimit).
8. **„zuletzt gesehen" ohne Heartbeat** ist event-getrieben — ein ruhiger Client
   (keine Tasks/Scans) erscheint „alt". Akzeptiert (Anforderung: keine Heartbeats);
   Dokumentation im View-Kommentar.
9. **Migrationskette**: zweite Kette `migrations/telemetry/` weicht vom „eine Kette"-
   Schema ab — bewusst, weil die Haupt-DB (Client) die Ingest-Tabellen nicht braucht
   und umgekehrt. `_sqlx_migrations` ist pro DB; Risiko: Verwechslung bei künftigen
   Migrationsarbeiten → Namensraum klar dokumentieren.

**Entscheidungen, die Stage 3 bestätigen muss:** exakte Event-Typ-Allowlist, exakte
Payload-Felder je Typ, Batch-Limits, Spool-Dateiname/-Format, Backoff-Parameter,
`retention_days`-Default, Views-Definitionen (finaler SQL-Text in der Migration).
