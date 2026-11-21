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

# ── 5b. a backpack with one source of every kind (tag / playlist / lens / track) ─
# The /backpacks view (#651) renders one card per backpack and one
# `.hub-backpack-sources li` per source, so the seed must exercise all four
# kinds or the screenshot could not prove the marker. The tag, playlist and
# track rows exist only to give the source columns a valid FK target.
say "seeding backpack with one source of each kind"
sqlite3 "$DB" <<SQL
INSERT INTO hub_tags (owner_user_id, slug, name, type, created_at)
VALUES ($UID_, 'demo-fokus', 'Demo Fokus', 'MOOD', strftime('%Y-%m-%dT%H:%M:%SZ','now'));

INSERT INTO hub_playlists (user_id, service, playlist_id, name, track_count, items_available, fetched_at)
VALUES ($UID_, 'spotify', 'demo-playlist-651', 'Demo Playlist', 0, 1,
        strftime('%Y-%m-%dT%H:%M:%SZ','now'));

INSERT INTO hub_tracks (service, service_track_id, title, artists, first_seen_at)
VALUES ('spotify', 'demo-track-651', 'Demo Track', 'Demo Artist',
        strftime('%Y-%m-%dT%H:%M:%SZ','now'));

INSERT INTO hub_backpacks (user_id, name, is_active, created_at, updated_at)
VALUES ($UID_, 'Demo-Quellen', 1,
        strftime('%Y-%m-%dT%H:%M:%SZ','now'), strftime('%Y-%m-%dT%H:%M:%SZ','now'));

INSERT INTO hub_backpack_items
  (backpack_id, source_kind, source_tag_id, source_playlist_id, source_lens,
   source_track_id, position, added_at)
SELECT (SELECT id FROM hub_backpacks WHERE user_id = $UID_ AND name = 'Demo-Quellen'),
       'tag', (SELECT id FROM hub_tags WHERE owner_user_id = $UID_ AND slug = 'demo-fokus'),
       NULL, NULL, NULL, 0, strftime('%Y-%m-%dT%H:%M:%SZ','now')
UNION ALL
SELECT (SELECT id FROM hub_backpacks WHERE user_id = $UID_ AND name = 'Demo-Quellen'),
       'lens', NULL, NULL, 'newest', NULL, 2, strftime('%Y-%m-%dT%H:%M:%SZ','now')
UNION ALL
SELECT (SELECT id FROM hub_backpacks WHERE user_id = $UID_ AND name = 'Demo-Quellen'),
       'track', NULL, NULL, NULL,
       (SELECT id FROM hub_tracks WHERE service = 'spotify' AND service_track_id = 'demo-track-651'),
       3, strftime('%Y-%m-%dT%H:%M:%SZ','now');
SQL

# Add the playlist source through the real form route (not a raw INSERT), so the
# screenshot proves the dropdown's id space matches the resolver (#651).
BP_ID="$(sqlite3 "$DB" "SELECT id FROM hub_backpacks WHERE user_id = $UID_ AND name = 'Demo-Quellen';")"
PL_ID="$(sqlite3 "$DB" "SELECT id FROM hub_playlists WHERE user_id = $UID_ AND playlist_id = 'demo-playlist-651';")"
[ -n "$BP_ID" ] && [ -n "$PL_ID" ] || die "demo backpack/playlist ids missing"
curl -fsS -o /dev/null -b "$TMP/cookies.txt" \
  -d "source_kind=playlist" -d "source_playlist_id=$PL_ID" \
  "http://127.0.0.1:$HUB_PORT/backpacks/$BP_ID/sources" \
  || die "playlist source POST failed"

