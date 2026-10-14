# ADR 0020: OMP message deduplication and child-session identity

## Status

Accepted.

## Context

OMP forks can copy assistant responses into a new session file. Counting each
file independently duplicates historical usage. A child transcript can also
outlive its parent transcript, so parent-file presence alone cannot determine
whether that child is a main session.

## Decision

The OMP integration owns one cross-file message identity scope per acquisition:

- A non-blank `message.responseId`, scoped by the observed or inferred provider,
  identifies a provider response.
- Without that ID, a non-blank entry `id` together with its parsed timestamp,
  provider, observed model, and normalized token buckets identifies the event.
- Session IDs, parent-entry IDs, paths, pricing, and model mappings do not
  participate. Tuple boundaries are encoded before hashing so field separators
  cannot introduce ambiguous identities.
- Records without either ID remain eligible and are not deduplicated. Token,
  model, and timestamp validation remains unchanged.

The fold emits the first occurrence of each identity and suppresses later
copies without reporting a record error. The same set covers cached records,
fresh records, all parsing batches, and records reparsed after cache damage.
Input shards retain every eligible source record before cross-file filtering,
so removing one file cannot erase the remaining file's copied history.

The surviving copy supplies session, workspace, and agent attribution. OMP's
existing fold processes valid cache hits before misses; which copy supplies
those attributes can therefore change when cache availability changes. This
decision guarantees one accounting contribution per event, not preferential
ownership by the original session.

A transcript directly inside a directory named `<timestamp>_<session-id>` is
a child even when the sibling parent JSONL is absent. The recognized layout
uses a valid UTC timestamp in `YYYY-MM-DDTHH-MM-SS[-mmm]Z` form and a UUID
or eight hexadecimal digits.
Existing parent-task metadata, Advisor, and Swarm rules still supply agent
identity; the directory alone does not invent an agent label. Parent transcript
creation, deletion, and modification remain input-cache dependencies.

The scan root remains `~/.omp/agent/sessions`, with additional roots supplied by
`scanner.extraScanPaths.omp`. Pi remains independent. OMP reasoning and
orchestration accounting is unchanged.

## Consequences

Forked histories contribute tokens, messages, and derived cost once. Retained
orphan child transcripts no longer inflate the main-session count. The
integration source changes invalidate decoder-derived caches automatically;
no cache-format migration or dependency change is required.
