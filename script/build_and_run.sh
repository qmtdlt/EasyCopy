#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-run}"
cd "$ROOT_DIR/apps/desktop"
# Stop only this application's executable, never other Tauri or development processes.
pkill -x EasyCopy >/dev/null 2>&1 || true
npm ci
npm run tauri -- build --debug --bundles app
APP_BUNDLE="$ROOT_DIR/target/debug/bundle/macos/EasyCopy.app"
case "$MODE" in
  run) open -n "$APP_BUNDLE" ;;
  --verify) open -n "$APP_BUNDLE"; sleep 2; pgrep -x EasyCopy >/dev/null ;;
  --acceptance) open -n --env "EASYCOPY_PROFILE_DIR=$ROOT_DIR/.local/desktop-acceptance" "$APP_BUNDLE"; sleep 2; pgrep -x EasyCopy >/dev/null ;;
  --debug) lldb -- "$APP_BUNDLE/Contents/MacOS/EasyCopy" ;;
  --logs|--telemetry) open -n "$APP_BUNDLE"; /usr/bin/log stream --info --style compact --predicate 'process == "EasyCopy"' ;;
  *) echo "usage: $0 [run|--verify|--acceptance|--debug|--logs|--telemetry]" >&2; exit 2 ;;
esac
