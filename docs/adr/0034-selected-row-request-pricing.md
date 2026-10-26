# ADR 0034: Selected-row request pricing

## Status

Accepted. Extends ADR 0012, ADR 0014, ADR 0017, and ADR 0028.

## Context

Public catalogs publish Haiku 5.5 rates above 100k prompt tokens, while earlier
schemas discarded those fields. Some first-party OpenAI rows omit cache-write
rates for models whose writes have the ordinary input tariff.

## Decision

Retain 100k input, output, cache-read, cache-write, and one-hour-write rates from
LiteLLM, OpenRouter overrides, and models.dev context tiers where published.
Custom pricing accepts the same per-token and per-million fields. Missing or
invalid required rates remain explicit pricing failures, never borrowed from
another source. Custom rows without an explicit 100k card retain their flat rate.

For canonical Haiku 5.5, prompt size includes ordinary input plus cache reads and
writes. A prompt strictly above 100,000 tokens selects the high card for the
whole request, including output and each observed cache duration. Aggregates
above the boundary cannot establish individual request prices and retain tokens
without a cost estimate. Diagnostics name the missing bucket or request boundary.

For the enumerated pre-5.6 GPT-5 models, including dated snapshots, missing
cache-write fields in a selected native OpenAI LiteLLM/OpenRouter row inherit
that same row's matching input rate. This includes 272k, priority, and flex
fields. Explicit write rates, including zero, remain authoritative. Reseller
routes, other model families, custom overrides, and models.dev are excluded.
No write volume or rate multiplier is inferred.

These rules follow [Anthropic's request pricing](https://platform.claude.com/docs/en/about-claude/pricing#long-context-pricing)
and [OpenAI's cache pricing](https://developers.openai.com/api/docs/guides/prompt-caching),
verified on 2026-10-25. Prices themselves still come from the selected catalog.
Pricing-cache schema 5 retains the new fields; generation schema 18 recomputes
derived costs. No cache migration is provided.

## Consequences

Long Haiku requests and observed pre-5.6 cache writes are priced from explicit
provider rules without mixing catalogs. Missing duration or long-context rates
cannot turn into an apparently complete cheaper estimate.
