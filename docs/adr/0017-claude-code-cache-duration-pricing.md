# ADR 0017: Claude Code cache-duration pricing

## Status

Accepted. Advances the cache versions from ADR 0014 and ADR 0016.

## Context

Claude Code transcripts report one-hour cache creation separately inside
`usage.cache_creation`, while `cache_creation_input_tokens` already includes
both durations. Discarding the split prices every write at the ordinary
five-minute rate. LiteLLM publishes separate one-hour rates, but other catalogs
and custom overrides can omit them.

## Decision

Source records retain `cache_write_1h` as a subset of `cache_write`. Totals do
not add it a second time. An absent nested counter means no observed one-hour
write. Claude's duplicate assistant records merge raw per-field maxima and
bound the subset by the final total once, after merging. Negative counters
remain record errors; malformed wire shapes retain the decoder's explicit
partial-input failure behavior.

Pricing splits ordinary and one-hour writes per record. One-hour writes use
`cache_creation_input_token_cost_above_1hr` and its optional
`_above_200k_tokens` tier; existing marginal per-bucket tier semantics apply
independently to each duration. Custom overrides accept both per-token and
per-million spellings.

Custom and catalog authority remain as specified in ADR 0012 and ADR 0014.
Tokenx does not borrow missing rates from another catalog, infer a multiplier,
or substitute the five-minute price. Missing or invalid one-hour rates retain
the record's tokens and exclude its cost, with a usage-derived
`cacheWrite1hUnavailable` diagnostic. A published zero rate is valid. The
diagnostic survives generation-cache rebinding and disappears when a later
selected pricing row can price that usage.

Input-record shard format 4 persists the duration subset. The generated Claude
decoder fingerprint invalidates previous parse semantics. Pricing-cache schema
3 rejects catalogs that previously discarded one-hour rates. Generation schema
7 rebuilds previously derived costs. Old layouts are rejected through the
existing cache diagnostics and acquisition lifecycle; no legacy migration is
introduced. Aggregated report token fields retain their combined write volume.

## Consequences

Cold scans, valid shard hits, and catalog repricing use identical recorded
duration evidence. Mixed-duration costs can be estimated accurately when the
selected row publishes both rates. Incomplete rows expose unpriced records
instead of reporting a lower five-minute estimate as a complete price.
