## Plan: background-loop-observability

**Status**: done ✅
**Branch**: `feat/background-loop-observability`
**Ready for review**: no
**Depends on**: nothing
**Migration needed**: no

### Description

Fünf Hintergrund-Loops laufen komplett blind — kein TaskManager, keine
Persistenz, keine UI-Sichtbarkeit. Nur `tracing`-Logs auf stdout. Jeder Loop
soll pro Iteration einen Task im TaskManager anlegen, Fortschritt und Ergebnisse
loggen und bei Fehlern mit Klartext-Meldung failen. Tasks sind 5 Min nach
Completion via bestehendem Auto-Prune auf `#tasks` sichtbar.

### Aktueller Zustand

| Loop                | Modul                  |    TaskManager?     | Sichtbarkeit |
| ------------------- | ---------------------- | :-----------------: | ------------ |
| Subscription Poller | `src/poller.rs`        |         ❌          | Nur stdout   |
| Global Poller       | `src/global_poller.rs` |         ❌          | Nur stdout   |
| Maintainer          | `src/maintainer.rs`    |   ✅ (ungenutzt)    | Nur stdout   |
| Folder Watcher      | `src/watch.rs`         |   ✅ (ungenutzt)    | Nur stdout   |
| Auto-Backup Poller  | inline in `main.rs`    | ✅ (nur Kind-Tasks) | Nur stdout   |

Alle 5 Loops existieren, laufen, tun Arbeit — und der Nutzer sieht nichts davon
außer zufälligen `info!`-Zeilen im Terminal.

### Design

Jeder Loop erzeugt pro Iteration einen Task. Granularität:

| Loop                | Task-Typ           | Granularität         | Frequenz   |
| ------------------- | ------------------ | -------------------- | ---------- |
| Subscription Poller | `PollSubscription` | **Pro Subscription** | ~5 Min/Sub |
| Global Poller       | `GlobalPollCycle`  | Pro Zyklus           | 15 Min     |
| Maintainer          | `MaintainerCycle`  | Pro Zyklus           | 1 Stunde   |
| Folder Watcher      | `FolderWatch`      | Pro Zyklus           | 5 Min      |
| Auto-Backup         | `AutoBackupCheck`  | Pro Zyklus           | 10 Min     |

**Warum Subscription Poller pro Subscription?** Weil der Nutzer wissen will
"Was ist mit BridalDJSet passiert?" — nicht "Irgendwas mit 16 Subscriptions".

**Task-Lifecycle**:

1. `start_task()` → Status `Running`
2. `add_log()` für jeden signifikanten Schritt
3. `update_task_status(Completed)` bei Erfolg
4. `update_task_status(Failed)` bei Fehler
5. Auto-Prune nach 5 Min (bestehend)

**Konflikt-Keys**: Keine — Loop-Tasks sind per Definition sequentiell innerhalb
eines Loops, und verschiedene Loop-Typen dürfen parallel laufen.

### Neue TaskType-Varianten (`src/tasks/mod.rs`)

```rust
pub enum TaskType {
    // ... existing variants ...

    /// Subscription poller: single subscription poll cycle
    PollSubscription {
        subscription_id: i64,
        playlist_name: String,
    },
    /// Global poller: one full cycle checking all Spotify playlists
    GlobalPollCycle,
    /// Maintainer: one housekeeping cycle
    MaintainerCycle,
    /// Folder watcher: one scan-all-active-folders cycle
    FolderWatch,
    /// Auto-backup: one check-all-auto-backup-folders cycle
    AutoBackupCheck,
}
```

Alle 5 bekommen `None` als conflict key in `task_type_conflict_key()`.

### Änderungen pro Loop

#### 1. Subscription Poller (`src/poller.rs`)

Signatur-Änderung:

```rust
pub async fn start_subscription_poller(
    db: Pool<Sqlite>,
    credentials: ServiceCredentials,
    task_manager: crate::tasks::TaskManager,  // NEU
    cancel_token: CancellationToken,
    subscription_count: i64,
)
```

