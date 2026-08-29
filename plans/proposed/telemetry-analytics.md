# Plan: telemetry-analytics

**Status**: in-progress
**Branch**: `feat/telemetry-analytics`
**Ready for review**: no
**Depends on**: nothing (nutzt `VACUUM INTO`, `tracing`-Logs, `task_history`, `axum`+`reqwest`)
**Migration needed**: no (Telemetrie = Datei-Push über HTTPS; `task_history` existiert via Migration 022)

### Description

Die (aktuell einzige) **Prod-Instanz auf dem MacBook** pusht regelmässig ein
selbstbeschreibendes **Telemetrie-Bundle über HTTPS** an einen kleinen Receiver auf dem
Music Server (`192.168.178.149`, Domain `telemetry.music.klimk.es`). Kern ist der
**konsistente SQLite-Voll-Snapshot (`VACUUM INTO` → `db.sqlite`)** — „das große Full-DB-Dump“.
Ergänzend: Logs, Task-History, Metriken, (redacted) Instance-Metadaten.

Zweck:

1. **Backup** — die Snapshots _sind_ das DB-Backup (zusätzlich zum NAS-File-Backup).
2. **Auswertung / Nutzung** — ein Analyzer liest die Snapshots und liefert Fehler-Statistik,
   Task-Statistik, Tabellen-Grössen, Orphans etc.
3. **Fehler finden** — failed Tasks + `ERROR`/`WARN`-Logzeilen werden aggregiert.
4. **Dev-Agent (openclaw)** — läuft auf dem LAN-Server (wie Caddy). Liest den gepushten
   Snapshot direkt aus dem Collector-Verzeichnis (`<base_dir>/macbook/latest/db.sqlite`) —
   gleicher Host, kein Pull nötig.

### Topologie / Rollen (Stand jetzt)

| Rolle                                         | Host       | IP / Domain                                    | DB                                      |
| --------------------------------------------- | ---------- | ---------------------------------------------- | --------------------------------------- |
| **Prod**                                      | MacBook    | `:3000` (läuft lokal)                          | `app.db` / `library.db` (lokal)         |
| **LAN Server** (Collector + Caddy + openclaw) | LAN Server | `192.168.178.149` · `telemetry.music.klimk.es` | — (Collector)                           |
| **openclaw (Agent)**                          | LAN Server | gleicher Host wie Caddy (.149)                 | liest `macbook/latest/db.sqlite` direkt |

```
MacBook (Prod, :3000)                          LAN Server 192.168.178.149
──────────────────────                          ──────────────────────────────
momos-music-manager                             Caddy (mellon-caddy)
  │  VACUUM INTO → db.sqlite                     │  telemetry.music.klimk.es
  │  + logs/tasks/metrics (JSON)                 │   └─ reverse_proxy 127.0.0.1:8330
  │  HTTPS PUT/POST (Bearer-Token)               │        │
  └────────────► https://telemetry.music.klimk.es ────────┘
                                                momos-telemetry (systemd)
                                                  └─ /home/momo/momos-analytics/
                                                       <instance>/<ts>/{db.sqlite, …}
                                                       <instance>/latest → <ts>

                                                openclaw (Dev-Agent, gleicher Host)
                                                  └─ liest macbook/latest/db.sqlite direkt
```

**Pro Host ein eigener Folder** (`instance`-Name): `macbook` (heute), später ggf.
`music-server` wenn Prod dorthin zieht.

> **Music Server (`.200`)** ist die separate Dev-Instanz — aktuell **nicht** Teil des
> Telemetrie-Flows. Erst wenn Prod dorthin zieht, wird sie ein eigener `instance`-Folder.

### Wichtige Erkenntnisse aus dem Code (Ground Truth)

