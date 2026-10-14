# Waiting for the owner's review

These were decided while the owner was away, or are still open. Each line names
the ADR or document that has the detail. Remove a line once the owner has
reviewed it.

## Open questions (not decided)

- ADR-020 (Proposed), `al-bcdb`: the Rust port of bcdb, and the proposed way to load CRONUS
  into the store on first touch of a table. Read the "Decided without owner review" part
  first. The loader itself is not built. Questions: where the `TableProvider` trait lives
  (`al-host`, as proposed), and whether names come from the compiled program (proposed) or
  from symbol files as bcdb does.

- (answered 2026-10-14, see below) Push the second batch of probe tests to the corpus branch `al-runtime-rs`?
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

- Corpus workflow fix: done (2026-10-14). The corpus branch `al-runtime-rs` now defines `BC29PLUS` on dispatch.
  The 29.0 run on 348a744 is the BC 29 oracle, and the 29 table has no `corpus_commit` any more.
- Probe batch 2 push: done (2026-10-14). Batches 2 and 2b are in the corpus at 348a744 (53 new cases). All three
  oracles come from that commit. The failing and not-run ones are the next fix round.

## Decided without owner review (2026-10-14), isolation branch

- Extra oracles for the OnPrem app (all three majors) and the isolation probe (BC 28.4 only, from
  the Windows nightly, XUnit): ADR-007 decision 4. There is no isolation oracle for 27 and 29.
- `TestIsolation = Function` is implemented (restore to the state after `OnRun` after each
  method). `Disabled`, a runner without the property, and a `Commit()` inside a Function-isolated
  test are refused: `docs/probes-needed.md`.
- A test codeunit is the unit of future parallel scheduling under every isolation mode, because
  its methods share instance state (`al-test-cli/src/isolation.rs`).

## Decided by the owner (recorded)

- 2026-10-14, ADR-014: BC server classes that Microsoft's AL reaches through
  DotNet (ALConfigSettings, NavTenantSettingsHelper, NavUserAccountHelper and
  others) are provided in Rust through the registry.
  - Their values mirror AL Runner v2 until a probe on real BC pins them.
  - This is not a fake of an AL object, so hard rule 7 does not apply.
  - The work is on branch `dotnet-registry`. Probe batch 3 (corpus-probes
    c542144) pinned the values it covers, and the probe wins where it differs
    from AL Runner: the oracle tenant is a sandbox on Azure topology
    (`IsSandbox` true, `IsAzure` true), AL Runner's is Production on premises.
    ADR-013's "Production by default" is therefore superseded for the registry.

- 2026-10-14, performance phase: run tests in parallel at the test runner's
  isolation unit.
  - BC's test isolation can be per codeunit or per test procedure.
  - Units share one common data store. A copy per unit or per thread would
    cost too much memory.
  - Design direction: every unit forks from one shared baseline snapshot, using
    the persistent maps of ADR-003 and ADR-004, so a fork costs O(1) and
    memory grows only with each unit's own changes.
  - Within a session, execution stays single-threaded, as in BC.

- 2026-10-14, performance goal: first close the remaining gaps, then work on
  performance.
  - The target is a corpus run WITH Microsoft's apps in under a second, and
    then further until nothing is left to squeeze.
  - Caching comes only after that, as the last step.
  - This overrides ADR-013's "persistent cache if the warm budget is missed".
- 2026-10-14: the Rust port of bcdb (BusinessCentral.BakReader) starts now, on
  branch `bcdb`, as a new crate.

- 2026-10-14, ADR-016: release path is "separate now, decide later". The runtime
  gets a new binary name, stays in this repo, and has a clean CLI. The cutover
  decision is made at the preview gate. Questions 1, 2 and 4 are answered;
  the binary name itself is still to pick.
- 2026-10-14: push all probe tests to the corpus branch. Done: 348a744, with
  the BC29PLUS dispatch fix.

- 2026-10-13, ADR-016: server mode, watch mode and `--affected` are deferred
  while start-up is not a concern. Revisit after dependency loading is
  measured, or when ALchemist or LethAL support is wanted. Those two use the
  server as their interface.