In `poll_subscribed_playlist()` (nimmt jetzt `&TaskManager`):

```rust
// Task anlegen
let task_id = task_manager.start_task(Task::new(
    TaskType::PollSubscription {
        subscription_id: subscription.id,
        playlist_name: subscription.playlist_name.clone().unwrap_or_default(),
    },
    Some("spotify".into()),
)).await;
task_manager.update_task_status(&task_id, TaskStatus::Running).await;

// Schritte loggen
task_manager.add_log(&task_id, "Fetching playlist metadata from Spotify...").await;
// → nach fetch:
task_manager.add_log(&task_id, format!("Fetched: '{}', {} tracks", name, total)).await;

// Snapshot-Check:
task_manager.add_log(&task_id, "Snapshot unchanged, skipping track fetch").await;
// oder:
task_manager.add_log(&task_id, format!("Snapshot changed, fetching {} tracks...", diff)).await;

// Neue Tracks:
task_manager.add_log(&task_id, format!("Found {} new track(s)", n)).await;

// Deemix:
task_manager.add_log(&task_id, "Deemix: attempting auto-download...").await;
// → from_db → None:
task_manager.add_log(&task_id, "Deemix: SKIPPED — not connected").await;
// → ensure_queued → Ok:
task_manager.add_log(&task_id, "Deemix: queued successfully").await;
// → ensure_queued → Err:
task_manager.add_log(&task_id, format!("Deemix: FAILED — {}", err)).await;

// Abschluss:
task_manager.update_task_status(&task_id, TaskStatus::Completed).await;
// Oder:
task_manager.update_task_status(&task_id, TaskStatus::Failed).await;
```

Die bestehenden `tracing::info!/warn!/error!` bleiben zusätzlich erhalten
(doppelte Buchführung: stdout fürs Terminal, TaskManager für die UI).

#### 2. Global Poller (`src/global_poller.rs`)

Signatur-Änderung:

```rust
pub async fn start_global_poller(
    db: Pool<Sqlite>,
    config: ServiceCredentials,
    task_manager: crate::tasks::TaskManager,  // NEU
    interval_secs: u64,
    cancel_token: CancellationToken,
)
```

Pro 15-Min-Zyklus ein Task:

```rust
let task_id = task_manager.start_task(Task::new(
    TaskType::GlobalPollCycle,
    Some("spotify".into()),
)).await;

task_manager.add_log(&task_id, format!("Checking {} playlists...", total)).await;
task_manager.add_log(&task_id, format!("{} changed, {} unchanged, {} skipped", changed, unchanged, skipped)).await;
task_manager.add_log(&task_id, format!("{} new track(s) added", new_tracks)).await;

task_manager.update_task_status(&task_id, TaskStatus::Completed).await;
```

#### 3. Maintainer (`src/maintainer.rs`)

Hat bereits `TaskManager`. Pro Zyklus einen Task anlegen:

```rust
let task_id = task_manager.start_task(Task::new(
    TaskType::MaintainerCycle,
    None,
)).await;

task_manager.add_log(&task_id, "Maintainer cycle starting...").await;
task_manager.add_log(&task_id, format!("Folder {} needs full scan (last: {} ago)", id, age)).await;
task_manager.add_log(&task_id, "No folders need scanning").await;
task_manager.add_log(&task_id, format!("Triggered backup for folder {}: {} files", id, n)).await;
task_manager.add_log(&task_id, "Maintainer cycle complete").await;

task_manager.update_task_status(&task_id, TaskStatus::Completed).await;
```

#### 4. Folder Watcher (`src/watch.rs`)

Hat bereits `TaskManager` (`self.task_manager`). In `scan_active_folders()`
bzw. im Watcher-Loop pro Zyklus einen Task anlegen:

