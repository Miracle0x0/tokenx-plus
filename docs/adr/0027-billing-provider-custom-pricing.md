# ADR 0027: Billing-provider custom pricing

## Status

Accepted. Extends ADR 0004, ADR 0012, and ADR 0013.

## Context

Provider attribution can be inferred from a model or normalized to its vendor.
It cannot distinguish the prices charged by different recorded routing channels.
Global model overrides therefore cannot represent channel-specific rates.

## Decision

Usage retains separate billing evidence: unknown, an observed channel label,
or conflicting observations. Only explicit record/session channel fields and
Zed's verified hosted ownership establish it. Model prefixes, inferred vendors,
transport protocol names, and current configuration do not. Labels are trimmed
and ASCII-lowercased as whole strings, without provider aliases or prefix
splitting. Blank and `unknown` labels do not match a scope.

`custom-pricing.json` supports `provider_overrides`, an array of
`provider_id`, `model_id`, and `pricing` objects. Exact scoped rows precede global
`models` rows, followed by the existing public-catalog authority. All custom
model keys match the final canonical model after configured mappings. There is
no raw-label precedence or compatibility alias lookup. Forced public sources
bypass all custom overrides.

A scoped row is selected whole. A used bucket without a rate keeps its tokens,
excludes that record's cost, and emits `billingRateUnavailable`; no global or
public row fills the gap. Explicit zero rates are valid, including globally.
Malformed scoped entries, null rates, unknown price fields, and conflicting unit
forms produce configuration diagnostics. Every entry in a duplicated normalized
provider/model scope is disabled, independent of input order. Global duplicate
keys retain their existing diagnosed last-valid-entry authority.

Billing identity is preserved in cost-free shards and incremental checkpoints.
Claude duplicate reconciliation combines missing and observed evidence; unequal
observations become conflicted and cannot be revived by later copies. Unknown
or conflicted identity can use global prices but never a scoped rule. Pricing
changes reuse unchanged cacheable input records without changing model grouping
or decoding those inputs again. Provider-reported money remains excluded.

`pricing lookup --billing-provider` selects a scope, and `pricing overrides`
lists it explicitly. A custom-only lookup stays offline. The input-shard and
generation formats advance without legacy migration, rejecting earlier cached
estimates and enum layouts. Missing lookup rates serialize as null rather than
a fabricated zero. Roo's per-message `modelInfo` keeps model and billing evidence
paired; absent task metadata does not erase self-contained
rows, while unavailable metadata remains an explicit error for dependent usage.

## Consequences

The same canonical model can carry different estimated prices by recorded
channel while remaining one model group. A label identifies log evidence, not
a verified merchant, account, endpoint, or invoice. Inputs without channel
evidence remain unscoped rather than guessing a billing route.
