# ADR 0035: Hermes recorded call times

## Status

Accepted. Extends ADR 0001, ADR 0003, and ADR 0028.

## Context

Hermes session totals can combine several models and several days. Its
`session_model_usage` rows identify model/provider totals, and adjacent rotated
`logs/agent.log*` files record call timestamps and token buckets. Counting only
`sessions` assigned all usage to its initial model and start date.

## Decision

For sessions with model-usage rows, those rows own the totals. Group valid rows
by session, observed model, and billing provider using checked arithmetic. Use
session totals only for sessions without model rows; a malformed model row does
not revive session totals. Message counts are credited once per session.

Match logged calls by the same identity. Cache reads and writes are subsets of
the log's inclusive input, and the log's output includes reasoning. Deduplicate
rotated copies using call number, timestamp, identity, and token counters.
Preserve each validated call's actual timestamp and buckets. The database is
authoritative: unrepresented tokens remain at the session start time, while
overreported logs produce a rejection and leave that group's database totals
intact. Distribute recorded reasoning within output using exact integer
proportions; output plus reasoning remains unchanged in every share.

Call timestamps use the captured acquisition timezone to interpret the logger's
local wall clock. Ambiguous or nonexistent local timestamps are rejected.
Mirrored logs must therefore use that same source timezone. Logged calls have
single-request pricing scope; residual database totals have aggregate scope.

Across duplicate databases, the first group wins unless another copy has equal
authoritative token buckets and more distinct logged dates. Equal coverage and
different totals keep the first copy. A private temporary file stores candidates
while an in-memory group index selects them, avoiding full copies of all inputs.

Discovery captures the optional log paths once, including the absent main log.
Logs participate in generation fingerprints and input footprint alongside the
database and required WAL. Log discovery/read failures preserve database usage
with explicit partial-input diagnostics. WAL metadata failures remain required
input failures. Changes during reading are partial and retried. Hermes continues
to bypass record shards; generation schema 19 invalidates prior aggregates.

## Consequences

Model, daily, hourly, and session reports share the same exact database totals.
Missing historical logs reduce time resolution rather than token coverage, and
additional log evidence can improve attribution without double counting copies.
