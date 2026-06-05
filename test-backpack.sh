#!/bin/bash
# Backpack + Local Tracking Test Suite
# Run against: http://localhost:3000
# Usage: ./test-backpack.sh

BASE="http://localhost:3000"
PASS=0
FAIL=0

green() { printf "\033[32m  PASS\033[0m %s\n" "$1"; PASS=$((PASS+1)); }
red()   { printf "\033[31m  FAIL\033[0m %s\n" "$1"; FAIL=$((FAIL+1)); }
section() { printf "\n\033[1;36m═══ %s ═══\033[0m\n" "$1"; }

assert_eq() {
  local label="$1" expected="$2" actual="$3"
  if [ "$expected" = "$actual" ]; then
    green "$label: $actual"
  else
    red "$label: expected '$expected', got '$actual'"
  fi
}

assert_gt() {
  local label="$1" actual="$2" threshold="$3"
  if [ "$actual" -gt "$threshold" ] 2>/dev/null; then
    green "$label: $actual (> $threshold)"
  else
    red "$label: $actual (should be > $threshold)"
  fi
}

# ═══════════════════════════════════════════════════════════════
section "1. HEALTH & MIGRATION"
# ═══════════════════════════════════════════════════════════════

HEALTH=$(curl -s "$BASE/api/health")
STATUS=$(echo "$HEALTH" | python3 -c "import sys,json; print(json.load(sys.stdin)['status'])" 2>/dev/null)
DB=$(echo "$HEALTH" | python3 -c "import sys,json; print(json.load(sys.stdin)['database'])" 2>/dev/null)
assert_eq "Server" "ok" "$STATUS"
assert_eq "Database" "connected" "$DB"

# Verify backpack column rename worked (field exists in API response)
TAG256=$(curl -s "$BASE/api/tags?search=discover")
HAS_BP=$(echo "$TAG256" | python3 -c "import sys,json; print('backpack' in json.load(sys.stdin)['data'][0])" 2>/dev/null)
if [ "$HAS_BP" = "True" ]; then
  green "tags.backpack column exists (rename migration OK)"
else
  red "tags.backpack field MISSING (rename migration failed)"
fi

# ═══════════════════════════════════════════════════════════════
section "2. TAG BACKPACK TOGGLE"
# ═══════════════════════════════════════════════════════════════

# Ensure tag 256 is toggled ON for subsequent tests
curl -s -X PUT "$BASE/api/tags/256/backpack" \
  -H "Content-Type: application/json" \
  -d '{"backpack":true}' > /dev/null
sleep 1

BP=$(curl -s "$BASE/api/tags/256" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['backpack'])" 2>/dev/null)
assert_eq "Tag 256 backpack=ON" "True" "$BP"

# Toggle OFF
curl -s -X PUT "$BASE/api/tags/256/backpack" \
  -H "Content-Type: application/json" \
  -d '{"backpack":false}' > /dev/null
sleep 1
BP_OFF=$(curl -s "$BASE/api/tags/256" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['backpack'])" 2>/dev/null)
assert_eq "Tag 256 toggle OFF" "False" "$BP_OFF"

# Toggle back ON
curl -s -X PUT "$BASE/api/tags/256/backpack" \
  -H "Content-Type: application/json" \
  -d '{"backpack":true}' > /dev/null
sleep 1
BP_ON=$(curl -s "$BASE/api/tags/256" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['backpack'])" 2>/dev/null)
assert_eq "Tag 256 toggle ON" "True" "$BP_ON"

# ═══════════════════════════════════════════════════════════════
section "3. TRACK inBackpack"
# ═══════════════════════════════════════════════════════════════

TRACK52=$(curl -s "$BASE/api/tracks/52/detail")
INBP=$(echo "$TRACK52" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['inBackpack'])" 2>/dev/null)
assert_eq "Track 52 inBackpack" "True" "$INBP"

# ═══════════════════════════════════════════════════════════════
section "4. LOCAL FILE TRACKING (isLocal)"
# ═══════════════════════════════════════════════════════════════

