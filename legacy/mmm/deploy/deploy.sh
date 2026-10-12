#!/usr/bin/env bash
set -euo pipefail

# ───────────────────────────────────────────────────────────────────
# deploy.sh — Build and deploy Momo's Music Manager to lan server
# ───────────────────────────────────────────────────────────────────

SERVER="lan"
REMOTE_DIR="/home/momo/momos-music-manager"
SERVICE_NAME="momos-music-manager"
BINARY="$REMOTE_DIR/target/release/momos-music-manager"
BRANCH="$(git branch --show-current)"

echo "╔══════════════════════════════════════════════════════════════╗"
echo "║  Momo's Music Manager — Deploy to $SERVER                   ║"
echo "║  Branch: $BRANCH                                          ║"
echo "╚══════════════════════════════════════════════════════════════╝"
echo ""

# ── Step 1: Push current branch to server ─────────────────────────
echo "→ Step 1/5: Syncing code to $SERVER..."
rsync -avz --delete \
    --exclude='target/' \
    --exclude='.git/' \
    --exclude='node_modules/' \
    --exclude='frontend/node_modules/' \
    --exclude='*.db' \
    --exclude='*.db-*' \
    --exclude='data/' \
    --exclude='deploy/' \
    ./ "$SERVER:$REMOTE_DIR/"

# Also copy the deploy directory (for the service file)
rsync -avz deploy/ "$SERVER:$REMOTE_DIR/deploy/"

# ── Step 2: Build release binary on server ─────────────────────────
echo ""
echo "→ Step 2/5: Building release binary on $SERVER..."
ssh "$SERVER" "cd $REMOTE_DIR && \
    export PATH=\$HOME/.cargo/bin:\$PATH && \
    cargo build --release 2>&1 | tail -20"

# ── Step 3: Create data directory ──────────────────────────────────
echo ""
echo "→ Step 3/5: Creating data directory..."
ssh "$SERVER" "mkdir -p $REMOTE_DIR/data"

# ── Step 4: Install systemd service ────────────────────────────────
echo ""
echo "→ Step 4/5: Installing systemd service..."
ssh "$SERVER" "sudo cp $REMOTE_DIR/deploy/momos-music-manager.service /etc/systemd/system/ && \
    sudo systemctl daemon-reload"

# ── Step 5: Restart service ────────────────────────────────────────
echo ""
echo "→ Step 5/5: Restarting service..."
ssh "$SERVER" "sudo systemctl enable $SERVICE_NAME && \
    sudo systemctl restart $SERVICE_NAME"

# ── Verify ─────────────────────────────────────────────────────────
echo ""
echo "→ Waiting for health check..."
sleep 5

if ssh "$SERVER" "curl -sf http://localhost:3000/api/health" > /dev/null 2>&1; then
    echo ""
    echo "╔══════════════════════════════════════════════════════════════╗"
    echo "║  ✓ DEPLOY SUCCESSFUL                                       ║"
    echo "║                                                             ║"
    echo "║  Service:  systemctl status $SERVICE_NAME                   ║"
    echo "║  Logs:     journalctl -u $SERVICE_NAME -f                   ║"
    echo "║  Web UI:   http://$SERVER:3000                              ║"
    echo "╚══════════════════════════════════════════════════════════════╝"
else
    echo ""
    echo "╔══════════════════════════════════════════════════════════════╗"
    echo "║  ✗ DEPLOY FAILED — check logs:                             ║"
    echo "║     ssh $SERVER journalctl -u $SERVICE_NAME -n 50           ║"
    echo "╚══════════════════════════════════════════════════════════════╝"
    exit 1
fi
