# ADR 0013: Configurable model mappings

## Status

Accepted.

## Context

Model aliases previously lived in Rust branches. Users could configure prices
but could not choose which observed model names were merged. Early aliasing in
decoders and repeated normalization in pricing also erased distinctions before
a user rule could take effect.

## Decision

`model-mappings.toml` is an optional file under the Tokenx product root. The
`config init-model-mappings` command creates an editable template with bundled
defaults shown as TOML comments. The command does not replace existing files.
Absence means the bundled defaults; invalid or unreadable input is an explicit
configuration error.

The engine maintains the default alias data in
`crates/tokenx-engine/model-mappings.toml`. User `[[rules]]` entries contain
`pattern` and `model`. Patterns match the entire string, ignore ASCII case, and
support only `*` as a wildcard. The first matching rule wins, with user rules
before defaults. `include_defaults = false` removes bundled aliases from the
effective list.

Each rule sees the raw observation, the terminal component after route/custom
prefix cleanup, the syntax-normalized spelling, and a human display label
converted to hyphen-separated words. Existing syntax rules for
dates, free-channel tags, reasoning tiers, and version spelling remain engine
mechanisms. Explicit user self-maps can preserve a source spelling. A target is
final: there is no mapping chain and no second normalization in pricing.

The mapped identity is authoritative for aggregation, display, sessions, model
ranking, and exact pricing lookup. Standalone pricing lookup uses the same
rules. Other grouping dimensions and provider attribution retain their owning
contracts. A missing price does not discard token usage.

Startup captures the optional file once. AcquisitionConfig contains the full
resolved rule list, including the active defaults, so either user or bundled
rule changes invalidate aggregate Generation identity. TUI refresh and pricing
source changes retain the captured mappings; projections remain pure.

Decoders defer general alias mapping to finalization. UsageRecord and its
cost-free shard representation retain the raw model observation alongside any
client-specific decoded spelling. Remapping unchanged inputs reuses those
shards and recomputes prices, aggregate buckets, and sessions together. Shard
and Generation envelopes use new versions for the changed wire types.

The default rules include `deepseek-v4.1-*` and `deepseek-flash`, both targeting
`deepseek-v4.1-flash`, as well as the existing named aliases.

## Consequences

Users can add aliases, override individual defaults, or replace the default
alias list without rebuilding Tokenx. Merging names also selects a shared price
key. Ordered first-match rules are deterministic; users place exceptions before
broad patterns. Changes take effect on the next command invocation.
