#!/bin/zsh
set -euo pipefail

if [[ "${OSTYPE:-}" != darwin* ]]; then
  echo "This script only works on macOS."
  exit 1
fi

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
APP_NAME="StickyTerminal"
VOLUME_NAME="StickyTerminal Installer"
ICON_PATH="$ROOT_DIR/assets/macos/StickyTerminal.icns"
DIST_DIR="$ROOT_DIR/dist"
APP_DIR="$DIST_DIR/$APP_NAME.app"
STAGE_DIR="$DIST_DIR/.dmg-stage"
MOUNT_DIR="/Volumes/$VOLUME_NAME"
BACKGROUND_PREVIEW="$DIST_DIR/$APP_NAME-dmg-background.png"
BACKGROUND_DIR="$STAGE_DIR/.background"
BACKGROUND_PATH="$BACKGROUND_DIR/background.png"
RW_DMG="$DIST_DIR/$APP_NAME-temp.dmg"
SWIFT_SCRIPT="$ROOT_DIR/scripts/make-dmg-background.swift"

PACKAGE_NAME="$(sed -n 's/^name = "\(.*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -n 1)"
APP_VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT_DIR/Cargo.toml" | head -n 1)"
FINAL_DMG="$DIST_DIR/$APP_NAME-$APP_VERSION.dmg"

if [[ -z "$PACKAGE_NAME" || -z "$APP_VERSION" ]]; then
  echo "Could not read package name or version from Cargo.toml."
  exit 1
fi

device=""

cleanup() {
  set +e
  if [[ -n "$device" ]]; then
    hdiutil detach "$device" -quiet
  fi
  rm -rf "$STAGE_DIR"
}

trap cleanup EXIT

hide_path() {
  local target="$1"
  [[ -e "$target" ]] || return 0

  chflags hidden "$target" 2>/dev/null || true

  if command -v SetFile >/dev/null 2>&1; then
    SetFile -a V "$target" 2>/dev/null || true
  fi
}

hide_volume_helper_items() {
  local helper_paths=(
    "$MOUNT_DIR/.background"
    "$MOUNT_DIR/.DS_Store"
    "$MOUNT_DIR/.fseventsd"
    "$MOUNT_DIR/.Trashes"
    "$MOUNT_DIR/.VolumeIcon.icns"
  )

  local helper_path
  for helper_path in "${helper_paths[@]}"; do
    hide_path "$helper_path"
  done
}

if [[ "${SKIP_APP_BUILD:-0}" != "1" ]]; then
  echo "Building app bundle..."
  "$ROOT_DIR/scripts/build-macos-app.sh"
else
  echo "Using existing app bundle..."
fi

if [[ ! -d "$APP_DIR" ]]; then
  echo "Expected app bundle not found at $APP_DIR"
  exit 1
fi

echo "Preparing DMG layout..."
rm -f "$FINAL_DMG" "$RW_DMG" "$BACKGROUND_PREVIEW"
rm -rf "$STAGE_DIR"
mkdir -p "$BACKGROUND_DIR"

swift "$SWIFT_SCRIPT" "$BACKGROUND_PREVIEW" "$APP_NAME"
cp "$BACKGROUND_PREVIEW" "$BACKGROUND_PATH"
cp "$ICON_PATH" "$STAGE_DIR/.VolumeIcon.icns"
cp -R "$APP_DIR" "$STAGE_DIR/"
ln -s /Applications "$STAGE_DIR/Applications"

stage_size_kb=$(du -sk "$STAGE_DIR" | awk '{print $1}')
dmg_size_mb=$(( stage_size_kb / 1024 + 32 ))

echo "Creating writable DMG..."
hdiutil create \
  -quiet \
  -volname "$VOLUME_NAME" \
  -srcfolder "$STAGE_DIR" \
  -fs HFS+ \
  -format UDRW \
  -size "${dmg_size_mb}m" \
  "$RW_DMG"

echo "Mounting DMG..."
if [[ -d "$MOUNT_DIR" ]]; then
  hdiutil detach "$MOUNT_DIR" -quiet || true
fi
device="$(
  hdiutil attach \
    -readwrite \
    -noverify \
    -noautoopen \
    -mountpoint "$MOUNT_DIR" \
    "$RW_DMG" | awk '/Apple_HFS/ {print $1; exit}'
)"

if [[ -z "$device" ]]; then
  echo "Could not attach writable DMG."
  exit 1
fi

if command -v SetFile >/dev/null 2>&1; then
  SetFile -a C "$MOUNT_DIR" || true
fi

hide_volume_helper_items

configure_finder_window() {
  osascript <<APPLESCRIPT
tell application "Finder"
  tell disk "$VOLUME_NAME"
    open
    set current view of container window to icon view
    set toolbar visible of container window to false
    set statusbar visible of container window to false
    set bounds of container window to {120, 120, 840, 540}

    set viewOptions to the icon view options of container window
    set arrangement of viewOptions to not arranged
    set icon size of viewOptions to 120
    set text size of viewOptions to 14
    set background picture of viewOptions to file ".background:background.png"

    set position of item "$APP_NAME.app" of container window to {170, 215}
    set position of item "Applications" of container window to {550, 215}

    update without registering applications
    delay 1
    close
    open
    delay 1
  end tell
end tell
APPLESCRIPT
}

echo "Styling Finder window..."
for attempt in {1..10}; do
  if configure_finder_window; then
    break
  fi

  if [[ "$attempt" -eq 10 ]]; then
    echo "Could not configure the DMG Finder window."
    exit 1
  fi

  sleep 1
done

hide_volume_helper_items

sync
sleep 2

echo "Finalizing DMG..."
hdiutil detach -quiet "$device"
device=""
hdiutil convert \
  -quiet \
  "$RW_DMG" \
  -format UDZO \
  -imagekey zlib-level=9 \
  -o "$FINAL_DMG"

rm -f "$RW_DMG"

echo
echo "Built DMG:"
echo "  $FINAL_DMG"
echo
echo "Background preview:"
echo "  $BACKGROUND_PREVIEW"
