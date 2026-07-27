# Architecture decision records

Architecture decision records (ADRs) document accepted Tokenx contracts and the reasons behind them. The numbered files are authoritative when a summary elsewhere conflicts with a decision.

| ADR | Decision | Status |
| --- | --- | --- |
| [0001](0001-no-silent-fallback.md) | No silent fallback and local input integrity | Accepted |
| [0002](0002-client-identity-catalog.md) | Client identity catalog and local input authority | Accepted |
| [0003](0003-single-copy-memory-pipeline.md) | Prepared input, single-copy fold, and cache storage | Accepted |
| [0004](0004-period-views-derive-from-daily.md) | Usage aggregation, identity, and pricing contract | Accepted |
| [0005](0005-explicit-subscription-usage-boundary.md) | Subscription Usage and credential boundary | Accepted |
| [0006](0006-deterministic-cli-command-semantics.md) | Deterministic CLI command semantics | Accepted |
| [0007](0007-tui-client-universe-and-view-selection.md) | TUI generation, projection, and presentation contract | Accepted |
| [0008](0008-semantic-tui-theme-contract.md) | Semantic TUI theme contract | Accepted |
| [0009](0009-canonical-generation-architecture.md) | Canonical generation architecture | Accepted |
| [0010](0010-tokenx-clean-target.md) | Tokenx clean target | Accepted |

## Conventions

New ADRs use the next sequential four-digit number and contain `Status`, `Context`, and `Decision` sections. Update an accepted ADR when its established contract changes; create a new ADR when a later decision supersedes or materially reframes it.
