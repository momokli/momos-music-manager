#!/usr/bin/env bash
# One-time server-side rename: mmm-hub -> music-hub (2026-10-12).
#
# Run ON the music-catalog host (192.168.178.200) as the service owner (momo):
#     ssh music-catalog '~/mmm-hub/deploy/rename-music-hub.sh'
#
# Prereq: the NEW tree (incl. deploy/music-hub.service + this script) is already
# synced into ~/mmm-hub (see music-hub/AGENT.md "Server rename"). Idempotent.
#
# What it does: stop old unit -> move dir -> fix .env paths -> install + enable
# the music-hub unit -> verify.
#
# NOT handled here (check manually, may point at the old path):
#   - Caddy on `lan`: only proxies host:port, no path change expected.
#   - Datasette / SchemaSpy jobs for `hub-public.db` (make-public-db.sh cron).
#   - Any external backup/rsync cron that hardcodes /home/momo/mmm-hub.
set -euo pipefail

OLD_DIR="$HOME/mmm-hub"
NEW_DIR="$HOME/music-hub"
OLD_UNIT="mmm-hub"
NEW_UNIT="music-hub"
UNIT_DIR="/etc/systemd/system"
NEW_UNIT_SRC="$NEW_DIR/deploy/music-hub.service"

say() { printf '== %s\n' "$*"; }

if [ -d "$NEW_DIR" ] && [ ! -d "$OLD_DIR" ]; then
  say "already renamed ($NEW_DIR exists, $OLD_DIR gone) — nothing to do"
  exit 0
fi
if [ ! -d "$OLD_DIR" ]; then
  echo "fatal: $OLD_DIR not found (unexpected state) — aborting" >&2
  exit 1
fi
if [ ! -f "$OLD_DIR/deploy/music-hub.service" ]; then
  echo "fatal: $OLD_DIR/deploy/music-hub.service missing — sync the new tree into ~/mmm-hub first" >&2
  exit 1
fi

say "stopping $OLD_UNIT"
sudo systemctl stop "$OLD_UNIT" 2>/dev/null || true

say "moving $OLD_DIR -> $NEW_DIR"
mv "$OLD_DIR" "$NEW_DIR"

say "rewriting absolute paths in .env (mmm-hub -> music-hub)"
if [ -f "$NEW_DIR/.env" ]; then
  sed -i.bak 's#/mmm-hub#/music-hub#g' "$NEW_DIR/.env" && rm -f "$NEW_DIR/.env.bak"
fi

say "installing systemd unit $NEW_UNIT"
sudo install -m644 "$NEW_UNIT_SRC" "$UNIT_DIR/$NEW_UNIT.service"
sudo rm -f "$UNIT_DIR/$OLD_UNIT.service"
sudo systemctl daemon-reload
sudo systemctl enable --now "$NEW_UNIT"

say "verify"
sleep 2
sudo systemctl is-active "$NEW_UNIT"
if curl -fsS -o /dev/null "http://127.0.0.1:8080/"; then
  echo "http ok"
else
  echo "(root not 200 — check: journalctl -u $NEW_UNIT -n 50)"
fi
say "done — the deploy loop in music-hub/AGENT.md is now literal"
