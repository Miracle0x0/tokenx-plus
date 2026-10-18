# ADR 0023: Claude recorded usage and catalog identity

## Status

Accepted. Refines ADR 0001, ADR 0012, and ADR 0019.

## Context

Recent Claude Code transcripts preserve assistant usage and cumulative
`cost-state` counters while adding request replay records. Tokenx also estimated
tool-output characters as separate input records. Those duplicated input
already represented in API usage, and positive-remainder snapshot
reconciliation could not remove the excess.

Usage canonicalization produces `claude-opus-5.5`, while LiteLLM publishes its
native price as `claude-opus-5-5`. Exact raw-key comparison missed this row,
and discarding `litellm_provider` lost its provider scope. OpenRouter's scoped
row then won but lacked the one-hour cache-write price, excluding whole response
costs under ADR 0017 despite complete native prices.

## Decision

Claude API accounting accepts assistant usage and cumulative cost-state
snapshots. User/tool-result payloads, including tool-local token metadata, do
not establish separate billable requests. Remove tool-output estimation and
its model-inheritance and deduplication state. Request replay records are
non-usage metadata. Turn detection and agent attribution remain supported.
Missing API usage is not reconstructed from text length.

Public catalog lookup indexes canonical model components while preserving raw
keys for diagnostics. The new normalization is narrow: unsuffixed modern
Claude family/major/minor names accept `5-5` and `5.5` equivalently. Dates,
route encodings, fast/batch labels, private suffixes, and non-Claude names
remain exact. User model mappings never rewrite catalog rows.

Retain LiteLLM's `litellm_provider` field. For its unprefixed `claude-*` rows,
an explicit `anthropic` declaration establishes Anthropic provider scope.
Rows without that evidence remain unscoped; prefixed rows retain their route
scope. Other catalogs do not acquire authority from LiteLLM metadata.
Provider-scoped matches still precede unscoped matches, with configured source
order breaking ties. Custom overrides and forced sources keep their authority.
Incomplete selected rates remain visible errors; no rate borrowing is added.

Pricing-cache schema 4 invalidates rows that omitted the provider declaration.
Generation schema 11 invalidates extra tool-input totals and omitted costs.
The generated Claude decoder fingerprint invalidates old source shards; their
wire format remains 6. No cache migration is provided.

## Consequences

Recent native Claude usage resolves complete LiteLLM prices under the default
order. Tool results stop inflating totals, including after shard and generation
cache reuse. Snapshot-only remainders still lack request timing and cache
duration. Nonstandard Claude speed/geography modifiers and server-tool charges
remain outside the verified accounting scope.