| Baustein | Ist-Zustand                                                                                                                                             | Konsequenz                                                                                                                    |
| -------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------- |
| SQLite   | WAL-Mode (`journal_mode=Wal`) in `db::connection::connect_db`                                                                                           | `VACUUM INTO` liefert konsistenten Snapshot ohne die Live-DB zu blockieren                                                    |
| Full DB  | `dump::export_dump_json()` exportiert **nur eine kuratierte Teilmenge** (kein `task_history`, `file_locations`, `file_resolved_tags`, `tag_bundles`, …) | **Kanonisch ist `db.sqlite` (VACUUM INTO), nicht der JSON-Dump**                                                              |
| Tasks    | `task_history`-Tabelle (Migration 022) persistiert Tasks inkl. `logs`, `error_message`, `status`                                                        | Als `tasks.json` exportierbar (bzw. steckt schon in `db.sqlite`)                                                              |
| Logging  | `tracing` + `tracing-appender`, täglich nach `~/.local/share/momos-music-manager/logs/server.log` (`MOMOS_LOG_DIR`)                                     | `server.log` + `server.log.YYYY-MM-DD` mitschicken                                                                            |
| HTTP     | `axum` (mit `multipart`-Feature) + `reqwest` bereits vorhanden; `infrastructure.rs` hat schon Upload/Download-Handler                                   | Receiver als axum-Router; Client via `reqwest`; kein neues Crate nötig (ggf. reqwest `stream`-Feature für `Body::from(File)`) |

### Receiver — neue `telemetry receive` Subcommand (gleiche Binary)

Ein minimaler axum-Router, der **kein sqlx** braucht (nur Datei-I/O + Retention):

| Endpoint                                     | Methode | Payload                                         | Effekt                                                  |
| -------------------------------------------- | ------- | ----------------------------------------------- | ------------------------------------------------------- |
| `/health`                                    | GET     | —                                               | `{ "status": "ok" }`                                    |
| `/api/telemetry/{instance}/db`               | PUT     | roher `db.sqlite`-Body (Stream)                 | `base_dir/<instance>/<ts>/db.sqlite`, `latest` → `<ts>` |
| `/api/telemetry/{instance}/meta`             | POST    | JSON `{ ts, instance, metrics, tasks, logs[] }` | `instance.json`, `metrics.json`, `tasks.json`, `logs/…` |
| `/api/telemetry/{instance}/latest/db.sqlite` | GET     | —                                               | neuesten Snapshot runterladen (Verify / openclaw-Pull)  |

Auth: `Authorization: Bearer <token>` (konstantzeitiger Vergleich). Ohne/bei falschem Token → `401`.

### Config

```toml
# ── Client (MacBook, Prod) ──────────────────────────────────────────
[telemetry]
enabled = true
base_url = "https://telemetry.music.klimk.es"
token = "…"          # Shared Secret — NIE im Repo (Env oder ~/.config)
instance = "macbook" # Pro-Host-Folder
interval_secs = 3600 # 0 = Loop aus, nur CLI
include_json = false # MVP: nur db.sqlite; später true für dump.json

# ── Receiver (LAN Server, 192.168.178.149) ──────────────────────────
[telemetry_receiver]
bind = "127.0.0.1:8330"
base_dir = "/home/momo/momos-analytics"
token = "…"          # gleiches Secret
retention_days = 30
```

Priorität wie gewohnt: **Env (`MOMOS_TELEMETRY_*`) > `config.toml` > Defaults**.

### Bundle-Layout auf dem Collector

```
/home/momo/momos-analytics/
├── macbook/
│   ├── 2026-08-26T12-00-00Z/
│   │   ├── db.sqlite        # VACUUM INTO — Voll-Snapshot (Kern)
│   │   ├── instance.json    # hostname, version, git commit, REDACTED config, db_size
│   │   ├── metrics.json     # task-stats, error-counts, table row-counts
│   │   ├── tasks.json       # task_history (rolling window)
│   │   └── logs/
│   │       ├── server.log
│   │       └── server.log.2026-08-25
│   └── latest -> 2026-08-26T12-00-00Z/   # Symlink, nach jedem Push
└── … (weitere Hosts je instance)
```

