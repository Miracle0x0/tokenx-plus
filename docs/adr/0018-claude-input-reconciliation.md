# ADR 0018: Claude input reconciliation before pricing

## Status

Accepted. Refines Claude's fold under ADR 0003 and advances the cache formats
from ADR 0017.

## Context

Claude can repeat one message/request ID with two different input conventions:
a bare prompt snapshot and a usage object that separates ordinary input from
cache reads and writes. Taking the maximum input across those conventions
counts cached tokens again as ordinary input. The records can occur in either
order, in separate files, or in a mixture of parsed inputs and cached shards.
Emitting the first copy immediately prevents later corrections before pricing.

## Decision

A usage object with an explicit `input_tokens` and at least one of
`cache_read_input_tokens` or `cache_creation_input_tokens` reports split input.
Presence establishes this authority even when a counter is zero. Missing input
does not assert zero and does not confer split-input authority on a previously
stored snapshot. Split input supersedes snapshot input. When both observations
have the same convention, their maximum wins. Other token buckets retain their
per-field maxima. Negative and malformed observations remain rejected under
ADR 0001.

Records retain the split-input provenance in cost-free source shards. The
Claude fold merges matching existing dedup keys across every input and batch
before canonicalization, pricing, and aggregation. Cross-file merges preserve
the first record's identity, timestamp, attribution, and emission order; only
token counters and input provenance are reconciled. Unkeyed records remain
independent. Merged totals are revalidated; merge or pricing rejections belong
to the first input that owns the record.

An anonymous private temporary file holds decoded usage records during this
fold. A fixed-size token/provenance prefix can be updated in place, while the
in-memory index holds only dedup keys and file offsets. Replay restores one
input's records at a time for pricing and emission. It never rereads client
transcripts or retains an integration-wide record vector. The temporary file
is deleted on close, and I/O or decoding failures are explicit pipeline errors.
This staging is independent of source caching, so partial inputs retain their
confirmed usage without becoming cacheable.

Input shards advance to format 5 for provenance. Generation caches advance to
schema 8 to discard totals computed using first-copy deduplication. Decoder
source contracts invalidate prior parse semantics. Old layouts are rejected;
there is no legacy migration.

## Consequences

Cold scans, cache hits, mixed acquisitions, and repricing use the same input
authority. The fold adds temporary disk I/O and an offset per distinct key,
instead of retaining all Claude records in memory or pricing incomplete copies.
