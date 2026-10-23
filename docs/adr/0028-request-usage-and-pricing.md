# ADR 0028: Disjoint usage buckets and request-level pricing

## Status

Accepted. Extends ADR 0004 and ADR 0014.

## Context

Qwen and Grok report inclusive token counters. Adding their detail again inflates totals and catalog estimates. Grok completions can also combine multiple requests, which do not establish the prompt length of any individual call.

## Decision

Qwen splits cache reads from the prompt when they are a valid subset. When `totalTokenCount` equals prompt plus candidates, reasoning is detail within candidates; otherwise the recorded thoughts remain additive. Oversized reasoning detail is bounded by the authoritative candidates in the inclusive case. Negative and overflowing counters are rejected. An oversized cache count is not treated as a prompt subset.

Grok `turn_completed.usage` supplies inclusive input/output counters, optional cache and reasoning detail, call counts, and per-model totals. Per-model rows preserve model identity and must reconcile with reported turn totals. Cache subsets must fit the prompt; reasoning is bounded by output. Money fields do not participate. A durable prompt ID identifies retransmitted snapshots; the greatest recorded total wins, with the later snapshot winning ties. Without a prompt ID, separate completion records remain separate. A completed turn supersedes cumulative allocation for that user turn only. Other cumulative-only turns retain ADR 0004's allocation. Malformed authoritative completion usage is diagnosed rather than replaced with imputed usage.

The ordinary, priority, and flex paths use the same inclusive-prompt 272k request rule. Explicit xAI 200k rates likewise apply to the whole request at their boundary. No rate multiplier or cross-source supplementation is inferred.

Grok records carry aggregate scope unless an explicit call count is one. An aggregate above the applicable request boundary lacks the per-call evidence needed to price it. It retains tokens, excludes cost, and emits `requestTierUnavailable`, including after cache reuse. This deliberately differs from Tokscale's base-rate approximation for multi-call rows. The input-shard and generation wire formats advance; decoder source fingerprints invalidate the changed parsing rules.

## Consequences

Disjoint token totals remain exact, request-wide prices are consistent across service tiers, and unresolvable aggregate pricing is visible. Retaining aggregate scope costs one field per source row and avoids inventing per-request distributions.
