#!/bin/bash
# Package Momo's Music Manager for Linux as a portable tar.gz + SHA256SUMS.
#
# Usage:
#   ./scripts/package-linux.sh [<target-triple>]
#
# <target-triple> defaults to the host triple (x86_64-unknown-linux-gnu).
# For cross builds, e.g.:
#   ./scripts/package-linux.sh aarch64-unknown-linux-gnu
#
# Requires: cargo, python3 (for reading the version), tar, sha256sum.
# The binary is built with `--release` for the given target.
# Output (in target/<triple>/release/ unless CARGO_TARGET_DIR is set):
#   momos-music-manager-<version>-<os>-<arch>.tar.gz
#   momos-music-manager-<version>-<os>-<arch>.tar.gz.sha256
set -euo pipefail

cd "$(dirname "$0")/.."

TARGET_TRIPLE="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
HOST_TRIPLE="$(rustc -vV | sed -n 's/^host: //p')"
APP_NAME="momos-music-manager"
VERSION=$(cargo metadata --no-deps --format-version 1 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['packages'][0]['version'])")
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
if [ "${TARGET_TRIPLE}" = "${HOST_TRIPLE}" ]; then
  RELEASE_DIR="${TARGET_DIR}/release"
else
  RELEASE_DIR="${TARGET_DIR}/${TARGET_TRIPLE}/release"
fi
BIN_PATH="${RELEASE_DIR}/${APP_NAME}"

# Map a rust target triple to <os>-<arch> for the artifact name.
# Examples: x86_64-unknown-linux-gnu  -> linux-x64
#           aarch64-unknown-linux-gnu -> linux-arm64
case "${TARGET_TRIPLE}" in
  x86_64-unknown-linux-gnu)  OS_ARCH="linux-x64" ;;
  aarch64-unknown-linux-gnu) OS_ARCH="linux-arm64" ;;
  *)
    echo "Unsupported Linux target triple: ${TARGET_TRIPLE}" >&2
    exit 1
    ;;
esac

ARTIFACT_BASE="${APP_NAME}-${VERSION}-${OS_ARCH}"
STAGING_DIR="${TARGET_DIR}/pkg-linux-staging-${OS_ARCH}"
ARCHIVE="${TARGET_DIR}/${ARTIFACT_BASE}.tar.gz"

echo "=== Packaging ${APP_NAME} v${VERSION} for ${OS_ARCH} (${TARGET_TRIPLE}) ==="

echo "--- Building release binary ---"
if [ "${TARGET_TRIPLE}" = "${HOST_TRIPLE}" ]; then
  cargo build --release
else
  cargo build --release --target "${TARGET_TRIPLE}"
fi

if [ ! -x "${BIN_PATH}" ]; then
  echo "ERROR: binary not found at ${BIN_PATH}" >&2
  exit 1
fi

echo "--- Staging files ---"
rm -rf "${STAGING_DIR}"
mkdir -p "${STAGING_DIR}"

cp "${BIN_PATH}" "${STAGING_DIR}/${APP_NAME}"
cp README.md "${STAGING_DIR}/README.md"
# Ship the systemd unit so users can install the headless server mode directly.
mkdir -p "${STAGING_DIR}/deploy"
cp deploy/momos-music-manager.service "${STAGING_DIR}/deploy/"

cat > "${STAGING_DIR}/VERSION" <<EOF
${VERSION}
EOF

# Tiny package-level README so users know what they got and how to run it.
cat > "${STAGING_DIR}/INSTALL.txt" <<EOF
Momo's Music Manager v${VERSION} (${OS_ARCH})

Run the server (headless):
  ./${APP_NAME} serve --host 0.0.0.0 --port 3000 --no-browser

Then open http://<host>:3000 in a browser.

Optional systemd service (server mode):
  sudo install -m 0644 deploy/momos-music-manager.service /etc/systemd/system/
  # adjust User/Environment in the unit or via:
  #   sudo systemctl edit momos-music-manager
  sudo systemctl daemon-reload
  sudo systemctl enable --now momos-music-manager

The binary is self-contained (SQLite is compiled in, TLS via rustls);
no system libraries beyond the base OS are required.
EOF

echo "--- Creating archive ---"
rm -f "${ARCHIVE}"
tar -czf "${ARCHIVE}" -C "${STAGING_DIR}" .

echo "--- Creating checksum ---"
(cd "$(dirname "${ARCHIVE}")" && sha256sum "$(basename "${ARCHIVE}")" > "${ARTIFACT_BASE}.tar.gz.sha256")

rm -rf "${STAGING_DIR}"

echo ""
echo "=== Done ==="
echo "Archive:  ${ARCHIVE}"
echo "Checksum: ${TARGET_DIR}/${ARTIFACT_BASE}.tar.gz.sha256"
ls -lh "${ARCHIVE}"
