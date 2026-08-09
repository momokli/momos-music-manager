## Plan: maintainer

**Status**: done ✅
**Branch**: feat/maintainer
**Depends on**: fix/local-file-tracking
**Migration needed**: no

### Description

Background task that keeps the DB in sync with reality. The system has 4 separate pollers but no coordinator. The Maintainer periodically checks for stale state and triggers corrective actions via existing task workers (ScanFolder, BackupFolder, BackupDiscovery).

### What it does

Single background loop, configurable interval (default 1h, env: MOMOS_MAINTAINER_INTERVAL_SECS):

| #   | Check             | Condition                                 | Action                                                           |
| --- | ----------------- | ----------------------------------------- | ---------------------------------------------------------------- |
| 1   | Full scan needed  | last_scanned > 24h for any active folder  | Spawn ScanFolder (populates file_locations.local, stale cleanup) |
| 2   | Unbacked-up files | auto_backup=true and files without backup | Spawn BackupFolder (reconcile + rsync)                           |
| 3   | Backup discovery  | Last discovery > 7 days, backup_path set  | Spawn BackupDiscovery (lists NAS, creates DB records)            |

Lightweight coordinator — doesn't do the work itself, only triggers tasks.

Replaces the manual "Run Full Scan" button — Maintainer does it proactively. Button stays as override.

### Config

```toml
[maintainer]
interval_secs = 3600          # cycle interval (default 1h)
full_scan_max_age_secs = 86400 # max age before full scan (default 24h)
backup_discovery_interval_secs = 604800 # backup discovery interval (default 7d)
```

### Files to modify

- src/maintainer.rs — NEW module
- src/main.rs — spawn in serve()
- src/config.rs — MaintainerConfig section

### Acceptance Criteria

- [ ] Maintainer starts on server boot
- [ ] Triggers full scan when last_scanned exceeds max age
- [ ] Triggers backup for auto-backup folders with unbacked-up files
- [ ] Triggers backup discovery on schedule
- [ ] Configurable via config.toml + env vars
- [ ] Cancel token honored for clean shutdown
- [ ] Zero cost when idle (timestamp checks only)
- [ ] cargo build passes

---

