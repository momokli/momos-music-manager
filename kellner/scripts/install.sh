#!/usr/bin/env bash
# Build and install the kellner binary (issue #370, "Install + smoke tests").
#
# Builds a release binary **without** the `tray` feature and installs it to
# `${PREFIX:-$HOME/.local}/bin/kellner`. Idempotent (re-running overwrites the
# installed binary); never touches the network. Pass --service to additionally
# register the OS service unit via `kellner service install`.
#
#   ./scripts/install.sh                 # install to ~/.local/bin
#   PREFIX=/usr/local ./scripts/install.sh
#   ./scripts/install.sh --service       # install binary + register the unit
#
# For a quick, offline check of an installed/side-by-side binary use
# `./scripts/smoke.sh` instead.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
WITH_SERVICE=0

usage() {
  cat <<'EOF'
usage: install.sh [--service] [--prefix DIR] [-h|--help]

  --service        also run `kellner service install` after installing
  --prefix DIR     install prefix (default: ${PREFIX:-$HOME/.local});
                   the binary lands in DIR/bin/kellner
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --service) WITH_SERVICE=1; shift ;;
    --prefix)  PREFIX="${2:?--prefix needs a directory}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) echo "install: unknown option '$1'" >&2; usage >&2; exit 2 ;;
  esac
done

cd "$ROOT"
echo "install: building kellner (cargo build --release --locked, no tray feature)"
cargo build --release --locked

BIN="$ROOT/target/release/kellner"
DEST_DIR="$PREFIX/bin"
DEST="$DEST_DIR/kellner"

mkdir -p "$DEST_DIR"
install -m 0755 "$BIN" "$DEST"
echo "install: installed $DEST"

if [[ "$WITH_SERVICE" == 1 ]]; then
  echo "install: registering the OS service unit"
  "$DEST" service install
fi

echo "install: done (run 'kellner --version' or '$ROOT/scripts/smoke.sh' to verify)"
