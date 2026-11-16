#!/usr/bin/env bash
# Idempotent deploy helper for music-api on the .200 host.
# Assumes /tmp/music-api and /tmp/music-api.service have been copied over.
set -euo pipefail

BIN_SRC="${1:-/tmp/music-api}"
ROOT=/opt/music-api
# The ledger DB is small and stays on the root fs; the file store must live on
# the big data volume (a full library is hundreds of GB).
DATA="$ROOT/data"
STORE=/data/public/media/music-api

echo "== 1. directories =="
sudo install -d -o momo -g momo "$ROOT"
install -d -o momo -g momo "$DATA"
install -d -o momo -g momo "$STORE" "$STORE/flac" "$STORE/320" "$STORE/128"

echo "== 2. binary =="
install -m755 "$BIN_SRC" "$ROOT/music-api"

echo "== 3. env file (fresh token) =="
if [ ! -f "$ROOT/music-api.env" ]; then
    TOKEN="$(openssl rand -hex 32)"
    umask 077
    cat > "$ROOT/music-api.env" <<EOF
MUSIC_API_TOKEN=$TOKEN
MUSIC_API_BIND=0.0.0.0:8710
# Deezer ARL used for native downloads. REQUIRED — never commit it.
DEEZER_ARL=
# Download quality: flac | mp3-320 | mp3-128 (tier fallback steps down).
DOWNLOAD_QUALITY=flac
DATA_DIR=$STORE
DATABASE_URL=sqlite:$ROOT/data/music-api.db
DEEZER_BASE=https://api.deezer.com
FFMPEG=ffmpeg
FFPROBE=ffprobe
# BPM/key analysis (Essentia on the hub host); unset/blank disables it:
# ANALYZER_URL=http://127.0.0.1:8711
# Vault resolver precedence (first analyzer with a value wins, per field):
# ANALYSIS_PRECEDENCE=traktor,essentia,reccobeats
WORKER_INTERVAL_SECS=5
DOWNLOAD_TIMEOUT_SECS=900
RUST_LOG=info
EOF
    chown momo:momo "$ROOT/music-api.env"
    chmod 600 "$ROOT/music-api.env"
    echo "   env written"
    echo "   !! DEEZER_ARL is empty — set it in $ROOT/music-api.env before you rely on downloads"
else
    echo "   env already present (left untouched)"
fi

echo "== 4. systemd =="
sudo install -m644 /tmp/music-api.service /etc/systemd/system/music-api.service
sudo systemctl daemon-reload
sudo systemctl enable --now music-api

sleep 2
echo "== 5. health =="
curl -s --max-time 5 http://127.0.0.1:8710/health || echo "(health check failed)"

echo
echo "== done =="
echo "token: $(grep '^MUSIC_API_TOKEN=' "$ROOT/music-api.env" | cut -d= -f2)"
sudo systemctl --no-pager --lines=5 status music-api | tail -6 || true
