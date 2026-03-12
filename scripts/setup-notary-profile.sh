#!/bin/zsh
set -euo pipefail

if [[ "${OSTYPE:-}" != darwin* ]]; then
  echo "This script only works on macOS."
  exit 1
fi

PROFILE_NAME="${1:-stickyterminal-notary}"
APPLE_ID="${APPLE_ID:-${2:-}}"
TEAM_ID="${APPLE_TEAM_ID:-${3:-}}"

if [[ -z "$APPLE_ID" || -z "$TEAM_ID" ]]; then
  echo "Usage:"
  echo "  ./scripts/setup-notary-profile.sh [profile-name] [apple-id] [team-id]"
  echo
  echo "Or set:"
  echo "  APPLE_ID=you@example.com"
  echo "  APPLE_TEAM_ID=YOURTEAMID"
  exit 1
fi

echo "Saving notary credentials in Keychain profile:"
echo "  $PROFILE_NAME"
echo
echo "Apple will prompt for an app-specific password if needed."

xcrun notarytool store-credentials \
  "$PROFILE_NAME" \
  --apple-id "$APPLE_ID" \
  --team-id "$TEAM_ID"

echo
echo "Saved profile:"
echo "  $PROFILE_NAME"
