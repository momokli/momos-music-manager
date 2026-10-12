#!/usr/bin/env bash
set -euo pipefail

# lab-stage.sh — Pull files from LAN for Traktor analysis, then clean up
#
# Usage: ./scripts/lab-stage.sh <tag> [format] [count]
#
# Arguments:
#   tag     Tag name to pull files from (default: laboratory)
#   format  File type: flac, stem.m4a (default: flac)
#   count   Number of files to pull (default: 50)
#
# Flow:
#   1. Query LAN API for files needing BPM/key in the given tag
#   2. rsync those files from LAN to MacBook Music directory
#   3. Prompt user to open Traktor, analyze, close
#   4. Clean up downloaded files from MacBook

TAG="${1:-laboratory}"
FORMAT="${2:-flac}"
COUNT="${3:-50}"
LAN="lan"
LAN_MUSIC="/home/momo/share/${FORMAT}s"

# Map file type to local Music subdirectory
case "$FORMAT" in
    flac)     LOCAL_MUSIC="$HOME/Music/flacs" ;;
    stem.m4a) LOCAL_MUSIC="$HOME/Music/stems" ;;
    *)        echo "Unsupported format: $FORMAT. Use 'flac' or 'stem.m4a'."; exit 1 ;;
esac

# Resolve tag name to tag ID via LAN API
echo "→ Resolving tag '${TAG}'..."
TAG_INFO=$(ssh "$LAN" "curl -sf 'http://localhost:3000/api/tags?search=${TAG}&limit=1' 2>/dev/null")
TAG_ID=$(echo "$TAG_INFO" | python3 -c "
import sys, json
data = json.load(sys.stdin)['data']
matches = [t for t in data if t['name'].lower() == '$TAG'.lower()]
print(matches[0]['id'] if matches else '')
" 2>/dev/null)

if [ -z "$TAG_ID" ]; then
    echo "✗ Tag '${TAG}' not found on LAN server."
    echo "  Create it first at http://music.klimk.es:3000/#tags"
    exit 1
fi

# Fetch needs-analysis list
echo "→ Querying files needing analysis in '${TAG}' (format=${FORMAT}, limit=${COUNT})..."
API_URL="http://localhost:3000/api/tags/${TAG_ID}/needs-analysis?format=${FORMAT}&limit=${COUNT}"
RESP=$(ssh "$LAN" "curl -sf '${API_URL}' 2>/dev/null")
RC=$?

if [ "$RC" -ne 0 ]; then
    echo "✗ Failed to query LAN API. Is the server running?"
    echo "  ssh $LAN 'curl -sf http://localhost:3000/api/health'"
    exit 1
fi

# Parse response
FILE_COUNT=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['fileCount'])")
NEEDS_BPM=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['needsBpm'])")
NEEDS_KEY=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['needsKey'])")

if [ "$FILE_COUNT" -eq 0 ]; then
    echo "✓ All ${COUNT} checked files in '${TAG}' are already analyzed."
    echo "  Run with a higher count or check a different tag."
    exit 0
fi

echo "→ Found ${FILE_COUNT} files needing analysis:"
echo "    Needs BPM:  ${NEEDS_BPM}"
echo "    Needs Key:  ${NEEDS_KEY}"
echo ""

# Create local directory if it doesn't exist
mkdir -p "$LOCAL_MUSIC"

# Extract filenames and rsync them
echo "→ Pulling files from ${LAN}:${LAN_MUSIC}/..."
echo "$RESP" | python3 -c "
import sys, json
data = json.load(sys.stdin)['data']
for f in data['files']:
    print(f['filePath'].split('/')[-1])
" | while read -r filename; do
    [ -z "$filename" ] && continue
    echo "   Pulling: ${filename}"
    rsync -az "${LAN}:${LAN_MUSIC}/${filename}" "${LOCAL_MUSIC}/" 2>&1 | tail -1
done

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo "  ✓ ${FILE_COUNT} file(s) staged for Traktor analysis"
echo "  Destination: ${LOCAL_MUSIC}/"
echo ""
echo "  1. OPEN TRAKTOR on your MacBook"
echo "     It will detect new files and analyze them."
echo ""
echo "  2. Wait for waveform/BPM/key analysis to complete."
echo "     (Check the browser column in Traktor for green bars)"
echo ""
echo "  3. CLOSE TRAKTOR when done."
echo "     collection.nml is saved on close."
echo ""
echo "  The NML will auto-sync to LAN within 15 min."
echo ""
echo "  Press ENTER after closing Traktor to clean up..."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
read -r

echo "→ Cleaning up downloaded files..."
echo "$RESP" | python3 -c "
import sys, json
data = json.load(sys.stdin)['data']
for f in data['files']:
    print(f['filePath'].split('/')[-1])
" | while read -r filename; do
    [ -z "$filename" ] && continue
    rm -f "${LOCAL_MUSIC}/${filename}"
done

echo "✓ Cleanup complete."
echo ""
echo "  Metadata will appear on the LAN after the next NML sync cycle"
echo "  (maintainer runs every hour). Check status at:"
echo "  http://music.klimk.es:3000/#files"
