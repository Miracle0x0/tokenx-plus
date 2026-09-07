# Codex local token-usage facts

Last verified: 2026-09-07

## Cache-write source fields

Codex stores usage in JSONL `event_msg` records whose `payload.type` is
`token_count`. Both `payload.info.total_token_usage` and
`payload.info.last_token_usage` can contain `cache_write_input_tokens`.
Read-only inspection of nine local Codex CLI `0.153.4` session files found
469 such events. Both usage objects carried the field, always with value zero.
These observations establish field presence, not observed nonzero writes.
Nonzero regression fixtures are synthetic.

The upstream [cache-write tracking commit](https://github.com/openai/codex/commit/2edad72de3e4fb12a7519027d5eb3cbda45eea6c)
maps Responses `input_tokens_details.cache_write_tokens` to
`TokenUsage.cache_write_input_tokens`. Its protocol definition gives the field
`#[serde(default)]`, and cumulative usage adds it alongside the other counters.
An omitted field therefore represents zero. There is no verified Codex rollout
contract for `cache_creation_input_tokens` or `cache_write_tokens` as aliases at
this location.

## Input accounting

The [OpenAI prompt-caching guide](https://developers.openai.com/api/docs/guides/prompt-caching)
defines ordinary input as inclusive input minus cache reads and cache writes.
Tokenx applies this mapping to each accepted `last_token_usage`:

```text
cache_read  = max(cached_input_tokens, cache_read_input_tokens)
cache_write = cache_write_input_tokens
input       = input_tokens - cache_read - cache_write
```

Missing optional buckets are zero. The maximum of the two cache-read fields
preserves Tokenx's existing read-field reconciliation. Negative buckets,
cache reads plus writes exceeding input, and overflowing counter sums are
rejected under the existing malformed-record diagnostic. Invalid counts are
never clamped into valid usage. Output and reasoning accounting is unchanged.

Tokenx uses `last_token_usage` as the request increment. Cumulative snapshots
participate in duplicate, regression/reset, and inherited fork-history checks;
they are not a direct billing delta because compaction can rewrite them. Cache
writes participate in those checks and in cross-file deduplication. A record
containing only cache-write usage remains eligible.

## Pricing and cached parsing

Cache-write volume is priced through the existing
`cache_creation_input_token_cost` catalog field. The local LiteLLM cache at
verification included this rate for `gpt-5.6-sol` ($5 per million tokens),
`gpt-5.6-terra`, `gpt-5.6-luna`, and `gpt-6-astra`. These are observations of the
local catalog, not fixed Tokenx prices; [pricing authority](../pricing.md)
continues to determine the selected rate.

The source-derived Codex decoder contract includes `codex/decode.rs`. Changing
it invalidates earlier input-record shards, including incremental parse state,
and changes the next acquisition's source fingerprint. Acquisition reparses
authoritative sessions without requiring manual cache deletion.