# ── 5c. devices + comment templates (for /kellner) ─────────────────────────
# /kellner renders per-device settings, so the seed needs a bound device. Two
# devices on purpose: one references a named template (#669), one still carries
# the legacy free-text field — the screenshot has to show both paths.
say "seeding devices + comment templates"
sqlite3 "$DB" <<SQL
INSERT INTO hub_comment_templates (owner_user_id, name, body, is_shared, created_at, updated_at)
VALUES
  ($UID_, 'Traktor DJ', '{tags} {bpm} {key}', 0,
   strftime('%Y-%m-%dT%H:%M:%SZ','now'), strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  ($UID_, 'Geteilt (Team)', '{tags}', 1,
   strftime('%Y-%m-%dT%H:%M:%SZ','now'), strftime('%Y-%m-%dT%H:%M:%SZ','now'));

INSERT INTO hub_devices (user_id, name, token_hash, created_at, paired_at)
VALUES ($UID_, 'MacBook', 'demo-token-macbook',
        strftime('%Y-%m-%dT%H:%M:%SZ','now'), strftime('%Y-%m-%dT%H:%M:%SZ','now'));
INSERT INTO hub_device_settings
  (device_id, preferred_format, comment_template, comment_template_id, updated_at)
VALUES
  ((SELECT id FROM hub_devices WHERE name = 'MacBook'), 'mp3', '',
   (SELECT id FROM hub_comment_templates WHERE name = 'Traktor DJ'),
   strftime('%Y-%m-%dT%H:%M:%SZ','now'));

INSERT INTO hub_devices (user_id, name, token_hash, created_at, paired_at)
VALUES ($UID_, 'Studio', 'demo-token-studio',
        strftime('%Y-%m-%dT%H:%M:%SZ','now'), strftime('%Y-%m-%dT%H:%M:%SZ','now'));
INSERT INTO hub_device_settings
  (device_id, preferred_format, comment_template, comment_template_id, updated_at)
VALUES
  ((SELECT id FROM hub_devices WHERE name = 'Studio'), 'flac', '{tags} BPM-{bpm}', NULL,
   strftime('%Y-%m-%dT%H:%M:%SZ','now'));
SQL

# ── 5d. tracks for the comment-template preview (#670) ──────────────────────
# The preview renders the union of the device's active backpacks. Give the
# active backpack ("Demo-Quellen") twelve resolved tracks with tags + audio, so
# the shot shows a real list, `10 von 12` and the `n/512` counter instead of the
# "Keine aktiven Backpacks" hint.
say "seeding 12 preview tracks (tags + audio) into the active backpack"
sqlite3 "$DB" <<SQL
INSERT INTO hub_tracks (service, service_track_id, title, artists, first_seen_at) VALUES
  ('spotify','demo-preview-01','Preview Track 01','Demo Artist',strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  ('spotify','demo-preview-02','Preview Track 02','Demo Artist',strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  ('spotify','demo-preview-03','Preview Track 03','Demo Artist',strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  ('spotify','demo-preview-04','Preview Track 04','Demo Artist',strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  ('spotify','demo-preview-05','Preview Track 05','Demo Artist',strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  ('spotify','demo-preview-06','Preview Track 06','Demo Artist',strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  ('spotify','demo-preview-07','Preview Track 07','Demo Artist',strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  ('spotify','demo-preview-08','Preview Track 08','Demo Artist',strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  ('spotify','demo-preview-09','Preview Track 09','Demo Artist',strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  ('spotify','demo-preview-10','Preview Track 10','Demo Artist',strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  ('spotify','demo-preview-11','Preview Track 11','Demo Artist',strftime('%Y-%m-%dT%H:%M:%SZ','now')),
  ('spotify','demo-preview-12','Preview Track 12','Demo Artist',strftime('%Y-%m-%dT%H:%M:%SZ','now'));

INSERT OR IGNORE INTO hub_track_resolved_tags (track_id, tag_id)
SELECT t.id, (SELECT id FROM hub_tags WHERE owner_user_id = $UID_ AND slug = 'demo-fokus')
FROM hub_tracks t WHERE t.service = 'spotify' AND t.service_track_id LIKE 'demo-preview-%';

INSERT OR REPLACE INTO hub_track_audio (track_id, bpm, musical_key, camelot, energy, source, updated_at)
SELECT t.id, 120 + (CAST(substr(t.service_track_id, 14) AS INTEGER) % 20), 'A-Minor', '8A', 0.6,
       'demo', strftime('%Y-%m-%dT%H:%M:%SZ','now')
FROM hub_tracks t WHERE t.service = 'spotify' AND t.service_track_id LIKE 'demo-preview-%';

INSERT INTO hub_backpack_items (backpack_id, source_kind, source_track_id, position, added_at)
SELECT (SELECT id FROM hub_backpacks WHERE user_id = $UID_ AND name = 'Demo-Quellen'),
       'track', t.id, 10 + CAST(substr(t.service_track_id, 14) AS INTEGER),
       strftime('%Y-%m-%dT%H:%M:%SZ','now')
FROM hub_tracks t WHERE t.service = 'spotify' AND t.service_track_id LIKE 'demo-preview-%';

INSERT INTO hub_device_backpacks (device_id, backpack_id, position, added_at)
VALUES ((SELECT id FROM hub_devices WHERE user_id = $UID_ AND name = 'MacBook'),
        (SELECT id FROM hub_backpacks WHERE user_id = $UID_ AND name = 'Demo-Quellen'),
        0, strftime('%Y-%m-%dT%H:%M:%SZ','now'));
SQL

# ── 6. screenshots ─────────────────────────────────────────────────────────
say "taking screenshots into $OUT_DIR"
HUB_BASE="http://127.0.0.1:$HUB_PORT" \
HUB_COOKIE="$COOKIE" \
OUT_DIR="$OUT_DIR" \
NODE_PATH="$PW_PREFIX/node_modules" \
  node "$ROOT/scripts/dev/screenshot-hub.cjs"

say "done"
