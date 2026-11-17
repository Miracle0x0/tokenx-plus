# ADR 0038: OpenCode ownership of native Claude sessions

## Status

Accepted. Extends ADR 0002, ADR 0003, and ADR 0021.

## Context

OpenCode's Claude Agent SDK integration writes usage to its database and also
produces ordinary Claude transcripts. Counting both attributes one operation
to two clients. Filtering only when both clients are selected would make
client selection change ownership rather than project the installed generation.

## Decision

A countable OpenCode `session_message` assistant row establishes ownership of
the Claude session IDs recorded in its nested `native.sessionID` fields. The
claim query uses the same schema selection and row decoder as OpenCode usage
ingestion, including positive valid tokens, model, timestamp, and required
identities. Zero-token, cost-only, rejected, and non-assistant records cannot
claim a session. Provider attribution remains optional metadata.

Acquisition prepares the OpenCode database inventory once. When OpenCode is
selected, Claude ownership reuses that integration's prepared inputs and
discovery failures. For a Claude-only universe, acquisition prepares the same
inventory as a dependency through the OpenCode driver. Both paths use the
captured root and configured database paths. Discovery snapshots database and
WAL metadata without reading their contents; it never takes a second ownership
snapshot that could be newer than a selected OpenCode cache hit. The dependency
set and OpenCode decoder contracts participate in generation identity; adding,
removing, or changing a database invalidates the projection.

During acquisition, ownership is read once per Claude input group and shared
across batches. Confirmed claims exclude the matching transcript and its nested
subagent transcripts, as well as records with the claimed session identity.
Raw Claude shards and append checkpoints stay unfiltered. A database change
therefore recomputes ownership while reusing unchanged transcript shards.
The client universe, picker projections, and refresh lifecycle are unchanged.

Ownership queries stream candidate rows. Metadata extraction reads container
structure and native session IDs, retaining unrelated scalars as raw JSON just
as the usage decoder ignores them. Malformed candidates or native metadata
produce record diagnostics and do not erase claims from earlier or later
records. Interrupted queries retain confirmed claims and report partial input.
Discovery, open, schema, or snapshot failures
are explicit Claude dependency diagnostics; a database that changed after
preparation cannot establish claims for that generation. In the absence of
confirmed ownership, valid Claude records remain reportable with those
diagnostics. An incomplete generation requires another acquisition rather than
becoming an ordinary complete cache hit.

## Consequences

OpenCode usage is counted once and keeps its producing-client attribution,
including when only Claude is selected. Cross-client ownership adds database
metadata work during discovery and one streaming claim query during generation
construction. It adds no persistent claim cache or subscription requests.
