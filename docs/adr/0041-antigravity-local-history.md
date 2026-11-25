# ADR 0041: Antigravity local history and transcript accounting

## Status

Accepted. Extends ADR 0002 and ADR 0013.

## Context

The Antigravity CLI and IDE extension write the same per-conversation SQLite
schema. Newer databases record the wall-clock time of a model reply in the
planner step. Using session creation for every reply can move later activity
to the wrong day. Antigravity also writes JSONL transcripts that can outlive a
database. A transcript repeats generation usage with fewer token buckets and
identifies replies by step index.

## Decision

Both products use the existing `antigravity` client identity. A discovered
database owns its session, including when reading it produces a Data Health
error. Read generation, step and trajectory tables in one read-only SQLite
snapshot. Generation usage fields 2, 4 and 5 supply uncached input, cache
writes and cache reads. Fields 9 and 10 supply reasoning and text output.
When text output is omitted or zero, the recorded inclusive output in field 3
can establish it after subtracting reasoning. Field 1 identifies the model enum.
Deduplicate by response ID, or by session and generation index when a response
ID is absent.

Date each generation by its explicit timestamp, then the matching planner
step, then the recorded session creation time. Match steps by response ID or
by the stored generation index when the available IDs agree. A malformed
planner timestamp contributes a record diagnostic. A positive-usage
row without any recorded time has the `missing-timestamp` diagnostic.

For a session without a database, read `transcript_full.jsonl` if present in
its generated logs directory, otherwise `transcript.jsonl`. Count
`PLANNER_RESPONSE` steps from `MODEL` and deduplicate copies by session and
step index. Use `created_at`, `model`, `input_tokens`, `cache_read_tokens`
and `output_tokens`. Logged output already includes thinking. Only the
database gives a separate reasoning count. Prompt history, chunk copies and
RPC-derived exports supply no additional usage.

Preserve the observed model label in input shards. The shared model mapping
boundary removes terminal Gemini effort qualifiers for default grouping and
pricing, including on observations from other clients. Explicit user rules
still see the original spelling. Generation cache schema 21 retires aggregates
with the previous identities and dates.
