#!/usr/bin/env bash
# Type-check the real Windows native module on macOS without a MinGW C compiler.
# This does not build or run the full Windows application.
set -euo pipefail
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"
rustup target add x86_64-pc-windows-gnu
mkdir -p .local/windows-clipboard-check/src
cat > .local/windows-clipboard-check/Cargo.toml <<'MANIFEST'
[workspace]
[package]
name = "windows-clipboard-check"
version = "0.0.0"
edition = "2021"
[dependencies]
anyhow = "1"
image = { version = "0.25", default-features = false, features = ["png", "tiff"] }
MANIFEST
python3 - "$ROOT_DIR" <<'PY'
import sys, json
from pathlib import Path
root=Path(sys.argv[1])
(root/'.local/windows-clipboard-check/src/lib.rs').write_text('#[path = '+json.dumps(str(root/'crates/client-core/src/clipboard/mod.rs'))+']\npub mod clipboard;\n')
PY
cargo check --manifest-path .local/windows-clipboard-check/Cargo.toml --target x86_64-pc-windows-gnu
