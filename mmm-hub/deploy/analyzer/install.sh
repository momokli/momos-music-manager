#!/usr/bin/env bash
#
# Install / update the mmm-analyzer service on the server host.
#
# Usage (on the target host, e.g. via:  ssh music-catalog 'sudo bash -s' < install.sh
# or after copying this directory to the host:  sudo ./install.sh):
#
#   sudo ./install.sh
#
# It is idempotent: re-running it updates the code, ensures the Essentia
# Discogs-EffNet model files are present, and restarts the service.
#
# The service runs as the (non-root) user 'momo' from:
#   /home/momo/mmm-analyzer/
# and listens on 127.0.0.1:8711.

set -euo pipefail

SERVICE_NAME="mmm-analyzer"
INSTALL_DIR="/home/momo/mmm-analyzer"
RUN_USER="momo"
RUN_GROUP="momo"
MODELS_DIR="${INSTALL_DIR}/models"
MODELS_BASE_URL="https://essentia.upf.edu/models"

SRC_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ "${EUID}" -ne 0 ]]; then
    echo "This script needs root (it installs a systemd unit). Re-run with sudo." >&2
    exit 1
fi

echo "==> Installing ${SERVICE_NAME} from ${SRC_DIR} into ${INSTALL_DIR}"
install -d -o "${RUN_USER}" -g "${RUN_GROUP}" "${INSTALL_DIR}"

# --- code -------------------------------------------------------------------
install -o "${RUN_USER}" -g "${RUN_GROUP}" -m 0644 \
    "${SRC_DIR}/server.py" "${INSTALL_DIR}/server.py"
install -o "${RUN_USER}" -g "${RUN_GROUP}" -m 0644 \
    "${SRC_DIR}/requirements.txt" "${INSTALL_DIR}/requirements.txt"
install -o "${RUN_USER}" -g "${RUN_GROUP}" -m 0644 \
    "${SRC_DIR}/README.md" "${INSTALL_DIR}/README.md"

# --- Essentia models (Discogs-EffNet embeddings + genre classifier) ---------
# Weights (.pb) are large; only fetch when missing or the size doesn't match,
# and verify the size after download so a truncated file never goes live.
install -d -o "${RUN_USER}" -g "${RUN_GROUP}" "${MODELS_DIR}"

fetch_model() {
    local rel="$1" name="$2" expect="$3"
    local dest="${MODELS_DIR}/${name}"
    if [[ -f "${dest}" ]] && [[ "$(stat -c %s "${dest}" 2>/dev/null)" == "${expect}" ]]; then
        echo "    present: ${name}"
        return 0
    fi
    echo "    fetching ${name}"
    local tmp
    tmp="$(mktemp)"
    if ! curl -fsSL --retry 3 --retry-delay 2 --max-time 900 \
        -o "${tmp}" "${MODELS_BASE_URL}/${rel}"; then
        echo "    ERROR: download failed for ${name}" >&2
        rm -f "${tmp}"
        return 1
    fi
    local got
    got="$(stat -c %s "${tmp}")"
    if [[ "${got}" != "${expect}" ]]; then
        echo "    ERROR: ${name} size ${got} != expected ${expect} (truncated download?)" >&2
        rm -f "${tmp}"
        return 1
    fi
    install -o "${RUN_USER}" -g "${RUN_GROUP}" -m 0644 "${tmp}" "${dest}"
    rm -f "${tmp}"
}

echo "==> Ensuring Essentia models in ${MODELS_DIR}"
fetch_model \
    "feature-extractors/discogs-effnet/discogs-effnet-bs64-1.pb" \
    "discogs-effnet-bs64-1.pb" "18366619"
fetch_model \
    "feature-extractors/discogs-effnet/discogs-effnet-bs64-1.json" \
    "discogs-effnet-bs64-1.json" "14983"
fetch_model \
    "classification-heads/genre_discogs400/genre_discogs400-discogs-effnet-1.pb" \
    "genre_discogs400-discogs-effnet-1.pb" "2057977"
fetch_model \
    "classification-heads/genre_discogs400/genre_discogs400-discogs-effnet-1.json" \
    "genre_discogs400-discogs-effnet-1.json" "14951"

# --- python venv ------------------------------------------------------------
if [[ ! -x "${INSTALL_DIR}/venv/bin/python" ]]; then
    echo "==> Creating virtualenv"
    sudo -u "${RUN_USER}" python3 -m venv "${INSTALL_DIR}/venv"
fi

echo "==> Installing Python dependencies (this may take a minute)"
sudo -u "${RUN_USER}" "${INSTALL_DIR}/venv/bin/pip" install --quiet --upgrade pip
# The TensorFlow-less `essentia` wheel shares the `essentia` module directory with
# `essentia-tensorflow` and would clobber/conflict with it. Remove it if present
# (a no-op when only essentia-tensorflow is installed).
sudo -u "${RUN_USER}" "${INSTALL_DIR}/venv/bin/pip" uninstall -y essentia >/dev/null 2>&1 || true
# --pre is required for the Essentia pre-release wheel (essentia-tensorflow).
sudo -u "${RUN_USER}" "${INSTALL_DIR}/venv/bin/pip" install --quiet --pre \
    -r "${INSTALL_DIR}/requirements.txt"

# --- smoke checks -----------------------------------------------------------
echo "==> Smoke import check"
sudo -u "${RUN_USER}" "${INSTALL_DIR}/venv/bin/python" - <<'PY'
import essentia, essentia.standard as es  # noqa: F401
print("essentia", essentia.__version__, "imported OK")
assert hasattr(es, "TensorflowPredictEffnetDiscogs"), "essentia wheel lacks TensorFlow support"
PY

echo "==> Smoke model-load check (embeddings)"
sudo -u "${RUN_USER}" "${INSTALL_DIR}/venv/bin/python" -c "
import essentia.standard as es
es.TensorflowPredictEffnetDiscogs(graphFilename='${MODELS_DIR}/discogs-effnet-bs64-1.pb', output='PartitionedCall:1')
print('embedding model loaded OK')
"

# --- systemd unit -----------------------------------------------------------
echo "==> Installing systemd unit"
install -o root -g root -m 0644 \
    "${SRC_DIR}/${SERVICE_NAME}.service" "/etc/systemd/system/${SERVICE_NAME}.service"
systemctl daemon-reload
systemctl enable "${SERVICE_NAME}"
systemctl restart "${SERVICE_NAME}"

sleep 2
echo "==> Status"
systemctl --no-pager --full status "${SERVICE_NAME}" | head -n 12 || true
echo
echo "==> Health check"
curl -fsS "http://127.0.0.1:8711/health" && echo
echo
echo "Done. Restart with:  sudo systemctl restart ${SERVICE_NAME}"
echo "Logs with:           sudo journalctl -u ${SERVICE_NAME} -f"