**Redaction (Pflicht):** `instance.json` darf **nie** `refresh_token`, `access_token`,
`spotify_client_secret`, `soundcloud_api_key`, `youtube_api_key`, deemix-ARL enthalten.
Allowlist: nur unkritische Felder.

### Neue Dateien / Module

| Datei                                | Inhalt                                                         |
| ------------------------------------ | -------------------------------------------------------------- |
| `src/telemetry/mod.rs`               | Bundle-Builder + Client-Push (`push_once`) + Config-Typen      |
| `src/telemetry/receiver.rs`          | axum-Router + Handler (`receive`) + Retention                  |
| `src/telemetry/metrics.rs`           | Aggregierte Metriken (Task-Statistik, Tabellen-Zeilen, Fehler) |
| `src/telemetry/analyze.rs`           | Analyzer (Report aus einem `db.sqlite`-Snapshot)               |
| `deploy/momos-telemetry.service`     | systemd-Unit für den Receiver auf dem Server                   |
| `deploy/Caddyfile.telemetry.snippet` | `telemetry.music.klimk.es`-Block (Doku)                        |

### Zu ändernde Dateien

| Datei               | Änderung                                                                                   |
| ------------------- | ------------------------------------------------------------------------------------------ |
| `src/config.rs`     | `TelemetryToml` + `TelemetryReceiverToml` + Felder in `ServiceCredentials` + Env-Overrides |
| `src/main.rs`       | Subcommands `telemetry {push, receive, analyze, status}` + Telemetrie-Loop in `serve()`    |
| `src/tasks/mod.rs`  | `TaskType::TelemetryPush` (+ `conflict_key = None`)                                        |
| `src/lib.rs`        | `pub mod telemetry;`                                                                       |
| `docs/DECISIONS.md` | ADR: Telemetrie-Pipeline                                                                   |
| `AGENT.md`          | Kurz-Abschnitt „Telemetrie & Analytics“                                                    |

### CLI

```bash
momos-music-manager telemetry push --once        # einmaliger HTTPS-Push (Kern: db.sqlite)
momos-music-manager telemetry push               # Loop (wenn interval_secs > 0)
momos-music-manager telemetry receive            # Receiver (Server, systemd)
momos-music-manager telemetry analyze <dir|db>   # Report (JSON + human-readable)
momos-music-manager telemetry status             # letzter Push + nächster Fällig
```

### Analyzer — Report (Low-Hanging-Optimizations + Fehler)

Aus `db.sqlite` (Kopie) via `sqlx`:

1. **Tasks** (`task_history`): counts je `status`, Top-10 `error_message`, failure-rate, avg/p95 Duration je `task_type`.
2. **Logs**: Anzahl `ERROR`/`WARN`-Zeilen, häufigste Muster (normalisiert).
3. **DB-Hygiene**: Tabellen-Zeilenzahlen, Orphans (`service_playlist_tracks` ohne Track/Playlist, `files` ohne `file_locations`, `tags` ohne `category`), Tombstones (`deleted_at`).
4. **Nutzung**: Play-/Tag-/Playlist-Counts, BPM/Key-Abdeckung (`bpm IS NULL`-Rate).

### Phasen (inkrementell, jede testbar)

| Phase                               | Lieferung                                                                        | Validierung                                                        |
| ----------------------------------- | -------------------------------------------------------------------------------- | ------------------------------------------------------------------ |
| **1. Receiver + Client-Push (MVP)** | `receiver.rs` + `telemetry push --once` + `VACUUM INTO`-Upload + `latest` + Auth | Roundtrip-Test in-process (tower `oneshot`), Auth-Tests, CLI-Parse |
| **2. Meta (logs/tasks/metrics)**    | `metrics.rs` + `POST …/meta` + Redaction                                         | Unit-Tests (Metriken, Redaction)                                   |
| **3. In-App-Loop**                  | `TaskType::TelemetryPush`, Loop in `serve()`, Sichtbarkeit auf `#tasks`          | Integrationstest Loop-Anlage                                       |
| **4. Retention**                    | Receiver räumt alte Snapshots                                                    | Unit-Test der Retention-Berechnung                                 |
| **5. Analyzer**                     | `analyze.rs` + CLI `analyze`                                                     | Integrationstests gegen geseedete Snapshot-DB                      |
| **6. Deploy auf Server**            | `momos-telemetry.service` + Caddy-Snippet + `home_domains.txt`-Eintrag           | Manuell: `curl` gegen `telemetry.music.klimk.es`                   |

