# Waiting for the owner's review

These were decided while the owner was away, or are still open. Each line names
the ADR or document that has the detail. Remove a line once the owner has
reviewed it.

## Open questions (not decided)

- ADR-016 (Proposed), Release: AL Runner v3. Eleven open questions, each with
  options and a recommendation in the ADR:
  - binary name;
  - an `al-runner` compatibility launcher;
  - the exit code for tests that were not run;
  - which repository v3 lives in;
  - NuGet/dotnet tool;
  - how long v2 stays supported;
  - features deferred past 3.0;
  - dependency apps that ship only compiled code;
  - WebAssembly scope;
  - extra build targets;
  - fallback to the `AL_RUNNER_*` environment variable names.

  Nothing is built on ADR-016 until it is accepted.

## Decided by the owner (recorded)

- 2026-10-13, ADR-016: server mode, watch mode and `--affected` are deferred
  while start-up is not a concern. Revisit after dependency loading is
  measured, or when ALchemist or LethAL support is wanted. Those two use the
  server as their interface.
- 2026-10-13, ADR-016, standing rule: every v2 feature that only hides
  start-up or run cost is deferred the same way. Today: `--watch`,
  `--affected`, warm server state, `--jobs`, persistent dependency caches.

## Decided without owner review

- 2026-10-13, orchestrator, hard rule 3: when a guarded XmlPort import fails,
  its last error cannot be read. Reading it refuses the test instead of
  returning an invented text.
- 2026-10-13, orchestrator: the XmlPort export bytes are an assumption: an XML
  declaration, no whitespace, UTF-8 with no BOM. No corpus test compares the
  bytes, and the assumption is listed in `probes-needed.md`.
- 2026-10-13, ADR-014: a .NET exception from a `DotNet` call refuses the test,
  until a probe pins BC's error text.
- 2026-10-13, orchestrator: when only part of an error text is pinned, the
  runtime fills in the rest and records the full text as an assumption in
  `probes-needed.md`. The probe agent pins these first. Examples: the
  "transaction is stopped" error, nested test codeunits, CommitBehavior Error,
  and "part not found" (the runtime uses ID 0).
- 2026-10-13, ADR-012 (page extensions): several page extensions that declare one
  page trigger both run, in application order (dependency app first, then
  object id); the page's own trigger runs first. Both are assumptions listed in
  `probes-needed.md`.
- 2026-10-13, ADR-012 (page extensions): only the void page triggers are
  allowed in an extension. The Boolean ones, OnInit, OnFindRecord and
  OnNextRecord are refused.
- 2026-10-13, ADR-012 (application areas): the session's areas are read when a
  page opens; a control with no area is removed while the areas are restricted;
  the test runner empties the areas before each test method.
- 2026-10-13, ADR-012 (page extensions): `DrillDown`, `AssistEdit` and `Lookup`
  on a TestField run the control's trigger (its own or a `modify` block's).
- 2026-10-13, ADR-013, host values: `SessionId()` and `ServiceInstanceId()` answer 1
  (positive), not 0, because codeunit 60023 asserts `> 0`.
- 2026-10-13, ADR-013, host values: the host clock is a trait. It is frozen at
  2024-01-15T12:00:00Z in the deterministic host and is the system clock on the
  command line. `Sleep` moves it forward without waiting.
- 2026-10-13, ADR-013, host values: the session time zone is UTC (AL Runner uses the
  machine's zone). `SessionSettings.Init` answers `UTC`.
- 2026-10-13, ADR-013, host values: `WorkDate()` is today until set, per session. A new
  test codeunit starts at today again.
- 2026-10-13, ADR-013, host values: `CurrentClientType()` is `Background` and
  `DefaultClientType()` is `Web`.
- 2026-10-13, ADR-013, host values: `CompanyName()` is `My Company` instead of the
  placeholder `CRONUS`.
- 2026-10-13, ADR-013, host values: temporary records are not stamped with the system
  created and modified fields.
