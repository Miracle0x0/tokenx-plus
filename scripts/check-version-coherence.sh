#!/usr/bin/env bash
set -euo pipefail

EXPECTED_VERSION="${1:-}"
if [[ "${EXPECTED_VERSION}" == "--expect-version" ]]; then
  if [[ -z "${2:-}" ]]; then
    echo "--expect-version requires a value" >&2
    exit 2
  fi
  EXPECTED_VERSION="${2}"
fi

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

if [[ -z "${EXPECTED_VERSION}" ]]; then
  EXPECTED_VERSION="$(jq -er '.version' packages/tokenx/package.json)"
fi

python3 scripts/release-manifests.py check --version "${EXPECTED_VERSION}"
