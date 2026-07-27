# Tokenx

[English](README.md) | [简体中文](README.zh-cn.md)

> Local AI coding-client usage accounting with explicit data semantics and predictable resource use on large transcript collections.

Tokenx reads token-bearing records from local AI coding clients and presents them through an interactive TUI or deterministic CLI output. Local transcripts and databases stay on the machine; only the optional Subscription view contacts provider account services.

## Install

```bash
npm install --global @juya-ai/tokenx
tokenx
```

Prebuilt npm packages support macOS on Apple silicon, Linux x64 with glibc, and Windows x64. Other targets can build Tokenx from source with Bun and a stable Rust toolchain:

```bash
git clone https://github.com/makoMakoGo/tokenx.git
cd tokenx
bun install --frozen-lockfile
bun run build
bun run cli
```

## Use

```bash
# Interactive TUI
tokenx
tokenx tui --tab models

# Script-friendly usage projection
tokenx models --no-spinner
tokenx models --json --no-spinner
tokenx models --client codex --group-by client,provider,model --no-spinner

# Date filters
tokenx tui --client opencode,claude --week
tokenx models --since 2026-01-01 --until 2026-01-31 --no-spinner

# Pricing catalog
tokenx pricing lookup claude-sonnet-4-5 --no-spinner
tokenx pricing overrides --json
```

When running from source, replace `tokenx` with `bun run cli --`.

## Behavior

- **Local usage remains local.** Token accounting is derived from accepted local records. Vendor spend, credits, balances, and cost-only rows are not mixed into local token cost.
- **Failures remain visible.** Unreadable inputs, rejected records, unknown attribution, and unmatched pricing appear as errors or Data Health diagnostics instead of guessed success.
- **Identity is stable.** One generated client catalog defines command IDs, display names, integration bindings, caches, and projections.
- **Views share one generation.** CLI Models and TUI views project the same immutable acquisition result; changing a view does not rescan inputs.
- **Caches are disposable.** Input shards and the generation cache accelerate reads but never replace authoritative client data.

## Supported clients

Tokenx supports local data from OpenCode, Claude Code, Codex, Gemini CLI, Amp, and many other coding clients. The executable catalog is `crates/tokenx-engine/client-catalog.json`; see [supported clients and data locations](docs/clients.md) for the complete current list, platform paths, schema boundaries, and Data Health behavior.

## Pricing

Tokenx canonicalizes model IDs before grouping and pricing. Exact custom overrides are checked first, followed by exact matches from the configured public catalogs. Prefix, substring, and fuzzy price guesses are not used. An unpriced model keeps its token usage and reports derived cost as `$0.00`.

See [pricing semantics](docs/pricing.md) for catalog precedence, total-only token allocation, and cost boundaries.

## Documentation

- [Supported clients and data locations](docs/clients.md)
- [CLI usage](docs/cli.md)
- [Configuration](docs/configuration.md)
- [Pricing semantics](docs/pricing.md)
- [Development and testing](docs/development.md)
- [Release process](docs/releases.md)
- [Architecture decisions](docs/adr/README.md)

## License and attribution

Tokenx originated from [Tokscale](https://github.com/junhoyeo/tokscale) by Junho Yeo and retains its MIT attribution. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