```rust
let task_id = task_manager.start_task(Task::new(
    TaskType::FolderWatch,
    None,
)).await;

task_manager.add_log(&task_id, format!("Scanning {} active folder(s)...", n)).await;
task_manager.add_log(&task_id, format!("Folder '{}': {} files scanned", path, count)).await;
task_manager.add_log(&task_id, "No changes detected").await;
task_manager.add_log(&task_id, "Folder watch cycle complete").await;

task_manager.update_task_status(&task_id, TaskStatus::Completed).await;
```

#### 5. Auto-Backup Poller (neue Datei `src/auto_backup.rs`)

Extrahiert aus dem inline `tokio::spawn` in `main.rs:427-470`.

```rust
pub async fn start_auto_backup_poller(
    db: Pool<Sqlite>,
    task_manager: crate::tasks::TaskManager,
    interval_secs: u64,
)
```

Pro 10-Min-Zyklus:

```rust
let task_id = task_manager.start_task(Task::new(
    TaskType::AutoBackupCheck,
    None,
)).await;

task_manager.add_log(&task_id, format!("Checking {} folder(s) with auto_backup...", n)).await;
task_manager.add_log(&task_id, format!("Folder '{}': {} unbacked files → triggered backup", path, n)).await;
task_manager.add_log(&task_id, "All folders up to date, no backups needed").await;

task_manager.update_task_status(&task_id, TaskStatus::Completed).await;
```

### `src/main.rs` — Änderungen

1. Subscription Poller: `task_manager.clone()` als neuer Parameter
2. Global Poller: `task_manager.clone()` als neuer Parameter
3. Auto-Backup: inline `tokio::spawn` ersetzen durch `start_auto_backup_poller(...)`
4. `pub mod auto_backup;` Declaration

### Files zu erstellen

| Datei                | Inhalt                                     |
| -------------------- | ------------------------------------------ |
| `src/auto_backup.rs` | Extrahierter Auto-Backup-Loop (~60 Zeilen) |

### Files zu ändern

| Datei                  | Änderung                                                                              |
| ---------------------- | ------------------------------------------------------------------------------------- |
| `src/tasks/mod.rs`     | 5 neue `TaskType`-Varianten + `conflict_key`-Einträge + `Task`-Import in Loop-Modulen |
| `src/poller.rs`        | `TaskManager`-Param + pro-Subscription `PollSubscription`-Task                        |
| `src/global_poller.rs` | `TaskManager`-Param + pro-Zyklus `GlobalPollCycle`-Task                               |
| `src/maintainer.rs`    | Pro-Zyklus `MaintainerCycle`-Task (TM bereits vorhanden)                              |
| `src/watch.rs`         | Pro-Zyklus `FolderWatch`-Task (TM bereits vorhanden)                                  |
| `src/main.rs`          | TM an Poller/GlobalPoller durchreichen, Auto-Backup extrahieren                       |

### Acceptance Criteria

- [ ] Alle 5 `TaskType`-Varianten definiert, alle mit `None` conflict key
- [ ] `cargo build` ohne Fehler
- [ ] Subscription Poller: pro Subscription-Poll erscheint ein Task auf `#tasks`
- [ ] Subscription Poller: Task-Logs zeigen Spotify-Fetch, Snapshot-Status, Deemix-Status
- [ ] Global Poller: pro 15-Min-Zyklus ein Task mit Summary
- [ ] Maintainer: pro Zyklus ein Task mit Aktionen
- [ ] Folder Watcher: pro 5-Min-Scan ein Task
- [ ] Auto-Backup: pro 10-Min-Check ein Task
- [ ] Tasks erscheinen auf `GET /api/tasks` und `#tasks`-Seite
- [ ] Tasks werden nach 5 Min via bestehendem Auto-Prune gelöscht
- [ ] Bestehende `tracing`-Logs bleiben erhalten (duale Ausgabe)
- [ ] `cargo test` besteht (alle existierenden Tests)

### Out of Scope

- DB-Persistenz der Task-Logs über den Auto-Prune hinaus (wäre `task_history`)
- Alerts/Notifications auf Fehler
- Task-Deduplizierung (der Loop ist inhärent sequentiell)
- `#tasks`-Seite UI-Verbesserungen (die Seite existiert bereits)

---

