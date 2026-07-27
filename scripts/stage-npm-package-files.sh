#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

PACKAGE_DIR="${1:-}"

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

[[ -n "${PACKAGE_DIR}" ]] || fail "Usage: $0 <package-dir>"
[[ -f "${PACKAGE_DIR}/package.json" ]] || fail "Missing package manifest: ${PACKAGE_DIR}/package.json"
[[ -f LICENSE ]] || fail "Canonical LICENSE is missing"
[[ -f NOTICE ]] || fail "Canonical NOTICE is missing"

package_name="$(python3 - "${PACKAGE_DIR}/package.json" <<'PY'
import json
import sys

path = sys.argv[1]
name = json.load(open(path, encoding="utf-8")).get("name")
if not isinstance(name, str) or not name:
    raise SystemExit(f"{path} missing string field name")
print(name)
PY
)"

include_third_party=false
case "${package_name}" in
  @juya-ai/tokenx)
    ;;
  @juya-ai/tokenx-darwin-arm64 | @juya-ai/tokenx-linux-x64-gnu | @juya-ai/tokenx-win32-x64-msvc)
    include_third_party=true
    ;;
  *)
    fail "Unsupported npm release package: ${package_name}"
    ;;
esac

cp LICENSE NOTICE "${PACKAGE_DIR}/"
if [[ "${include_third_party}" == "true" ]]; then
  [[ -f THIRD_PARTY_LICENSES.html ]] || fail "Canonical THIRD_PARTY_LICENSES.html is missing"
  cp THIRD_PARTY_LICENSES.html "${PACKAGE_DIR}/"
fi

echo "Staged npm package legal files: ${package_name}"
