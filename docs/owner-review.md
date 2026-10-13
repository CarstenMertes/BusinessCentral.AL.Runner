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
