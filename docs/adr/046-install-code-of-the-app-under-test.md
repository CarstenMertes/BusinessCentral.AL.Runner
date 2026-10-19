# ADR-046: The install code of the app under test runs in the install baseline; several companies only as an opt-in

- Status: Accepted (owner, 2026-10-19, with the `--companies` flag)
- Date: 2026-10-19
- Amends: ADR-013 decision 5 (items 1 and 2) and its section "Install baseline and Global Triggers as built"

## Context

Owner decisions (2026-10-19):

- The install baseline runs the install triggers of the app under test, the way real BC runs them when the app is
  installed.
- The runner runs **one company by default**. It never runs the install baseline again for other companies of the
  database, whatever the database holds.
- **Several companies are an opt-in**, for partner tests that rely on several companies. Under the opt-in the app under
  test's install code runs once per company, session company first, then the per-database part once.
- The opt-in is one flag that lists the companies to run, `--companies <NAME>[,<NAME>...]`, or `--companies all` for
  every company in the database. Without the flag the run has one company, the session company.

This ADR says what runs in each mode, how a test that observes another company is handled in the default mode, how
the corpus run gets the opt-in, and what still needs a corpus probe before it is built.

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
- Everything runs in one company. The Company table holds one row. A read of the Company table, or a call that names
  another company, is answered as if the database held only the session company and the companies the run created.
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

2. **Two company modes.**
   - **One company (the default).** The run has one company, the session company. The install baseline never runs for
     another company, whatever the database holds; a test-data backup with several companies (the demo backup holds
     "My Company" too, `docs/probes-needed.md:215`) is no exception.
   - **Several companies (opt-in).** One flag, `--companies <NAME>[,<NAME>...]`, on `alrunner test` and `al-test-cli`
     (later `companies` in `alrunner serve`, ADR-028). It declares the complete company list of the database, in run
     order. Two forms:
     - **A list of names.** The first name is the session company. The session company is still chosen as today (the
       company restored from the test data, `--test-data-company`, the recipe lane, or the default name); the first name
       must be that company, or the runner stops with a usage error before anything runs. The other names follow in the
       order given. A name given twice is a usage error. A name with a comma cannot be given (none is known to be
       needed).
     - **`all`.** Every company in the database the run starts from: the session company first, then the others in the
       Company table's key order (the order of the opt-in is probe P2). The database the run starts from is the
       test-data backup; a run without test data (`--no-test-data`, the corpus run) starts from an empty database, so
       there `all` gives only the session company, the same as the default.

     The resolved list is a host fact next to the company name (`al-host` `Identity`, an added method whose default is
     the empty list, meaning one company) and part of the baseline cache key, next to `company_name`
     (`runtime/corpus.rs:1151-1152`).

3. **The steps of the install baseline**, once per run, on one fresh session:

   | Step | What runs | One company (default) | Several companies (opt-in) |
   |---|---|---|---|
   | 1 | Seed the platform rows (ADR-013 decision 5 item 2): the Company rows, the User, the SUPER Access Control row | one Company row | one Company row per declared company |
   | 2 | Each dependency app, in dependency order: its per-company install code, then its per-database install code (unchanged) | session company; per-database with no company | the same: session company only (decision 5) |
   | 3 | `Company-Initialize` (unchanged) | session company | the same: session company only |
   | 4a | Each app under test, in dependency order, one app complete before the next: its per-company install code | session company | once per declared company, in list order (session company first) |
   | 4b | The same app's per-database install code, after 4a | once, with no company | once, with no company |
   | 5 | Capture the baseline (ADR-004), as today | | |

   `CompanyName()` is the company being installed in 2, 3 and 4a, and empty in the per-database part. Within 4a and 4b
   the routines run in object order, as today (the order between an install codeunit's trigger and a subscriber is
   probe P6). Each routine commits when it ends (as today). Step 4 changes today's grouping of all workspace apps into
   one per app.

