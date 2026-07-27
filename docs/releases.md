# Release process

Repository releases publish the `@juya-ai/tokenx` package family from an immutable version-bump commit on the default branch. The publish workflow never edits or pushes the protected branch.

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

When the version-bump commit reaches the default branch, the `Publish` workflow automatically:

1. verifies that every Rust, Bun, and npm manifest has the same increasing version;
2. verifies that the commit is an exact version-only change;
3. builds and smoke-tests the three supported native packages with locked dependencies;
4. rechecks that the release commit is still the default-branch head;
5. publishes native packages and then the launcher package;
6. creates `v<version>` and the GitHub Release at the verified commit after every npm publish succeeds.

No manual workflow dispatch is needed for a normal release.

## Maintainer direct release

The repository owner may prepare the same version-only commit directly on the default branch and push it using an existing ruleset bypass. That push receives the same validation. This is an explicit maintainer path, not the routine release path.

Do not combine code, documentation, dependency, or workflow changes with the version bump.

## Recovery

npm publication is not atomic. If only part of the package family was published, run the `Publish` workflow manually from the default branch with:

- `version`: the failed release version;
- `commit`: the exact version-bump commit SHA from the failed run.

Recovery accepts only a commit on default-branch history that introduced the requested version and passed the exact version-only check. Existing npm package versions are skipped, missing packages are published, and the tag and GitHub Release are completed at that same commit.

Do not use a later commit that happens to retain the same version. An npm version cannot be replaced; an incorrect completed release must be followed by a new version.

## Required repository state

The repository must provide an Actions secret named `NPM_TOKEN` with publish access to all four `@juya-ai/tokenx*` packages. Treat any token exposed in chat, logs, shell history, or source as compromised and rotate it before use. `GITHUB_TOKEN` is granted `contents: write` only in the final tag/Release job and cannot push the default branch.

The repository must also authorize CodSpeed, enable appropriate branch rules, and allow the pinned third-party Actions used by the workflows.

Before changing release infrastructure, run:

```bash
bun run test:release-tooling
```

This is the canonical release-tooling suite used by CI. Add or remove release checks there instead of maintaining separate command lists in workflow YAML.
