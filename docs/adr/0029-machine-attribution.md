# ADR 0029: Machine attribution as a generation projection

## Status

Accepted. Extends ADR 0004 and ADR 0007.

## Context

Explicit extra scan roots can contain usage mirrored from several machines. Client and workspace identity do not name the machine holding the original session. A grouping change must not reacquire inputs or modify caches.

## Decision

`scanner.machineLabels` maps absolute roots to trimmed, nonblank labels. `scanner.localMachineLabel` defaults to `local`. The longest containing root wins on whole path components. Resolution considers configured and canonical paths, with ASCII case-insensitive Windows matching. Canonical aliases with different labels are configuration errors; unreadable roots fail explicitly. An absent label root discovers no usage and adds no inputs.

Labels are assigned while each input's source path remains known, on fresh parsing, cache reads, and incremental paths. Reconciliation retains the label of its retained source record. Existing cross-file usage deduplication is unchanged: a copied response is counted once, under its retained source, rather than counted again per label. Sessions retain their existing client/session identity.

Machine labels are derived input attribution and are excluded from cost-free source shards. The prepared inventory captures the resolved labels and includes their effective input assignments in its fingerprint. A new generation relabels unchanged cached records from the captured settings. Canonical model, daily, and hourly keys retain the dimension so `machine,model` is an in-memory projection. Weekly and monthly derive it from daily models. Client selection, totals, health, sessions, and refresh clocks retain their existing contracts.

CLI JSON uses a dedicated `machine` field only in machine grouping. The TUI picker and Models, daily detail, period detail, and hourly tables display the dimension without changing canonical model IDs or colors. Canonical model rankings and contribution totals remain grouping-independent. The generation cache schema advances without migration.

## Consequences

Mirrored roots become distinguishable without a separate scanner or reporting pipeline. Labels identify user configuration, not verified remote host identities. Reading a generation never requires the original inputs to remain available.
