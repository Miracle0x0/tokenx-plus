#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

TARGET_PACKAGE_DIR="${1:-}"

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

package_dirs=(
  packages/tokenx
  packages/tokenx-darwin-arm64
  packages/tokenx-linux-x64-gnu
  packages/tokenx-win32-x64-msvc
)
native_package_dirs=(
  packages/tokenx-darwin-arm64
  packages/tokenx-linux-x64-gnu
  packages/tokenx-win32-x64-msvc
)

is_native_package() {
  local candidate="$1"
  local package_dir
  for package_dir in "${native_package_dirs[@]}"; do
    [[ "${candidate}" == "${package_dir}" ]] && return 0
  done
  return 1
}

check_package_files() {
  local package_dir="$1"
  local require_staged_third_party="${2:-false}"
  cmp -s LICENSE "${package_dir}/LICENSE" ||
    fail "${package_dir}/LICENSE is missing or stale; run scripts/generate-third-party-licenses.sh"
  cmp -s NOTICE "${package_dir}/NOTICE" ||
    fail "${package_dir}/NOTICE is missing or stale; run scripts/generate-third-party-licenses.sh"
  if is_native_package "${package_dir}" && [[ "${require_staged_third_party}" == "true" ]]; then
    cmp -s THIRD_PARTY_LICENSES.html "${package_dir}/THIRD_PARTY_LICENSES.html" ||
      fail "${package_dir}/THIRD_PARTY_LICENSES.html is missing or stale; run scripts/stage-npm-package-files.sh ${package_dir}"
  fi
}

if [[ -n "${TARGET_PACKAGE_DIR}" ]]; then
  recognized=false
  for package_dir in "${package_dirs[@]}"; do
    if [[ "${TARGET_PACKAGE_DIR}" == "${package_dir}" ]]; then
      recognized=true
      break
    fi
  done
  [[ "${recognized}" == "true" ]] || fail "Unknown npm package directory: ${TARGET_PACKAGE_DIR}"
  check_package_files "${TARGET_PACKAGE_DIR}" true
  if [[ "${TARGET_PACKAGE_DIR}" == "packages/tokenx" ]]; then
    [[ -f packages/tokenx/README.md ]] || fail "packages/tokenx/README.md is missing"
  fi
  echo "npm package legal files OK: ${TARGET_PACKAGE_DIR}"
  exit 0
fi

[[ -f THIRD_PARTY_LICENSES.html ]] ||
  fail "Canonical THIRD_PARTY_LICENSES.html is missing; run scripts/generate-third-party-licenses.sh"

for package_dir in "${package_dirs[@]}"; do
  check_package_files "${package_dir}" false
done
[[ -f packages/tokenx/README.md ]] || fail "packages/tokenx/README.md is missing"

for package_dir in "${native_package_dirs[@]}"; do
  duplicate="${package_dir}/THIRD_PARTY_LICENSES.html"
  if git ls-files --error-unmatch -- "${duplicate}" >/dev/null 2>&1; then
    fail "${duplicate} must be staged from the canonical root file, not tracked in Git"
  fi
done

python3 - <<'PY'
import json
import pathlib

launcher_path = pathlib.Path("packages/tokenx/package.json")
launcher = json.loads(launcher_path.read_text(encoding="utf-8"))
expected_platforms = {
    "@juya-ai/tokenx-darwin-arm64": {
        "path": pathlib.Path("packages/tokenx-darwin-arm64/package.json"),
        "description": "Tokenx native binary for macOS on Apple silicon.",
        "os": ["darwin"],
        "cpu": ["arm64"],
        "main": "bin/tokenx",
    },
    "@juya-ai/tokenx-linux-x64-gnu": {
        "path": pathlib.Path("packages/tokenx-linux-x64-gnu/package.json"),
        "description": "Tokenx native binary for Linux x64 with glibc.",
        "os": ["linux"],
        "cpu": ["x64"],
        "libc": ["glibc"],
        "main": "bin/tokenx",
    },
    "@juya-ai/tokenx-win32-x64-msvc": {
        "path": pathlib.Path("packages/tokenx-win32-x64-msvc/package.json"),
        "description": "Tokenx native binary for Windows x64.",
        "os": ["win32"],
        "cpu": ["x64"],
        "main": "bin/tokenx.exe",
    },
}
errors = []

if launcher.get("description") != "Local AI coding usage reports for CLI and TUI workflows.":
    errors.append("launcher description is stale")
if launcher.get("engines") != {"node": ">=20"}:
    errors.append("launcher must require Node.js >=20")
if launcher.get("files") != ["bin.js", "dist/**/*", "README.md", "LICENSE", "NOTICE"]:
    errors.append("launcher files list is incomplete")
if set(launcher.get("optionalDependencies", {})) != set(expected_platforms):
    errors.append("launcher optionalDependencies do not match supported native packages")

expected_native_files = ["bin", "LICENSE", "NOTICE", "THIRD_PARTY_LICENSES.html"]
for package_name, expected in expected_platforms.items():
    path = expected.pop("path")
    manifest = json.loads(path.read_text(encoding="utf-8"))
    if manifest.get("name") != package_name:
        errors.append(f"{path}: unexpected package name")
    for field, value in expected.items():
        if manifest.get(field) != value:
            errors.append(f"{path}: expected {field}={value!r}, found {manifest.get(field)!r}")
    if manifest.get("files") != expected_native_files:
        errors.append(f"{path}: native package files list is incomplete")

if errors:
    raise SystemExit("npm package metadata check failed:\n- " + "\n- ".join(errors))
PY

echo "npm package files and metadata OK"
