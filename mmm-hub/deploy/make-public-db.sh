#!/usr/bin/env bash
# Rebuild the sanitized read-only copy and restart Datasette.
# Driven by make-public-db.timer (every 10 min) on the music-catalog host.
set -euo pipefail
TMP=/home/momo/mmm-hub/hub-public.db.tmp
DST=/home/momo/mmm-hub/hub-public.db
rm -f "$TMP" "$TMP-wal" "$TMP-shm"
sqlite3 "$TMP" < /home/momo/mmm-hub/make-public.sql
mv -f "$TMP" "$DST"
sudo systemctl restart datasette
