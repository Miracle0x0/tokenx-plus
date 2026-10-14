#!/usr/bin/env python3
"""Package a built native binary and its notices for GitHub Releases."""

import argparse
import hashlib
from pathlib import Path
import tarfile
import zipfile


TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "x86_64-pc-windows-msvc",
)
NOTICES = ("LICENSE", "NOTICE", "THIRD_PARTY_LICENSES.html")


def package(target: str, root: Path, output: Path) -> Path:
    windows = target.endswith("windows-msvc")
    binary_name = "tokenx.exe" if windows else "tokenx"
    binary = root / "target" / target / "release" / binary_name
    files = [binary, *(root / name for name in NOTICES)]
    # Missing binaries or legal notices are build failures, not partial assets.
    for path in files:
        if not path.is_file():
            raise FileNotFoundError(path)
    output.mkdir(parents=True, exist_ok=True)
    suffix = "zip" if windows else "tar.gz"
    archive_path = output / f"tokenx-{target}.{suffix}"
    if windows:
        with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for path in files:
                archive.write(path, path.name)
    else:
        with tarfile.open(archive_path, "w:gz") as archive:
            for path in files:
                info = archive.gettarinfo(path, arcname=path.name)
                info.mode = 0o755 if path == binary else 0o644
                info.uid = info.gid = 0
                info.uname = info.gname = ""
                with path.open("rb") as source:
                    archive.addfile(info, source)
    with archive_path.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    archive_path.with_name(archive_path.name + ".sha256").write_text(
        f"{digest}  {archive_path.name}\n", encoding="utf-8", newline="\n"
    )
    return archive_path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=TARGETS, required=True)
    parser.add_argument("--output", type=Path, default=Path("dist"))
    args = parser.parse_args()
    print(package(args.target, Path.cwd(), args.output))


if __name__ == "__main__":
    main()
