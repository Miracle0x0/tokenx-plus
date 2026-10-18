# Claude Code transcript accounting

Verified on 2026-10-18 against local Claude Code 2.1.299–2.1.302 transcripts,
the [Claude Code changelog](https://github.com/anthropics/claude-code/blob/main/CHANGELOG.md#21300),
and the [Anthropic pricing reference](https://platform.claude.com/docs/en/about-claude/pricing).
The local comparison covered nine main sessions and eight nested subagent files.
Committed fixtures are synthetic and contain no local prompts or tool output.

## Recorded usage

Tokenx reads `~/.claude/projects/**/*.jsonl` and
`~/.claude/transcripts/**/*.jsonl`, including subagents. The accounting sources
are assistant response usage and the last `cost-state.modelUsage` snapshot.

| Source field | Meaning and Tokenx handling |
| --- | --- |
| `assistant.message.model` | Observed model identity, retained before canonicalization. `claude-opus-5-5` groups as `claude-opus-5.5`. |
| `message.id`, `requestId` | Composite response identity for streaming and cross-file deduplication. With only a message ID, that ID supplies the deduplication key. |
| `usage.input_tokens` | Ordinary, uncached input. An explicit input counter with either cache counter establishes split-input authority over bare prompt snapshots. |
| `usage.output_tokens` | Total output, including thinking tokens. |
| `usage.output_tokens_details.thinking_tokens` | A component of output; never added a second time. Tokenx does not currently expose a separate Claude reasoning bucket. |
| `usage.cache_read_input_tokens` | Input served from the prompt cache, counted and priced separately. |
| `usage.cache_creation_input_tokens` | Combined cache writes for both durations. |
| `usage.cache_creation.ephemeral_1h_input_tokens` | One-hour subset of combined writes, retained for duration-specific pricing. |
| `usage.cache_creation.ephemeral_5m_input_tokens` | Five-minute detail. Tokenx uses combined writes minus the bounded one-hour subset, rather than adding this field again. |
| `usage.service_tier`, `usage.speed`, `usage.inference_geo` | Samples reported `standard`, `standard`, and `global`. These fields are not currently applied as Claude pricing modifiers; nonstandard service or geography is outside this verified pricing scope. |
| `usage.server_tool_use` | Samples contained zero `web_search_requests` and `web_fetch_requests`. Server-tool request charges are not token buckets and are not included in Tokenx token pricing. |
| `usage.iterations`, `usage.fallback_credit` | Observed as `[]` and `null`; no additional accounting semantics are inferred. |

Streaming copies reconcile counters before pricing. The input convention
determines input authority; other counters use per-field maxima. Malformed or
negative usage remains an explicit input-health issue.

## Request replay records added in 2.1.300

Claude Code 2.1.300 added the system prompt, tools, and API request parameters to
local transcripts for request reproduction. These shapes were observed in
2.1.300–2.1.302 and describe requests, not additional completed usage.

| Record type | Observed fields | Accounting treatment |
| --- | --- | --- |
| `api-request-shape` | `sessionId`, `shapeHash`, `timestamp`, `version`, and `shape` containing `system` and `tools` | Does not emit usage. |
| `api-request-blob` | `sessionId`, `hash`, and a `message` containing `role` and `content` | Replay content; does not emit usage or start a human turn. |
| `api-request` | `sessionId`, `id`, `timestamp`, `version`, `querySource`, `shapeHash`, `params`, `keep`, `tail`, `cacheControl`, `requestDigest` | Request replay metadata; does not emit usage. |

Observed `params` include `model`, `betas`, `max_tokens`, `thinking`,
`context_management`, and `output_config`. `max_tokens` is an output limit,
not consumed tokens. `tail` contains blob references; `keep`, `cacheControl`,
and `requestDigest` accompany request reconstruction. Tokenx reads the
response's usage without reconstructing these requests.

User and tool-result content contributes no separate usage record. Tool-output
character counts and tool-local `token_count` or `input_tokens` metadata do not
establish another API request. When that content is sent to the model, its
consumption is already represented in response usage. Tool results may still
supply agent attribution, and genuine user prompts still mark turns.

## Cumulative snapshots and time attribution

`cost-state` carries a native `sessionId`, a Unix-millisecond `startTime`, and
`modelUsage` keyed by model. Each model's `inputTokens`, `outputTokens`,
`cacheReadInputTokens`, and `cacheCreationInputTokens` are cumulative counters.
`thinkingTokens` is included in `outputTokens`. `costUSD` and the top-level
`totalCostUSD` are not Tokenx pricing inputs.

Tokenx subtracts deduplicated assistant usage, including matching subagents,
from the final snapshot and adds only positive per-bucket remainders. This
retains background-only models such as Haiku without double counting.
Model spellings and `[1m]` context suffixes are reconciled before user model
mappings. See [ADR 0019](../adr/0019-claude-cost-state-usage.md).

Snapshot-only remainders have no request timestamp or cache-duration split.
They use session `startTime`, add no messages or turns, and contain no observed
one-hour write subset. Daily attribution and costs retain these limits even
when total tokens reconcile exactly.

## Catalog identity and pricing

LiteLLM publishes native Claude rows such as `claude-opus-5-5` with
`litellm_provider: "anthropic"`. Tokenx retains that declaration and treats the
row as provider-scoped. Public catalog model components normalize unsuffixed
Claude numeric versions: `claude-opus-5-5` and `claude-opus-5.5` identify the
same model. The raw catalog key remains available in lookup results. Provider
routes, dates, fast/batch variants, and private suffixes remain distinct.

Custom prices, configured catalog order, and missing-rate diagnostics keep
their authority. A selected catalog without an observed one-hour write rate
reports `cacheWrite1hUnavailable`; rates are not assembled from different
catalogs. See [pricing semantics](../pricing.md) and
[ADR 0023](../adr/0023-claude-recorded-usage-and-catalog-identity.md).
