# ADR 0016: Buffered and compressed cache I/O

## Status

Accepted. Revises the generation envelope and public-pricing cache freshness
contract in ADR 0003 and ADR 0015.

## Context

Buffered generation loading removed many small reads from warm startup, but
record-shard decoding still issued a file read for individual bincode fields.
Startup also loaded unchanged public pricing catalogs twice. Refreshes wrote
uncompressed generations and rewrote complete pricing catalogs merely to renew
their embedded freshness timestamps.

## Decision

### Record shards and directory metadata

Header-only shard planning continues to read only the envelope and header.
After the complete body digest is verified, a 64 KiB buffer serves body
deserialization. The logical byte limit is outside that buffer so read-ahead
cannot hide trailing encoded data. Shard format and record eligibility do not
change.

Existing cache directories are still checked for symlinks and directory type.
On Unix their permissions are changed only when they differ from `0700`.
Creating a directory or repairing its permissions remains explicit and fallible.

### Compressed generation envelope

Generation schema 6 stores one Zstd level-3 frame containing the canonical
bincode generation. The authenticated fixed header contains both compressed
and decoded body lengths. It retains the source-independent save timestamp,
compressed-body digest, and retry metadata. Older generation envelopes are
rejected and rebuilt through the existing diagnostic path.

Both lengths retain the existing 256 MiB body bound. The same bound constrains
the decoder window; decompression cannot bypass the generation capacity
contract. Reading authenticates the compressed bytes before decoding, hashes
the second file pass, and validates the complete decoded generation. Extra
frames, trailing compressed bytes, trailing decoded values, truncated frames,
and inconsistent lengths are errors. Both compressed input and decoded output
are buffered; no full serialized or decompressed body is materialized in memory.

Writing computes the serialized size, streams bincode through the compressor,
then completes the authenticated header. Durable atomic publication and the
display-before-persistence lifecycle from ADR 0015 remain unchanged.

### Public pricing snapshots and freshness

Pricing cache schema 2 contains `version` and `data`. The opened file's
modification time is the sole freshness timestamp, with the existing one-hour
TTL and explicit rejection of future timestamps. Schema 1 and unversioned
catalogs are not reused. Catalog identity continues to depend on pricing data,
not freshness metadata.

A locally resolved snapshot records the paths, sizes, modification times, and
native identities of its three successfully loaded fresh catalogs. Startup's
background pricing task reuses that immutable service when all three files
still match and remain fresh. A failed check, expired catalog, different cache
directory, or replaced file performs ordinary catalog resolution with its
existing diagnostics and remote-refresh rules. A same-size, same-mtime atomic
replacement changes native identity and therefore cannot reuse the old service.

When a fetched catalog has the same JSON value as its existing cache, saving
updates and syncs only the modification time of that exact opened inode.
Object-key order is insignificant. Changed data is published by durable atomic
replacement. The timestamp update cannot be redirected to a concurrently
replaced path. Read, timestamp-update, sync, and replacement failures remain
observable to the caller.

## Consequences

Record-cache hits issue buffered reads instead of per-field syscalls. Fresh
startup avoids a second catalog parse and fingerprint calculation. Compressed
generation files reduce transfer and write volume at the cost of compression
and decompression CPU work. Identical pricing responses renew freshness without
rewriting their data. Filesystem copying tools that preserve modification time
also preserve a pricing catalog's age.
