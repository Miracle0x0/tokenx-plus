# ADR 0037: Decoder contracts exclude explicit test-only source files

## Status

Accepted. Refines the source-derived decoder contract in ADR 0003.

## Context

Decoder cache identity hashes shared and integration-local Rust sources. The
previous recursive discovery also hashed standalone test modules. Editing an
assertion therefore invalidated authoritative-input shards even though the
production decoder was unchanged. Large inline decoder test modules also made
production source files harder to navigate.

Watching only the files found during one build missed newly added source files.
Rewriting identical generated output on every build-script run caused additional
file churn.

## Decision

The decoder contract remains a SHA-256 digest of source paths and complete file
bytes. It is derived automatically; no handwritten decoder revision or alternate
cache interpretation is introduced.

Shared source ownership consists of the explicitly listed crate-root files,
the `records`, `acquisition`, and `finalization` source directories, and the Rust
files directly under `integrations`. Each integration additionally owns the Rust
files recursively discovered beneath its own directory. New production helpers
under these directories automatically enter the corresponding contract. A new
shared source root must be added to the build-time source declarations.

A discovered file whose first line is `#![cfg(test)]`, ignoring surrounding
whitespace, is excluded from the contract. This is a Rust inner attribute that
excludes the entire file from production compilation, not a filename convention
or a comment interpreted by Tokenx. Standalone decoder tests and test-support
modules use this attribute. Claude and Codex decoder test bodies live in their
integration's `tests/decode.rs` while preserving their original module names.

Names such as `tests.rs`, `test_support.rs`, or a `tests` directory do not imply
exclusion. Files without the first-line attribute remain included. Inline test
code, comments, and other conditional-compilation layouts inside production
files are still hashed conservatively. The generator does not parse or rewrite
Rust source to infer more exclusions.

Cargo watches each discovered production file and the source directories used
for discovery. Directory watches cover additions, removals, and changes to a
file's first-line attribute. An edit to a marked test file may still rerun the
build script because Cargo watches directories recursively, but it does not
change the generated decoder contract.

Generated client-catalog and decoder-contract files are written only when their
contents differ. A missing output is created; other read or write failures remain
explicit build errors. Unchanged output keeps its modification time.

## Consequences

- Production changes still invalidate the owning decoder automatically; shared
  changes invalidate every decoder.
- Editing a marked standalone test file preserves decoder identity. Removing its
  attribute restores ordinary production-source invalidation.
- The first build with this source selection may invalidate existing shards;
  authoritative inputs rebuild them through the existing cache-miss path.
- The shard serialization format, input stamps, parsing, pricing, and runtime
  cache behavior are unchanged.
- This decision reduces unnecessary cache invalidation and generated-file
  writes. It does not promise that test edits avoid all compilation, or that
  build-script hashing is a measured build-time bottleneck.
