#!/usr/bin/env bash
# Idempotent deploy helper for music-api on the .200 host.
# Assumes /tmp/music-api, /tmp/music-api.service and
# /tmp/deemix-api.compose.yml have been copied over.
set -euo pipefail

BIN_SRC="${1:-/tmp/music-api}"
ROOT=/opt/music-api
DEEMIX_DIR="$ROOT/deemix-config"
# The ledger DB is small and stays on the root fs; the file store must live on
# the big data volume (a full library is hundreds of GB).
DATA="$ROOT/data"
STORE=/data/public/media/music-api

echo "== 1. directories =="
sudo install -d -o momo -g momo "$ROOT" "$DEEMIX_DIR"
install -d -o momo -g momo "$DATA"
install -d -o momo -g momo "$STORE" "$STORE/incoming" "$STORE/flac" "$STORE/320" "$STORE/128"

echo "== 2. binary =="
install -m755 "$BIN_SRC" "$ROOT/music-api"

echo "== 3. deemix-api config (FLAC + bitrate fallback, flat filenames) =="
python3 - <<'PY'
import json, pathlib
candidates = [
    "/opt/music-stack/deemix-320-config/config.json",
    "/opt/music-stack/deemix-flac-config/config.json",
]
src = next((pathlib.Path(p) for p in candidates if pathlib.Path(p).exists()), None)
cfg = json.loads(src.read_text()) if src else {}
cfg.update({
    "maxBitrate": 9,
    "fallbackBitrate": True,
    "downloadLocation": "/downloads",
    "createArtistFolder": False,
    "createAlbumFolder": False,
    "createPlaylistFolder": False,
    "tracknameTemplate": "%artist% - %title%",
    "overwriteFile": "n",
})
pathlib.Path("/opt/music-api/deemix-config/config.json").write_text(json.dumps(cfg, indent=1))
print("   deemix config seeded")
PY

echo "== 4. deemix login (pick a config with a usable ARL) =="
# Never clobber an ARL that is already in place and valid — the operator may
# have put a Premium ARL here that no source instance has.
if python3 -c "
import json, sys
from pathlib import Path
p = Path('/opt/music-api/deemix-config/login.json')
try:
    a = json.loads(p.read_text()).get('arl')
except Exception:
    a = None
sys.exit(0 if a and len(str(a)) > 100 else 1)
" 2>/dev/null; then
    echo "   existing login.json is valid — kept"
else
    # Some instances exist but were never logged in (`"arl": null`) — e.g.
    # deemix-flac. Pick the first one that actually has a usable token.
    ARL_SRC=""
    for cfg in deemix-320-config deemix-128-config deemix-flac-config deemix-audio-config; do
        candidate="/opt/music-stack/$cfg/login.json"
        if [ -f "$candidate" ] && python3 -c "
import json, sys
a = json.load(open('$candidate')).get('arl')
sys.exit(0 if a and len(str(a)) > 100 else 1)
" 2>/dev/null; then
            ARL_SRC="$candidate"
            break
        fi
    done

    if [ -n "$ARL_SRC" ]; then
        install -m600 -o momo -g momo "$ARL_SRC" "$DEEMIX_DIR/login.json"
        echo "   login.json sourced from $ARL_SRC"
    else
        echo "   !! no config with a usable ARL found — DEEMIX_ARL must be set by hand"
    fi
fi

echo "== 5. env file (fresh token) =="
if [ ! -f "$ROOT/music-api.env" ]; then
    TOKEN="$(openssl rand -hex 32)"
    ARL="$(python3 -c 'import json;print(json.load(open("/opt/music-api/deemix-config/login.json"))["arl"])')"
    umask 077
    cat > "$ROOT/music-api.env" <<EOF
MUSIC_API_TOKEN=$TOKEN
MUSIC_API_BIND=0.0.0.0:8710
DEEMIX_URL=http://127.0.0.1:6599
DEEMIX_ARL=$ARL
DEEMIX_BITRATE=9
DEEMIX_DOWNLOAD_DIR=$STORE/incoming
DATA_DIR=$STORE
DATABASE_URL=sqlite:$ROOT/data/music-api.db
DEEZER_BASE=https://api.deezer.com
FFMPEG=ffmpeg
FFPROBE=ffprobe
WORKER_INTERVAL_SECS=5
DOWNLOAD_TIMEOUT_SECS=900
RUST_LOG=info
EOF
    chown momo:momo "$ROOT/music-api.env"
    chmod 600 "$ROOT/music-api.env"
    echo "   env written"
else
    echo "   env already present (left untouched)"
fi

echo "== 6. deemix-api container =="
sudo docker compose -f /tmp/deemix-api.compose.yml up -d

echo "== 7. systemd =="
sudo install -m644 /tmp/music-api.service /etc/systemd/system/music-api.service
sudo systemctl daemon-reload
sudo systemctl enable --now music-api

sleep 2
echo "== 8. health =="
curl -s --max-time 5 http://127.0.0.1:8710/health || echo "(health check failed)"

echo
echo "== done =="
echo "token: $(grep '^MUSIC_API_TOKEN=' "$ROOT/music-api.env" | cut -d= -f2)"
sudo systemctl --no-pager --lines=5 status music-api | tail -6 || true
sudo docker ps --filter name=deemix-api --format '{{.Names}} {{.Status}} {{.Ports}}'
