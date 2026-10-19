# ADR 0026: Claude verified append parsing

## Status

Accepted. Extends ADR 0003 and ADR 0019.

## Context

Growing Claude transcripts were decoded from byte zero on every changed input.
Resuming only from the last byte offset is insufficient: later streaming rows
can update earlier responses, workspace evidence can reclassify earlier rows,
and a newer cost-state snapshot replaces the previous one completely.

## Decision

A complete newline-terminated scan may store a checkpoint with its consumed
offset, full prefix SHA-256, parser state, provider evidence, and last decoded
cost-state snapshot. The record body stores raw per-file merged observations
before zero-token filtering, cache-duration bounding, workspace resolution, and
cross-file cost-state reconciliation. It is the only full record collection;
the checkpoint does not duplicate it. Snapshot state stores decoded accounting
facts, not source JSON or tool/request payloads.

Append reuse requires the same native file identity, an increasing size, and
unchanged related-input stamps. After authenticating the cached body and checking
its record/state relationship, hash the entire historical prefix and compare it
with the checkpoint. Only the appended tail is JSON-decoded; the same hasher
continues across it. A prefix rewrite, truncation, replacement, or related-input
change requires a complete parse of current authoritative input.

A checkpoint also retains resolved workspace context, so an exact cache hit
needs no transcript or parent-metadata read during record reuse. Unterminated complete files may
cache their finalized records, but cannot resume an append. Partial scans are
never cached. Corrupt checkpoints and record bodies trigger visible cache
diagnostics and authoritative reparsing. No missing source revives old usage.

The fold consumes cached bodies and resumes appended transcripts there, allowing
streaming updates to mutate those records without a second full copy. Cold
misses retain parallel parsing. Finalization and cross-file reconciliation run
after raw checkpoint publication and derive the same result as a full scan.
External workspace metadata still disables caching; parent/sidecar dependencies
retain their existing fingerprint rules. The input-shard format advances with
no compatibility decoding.

## Consequences

Growing sessions avoid decoding their historical JSON again, while retaining
the prefix I/O needed to prove append identity. Checkpoint metadata and a
per-file dedup index add memory proportional to the current transcript's rows;
acquisition still consumes input batches and keeps one record copy per input.
