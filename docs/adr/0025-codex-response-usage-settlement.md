# ADR 0025: Codex response usage settlement

## Status

Accepted. Extends ADR 0003 and ADR 0014.

## Context

Codex writes per-response `token_usage_record` entries before the corresponding
`token_count`. Compaction can leave a response record followed by `compacted`
and a non-advancing counter; aborted or completed turns can lack the counter
entirely. Ignoring response records loses that recorded usage.

## Decision

Keep a validated response pending until its counter confirms it. An adjacent
ordinary counter remains authoritative, including differing cache detail.
After `compacted`, only matching input/output counts confirm the response;
a counter that emits nothing or describes another response settles compaction
separately. Another response, a new turn, or a matching `task_complete` or
`turn_aborted` also settles pending usage. EOF alone does not settle an active
response because a later append can contain its counter.

Pending records capture model, timestamp, provider, request tier, workspace,
session, and turn evidence when read. Request-level service tier takes priority
over the thread setting for that response alone. Required model and timestamp
failures remain explicit record errors. No filesystem timestamp or invented
model substitutes for them. Existing strict counter shape and token validation
remain unchanged.

Response IDs identify repeated response records; records without an ID use
their timestamp and full token signature within the upstream session scope.
Foreign-thread usage and inherited fork history remain excluded. Settled
signatures recognize late counters without counting them again; explicit turn
and model evidence constrain matches. A pending current response wins over a
historical signature with equal counts. No fixed history-size limit discards
deduplication evidence.

Pending state, settled signatures, and observed response identities survive
verified append parsing. The input-shard format advances because its serialized
parser state changes. Compressed rollouts use the same settlement state machine.

## Consequences

Compaction and terminal responses contribute recorded usage exactly once in
the covered protocol sequences. Additional per-session identity state supports
append correctness; active unresolved responses wait for authoritative progress.
