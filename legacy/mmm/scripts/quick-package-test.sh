#!/bin/bash
set -euo pipefail

APP_NAME="Momo's Music Manager"
BUNDLE_NAME="${APP_NAME}.app"
TARGET_DIR="${CARGO_TARGET_DIR:-target}"
VERSION=$(cargo metadata --no-deps --format-version 1 2>/dev/null | python3 -c "import sys,json; print(json.load(sys.stdin)['packages'][0]['version'])")

echo "=== Quick test: ${APP_NAME} v${VERSION} (arm64 only) ==="

# ── 1. Build native arch only ──────────────────────────────────────────────

echo "--- Building release (arm64) ---"
cargo build --release

# Strip debug symbols (reduces binary from ~128MB to ~30MB)
echo "--- Stripping binary ---"
strip "${TARGET_DIR}/release/momos-music-manager"

echo "Binary size:"
ls -lh "${TARGET_DIR}/release/momos-music-manager"

# ── 2. Create .app bundle ─────────────────────────────────────────────────

echo "--- Creating .app bundle ---"
BUNDLE_DIR="${TARGET_DIR}/${BUNDLE_NAME}"
rm -rf "${BUNDLE_DIR}"
mkdir -p "${BUNDLE_DIR}/Contents/MacOS"
mkdir -p "${BUNDLE_DIR}/Contents/Resources"

cp "${TARGET_DIR}/release/momos-music-manager" "${BUNDLE_DIR}/Contents/MacOS/"
cp resources/icon.icns "${BUNDLE_DIR}/Contents/Resources/AppIcon.icns"

cat > "${BUNDLE_DIR}/Contents/Info.plist" << PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleExecutable</key>
	<string>momos-music-manager</string>
	<key>CFBundleIdentifier</key>
	<string>com.momo.music-manager</string>
	<key>CFBundleName</key>
	<string>${APP_NAME}</string>
	<key>CFBundleDisplayName</key>
	<string>${APP_NAME}</string>
	<key>CFBundleVersion</key>
	<string>${VERSION}</string>
	<key>CFBundleShortVersionString</key>
	<string>${VERSION}</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>LSMinimumSystemVersion</key>
	<string>13.0</string>
	<key>LSUIElement</key>
	<true/>
	<key>NSHighResolutionCapable</key>
	<true/>
</dict>
</plist>
PLIST

codesign --force --deep --sign - "${BUNDLE_DIR}" 2>/dev/null || true

# ── 3. Create DMG ─────────────────────────────────────────────────────────

DMG_NAME="Momo's-Music-Manager-v${VERSION}-arm64.dmg"
echo "--- Creating DMG: ${TARGET_DIR}/${DMG_NAME} ---"

STAGING="${TARGET_DIR}/dmg-staging"
rm -rf "${STAGING}"
mkdir -p "${STAGING}"

cp -R "${BUNDLE_DIR}" "${STAGING}/"
# create-dmg adds /Applications via --app-drop-link, don't pre-create it

if command -v create-dmg &> /dev/null; then
    create-dmg \
        --volname "${APP_NAME}" \
        --window-pos 200 120 \
        --window-size 600 400 \
        --icon-size 100 \
        --icon "${BUNDLE_NAME}" 175 190 \
        --hide-extension "${BUNDLE_NAME}" \
        --app-drop-link 425 190 \
        "${TARGET_DIR}/${DMG_NAME}" \
        "${STAGING}/"
else
    echo "create-dmg not found, using hdiutil fallback..."
    hdiutil create -volname "${APP_NAME}" \
        -srcfolder "${STAGING}" \
        -ov -format UDZO \
        "${TARGET_DIR}/${DMG_NAME}"
fi

rm -rf "${STAGING}"

echo ""
echo "=== Done ==="
ls -lh "${TARGET_DIR}/${DMG_NAME}"
echo ""
echo "Ready to test: open ${TARGET_DIR}/${DMG_NAME}"
echo "Or launch the app directly: open ${BUNDLE_DIR}"
