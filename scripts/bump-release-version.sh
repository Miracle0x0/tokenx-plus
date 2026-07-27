#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

[[ $# -eq 1 ]] || fail "Usage: bun run release:bump -- <major|minor|patch|version>"
request="$1"
current_version="$(jq -er '.version' packages/tokenx/package.json)"

new_version="$(python3 - "${current_version}" "${request}" <<'PY'
import re
import sys

current, request = sys.argv[1:]
identifier = r"(?:0|[1-9A-Za-z-][0-9A-Za-z-]*)"
pattern = re.compile(
    r"^(0|[1-9][0-9]*)\."
    r"(0|[1-9][0-9]*)\."
    r"(0|[1-9][0-9]*)"
    rf"(?:-({identifier}(?:\.{identifier})*))?"
    rf"(?:\+({identifier}(?:\.{identifier})*))?$"
)


def parse(value: str) -> tuple[tuple[int, int, int], list[str] | None]:
    match = pattern.fullmatch(value)
    if not match:
        raise SystemExit(f"Invalid semantic version: {value}")
    major, minor, patch, prerelease, _build = match.groups()
    return (int(major), int(minor), int(patch)), prerelease.split(".") if prerelease else None


def compare_identifiers(left: list[str] | None, right: list[str] | None) -> int:
    if left is None or right is None:
        if left is None and right is None:
            return 0
        return 1 if left is None else -1
    for left_item, right_item in zip(left, right):
        if left_item == right_item:
            continue
        left_numeric = left_item.isdigit()
        right_numeric = right_item.isdigit()
        if left_numeric and right_numeric:
            return 1 if int(left_item) > int(right_item) else -1
        if left_numeric != right_numeric:
            return -1 if left_numeric else 1
        return 1 if left_item > right_item else -1
    return (len(left) > len(right)) - (len(left) < len(right))


current_core, current_pre = parse(current)
major, minor, patch = current_core
if request == "major":
    release = f"{major + 1}.0.0"
elif request == "minor":
    release = f"{major}.{minor + 1}.0"
elif request == "patch":
    release = f"{major}.{minor}.{patch + 1}"
else:
    parse(request)
    release = request

release_core, release_pre = parse(release)
comparison = (release_core > current_core) - (release_core < current_core)
if comparison == 0:
    comparison = compare_identifiers(release_pre, current_pre)
if comparison <= 0:
    raise SystemExit(f"Release version {release} must be greater than current version {current}")
print(release)
PY
)"

release_path_output="$(python3 scripts/release-manifests.py paths)"
mapfile -t release_paths <<< "${release_path_output}"

git diff --quiet -- "${release_paths[@]}" ||
  fail "Release manifests contain unstaged changes"
git diff --cached --quiet -- "${release_paths[@]}" ||
  fail "Release manifests contain staged changes"

python3 scripts/release-manifests.py bump \
  --old "${current_version}" \
  --new "${new_version}"

cargo metadata --locked --format-version 1 >/dev/null
bash scripts/check-version-coherence.sh --expect-version "${new_version}"

echo "Prepared release version ${current_version} -> ${new_version}"
echo "Commit only the release manifests before merging or pushing to ${DEFAULT_BRANCH:-the default branch}."
