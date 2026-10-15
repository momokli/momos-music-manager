# Progress — feat/kellner-service-tray (Issue #368, PR #459)

## Rework (Review `[VERDICT: REQUEST_CHANGES]`, 2026-10-15T18:56Z)

### Blocker 1 — Tray „feierabend" beendet den Loop nicht zeitnah ✅
- `kellner/src/sync/run.rs`: Sleep ist jetzt unterbrechbar —
  `sleep_or_stop(wait, stop)` pollt das Stop-Flag alle 500 ms (`STOP_POLL`)
  und kehrt sofort zurück, statt das ganze (power-skalierte) Intervall
  auszusitzen. `run_with_tray` blockiert in `worker.join()` damit nur noch
  ~500 ms.
- Neuer Test: `sync::run::tests::stop_flag_interrupts_the_sleep`
  (base_interval 3600 s, Flag nach 100 ms gesetzt, Rückkehr < 5 s).

### Blocker 2 — Screenshots für Tray-UI ✅ (dokumentierte Ausnahme)
- Tray/Menu-Bar ist eine **native, non-Web**-UI (`tray.rs`), braucht GTK/
  appindicator bzw. macOS-Window-Server — headless nicht renderbar. Ausnahme
  dokumentiert in `kellner/AGENT.md` (§5) und `kellner/README.md`
  („Tray screenshots"). Vertrag stattdessen per Unit-Test
  `tray::tests::menu_has_exactly_feierabend_and_reauth` gepinnt.

### Nicht-blockierende Hinweise (mitgenommen)
- **SIGTERM**: `run` lauscht jetzt auf SIGINT **und** SIGTERM
  (`shutdown_signal()`, Unix) → `systemctl --user stop kellner` meldet
  „shutdown requested".
- **systemd ExecStart-Quoting**: `systemd_quote()` quotet argv-Token mit
  Sonderzeichen/Leerzeichen; Test `systemd_quotes_program_path_with_spaces`.
- **User-Unit-Target**: `network-online.target` → `network.target`
  (`Wants=`/`After=`); Test `systemd_uses_network_target_not_network_online`.
- **MIN_INTERVAL-Floor**: `--no-power-awareness`-Zweig nutzt jetzt
  `base_interval.max(MIN_INTERVAL)` → `--interval-secs 1` umgeht den Hard-Floor
  nicht mehr.

## Verifikation (planet, host=node)
- `cargo build --locked` — EXIT 0, 0 Warnungen
- `cargo test --locked` — EXIT 0 (87 Unit + Integration)
- `cargo check --features tray --locked` — EXIT 0
- `cargo test --features tray --locked` — EXIT 0
- `cargo fmt`/`clippy`: bewusst non-required (informativ, `ci-pr.yml`)
