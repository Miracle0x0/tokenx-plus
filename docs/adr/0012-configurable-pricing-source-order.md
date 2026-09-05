# ADR 0012: Configurable public pricing-source order

## Status

Accepted

## Context

Tokenx has three public pricing catalogs. Their fixed lookup order made a
catalog choice impossible to tune when sources disagree. Input-record shards
already retain token buckets, timestamps, and model/provider identity without
derived cost, while the canonical Generation stores aggregate costs.

## Decision

`settings.json` stores a complete permutation of `litellm`, `openrouter`, and
`models.dev` under `pricingSourceOrder`. Custom pricing overrides remain the
highest authority. For DeepSeek V4, an OpenRouter row with time-period pricing
remains ahead of the configured public order. Provider-scoped exact matches
still take precedence over unscoped matches; the configured order breaks ties
between catalogs at the same match level.

The order is part of `PricingContext` and therefore Generation-cache identity.
Changing it reuses valid cost-free input-record shards and rebuilds canonical
aggregation with the new pricing service. It does not reparse unchanged raw
transcripts when those shards are valid, or redownload catalogs solely because
their order changed; normal metadata discovery and cache validation still occur.

The TUI exposes the setting through its pricing-source order dialog. Applying
the dialog persists the typed setting and schedules a background Generation
rebuild. Until that rebuild succeeds, the installed Generation remains visible;
the transition reports its requested state and any rebuild failure explicitly.

## Consequences

Pricing results are consistent across TUI, Models, cache warm, and standalone
pricing lookup commands. A source-order change still costs a full aggregation
pass over cached records because tiered and time-period prices cannot in general
be derived exactly from model-level token totals alone. The Generation cache
schema is versioned so an older cache cannot be interpreted under the new
pricing identity.