### Server-Seite (einmalig, manuell — NICHT Teil der Rust-Implementierung)

1. `~/home_domains.txt`: `telemetry.music.klimk.es` als belegt markieren.
2. `~/caddy/Caddyfile` ergänzen + reload:
   ```caddy
   telemetry.music.klimk.es {
       reverse_proxy 127.0.0.1:8330
       encode gzip
   }
   ```
3. `deploy/momos-telemetry.service` installieren (`momos-music-manager telemetry receive`),
   Token via `systemctl edit` als `Environment="MOMOS_TELEMETRY_TOKEN=…"`.
4. TLS: `*.klimk.es` provisioniert Caddy automatisch über Cloudflare DNS.

### Acceptance Criteria

- [ ] `cargo build` passes
- [ ] `cargo test` passes (bestehende + neue Tests)
- [ ] `[telemetry]`/`[telemetry_receiver]`-Config wird geladen (Env > TOML > Default); Default `enabled=false`/`interval_secs=0` ändert kein Verhalten
- [ ] `telemetry push --once` lädt einen konsistenten `VACUUM INTO`-Snapshot als `db.sqlite` per HTTPS hoch
- [ ] Receiver speichert unter `base_dir/<instance>/<ts>/db.sqlite`, setzt `latest`-Symlink
- [ ] Auth: ohne/falscher Token → `401`; korrekter Token → `2xx`
- [ ] `instance.json` enthält **keine** Secrets (Allowlist getestet)
- [ ] In-App-Loop legt pro Zyklus einen `TelemetryPush`-Task an (`#tasks` + `task_history`)
- [ ] Retention löscht Snapshots älter als `retention_days`
- [ ] `telemetry analyze` liefert Task-/Fehler-/DB-Hygiene-Statistik
- [ ] `GET …/latest/db.sqlite` liefert den neuesten Snapshot (Verify/openclaw-Pull)
- [ ] Kein Secret wird je in Bundle oder Logs geschrieben

### Out of Scope (erste Iteration)

- Telemetrie für die Python `download-service` (eigene Logs/DB) — Follow-up analog
- Dedizierter Analytics-Dashboard (Analyzer ist CLI-first; openclaw liest direkt)
- Push von Musik-**Dateien** (macht das Folder-/NAS-Backup)
- Anomalie-Erkennung / Alerts (später auf `metrics.json`)
- Bidirektionales Sync (Push-only; `restore` bleibt Rückweg)
- gzip/kompression der `db.sqlite` (LAN reicht roh; später `flate2` als Optimierung)

### Agent Decomposition (für die Umsetzung nach Review)

| Agent | Dateien (disjunkt)                                                                               | Arbeit                                 |
| ----- | ------------------------------------------------------------------------------------------------ | -------------------------------------- |
| **A** | `src/telemetry/receiver.rs`, `src/config.rs`, `src/lib.rs`, `deploy/*`                           | Receiver + Config + systemd/Caddy-Doku |
| **B** | `src/telemetry/mod.rs`, `src/telemetry/metrics.rs`, `src/main.rs` (CLI+Loop), `src/tasks/mod.rs` | Client-Push + Metriken + CLI + Loop    |
| **C** | `src/telemetry/analyze.rs`, `docs/DECISIONS.md`, `AGENT.md`                                      | Analyzer + ADR + Doku                  |

Hinweis: `src/main.rs` und `src/config.rs` werden von A und B angefasst → CLI vs. Receiver
vorab sauber aufteilen oder sequenziell fahren. Tests jeweils im eigenen Scope.

---
