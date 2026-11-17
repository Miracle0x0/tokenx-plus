#!/usr/bin/env python3
"""Exercise staged/unstaged isolation with real Git, Bun and Gitleaks."""

import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SOURCE = Path(__file__).resolve().parent.parent


class StagedChecks(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix="tokenx-hook-test-")
        self.addCleanup(self.temporary.cleanup)
        self.repo = Path(self.temporary.name)
        self.environment = os.environ.copy()
        self.environment["CARGO_TARGET_DIR"] = str(self.repo / "cargo-target")
        self.run_git("init", "--quiet", "--initial-branch=main")
        self.run_git("config", "user.name", "Tokenx Hook Fixture")
        self.run_git("config", "user.email", "hook-fixture@example.invalid")
        for file in [".githooks/pre-commit", "scripts/check-staged-ci.py"]:
            target = self.repo / file
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(SOURCE / file, target)
        (self.repo / "package.json").write_text(
            '{"scripts":{"check:ci":"python3 verify.py"}}\n'
        )
        (self.repo / "verify.py").write_text(
            "from pathlib import Path\n"
            "assert Path('value.txt').read_text() == 'staged-good'\n"
            "assert Path('target/shared-probe').read_text() == 'cache-location'\n"
        )
        (self.repo / ".gitignore").write_text("cargo-target/\ntarget/\n")
        (self.repo / "value.txt").write_text("baseline")
        self.run_git("add", ".")
        self.run_git("commit", "--quiet", "-m", "initial fixture")
        self.run_git("config", "core.hooksPath", ".githooks")
        (self.repo / "cargo-target").mkdir()
        (self.repo / "cargo-target/shared-probe").write_text("cache-location")

    def run_git(self, *arguments, check=True):
        return subprocess.run(
            ["git", *arguments], cwd=self.repo, text=True,
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=check, env=self.environment,
        )

    def test_checks_staged_content_and_preserves_unstaged_edits(self):
        path = self.repo / "value.txt"
        path.write_text("staged-good")
        self.run_git("add", "value.txt")
        path.write_text("unstaged-bad")
        result = self.run_git("commit", "--quiet", "-m", "valid staged change", check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.run_git("show", "HEAD:value.txt").stdout, "staged-good")
        self.assertEqual(path.read_text(), "unstaged-bad")
        self.assertEqual(self.run_git("worktree", "list", "--porcelain").stdout.count("worktree "), 1)

    def test_unstaged_fix_cannot_make_a_bad_index_pass(self):
        head = self.run_git("rev-parse", "HEAD").stdout
        path = self.repo / "value.txt"
        path.write_text("staged-bad")
        self.run_git("add", "value.txt")
        path.write_text("staged-good")
        result = self.run_git("commit", "--quiet", "-m", "invalid staged change", check=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.run_git("rev-parse", "HEAD").stdout, head)
        self.assertEqual(self.run_git("show", ":value.txt").stdout, "staged-bad")
        self.assertEqual(path.read_text(), "staged-good")
        self.assertEqual(self.run_git("worktree", "list", "--porcelain").stdout.count("worktree "), 1)


if __name__ == "__main__":
    unittest.main()