4. **In the default mode, a test that observes another company is refused at run time** (`Signal::Unsupported`, with
   the reason), never answered. "Known companies" are the session company and the companies the run itself inserted,
   renamed or deleted in the Company table (they are handled as today, `docs/probes-needed.md:201`). Three things
   show another company:
   - **a. The Company table.** A `Get` of a known company, and a read whose filters select one known name (a single-value
     filter on `Name`), are answered. Every other read is refused: a `Get` of another name, `Find*`, `Next`, `Count`,
     `IsEmpty`, `CalcSums`, and a FlowField or `TableRelation` check that reads the table. The database may hold more
     companies than the run knows.
   - **b. A call that names a company that is not known.** `Record.ChangeCompany`, `RecordRef.ChangeCompany`,
     `RecordRef.Open` with a company, and every other builtin whose answer depends on whether a named company exists.
     Today such a company is answered as missing. That answer is a guess, because the database may hold it.
   - **c. A shared table that per-company install code of an app under test writes.** On BC it holds one write per
     company of the database, here one. The tables are found before anything runs: the static closure of the per-company
     roots of step 4a (`tables_written`), tables with `DataPerCompany = false` only. They join `Engine::unseeded`, so a
     read whose answer depends on the rows is refused at run time (the existing rule: `Find*`, `Next`, `Count`, `IsEmpty`,
     `CalcSums`, a FlowField, a `TableRelation` miss, a `Get` that finds nothing; a `Get` that finds its row is
     answered). In the corpus this is "LT V Event Log" only, so the two PeV tests are not run in the default mode.

   Why at run time, not statically: the table list of item c is static, but the read decides. A static refusal of every
   test that reaches the Company table or one of those tables would also refuse correct work, such as
   `Company.Get(CompanyName())` (`session/lt-company-record/CompanyRecord.al:20`) or an insert into a shared table. The
   runner reports a refused test as not run, with the reason.

   Under the opt-in the declared list is complete: items a and b are answered from it (a name that is not on it does
   not exist), and item c does not apply, because step 4a ran once per declared company.

   **Not decided:** dependency apps' per-company install code also writes shared tables. By the reasoning of item c it
   shows another company too. Step A counts these tables (`tables_written` over the dependency roots, shared tables
   only) and the Microsoft tests a refusal would stop, and reports both numbers to the owner, who decides.

5. **Under the opt-in, dependency apps stay in the session company.** What another company holds from the dependency
   apps and `Company-Initialize` depends on how and when that company was created, which no corpus test pins (probe
   P3). So in a declared company other than the session company:
   - a per-company table of an app under test is answered normally. It is new at that app's install, so its rows are
     exactly what install code and tests wrote there (consistent with the exact counts at
     `install/TestInstallTriggerSeed_Tests.al:58-59`);
   - a read or write of any other per-company table (dependency tables, per-company platform tables, and the rows a
     test-data backup holds for that company) raises `Signal::Unsupported` naming the table and the company;
   - shared tables behave as today.

6. **A root that stops** is rolled back, as today. Under the opt-in, a stop in another company touches only that
   company's rows and the shared tables. The tables the root would have written (`tables_written`) become unseeded per
   company: a per-company table only in the company where the root stopped, a shared table everywhere.
   `Engine::unseeded` gets a company in its key (an internal change).

7. **One function runs step 4** for both paths: the baseline (after step 3) and the run without a baseline
   (`runtime/run.rs:229-237`). A skipped baseline and a run baseline give the same rows. This is already true for the
   install codeunits; the subscribers and the opt-in companies join that function.

8. **One session for every root**, as today; the test session does not see its SingleInstance state (fact 6). Whether
   BC uses one session per company is probe P8.

### How the corpus run gets the opt-in

