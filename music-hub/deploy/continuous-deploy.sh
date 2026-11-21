#!/bin/bash
# continuous-deploy.sh — `main` → prod, ohne Handgriff (Timer: alle 5 min).
#
# Ablauf je Lauf:
#   1. shallow fetch von origin/main; SHA gegen ~/music-hub/.deployed-sha vergleichen
#   2. gleich → sofort raus (kein Log-Spam, kein Build)
#   3. neue Migrationen? → hub.db sichern (nur dann; `.backup` ist online-konsistent)
#   4. `git archive` → in ~/music-hub entpacken (nur getrackte Dateien —
#      hub.db, .env und target/ werden nie angefasst)
#   5. `cargo build --release`, altes Binary nach ~/music-hub.prev
#   6. restart + Health-Gate gegen `gitSha` aus /api/hub/health
#   7. Gate rot → Rollback auf music-hub.prev + Merkzettel, damit derselbe Commit
#      nicht in einer Build-Schleife landet (der nächste Commit versucht es neu)
#
# Warum pull-basiert: das Repo ist öffentlich, der Host braucht also keinen Token,
# und die DB bleibt dort, wo sie ist. Kein Push-Zugang zu prod nötig.
#
# Nutzung: continuous-deploy.sh [--dry-run]
# Exit: 0 = ok / nichts zu tun · 3 = Setup-/Build-Fehler · 4 = Health-Gate rot (zurückgerollt)
#       5 = neue Migration mit zerstörendem SQL (blockiert)
set -uo pipefail

SRC="${HUB_DEPLOY_SRC:-$HOME/mmm-deploy-src}"
TREE="${HUB_DEPLOY_TREE:-$HOME/music-hub}"
URL="${HUB_DEPLOY_URL:-https://github.com/momokli/momos-music-manager.git}"
BRANCH="${HUB_DEPLOY_BRANCH:-main}"
CARGO="${CARGO_BIN:-$HOME/.cargo/bin/cargo}"
STATE="$TREE/.deployed-sha"
FAILED="$HOME/.hub-deploy-failed"
HEALTH="${HUB_HEALTH_URL:-http://127.0.0.1:8080/api/hub/health}"
DRY=0
[ "${1:-}" = "--dry-run" ] && DRY=1

log() { printf '%s hub-deploy: %s\n' "$(date -Is)" "$*"; }

# Überlappung mit einem manuellen Lauf verhindern (Build dauert ~1,5 min).
# Gemeinsamer Lock mit dem api-Deploy: beide benutzen denselben Quell-Klon.
exec 9>"$HOME/.mmm-deploy.lock"
# NICHT `-n`: beide Timer (hub/api) feuern in derselben Sekunde und teilen den
# Quell-Klon. Mit "nicht warten" verliert der zweite Deploy jede Runde und faellt
# still aus. Warten ist gedeckelt durch TimeoutStartSec=900 der Units.
if ! flock -w 900 9; then log "wartet auf anderen Deploy (Timeout)"; exit 0; fi

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
  log "DRY-RUN: würde syncen, bauen und restarten"
  exit 0
fi

# Nur bei neuen Migrationen sichern. Die SQLx-Checksummen machen bestehende
# Migrationen unveränderlich; das Risiko sitzt in der *neuen* Migration.
NEW_MIG="$(git -C "$SRC" ls-tree -r --name-only FETCH_HEAD music-hub/migrations | wc -l)"
APPLIED="$(sqlite3 "$TREE/hub.db" 'SELECT count(*) FROM _sqlx_migrations' 2>/dev/null || echo 0)"
if [ "$NEW_MIG" -gt "$APPLIED" ]; then
  log "neue Migrationen ($APPLIED → $NEW_MIG): sichere hub.db"
  sqlite3 "$TREE/hub.db" ".backup $HOME/hub.db.autodeploy" || { log "FEHLER db-backup"; exit 3; }

  # Zerstörende Migrationen blockieren. Die SQLx-Transaktion plus Health-Gate fangen
  # eine *fehlschlagende* Migration ab — nicht eine, die erfolgreich Daten vernichtet
  # (das DB-Backup hilft dann nur unter Verlust aller seither geschriebenen Daten).
  MAXAPPLIED="$(sqlite3 "$TREE/hub.db" 'SELECT COALESCE(MAX(version),0) FROM _sqlx_migrations' 2>/dev/null || echo 0)"
  DESTRUCTIVE="$(git -C "$SRC" ls-tree -r --name-only FETCH_HEAD music-hub/migrations | while IFS= read -r f; do
      [ -n "$f" ] || continue
      v="$(basename "$f" | sed 's/^0*//; s/_.*//')"
      [ "$v" -gt "$MAXAPPLIED" ] 2>/dev/null || continue
      git -C "$SRC" show "FETCH_HEAD:$f" 2>/dev/null \
        | grep -inE '\b(drop[[:space:]]+(table|column|index)|delete[[:space:]]+from|truncate|rename[[:space:]]+to)\b' \
        | sed "s|^|$(basename "$f"):|"
    done)"
  if [ -n "$DESTRUCTIVE" ] && [ "${HUB_DEPLOY_ALLOW_DESTRUCTIVE:-0}" != "1" ]; then
    log "BLOCKIERT — neue Migration mit zerstörendem SQL (Binary-Rollback deckt das nicht ab):"
    printf '%s\n' "$DESTRUCTIVE" | head -10
    log "Freigabe nur bewusst: HUB_DEPLOY_ALLOW_DESTRUCTIVE=1 (vorher DB-Backup ansehen)"
    exit 5
  fi
fi

git -C "$SRC" archive FETCH_HEAD music-hub | tar -x -C "$TREE" --strip-components=1 \
  || { log "FEHLER sync"; exit 3; }

# `music-hub` deklariert seit #664 `comment-template = { path = "../comment-template" }`.
# Der Tree oben traegt nur music-hub/, also muss die Schwester-Crate daneben
# liegen - sonst scheitert cargo mit
#   "unable to update /home/momo/comment-template: No such file or directory".
# Reihenfolge egal: erst nach dem Entpacken liegt die neue music-hub/Cargo.toml.
mkdir -p "$HOME/comment-template"
git -C "$SRC" archive FETCH_HEAD comment-template \
  | tar -x -C "$HOME/comment-template" --strip-components=1 \
  || { log "FEHLER sync comment-template"; exit 3; }

cp -a "$TREE/target/release/music-hub" "$HOME/music-hub.prev" 2>/dev/null || true
if ! ( cd "$TREE" && GIT_SHA="$SHORT" GIT_DESCRIBE="$DESC" "$CARGO" build --release --locked \
        >/tmp/hub-deploy-build.log 2>&1 ); then
  log "FEHLER build — /tmp/hub-deploy-build.log"
  tail -3 /tmp/hub-deploy-build.log
  exit 3
fi

sudo systemctl restart music-hub || { log "FEHLER restart"; exit 3; }
sleep 6
LIVE="$(curl -s --max-time 5 "$HEALTH" | sed -n 's/.*"gitSha":"\([0-9a-f]*\)".*/\1/p')"

if [ "$LIVE" = "$SHORT" ]; then
  printf '%s\n' "$SHA" > "$STATE"
  rm -f "$FAILED"
  log "OK $SHORT live ($DESC)"
  exit 0
fi

log "HEALTH-GATE ROT (erwartet $SHORT, health: '${LIVE:-keine Antwort}') — Rollback"
cp -a "$HOME/music-hub.prev" "$TREE/target/release/music-hub" 2>/dev/null
sudo systemctl restart music-hub
printf '%s\n' "$SHA" > "$FAILED"
exit 4
