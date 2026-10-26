# ADR 0031: Claude advisor usage reconciliation

## Status

Accepted. Extends ADR 0019, ADR 0023, and ADR 0026.

## Context

Claude assistant usage can include `iterations` whose `advisor_message` entries
record additional model calls. Parent counters include ordinary iterations but
exclude advisor calls. A later streaming duplicate can introduce or update those
calls, including several advisors using the same model.

## Decision

Decode each advisor's explicit model and token counters as additional usage,
using the parent request's session, timestamp, workspace, and billing evidence.
Ordinary iterations add nothing. The parent deduplication identity and original
iteration index identify each advisor across streaming updates and copied files.
Advisors add no message or human-turn count. Invalid advisors produce record
rejections without discarding valid parent or sibling usage.

Raw advisor records participate in the existing verified append checkpoint and
cross-file merge. Cache duration subsets remain unbounded until finalization,
as for parent usage. Detailed advisor counters subtract from the corresponding
session/model cost-state snapshot before positive remainders are emitted.

The source-derived Claude decoder fingerprint invalidates source shards, and
generation schema 15 invalidates aggregates without advisor details. No cache
migration or text-based usage estimation is introduced.

## Consequences

Advisor-only usage remains visible without requiring a cost-state snapshot.
Snapshots cannot double count detailed calls, and cold, warm, appended, and
mirrored transcripts retain identical accounting semantics.
