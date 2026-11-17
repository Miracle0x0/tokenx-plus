#!/usr/bin/env python3
"""Run local CI against the index without touching unstaged user changes."""

from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import tempfile


def main() -> None:
    root = Path(subprocess.check_output(
        ["git", "rev-parse", "--show-toplevel"], text=True
    ).strip())
    # Preserve commit -a/-o's temporary index when capturing the tree, then
    # remove repository-local Git variables before entering any other worktree.
    tree = subprocess.check_output(["git", "write-tree"], cwd=root, text=True).strip()
    local_variables = subprocess.check_output(
        ["git", "rev-parse", "--local-env-vars"], cwd=root, text=True
    ).splitlines()
    environment = os.environ.copy()
    for name in local_variables:
        environment.pop(name, None)
    target = Path(environment.get("CARGO_TARGET_DIR", str(root / "target")))
    if not target.is_absolute():
        target = root / target
    environment["CARGO_TARGET_DIR"] = str(target)

    with tempfile.TemporaryDirectory(prefix="tokenx-pre-commit-") as directory:
        snapshot = Path(directory) / "snapshot"
        subprocess.run(
            ["git", "worktree", "add", "--detach", "--no-checkout", str(snapshot), "HEAD"],
            cwd=root, env=environment, check=True,
        )
        try:
            subprocess.run(["git", "read-tree", tree], cwd=snapshot, env=environment, check=True)
            subprocess.run(["git", "checkout-index", "--all"], cwd=snapshot, env=environment, check=True)
            # The candidate commit is not in history yet. Scan its exact files
            # before build outputs and dependencies enter this temporary tree.
            subprocess.run(
                ["gitleaks", "dir", "--no-banner", "--redact", "."],
                cwd=snapshot, env=environment, check=True,
            )
            # Launcher/package scripts use the repository-relative target path.
            # Bind it to the same real artifacts selected for Cargo above.
            (snapshot / "target").symlink_to(target, target_is_directory=True)
            subprocess.run(["bun", "run", "check:ci"], cwd=snapshot, env=environment, check=True)
        finally:
            subprocess.run(
                ["git", "worktree", "remove", "--force", str(snapshot)],
                cwd=root, env=environment, check=True,
            )


if __name__ == "__main__":
    try:
        main()
    except subprocess.CalledProcessError as error:
        print(f"pre-commit check failed: {error.cmd}", file=sys.stderr)
        raise SystemExit(error.returncode)
