# Pricing semantics

Tokenx pricing estimates what parsed token buckets would cost under the
configured pricing catalog. It is not an invoice reconciler.

## Local usage cost

For local usage, parsers emit token usage. App or vendor fields such as
`cost`, `credits`, `cost_usd`, `dollar_float`, `spendCents`,
`estimated_cost_usd`, `actual_cost_usd`, and `usage.cost.total` are ignored.

`AttributedUsageRecord.cost` is derived by applying Tokenx pricing to these token
buckets:

- input tokens
- output tokens
- cache read tokens
- cache write or cache creation tokens
- reasoning tokens

Codex includes cache reads and writes in `input_tokens`. Tokenx subtracts
`cached_input_tokens` (or `cache_read_input_tokens`) and
`cache_write_input_tokens` from that inclusive input count, then prices the three
buckets separately. Cache writes use the catalog's
`cache_creation_input_token_cost`, including custom overrides. An absent
`cache_write_input_tokens` means zero; no write volume or model-specific write
rate is inferred. See [Codex token usage](facts/codex.md).

Rows without positive token buckets are not usage rows. Cost-only or
credits-only records are dropped instead of being converted into local token
cost.

Total-only usage records with accepted client attribution use the fixed bucket
allocation from [ADR 0004](adr/0004-period-views-derive-from-daily.md). Their
derived cost is approximate because the recorded total is projected into buckets
before pricing.

See [ADR 0004](adr/0004-period-views-derive-from-daily.md).

## Pricing Source authority

Exact custom overrides from `custom-pricing.json` are checked first. For a
canonical `deepseek-v4` family model, an exact OpenRouter row carrying
time-period prices is checked before the general public-catalog order. Otherwise,
Tokenx searches public catalogs in the configured order, whose default is
LiteLLM, OpenRouter, and models.dev. A forced
`--pricing-source` limits lookup to one catalog. Public lookup receives the
canonical model component without a provider or route prefix and considers only
catalog rows with that exact component.

The public-catalog order can be changed with `pricingSourceOrder` in
`settings.json`, or from the TUI pricing-source order panel (`o`). The setting
must contain all three public catalogs exactly once. Changing it invalidates the
Generation pricing identity. Valid input-record shards for unchanged inputs are
reused and their records are re-priced and re-aggregated; raw transcript parsing
is not repeated for those cache hits. The DeepSeek V4
OpenRouter time-period rule remains ahead of this configurable order.

A non-empty observed provider, or otherwise shared deterministic model-family
inference, defines provider scope. Exact rows for a known provider are selected
before exact unscoped rows across all catalogs; catalog order breaks ties inside
each class. With unknown provider scope, only an exact unscoped row is eligible.
Prefix, substring, fuzzy/edit-distance, arbitrary separator, and private alias
matching are not pricing strategies.

### Codex service-tier pricing

Codex `event_msg/thread_settings_applied` snapshots supply
`payload.thread_settings.service_tier`. The setting applies to subsequent usage
records and survives cached and incremental parsing. A later setting does not
reprice earlier records; inherited fork settings do not configure the child.
Missing or null tier metadata uses the model's ordinary catalog estimate.
`default` uses ordinary rates, `fast` and `priority` select the catalog's
`_priority` rates, and `flex` selects `_flex` rates. Model identity and grouping
remain independent of tier.

LiteLLM's per-token input, output, cache-read, and cache-creation tier fields are
retained, including `_above_272k_tokens_priority` and
`_above_272k_tokens_flex`. When long-context pricing is present, tier selection
uses inclusive prompt size (ordinary input plus cache reads and writes). Above
272,000 prompt tokens, the long-context rates apply to the whole request.
No fixed multiplier is inferred. Custom rows can provide the same per-token
suffix fields; for example, `input_cost_per_token_priority`. Custom overrides
and configured catalog order keep their existing authority: rates are not
assembled from different sources.

An unsupported explicit tier or a missing/invalid rate for a used token bucket
leaves that record's tokens intact but excludes its cost. The generation reports
`serviceTierUnavailable` with the model, tier, and affected record count, and
pricing status becomes `availableWithWarnings` when catalogs are otherwise
available. These usage-derived diagnostics survive generation-cache reuse.