- 2026-10-13, ADR-016, standing rule: every v2 feature that only hides
  start-up or run cost is deferred the same way. Today: `--watch`,
  `--affected`, warm server state, `--jobs`, persistent dependency caches.

## Decided without owner review

- 2026-10-14, User table triggers (`usertable.rs`): applied for every User write, with no check of
  the topology gate the corpus header describes (unique emails only on premises). The Cloud app's
  tier is not known to take the same branch. The error texts are the corpus fragments only.

- 2026-10-14, ADR-014 registry: `NavTenantSettingsHelper.GetEnvironmentName` answers an empty
  text. The probe only sees `'Sandbox'` through the wrapper, which also answers `'Sandbox'` for an
  empty name. `SetTestTenantEnvironmentType` is not provided: no corpus test reaches it and the
  tenant is already a sandbox. `PermissionTestHelper` (Permissions Mock) stays refused: it needs
  permission enforcement, which is not modelled.

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
- 2026-10-14, ADR-019: a column of a platform table that no corpus test pins is refused where code names it
  (at lowering), not filled with a default.
- 2026-10-14, ADR-019: package ids are a function of the app id (not empty, one per app, different between apps);
  objects of the platform app have none.
- 2026-10-14, ADR-019: the default host lists five Windows time zones; Windows Language holds three rows (1031,
  1033, 2057) and refuses a read that needs any other id up to 65535.
- 2026-10-14, ADR-019: a `Get` whose result is used, in dependency code, on a table the install baseline could not seed, is refused like the raising form (the order in `records.rs` `fail`).
- 2026-10-14, ADR-019: the session seeds the Company (`My Company`), the User (`TESTUSER`) and the SUPER Access
  Control row when it starts, as ordinary stored rows. The install baseline branch must not seed them again.
- 2026-10-14, ADR-019: Page Metadata and Page Control Field spellings the corpus does not pin (declared
  `Visible`/`Enabled`/`Editable` in lower case, `SourceTableView` only for one `where(f = const(v))`).
- 2026-10-14, ADR-019 (round 2): All Profile is a stored table seeded from the profile objects of the loaded apps, with native delete and rename rules for installed apps' rows (texts pinned in profile/TestAllProfileTable.al).
- 2026-10-14, ADR-019 (round 2): Report Data Items numbers its items 10, 20, 30, ...; Action Index and Action ID of Page Action count from 1 (container, then actions depth first, container parent 0); "Condition No." of Table Relations Metadata counts a row's conditions from 1 and is 0 for an arm with none. The corpus pins none of these numbers, only the relations between rows.
- 2026-10-14, ADR-019 (round 2): XmlPort Metadata reads the documented AL defaults for properties an xmlport leaves out (Direction Both, Format Xml, UseRequestPage true, TransactionType UpdateNoLocks, TextEncoding UTF8).
- 2026-10-14, ADR-019 (round 2): Event Subscription lists compiled subscribers only; an uncompiled codeunit is a poisoned prefix row, so unfiltered walks and publisher filters are refused. A table trigger event has the event type Trigger.
- 2026-10-14, ADR-019 (round 2): the code coverage tables are stored and empty; once recording started they are refused until an unfiltered DeleteAll. Feature Key (a hardcoded platform list that changes by version) and Tenant License State stay without a provider.
- 2026-10-14, ADR-013 (baseline): the install baseline runs per app: every `OnInstallAppPerCompany`, then every
  `OnInstallAppPerDatabase` with no company, following the order measured on BC 27.5 and not AL Runner's per-codeunit order.
- 2026-10-14, ADR-013 (baseline): `Company-Initialize` keeps the rows it wrote before it stopped; an install trigger that
  stops is rolled back; the tables they would have written and that hold no row are `unseeded`, and a `Get` that finds
  nothing in dependency code is refused only for those.
- 2026-10-14, ADR-013 (Global Triggers): an event that a dependency subscriber which cannot run (Base Application's
  `GlobalTriggerManagement`, platform tables with no provider) would receive is refused; the setup events skip it while
  the configuration tables it reads are empty. The 73 tests behind `ALT Triggered` stay not run until `API Webhook
  Subscription` and `Database::User` exist.