The corpus CI database has two companies (facts 2 and 4). It is a fact of the oracle's run, like the Microsoft build
of each major. So `oracle/oracle.toml` gets a top-level `companies` key, set at the corpus bump that brings probe P1
and citing it, in the same way as `bc_build`. It takes the same values as the flag: a list of names, session company
first (`companies = ["CRONUS International Ltd.", "My Company"]` if P1 confirms those names), or `"all"`. Because the
corpus run starts from an empty database, `"all"` gives the corpus only the session company; the corpus needs the
list of names. A `[[version]]` table may override the key if P1 shows that the majors differ. `al-test-cli` runs every
suite of the corpus job with that value as the opt-in; `--companies` on the command line overrides it. The OnPrem
suites run in separate CI jobs of the same artifact (`.github/workflows/ci.yml` in the corpus, jobs `test-onprem` and
`test-onprem-dotnet`); P1 checks their databases too.

Until P1 lands, `companies` is absent and the corpus runs in the default mode: the two PeV tests are not run (item 4c).

The default mode also needs coverage. A new check, `tools/company_mode_check.sh run`, runs the corpus with the opt-in
(the `companies` value of `oracle.toml`) and again with one company (the run ignores `companies`), and compares the JUnit files (like `tools/baseline_rule_check.sh`). A case may differ only by
being not run in the default mode. A case that passes or fails differently is an error. This shows that the default
mode refuses what it cannot know and never answers it differently.

### Build order

Each step is built only after the probes it names have landed and the submodule and oracle are bumped (owner,
2026-10-19).

- **Step A** (waits for P4, P6, P7): decision 1 for every app, step 4 per app in the default mode, decision 7, item
  4c, the two consumers below (demand roots, impact points), and the measurement under "Not decided". Expected: no
  baseline case lost; the two PeV tests become not run.
- **Step B** (waits for P1 and P8): the opt-in (`--companies`, the `Identity` method, `companies` in
  `oracle.toml`), step 4a per declared company, decisions 5 and 6, the default-mode refusals 4a and 4b, and
  `tools/company_mode_check.sh`. Items 4a and 4b come only together with the corpus opt-in, so the corpus cases that
  read the Company table or name a company keep passing. Two baseline cases of that kind: `Company_Names` counts the
  Company table (`session/TestProbeSessionMiscContracts.al:705`), and `TestInstall_CompanyGetConsultsTheKeyDuringInstall`
  reads what the install code's `Comp.Get('NO SUCH COMPANY')` answered (`install/TestInstallSeeder.al:72`,
  `install/TestInstallEnvVisible_Tests.al:64`). Expected: `EventLog_InstallRowCompany` passes on 27, 28 and 29.
- **Not in this ADR:** `EventLog_RowsWrittenBeforeTheTestSessionStarted` also needs Company Triggers `OnCompanyOpen`
  and `OnCompanyOpenCompleted` raised at the start of the test session (`PeVTests.al:297`). No code raises them today.
  That is an additive change with its own pins.

## Fit with the install-baseline skip rule

- **The rule needs no new case.** `observe.rs` skips the baseline only when no selected test can read the data. All new
  data (rows of the apps under test, the Company rows of the opt-in, the AutoIncrement counter of a shared table, the
  unseeded set) and every refusal of decision 4 is reached only through a record, `RecordRef`, query, page, report or an
  event publisher. Each of these already says "may observe". `CompanyName()` still answers the session company. The
  module comment ("the data a session holds after the dependency apps' install code ran") is updated to name the apps
  under test and the opt-in companies.
- **The skip path stays correct** because of decision 7: when the baseline is skipped, step 4 runs per test codeunit
  through the same function.
- **Checks after each build step:** `tools/baseline_rule_check.sh run` shows 0 differences (CLAUDE.md),
  `tools/demand_vs_eager.sh run` shows no difference, and after step B `tools/company_mode_check.sh run` passes.
- **Demand roots.** Workspace objects are checked anyway. A dependency app's "Extension Triggers" subscribers become
  roots in `reach_baseline` (`al-sema/src/check/demand.rs:351-377`) next to its install triggers. No Microsoft app has
  one, so the Microsoft compile does not change.
- **Impact selection (ADR-043).** `impact_static.rs:885` and `impact_trace::impact_points` find install code by trigger
  name in install codeunits. They must also list "Extension Triggers" subscribers, because a partner extension can
  install through them.

## Cost

