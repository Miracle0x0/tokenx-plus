# ADR 0030: Captured catalog price provenance

## Status

Accepted. Extends ADR 0004 and ADR 0027.

## Context

A numeric cost does not identify which catalog supplied a rate or distinguish an explicit free price from missing pricing. Repeating lookup during export could select a different authority from the one that calculated the installed generation.

## Decision

The pricing service returns the calculated cost, its selected catalog/custom row key, matched custom billing scope, observed service tier, and used-bucket rate completeness. Finalization captures this evidence at the same point as cost. It never derives provenance from positive or zero amounts, provider-reported money, or a second lookup during projection.

Canonical model, client-total, agent, and daily-client buckets retain a sorted union of origins plus an all-records completeness flag. Projection merges these values over the selected clients. Model and total JSON, and exported agent and daily JSON, expose `costProvenance` with `complete` and `sources`. Empty aggregates have an empty union and are complete; an unpriced participating record makes the aggregate incomplete. Explicit zero rates participate with their real origin and remain complete. Missing used-bucket rates are incomplete even where existing ordinary estimation computes a partial amount.

This is catalog-estimate provenance only. It does not add vendor-reported bills or compatibility graph APIs. Origin sets do not retain paths, session payloads, or credentials. Sources survive generation serialization and regrouping. Input shards exclude both derived cost and its provenance, so a new pricing snapshot recomputes them together. The generation format advances without legacy readers.

## Consequences

JSON consumers can inspect mixed price sources and distinguish free from unpriced usage. Completeness describes pricing coverage, independently of input health and catalog-fetch status. The additional state scales with distinct origins per aggregate, not with the number of raw messages.
