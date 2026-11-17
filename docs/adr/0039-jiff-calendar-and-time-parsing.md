# ADR 0039: Jiff calendar and timestamp semantics

## Status

Accepted. Refines ADR 0003 and ADR 0004.

## Context

RustSec marks every Chrono and chrono-tz version as unmaintained. Upgrading their
versions cannot satisfy the existing audit policy. Tokenx needs a maintained
time library for local transcript timestamps, IANA calendars, date filters,
period projections, and presentation.

## Decision

Use Jiff for absolute timestamps and civil dates/times. Bundle its IANA timezone
database on every platform instead of depending on each host's zoneinfo files.
The system timezone name is still resolved once at startup by `iana-time-zone`;
an explicit configured timezone remains authoritative. Missing or invalid
timezone identity is an error, including Jiff's `Etc/Unknown` UTC substitute.

`CalendarContext` owns the resolved timezone and serializes its IANA name.
Readers borrow it; ownership transfers clone the immutable timezone handle.
Projection dates and hours use Jiff civil values, and date arithmetic remains
calendar arithmetic. ISO weeks keep their ISO year across Gregorian year
boundaries. Positive epoch values remain the record-eligibility boundary;
timestamps must also fit Jiff's representable range. Civil years range from
-9999 through 9999. Inputs outside that range produce existing date or record
diagnostics rather than substituting another date.

Junie and CodeBuddy wall-clock timestamps choose the earlier occurrence in a
repeated DST hour and reject nonexistent times. Hermes continues to require an
unambiguous local time. Timestamp offsets are authoritative for absolute usage
records. These rules are explicit rather than using Jiff's default gap shifting.

The generation-cache schema advances to version 20. Existing generation caches
are rebuilt from authoritative inputs; no Chrono wire compatibility layer is
retained. Production decoder-source changes invalidate affected input shards
through the existing generated contracts.

## Consequences

Chrono and chrono-tz leave the dependency graph without audit exceptions. The
calendar's timezone data is consistent across supported platforms, while its
existing startup capture and pure projection boundaries remain intact. Stored
public dates keep their ISO text representation, and local cache state is
disposable across the library transition.
