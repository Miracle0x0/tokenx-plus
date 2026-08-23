# Development and testing

Tokenx is a Rust workspace with Bun-managed JavaScript packages.

## Layout

```text
crates/
  tokenx-engine/        parsing, scanning, aggregation, pricing, session readers
  tokenx/               binary, CLI, TUI, subscriptions, integration tests

packages/
  tokenx/               @juya-ai/tokenx TypeScript launcher
  tokenx-*/             @juya-ai/tokenx-* native package manifests

docs/
  adr/                  architecture decisions
  releases.md           release and recovery procedure
```

## Build

Install the exact JavaScript dependency graph and build release artifacts:

```bash
bun install --frozen-lockfile
bun run build
```

For narrower checks:

```bash
bun run build:native
bun run build:launcher
bun run cli -- models --no-spinner
```

## Test

```bash
cargo test --locked --workspace --all-features
cargo clippy --locked --workspace --all-features -- -D warnings
cargo fmt --all -- --check
bun run test:release-tooling
bun run test:launchers
```

When running Tokenx from automation, pass `--no-spinner` unless spinner behavior is under test.

The launcher test builds the TypeScript launcher and native binary, packs every npm platform package, checks exact tarball contents, installs local tarballs, and verifies Node and Bun execution. Use `TOKENX_SMOKE_BUILD_PROFILE=release` for release-binary coverage.

## Security checks

The Security workflow runs RustSec, Gitleaks, and Zizmor. Equivalent local checks are:

```bash
cargo audit --deny warnings
bun audit
gitleaks detect --source . --no-banner --redact
uvx zizmor --pedantic --no-online-audits --min-severity medium .github
```

`.cargo/audit.toml` records the single maintenance-only `bincode 1` exception. Bincode defines the current cache wire format, so replacing it requires a deliberate versioned cache migration.

## Package licenses

Native npm tarballs include the license and copyright notices for Rust crates distributed inside the compiled binary. The repository tracks one canonical `THIRD_PARTY_LICENSES.html` at its root; `scripts/publish-npm-package.sh` copies it into each native package staging directory immediately before validation and publication. Platform package copies are generated staging files and must not be tracked.

Install the pinned generator and refresh the canonical report after dependency changes:

```bash
cargo install cargo-about --locked --version 0.9.1 --features cli
bun run licenses:generate
bash scripts/check-npm-package-files.sh
```

Commit the root `THIRD_PARTY_LICENSES.html` together with the dependency change. The generator also synchronizes the tracked `LICENSE` and `NOTICE` package files.

## Performance benchmarks

Engine microbenchmarks use `codspeed-criterion-compat` and run through `.github/workflows/codspeed.yml`. The workflow authenticates with GitHub OIDC and does not require a long-lived `CODSPEED_TOKEN`.

Measure startup, acquisition, and RSS only with `target/release/tokenx`; debug binaries are correctness artifacts. Every new repository must be imported into CodSpeed settings and authorized for the CodSpeed GitHub App before its workflow can publish a baseline.

For local end-to-end acquisition measurements, `scripts/measure-scan-performance.sh` requires `jq` and GNU time. It prefers `gtime`; otherwise it uses `/usr/bin/time` only after checking support for GNU `-f` and `-o`.

## Client identity

Client identity is catalog-driven. Update `crates/tokenx-engine/client-catalog.json` when adding or renaming a client; the Rust build script validates the catalog and generates compiled identity data.

## Releases

Use `bun run release:bump -- <major|minor|patch|version>` to update every Rust, Bun, and npm release manifest together. The standard path is a version-only pull request; merging it to the default branch triggers native release builds, the version tag, and the GitHub Release. See [the release process](releases.md).

## Documentation changes

Keep `README.md` as the concise product entry page and synchronize `README.zh-cn.md` whenever its structure or claims change. Put detailed command, client, pricing, configuration, and development material under `docs/`.

If a client list becomes repetitive, link to `docs/clients.md` or generate it from `crates/tokenx-engine/client-catalog.json` rather than maintaining another manual catalog.
