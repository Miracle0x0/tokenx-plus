# ADR 0015: Display before background persistence

## Status

Accepted. Refines the startup and refresh ordering in ADR 0003, ADR 0007,
ADR 0009, and ADR 0011.

## Context

The TUI previously resolved local prices, decoded the generation cache, and
prepared all local projections before its first frame. Acquisition ran in the
background, but its result waited for durable cache persistence and then for
the terminal's animation tick. An expired startup cache also forced a complete
fold even when its source inventory was unchanged.

## Decision

The initial frame precedes local pricing resolution and generation-cache I/O.
A supervised startup worker resolves the local pricing snapshot, authenticates
and decodes the complete generation, and prepares its projections. Until that
result exists, local pages remain in the existing Loading state. Cached usage
is installed atomically and drawn before startup inventory checks or public
pricing refresh begin.

Acquisition workers prepare the complete local projection as well as the
generation. If the user changes projection settings while acquisition is in
flight, installation reconciles the result with the current committed query
and detail selection. This remains a pure projection without another scan.

Startup, acquisition, pricing, subscription, and persistence completions wake
the terminal event loop directly. The loop consumes completed results before
rendering; the 100 ms animation tick is not a data-delivery timer.

After a newly acquired generation is installed and drawn, the supervisor
persists it on an owned worker. The controller keeps subsequent acquisition
requests queued until that write completes, preserving write order and retry
metadata continuity while the installed data and projection controls remain
available. Persistence completion carries the acquisition request identity;
an obsolete completion cannot clear a current diagnostic or retry schedule.
Write errors preserve the installed data and expose an explicit cache warning.
Existing warnings clear only after a successful write.

The UI and writer share an immutable generation. Its canonical usage index and
session data are reference counted, so changing runtime pricing diagnostics
does not copy the usage index or modify the writer's captured snapshot. The
cache continues to serialize one canonical generation, with no renderer DTOs
or additional persisted view cache.

An ordinary expired startup generation uses the same source-fingerprint check
as automatic refresh. An unchanged inventory skips parsing, folding, and
generation-cache writes. Missing data, changed acquisition configuration,
explicit manual refresh, and due input-integrity retries retain their existing
acquisition semantics. File stamps still include presence, size, nanosecond
mtime, native identity, and declared dependencies; no global latest timestamp
replaces them.

Generation reads and writes use fixed-size I/O buffers. Both authenticated
read passes, complete generation validation, durable atomic replacement, and
explicit persistence failures remain part of the contract. Allocator trimming
for TUI acquisition occurs after background persistence rather than during
generation installation on the terminal thread.

## Consequences

First-frame latency no longer scales with local pricing catalogs, generation
decoding, or usage projection work. A loading frame can precede cached data;
completion wakes the UI as soon as that data is ready. Newly acquired usage
becomes visible before durable storage completes, so a process terminated
before persistence may need to reacquire that generation on its next launch.
Normal shutdown signals cancellation, restores the terminal, and then drains
owned work. Changed inventories still fold unchanged record shards; this
decision does not introduce per-client incremental aggregation.
