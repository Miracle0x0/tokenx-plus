# ADR 0040: LiteLLM GPT provider scope

## Status

Accepted. Extends ADR 0012 and ADR 0023.

## Context

OpenAI usage could select the LiteLLM row `gmi/openai/gpt-6-astra` because
its path contained `openai`. The native `gpt-6-astra` row declared
`litellm_provider: "openai"` and published priority rates, but lookup treated
it as unscoped. The selected reseller row lacked those rates, leaving
priority usage without a cost estimate.

## Decision

Treat an unprefixed LiteLLM GPT row declaring OpenAI as provider-scoped.
For prefixed LiteLLM GPT rows the outer route component identifies the
provider. Resolve a qualified GPT usage provider from its outer component
before normalizing provider identity for report grouping. Within the same
catalog and provider, use a usable declared native
row before routed rows. A selected row supplies all rates under ADR 0014.
Configured catalog order continues to break ties between scoped catalogs.

Advance the pricing catalog fingerprint so cached generations are repriced
from the existing catalogs and cost-free input shards. A reseller with no
matching tariff can consequently have less pricing coverage.
