#!/usr/bin/env bash
# deploy.sh — Einmal-Setup des Download-Service auf dem Hetzner-Server.
#
# Credentials werden AUSSERHALB des Repos gespeichert:
#   ~/.config/download-service/config.toml
#
# Usage auf dem Server:
#   cd /srv/momos-music-manager/download-service
#   bash deploy.sh
#
# Vor dem ersten Run:
#   mkdir -p ~/.config/download-service
#   cp config.example.toml ~/.config/download-service/config.toml
#   vim ~/.config/download-service/config.toml  # credentials eintragen
#
# Danach:
#   sudo systemctl start download-service
#   sudo systemctl start dufs-downloads
#   curl http://localhost:8000/health | jq

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SERVICE_NAME="download-service"
VENV_DIR="$SCRIPT_DIR/.venv"
CONFIG_DIR="$HOME/.config/download-service"
CONFIG_FILE="$CONFIG_DIR/config.toml"

echo "=== Download Service Deployment ==="
echo ""

# ── 1. Python virtualenv ───────────────────────────────────────────
echo "[1/5] Creating Python virtualenv..."
python3 -m venv "$VENV_DIR"
source "$VENV_DIR/bin/activate"

echo "[2/5] Installing Python dependencies..."
pip install -q --upgrade pip
pip install -q -r "$SCRIPT_DIR/requirements.txt"

echo "[3/5] Installing deemix and spotdl..."
pip install -q deemix spotdl

echo -n "  deemix: "
python3 -m deemix --help >/dev/null 2>&1 && echo "OK" || echo "NOT FOUND (will fail at runtime)"
echo -n "  spotdl: "
spotdl --help >/dev/null 2>&1 && echo "OK" || echo "NOT FOUND (will fail at runtime)"

# ── 2. Create output directory ─────────────────────────────────────
echo "[4/5] Creating output directories..."
sudo mkdir -p /opt/download-service/downloads/tracks
sudo chown -R "$(whoami):$(whoami)" /opt/download-service
chmod 755 /opt/download-service/downloads/tracks

# ── 3. Config check ────────────────────────────────────────────────
echo "[5/5] Checking config..."
if [ ! -f "$CONFIG_FILE" ]; then
    echo ""
    echo "  CONFIG NOT FOUND: $CONFIG_FILE"
    echo ""
    echo "  Run these commands first:"
    echo "    mkdir -p $CONFIG_DIR"
    echo "    cp $SCRIPT_DIR/config.example.toml $CONFIG_FILE"
    echo "    vim $CONFIG_FILE"
    echo ""
    echo "  Fill in:"
    echo "    deemix.arl            = \"...\"   (from Deezer browser cookies)"
    echo "    spotify.client_id     = \"...\"   (from Spotify Developer Dashboard)"
    echo "    spotify.client_secret = \"...\""
    echo ""
    echo "  Then re-run: bash deploy.sh"
    echo ""
    exit 1
fi
echo "  $CONFIG_FILE OK"

# ── 4. systemd services ─────────────────────────────────────────────

# Download Service
sudo tee /etc/systemd/system/download-service.service > /dev/null << UNITFILE
[Unit]
Description=Download Service (deemix + spotDL pipeline)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$(whoami)
WorkingDirectory=$SCRIPT_DIR
Environment="DOWNLOAD_SERVICE_CONFIG=$CONFIG_FILE"
ExecStart=$VENV_DIR/bin/uvicorn main:app --host 0.0.0.0 --port 8000
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
UNITFILE

# dufs — browse downloads via web
if command -v dufs &>/dev/null; then
    sudo tee /etc/systemd/system/dufs-downloads.service > /dev/null << UNITFILE
[Unit]
Description=dufs file browser for download output
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$(whoami)
ExecStart=dufs /opt/download-service/downloads/tracks --port 8321 --allow-all
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
UNITFILE
    echo "  dufs service installed"
else
    echo "  dufs not installed. Get it: cargo install dufs"
fi

sudo systemctl daemon-reload
sudo systemctl enable download-service
sudo systemctl enable dufs-downloads 2>/dev/null || true

echo ""
echo "=== Deployment complete ==="
echo ""
echo "Config:  $CONFIG_FILE"
echo "Output:  /opt/download-service/downloads/tracks"
echo ""
echo "Start:"
echo "  sudo systemctl start download-service"
echo "  sudo systemctl start dufs-downloads"
echo ""
echo "Verify:"
echo "  curl http://localhost:8000/health | jq"
echo "  curl http://localhost:8321                # dufs file browser"
echo ""
echo "Test playlist download:"
echo "  curl -s -X POST http://localhost:8000/download/playlist \\"
echo "    -H 'Content-Type: application/json' \\"
echo "    -d '{\"url\": \"https://open.spotify.com/playlist/...\"}' | jq"
echo ""
echo "Logs:"
echo "  sudo journalctl -u download-service -f"
