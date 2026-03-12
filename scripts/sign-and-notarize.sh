#!/bin/zsh
set -euo pipefail

if [[ "${OSTYPE:-}" != darwin* ]]; then
  echo "This script only works on macOS."
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
DIST_DIR="$ROOT_DIR/dist"
APP_NAME="StickyTerminal"
APP_DIR="$DIST_DIR/$APP_NAME.app"
APP_ZIP="$DIST_DIR/$APP_NAME-notarize.zip"
PACKAGE_NAME="$(sed -n 's/^name = "\(.*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -n 1)"
APP_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -n 1)"
DMG_PATH="$DIST_DIR/$APP_NAME-$APP_VERSION.dmg"
NOTARY_PROFILE="${APPLE_NOTARY_PROFILE:-stickyterminal-notary}"

pick_identity() {
  security find-identity -v -p codesigning 2>/dev/null \
    | sed -n 's/.*"\(Developer ID Application:.*\)"/\1/p' \
    | head -n 1
}

SIGN_IDENTITY="${APPLE_DEVELOPER_IDENTITY:-$(pick_identity)}"

if [[ -z "$PACKAGE_NAME" || -z "$APP_VERSION" ]]; then
  echo "Could not read package name or version from Cargo.toml."
  exit 1
fi

if [[ -z "$SIGN_IDENTITY" ]]; then
  echo "No Developer ID Application certificate was found."
  echo
  echo "Open Apple Developer > Certificates and create/download:"
  echo "  Developer ID Application"
  echo
  echo "Then install it into your login keychain and rerun this script."
  exit 1
fi

echo "Using signing identity:"
echo "  $SIGN_IDENTITY"
echo "Using notary profile:"
echo "  $NOTARY_PROFILE"

if ! xcrun notarytool info --keychain-profile "$NOTARY_PROFILE" >/dev/null 2>&1; then
  echo
  echo "The notary profile '$NOTARY_PROFILE' is not set up yet."
  echo "Run:"
  echo "  ./scripts/setup-notary-profile.sh $NOTARY_PROFILE your-apple-id@example.com YOURTEAMID"
  exit 1
fi

echo
echo "1. Building signed app..."
APPLE_DEVELOPER_IDENTITY="$SIGN_IDENTITY" "$ROOT_DIR/scripts/build-macos-app.sh"

if [[ ! -d "$APP_DIR" ]]; then
  echo "Expected app bundle not found at $APP_DIR"
  exit 1
fi

echo
echo "2. Zipping app for notarization..."
rm -f "$APP_ZIP"
ditto -c -k --keepParent "$APP_DIR" "$APP_ZIP"

echo
echo "3. Notarizing app archive..."
xcrun notarytool submit \
  "$APP_ZIP" \
  --keychain-profile "$NOTARY_PROFILE" \
  --wait

echo
echo "4. Stapling app..."
xcrun stapler staple "$APP_DIR"
xcrun stapler validate "$APP_DIR"

echo
echo "5. Building DMG from stapled app..."
SKIP_APP_BUILD=1 "$ROOT_DIR/scripts/build-dmg.sh"

if [[ ! -f "$DMG_PATH" ]]; then
  echo "Expected DMG not found at $DMG_PATH"
  exit 1
fi

echo
echo "6. Signing DMG..."
codesign --force --sign "$SIGN_IDENTITY" --timestamp "$DMG_PATH"
codesign --verify --verbose=2 "$DMG_PATH"

echo
echo "7. Notarizing DMG..."
xcrun notarytool submit \
  "$DMG_PATH" \
  --keychain-profile "$NOTARY_PROFILE" \
  --wait

echo
echo "8. Stapling DMG..."
xcrun stapler staple "$DMG_PATH"
xcrun stapler validate "$DMG_PATH"

echo
echo "Done."
echo "Signed and notarized files:"
echo "  $APP_DIR"
echo "  $DMG_PATH"