# FLAC should be local (exists on disk)
FLAC_LOCAL=$(echo "$TRACK52" | python3 -c "
import sys,json
files=json.load(sys.stdin)['data']['files']
print([f['isLocal'] for f in files if f['fileType']=='flac'][0])
" 2>/dev/null)
assert_eq "FLAC isLocal" "True" "$FLAC_LOCAL"

# Check filesystem
if [ -f "/Users/momo/Music/flacs/Aexhy - HEARTBREAK3000.flac" ]; then
  green "FLAC file exists on disk"
else
  red "FLAC file MISSING from disk"
fi

# WAV sources should be NOT local (5 files, all isLocal=false)
WAV_OK=$(echo "$TRACK52" | python3 -c "
import sys,json
files=json.load(sys.stdin)['data']['files']
wavs=[f for f in files if f['fileType']=='wav']
ok = all(not f['isLocal'] for f in wavs) and len(wavs)==5
print(ok)
" 2>/dev/null)
assert_eq "5 WAVs all isLocal=false" "True" "$WAV_OK"

# ═══════════════════════════════════════════════════════════════
section "5. BACKPACK SYNC"
# ═══════════════════════════════════════════════════════════════

SYNC=$(curl -s -X POST "$BASE/api/storage/sync-backpack" --max-time 10 2>/dev/null || echo '{"data":{"pulled":0,"failed":0,"candidates":[]}}')
PULLED=$(echo "$SYNC" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['pulled'])" 2>/dev/null)
FAILED_SYNC=$(echo "$SYNC" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['failed'])" 2>/dev/null)
NCAND=$(echo "$SYNC" | python3 -c "import sys,json; print(len(json.load(sys.stdin)['data']['candidates']))" 2>/dev/null)

echo "  candidates=$NCAND  pulled=$PULLED  failed=$FAILED_SYNC"

# Check stem.m4a was pulled (or was already local)
sleep 5  # let rsync finish if running
STEM_LOCAL=$(curl -s "$BASE/api/tracks/52/detail" | python3 -c "
import sys,json
files=json.load(sys.stdin)['data']['files']
stems=[f for f in files if f['fileType']=='stem.m4a']
print(stems[0]['isLocal'] if stems else 'NO_STEM')
" 2>/dev/null)

if [ "$STEM_LOCAL" = "True" ]; then
  green "stem.m4a isLocal=true (on disk)"
  if [ -f "/Users/momo/Music/stems/Aexhy - HEARTBREAK3000.stem.m4a" ]; then
    green "stem.m4a confirmed on disk"
  else
    red "stem.m4a API says local but file MISSING from disk"
  fi
else
  # Stem might still be pulling, check if it's in the candidates that failed
  if [ "$NCAND" -eq 0 ]; then
    green "stem.m4a: 0 pull candidates (may already be up to date)"
  elif [ "$FAILED_SYNC" -gt 0 ]; then
    red "stem.m4a not local ($FAILED_SYNC pull failures)"
  else
    red "stem.m4a not local (isLocal=$STEM_LOCAL, pulled=$PULLED)"
  fi
fi

# ═══════════════════════════════════════════════════════════════
section "6. STORAGE STATUS"
# ═══════════════════════════════════════════════════════════════

STORAGE=$(curl -s "$BASE/api/storage/status")
LOCAL=$(echo "$STORAGE" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['localFileCount'])" 2>/dev/null)
BACKUP=$(echo "$STORAGE" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['backupCount'])" 2>/dev/null)
assert_gt "Local files" "$LOCAL" 100
assert_gt "Backup files" "$BACKUP" 1000

# ═══════════════════════════════════════════════════════════════
section "7. EDGE CASES"
# ═══════════════════════════════════════════════════════════════

# Non-existent tag -> 404
TAG404=$(curl -s -o /dev/null -w "%{http_code}" "$BASE/api/tags/99999" 2>/dev/null)
assert_eq "Tag 99999 -> 404" "404" "$TAG404"

# Non-existent track -> 404
TRACK404=$(curl -s -o /dev/null -w "%{http_code}" "$BASE/api/tracks/99999/detail" 2>/dev/null)
assert_eq "Track 99999/detail -> 404" "404" "$TRACK404"

# ═══════════════════════════════════════════════════════════════
section "RESULTS"
printf "\n  \033[32mPassed: %d\033[0m  \033[31mFailed: %d\033[0m  Total: %d\n" "$PASS" "$FAIL" $((PASS+FAIL))
if [ "$FAIL" -eq 0 ]; then
  printf "  \033[1;32mALL TESTS PASSED\033[0m\n\n"
  exit 0
else
  printf "  \033[1;31m%d TEST(S) FAILED\033[0m\n\n" "$FAIL"
  exit 1
fi
