#!/usr/bin/env bash
# Offline smoke test for a kellner build (issue #370, "Install + smoke tests").
#
# Builds the binary (unless KELLNER_BIN points at one) and then asserts only the
# CLI surface that must work **with no network, no pairing and no GTK/tray**:
#   * `kellner --version` exits 0 and names the program;
#   * `kellner --help` lists every subcommand;
#   * `kellner service install --print --dry-run` exits 0, prints the unit text
#     and writes nothing.
# CI-safe: no hub / music-api contact, no filesystem writes outside a tempdir.
#
# A paired dry pass (`kellner run --once`) needs a paired device and is therefore
# opt-in: set KELLNER_SMOKE_RUN=1 (see the tail of this script).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

fail() { echo "smoke: FAIL: $*" >&2; exit 1; }

if [[ -z "${KELLNER_BIN:-}" ]]; then
  echo "smoke: building kellner (cargo build --locked)"
  cargo build --locked
  # Honour CARGO_TARGET_DIR (e.g. a shared ~/.cargo-target); relative values
  # resolve against $ROOT, exactly like cargo does.
  TARGET_DIR="${CARGO_TARGET_DIR:-target}"
  case "$TARGET_DIR" in
    /*) : ;;
    *) TARGET_DIR="$ROOT/$TARGET_DIR" ;;
  esac
  KELLNER_BIN="$TARGET_DIR/debug/kellner"
fi
[[ -x "$KELLNER_BIN" ]] || fail "binary not executable: $KELLNER_BIN"

# 1. --version: exit 0 and names the program.
ver="$("$KELLNER_BIN" --version)" || fail "--version exited non-zero"
[[ "$ver" == kellner* ]] || fail "--version did not name the program: $ver"
echo "smoke: version OK — $ver"

# 2. --help: lists every subcommand (skeleton must stay complete).
help="$("$KELLNER_BIN" --help)" || fail "--help exited non-zero"
for cmd in pair backpack backup traktor-upload downloads run service; do
  grep -q "$cmd" <<<"$help" || fail "--help does not list the '$cmd' command"
done
echo "smoke: help OK — all subcommands listed"

# 3. service install --print --dry-run: prints a unit, writes nothing.
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

unit="$("$KELLNER_BIN" service install --print --dry-run)" || fail "service install --print --dry-run exited non-zero"
[[ -n "$unit" ]] || fail "service install --print --dry-run printed no unit text"
grep -qE 'ExecStart=|ProgramArguments|schtasks' <<<"$unit" \
  || fail "printed text is not a service unit/registration"

# Re-run with HOME/XDG_CONFIG_HOME redirected at the tempdir; a print must not write.
HOME="$tmp" XDG_CONFIG_HOME="$tmp/cfg" "$KELLNER_BIN" service install --print --dry-run >/dev/null \
  || fail "service install --print --dry-run failed with a redirected HOME"
[[ -z "$(ls -A "$tmp")" ]] || fail "service install --print --dry-run wrote files under HOME"
echo "smoke: service install --print --dry-run OK — unit printed, nothing written"

# Optional: a paired dry pass. Requires a device already paired with the hub, so
# it is never part of the offline smoke unless explicitly requested.
if [[ "${KELLNER_SMOKE_RUN:-0}" == "1" ]]; then
  echo "smoke: KELLNER_SMOKE_RUN=1 — running a paired dry pass (needs a paired device)"
  "$KELLNER_BIN" run --once --no-power-awareness || fail "paired dry pass failed"
fi

echo "smoke: PASS"
