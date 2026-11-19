#!/usr/bin/env bash
# Boot a throw-away hub + a fake music-api and screenshot the album pages.
#
#   scripts/dev/screenshot.sh [out-dir]        (default: screenshots/)
#
# No network services: the hub runs on a temp SQLite DB, logs in with a local
# bcrypt password, and talks to scripts/dev/mock-music-api.py instead of the
# real music-api on the host. Everything is torn down on exit.
#
# Why local login works: `POST /login` verifies `hub_users.password_hash` with
# bcrypt (music-hub/src/web.rs), Keycloak is only the SSO path on production.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT_DIR="${1:-$ROOT/screenshots}"
HUB_PORT="${HUB_PORT:-18080}"
API_PORT="${MUSIC_API_PORT:-18710}"
PW_VERSION="${PW_VERSION:-1.65.1}"
PW_PREFIX="$ROOT/scripts/dev/.playwright"
USER="demo"
PASS="demo"

TMP="$(mktemp -d)"
HUB_PID=""
API_PID=""
cleanup() {
  [ -n "$HUB_PID" ] && kill "$HUB_PID" 2>/dev/null || true
  [ -n "$API_PID" ] && kill "$API_PID" 2>/dev/null || true
  wait 2>/dev/null || true
  rm -rf "$TMP"
}
trap cleanup EXIT

say() { printf '\033[1m%s\033[0m\n' "$*"; }
die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }

# ── 1. binaries ─────────────────────────────────────────────────────────────
say "building music-hub"
cargo build --manifest-path "$ROOT/music-hub/Cargo.toml" --bin music-hub >/dev/null
TARGET_DIR="$(cargo metadata --manifest-path "$ROOT/music-hub/Cargo.toml" --format-version 1 --no-deps \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
HUB_BIN="$TARGET_DIR/debug/music-hub"
[ -x "$HUB_BIN" ] || die "hub binary not found at $HUB_BIN"

if [ ! -d "$PW_PREFIX/node_modules/playwright" ]; then
  say "installing playwright $PW_VERSION (browser download not needed, system Chrome is used)"
  npm install --prefix "$PW_PREFIX" --silent "playwright@$PW_VERSION" >/dev/null
fi

# ── 2. fake music-api ───────────────────────────────────────────────────────
say "starting mock music-api on :$API_PORT"
MUSIC_API_PORT="$API_PORT" python3 "$ROOT/scripts/dev/mock-music-api.py" &
API_PID=$!
for _ in $(seq 1 50); do
  curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null 2>&1 && break
  sleep 0.1
done
curl -fsS "http://127.0.0.1:$API_PORT/health" >/dev/null || die "mock music-api did not come up"

# ── 3. hub on a fresh DB ────────────────────────────────────────────────────
DB="$TMP/hub.db"
say "starting hub on :$HUB_PORT with a temp DB"
HUB_DATABASE_URL="sqlite:$DB" \
MUSIC_API_BASE="http://127.0.0.1:$API_PORT" \
MUSIC_API_TOKEN="dev-token" \
  "$HUB_BIN" serve --host 127.0.0.1 --port "$HUB_PORT" >"$TMP/hub.log" 2>&1 &
HUB_PID=$!
for _ in $(seq 1 100); do
  curl -fsS -o /dev/null "http://127.0.0.1:$HUB_PORT/login" 2>/dev/null && break
  sleep 0.1
done
curl -fsS -o /dev/null "http://127.0.0.1:$HUB_PORT/login" || die "hub did not come up (see $TMP/hub.log)"

# ── 4. a user with a local password ────────────────────────────────────────
# Migrations run when the hub opens the DB, which can happen just after the
# listener is up — wait for the schema instead of predicting it.
for _ in $(seq 1 100); do
  sqlite3 "$DB" "SELECT 1 FROM hub_users LIMIT 1;" >/dev/null 2>&1 && break
  sleep 0.1
done
sqlite3 "$DB" "SELECT 1 FROM hub_users LIMIT 1;" >/dev/null 2>&1 || die "hub did not migrate the DB"

say "creating dev user '$USER'"
sqlite3 "$DB" "INSERT INTO hub_settings(key, value, updated_at)
               VALUES('registration_open', '1', strftime('%Y-%m-%dT%H:%M:%SZ','now'))
               ON CONFLICT(key) DO UPDATE SET value='1';"
curl -fsS -o /dev/null -c "$TMP/cookies.txt" \
  -d "username=$USER" -d "password=$PASS" \
  "http://127.0.0.1:$HUB_PORT/signup" || die "signup failed"
COOKIE="$(awk '$6 == "hub_session" {print $6"="$7}' "$TMP/cookies.txt")"
[ -n "$COOKIE" ] || die "no hub_session cookie after signup"

# ── 5. a tonie binding whose source is the mock artist + a cart ────────────
say "seeding binding + cart for ct1"
UID_="$(sqlite3 "$DB" "SELECT id FROM hub_users WHERE slug = '$USER';")"
sqlite3 "$DB" <<SQL
INSERT INTO hub_tonie_bindings
  (user_id, tonie_id, tonie_name, source_kind, source_id, format, cap_seconds,
   mode, album_mode, order_by, created_at, updated_at)
VALUES
  ($UID_, 'ct1', 'Der Kreativ-Tonie', 'albums', 15167501, 'mp3-128', 5400,
   'manual', 'basket', 'playlist',
   strftime('%Y-%m-%dT%H:%M:%SZ','now'), strftime('%Y-%m-%dT%H:%M:%SZ','now'));

INSERT INTO hub_tonie_cart_items
  (user_id, tonie_id, position, album_id, title, cover_url, seconds, added_at)
VALUES
  ($UID_, 'ct1', 0, '1137915301', 'Folge 459: Die Robo-Ente',
   'http://127.0.0.1:$API_PORT/cover/1137915301.svg', 1174, strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  ($UID_, 'ct1', 1, '1137915302', 'Folge 458: Wilder Ritt auf dem Dino',
   'http://127.0.0.1:$API_PORT/cover/1137915302.svg', 1290, strftime('%Y-%m-%dT%H:%M:%SZ','now'));

SQL

# ── 6. screenshots ─────────────────────────────────────────────────────────
say "taking screenshots into $OUT_DIR"
HUB_BASE="http://127.0.0.1:$HUB_PORT" \
HUB_COOKIE="$COOKIE" \
OUT_DIR="$OUT_DIR" \
NODE_PATH="$PW_PREFIX/node_modules" \
  node "$ROOT/scripts/dev/screenshot-hub.cjs"

say "done"
