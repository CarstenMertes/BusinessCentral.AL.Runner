# ADR-043: Impact test selection: which Microsoft tests a partner extension can change, run with and without it

- Status: Accepted (owner, 2026-10-18)
- Date: 2026-10-18

## Decision in short

1. `alrunner test --impact-of APP [MS-TEST-APPS...]` runs Microsoft's tests without and with a partner extension and
   reports, per test, what changed and which extension code ran.
2. The **impact points** of the extension (event subscribers, table/page/report/enum extensions, interface
   implementations, install-time code) are read from the compiled program, not from text.
3. Tests are selected **dynamically by default**: a per-test index of what each test raised and touched, recorded once
   in a baseline run per BC build, country and company-data lane, and cached like the compile cache. The same baseline
   run supplies the "without" outcomes, so an impact run only runs the selected codeunits once, with the extension.
   The index is never built implicitly; `alrunner impact-index build` builds it (owner, 2026-10-18).
4. **Static selection** (per-test reach over the call graph) is used when no index is cached, with a warning that
   says what it misses and the exact command that builds the index. It also covers tests the baseline could not run
   and cross-checks the index. With an index cached, dynamic selection is the default.
5. **First step:** no index and no static analysis. Run the given test apps twice, diff the outcomes, and name the
   extension routines each test entered. Selection then only makes this cheaper; the report stays the same.

## Context

Owner goal (2026-10-18): run Microsoft's tests out of the box, and for a partner who extends Business Central (for
example an event subscriber in sales posting), "any test that touches that event publisher, the runtime will be able
to identify those ... so I can rerun Microsoft tests and see if my customization breaks any of the custom or the
default logic that Microsoft tests cover."

What exists today:

- `alrunner test` runs Microsoft's test apps (32 Base App buckets, 40300 tests). Full run on main, 2026-10-18:
  26994 pass (14811 of them flagged: they pass through an unprobed assumption), 1069 fail, 12237 not run; 441 s wall,
  up to 10 GB.
- The compile cache (ADR-022) keeps Microsoft's apps as layer 1 (a complete compile) and every other app as its own
  layer. A layer that extends a Base App object relowers that object (`provided::relower_set`). So a run with an
  extension costs the extension's layer, not a new Base App compile.
- Demand-driven reach (al-sema `check/demand.rs`) knows the implicit edges BC adds: a table written runs its triggers
  and the subscribers to its events; an interface or enum used runs its implementers; `Codeunit.Run` on a literal runs
  `OnRun`; `RunObject` actions. But it computes one reach for the whole compile, not one per test. The reach report
  (`al-testrun/src/reach.rs`, `AL_REACH_REPORT=1`) also works on the whole compile.
- `--coverage` counts statements per test for the given APPs (`CoverageState::coverage_counts`). Dependency routines
  carry source positions since ADR-041.
- Event raise goes through `dispatch::publish` -> `raise` -> `event_targets` (al-vm). Table events, Global Triggers
  events and page events build their `EventCall` in their own modules.
- The Microsoft recipe (lanes, disabled tests, company data per lane) is queued as backlog item ms-recipe-3.
  `--disabled-tests DIR`, `--parity`, `--company-data package` and `--country` exist.
- ADR-016 deferred AL Runner v2's `--affected` (rerun only tests an edit can affect) because a v3 run is fast. This
  ADR is not that feature. `--affected` hides run cost for one's own tests. Impact selection answers a different
  question: which of Microsoft's tests cover my change, and did my change alter their result. The diff report is the
  product; selection only lowers its cost.

## Decision

### 1. Impact points of an extension

An impact point is a place where Base App code (or the platform) runs, shows or reads something the extension adds.
They are listed from the compiled program (`Workspace` object table, `events::Registry`, the IR of the extension's
objects), for every app given with `--impact-of` and every non-Microsoft app those apps depend on.

