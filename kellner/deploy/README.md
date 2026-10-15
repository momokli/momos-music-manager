# kellner — deploy assets

Reference templates for running `kellner` as a background service, plus the
install / smoke scripts.

## Templates

These files are **documentation-grade references**. The authoritative generator
is `src/service.rs`: `kellner service install` renders the same shape with the
absolute path of the running binary and (on macOS) the user's log paths.

| File | Purpose |
| ---- | ------- |
| [`systemd/kellner.service`](systemd/kellner.service) | Linux **user** unit (`systemd_user_unit`). |
| [`launchd/de.zukkafabrik.kellner.plist`](launchd/de.zukkafabrik.kellner.plist) | macOS LaunchAgent (`launchd_plist`, battery-friendly keys). |

Windows has no unit file — `kellner service install` prints a `schtasks /Create`
hint instead (`windows_task_hint`).

## Install

```bash
# Build a release binary (no tray feature) and install it to ~/.local/bin/kellner
./scripts/install.sh

# Custom prefix + register the OS service unit in one go
PREFIX=/usr/local ./scripts/install.sh --service
```

The service unit runs `kellner run` (the power-aware background loop). See the
[README](../README.md) for the platform paths and the follow-up commands
(`launchctl load -w …` / `systemctl --user enable --now …`).

## Smoke test

```bash
./scripts/smoke.sh
```

Offline and CI-safe: it builds the binary and checks `--version`, `--help`
(all subcommands) and `service install --print --dry-run` (prints a unit, writes
nothing). A paired pass is opt-in via `KELLNER_SMOKE_RUN=1`.
