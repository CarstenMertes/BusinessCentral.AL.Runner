# ADR-046: The install code of the app under test runs in every company of the install baseline

- Status: Proposed
- Date: 2026-10-19
- Amends: ADR-013 decision 5 (items 1 and 2) and its section "Install baseline and Global Triggers as built"

## Context

Owner decision (2026-10-19): the install baseline runs the install triggers of the app under test, the way real BC
runs them when the app is installed. This ADR says what runs, in which order and in which companies, and what still
needs a corpus probe before it is built.

"App under test" means the apps of the workspace (`lowered.dependency[oi]` is false): the corpus apps in a corpus
run, the APPs given to `alrunner test`. In a run of NP Retail's tests, NP Retail itself is a dependency app and only
its test app is under test.

### What real BC does (pinned by the corpus)

All file references are in the corpus submodule at `db082fc`, under `tests/al-language/` unless a path says otherwise.
Both tests below pass on real BC 27, 28 and 29 (`oracle/27|28|29/junit.xml`, codeunit 69852).

1. **An ordinary codeunit that subscribes to "Extension Triggers" runs at install.** Codeunit 69851 "LT V Subs" has no
   `Subtype` (`events/lt-platform-events-v/PeVFixtures.al:222`). It subscribes to `OnInstallAppPerCompany` and
   `OnInstallAppPerDatabase` of codeunit "Extension Triggers" (`PeVFixtures.al:395-407`). Each subscriber writes a row
   into table 69840 "LT V Event Log" (`DataPerCompany = false`, `AutoIncrement` Id: `PeVFixtures.al:9-23`, written by
   `WriteRow` at `:237-246`). A test finds those rows (`events/lt-platform-events-v/PeVTests.al:284-285`).
2. **The per-company part runs once in each of two companies.** There are 2 `OnInstallAppPerCompany` rows
   (`PeVTests.al:284`). Each row holds `CompanyName()` as the subscriber saw it (`PeVFixtures.al:398-399`). Row #1
   holds the company of the test session, row #2 another company (`PeVTests.al:313-314`). So the corpus CI database
   has two companies, and the install code runs in each with `CompanyName()` set to that company.
3. **The per-database part runs once** (`PeVTests.al:285`).
4. **Order: the session company, then the other company, then the per-database part.** `Verify` compares the
   observations as an ordered list and fails on a missing or an extra one (`_fixtures/ProbeObserver.Codeunit.al:156-189`).
   So the two per-company rows have the Ids 1 and 2, and with 3 rows in all (`PeVTests.al:298`) the per-database row
   has Id 3. AutoIncrement gives each insert the next number (`record/TestAutoIncrementExplicitKey.al:50-51`). This is a
   corpus pin, for subscribers of one app, of the order the probe agent measured for install codeunits
   (`docs/probes-needed.md:28`, not a corpus test).
5. **No upgrade, environment-copy or user-initialization event runs at install** (0 rows each, `PeVTests.al:286-296`).
6. **The test session does not see the SingleInstance state of the install run.** The subscribers also add to a list
   in a SingleInstance codeunit (`PeVFixtures.al:133-135`, `:248-252`). In the test session that list holds only the
   two Company Triggers entries (`PeVTests.al:297`).
7. **The test company is "CRONUS International Ltd."** (`session/TestEnvironmentUserTenantContracts.al:253`).
8. **For install codeunits (already pinned):** the per-company trigger runs in the test company and finds its Company row
   (`install/TestInstallEnvVisible_Tests.al:39-58`); the test company holds exactly 2 per-company seed rows and the
   database exactly 1 per-database seed row (`install/TestInstallTriggerSeed_Tests.al:58-59`). Two companies do not
   change these: "Install Seed" is per company and "Install Seed Database" is written only by the per-database trigger.

### What the runtime does today

