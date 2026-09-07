#!/usr/bin/env python3
"""Check the GNU Zig linker adapter's argument and process contract."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ADAPTER = Path(__file__).with_name("zig-linker.py")


class ZigLinkerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="tokenx-zig-linker-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.linker = self.root / "linker with spaces"
        self.linker.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, sys\n"
            "print(json.dumps(sys.argv[1:]))\n"
            "print('warning: retained linker diagnostic', file=sys.stderr)\n"
            "sys.exit(int(os.environ['TEST_LINKER_EXIT']))\n",
            encoding="utf-8",
        )
        self.linker.chmod(0o755)

    def invoke(self, args: list[str], exit_code: int = 0) -> subprocess.CompletedProcess:
        return subprocess.run(
            [sys.executable, str(ADAPTER), *args],
            env={
                **os.environ,
                "CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER": str(self.linker),
                "TEST_LINKER_EXIT": str(exit_code),
            },
            capture_output=True,
            text=True,
            check=False,
        )

    def test_only_ignored_linker_optimization_is_removed(self) -> None:
        kept = ["-O3", "-Wl,-O2", "-Wl,--gc-sections", "input with spaces.o", "-o", "out"]
        result = self.invoke(["-Wl,-O1", *kept, "-Wl,-O1"])
        self.assertEqual(result.returncode, 0)
        self.assertEqual(json.loads(result.stdout), kept)
        self.assertEqual(result.stderr, "warning: retained linker diagnostic\n")

    def test_failure_status_and_diagnostic_are_preserved(self) -> None:
        result = self.invoke(["-Wl,-O1", "missing.o"], exit_code=23)
        self.assertEqual(result.returncode, 23)
        self.assertEqual(result.stderr, "warning: retained linker diagnostic\n")

    def test_response_file_preserves_arguments_and_source(self) -> None:
        source = self.root / "linker-arguments"
        kept = [r"input\ with\ spaces.o", r"quote\'in-name.o", "-Wl,--gc-sections", ""]
        original = "\n".join(["-Wl,-O1", *kept])
        source.write_text(original, encoding="utf-8")
        result = self.invoke(["@" + str(source)])
        self.assertEqual(result.returncode, 0)
        (response_arg,) = json.loads(result.stdout)
        self.assertTrue(response_arg.endswith("linker-arguments"))
        rewritten = Path(response_arg[1:])
        self.assertEqual(rewritten.read_bytes(), "\n".join(kept).encode())
        self.assertEqual(source.read_text(encoding="utf-8"), original)


if __name__ == "__main__":
    unittest.main()
