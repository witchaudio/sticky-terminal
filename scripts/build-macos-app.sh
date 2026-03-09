#!/bin/zsh
set -euo pipefail

if [[ "${OSTYPE:-}" != darwin* ]]; then
  echo "This script only works on macOS."
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="StickyTerminal"
APP_BUNDLE_ID="com.witchaudio.stickyterminal"
ICON_PATH="$ROOT_DIR/assets/macos/StickyTerminal.icns"
DIST_DIR="$ROOT_DIR/dist"
APP_DIR="$DIST_DIR/$APP_NAME.app"
CONTENTS_DIR="$APP_DIR/Contents"
MACOS_DIR="$CONTENTS_DIR/MacOS"
RESOURCES_DIR="$CONTENTS_DIR/Resources"

PACKAGE_NAME="$(sed -n 's/^name = "\(.*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -n 1)"
APP_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -n 1)"
BINARY_PATH="$ROOT_DIR/target/release/$PACKAGE_NAME"
EXECUTABLE_PATH="$MACOS_DIR/$APP_NAME"

if [[ -z "$PACKAGE_NAME" || -z "$APP_VERSION" ]]; then
  echo "Could not read package name or version from Cargo.toml."
  exit 1
fi

echo "Building release binary..."
cargo build --release --manifest-path "$ROOT_DIR/Cargo.toml"

if [[ ! -f "$BINARY_PATH" ]]; then
  echo "Expected binary not found at $BINARY_PATH"
  exit 1
fi

echo "Creating app bundle..."
rm -rf "$APP_DIR"
mkdir -p "$MACOS_DIR" "$RESOURCES_DIR"

cp "$BINARY_PATH" "$EXECUTABLE_PATH"
chmod +x "$EXECUTABLE_PATH"
cp "$ICON_PATH" "$RESOURCES_DIR/$APP_NAME.icns"

cat > "$CONTENTS_DIR/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>$APP_NAME</string>
  <key>CFBundleExecutable</key>
  <string>$APP_NAME</string>
  <key>CFBundleIconFile</key>
  <string>$APP_NAME</string>
  <key>CFBundleIdentifier</key>
  <string>$APP_BUNDLE_ID</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>$APP_NAME</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$APP_VERSION</string>
  <key>CFBundleVersion</key>
  <string>$APP_VERSION</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.productivity</string>
  <key>LSMinimumSystemVersion</key>
  <string>12.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
PLIST

if command -v codesign >/dev/null 2>&1; then
  echo "Applying ad-hoc code signature..."
  codesign --force --deep --sign - "$APP_DIR" >/dev/null
fi

echo
echo "Built app bundle:"
echo "  $APP_DIR"
echo
echo "You can now move $APP_NAME.app into /Applications."
