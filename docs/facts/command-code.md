# Command Code local-session facts

Verified on 2026-07-18 against the globally installed npm package
`command-code@0.52.1`, its bundled runtime, and the local
`~/.commandcode` corpus. Prompt contents and credentials were not inspected.

## Local storage shape

```text
~/.commandcode/
├── config.json
├── projects/
│   └── <project-slug>/
│       ├── <session-id>.jsonl
│       ├── <session-id>.meta.json
│       └── <session-id>.checkpoints.jsonl
└── file-history/
    └── <session-id>/
```

The verified corpus contains 7 project directories, 11 main session files,
11 matching checkpoint files, and 8 session metadata files.

Each main session JSONL record has this top-level shape:

```text
id
timestamp
sessionId
parentId
role
content
gitBranch
metadata
```

All 311 verified main-session records are valid JSON. The main transcript
schema does not persist a model, provider, or token usage.

Session metadata is stored separately in `<session-id>.meta.json`. Of the 8
verified metadata files, 1 contains `model`; none contains `provider`.

Each checkpoint record has this shape:

```text
type: "file-history-snapshot"
messageId
snapshot:
  messageId
  trackedFileBackups
  timestamp
isSnapshotUpdate
```

The 11 checkpoint files contain 35 valid records. They are file-rewind
sidecars and do not contain model or usage data.

The verified global `config.json` has the keys `firstMessageSent`, `installed`,
`model`, and `provider`. At verification time its identity fields were:

```text
model: Qwen/Qwen3.7-Max-Free
provider: command-code
```

## Tokenx model projection

For every discovered main transcript, the Command Code integration attaches the
same-stem `<session-id>.meta.json` path as an optional cache dependency. The
metadata file may be absent; its creation, deletion, or content change alters
the input fingerprint and invalidates an existing shard.

Model attribution follows one stable order:

1. a present, valid, non-empty `<session-id>.meta.json.model` is canonicalized
   and used for every estimated assistant record in that session;
2. when the sidecar is absent or has no `model` field, Tokenx uses
   `commandcode-model-unknown` with provider `unknown`;
3. the current global `config.json.model` and `config.json.provider` are ignored
   for historical session attribution.

The unknown identity retains estimated input and output tokens but is explicitly
unpriced, including when a pricing catalog contains the same key. A metadata
sidecar that exists but is unreadable, malformed, has a non-string model, or has
an empty model makes the session input unavailable in Data Health and prevents
that result from being cached.

Checkpoint JSONL files remain file-history sidecars and are not discovered as
usage inputs.
