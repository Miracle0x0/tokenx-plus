#!/usr/bin/env python3
"""Apply and verify Tokenx's release-only version changes."""

from __future__ import annotations

import argparse
import copy
import json
import pathlib
import re
import subprocess
from collections.abc import Callable

ROOT = pathlib.Path.cwd()
PLATFORM_PACKAGES = (
    "@juya-ai/tokenx-darwin-arm64",
    "@juya-ai/tokenx-linux-x64-gnu",
    "@juya-ai/tokenx-win32-x64-msvc",
)
LAUNCHER_PATH = pathlib.Path("packages/tokenx/package.json")
WORKSPACE_PACKAGES = ("tokenx", "tokenx-engine")
PLATFORM_PREFIX = "@juya-ai/tokenx-"


def fail(message: str) -> None:
    raise SystemExit(message)


def read_json(text: str, label: str) -> dict:
    try:
        value = json.loads(text)
    except json.JSONDecodeError as error:
        fail(f"{label}: invalid JSON: {error}")
    if not isinstance(value, dict):
        fail(f"{label}: expected a JSON object")
    return value


def dump_json(value: dict) -> str:
    return json.dumps(value, indent=2, ensure_ascii=False) + "\n"


def platform_manifests(launcher: dict) -> list[tuple[str, pathlib.Path]]:
    optional = launcher.get("optionalDependencies")
    if not isinstance(optional, dict) or not optional:
        fail("packages/tokenx/package.json must define platform optionalDependencies")

    result: list[tuple[str, pathlib.Path]] = []
    for package_name in sorted(optional):
        if not package_name.startswith(PLATFORM_PREFIX):
            fail(f"Unexpected optional dependency package name: {package_name}")
        package_dir = package_name.removeprefix("@juya-ai/")
        result.append((package_name, pathlib.Path("packages") / package_dir / "package.json"))
    actual = tuple(name for name, _ in result)
    if actual != PLATFORM_PACKAGES:
        fail(f"Expected platform packages {PLATFORM_PACKAGES}, found {actual}")
    return result


def release_paths_from_launcher(launcher: dict) -> list[pathlib.Path]:
    return [
        pathlib.Path("Cargo.toml"),
        pathlib.Path("Cargo.lock"),
        pathlib.Path("bun.lock"),
        LAUNCHER_PATH,
        *(path for _, path in platform_manifests(launcher)),
    ]


def replace_workspace_version(text: str, old: str, new: str) -> str:
    pattern = re.compile(
        r'(\[workspace\.package\](?:(?!^\[).|\n)*?^version = ")([^"]+)(")',
        re.MULTILINE,
    )
    match = pattern.search(text)
    if match is None:
        fail("Cargo.toml: missing [workspace.package] version")
    if match.group(2) != old:
        fail(f"Cargo.toml: expected version {old}, found {match.group(2)}")
    updated, count = pattern.subn(rf"\g<1>{new}\g<3>", text, count=1)
    if count != 1:
        fail("Cargo.toml: failed to update workspace version exactly once")
    return updated


def replace_lock_versions(text: str, old: str, new: str) -> str:
    parts = re.split(r"(?=^\[\[package\]\]\n)", text, flags=re.MULTILINE)
    found: set[str] = set()
    for index, block in enumerate(parts):
        name_match = re.search(r'^name = "([^"]+)"$', block, re.MULTILINE)
        if name_match is None or name_match.group(1) not in WORKSPACE_PACKAGES:
            continue
        if re.search(r"^source = ", block, re.MULTILINE):
            continue
        package_name = name_match.group(1)
        version_match = re.search(r'^version = "([^"]+)"$', block, re.MULTILINE)
        if version_match is None:
            fail(f"Cargo.lock: {package_name} is missing a version")
        if version_match.group(1) != old:
            fail(
                f"Cargo.lock: expected {package_name} {old}, "
                f"found {version_match.group(1)}"
            )
        parts[index], count = re.subn(
            rf'^version = "{re.escape(old)}"$',
            f'version = "{new}"',
            block,
            count=1,
            flags=re.MULTILINE,
        )
        if count != 1:
            fail(f"Cargo.lock: failed to update {package_name}")
        found.add(package_name)

    missing = set(WORKSPACE_PACKAGES) - found
    if missing:
        fail(f"Cargo.lock: missing workspace packages: {sorted(missing)}")
    return "".join(parts)


