#!/usr/bin/env bash
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MODE="${1:-run}"
case "$MODE" in
  run|--verify|--acceptance|--debug|--logs|--telemetry|--build) ;;
  *) echo "usage: $0 [run|--build|--verify|--acceptance|--debug|--logs|--telemetry]" >&2; exit 2 ;;
esac
BUILD_PROFILE="release"
BUILD_FLAGS=()
if [[ "$MODE" == --debug ]]; then
  BUILD_PROFILE="debug"
  BUILD_FLAGS=(--debug)
fi
cd "$ROOT_DIR/apps/desktop"
# Stop only this application's executable, never other Tauri or development processes.
if [[ "$MODE" != --build ]]; then
  pkill -x EasyCopy >/dev/null 2>&1 || true
fi
npm ci
npm run tauri -- build ${BUILD_FLAGS[@]+"${BUILD_FLAGS[@]}"} --bundles app -- --locked
TARGET_DIR="$(cargo metadata --no-deps --format-version 1 --locked | node -e 'let data=""; process.stdin.on("data", c => data += c); process.stdin.on("end", () => process.stdout.write(JSON.parse(data).target_directory));')"
APP_BUNDLE="$TARGET_DIR/$BUILD_PROFILE/bundle/macos/EasyCopy.app"
# Tauri debug bundles can retain only rustc's linker signature, whose identifier
# differs from CFBundleIdentifier. Sign the complete bundle before LaunchServices
# registers it so macOS can associate local-network permission with this app.
SIGNING_IDENTITY="${EASYCOPY_SIGNING_IDENTITY:-}"
if [[ -z "$SIGNING_IDENTITY" ]]; then
  DEVELOPMENT_IDENTITIES="$(security find-identity -v -p codesigning | sed -n 's/^[[:space:]]*[0-9][0-9]*) \([[:xdigit:]]\{40\}\) "Apple Development:.*$/\1/p')"
  if [[ -n "$DEVELOPMENT_IDENTITIES" && "$(printf '%s\n' "$DEVELOPMENT_IDENTITIES" | wc -l | tr -d ' ')" == 1 ]]; then
    SIGNING_IDENTITY="$DEVELOPMENT_IDENTITIES"
  else
    SIGNING_IDENTITY="-"
    echo "未找到唯一的 Apple Development 证书，使用完整临时签名；局域网授权可能需重新确认。可设置 EASYCOPY_SIGNING_IDENTITY 指定证书。" >&2
  fi
fi
codesign --force --sign "$SIGNING_IDENTITY" --identifier com.easycopy.desktop --timestamp=none "$APP_BUNDLE"
codesign --verify --strict --verbose=2 "$APP_BUNDLE"
case "$MODE" in
  --build) echo "已构建并签名：$APP_BUNDLE" ;;
  run) open -n "$APP_BUNDLE" ;;
  --verify) open -n "$APP_BUNDLE"; sleep 2; pgrep -x EasyCopy >/dev/null ;;
  --acceptance) open -n --env "EASYCOPY_PROFILE_DIR=$ROOT_DIR/.local/desktop-acceptance" "$APP_BUNDLE"; sleep 2; pgrep -x EasyCopy >/dev/null ;;
  --debug) lldb -- "$APP_BUNDLE/Contents/MacOS/EasyCopy" ;;
  --logs|--telemetry) open -n "$APP_BUNDLE"; /usr/bin/log stream --info --style compact --predicate 'process == "EasyCopy"' ;;
esac
