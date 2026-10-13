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
- 2026-10-13, ADR-011: source positions in the IR are `Stmt::Pos` marker statements and a
  `SrcPos` table; call expressions use their statement's position.
- 2026-10-13, ADR-011: `GetLastErrorCallStack` uses AL Runner v2's measured frame format
  (relative line, quoted object name, app tail) for codeunits and tables. Other object types
  stay refused. Only substrings of it are pinned by the corpus.