def object_bounds(text: str, key: str) -> tuple[int, int]:
    marker = f'    "{key}": {{'
    marker_start = text.find(marker)
    if marker_start < 0:
        fail(f"bun.lock: missing workspace entry {key}")
    start = text.find("{", marker_start)
    depth = 0
    in_string = False
    escaped = False
    for index in range(start, len(text)):
        character = text[index]
        if in_string:
            if escaped:
                escaped = False
            elif character == "\\":
                escaped = True
            elif character == '"':
                in_string = False
            continue
        if character == '"':
            in_string = True
        elif character == "{":
            depth += 1
        elif character == "}":
            depth -= 1
            if depth == 0:
                return start, index + 1
    fail(f"bun.lock: unterminated workspace entry {key}")


def replace_bun_workspace_version(
    text: str,
    workspace: str,
    old: str,
    new: str,
    optional_packages: list[str] | None = None,
) -> str:
    start, end = object_bounds(text, workspace)
    block = text[start:end]
    block, count = re.subn(
        rf'(?m)^(      "version": "){re.escape(old)}(",)$',
        rf"\g<1>{new}\g<2>",
        block,
        count=1,
    )
    if count != 1:
        fail(f"bun.lock: {workspace} must contain direct version {old}")

    for package_name in optional_packages or []:
        block, count = re.subn(
            rf'(?m)^(        "{re.escape(package_name)}": "){re.escape(old)}(",)$',
            rf"\g<1>{new}\g<2>",
            block,
            count=1,
        )
        if count != 1:
            fail(
                f"bun.lock: {workspace} optional dependency {package_name} "
                f"must have version {old}"
            )
    return text[:start] + block + text[end:]


def transform_files(
    load: Callable[[pathlib.Path], str], old: str, new: str
) -> dict[pathlib.Path, str]:
    launcher = read_json(load(LAUNCHER_PATH), LAUNCHER_PATH.as_posix())
    if launcher.get("version") != old:
        fail(
            f"{LAUNCHER_PATH}: expected version {old}, "
            f"found {launcher.get('version')!r}"
        )
    platforms = platform_manifests(launcher)

    transformed: dict[pathlib.Path, str] = {}
    transformed[pathlib.Path("Cargo.toml")] = replace_workspace_version(
        load(pathlib.Path("Cargo.toml")), old, new
    )
    transformed[pathlib.Path("Cargo.lock")] = replace_lock_versions(
        load(pathlib.Path("Cargo.lock")), old, new
    )

    expected_launcher = copy.deepcopy(launcher)
    expected_launcher["version"] = new
    for package_name, _ in platforms:
        if expected_launcher["optionalDependencies"].get(package_name) != old:
            fail(
                f"{LAUNCHER_PATH}: expected optional dependency {package_name} "
                f"at {old}"
            )
        expected_launcher["optionalDependencies"][package_name] = new
    transformed[LAUNCHER_PATH] = dump_json(expected_launcher)

    for package_name, path in platforms:
        manifest = read_json(load(path), path.as_posix())
        if manifest.get("name") != package_name:
            fail(f"{path}: expected package name {package_name}")
        if manifest.get("version") != old:
            fail(f"{path}: expected version {old}, found {manifest.get('version')!r}")
        expected_manifest = copy.deepcopy(manifest)
        expected_manifest["version"] = new
        transformed[path] = dump_json(expected_manifest)

    bun_text = load(pathlib.Path("bun.lock"))
    optional_names = [name for name, _ in platforms]
    bun_text = replace_bun_workspace_version(
        bun_text, "packages/tokenx", old, new, optional_names
    )
    for _, path in platforms:
        bun_text = replace_bun_workspace_version(
            bun_text, path.parent.as_posix(), old, new
        )
    transformed[pathlib.Path("bun.lock")] = bun_text
    return transformed


def load_working(path: pathlib.Path) -> str:
    return (ROOT / path).read_text(encoding="utf-8")