- 2026-10-14, ADR-013 (baseline): every read of a table the baseline could not fill is refused, not only a missing `Get`.
- 2026-10-14, ADR-013 (Global Triggers): `Session.LogMessage` is a no-op (telemetry is not observable from AL).
- 2026-10-14, ADR-007: an oracle may be recorded for an older corpus commit when the corpus diff since then
  only adds files under `tests/` (checked by the CLI). Since 348a744 all three oracles are on the pin; the rule stays
  for later bumps.
- 2026-10-14, ADR-007: the corpus bump to 348a744 appends one test to an existing file
  (`handlers/TestPageRuntimeContracts.al`, `TestPage_List_Window_IsRepopulatedAroundAGoToKeyOutsideIt`). All three
  oracles are on the pin, so the older-commit check is not involved. A bump that used it would have been refused.
- 2026-10-14, ADR-007: the corpus bump to 6219bf4 (probe batch 3: 35 new cases and 3 new OnPrem cases) only adds 8 files under
  `tests/`. All three oracles are on the pin. No baseline case stopped passing, so nothing was removed.
- 2026-10-14, ADR-007: corpus cases the runtime fails or refuses never turn CI red. Only a baseline case that stops
  passing does. The 70 failing and 120 refused new probe cases (BC 27 and 28) are the work list for the fix rounds.
- 2026-10-14, ADR-018, background worker: a failing guarded `Codeunit.Run` in a worker does not undo the caller's pending rows; a direct `Commit` and an unguarded `Codeunit.Run` there stay refused.
- 2026-10-14, ADR-018, telemetry: `Session.LogMessage` is a no-op in every overload.
- 2026-10-14, ADR-018, SessionSettings: `RequestSessionUpdate(false)` hands a copy to the handler and needs true back; `(true)` and a missing handler are refused.
- 2026-10-14, ADR-018, InitValue: `0DT` and closing-date literals raise in `Init()` of a written record and on the first read of an untouched one.
- 2026-10-14, ADR-018, held branch: the narrower static-subscriber global check is not merged (net zero) until the `DeleteAll(false)` probe is in.
- 2026-10-14, ADR-009, fix-records: a duplicate-key Insert from AL code is deferred to the next table access or Commit only for persistent tables without Blob and AutoIncrement fields; the page client, XmlPort import, temporary records and AutoIncrement tables raise at once.
- 2026-10-14, ADR-009, fix-records: a test refused midway rolls back what it wrote, so a later test of the codeunit cannot see the leftovers.
- 2026-10-14, ADR-009, fix-records: the emit version in "object does not exist" errors is major*1000+25 for BC majors other than 27 (27025 is measured).
- 2026-10-14, ADR-009, fix-records: baseline case 60013 `CalcFields_TableExtensionFlowFieldOnStockkeepingUnit_WithNoMatchingRows_IsZero` removed from all three baselines as a pass by accident (see the commit message).
- 2026-10-14, ADR-009, fix-data: `Query.ColumnNo` is `4001 + 97 * position` (only "not the position" is pinned); the real numbers are a probe to run.
- 2026-10-14, ADR-009, fix-data: `Query.GetFilters` puts the filter of a Text column last (the probe cannot tell "Text column" from "OrderBy column"; the Text reading keeps a baseline test running).
- 2026-10-14, ADR-009, fix-data: an XmlPort import stores with `Insert(true)` / `Modify(true)` through the ordinary record path, so the record rule for duplicate keys applies to it (the earlier `immediate_errors` override for XmlPort import is dropped, as xmlport/TestXmlPortExportImportContracts.al:XmlPortImport_DuplicateKey_... pins a deferral).
- 2026-10-14, ADR-009, fix-data: `SaveAsXml` / `SaveAsJson` of a query write UTF-8 without a byte order mark and the XML in one line (the probe trims and joins the lines).
- 2026-10-14, ADR-020, `al-bcdb`: two crates (core without OS access, command line with it), `pread` instead of a memory map, an approximated culture order for `tables`, bcdb's synthetic `typeprobe` fixtures (8.2 MB) committed, new dependencies `sha2`, `flate2` (zlib-rs) in the core and `rayon` in the command line. Parity with the published bcdb is tested, not assumed (`docs/bcdb.md`).