| Impact point | Found from | Selected by (index key) |
|---|---|---|
| Event subscriber to a Base App or System App publisher (integration, business, trigger events) | `EventSubscriber` attributes in the IR (`events::Registry::build`); publisher object and event name | publisher `(object, event)` raised |
| Subscriber to a table trigger event (`OnBeforeInsertEvent`, `OnAfterValidateEvent` ...) | same, with table and, for validate events, field | table and operation (insert/modify/delete/rename, run-trigger flag) or `(table, field)` validated |
| Subscriber to `Global Triggers` events (`OnDatabaseInsert`, `GetDatabaseTableTriggerSetup` ...) | same | every table written; broad, see 2.4 |
| Subscriber to page events, report events | same | page opened / report run |
| Table extension: trigger extensions (`OnBeforeInsert`, `OnAfterModify`, field `OnBeforeValidate`/`OnAfterValidate`) | trigger routines of the `TableExtension` object | table and operation, or `(table, field)` validated |
| Table extension: new fields, keys, field property changes | `TableExtension` members (`members::build`, fields merged) | table read or written (fields with `InitValue` change `Init`) |
| Page extension: triggers, actions, control property changes (`Visible`, `Editable`, `Caption`) | `PageExtension` object | page opened (normal run or `TestPage`) |
| Report extension: dataset columns, triggers, request page | `ReportExtension` object (ADR-025) | report run |
| Enum extension: new values; values that implement an interface | `EnumExtension` object and its `implements` | enum value list read (`Ordinals`, `Names`, iteration, `case` with an else branch); interface dispatch through that enum |
| Interface implementation used through a Base App enum or variable | `implements` on the extension's codeunits (`Workspace::implementers_of`) | `(interface, implementer)` dispatched |
| Install-time code: `Subtype = Install` codeunits, subscribers to company-initialize events | Install codeunits; subscribers whose publisher runs in the install baseline (`baseline.rs`) | every table the install baseline writes with the extension and not without it (`baseline::tables_written`), matched against tables a test reads |
| Manual subscribers (`EventSubscriberInstance = Manual`), procedures only the extension calls | IR | not impact points of their own; they run only through one of the above |

Not impact points: upgrade codeunits (an impact run installs fresh), permission set extensions (the runtime has no
permission checks), entitlements.

Data-driven dispatch (a setup row that names a codeunit or report id, such as Report Selections) is covered by the
install-time row: if the extension's install code writes the row, every test that reads that table is selected.
Setup the partner creates by hand in production is out of reach of any selection; see open question 3.

### 2. Selecting the tests that reach an impact point

#### 2.1 Unit of selection

Tests of one codeunit share state (`Initialize` with `IsInitialized`, isolation per codeunit). The runner already
runs a selected codeunit whole. Selection therefore picks tests, but the run takes their codeunits whole. The report
lists the selected tests first. An unselected test in a run codeunit whose outcome changed is a selection miss and is
reported as such (section 3); it should never happen, and when it does it is a runtime bug to fix.

#### 2.2 Dynamic selection (default)

A baseline run of the Microsoft test apps without any partner app records, per test, the sets of:

- publishers raised `(object, event)`, including raises with no subscriber;
- table operations `(table, op, run-trigger)` and fields validated `(table, field)`, including through `RecordRef`;
- tables read;
- pages opened and reports run;
- enum value lists read and interface dispatches `(interface, implementer)`.

Each set is a sorted list of interned `u32` keys, plus the test's outcome, error text hash and flagged flag.
Recording is off unless asked for and costs one branch per raise and per record operation when off, like
`coverage_counts`. Recording must happen at the publisher call itself, before the subscriber lookup, so a raise with
no subscriber is still recorded. Estimated size: 40300 tests times a few hundred keys, compressed a few MB
(to be measured in step 2).

**Why it is exact.** The runtime is deterministic (fixed clock per run, deterministic `CreateGuid`). If a test starts
from the same company state and never reaches an impact point in the baseline run, it runs the same instructions
with the extension, so its outcome cannot change. The first impact point a test reaches is the same in both runs,
because the runs are identical up to it. So the index needs only the baseline run, not a run with each extension.
Three conditions keep this true, and the implementation must test each:

1. Outcomes do not depend on which other codeunits are in the run, nor on which other apps are in the compile
   (the selection-independence work, issue "outcomes depend on selection"). Both runs compile Base App the same way:
   the complete compile of layer 1 from the compile cache, never the demand-driven compile, which reaches more or
   less Base App code depending on the extension.
2. The company state before each test is the same, except for what install-time impact points cover.
3. The list of impact points in section 1 is complete for what the runtime supports. A kind of extension object the
   list does not cover makes the impact run refuse with a reason (hard rule 4), not run with a guessed selection.

**Limits.** A test refused or stopped in the baseline records only what it reached before it stopped. Such a test is
reported as "not verified" (section 3), with the static verdict where one exists.

