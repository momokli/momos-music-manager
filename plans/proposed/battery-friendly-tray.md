# Plan: Battery-Friendly Background Operation

**Status**: proposed
**Branch**: `feat/battery-friendly-tray`
**Ready for review**: yes
**Depends on**: nothing
**Migration needed**: no

### Description

momos-music-manager läuft als macOS Launch Agent permanent im Hintergrund und verbraucht im Tray-Modus zu viel Akku. Hintergrund-Tasks pollen mit festen kurzen Intervallen (30s Subscription Poller, 5min Folder Watcher, 10min Auto-Backup, 15min Global Poller) — ohne Rücksicht darauf, ob das Gerät am Akku oder Netzteil hängt und ob überhaupt jemand die App aktiv nutzt.

### Root Cause Analysis

**Bereits gefixt (Commit `db2f248`):**
- DB-aware scan skip (kein Re-Extract unveränderter Files)
- SHA256 → mtime+size (Zero-I/O)
- Exiftool nur für relevante File-Typen
- Poller Loop-Struktur (kein tight error loop)
- Global Poller 60s Cold-Start statt 15min
- Folder Watcher ohne initialen Scan

**Verbleibende Akku-Fresser (nach Schweregrad):**

| # | Problem | Impact | 
|---|---------|--------|
| 1 | **Subscription Poller 30s** — wacht alle 30 Sekunden auf, queried DB, macht ggf. Spotify API-Calls, schreibt Logs. Selbst bei 0 Subscriptions läuft die Loop. | 🔴 Hoch |
| 2 | **Keine Power-Source-Awareness** — App weiß nicht, ob macOS am Akku oder Netzteil hängt. Alle Intervalle sind starr. | 🔴 Hoch |
| 3 | **Keine User-Activity-Awareness** — Auch wenn niemand das Web-UI offen hat (Tray-only), laufen alle Tasks mit voller Frequenz. | 🟡 Mittel |
| 4 | **Embedding Model persistent** — all-MiniLM-L6-v2 (~90MB) wird lazy geladen, aber nie entladen. Hält RAM hoch, verhindert Deep Sleep. | 🟡 Mittel |
| 5 | **Auto-Backup Poller hardcoded 600s** — Nicht konfigurierbar, kein Battery-Aware Throttling. | 🟢 Niedrig |
| 6 | **Launch Agent ohne Power-Management-Keys** — `KeepAlive: true` ohne `LowPriorityIO`, `Nice`, `ProcessType`. | 🟢 Niedrig |
| 7 | **tokio `full` features** — Multi-Thread Runtime hält mehrere OS-Threads alive, selbst wenn idle. | 🟢 Niedrig (zu invasiv) |
| 8 | **File-Logging auf jedem Poll-Cycle** — tracing_appender schreibt bei jedem 30s-Tick auf Disk. | 🟢 Niedrig |

### Proposed Solution

#### Step 1: Power Mode Detection (`src/power.rs`)

Neues Modul zur Erkennung des macOS-Power-Status:

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PowerMode {
    Battery,
    AcPower,
    Unknown, // non-macOS fallback
}

impl PowerMode {
    /// Detect via `pmset -g batt` or `system_profiler SPPowerDataType`
    pub fn detect() -> Self { ... }
}

