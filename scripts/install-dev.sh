#!/usr/bin/env bash
# Backward-compatible developer shortcut: Creator Edition from this checkout.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
exec "$SCRIPT_DIR/install.sh" --edition creator "$@"
