#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo test --locked -p easycopy-protocol -p easycopy-client-core -p easycopy-relay -p easycopy-integration-tests
(cd apps/desktop && npm ci && npm test && npm run build)
cargo test --locked -p easycopy-desktop --lib
