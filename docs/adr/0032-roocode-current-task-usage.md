# ADR 0032: Roo Code current task usage

## Status

Accepted. Refines ADR 0001 and ADR 0002.

## Context

Current Roo Code task logs store inclusive prompt tokens in `tokensIn`.
Adding cache reads and writes to that counter overcounts usage. Bare
`apiProtocol` values describe the API wire format rather than the serving
provider. Discovery previously assumed Linux's VS Code data directory.

## Decision

The accepted Roo task schema uses inclusive `tokensIn` on every transport.
Subtract `cacheReads` and `cacheWrites` exactly once. Negative counters,
overflow, or cache subsets exceeding the prompt reject that event visibly.
There is no timestamp heuristic for older net-input formats.

Preserve nested protocol routing labels, then explicit `modelInfo.providerId`.
Otherwise use the central model-family inference and `unknown` rule. A bare
`openai` or `anthropic` transport does not establish a provider. Billing scope
continues to come from explicit `modelInfo.providerId`, and recorded monetary
costs do not replace Tokenx catalog estimates.

Resolve the current platform's VS Code directory beneath the captured home:
`.config/Code` on Linux, `Library/Application Support/Code` on macOS, and
`AppData/Roaming/Code` on Windows. Also retain the `.vscode-server` root and
explicit extra scan roots. Ambient APPDATA does not override the captured home.

The source-derived decoder fingerprint invalidates old source shards.
Generation schema 16 invalidates previous Roo totals without migration.

## Consequences

Cache-heavy Roo requests have disjoint token buckets and retain provider
attribution independent of their transport. All supported desktop platforms
discover their native VS Code task store.
