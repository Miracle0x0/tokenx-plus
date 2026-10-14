# ADR 0021: OpenCode SQLite schemas, discovery, and accounting

## Status

Accepted.

## Context

OpenCode databases can contain assistant usage in `message`, `session_message`,
or both. Reading only `message` can report empty or incomplete usage even when
the database is healthy. Newer payloads carry model identity under `model` and
use a SQL column for the message role. Separate messages can also share every
token and timestamp field, so a content fingerprint alone cannot establish
duplicate identity.

## Decision

OpenCode discovery selects `opencode.db` and `opencode-<channel>.db` under
`$XDG_DATA_HOME/opencode` when XDG_DATA_HOME is non-empty, otherwise under
`~/.local/share/opencode`. The environment value must be absolute. An explicit
`--home` disables environment root overrides. The composition root captures
client root overrides once; the acquisition configuration owns them, includes
them in generation-cache identity, and preserves them across TUI refreshes and
pricing changes. Discovery never reads ambient environment variables.
`scanner.opencodeDbPaths` remains the authority for additional database files.
Legacy JSON stores are outside this integration's accepted inputs.

The decoder inspects table identities before querying. It reads
`session_message` first and `message` second when both exist, using one
accumulator. It does not choose schemas by catching failed queries. A database
with neither table, or a recognized table missing required columns, is an
explicit input error. The existing committed-WAL snapshot contract applies to
both formats.

For `message`, JSON `role` is required. For `session_message`, the `type`
column is authoritative: only `assistant` rows are decoded, JSON `role` may
be absent, and an explicit conflicting JSON role is a rejected record.
Top-level `modelID` and `providerID` take precedence over `model.id` and
`model.providerID`. A blank explicit model is invalid. Optional provider
attribution still follows ADR 0001. Token values retain their existing
validation and pricing semantics; malformed and negative token records are
rejected rather than normalized into plausible usage.

`message` uses `session`; `session_message` uses `session_v2` when present,
otherwise `session` when present. A selected metadata table must provide `id`
and `directory`; query failures remain explicit. When neither applicable
metadata table exists, the message's own session ID, model, time, and token
facts remain sufficient for usage. A non-empty session directory has priority
over the optional payload `path.root`; absent workspace evidence stays absent.
The optional metadata column `parent_id` identifies child sessions. Without
parent evidence, the existing main-session classification is retained.
This replaces the previous requirement that every accepted database contain
`session.directory`, while preserving errors for malformed selected schemas.

Within a database, copied turns with matching timestamp, model, provider,
tokens, and agent fingerprints share one usage record unless both carry
different embedded message IDs. Different explicit IDs always retain separate
records. An embedded ID takes priority over a SQL row ID for cross-database
deduplication. The existing fold keeps one identity set across channels,
batches, and cached or freshly parsed records. Conflicting workspace evidence
on merged copies clears workspace attribution.

## Consequences

New-schema usage is counted even when an empty older message table remains.
Mixed databases contribute all distinct turns, and equal token fingerprints
cannot erase distinct explicit message identities. Changes to decoder sources
invalidate derived caches through the existing generated decoder contract;
there is no cache migration or historical JSON compatibility layer.
