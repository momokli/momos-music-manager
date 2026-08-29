#!/bin/bash
# test_e2e.sh — End-to-end test for wish.zukkafabrik.de
set -e
HOST="localhost:8700"
PASS=0; FAIL=0

ok() { echo "  ✅ $1"; PASS=$((PASS+1)); }
fail() { echo "  ❌ $1: $2"; FAIL=$((FAIL+1)); }

echo "=== E2E Test wish.zukkafabrik.de ==="

# 1. Health
echo "── Health ──"
H=$(curl -s http://$HOST/health)
echo "$H" | python3 -c "import json,sys; d=json.load(sys.stdin); exit(0 if d['deemix_arl_configured'] else 1)" && ok "deemix ready" || fail "deemix" "$H"
echo "$H" | python3 -c "import json,sys; d=json.load(sys.stdin); exit(0 if d['spotify_configured'] else 1)" && ok "spotify ready" || fail "spotify" "$H"

# 2. Search
echo "── Search ──"
S=$(curl -s "http://$HOST/search?q=Dancing+Queen&limit=2")
SC=$(echo "$S" | python3 -c "import json,sys; print(len(json.load(sys.stdin)['spotify']))")
YC=$(echo "$S" | python3 -c "import json,sys; print(len(json.load(sys.stdin).get('youtube',[])))")
[ "$SC" -ge 1 ] && ok "spotify: $SC results" || fail "spotify" "$SC"
[ "$YC" -ge 1 ] && ok "youtube: $YC results" || fail "youtube" "$YC"

# 3. Page
echo "── Page ──"
PCODE=$(curl -s -o /dev/null -w '%{http_code}' http://$HOST/)
PSIZE=$(curl -s http://$HOST/ | wc -c)
[ "$PCODE" = "200" ] && ok "page 200 ($PSIZE bytes)" || fail "page" "$PCODE"
curl -s http://$HOST/ | grep -q 'search-input' && ok "search input" || fail "search input" "missing"
curl -s http://$HOST/ | grep -q 'Requests' && ok "requests tab" || fail "requests tab" "missing"

# 4. Download
echo "── Download ──"
# Use a fresh track
TASK=$(curl -s -X POST http://$HOST/download -H 'Content-Type: application/json' -d '{"url":"spotify:track:0VjIjW4GlUZAMYd2vXMi3b"}')
TID=$(echo "$TASK" | python3 -c "import json,sys; print(json.load(sys.stdin).get('id',''))" 2>/dev/null)
[ -n "$TID" ] && ok "task created: $TID" || fail "task create" "$TASK"

if [ -n "$TID" ]; then
  for i in $(seq 1 15); do
    sleep 4
    ST=$(curl -s "http://$HOST/download/$TID" | python3 -c "import json,sys; print(json.load(sys.stdin).get('status',''))" 2>/dev/null)
    [ "$ST" = "ready" ] || [ "$ST" = "failed" ] && break
  done
  R=$(curl -s "http://$HOST/download/$TID")
  FS=$(echo "$R" | python3 -c "import json,sys; print(json.load(sys.stdin).get('file_size',0))" 2>/dev/null)
  CV=$(echo "$R" | python3 -c "import json,sys; d=json.load(sys.stdin); print(d.get('cover_url','')[:5])" 2>/dev/null)
  [ "$ST" = "ready" ] && ok "status: ready" || fail "status" "$ST"
  [ "$FS" -gt 500000 ] 2>/dev/null && ok "size: $((FS/1024))KB" || fail "size" "$FS"
  [ "$CV" = "https" ] && ok "cover_url present" || fail "cover_url" "$CV"
fi

# 5. Stats
echo "── Stats ──"
ST=$(curl -s http://$HOST/stats)
echo "$ST" | python3 -c "import json,sys; d=json.load(sys.stdin); exit(0 if d['ready']>0 else 1)" && ok "stats: $(echo $ST | python3 -c 'import json,sys;print(json.load(sys.stdin)["ready"])') ready" || fail "stats" "$ST"

# 6. 409 duplicate
echo "── Edge Cases ──"
DUP=$(curl -s -o /dev/null -w '%{http_code}' -X POST http://$HOST/download -H 'Content-Type: application/json' -d '{"url":"spotify:track:0VjIjW4GlUZAMYd2vXMi3b"}')
[ "$DUP" = "409" ] && ok "409 on duplicate" || fail "duplicate" "$DUP"

NF=$(curl -s -o /dev/null -w '%{http_code}' http://$HOST/download/nope)
[ "$NF" = "404" ] && ok "404 on unknown" || fail "404" "$NF"

# Summary
echo "========================="
if [ "$FAIL" -eq 0 ]; then
  echo "✅ ALL $PASS TESTS PASSED"
  exit 0
else
  echo "❌ $PASS passed, $FAIL FAILED"
  exit 1
fi
