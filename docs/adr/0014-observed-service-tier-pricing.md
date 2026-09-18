# ADR 0014: Observed service-tier pricing

## Status

Accepted.

## Context

Codex records service-tier changes independently of model identity. The public
LiteLLM catalog provides priority and flex prices, but Tokenx discarded both
the observed setting and the corresponding rates. Model-only costs therefore
could not reflect recorded tier changes. This decision supersedes ADR 0004's
former exclusion of service tier as a pricing dimension.

## Decision

Codex thread-settings snapshots configure subsequent usage records. Tier is
source metadata retained in incremental parser state and cost-free input shards;
it is not a model alias or aggregation dimension. No current configuration,
future setting, or inherited fork setting supplies missing historical metadata.
Absent/null metadata retains ordinary catalog estimation, while explicit
`default`, `priority`/`fast`, and `flex` select ordinary, priority, and flex rates.
Unknown explicit tiers are not normalized to standard.

Pricing retains the catalog's explicit per-bucket tier prices. No multiplier is
invented. For the supported OpenAI 272k long-context fields, inclusive prompt
size selects rates for the entire request. Custom overrides and source order
retain authority; an incomplete selected row is not supplemented from another
catalog or from standard rates.

Missing or invalid tier rates keep authoritative tokens and exclude the
unpriced record's cost. A typed generation pricing diagnostic identifies the
model, tier, and affected record count. These diagnostics are derived during
aggregation, excluded from source shards, and retained when catalog diagnostics
are rebound on a cached generation. Standard pricing's existing computation
error handling is unchanged.

Input-shard and generation formats are advanced. Pricing-cache envelopes gain
an explicit version, making old catalogs that discarded tier fields invalid;
the existing catalog acquisition lifecycle fetches current data. Decoder
source fingerprints also invalidate affected parse state automatically.

## Consequences

Mixed-tier sessions are priced per usage record before aggregation. Cold scans,
cache hits, incremental appends, and repricing use the same observed metadata.
Records without the necessary rate remain visible as token usage with a
pricing warning, not a fabricated standard-tier cost. The recorded thread
setting is requested-service evidence, so estimates cannot reconcile server
reclassification, provider invoices, or ChatGPT subscription credits.
