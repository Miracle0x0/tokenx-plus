# ADR 0033: DSH streaming transcripts

## Status

Accepted. Extends ADR 0001 and ADR 0003.

## Context

DSH logs can exceed the former 64 MiB file or decoded-transcript ceiling.
Materializing the complete transcript multiplied memory usage across parsing
workers and made otherwise valid large sessions unavailable.

## Decision

Read a snapshot of the opened file's physical length through a buffered line
reader. Detect Zstandard by magic bytes and decode concatenated frames through
the same line parser. Accept a valid final JSON record without a newline.
There is no whole-file byte ceiling; input buffering scales with the largest
record and decoder window, while accounting state scales with usage records.

A read or decode failure preserves previously completed records, marks the
input partial, and prevents shard publication. The unfinished line contributes
nothing. Failure before the first completed line remains an input error.
Malformed complete records retain the existing rejection handling. Replacing a
complete cached input with a partial one invalidates its old shard, and every
subsequent scan retries it until repair.

For ordered seeded v3/v4 streams, each inherited `session/end-seed` marker
discards the preceding usage and settlement identity state. The last inherited
marker therefore owns the fork boundary; ordinary resume markers do not erase
usage. A seeded stream without a valid inherited boundary is an explicit error.

The source-derived DSH contract invalidates old shards, and generation schema
17 rebuilds reports that previously excluded large inputs.

## Consequences

Large plain and compressed sessions remain countable without retaining their
entire decoded transcript. Existing retry, model attribution, and cross-input
deduplication semantics remain shared by both encodings.
