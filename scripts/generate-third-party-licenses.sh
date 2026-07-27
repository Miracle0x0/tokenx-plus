#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

command -v cargo-about >/dev/null 2>&1 || {
  echo "cargo-about is required: cargo install cargo-about --locked --version 0.9.1 --features cli" >&2
  exit 1
}

cargo about generate \
  --workspace \
  --all-features \
  --locked \
  scripts/third-party-licenses.hbs \
  -o THIRD_PARTY_LICENSES.html

# Normalize line endings and trailing whitespace from upstream license files.
perl -pi -e 's/[ \t\r]+$//' THIRD_PARTY_LICENSES.html

package_dirs=(
  packages/tokenx
  packages/tokenx-darwin-arm64
  packages/tokenx-linux-x64-gnu
  packages/tokenx-win32-x64-msvc
)
for package_dir in "${package_dirs[@]}"; do
  cp LICENSE NOTICE "${package_dir}/"
done

echo "Generated canonical third-party license notice and synchronized npm package legal files"
