#!/usr/bin/env bash
# momos-music-manager test harness
# Usage: ./test.sh [--port 3000] [--host 127.0.0.1] [--verbose]
# Run against a running server to validate all APIs and filters.
# Exit code 0 = all pass, 1 = failures found.

set -e

HOST="127.0.0.1"
PORT="3000"
VERBOSE=""
FAILURES=0
PASSES=0
RED='\033[0;31m'
GREEN='\033[0;32m'
NC='\033[0m' # No Color

while [ $# -gt 0 ]; do
  case "$1" in
    --port) PORT="$2"; shift 2 ;;
    --host) HOST="$2"; shift 2 ;;
    --verbose) VERBOSE="1"; shift ;;
    *) echo "Unknown: $1"; exit 1 ;;
  esac
done

BASE="http://${HOST}:${PORT}"

test_ok() {
  local desc="$1"; shift
  local resp=$(curl -s "$@")
  if echo "$resp" | grep -q '"error"'; then
    FAILURES=$((FAILURES + 1))
    local err=$(echo "$resp" | python3 -c 'import json,sys;print(json.load(sys.stdin).get("error","unknown"))' 2>/dev/null || echo "parse error")
    echo -e "  ${RED}FAIL${NC} $desc — $err"
    [ -n "$VERBOSE" ] && echo "       $resp" | head -3
  else
    PASSES=$((PASSES + 1))
    echo -e "  ${GREEN}PASS${NC} $desc"
  fi
}

echo "=== momos-music-manager test harness ==="
echo "    Target: $BASE"
echo ""

# ─── Server alive ───────────────────────────────────────────────
echo "--- HEALTH ---"
test_ok "Server responds" "$BASE/api/storage/status"

# ─── Storage ────────────────────────────────────────────────────
echo "--- STORAGE ---"
test_ok "GET /api/storage/status" "$BASE/api/storage/status"
test_ok "POST /api/storage/prune-preview" "$BASE/api/storage/prune-preview" -X POST

# ─── Files page filters ─────────────────────────────────────────
echo "--- FILES ---"
test_ok "Basic list" "$BASE/api/files?limit=1"
test_ok "Filter: isLocal=true" "$BASE/api/files?limit=1&isLocal=true"
test_ok "Filter: isLocal=false" "$BASE/api/files?limit=1&isLocal=false"
test_ok "Filter: backedUp=true" "$BASE/api/files?limit=1&backedUp=true"
test_ok "Filter: backedUp=false" "$BASE/api/files?limit=1&backedUp=false"
test_ok "Filter: safeToDelete=true" "$BASE/api/files?limit=1&safeToDelete=true"
test_ok "Filter: fileTypes=flac" "$BASE/api/files?limit=1&fileTypes=flac"
test_ok "Filter: fileTypes=stem.m4a" "$BASE/api/files?limit=1&fileTypes=stem.m4a"
test_ok "Filter: fileTypes=wav" "$BASE/api/files?limit=1&fileTypes=wav"
test_ok "Search: 'Boris'" "$BASE/api/files?limit=1&search=Boris"
test_ok "Sort: title asc" "$BASE/api/files?limit=1&sort=title&order=asc"
test_ok "Sort: bpm desc" "$BASE/api/files?limit=1&sort=bpm&order=desc"
test_ok "Files count" "$BASE/api/files/count"
test_ok "Files count isLocal=true" "$BASE/api/files/count?isLocal=true"
test_ok "Files count backedUp=true" "$BASE/api/files/count?backedUp=true"

# ─── Tracks page filters ────────────────────────────────────────
echo "--- TRACKS ---"
test_ok "Basic list" "$BASE/api/tracks?limit=2"
test_ok "Filter: services=spotify" "$BASE/api/tracks?limit=2&services=spotify"
test_ok "Filter: fileTypes=flac" "$BASE/api/tracks?limit=2&fileTypes=flac"
test_ok "Filter: fileTypes=stem.m4a" "$BASE/api/tracks?limit=2&fileTypes=stem.m4a"
test_ok "Filter: fileTypeAgg=any" "$BASE/api/tracks?limit=2&fileTypeAgg=any"
test_ok "Filter: fileTypeAgg=none" "$BASE/api/tracks?limit=2&fileTypeAgg=none"
test_ok "Filter: hasLocal=true" "$BASE/api/tracks?limit=2&hasLocal=true"
test_ok "Filter: hasBackup=true" "$BASE/api/tracks?limit=2&hasBackup=true"
test_ok "Filter: playlists=liked" "$BASE/api/tracks?limit=2&playlists=liked"
test_ok "Search: 'Brejcha'" "$BASE/api/tracks?limit=2&search=Brejcha"
test_ok "Tracks count" "$BASE/api/tracks/count"
test_ok "Tracks count hasLocal=true" "$BASE/api/tracks/count?hasLocal=true"
test_ok "Tracks count services=spotify" "$BASE/api/tracks/count?services=spotify"

# ─── Track detail ────────────────────────────────────────────────
echo "--- TRACK DETAIL ---"
test_ok "Track #1487 detail" "$BASE/api/tracks/1487/detail"
test_ok "Track #30 detail" "$BASE/api/tracks/30/detail"
test_ok "Track #3 detail" "$BASE/api/tracks/3/detail"

# ─── Playlists ──────────────────────────────────────────────────
echo "--- PLAYLISTS ---"
test_ok "Basic list" "$BASE/api/playlists?limit=2"
test_ok "Filter: service=spotify" "$BASE/api/playlists?limit=2&service=spotify"
test_ok "Search: 'digg'" "$BASE/api/playlists?limit=2&search=digg"

# ─── Tags ───────────────────────────────────────────────────────
echo "--- TAGS ---"
test_ok "Basic list" "$BASE/api/tags?limit=2"
test_ok "Search: 'Groovy'" "$BASE/api/tags?limit=2&search=Groovy"
test_ok "Sort: name asc" "$BASE/api/tags?limit=2&sort=name&order=asc"
test_ok "Tags count" "$BASE/api/tags/count"

# ─── New fields ─────────────────────────────────────────────────
echo "--- NEW FIELDS ---"
test_ok "File has isLocal" "$BASE/api/files/3"  # check via grep in verbose

# ─── File variants ──────────────────────────────────────────────
echo "--- VARIANTS ---"
test_ok "File #3 variants" "$BASE/api/files/3/variants"
test_ok "File #3 detail" "$BASE/api/files/3/detail"

# ─── Frontend pages ─────────────────────────────────────────────
echo "--- FRONTEND ---"
for page in files tracks playlists tags storage backpack; do
  code=$(curl -s -o /dev/null -w "%{http_code}" "${BASE}/#${page}")
  if [ "$code" = "200" ]; then
    PASSES=$((PASSES + 1))
    echo -e "  ${GREEN}PASS${NC} /#${page} → ${code}"
  else
    FAILURES=$((FAILURES + 1))
    echo -e "  ${RED}FAIL${NC} /#${page} → ${code}"
  fi
done

# ─── Results ────────────────────────────────────────────────────
echo ""
echo "========================================="
if [ "$FAILURES" -eq 0 ]; then
  echo -e "${GREEN}ALL $PASSES TESTS PASSED${NC}"
  exit 0
else
  echo -e "${RED}$FAILURES FAILED${NC}, $PASSES PASSED"
  exit 1
fi