The recorded setting is evidence of requested service, not a server-confirmed
billing tier. [OpenAI Fast mode](https://developers.openai.com/api/docs/guides/fast-mode)
can serve a request at the standard tier; the inspected Codex token events do
not record that response tier. Local costs remain catalog estimates, separate
from invoices and [ChatGPT credit consumption](https://learn.chatgpt.com/docs/agent-configuration/speed).

### Claude Code cache durations

Claude Code records one-hour cache creation in
`usage.cache_creation.ephemeral_1h_input_tokens`. Tokenx retains it as a subset
of `cache_creation_input_tokens`: ordinary cache writes are the total minus
the one-hour portion, and token totals count the combined write volume once.
An absent split means zero observed one-hour writes. Duplicate streaming
records merge each raw counter by maximum before bounding the one-hour subset
by the final cache-write total.

Ordinary writes use `cache_creation_input_token_cost`; one-hour writes use
`cache_creation_input_token_cost_above_1hr`, including the optional
`cache_creation_input_token_cost_above_1hr_above_200k_tokens` tier. The existing
per-bucket tier calculation applies separately to each duration. Rates come
from the selected catalog row, with no inferred multiplier or supplementation
from another source. Anthropic publishes different rates for the two durations;
see [prompt-cache pricing](https://platform.claude.com/docs/en/build-with-claude/prompt-caching#pricing).

An observed one-hour write without a valid one-hour rate retains its tokens
but excludes that record's cost. The generation reports
`cacheWrite1hUnavailable` and `availableWithWarnings` when catalogs are
otherwise available. Explicit zero rates are valid. These diagnostics survive
generation-cache reuse; repricing uses the duration evidence in cost-free input
shards. See [ADR 0017](adr/0017-claude-code-cache-duration-pricing.md).

### DeepSeek V4 time-period pricing

OpenRouter time-period prices use the usage record's request timestamp in UTC.
`utc_start` and `utc_end` are `HHMM` clock values rather than minute offsets.
The start is inclusive, the end is exclusive, and a start later than the end
wraps across midnight. `utc_days` is evaluated from the UTC weekday at the
request instant; an entry with days but no clock window applies for those whole
UTC days.

All matching entries are applied in source order. Later entries replace only
the price fields they contain, while omitted price fields retain the base rate
or an earlier matching value. A missing timestamp or no matching time entry uses
the model's base price. Time-period selection is currently limited to the
`deepseek-v4` family; token accounting and all non-DeepSeek V4 pricing behavior
remain unchanged.

### Model identity before pricing

Tokenx resolves model identity before aggregation and pricing. The optional
[`model-mappings.toml`](configuration.md#model-mappings) supplies ordered exact
or `*` wildcard rules ahead of the bundled aliases. Both local usage and
standalone `pricing lookup` use the final mapped name. A mapping therefore
changes the price key as well as the aggregation and display name; it does not
retain separate prices for the merged input names.

Parsers and cost-free input shards retain raw model observations. Finalization
applies the captured mapping rules once, then prices the final model identity.
Matched targets are never recursively mapped or normalized again by the price
resolver. A missing mapping file enables the bundled defaults, while an invalid
file is a configuration error.

The pricing resolver is therefore not a route cleanup layer. It receives the
final canonical usage model id and matches that id against custom overrides
and public catalog rows.

If no pricing match exists, derived cost stays `$0.00`. The unresolved model id
should remain visible so the missing catalog entry can be fixed explicitly.

See [ADR 0004](adr/0004-period-views-derive-from-daily.md).

## Custom pricing overrides

Create `custom-pricing.json` in the Tokenx config directory:

```json
{
  "models": {
    "kimi-k2.6": {
      "input_cost_per_million_tokens": 2.0,
      "output_cost_per_million_tokens": 8.0,
      "cache_read_input_token_cost_per_million_tokens": 0.3,
      "pricingSource": "https://docs.fireworks.ai/serverless/pricing",
      "notes": "Kimi K2.6 local usage override"
    }
  }
}
```

Per-million-token fields are the recommended user-facing form. At least one of
`input_cost_per_million_tokens` or `output_cost_per_million_tokens` must be
present and positive. Cache-read and cache-creation prices are optional.
One-hour cache creation accepts
`cache_creation_input_token_cost_per_million_tokens_above_1hr` and
`cache_creation_input_token_cost_per_million_tokens_above_1hr_above_200k_tokens`,
or the corresponding per-token fields without `_per_million_tokens`.

Overrides are exact-only and case-insensitive:

- Local usage matches the canonical model id after model canonicalization, not
  necessarily the raw observed label emitted by a client or parser.
- Key each local usage override by that final canonical id.
- `tokenx pricing lookup <model>` matches the command argument as a catalog query.

Restart the command after editing the file because overrides are loaded at
startup.

## Cache files

Pricing data is cached under `${TOKENX_CONFIG_DIR}/cache/`:

- `pricing-litellm.json`
- `pricing-openrouter.json`
- `pricing-models-dev.json`

Pricing cache schema 3 stores `version` and `data`; file modification time
determines the one-hour freshness window. Older formats are refreshed.
Identical fetched data renews only the file timestamp; changed data replaces
the cache atomically. Deleting these files forces a fetch on the next lookup
or usage load that needs pricing.

Input-record shards are cost-free: they retain token buckets, timestamps, and
model/provider identity, observed service tier, and cache-duration evidence,
but not derived prices. The Generation cache contains aggregated costs and is
invalidated when the pricing
context, including source order, changes.

Headless usage commands refresh missing or expired public catalogs before
building their generation. The TUI enters immediately from its captured local
snapshot and performs the same refresh in its supervised background acquisition
lifecycle, reusing already loaded fresh catalogs while their file identities,
sizes, and modification times match. If refreshed catalog identity changes, the TUI installs a newly
priced generation when the background build completes; the existing generation
remains visible during a warm refresh.

If refresh fails, readable older catalogs remain usable and pricing status
becomes `cachedFallback`. A partial catalog set or custom-only pricing remains
usable with `availableWithWarnings`; it is never reported as complete public
pricing. Successfully fetched data remains active for the current acquisition
when cache persistence fails; the write failure is reported as a warning
instead of replacing fresh rates with missing or stale disk state.

## Standalone lookup

```bash
tokenx pricing lookup claude-sonnet-4-5 --no-spinner
tokenx pricing lookup grok-code --pricing-source openrouter --no-spinner
tokenx pricing overrides --json
```

Standalone lookup does not infer arbitrary observed-model prefixes, route
prefixes, private aliases, or reasoning-tier suffixes. It is a pricing catalog
query over the exact canonical model component, not a parser repair path. When
the matched row carries time-period pricing, text output lists the UTC schedule
and JSON output includes `pricing.timePeriodPrices`.
One-hour cache-write rates appear in text output and JSON's
`pricing.cacheCreationInputTokenCostAbove1hr`, with the optional long-context
rate in `pricing.cacheCreationInputTokenCostAbove1hrAbove200kTokens`.

## Subscription usage is separate

The TUI Subscription tab calls provider-specific quota endpoints and shows what the
provider reports. Those numbers are not mixed into normal local token reports.
