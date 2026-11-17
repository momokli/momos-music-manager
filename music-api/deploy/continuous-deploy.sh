#!/bin/bash
# continuous-deploy.sh (music-api) — `main` → prod, ohne Handgriff (Timer: alle 5 min).
#
# Ablauf je Lauf:
#   1. shallow fetch von origin/main (geteilter Klon mit dem Hub-Deploy), SHA gegen
#      ~/.music-api-deployed-sha vergleichen
#   2. gleich → sofort raus
#   3. music-api/data/... : der Dienst hat KEINE sqlx-Migrationen (das Schema entsteht
#      im Code), deshalb wird bei jedem Deploy gesichert — nicht nur bei Schema-Änderung.
#      Ein Rollback des Binaries heilt keine Schema-Drift.
#   4. `git archive music-api` → ~/music-api-build (nur getrackte Dateien)
#   5. `cargo build --release --locked` (native auf der Ziel-CPU), altes Binary nach
#      ~/music-api.prev
#   6. install nach /opt/music-api/music-api, restart, Health-Gate gegen `gitSha`
#   7. Gate rot → Binary-Rollback + Merkzettel (derselbe Commit wird nicht wiederholt)
#
# Nutzung: continuous-deploy.sh [--dry-run]
# Exit: 0 = ok / nichts zu tun · 3 = Setup-/Build-Fehler · 4 = Health-Gate rot
set -uo pipefail

SRC="${API_DEPLOY_SRC:-$HOME/mmm-deploy-src}"
BUILD="${API_DEPLOY_BUILD:-$HOME/music-api-build}"
TREE="${API_DEPLOY_TREE:-/opt/music-api}"
URL="${HUB_DEPLOY_URL:-https://github.com/momokli/momos-music-manager.git}"
BRANCH="${HUB_DEPLOY_BRANCH:-main}"
CARGO="${CARGO_BIN:-$HOME/.cargo/bin/cargo}"
STATE="$HOME/.music-api-deployed-sha"
FAILED="$HOME/.music-api-deploy-failed"
HEALTH="${API_HEALTH_URL:-http://127.0.0.1:8710/health}"
DB="${API_DB:-/opt/music-api/data/music-api.db}"
UNIT="${API_UNIT:-music-api}"
DRY=0
[ "${1:-}" = "--dry-run" ] && DRY=1

log() { printf '%s api-deploy: %s\n' "$(date -Is)" "$*"; }

# Gleicher Lock wie der Hub-Deploy: beide benutzen denselben Quell-Klon.
exec 9>"$HOME/.mmm-deploy.lock"
if ! flock -n 9; then log "läuft bereits — skip"; exit 0; fi

if [ ! -d "$SRC/.git" ]; then
  log "klone $URL → $SRC"
  git clone --depth 1 --single-branch --branch "$BRANCH" "$URL" "$SRC" -q \
    || { log "FEHLER clone"; exit 3; }
fi
git -C "$SRC" fetch --depth 1 origin "$BRANCH" -q || { log "FEHLER fetch"; exit 3; }
SHA="$(git -C "$SRC" rev-parse FETCH_HEAD)" || { log "FEHLER rev-parse"; exit 3; }
SHORT="${SHA:0:8}"
DESC="$(git -C "$SRC" describe --tags --always FETCH_HEAD 2>/dev/null || echo "$SHORT")"

DEPLOYED="$(cat "$STATE" 2>/dev/null || true)"
[ "$SHA" = "$DEPLOYED" ] && exit 0

if [ -f "$FAILED" ] && [ "$(cat "$FAILED")" = "$SHA" ]; then
  log "SKIP $SHORT — dieser Commit ist beim Deploy gescheitert (wartet auf den nächsten)"
  exit 0
fi

log "neu: $SHORT ($DESC), live: ${DEPLOYED:0:8}"
if [ "$DRY" = 1 ]; then
  log "DRY-RUN: würde syncen, bauen, installieren und restarten"
  exit 0
fi

mkdir -p "$BUILD" || { log "FEHLER build-dir"; exit 3; }
git -C "$SRC" archive FETCH_HEAD music-api | tar -x -C "$BUILD" --strip-components=1 \
  || { log "FEHLER sync"; exit 3; }

sqlite3 "$DB" ".backup $HOME/music-api.db.autodeploy" \
  || { log "FEHLER db-backup"; exit 3; }

cp -a "$TREE/music-api" "$HOME/music-api.prev" 2>/dev/null || true
if ! ( cd "$BUILD" && GIT_SHA="$SHORT" GIT_DESCRIBE="$DESC" "$CARGO" build --release --locked \
        >/tmp/music-api-deploy-build.log 2>&1 ); then
  log "FEHLER build — /tmp/music-api-deploy-build.log"
  tail -3 /tmp/music-api-deploy-build.log
  exit 3
fi

sudo install -m755 "$BUILD/target/release/music-api" "$TREE/music-api" \
  || { log "FEHLER install"; exit 3; }
sudo systemctl restart "$UNIT" || { log "FEHLER restart"; exit 3; }
sleep 5
LIVE="$(curl -s --max-time 5 "$HEALTH" | sed -n 's/.*"gitSha":"\([0-9a-f]*\)".*/\1/p')"

if [ "$LIVE" = "$SHORT" ]; then
  printf '%s\n' "$SHA" > "$STATE"
  rm -f "$FAILED"
  log "OK $SHORT live ($DESC)"
  exit 0
fi

log "HEALTH-GATE ROT (erwartet $SHORT, health: '${LIVE:-keine Antwort}') — Rollback"
[ -f "$HOME/music-api.prev" ] && sudo install -m755 "$HOME/music-api.prev" "$TREE/music-api"
sudo systemctl restart "$UNIT"
printf '%s\n' "$SHA" > "$FAILED"
exit 4
