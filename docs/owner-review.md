# Waiting for the owner's review

These were decided while the owner was away, or are still open. Each line names
the ADR or document that has the detail. Remove a line once the owner has
reviewed it.

## Open questions (not decided)

- Push the second batch of probe tests to the corpus branch `al-runtime-rs`?
  - It is about 60 tests in 8 files, at local head b06fdc0 in corpus-probes.
  - All of them pass on the local BC 27.5 container, and the corpus gate
    scripts pass.
  - A push means 3 more corpus CI runs (27.5, 28.5 and 29.0) and then an
    oracle bump.
  - The findings: report filters, transactions, Skip/Break/Quit, RunObject
    page types, xmlport nesting and VariableText, Text filter case
    sensitivity, Copy and SetView, the order of Global Triggers, the SystemId
    of temp records, the RunModal result values, and the order of part close
    triggers.
  - Held because tonight's CI budget is used up.

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

- 2026-10-14, orchestrator: there were 7 corpus CI runs overnight, not about 3.
  - 3 were the probe batches.
  - 1 was the final run on the probe head. Its corpus lint failed: two
    probe codeunits shared a SingleInstance fixture.
  - 1 re-ran after the fix (cdf315a).
  - 2 were single-version runs for 28.5 and 29.0 on cdf315a.

  The 28.5 and 29.0 runs were needed because the CLI requires every
  per-version oracle to come from the same corpus commit. Without them, the
  corpus bump for the new probe tests would have stalled.

- 2026-10-14, ADR-013, dependency apps: the 45 s golden test of unresolved references
  (`corpus_resolve_deps.rs`) is `#[ignore]`d in debug builds; CI runs it in the release step.
- 2026-10-14, ADR-013, dependency apps: the missing test-library apps (System Application Test
  Library, Permissions Mock, and Any, which the first needs) come from the artifact feed through the
  download fallback (Layer A, built). Nothing of theirs is committed. Azure AD Plan Test Library
  is a module inside System Application Test Library, not a separate app.
- 2026-10-14, ADR-013, Layer A: `--no-download` skips a missing app with a printed line instead of
  failing; a feed that does not answer fails the run (exit 2). The library keeps the error policy.
- 2026-10-14, ADR-013, Layer A: a cached build wins over a newer published one, and the project's
  `.alpackages` wins over both; an exact build the feed has dropped is an error.
- 2026-10-14, ADR-013, Layer A: the cache keeps each entry as Microsoft ships it with a record of
  build, entry, size and CRC-32; the build variables in Base Application's `app.json` are filled
  from the build (evidence: the compiled manifest of 27.5.46862.48827).
- 2026-10-14, ADR-013, Layer A: no build is chosen and nothing is requested when every dependency is
  on disk; `--project DIR --fetch-deps` is the only project mode so far.
- 2026-10-14, ADR-013 and ADR-016, Layer A: names `ALRUNNER_CACHE`, `ALRUNNER_ARTIFACT_URL`,
  `--cache-dir`, with `AL_RUNTIME_CACHE` and `AL_RUNNER_CACHE_ROOT/v3` read as fallbacks.
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
- 2026-10-14, ADR-012, pages-misc: a `Page` variable is constructed (OnInit once) at its first procedure call.
- 2026-10-14, ADR-012, pages-misc: lists with OnFindRecord/OnNextRecord are walked through the triggers; `Which` is `-`/`+`
  for First/Last (BC passes `=`-forms the corpus does not pin).
- 2026-10-14, ADR-012, pages-misc: the deleted-row sequence on such lists is inferred from four tests and refused elsewhere.
- 2026-10-14, ADR-012, pages-misc: `TestField.Lookup` order is control trigger, table field trigger, TableRelation page; a
  table-field OnLookup marks the row edited.
- 2026-10-14, ADR-012, pages-misc: page background tasks run synchronously in a read-only session; tasks queued for
  other rows are not cancelled.
- 2026-10-14, ADR-012, pages-misc: `asserterror` rollback extent around a failing TestPage lookup is left as is and the case is refused
  while writes are pending.
- 2026-10-14, ADR-017, error collection: closing a collecting scope that holds several errors raises
  `Multiple errors occurred. <first message>`; the corpus pins only the leading words and the first message.
- 2026-10-14, ADR-017, error collection: `HasCollectedErrors()` is true outside a scope, as the platform
  computes it.
- 2026-10-14, ADR-017, media: Media and MediaSet fields hold a Guid and the objects live in a session store, a
  stand-in for the Tenant Media tables until ADR-013 lands.
- 2026-10-14, ADR-017, partial records: `AddLoadFields` and `SetBaseLoadFields` on a full load answer false.
- 2026-10-14, ADR-017, call depth: the ceiling is 1000 nested calls; beyond it the run is refused.
- 2026-10-13, ADR-012 (request pages): a report without a `requestpage` section opens a request page of
  its data items' filters unless `UseRequestPage` is switched off; a run that opens a request page where the
  test declares no `[RequestPageHandler]` is refused, not answered with `Unhandled UI`.
- 2026-10-13, ADR-012 (request pages): `RunRequestPage` returns an XML text of this runtime's own layout and
  accepts only that text back as a seed; `RunRequestPage` commits its transaction world like `Run`.
- 2026-10-13, ADR-012 (request pages): the id in "The action with ID = N is not found on the page." is
  FNV-1a over the UTF-16 text of the report id and the action name, fitted to three corpus ids.
- 2026-10-13, ADR-012 (request pages): a report that renders output (not `ProcessingOnly`, or a layout) is
  refused when its run reaches the output, so its request page can still be tested.
- 2026-10-13, ADR-012 (FilterPageBuilder): `RunModal` and `[FilterPageHandler]` are refused; the default page
  caption is `Filter`; `AddRecord` and `AddRecordRef` copy the record; two error sentences are filled in
  around the control name (the corpus pins only the name).
- 2026-10-14, ADR-018, background worker: a failing guarded `Codeunit.Run` in a worker does not undo the caller's pending rows; a direct `Commit` and an unguarded `Codeunit.Run` there stay refused.
- 2026-10-14, ADR-018, telemetry: `Session.LogMessage` is a no-op in every overload.
- 2026-10-14, ADR-018, SessionSettings: `RequestSessionUpdate(false)` hands a copy to the handler and needs true back; `(true)` and a missing handler are refused.
- 2026-10-14, ADR-018, InitValue: `0DT` and closing-date literals raise in `Init()` of a written record and on the first read of an untouched one.
- 2026-10-14, ADR-018, held branch: the narrower static-subscriber global check is not merged (net zero) until the `DeleteAll(false)` probe is in.
