#!/usr/bin/env python3
"""Remove Zig's ignored linker -O1 without hiding linker diagnostics."""

import os
from pathlib import Path
import sys


def linker_args(args: list[str]) -> list[str]:
    result = []
    for arg in args:
        if arg == "-Wl,-O1":
            continue
        if arg.startswith("@") and arg.endswith("linker-arguments"):
            source = Path(arg[1:])
            # Rust's GNU response files and cargo-zigbuild's adapter use one
            # argument per line. Preserve the remaining bytes and escaping.
            filtered = b"\n".join(
                line for line in source.read_bytes().split(b"\n") if line != b"-Wl,-O1"
            )
            # Keep cargo-zigbuild's response-file recognition and Rust's
            # temporary-directory cleanup. Do not rewrite the original input.
            target = source.with_name("tokenx-" + source.name)
            target.write_bytes(filtered)
            arg = "@" + str(target)
        result.append(arg)
    return result


def main() -> None:
    # cargo-zigbuild exports the real linker before cargo rustc applies our
    # final-crate override. This preserves CPU, ABI and Zig toolchain selection.
    linker = os.environ["CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER"]
    os.execvp(linker, [linker, *linker_args(sys.argv[1:])])


if __name__ == "__main__":
    main()
