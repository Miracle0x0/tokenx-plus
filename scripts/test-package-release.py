#!/usr/bin/env python3
"""Validate archive contents, executable modes, and release checksums."""

import hashlib
import importlib.util
from pathlib import Path
import tarfile
import tempfile
import unittest
import zipfile


SPEC = importlib.util.spec_from_file_location(
    "package_release", Path(__file__).with_name("package-release.py")
)
packager = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(packager)


class PackageReleaseTests(unittest.TestCase):
    def write_fixture(self, root: Path, target: str) -> dict[str, bytes]:
        binary_name = "tokenx.exe" if target.endswith("windows-msvc") else "tokenx"
        binary = root / "target" / target / "release" / binary_name
        binary.parent.mkdir(parents=True)
        files = {binary_name: b"binary fixture\x00\xff"}
        binary.write_bytes(files[binary_name])
        for name in packager.NOTICES:
            files[name] = f"Legal notice: {name}\n".encode()
            (root / name).write_bytes(files[name])
        return files

    def test_all_targets_include_binary_notices_and_valid_checksum(self):
        for target in packager.TARGETS:
            with self.subTest(target=target), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                expected = self.write_fixture(root, target)
                output = packager.package(target, root, root / "dist")
                if output.suffix == ".zip":
                    with zipfile.ZipFile(output) as archive:
                        actual = {name: archive.read(name) for name in archive.namelist()}
                else:
                    with tarfile.open(output) as archive:
                        actual = {entry.name: archive.extractfile(entry).read() for entry in archive}
                        self.assertEqual(archive.getmember("tokenx").mode, 0o755)
                self.assertEqual(actual, expected)
                self.assertEqual(
                    output.with_name(output.name + ".sha256").read_text(),
                    f"{hashlib.sha256(output.read_bytes()).hexdigest()}  {output.name}\n",
                )

    def test_missing_inputs_fail_without_creating_release_assets(self):
        target = packager.TARGETS[0]
        for missing in ["tokenx", *packager.NOTICES]:
            with self.subTest(missing=missing), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                self.write_fixture(root, target)
                path = root / "target" / target / "release" / missing if missing == "tokenx" else root / missing
                path.unlink()
                with self.assertRaises(FileNotFoundError):
                    packager.package(target, root, root / "dist")
                self.assertFalse((root / "dist").exists())


if __name__ == "__main__":
    unittest.main()