/// Background task: check power mode every 5 minutes, update shared state
pub async fn start_power_monitor(
    current_mode: Arc<AtomicU8>,  // 0=unknown, 1=battery, 2=ac
    cancel_token: CancellationToken,
) { ... }
```

#### Step 2: Idle Detection via WebSocket Tracking

`AppState` bekommt einen Zähler für aktive WebSocket-Connections. Wenn für N Minuten keine Connection aktiv ist → "idle mode". Neue Connections setzen "active mode".

```rust
pub struct AppState {
    // ... existing fields ...
    pub active_ws_connections: Arc<AtomicU32>,
    pub last_user_activity: Arc<Mutex<Instant>>,
}
```

#### Step 3: Adaptive Polling Intervals

Jeder Background-Task bekommt eine `PollingConfig`, die je nach Modus (`Battery+Idle`, `Battery+Active`, `AcPower+Idle`, `AcPower+Active`) unterschiedliche Intervalle verwendet:

| Task | Ac+Active (jetzt) | Ac+Idle | Battery+Active | Battery+Idle |
|------|-------------------|---------|----------------|--------------|
| Subscription Poller | 30s | 120s | 300s (5min) | 900s (15min) |
| Global Poller | 900s (15min) | 1800s | 3600s | 7200s |
| Folder Watcher | 300s (5min) | 600s | 1800s | 3600s |
| Auto-Backup Poller | 600s (10min) | 1200s | 3600s | 7200s |
| Maintainer | 3600s (1h) | 7200s | 21600s (6h) | 43200s |

Modi-Wechsel werden via `tokio::sync::watch` channel propagiert, alle Tasks reagieren dynamisch.

#### Step 4: Embedding Model Lifecycle

- `EmbeddingModel` wird nach **30 Minuten** Inaktivität (kein API-Call) entladen
- `AppState.embeddings` Mutex + Timestamp für `last_used`
- Hintergrund-Task prüft alle 10 Minuten, droppt das Model bei Timeout
- Nächster API-Call lädt es transparent nach

#### Step 5: Launch Agent Optimierung

Plist erweitern um Power-Management-Keys:

```xml
<key>LowPriorityIO</key>
<true/>
<key>Nice</key>
<integer>10</integer>
<key>ProcessType</key>
<string>Background</string>
```

#### Step 6: Poller Idle-Efficiency (Quick Wins)

- Subscription Poller: bei **0 Subscriptions** → komplett pausieren bis eine Subscription erstellt wird (via `watch` channel)
- Auto-Backup Poller: bei **0 Folders mit auto_backup** → pausieren
- Folder Watcher: bei **0 aktiven Foldern** → pausieren

### Files to modify

| File | Change |
|------|--------|
| `src/power.rs` | **NEU** — Power-Mode-Detection für macOS (pmset/IOPS) |
| `src/lib.rs` | AppState erweitern: `Arc<AtomicU32>` für WS-Connections, Power-Mode-Watch-Channel |
| `src/main.rs` | Power-Monitor starten, PollingConfig an Tasks übergeben, Watch-Channels für Sub/Folder-Counts |
| `src/poller.rs` | Adaptives Intervall via watch, 0-subscriptions → Pause |
| `src/global_poller.rs` | Adaptives Intervall via watch |
| `src/watch.rs` | Adaptives Intervall, 0-folders → Pause, watch-Channel für Folder-Count |
| `src/auto_backup.rs` | Adaptives Intervall, 0-auto_backup-folders → Pause |
| `src/maintainer.rs` | Adaptives Intervall via watch |
| `src/embeddings.rs` | `last_used` Timestamp + unload-logic |
| `src/api/websocket.rs` | WS-Connection-Tracking (increment/decrement counter) |
| `src/launch_agent.rs` | Plist: `LowPriorityIO`, `Nice`, `ProcessType` |
| `src/config.rs` | Polling-Konfiguration per Mode (konfigurierbar via config.toml/env) |

### Acceptance Criteria

- [ ] `cargo build` passes
- [ ] `cargo test` passes
- [ ] `cd frontend && npx playwright test` passes
- [ ] Power-Mode wird auf macOS korrekt erkannt (getestet via Unit-Test mit Mock)
- [ ] Poller-Intervalle adaptieren dynamisch bei Mode-Wechsel
- [ ] Bei 0 Subscriptions pausiert der Subscription Poller vollständig
- [ ] Embedding Model wird nach Timeout entladen und bei Bedarf nachgeladen
- [ ] Launch Agent Plist enthält Power-Management-Keys
- [ ] Non-macOS: alles kompiliert, Power-Mode ist `Unknown` → verhält sich wie `AcPower`