def git_show(revision: str, path: pathlib.Path) -> str:
    result = subprocess.run(
        ["git", "show", f"{revision}:{path.as_posix()}"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=False,
    )
    if result.returncode != 0:
        fail(f"Unable to read {path} at {revision}: {result.stderr.strip()}")
    return result.stdout


def check_version(version: str) -> None:
    launcher = read_json(load_working(LAUNCHER_PATH), LAUNCHER_PATH.as_posix())
    platforms = platform_manifests(launcher)
    errors: list[str] = []

    try:
        import tomllib
    except ModuleNotFoundError:
        fail("Python tomllib is required (Python 3.11+)")

    cargo_data = tomllib.loads(load_working(pathlib.Path("Cargo.toml")))
    workspace_version = cargo_data.get("workspace", {}).get("package", {}).get("version")
    if workspace_version != version:
        errors.append(f"Cargo.toml workspace version: expected {version}, found {workspace_version}")

    cargo_lock = tomllib.loads(load_working(pathlib.Path("Cargo.lock")))
    lock_versions = {
        package.get("name"): package.get("version")
        for package in cargo_lock.get("package", [])
        if package.get("name") in WORKSPACE_PACKAGES and "source" not in package
    }
    for package_name in WORKSPACE_PACKAGES:
        if lock_versions.get(package_name) != version:
            errors.append(
                f"Cargo.lock package {package_name}: expected {version}, "
                f"found {lock_versions.get(package_name)}"
            )

    if launcher.get("name") != "@juya-ai/tokenx":
        errors.append("packages/tokenx/package.json has an unexpected package name")
    if launcher.get("version") != version:
        errors.append(
            f"packages/tokenx/package.json: expected {version}, "
            f"found {launcher.get('version')}"
        )

    expected_names = {name for name, _ in platforms}
    optional = launcher.get("optionalDependencies", {})
    if set(optional) != expected_names:
        errors.append("launcher optionalDependencies do not match platform manifests")
    for package_name, path in platforms:
        if optional.get(package_name) != version:
            errors.append(f"launcher optional dependency {package_name}: expected {version}")
        manifest = read_json(load_working(path), path.as_posix())
        if manifest.get("name") != package_name:
            errors.append(f"{path}: expected package name {package_name}")
        if manifest.get("version") != version:
            errors.append(f"{path}: expected version {version}")

    bun_text = load_working(pathlib.Path("bun.lock"))
    try:
        replace_bun_workspace_version(
            bun_text, "packages/tokenx", version, version, sorted(expected_names)
        )
        for _, path in platforms:
            replace_bun_workspace_version(
                bun_text, path.parent.as_posix(), version, version
            )
    except SystemExit as error:
        errors.append(str(error))

    if errors:
        fail("Version coherence check failed:\n- " + "\n- ".join(errors))
    print(f"Version coherence OK: {version}")


def main() -> None:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)
    subparsers.add_parser("paths")

    check_parser = subparsers.add_parser("check")
    check_parser.add_argument("--version", required=True)

    bump_parser = subparsers.add_parser("bump")
    bump_parser.add_argument("--old", required=True)
    bump_parser.add_argument("--new", required=True)

    verify_parser = subparsers.add_parser("verify")
    verify_parser.add_argument("--base", required=True)
    verify_parser.add_argument("--release", required=True)
    verify_parser.add_argument("--old", required=True)
    verify_parser.add_argument("--new", required=True)

    args = parser.parse_args()
    if args.command == "paths":
        launcher = read_json(load_working(LAUNCHER_PATH), LAUNCHER_PATH.as_posix())
        for path in release_paths_from_launcher(launcher):
            print(path.as_posix())
        return
    if args.command == "check":
        check_version(args.version)
        return
    if args.command == "bump":
        transformed = transform_files(load_working, args.old, args.new)
        for path, content in transformed.items():
            (ROOT / path).write_text(content, encoding="utf-8")
        return
    if args.command == "verify":
        expected = transform_files(
            lambda path: git_show(args.base, path), args.old, args.new
        )
        for path, expected_content in expected.items():
            actual = git_show(args.release, path)
            if actual != expected_content:
                fail(
                    f"Release manifest {path} contains changes other than the "
                    f"expected version update {args.old} -> {args.new}"
                )
        return
    fail(f"Unsupported command: {args.command}")


if __name__ == "__main__":
    main()