- `baseline.rs` runs, once per run: each dependency app's install codeunits (all per-company triggers in the session
  company, then all per-database triggers with no company, per app, `baseline.rs:112-127`), then `Company-Initialize`,
  then the workspace apps' `Subtype = Install` codeunits (`baseline.rs:210`, through `run_install_codeunits`,
  `runtime/blocked.rs:8`). Without a baseline (skipped by the rule, `AL_NO_BASELINE`, or no dependency root,
  `baseline.rs:139`) the workspace installs run per test codeunit instead (`runtime/run.rs:229-237`).
- Everything runs in one company. The Company table holds one row.
- Nothing raises the events of "Extension Triggers" (2000000010). Its subscribers never run.
- `run_install_codeunits` groups all workspace apps together: every per-company trigger of every workspace app, then
  every per-database trigger. Dependency apps are handled per app.

Result on BC 29 today: `EventLog_InstallRowCompany` fails ("observation 1: expected [install per company #1 info is the
company of the test session = Yes] but was []") and `EventLog_RowsWrittenBeforeTheTestSessionStarted` fails ("expected
[rows OnInstallAppPerCompany = 2] but was [rows OnInstallAppPerCompany = 0]").

## Decision

1. **Install code of an app** is its `Subtype = Install` codeunits' `OnInstallAppPerCompany` and
   `OnInstallAppPerDatabase` triggers, and its subscribers to the same two events of "Extension Triggers" (fact 1). This
   holds for every app. Microsoft's apps have no such subscribers outside install codeunits (see Cost), so for them
   nothing changes. A look-alike procedure on a normal codeunit is still not install code
   (`install/TestInstallTriggerSeed_Tests.al`, `TestInstall_NonInstallCodeunitDidNotAutoRun`).
2. **The steps of the install baseline**, once per run, on one fresh session:

   | Step | What runs | Companies | `CompanyName()` |
   |---|---|---|---|
   | 1 | Seed the platform rows (ADR-013 decision 5 item 2): one Company row for each company of the run (new: also the second company), the User, the SUPER Access Control row | - | - |
   | 2 | Each dependency app, in dependency order: its per-company install code, then its per-database install code (unchanged) | session company only (decision 4); per-database with no company | the session company; empty |
   | 3 | `Company-Initialize` (unchanged) | session company only | the session company |
   | 4 | Each app under test, in dependency order, one app complete before the next: (a) its per-company install code once in each company of the run, the session company first; (b) its per-database install code once | every company of the run; per-database with no company | the company being installed; empty |
   | 5 | Capture the baseline (ADR-004), as today | - | - |

   Within 4a and 4b the routines run in object order, as today (the order between an install codeunit's trigger and a
   subscriber is probe P6). Each routine commits when it ends (as today). Step 4 changes today's grouping of all
   workspace apps into one per app.
3. **The companies of the run** are a host fact next to the company name (`al-host` `Identity`, an added method with a
   default): the session company first, then the other companies. The default is the session company plus the second
   company that probe P1 names. Until P1 lands there is no second company (build step B waits for it). The list is part
   of the baseline cache key, next to `company_name` (`runtime/corpus.rs:1151-1152`).
