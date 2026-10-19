# ADR 0024: Copilot cross-file usage snapshots

## Status

Accepted. Refines ADR 0001 and ADR 0003.

## Context

Copilot OTEL exports can repeat one response across files or record increasingly
complete snapshots. Summing those records counts the same request repeatedly.
Per-file filtering cannot reconcile snapshots across acquisition batches.

## Decision

The Copilot integration merges snapshots before pricing and aggregation. A
trace plus span, or a trace plus explicit turn index, identifies a response;
sparse identities are scoped to their input path. Conflicting observed model,
provider, timestamp, session, agent, or workspace evidence stays separate.
Missing session and agent attribution can be enriched only when the original
snapshots establish one unambiguous owner. Display defaults are not evidence.

Token buckets use maxima across snapshots. Input is reconciled inclusively
with cache reads before restoring the exclusive input bucket. The merged
record is validated and priced once; impossible merged totals remain explicit
record errors. Source shards retain unmerged records and attribution evidence,
so cache hits and fresh input use the same reconciliation. The shard format is
advanced without compatibility decoding.

An anonymous temporary spool holds payloads while an in-memory key/offset
index spans batches. Only one response group is materialized during merging.
Spool failures abort acquisition explicitly. The client-bound fold cannot
merge records from another integration.

## Consequences

Repeated exports no longer inflate usage or cost. Temporary disk I/O and a
response index replace an integration-wide in-memory record collection.
