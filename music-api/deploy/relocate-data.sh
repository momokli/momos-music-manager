#!/usr/bin/env bash
# Move music-api's file store from the 30G root filesystem to the big data
# volume. The ledger DB stays in /opt (small); only files move.
set -euo pipefail

NEW=/data/public/media/music-api
OLD=/opt/music-api/data

echo "== 1. move the store =="
sudo install -d -o momo -g momo "$NEW"
for d in flac 320 128 incoming; do
    if [ -d "$OLD/$d" ]; then
        mv "$OLD/$d" "$NEW/"
    fi
    install -d -o momo -g momo "$NEW/$d"
done
ls -la "$NEW"

echo "== 2. env =="
python3 - <<'PY'
import pathlib
p = pathlib.Path("/opt/music-api/music-api.env")

def setkey(lines, key, val):
    out, seen = [], False
    for l in lines:
        if l.startswith(key + "="):
            out.append(f"{key}={val}")
            seen = True
        else:
            out.append(l)
    if not seen:
        out.append(f"{key}={val}")
    return out

lines = p.read_text().splitlines()
lines = setkey(lines, "DATA_DIR", "/data/public/media/music-api")
lines = setkey(lines, "DEEMIX_DOWNLOAD_DIR", "/data/public/media/music-api/incoming")
lines = setkey(lines, "DATABASE_URL", "/opt/music-api/data/music-api.db")
p.write_text("\n".join(lines) + "\n")
print("   env updated")
PY

echo "== 3. systemd unit path =="
sudo sed -i 's|^ReadWritePaths=.*|ReadWritePaths=/opt/music-api/data /data/public/media/music-api|' /etc/systemd/system/music-api.service
sudo systemctl daemon-reload

echo "== 4. compose mount =="
if [ -f /tmp/deemix-api.compose.yml ]; then
    sudo sed -i 's|/opt/music-api/data/incoming:/downloads|/data/public/media/music-api/incoming:/downloads|' /tmp/deemix-api.compose.yml
fi

echo "== 5. rewrite stored paths in the ledger DB =="
python3 - <<'PY'
import sqlite3
con = sqlite3.connect("/opt/music-api/data/music-api.db")
cur = con.cursor()
cur.execute(
    "UPDATE tracks SET "
    "path_flac = replace(path_flac, '/opt/music-api/data/', '/data/public/media/music-api/'), "
    "path_320  = replace(path_320,  '/opt/music-api/data/', '/data/public/media/music-api/'), "
    "path_128  = replace(path_128,  '/opt/music-api/data/', '/data/public/media/music-api/') "
    "WHERE path_flac LIKE '/opt/music-api/data/%' "
    "   OR path_320  LIKE '/opt/music-api/data/%' "
    "   OR path_128  LIKE '/opt/music-api/data/%'"
)
con.commit()
print("   rows updated:", con.total_changes)
con.close()
PY

echo "== 6. restart =="
sudo systemctl restart music-api
sudo docker compose -f /tmp/deemix-api.compose.yml up -d
sleep 3
curl -s --max-time 5 http://127.0.0.1:8710/health
echo
echo "== done =="
df -h /opt/music-api/data /data/public | tail -2