4. **Dependency apps stay in the session company.** What the second company holds from the dependency apps and
   `Company-Initialize` depends on how and when that company was created, which no corpus test pins (probe P3). So in
   the second company:
   - a per-company table of an app under test is answered normally. It is new at that app's install, so its rows are
     exactly what install code and tests wrote there (consistent with the exact counts at
     `install/TestInstallTriggerSeed_Tests.al:58-59`);
   - a read or write of any other per-company table (dependency tables, platform tables that are per company, and the
     rows of a test-data backup's second company) raises `Signal::Unsupported` naming the table and the company. This
     replaces, for the second company only, the narrower rule "refuse tables the install baseline wrote rows to"
     (`docs/probes-needed.md:201`);
   - per-database tables are shared and behave as today.
5. **A root that stops** is rolled back, as today. In the second company that touches only the second company's
   rows and the shared tables. The tables it would have written (`tables_written`, static closure) become unseeded
   per company: a per-company table only in the company where the root stopped, a per-database table everywhere.
   `Engine::unseeded` gets a company in its key (an internal change).
6. **One function runs step 4** for both paths: the baseline (after step 3) and the run without a baseline
   (`runtime/run.rs:229-237`). A skipped baseline and a run baseline give the same rows. This is already true for the
   install codeunits; the subscribers and the second company join that function.
7. **One session for every root**, as today; the test session does not see its SingleInstance state (fact 6). Whether
   BC uses one session per company is probe P8.
8. **Until build step B, a partial answer is refused.** With one company, a per-database table that per-company
   install code of an app under test writes holds fewer rows than on BC (here: 1 of 2 `OnInstallAppPerCompany` rows).
   Build step A marks such tables unseeded (hard rule 4), so the two PeV tests become not run instead of failing.
   Step B removes this rule.

### Build order

Each step is built only after the probes it names have landed and the submodule and oracle are bumped (owner,
2026-10-19).

- **Step A** (waits for P4, P6, P7): decision 1 for every app, step 4 per app in the session company, decision 6,
  decision 8, and the two consumers below (demand roots, impact points). Expected: no baseline case changes; the two PeV
  tests become not run.
- **Step B** (waits for P1 and P8): the second company (decisions 3, 4, 5); decision 8 goes. Expected:
  `EventLog_InstallRowCompany` passes on 27, 28 and 29.
- **Not in this ADR:** `EventLog_RowsWrittenBeforeTheTestSessionStarted` also needs Company Triggers `OnCompanyOpen`
  and `OnCompanyOpenCompleted` raised at the start of the test session (`PeVTests.al:297`). No code raises them today.
  That is an additive change with its own pins.

## Fit with the install-baseline skip rule

- **The rule needs no new case.** `observe.rs` skips the baseline only when no selected test can read the data. All new
  data (rows of the apps under test in both companies, the second Company row, the AutoIncrement counter of a shared
  table, the unseeded set) is reached only through a record, `RecordRef`, query, page, report or an event publisher.
  Each of these already says "may observe". `CompanyName()` still answers the session company. The module comment
  ("the data a session holds after the dependency apps' install code ran") is updated to name the apps under test and
  the companies.
- **The skip path stays correct** because of decision 6: when the baseline is skipped, step 4 runs per test codeunit
  through the same function.
- **Checks after each build step:** `tools/baseline_rule_check.sh run` shows 0 differences (CLAUDE.md), and
  `tools/demand_vs_eager.sh run` shows no difference.
- **Demand roots.** Workspace objects are checked anyway. A dependency app's "Extension Triggers" subscribers become
  roots in `reach_baseline` (`al-sema/src/check/demand.rs:351-377`) next to its install triggers. No Microsoft app has
  one, so the Microsoft compile does not change.
- **Impact selection (ADR-043).** `impact_static.rs:885` and `impact_trace::impact_points` find install code by trigger
  name in install codeunits. They must also list "Extension Triggers" subscribers, because a partner extension can
  install through them.

## Cost

**Microsoft app runs.** Measured on 2026-10-19 with a scan of every `*.Source.zip` (206 apps) under
`Applications/` of the BC 27.5.46862.48827 sandbox artifact:

- No Microsoft app subscribes to "Extension Triggers" outside an install codeunit. Decision 1 adds no root to a Microsoft app.
- The 35 BaseApp test apps (`Applications/BaseApp/Test/Tests-*`) have no install code. In those bucket runs step 4 runs
  nothing, so step B adds only one Company row: no compile cost, no added baseline time. Today the baseline takes 0.08
  to 0.15 s per run in the bucket logs (for example "16 of 17 install roots ran to their end; 14 tables hold rows, 1 are
  unseeded, 0.08 s"), against about 300 s of tests.
- Five Microsoft test apps have one per-company install trigger each: Bank Account Reconciliation With AI Tests,
  Business Foundation Tests, E-Document Core Tests, Sales Lines Suggestions Tests, Sustainability Copilot Tests. Run as
  apps under test, each trigger runs once more, in the second company. Where it reads a Microsoft per-company table
  there, it stops (decision 4) and only the shared tables it writes become unseeded.
- Dependency apps: no change (decision 4).

**Partner runs.** NP Retail's 14 install codeunits are dependency roots in its run: unchanged. Its test app (1 install
codeunit) runs its per-company trigger once more.

**Corpus.** The corpus app has six per-company install routines (five install codeunit triggers and the PeV
subscriber); each runs once more. Of the tables they write, only "LT V Event Log" is shared between companies, so
decision 8 touches only the two PeV tests. The baseline cache (ADR-022) keeps the result per key.

## Not pinned yet: probe questions

Corpus rule for all install probes: install code must never fail, or the app does not install on any BC version
(`.github/scripts/check-install-triggers.py:1-7`). A probe writes with `if Insert() then;` into a `DataPerCompany = false`
log and a test reads the log, as `PeVFixtures.al:237-252` does. New probes use "Extension Triggers" subscribers; a new
`Subtype = Install` codeunit needs a change of the `ALLOWED` list (`check-install-triggers.py:13`). Corpus issue 647
items 1 to 3 ask related questions for install codeunits.

- **P1. Which companies does the corpus CI database have?** Their number, `Name` and `Display Name`, in the Company
  table's key order, on BC 27, 28 and 29. Check the Windows nightly too, because the company list is a fact of the
  database the container starts from. Evidence so far, not a pin: the probe agent measured "CRONUS International Ltd."
  then "My Company" (`docs/probes-needed.md:29`), and the demo backup holds a second company "My Company"
  (`docs/probes-needed.md:215`). Decides: the default of decision 3. Blocks step B.
- **P2. In which order do the companies get the per-company install code: the session company first, or the Company
  table's key order?** In the corpus CI the session company also sorts first by name, so `PeVTests.al:313-314` cannot
  tell the two apart. The corpus CI cannot vary its companies, so this needs another CI setup (a test company that does
  not sort first), or stays a documented assumption. It matters only for a run whose session company does not sort first
  (Microsoft's UnitTest lane uses "Empty Company") and whose app under test has per-company install code. Does not block
  A or B.
- **P3. What does the second company hold before the corpus app's install code runs?** From a test: `ChangeCompany` (or
  `RecordRef.Open(.., false, Company)`) to the other company and count the rows of Company Information (79), General
  Ledger Setup (98) and one per-company table that System Application's install code fills. Inside the per-company
  subscriber: the same counts, logged when it runs in the second company. Decides whether decision 4 can answer
  dependency tables in the second company, and whether dependency install code should run there (Options, item 1).
- **P4. Does an "Extension Triggers" subscriber run only at the install of its own app, or also when a later app is
  installed?** Put subscribers into `al-language-internals-fixture` (published before `al-language`, `ci.yml:207-208`)
  that log into a table of that app; a test in `al-language` counts the rows. 2 + 1 means its own install only. Blocks
  step A.
- **P5. Order across apps:** do all companies and the per-database part of the fixture app's install end before
  `al-language`'s install begins? The same probe as P4, with both apps writing one shared AutoIncrement log (al-language
  depends on the fixture app, so it can write the fixture's table). Confirms step 4's "one app complete before the
  next".
- **P6. Order within one app** between an install codeunit's `OnInstallAppPerCompany` trigger and an "Extension
  Triggers" subscriber, and among two subscriber codeunits (object id order or another order). Both write one shared
  AutoIncrement log. Blocks step A.
- **P7. What does `CompanyName()` answer inside an "Extension Triggers" `OnInstallAppPerDatabase` subscriber, and what
  does a per-company table access there do?** `PeVFixtures.al:405-406` writes a constant, not `CompanyName()`. For install
  codeunits it was measured (empty; `docs/probes-needed.md:28`), not pinned; corpus issue 647 item 1 asks for the error
  text. Log `CompanyName()` into `Info`; wrap the table access in a TryFunction and log `GetLastErrorText` and
  `GetLastErrorCode`. Blocks step A.
- **P8. Session facts of the install run.** Do the three rows carry the same `SessionId()`? The column is written
  (`PeVFixtures.al:244`) but no test asserts it; `RowsOfSession` (`PeVTests.al:250-257`) is not used. Does a SingleInstance
  codeunit keep its state from the session company's run into the second company's run and the per-database run (log
  `Log.Early()` into `Info`)? What does `Session.GetExecutionContext()` answer inside a subscriber (pinned only for install
  codeunit triggers, `install/TestInstallEnvVisible_Tests.al`)? Blocks step B.
- **P9. Which companies exist in each lane of Microsoft's CI** (Legacy, UnitTest, IntegrationTest, Uncategorized)? Not
  a corpus probe: read it from BCApps' AL-Go setup or a run on a Windows container. Until then the recipe lanes get the
  same default as every other run, and the Microsoft buckets are run before and after step B; any changed outcome is
  reviewed before merge.

Not decided here, to be measured in step A: dependency apps' per-company install code also writes per-database tables.
On BC that code ran once per company that existed at its install, here once. Microsoft's install code usually guards
its inserts with a `Get`, so the rows may match, but this is not checked. Step A counts these tables (`tables_written`
over the dependency roots, per-database tables only) and reports the number to the owner.

## Options considered

1. **Dependency apps and `Company-Initialize` also run in every company.** This would give the second company setup data,
   so the app under test's install code could read it there. Not chosen now: what the second company holds on real BC
   depends on when it was created (P3), and running `Company-Initialize` there would guess at it. Cost: the per-company
   part of the baseline about doubles (BC 27.5: 3 install triggers in Base Application, 5 in System Application, 1 in
   System Application Test Library, plus `Company-Initialize`, the longest root; the whole baseline of a corpus run
   on BC 29 took 2.68 s today, 18 of 18 roots to their end). Return to it after P3.
2. **A placeholder name for the second company now.** Any name other than the session company would make
   `EventLog_InstallRowCompany` pass. Rejected: the Company table and `CompanyName()` inside install code show the
   name, and nothing pins it (hard rule 3).
3. **Raise "Extension Triggers" to every subscriber of every app at every install.** Not chosen until P4: it would run
   a dependency app's subscribers again at each later install.
4. **Run the second company only when a test can read it.** Rejected: the cost is small (see Cost) and the baseline
   cache keeps the result.

## Consequences

- `EventLog_InstallRowCompany` can pass after step B. `EventLog_RowsWrittenBeforeTheTestSessionStarted` also needs the
  Company Triggers change.
- The Company table holds two rows. `ChangeCompany` to the second company works, and decision 4 refuses what is not
  known there.
- A partner extension that installs through "Extension Triggers" gets its install data, which today it does not.
- Two workspace apps with install code now run one after the other, not interleaved. No corpus app other than
  `al-language` has install code, so no corpus outcome changes from this.
- `docs/probes-needed.md` rows 28 and 29 change when the probes land: the order for subscribers of one app is now
  pinned (fact 4).

## Conformance

- Corpus: after step A, `EventLog_InstallRowCompany` and `EventLog_RowsWrittenBeforeTheTestSessionStarted` are not run;
  after step B, `EventLog_InstallRowCompany` passes on BC 27, 28 and 29. The install tests stay passing:
  `install/TestInstallTriggerSeed_Tests.al`, `TestInstallEnvVisible_Tests.al`, `TestInstallEventDispatch_Tests.al`,
  `TestInstallStartSession_Tests.al`. `--baseline-diff` shows no baseline case lost.
- Runtime tests in `al-testrun/src/runtime/tests.rs`, next to the install order tests: an "Extension Triggers"
  subscriber of a workspace app runs per company, then per database; a look-alike procedure on a normal codeunit does
  not run; with two companies, each company holds its per-company rows and a shared AutoIncrement table counts 1, 2, 3
  in the order of fact 4; two workspace apps install one after the other; a root that stops in the second company rolls
  back only there and marks its tables unseeded in that company; a dependency per-company table is refused in the
  second company; the baseline path and the path without a baseline give equal rows.
- `tools/baseline_rule_check.sh run`: 0 differences. `tools/demand_vs_eager.sh run`: no difference.
- Microsoft buckets before and after step B (at least Tests-Bank and one of the five test apps with install code): the
  outcomes are listed in the pull request.