#### 2.3 Static selection (fallback and cross-check)

From the compiled program: the call graph (`Lowered::calls_of`, `calls_into`, `driven_edges`) plus the implicit
edges demand already knows (record operation on a typed table -> its triggers and table-event publishers; publisher
call -> its subscribers; interface or enum use -> implementers; `Codeunit.Run` and `Report.Run` on a literal ->
the object; `RunObject` actions). Roots per test: the test routine, its codeunit's `OnRun`, the handler functions it
declares, and the test runner's `OnBeforeTestRun` / `OnAfterTestRun`. The search runs backwards, once per impact
point, from the routines that raise or touch it to the test roots (one reverse search per impact point instead of
40300 forward ones).

Static reach over-selects (an interface variable reaches every implementer; a branch that never runs counts) and,
worse, misses what it cannot see:

- `Codeunit.Run`, `Page.Run`, `Report.Run` with an id held in a variable or a table field;
- `RecordRef.Open` with a table number from a variable (dimension management, change log, document attachments);
- `Variant` records, `TransferFields` across tables;
- events the platform raises (`Global Triggers` database events, page events driven by the test client, session
  and company events);
- data-driven dispatch (Report Selections, Job Queue entries).

A test whose reach contains such a site gets the verdict "cannot rule out", not "not reached". Because a missed test
hides a breakage silently, static selection is never the default. Uses:

- **No cached index (owner, 2026-10-18).** The impact run does not build the index. It falls back to static
  selection and both sides run on the selected and "cannot rule out" codeunits. It prints a warning before the run
  and again above the summary, for example:
  `no impact index for BC 27.5.46862.48827, country us, lane CRONUS: tests were selected by static reach, which
  misses Codeunit/Page/Report.Run with an id from a variable or a table, RecordRef on a table number from a
  variable, events raised by the platform and setup-driven dispatch; N selected tests reach such a site
  ("cannot rule out") and were run. Build the index once with: alrunner impact-index build <the same test apps
  and flags>`. The command in the warning repeats the run's test apps, `--country` and data flags exactly.
- `--impact static`: the same fallback on request, even when an index exists.
- For tests refused in the baseline: the report adds "static: reaches impact point X" or "static: cannot rule out".
- Cross-check in our own CI: a test the dynamic index selects and static reach does not is a bug in static reach.

#### 2.4 Broad impact points

A `Global Triggers` subscriber or an install-time write to a common table (General Ledger Setup) selects most tests.
The report says so up front ("this impact point selects 31200 of 40300 tests") and names the point, so the partner
sees why the run is large. The run still happens; nothing is cut to save time.

### 3. The run and the report

1. **Without:** the outcomes from the cached baseline run (same key as the index, section 4). With `--impact static`
   or `--impact all` they come from a fresh run of the same codeunits.
