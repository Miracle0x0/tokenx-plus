# ADR 0036: Explicit offline mode

## Status

Accepted. Extends ADR 0005, ADR 0006, and ADR 0012.

## Context

Local reports could refresh stale pricing catalogs implicitly. Users needed a
way to prohibit network access while retaining local usage, cached prices, and
custom overrides, plus a separate command for deliberate catalog maintenance.

## Decision

The global `--offline` option is an immutable process choice, carried in the
resolved startup/command plan. Models and cache warming bind local pricing
without refreshing it. Pricing lookup reads local catalogs and overrides only.
Available cached catalogs may be used regardless of age; missing or invalid
catalogs retain their existing explicit diagnostics and unpriced token usage.
Offline mode does not modify settings or synthesize replacement prices.

The TUI still acquires and refreshes local inputs, projects the installed
generation, and exports local reports. It never schedules public catalog
refresh in offline mode. Subscription shows its eligible local cache, disables
remote refresh actions, and identifies its offline state. Neither initial entry
nor manual fetch requests queue network or credential work.

`pricing refresh` explicitly refreshes all three public catalogs, even when
their caches are fresh. Sources retain independent atomic publication: a failed
source leaves its previous file intact, while successful sources can advance.
Diagnostics are printed and produce a nonzero exit status, including warnings
from a recovered retry or cache-write failure. An invocation with no diagnostics
reports completion. `--offline pricing refresh` is an invalid invocation and
fails before network work. `--no-spinner` controls refresh presentation.

Network policy does not enter the generation identity: the captured pricing
contents and input configuration already determine that identity. A mode change
alone cannot change token or cost facts or require another local scan.

## Consequences

Offline reports remain usable without network access and expose incomplete
pricing. Explicit catalog refresh has observable outcomes without changing
custom overrides or account credentials.
