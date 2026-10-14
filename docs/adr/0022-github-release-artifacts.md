# ADR 0022: Version-triggered native GitHub Release assets

## Status

Accepted.

## Context

Release builds uploaded native binaries to Actions storage but did not attach
them to the GitHub Release. Unused npm publication scripts remained even though
the workflow no longer published packages. Requiring exactly one commit per
push also prevented maintainers from pushing reviewed code and CI changes
together with a final version-bump commit.

## Decision

The release destination is GitHub Releases. Remove npm registry publication,
registry-state checks, and their tests. Retain launcher workspace manifests and
their local checks; release version updates keep all workspace versions coherent.

`Cargo.toml` owns the release version and the release workflow's push path
filter on `main`. Ordinary changes without a version increase do not build or
publish a release. The head commit must introduce the new version and contain
the exact version-only transformation. Compare that commit to its parent,
allowing preceding implementation commits in the same push. CI consumes the
committed version and never writes commits or pushes the branch.

Each supported target builds and smoke-tests its native binary, then packages
it with `LICENSE`, `NOTICE`, and the canonical `THIRD_PARTY_LICENSES.html`.
macOS and Linux use tar.gz archives with executable binary permissions;
Windows uses a ZIP archive. Every archive has a SHA-256 sidecar. The native
validation workflow exercises the same packaging step.

After all targets succeed, the release job downloads the archives, checks
their hashes, and publishes them at the validated version commit. Only this
job receives `contents: write`. Existing explicit recovery remains tied to the
original version-bump commit and may replace assets without retargeting a tag.

## Consequences

Users can install released native binaries without npm. The project needs no
npm publishing credentials. Maintainers can push code, CI, and an independent
final version commit together; local preparation does not start remote builds
or publish anything until that push occurs.