**Microsoft app runs (default mode).** Measured on 2026-10-19 with a scan of every `*.Source.zip` (206 apps) under
`Applications/` of the BC 27.5.46862.48827 sandbox artifact:

- No Microsoft app subscribes to "Extension Triggers" outside an install codeunit. Decision 1 adds no root to a Microsoft app.
- The 35 BaseApp test apps (`Applications/BaseApp/Test/Tests-*`) have no install code. In those bucket runs step 4 runs
  nothing: no compile cost, no added baseline time. Today the baseline takes 0.08 to 0.15 s per run in the bucket logs
  (for example "16 of 17 install roots ran to their end; 14 tables hold rows, 1 are unseeded, 0.08 s"), against about
  300 s of tests.
- Five Microsoft test apps have one per-company install trigger each (Bank Account Reconciliation With AI Tests,
  Business Foundation Tests, E-Document Core Tests, Sales Lines Suggestions Tests, Sustainability Copilot Tests). In the
  default mode they run in the session company, as today.
- The refusals 4a and 4b can turn Microsoft tests that scan the Company table or name another company into not run.
  The buckets are run before and after step B, and the pull request lists every changed outcome. A recipe lane can use
  the opt-in (`--companies` with a list, or `all` on a lane that restores a backup) once its companies are known (P9).

**Partner runs.** NP Retail's 14 install codeunits are dependency roots in its run: unchanged. Its test app (1 install
codeunit) runs as today in the default mode. Under the opt-in, each per-company routine of the apps under test runs once
more per declared company; dependency apps do not.

**Corpus (opt-in from step B).** The corpus app has six per-company install routines (five install codeunit triggers
and the PeV subscriber); each runs once more, in the second company. The baseline cache (ADR-022) keeps the result per
key. `tools/company_mode_check.sh` adds one corpus run in the default mode.

## Not pinned yet: probe questions

Corpus rule for all install probes: install code must never fail, or the app does not install on any BC version
(`.github/scripts/check-install-triggers.py:1-7`). A probe writes with `if Insert() then;` into a `DataPerCompany = false`
log and a test reads the log, as `PeVFixtures.al:237-252` does. New probes use "Extension Triggers" subscribers; a new
`Subtype = Install` codeunit needs a change of the `ALLOWED` list (`check-install-triggers.py:13`). Corpus issue 647
items 1 to 3 ask related questions for install codeunits.

- **P1. Which companies does the corpus CI database have?** Their number, `Name` and `Display Name`, in the Company
  table's key order, on BC 27, 28 and 29, in the main job and the two OnPrem jobs; check the Windows nightly too.
  Evidence so far, not a pin: the probe agent measured "CRONUS International Ltd." then "My Company"
  (`docs/probes-needed.md:29`), and the demo backup holds a second company "My Company" (`docs/probes-needed.md:215`).
  Decides: the list of names in `companies` in `oracle.toml`. Blocks step B.
- **P2. Under the opt-in, in which order do the companies get the per-company install code: the session company first,
  or the Company table's key order?** In the corpus CI the session company also sorts first by name, so
  `PeVTests.al:313-314` cannot tell the two apart. The corpus CI cannot vary its companies, so this needs another CI setup
  (a test company that does not sort first), or stays a documented assumption. Does not block A or B.
- **P3. What does the second company hold before the corpus app's install code runs?** From a test: `ChangeCompany` (or
  `RecordRef.Open(.., false, Company)`) to the other company and count the rows of Company Information (79), General
  Ledger Setup (98) and one per-company table that System Application's install code fills. Inside the per-company
  subscriber: the same counts, logged when it runs in the second company. Decides whether decision 5 can answer
  dependency tables in other companies (Options, item 2).
- **P4. Does an "Extension Triggers" subscriber run only at the install of its own app, or also when a later app is
  installed?** Put subscribers into `al-language-internals-fixture` (published before `al-language`, corpus `ci.yml:207-208`)
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
  a corpus probe: read it from BCApps' AL-Go setup or a run on a Windows container. Until then the recipe lanes run in
  the default mode.

