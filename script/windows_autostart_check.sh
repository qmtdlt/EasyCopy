#!/usr/bin/env bash
# Compile the real Windows login-startup backend, including its unit tests.
# This is a cross-platform type check, not a Windows execution/installation test.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
mkdir -p .local/windows-autostart-check/src
cat > .local/windows-autostart-check/Cargo.toml <<'MANIFEST'
[workspace]
[package]
name = "windows-autostart-check"
version = "0.0.0"
edition = "2021"
[dependencies]
anyhow = "1"
winreg = "0.56"
MANIFEST
python3 - "$ROOT_DIR" <<'PY'
import json, sys
from pathlib import Path
root = Path(sys.argv[1])
source = root / 'apps/desktop/src-tauri/src/autostart/windows.rs'
(root / '.local/windows-autostart-check/src/lib.rs').write_text(
    'pub const ENTRY_NAME: &str = "com.easycopy.desktop";\n'
    'pub const LOGIN_ARG: &str = "--autostart";\n'
    '#[path = ' + json.dumps(str(source)) + ']\npub mod platform;\n')
PY
cargo check --manifest-path .local/windows-autostart-check/Cargo.toml --target x86_64-pc-windows-gnu --tests
