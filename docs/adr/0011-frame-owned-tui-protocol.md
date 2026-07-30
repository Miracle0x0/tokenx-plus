# ADR 0011: Frame-owned TUI protocol

## Status

Accepted.

## Context

The canonical generation architecture gives the TUI one local-data authority,
but the interactive shell previously coordinated several overlapping
authorities:

- `App` mixed durable product state, page interaction state, rendered
  geometry, terminal measurements, and process I/O;
- `ViewState` and `App` both interpreted raw key and mouse events;
- `ActionSet` advertised capabilities through a key table separate from the
  dispatch table;
- renderers mutated selection and viewport state while drawing; and
- header, footer, graph, and dialog click geometry survived as mutable
  application state after the frame that produced it.

Those protocols made ordering part of correctness. A resize, tab transition,
dialog close, or empty projection could leave one protocol observing state
prepared by another page or an earlier frame. Adding another abstraction
interface around each participant would preserve those split authorities
rather than remove them.

## Decision

The TUI uses one frame-owned interaction protocol:

```text
terminal Event
      |
      v
typed Intent --> capability check --> state transition --> queued Effect
                                          |
                                          v
                               immutable render inputs
                                          |
                                          v
                                  RenderArtifacts
```

### One frame owner

`TuiFrame` owns one `TuiModel`, one `PageStates`, and the artifacts from the
last completed draw. It is the only component that orders event decoding,
capability checks, transitions, effect execution, generation reconciliation,
rendering, and installation of rendered measurements.

The event loop does not separately coordinate model state and page state.
Acquisition and subscription task ownership remains outside `TuiFrame` in the
existing controllers and `TaskSupervisor`; this ADR does not create a second
generation lifecycle.

### Typed intent and one capability vocabulary

Raw keyboard input is decoded exactly once into `Intent`. Mouse hit targets
also carry `Intent`, so keyboard and pointer input enter the same transition
path. `ActionSet` remains the view capability projection used by the footer
and dispatch guard, but it no longer owns another raw-key decoder.

Dialogs retain their local editing vocabulary. Except for the global interrupt,
`TuiFrame` gives an active dialog first refusal over raw input, then reconciles
page state when the dialog closes.

### State ownership

`TuiModel` owns durable TUI product and shell state: installed-generation
projection state, sort choices, detail selections, refresh requests, theme,
language, settings snapshots, status, and subscription lifecycle.

`PageStates` owns modes and interaction state meaningful only to a particular
page, including profile text viewports and Sessions selection. Generic table
selection is keyed by tab or detail context; switching contexts selects a
different interaction value instead of saving and restoring shared scalar
fields.

Presentation and `ActionSet` remain derived projections. They do not become
mutable stores.

### Rendering is observation

Render functions borrow `TuiModel` and `PageStates` immutably. A draw builds a
fresh `RenderArtifacts` value containing:

- hit targets for pixels actually drawn;
- measured list capacities;
- measured text viewports; and
- the active dialog rectangle.

At the end of the render pass, `TuiFrame` installs the measurements and
atomically replaces the previous artifacts. A resize, page change, or
conditional control therefore cannot leave stale clickable geometry from an
earlier frame.

### Effects are explicit and concrete

Transitions prepare concrete `TuiEffect` values for settings persistence,
clipboard writes, report export, and subscription-cache persistence. The
effect boundary executes them and returns typed outcomes that update status
and diagnostics.

Tokenx does not introduce service traits, an event bus, reducers, or a generic
command framework for these process-local operations. A new effect variant is
justified only when a transition must request a real side effect.

### Generation invariants remain unchanged

This protocol is downstream of the immutable installed `Generation`.
Projection controls still cannot scan inputs, reset acquisition clocks, write
generation caches, or replace a warm generation. Startup, automatic refresh,
and explicit manual refresh remain the only local acquisition triggers as
defined by ADR 0007 and ADR 0009.

## Consequences

- Event ordering has one owner and raw key mappings have one definition.
- Rendering can be tested as an immutable observation plus explicit frame
  output.
- Selection and scrolling cannot leak between unrelated tabs or detail views.
- Click behavior always corresponds to the last successfully drawn frame.
- Model transitions are testable without performing filesystem or clipboard
  I/O; effect execution is tested at its concrete boundary.
- Adding a page requires page-owned state only when the page has genuine
  interaction state. It does not require another global dispatcher, mutable
  render context, or compatibility shim.
- The former `App` and `ViewState` internal APIs are intentionally removed;
  they are not retained as aliases.
