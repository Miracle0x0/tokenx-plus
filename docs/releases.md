# Release process

Repository releases build native archives and publish them as GitHub Release assets from an immutable version-bump commit on `main`. Publishing to npm is not part of this process. The release workflow never edits or pushes the branch.

## Standard release

Prepare the release on a dedicated branch:

```bash
git switch -c release/v<version>
bun run release:bump -- <major|minor|patch|version>
git add Cargo.toml Cargo.lock bun.lock packages/tokenx/package.json \
  packages/tokenx-*/package.json
git commit -m "chore(release): bump version to <version>"
```

Open a pull request targeting the default branch. The release commit must contain only those manifests, and each file must match the exact version-only transformation produced by `scripts/release-manifests.py`. Dependency, script, description, or other metadata changes inside an allowed manifest are rejected.

CI verifies version coherence and release tooling. Squash-merge the pull request after required checks pass.

A push to `main` that changes `Cargo.toml` starts release validation. The workspace version in `Cargo.toml` is authoritative; unchanged versions skip release builds. When the final commit introduces an increasing version, the `Release` workflow automatically:

1. verifies that every Rust, Bun, and npm manifest has the same increasing version;
2. verifies that the commit is an exact version-only change;
3. builds and smoke-tests the three supported native binaries with locked dependencies;
4. packages each binary with `LICENSE`, `NOTICE`, and `THIRD_PARTY_LICENSES.html`, and computes a SHA-256 checksum;
5. downloads all build artifacts, verifies their checksums, and revalidates the release commit;
6. creates `v<version>` and attaches the archives and checksums to the GitHub Release at the verified commit.

No manual workflow dispatch is needed for a normal release. Code and CI commits may be pushed together with the version bump, provided the final commit is the independent version-only commit. A version bump followed by an unrelated commit does not qualify as a release head.

The release contains these archives and a corresponding `.sha256` file for each:

| Platform | Archive |
| --- | --- |
| macOS Apple silicon | `tokenx-aarch64-apple-darwin.tar.gz` |
| Linux x64 glibc | `tokenx-x86_64-unknown-linux-gnu.tar.gz` |
| Windows x64 | `tokenx-x86_64-pc-windows-msvc.zip` |

## Maintainer direct release

The repository owner may prepare the same version-only commit directly on the default branch and push it using an existing ruleset bypass. That push receives the same validation. This is an explicit maintainer path, not the routine release path.

Do not combine code, documentation, dependency, or workflow changes with the version bump.

## Recovery

If the native build or GitHub Release creation fails, run the `Release` workflow manually from the default branch with:

- `version`: the failed release version;
- `commit`: the exact version-bump commit SHA from the failed run.

Recovery accepts only a commit on default-branch history that introduced the requested version and passed the exact version-only check. The native binaries are rebuilt, and the tag, GitHub Release, and attached archives are completed at that same commit. Recovery replaces same-named assets; it does not move an existing tag.

Do not use a later commit that happens to retain the same version. An incorrect completed release must be followed by a new version.

## Required repository state

`GITHUB_TOKEN` is granted `contents: write` only in the final tag/Release job. The workflow creates no commits and pushes no branches. The repository must allow the pinned build and artifact Actions. The release requires no npm credentials.

Before changing release infrastructure, run:

```bash
bun run test:release-tooling
```

This is the canonical release-tooling suite used by CI. Add or remove release checks there instead of maintaining separate command lists in workflow YAML.
