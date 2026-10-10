# ADR 0019: Claude cost-state usage reconciliation

## Status

Accepted. Extends ADR 0018 with cumulative session/model usage.

## Context

Claude Code writes `cost-state.modelUsage` snapshots containing background API
usage that has no assistant transcript entry. Local Haiku 5.5 calls were present
only in these snapshots and were consequently absent from Tokenx. The snapshots
also include ordinary transcript usage, so adding them directly double counts
tokens. Claude Code restores the last `cost-state` object in a transcript.

## Decision

The decoder retains the last complete `cost-state` object in each input. Its
`sessionId`, `startTime` (Unix milliseconds), model keys, and four token counters
are required source facts. Malformed model entries are reported independently;
invalid snapshot metadata is reported without discarding valid transcript
usage. An invalid last snapshot does not revive an earlier snapshot.

Cost-free source shards retain a typed snapshot marker, the original model
labels, and the transcript's non-empty native `sessionId` when present. The
native identity scopes reconciliation even for renamed transcript copies;
inputs without it retain their decoder-established session scope. Existing
filename-based report grouping remains unchanged. The Claude fold collects
these totals by source session and model,
normalizing syntax such as `5-5`, `5.5`, and `[1m]` before reconciliation. User
model mappings are applied afterward, so merging two report identities cannot
erase their separately recorded usage. Mirrored inputs contribute per-bucket
maxima, not added snapshots; merged totals are validated before reconciliation.

After the existing cross-file and cross-batch transcript deduplication, the
fold subtracts accepted transcript counters from the corresponding snapshot
buckets. It emits only positive remainders alongside the original detailed
records. Detail exceeding a snapshot remains intact and contributes no extra
snapshot tokens. This is reconciliation of recorded totals, not token
estimation or recovery from failed authoritative input.

Snapshot `outputTokens` already includes `thinkingTokens`; it is counted once.
Snapshot-only usage has no request count or precise request timestamps: it
contributes zero messages and turns and is attributed to the recorded session
`startTime`. Detailed usage retains its actual timestamps and cache durations.
The snapshot has no cache-write duration split, so it provides no observed
one-hour subset. Tokenx applies its normal catalog/custom pricing to the
reconciled counters, rather than adding the snapshot's `costUSD`. Missing
catalog prices retain tokens under the existing pricing contract.

The in-memory state contains one total per snapshot session/model. Detailed
records remain in the existing temporary-file fold; transcripts are not reread.
Reconciliation runs identically for cold scans, shard hits, mixed acquisitions,
and repricing. Input shards advance to format 6 and generation caches to schema
9, invalidating previously incomplete results without a legacy migration.

## Consequences

Models used only for background calls become visible, including Haiku 5.5,
without relabeling Haiku 4.5 or counting repeated snapshots as new requests.
Snapshot-only usage has session-level time resolution; it cannot reconstruct
the distribution or number of individual background calls.
