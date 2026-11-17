#!/usr/bin/env bash
set -euo pipefail

PROJECT_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${PROJECT_ROOT}"

bun install --frozen-lockfile
python3 scripts/test-check-staged-ci.py
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-features -- -D warnings
cargo test --locked --workspace --all-features
cargo build --locked --workspace --all-targets --all-features
bun run build
bun run test:release-tooling
bun run test:launchers
gitleaks git --no-banner --redact --log-opts=--all
uvx --from zizmor==1.28.0 zizmor --pedantic --no-online-audits --min-severity medium .github

# Fetch the advisory database after the longer build/test checks so a previous
# successful audit is never reused as the push-time dependency check.
cargo audit --deny warnings