## Options considered

1. **Several companies by default** (the first draft of this ADR). Rejected by the owner (2026-10-19): the runner runs
   one company by default; several companies are an opt-in for the tests that need them.
2. **Under the opt-in, dependency apps and `Company-Initialize` also run in every company.** This would give the other
   companies setup data, so the app under test's install code could read it there. Not chosen now: what another company
   holds on real BC depends on when it was created (P3), and running `Company-Initialize` there would guess at it. Cost:
   the per-company part of the baseline about doubles (BC 27.5: 3 install triggers in Base Application, 5 in System
   Application, 1 in System Application Test Library, plus `Company-Initialize`, the longest root; the whole baseline of
   a corpus run on BC 29 took 2.68 s today, 18 of 18 roots to their end). Return to it after P3.
3. **In the default mode, answer other-company reads as a database with one company.** That is what the runtime does
   today. Rejected: the database a user's CI runs on may hold more companies (the demo backup holds two), so the answer
   would be a guess (hard rule 4).
4. **A placeholder name for the second company now.** Any name other than the session company would make
   `EventLog_InstallRowCompany` pass. Rejected: the Company table and `CompanyName()` inside install code show the
   name, and nothing pins it (hard rule 3).
5. **Raise "Extension Triggers" to every subscriber of every app at every install.** Not chosen until P4: it would run
   a dependency app's subscribers again at each later install.

## Consequences

- In the default mode the app under test's "Extension Triggers" subscribers now run, in the session company. A partner
  extension that installs through them gets its install data.
- In the default mode, tests that scan the Company table, name a company the run does not know, or read a shared table
  that per-company install code fills are not run, with the reason. Today they are answered. Partner tests that need
  them use the opt-in: `--companies` with their CI's companies, or `--companies all` when the test data holds them.
- With the corpus opt-in, `EventLog_InstallRowCompany` can pass after step B. `EventLog_RowsWrittenBeforeTheTestSessionStarted`
  also needs the Company Triggers change.
- Two workspace apps with install code now run one after the other, not interleaved. No corpus app other than
  `al-language` has install code, so no corpus outcome changes from this.
- `docs/probes-needed.md` rows 28 and 29 change when the probes land: the order for subscribers of one app is now
  pinned (fact 4).

## Conformance

- Corpus: after step A, `EventLog_InstallRowCompany` and `EventLog_RowsWrittenBeforeTheTestSessionStarted` are not run;
  after step B, with the corpus opt-in, `EventLog_InstallRowCompany` passes on BC 27, 28 and 29. The install tests stay
  passing: `install/TestInstallTriggerSeed_Tests.al`, `TestInstallEnvVisible_Tests.al`,
  `TestInstallEventDispatch_Tests.al`, `TestInstallStartSession_Tests.al`. `--baseline-diff` shows no baseline case lost.
- `tools/company_mode_check.sh run` (step B): a corpus case differs between the two modes only by being not run in the
  default mode.
- Runtime tests in `al-testrun/src/runtime/tests.rs`, next to the install order tests: an "Extension Triggers"
  subscriber of a workspace app runs per company, then per database; a look-alike procedure on a normal codeunit does
  not run; two workspace apps install one after the other; the baseline path and the path without a baseline give equal
  rows. Default mode: one Company row; `Company.Get(CompanyName())` is answered, `Company.Count()` and
  `ChangeCompany` to an unknown name are refused, a read of a shared table written by per-company install code is
  refused and an insert into it is not. Opt-in: each declared company holds its per-company rows; a shared AutoIncrement
  table counts 1, 2, 3 in the order of fact 4; `ChangeCompany` to a name not on the list answers that it does not exist;
  a dependency per-company table is refused in another company; a root that stops in another company rolls back only
  there and marks its tables unseeded in that company.
- `tools/baseline_rule_check.sh run`: 0 differences. `tools/demand_vs_eager.sh run`: no difference.
- Microsoft buckets before and after step B (at least Tests-Bank and one of the five test apps with install code): the
  changed outcomes are listed in the pull request.
