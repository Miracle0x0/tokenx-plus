# Release process

Repository releases build the supported native binaries and create a GitHub Release from an immutable version-bump commit on the default branch. The release workflow never edits or pushes the protected branch.

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

When the version-bump commit reaches the default branch, the `Release` workflow automatically:

1. verifies that every Rust, Bun, and npm manifest has the same increasing version;
2. verifies that the commit is an exact version-only change;
3. builds and smoke-tests the three supported native binaries with locked dependencies;
4. rechecks that the release commit is still the default-branch head;
5. creates `v<version>` and the GitHub Release at the verified commit after every native build succeeds.

No manual workflow dispatch is needed for a normal release.

## Maintainer direct release

The repository owner may prepare the same version-only commit directly on the default branch and push it using an existing ruleset bypass. That push receives the same validation. This is an explicit maintainer path, not the routine release path.

Do not combine code, documentation, dependency, or workflow changes with the version bump.

## Recovery

If the native build or GitHub Release creation fails, run the `Release` workflow manually from the default branch with:

- `version`: the failed release version;
- `commit`: the exact version-bump commit SHA from the failed run.

Recovery accepts only a commit on default-branch history that introduced the requested version and passed the exact version-only check. The native binaries are rebuilt, and the tag and GitHub Release are completed at that same commit.

Do not use a later commit that happens to retain the same version. An incorrect completed release must be followed by a new version.

## Required repository state

`GITHUB_TOKEN` is granted `contents: write` only in the final tag/Release job and cannot push the default branch. The repository must also authorize CodSpeed, enable appropriate branch rules, and allow the pinned third-party Actions used by the workflows.

Before changing release infrastructure, run:

```bash
bun run test:release-tooling
```

This is the canonical release-tooling suite used by CI. Add or remove release checks there instead of maintaining separate command lists in workflow YAML.
