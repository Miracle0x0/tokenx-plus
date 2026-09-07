#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

# Keep cargo-zigbuild's target and compiler setup; adapt only the final link.
exec cargo-zigbuild rustc --locked --release -p tokenx --bin tokenx \
  --target x86_64-unknown-linux-gnu "$@" -- \
  -C "linker=${ROOT_DIR}/scripts/zig-linker.py"