2. **With:** the selected codeunits run once with the extension, with recording of extension code on: which
   extension routines each test entered (from statement counts on the extension's positions, as `--coverage` does).
3. **Report per test:** test, codeunit, outcome without, outcome with, change, the impact points it reached in the
   baseline, the extension routines that ran (for example "subscriber `My Sales Ext.CheckCreditLimit` to
   `Sales-Post.OnBeforePostSalesDoc`, 3 calls"), the error text on each side.

Change classes, in report order:

| Class | Meaning |
|---|---|
| newly failing | passed without, fails with |
| fails differently | failed on both sides, other error text or code |
| newly refused | ran without, refused with (the reason names the extension routine or object) |
| selection miss | changed but not selected (a runtime bug, see 2.1) |
| newly passing | failed without, passes with |
| newly runs | refused without, runs with |
| same, reached | reached an impact point, same outcome: the confidence statement ("412 tests ran your code; all unchanged") |
| not verified | refused on both sides, or stopped in the baseline before it could reach anything; top refusal reasons listed |

Flagged passes keep their flag on both sides.

**Intended changes are the partner's call.** `--impact-accept FILE` reads a list of expected changes (test, outcome
with, error text). An accepted change is listed under its class, marked accepted, and does not fail the run.
`--impact-json FILE` writes the whole diff in a form that can be edited into such a file. Exit code: an unaccepted
newly failing, fails differently, newly refused or selection miss gives exit 1; otherwise the existing codes of
`output::exit_code`. JUnit (`--junit`) is written for the "with" run, with the change class in each test's
properties.

### 4. Command line, recipe, country, cost and caching

```
alrunner test --impact-of APP [--impact-of APP ...] [MS-TEST-APPS ...]
              [--impact dynamic|static|all] [--impact-accept FILE] [--impact-json FILE]
              [--country CC] [--recipe microsoft | --disabled-tests DIR] [existing flags]
```

- `APP` is a folder, `.app` or source zip, read like any APP (`apps::load`). Its non-Microsoft dependencies are
  part of the "with" side and of the impact points. The "without" side holds Microsoft's apps only.
- `MS-TEST-APPS` omitted: every Microsoft test app of the artifact (the 32 Base App buckets and the System App test
  apps the recipe runs). Given: only those, and the index is built and cached per test app.
- `--impact all`: no selection, both sides run everything given. This is step 1 and stays as the reference.
- `alrunner impact-index build [MS-TEST-APPS] [--country CC]` builds the index and baseline outcomes ahead of time
  (for a CI cache step). `alrunner impact-points APP` lists the impact points and, when an index exists, how many
  tests each selects, without running anything.
- **Recipe (ms-recipe-3).** The index and baseline outcomes are per company-data lane (Legacy, UnitTest,
  Integration, Uncategorized). Disabled tests are skipped in an impact run (owner, 2026-10-18): Microsoft does not
  run them, so they are noise for a partner. (Our own development runs still run them, as decided 2026-10-17.) Until
  ms-recipe-3 is built, impact runs use the CRONUS backup lane and `--disabled-tests` as today.
- **Country.** `--country CC` (flag only, default w1) picks the localized Base App and test apps. It is part of the
  cache key; a partner on the US localization builds a US index once.
- **Cache key** of index and baseline outcomes: BC build, country, lane, recipe version (disabled list hash), hash of
  each Microsoft test app, and the runtime build (executable hash, as ADR-022). A new runtime build changes refusals
  and outcomes, so it rebuilds the index. Stored under `<cache dir>/impact/<key>/`, written by temporary file and
  rename like `compile_cache::write_entry`.
- **Cost.** Index build: one full run of the chosen test apps with recording on (today about 441 s and up to 10 GB for
  all 32 buckets on the owner's machine; recording overhead to be measured, target under 10 %). Impact run with an
  index: the extension's compile layer (seconds, ADR-026) plus the selected codeunits once. Without an index the
  run falls back to static selection and runs both sides on its selection (2.3); it never builds the index itself.

### 5. What blocks it today

- **Refusals.** 12237 of 40300 Microsoft tests are not run (30 %). For a partner each is "not verified". The report
  must show this plainly; the number shrinks with the language work. Impact runs make it visible per area
  (sales posting may be better or worse than the average).
- **Flagged passes.** 14811 passes rely on unprobed assumptions. A "same" result on a flagged pass is weaker; the
  flag stays visible.
- **Data recipe.** Most of the 1069 failures are data setup (ms-recipe-3). A test that fails in the baseline before it
  reaches the posting routine cannot show a change there. Same mechanism as refusals: it lands in "not verified" when
  it reached no impact point.
- **Partner code itself.** The extension's own code may hit refusals (HTTP, `DotNet`, platform tables). These show as
  "newly refused" with the reason, which is correct but noisy until the runtime covers more.
- **Selection independence.** Condition 1 of 2.2 must hold and be tested before dynamic selection is trusted.
- **Missing pieces:** per-test recording of raises and touches; per-test static reach; the diff report; the index
  cache.

### 6. Staged plan

1. **Two runs and a diff (smallest useful step).** `--impact-of APP` with `--impact all` as the only mode: run the
   given test apps without and with the extension, classify per test (section 3), and name the extension routines
   each test entered (statement counts on the extension's positions). No index, no static reach. Cost: twice the
   chosen test apps. A partner can already answer "did my subscriber break Tests-ERM" today. Includes the
   selection-independence test (condition 1).
2. **Dynamic index.** Recording in al-vm (additive `Session` state, like `CoverageState`), the index format and cache,
   `impact-index build`, `impact-points`. The "with" side runs only selected codeunits; the "without" side comes from
   the cache. Measure recording overhead and index size.
3. **Accept file and JSON diff** (`--impact-accept`, `--impact-json`), exit codes, JUnit properties.
4. **Recipe and country.** Lanes and disabled tests from ms-recipe-3 in the cache key; `--country` index.
5. **Static reach.** Per-impact-point reverse search, verdicts for refused tests, `--impact static`, and the
   dynamic-vs-static cross-check in our CI.
   Until this step lands, an impact run with no cached index stops with the warning of 2.3 (what is missing and
   the exact `impact-index build` command) and offers `--impact all`; it does not build the index.

Steps 1 to 3 need no new seam. Recording is additive `Session` state; the report is in `al-testrun` and
`alrunner-cli`. Step 2 adds a cache file format, which is internal tooling (ADR-009: free to improve).

## Alternatives considered

- **Run everything twice, always.** Simple and exact. Twice the full run (about 15 minutes, 10 GB) for every
  change. Kept as `--impact all` and as step 1; not the default once an index exists.
- **Static selection as the default.** No baseline run. Misses `RecordRef`, ids in variables, platform-raised events
  and data-driven dispatch, which posting code uses. A missed test hides a breakage without any signal, against the
  spirit of hard rule 4. Rejected as default when an index exists; used, with a warning, when none is cached, and
  as cross-check.
- **Select from the "with" run only** (run all with the extension, keep tests that entered extension code, run those
  without). Halves the cost without an index, but misses passive impact points (new enum values, page property
  changes, install-time data) that run no extension code. Used inside step 1 only to name the routines that ran.
- **Text search** (test code that names the extended objects, as v2's `--affected` does with files). Far too coarse:
  posting is reached through library codeunits several calls deep.
- **Index from real BC's code coverage.** Needs a BC service tier per build and country; the runtime exists so that
  this is not needed.

## Consequences

- Partners get a per-test answer to "did my extension change what Microsoft's tests check", with the extension code
  that ran, and a way to mark intended changes.
- The runtime gains an opt-in per-test trace of raises and table, page, report, enum and interface touches. It is
  also useful for `--explain` and for our own reach work.
- The cache gains a second kind of entry (index and baseline outcomes) per BC build, country and lane.
- The value of the feature grows with the pass rate: today 30 % of Microsoft's tests are "not verified".

## Owner decisions (2026-10-18)

1. **No cached index:** fall back to static selection with a clear warning that says what static selection misses
   and exactly what to run to build the index. Do not build the index implicitly. (Section 2.3.)
2. **Disabled tests:** Microsoft's disabled tests are skipped in impact runs.
3. **Partner setup data:** comes from the extension's install code for now. No `--impact-setup` flag.
4. **Prebuilt index:** not published for now; local and CI cache only.

## Conformance

Planned, per step:

- Step 1: an `alrunner-cli` test with a small extension (corpus-style AL, not Microsoft code) that subscribes to a
  publisher in a dependency app: one test newly failing, one same-reached, one unreached; the report classes and the
  named subscriber are asserted. A selection-independence test: the same codeunit's outcomes alone and in a full run,
  with and without an unrelated app in the compile.
- Step 2: a test that the index of a run equals the raises and touches the test made (including a raise with no
  subscriber and a `RecordRef` write); a test that the "with" run of the selected codeunits gives the same classes as
  `--impact all` on the same apps (no selection miss).
- Step 5: the dynamic-vs-static cross-check over a Microsoft bucket in CI: every dynamically selected test is
  statically selected or "cannot rule out".

## Step 1 as built (impact-1, 2026-10-18)

`alrunner test APPS --impact-of APP --impact all` (`crates/alrunner-cli/src/impact.rs`). The "without" side runs the
test codeunits of the APPs; the "with" side gives the extensions as further APPs, counts their statements per test
(`--coverage` machinery), and runs the same test codeunits (the extensions' own test codeunits are not selected).
Disabled tests are skipped when `--disabled-tests` is given. Tests: `crates/alrunner-cli/tests/impact.rs` (fixture
apps in `tests/fixtures/impact`, including the selection-independence test).

### Decided without owner review (2026-10-18)

1. `--impact` must be given as `all`; without it the run stops (exit 2) and says that `dynamic` and `static` are not
   built yet. `--no-compile-cache` is refused (both sides must compile the dependencies completely, condition 1).
2. A ninth class, "same, not reached", counts unchanged tests that entered no extension code. Step 1 knows only the
   extension code a test entered, not passive impact points; "same, reached" means "extension code ran". Unchanged
   unreached tests are counted, not listed; "same, reached" lists 30 unless `--verbose`.
3. Exit code: 1 when a test is newly failing, fails differently or is newly refused; otherwise 0, or the code of a
   side that selected nothing or stopped. `--junit`, `--output-json`, `--coverage*` and `--explain` are refused with
   `--impact-of` until step 3.
4. "Calls" of a routine are the runs of its first statement; a routine with no statement (empty body) is not seen.
5. A failure the runner does not trust (an earlier test of the codeunit did not run) is "newly refused"; the report
   prints the failure text under it.

## Step 2 as built (impact-2, 2026-10-18)

- **Recording** (`al-vm/src/impact.rs`, additive `Session` state in `CoverageState`): `enable_impact_trace`,
  `take_impact_trace` -> `Touches`. Noted: every event raise at its raise, before the subscriber lookup (publisher
  routines in `dispatch::publish`, the ten table events in `records::table_event`, the page, page-field and
  page-action events, `OnResolveCaptionClass`); every record operation's table (`records::record_op`, which typed
  records, `RecordRef` and the page client go through) and whether it wrote; every page instance
  (`pages::page_instance`); every report schema read (`reports::report_schema`). Off, each site costs one branch.
- **Keys** (`al-testrun/src/impact_trace.rs`): objects by kind and id, never by program index:
  `event <kind> <id> <event>` and `.../<element>`, `table <id>`, `write <id>`, `page <id>`, `report <id>`. The
  runner records per test (the first test of a codeunit also holds what ran before it) and, with the trace on, runs
  the install baseline instead of reading it from the compile cache, so the index also holds what the install code
  raised and touched.
- **Impact points** (`impact_points`): subscribers of the extension (not manual ones, not to its own publishers) ->
  their event key; a `Global Triggers` subscriber to the table-write events -> any `write`; table, page and report
  extensions -> `table`/`page`/`report` of the base object (fields, keys and triggers alike: over-selects a little).
  Not covered, so the run takes every test codeunit with a warning that names them: enum extensions, install
  codeunits, subscribers to events whose raises are not noted, profiles and other kinds. New objects, interfaces and
  permission sets are no impact points of their own.
- **Index** (`alrunner-cli/src/impact_index.rs`): gzip JSON, `format: 1`, at `<cache>/impact/<key>/index.json.gz`.
  It holds the compiled apps (id, name, version), the key table, the baseline's keys, and per test: app, codeunit,
  name, outcome with its text, assumption ids, key numbers (delta coded). The key hashes the runtime build, the test
  apps' source, `--country`, `--bc-version`, the company and test data options, `--isolation`, `--define`, the
  environment options and the `--disabled-tests` files. `alrunner impact-index build APP... [options]` runs every
  test (selection flags are refused) and writes it.
- **Impact run** (`impact.rs`): with no `--impact`, or `--impact dynamic`, and an index cached: the "with" side is
  compiled, `decide` lists the impact points and, through `RunOptions::narrow` (a callback between compile and run),
  runs only the codeunits with a test that reached one; the "without" outcomes come from the index. Tests in run
  codeunits that reached nothing and whose outcome changed with no reached test before them in their codeunit are
  the class **selection miss** (exit 1). Unselected codeunits are counted on a `not selected:` line. `--impact all`
  is step 1. No index: step 1 behavior with the warning, printed before the run and above the summary.

### Decided without owner review (2026-10-18, impact-2)

1. No index cached: the run does what `--impact all` does, with the warning, because static selection (step 5) does
   not exist yet. Section 6 said to stop with the warning; the orchestrator's task asked for the fallback.
   `--impact static` stops with exit 2 ("not built yet").
2. An impact point the index does not cover, or one the install code reached in the index's run (a subscriber, or a
   table extension with triggers on a table the install code used), does not stop the run: every test codeunit
   runs with the extension and the "without" outcomes still come from the index (they are exact for any selection).
3. The index records Microsoft's app versions; when the impact run compiles other versions under the same key, the
   index is stale: the run warns and falls back to both sides.
4. A table extension selects every test that used its table, whatever it adds (fields, keys or triggers); the
   finer keys of section 2.2 (operation, validated field) are recorded for events only.
5. Selection miss: a changed test is not a miss when an earlier test of its codeunit reached an impact point (the
   codeunit's tests share state, section 2.1).
