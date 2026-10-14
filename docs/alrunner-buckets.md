# Microsoft's BaseApp test buckets under `alrunner`

How to run `Tests-*.Source.zip` of a BC artifact with `alrunner test`, and what the first measurement found
(2026-10-14, branch `alrunner-cli`). Nothing here is Microsoft's code; the sources are read from the local
artifact and never copied into this repository.

## Recipe

`A` is `Applications/` of the platform artifact (here BC 27.5, build in the folder name `46862.48827`). The build in
the folder name fills the `$(app_...)` variables of the source archives. Everything comes from one artifact, with
`--no-download`, so the Microsoft apps and the tests are the same build.

```
alrunner test "$A/BaseApp/Test/Tests-SMB.Source.zip" --no-download \
  --dep "$A/BaseApp/Test/Tests-TestLibraries.Source.zip" \
  --dep "$A/TestFramework/TestLibraries/Variable Storage/Library Variable Storage.Source.zip" \
  --dep "$A/TestFramework/TestLibraries/Assert/Library Assert.Source.zip" \
  --dep "$A/TestFramework/TestLibraries/Any/Any.Source.zip" \
  --dep "$A/TestFramework/TestLibraries/permissions mock/Permissions Mock.Source.zip" \
  --dep "$A/System Application/Test/System Application Test Library.Source.zip" \
  --dep "$A/BusinessFoundation/Test/Business Foundation Test Libraries.Source.zip" \
  --dep "$A/Application/Source/Application.Source.zip" \
  --dep "$A/BaseApp/Source/Base Application.Source.zip" \
  --dep "$A/System Application/Source/System Application.Source.zip" \
  --dep "$A/BusinessFoundation/Source/Business Foundation.Source.zip" \
  --dep "$A/../ModernDev/pfiles/microsoft dynamics nav/270/al development environment/System.app"
```

Without `--no-download` and without the last five `--dep`s, the feed supplies Application, Base Application, System
Application and Business Foundation (build of the newest published match, which can differ from the tests' build). The
platform `System.app` is not on the feed: without it the platform tables (`Field`, `AllObj`, `Company`) are outside
the workspace.

Several buckets can be named in one run (all 32 together: 40300 tests, one compile).

## Reading the result

Without `--test-data` there is no company data. Microsoft's tests expect a CRONUS company, so most failures are
missing setup data. The not-run counts and their reasons are the measurement; use `--top-refusals N`. The failures
are not.

With `--test-data` (ADR-020) the cached demo backup of the same build (`~/.bcartifacts.cache/sandbox/27.5.46862.48827/w1/`)
is restored table by table when a test first touches the table: add `--test-data` to the command above, and
`--top-failures N` to rank the failure messages. Measured on 2026-10-14 (branch `test-data`; 4 threads each for
check, lowering and tests; one run each; under `MemoryMax=4G`):

| Bucket | Test data | Passed | Failed | Not run | Run | Total | Peak memory |
|---|---|---:|---:|---:|---:|---:|---:|
| Tests-SMB | off | 6 | 6 | 1018 | 0.13 s | 2.72 s | 1833 MB |
| Tests-SMB | on | 10 | 3 | 1017 | 0.18 s | 4.29 s | 1826 MB |
| Tests-ERM | off | 227 | 121 | 8985 | 0.24 s | 4.40 s | 2878 MB |
| Tests-ERM | on | 434 | 9 | 8890 | 2.12 s | 6.67 s | 2928 MB |

Those four rows are the morning measurement. Afternoon, main after `fixes-8`, 4 threads, `MemoryMax=4G`: Tests-SMB off 6 passed, 6 failed, 1018 not run (total 4.71 s); Tests-SMB on 12 passed, 3 failed, 1015 not run (4.49 s); Tests-ERM off 241 passed, 149 failed, 8943 not run (6.74 s); Tests-ERM on 487 passed, 5 failed, 8841 not run (13.56 s, 2953 MB).

With test data Tests-SMB loads 2953 rows from 97 tables and Tests-ERM 15352 rows from 167 tables; no table is
refused. The Tests-SMB total includes 1.6 s to open the 860 MB backup on a cold page cache (0.1 to 0.2 s warm). The data costs nothing before a table is touched, which is why the run phase grows only with the tests that
now get far enough to run (ERM: 2.12 s for 434 passing tests instead of 0.24 s).

### Failures with test data

Triage rule (the AL Runner v2 skill): a failure is a data-recipe difference when the runtime did the right
thing for the company it was handed. Of the first nine Tests-ERM failures none were; `fixes-8` (2026-10-14)
dealt with them. Tests-ERM with test data went from 480 passed, 18 failed (main after `fixes-7`) to 484 passed,
5 failed; Tests-SMB is unchanged (11 passed, 3 failed).

| Was | Cause | Now |
|---|---|---|
| 3 tests: `The record in table <Positive Pay Entry / Purch. Comment Line Archive> already exists` | `CreateGuid` had a counter in the last 8 bytes, so `CopyStr(Format(CreateGuid()), 1, 20)` was `{00000000-0000-4000-` for every Guid | Fixed: Guids are version 4 in every byte, from a seeded generator (the host's `random_seed` and a counter) |
| 2 tests: `An error was expected inside an ASSERTERROR statement` (Pos. Pay Test Unit) | Same Guid collision (the setup rows shared one key) | Fixed with the Guids |
| 2 tests: same message (Price List Line UT and Price Worksheet Line UT `T029_ValidateAmountType`) | `fixes-7` and `fixes-8` both fixed it: an overload chosen by arity picked `CopyTo(var Price Asset)` for a `Price Source` argument. `fixes-8` extends the rule to every object a parameter names | Fixed |
| 1 test: `Bal. Account Type must not be   in ...` vs `... must have a value ...`; 6 more once the overload fix ran them (`Product Type must not be (All)`) | `TestField` on an Option or Enum holding its zero member says `must not be <member>`; the corpus does not pin the text | Refused (Unsupported), probe in "Probe batch 6"; 7 tests not run |
| 1 test: `The User Personalization does not exist ... User SID=<name>` | BC creates the row of the session user at sign-in; the corpus does not pin it | Refused, probe in "Probe batch 6" |
| 198 not run: `Format of Record(RefCell { ... })` | A debug dump of the value in the refusal text | The text names the type: `Format of a Record (standard format 0) is not pinned yet`. 213 Tests-ERM tests and 1 more (`FieldRef`) stop there: Base Application compares `Format(Rec) = Format(xRec)` in its Global Triggers subscribers. Probe in "Probe batch 6" |
| 3 (Tests-SMB): `The Office Add-in does not exist` | Unchanged. The test that creates the default add-ins is not run (`Uri.Scheme` is a DotNet value used as an operand), and three later tests of the codeunit rely on its rows. The backup holds no Office Add-in row | ADR-014 question |

`T043_AmountTypeMustBeDiscountForItemDiscGroup` (Price List Line UT and Price Worksheet Line UT) failed because
variables of a table object did not persist per record variable; ADR-021 (`table-globals`) fixes it: Tests-ERM with
test data went from 487 passed, 5 failed to 490 passed, 3 failed.

Still failing in Tests-ERM (3): `GLAccountCategoryDelete`, `AddTextforLanguageInReminderLevelCommunication` (`The TestPage is not open`) and
`AccountScheduleNoIndentationAnyValue` (`Commit` under AutoRollback) are not triaged.

Without test data Tests-ERM failed 121 tests; the first 30 failures are missing setup data (`General Posting Setup`
must contain records, `Sales & Receivables Setup`, `Tax Setup` and `Purchases & Payables Setup` do not exist, a date
calculation on an undefined date).

## Alone and together (issue 3, 2026-10-14)

The 32 buckets that hold tests, run one at a time and all in one `alrunner test` process, do not give the same
counts. Measured on BC 27.5 build 46862.56320 (the build of the first measurement, 46862.48827, is no longer on
the artifact feed), without test data, `tools/bucket_diff.py` over the JUnit files of both:

| | Passed | Failed | Not run | Tests |
|---|---:|---:|---:|---:|
| Sum of the 32 runs | 1109 | 890 | 38327 | 40326 |
| One process, 32 buckets | 1102 | 852 | 38372 | 40326 |

The same 40326 tests are in both. 39 pass or fail alone and are not run together, 7 pass alone and are not run together, 1 is not run alone
and fails together. Everything else has the same outcome. No test is nondeterministic: the same selection gives the same
result twice.

Every difference has one cause: **what is compiled and reached depends on the selection.** Compilation is demand
driven (ADR-013), and whatever one test reaches is active for all tests of the run: the reportextensions that change a
report, the page extension code of a base page, the subscribers of an event, the install code a baseline root
reaches. A refusal that depends on that set (more than one reportextension, a page control kind, a report property) hits a
test only when some test of the run activates the other part. In BC every installed app is active whatever a test uses,
so the combined run is the closer one, and a test that passes alone only because a part was not active is an accident of the selection.

| Codeunit (bucket) | Tests | Alone | Together | Why |
|---|---:|---|---|---|
| 134485 (Dimension) | 30 | failed (`The General Ledger Setup does not exist`, no test data) | not run | `Document Entries` has two reportextensions that change a run (`Mfg. Document Entries`, `Serv. Document Entries`, both in Base Application). Alone only one is active (`lower.rs`, "more than one reportextension changes the run"). Any second bucket I paired with Dimension made both active, except Tests-SCM-Manufacturing. Which test activates the second one is not pinned down |
| 137101 (SCM-Assembly) | 5 | failed (no setup data) | not run | `Mfg. ItemAvailByLocationLines` (page extension code on base page members, not lowered yet) is active only together |
| 136212 (Marketing) | 3 | failed (no setup data) | not run | column 9 of `Table Relations Metadata` is reached only together |
| 134821 (Cost Accounting) | 1 | failed | not run | page control kind `fixed` of `Apply Employee Entries` is reached only together |
| 134930 | 1 | passed | not run | `Mfg. Adjust Add. Rep. Currency` (reportextension `addafter(Job Ledger Entry)`) is active only together |
| 137462 (Physical Inventory) | 2 | passed | not run | report `Lot No Label` (property `wordmergedataitem`) is active only together |
| 135952 (Upgrade) | 4 | passed | not run | Tests-Upgrade depends on Library Assert only. Alone the baseline runs further (10 of 16 install roots, 49 tables hold rows, 20 unseeded); with any other bucket it is the 5 of 17 roots, 8 tables, 58 unseeded that every other run shows, because `Global Triggers` events are enabled for the tables the install code inserts into (`globaltriggers.rs`; the `GetGlobalTableTriggerMask` subscribers are in Base Application). Alone the Retention Policy install roots reach a different closure, the tables they would fill are not marked unseeded, and an empty table passes where BC holds rows |
| 134562 (Misc) | 1 | not run (`report 'Chart of Accounts' was not compiled`) | failed | the report is compiled only together; it then runs and fails on missing data |

Not found out: which test or bucket activates each part (only the pairing with Dimension was tried), and why alone the
`GetGlobalTableTriggerMask` subscribers are not active in Tests-Upgrade. No code was changed: making activation
independent of the selection (an extended object always activates its extensions at the level of a test that reaches it, as BC
does, or a stable "activate every extension of an activated object") changes which tests of the corpus
pass or are refused, and belongs in its own task with the baselines as the check.

`tools/bucket_diff.py --a <JUnit of each bucket> --b <JUnit of the combined run>` prints the table above for any two
sets of runs. A bucket name with a space needs quoting in the shell.

## Measured (4 threads each for check, lowering and tests; one run per bucket; under `MemoryMax=3G`; 2026-10-14, afternoon, main after `fixes-8`)

The first measurement of the morning of the same day is in the git history of this file. Since then the platform-table
providers (ADR-019) and `fixes-8` were merged; without test data the sum went from 651 passed, 175 failed, 39474 not run
to the figures below. The machine was shared and loaded, so the Tests/s column varies between runs.

| Bucket | Tests | Passed | Failed | Not run | Compile | Run | Tests/s | Peak memory |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Tests-Bank | 667 | 7 | 1 | 659 | 4.14 s | 0.24 s | 2827 | 1520 MB |
| Tests-Cash Flow | 399 | 26 | 3 | 370 | 4.17 s | 0.24 s | 1678 | 1587 MB |
| Tests-Cost Accounting | 562 | 56 | 26 | 480 | 3.24 s | 0.32 s | 1751 | 1390 MB |
| Tests-CRM integration | 961 | 11 | 6 | 944 | 4.82 s | 0.40 s | 2419 | 1597 MB |
| Tests-Data Exchange | 399 | 27 | 9 | 363 | 4.93 s | 0.89 s | 449 | 1568 MB |
| Tests-Dimension | 976 | 21 | 97 | 858 | 5.48 s | 0.33 s | 2980 | 1946 MB |
| Tests-ERM | 9333 | 241 | 149 | 8943 | 9.22 s | 0.91 s | 10280 | 2903 MB |
| Tests-Fixed Asset | 462 | 6 | 0 | 456 | 4.90 s | 0.26 s | 1810 | 1513 MB |
| Tests-General Journal | 825 | 11 | 24 | 790 | 4.42 s | 0.19 s | 4375 | 1669 MB |
| Tests-Graph | 77 | 4 | 0 | 73 | 4.31 s | 0.24 s | 321 | 1418 MB |
| Tests-Integration | 365 | 10 | 0 | 355 | 5.64 s | 0.35 s | 1051 | 1661 MB |
| Tests-Job | 1289 | 10 | 3 | 1276 | 5.54 s | 0.24 s | 5266 | 1736 MB |
| Tests-Marketing | 737 | 4 | 13 | 720 | 5.34 s | 0.21 s | 3511 | 1655 MB |
| Tests-Misc | 3245 | 358 | 167 | 2720 | 7.70 s | 6.89 s | 471 | 2434 MB |
| Tests-Monitor Sensitive Fields | 19 | 3 | 2 | 14 | 2.96 s | 0.20 s | 97 | 1250 MB |
| Tests-Permissions | 105 | 4 | 2 | 99 | 4.34 s | 0.22 s | 487 | 1664 MB |
| Tests-Physical Inventory | 157 | 35 | 6 | 116 | 3.05 s | 0.15 s | 1041 | 1401 MB |
| Tests-Prepayment | 567 | 0 | 4 | 563 | 3.63 s | 0.17 s | 3430 | 1689 MB |
| Tests-Rapid Start | 385 | 71 | 2 | 312 | 3.56 s | 0.57 s | 672 | 1575 MB |
| Tests-Report | 781 | 3 | 1 | 777 | 3.42 s | 0.22 s | 3568 | 1698 MB |
| Tests-Resource | 346 | 9 | 35 | 302 | 2.36 s | 0.15 s | 2255 | 1513 MB |
| Tests-Reverse | 198 | 0 | 1 | 197 | 1.66 s | 0.10 s | 2063 | 1385 MB |
| Tests-SCM-Assembly | 1764 | 5 | 48 | 1711 | 2.64 s | 0.15 s | 11708 | 1793 MB |
| Tests-SCM-Manufacturing | 1179 | 8 | 61 | 1110 | 3.41 s | 0.16 s | 7240 | 1739 MB |
| Tests-SCM-Service | 1917 | 20 | 47 | 1850 | 3.44 s | 0.16 s | 11956 | 1908 MB |
| Tests-SCM | 8295 | 61 | 123 | 8111 | 5.63 s | 1.10 s | 7518 | 2660 MB |
| Tests-SINGLESERVER | 922 | 29 | 4 | 889 | 5.20 s | 0.71 s | 1290 | 2126 MB |
| Tests-SMB | 1030 | 6 | 6 | 1018 | 4.96 s | 0.30 s | 3391 | 1914 MB |
| Tests-Upgrade | 12 | 11 | 0 | 1 | 2.83 s | 0.36 s | 33 | 1403 MB |
| Tests-User | 73 | 0 | 0 | 73 | 3.89 s | 0.19 s | 391 | 1591 MB |
| Tests-VAT | 1197 | 8 | 51 | 1138 | 2.95 s | 0.10 s | 12240 | 1838 MB |
| Tests-Workflow | 1056 | 25 | 5 | 1026 | 2.49 s | 0.15 s | 7075 | 1801 MB |
| Sum of the 32 buckets | 40300 | 1090 | 896 | 38314 | | | | |

All 32 buckets in one process (8 GB cap, 4 threads; 2026-10-14, afternoon, main after `fixes-8`; three runs, the same counts each time):
40300 tests, **1083 passed, 858 failed, 38359 not run**. Load 0.40 to 0.48 s, compile 12.1 to 12.9 s, run 4.1 to 5.4 s
(6762 to 9885 tests/s), total 16.8 to 19.1 s, peak memory 5.8 to 5.9 GB. The per-bucket sum above (1090, 896, 38314) differs from
this one-process run; the cause of the difference is not investigated. The one-process figures are the ones to quote.
Morning figures of the same day (before the platform-table round): 651 passed, 175 failed, 39474 not run; total 21.17 s.
`Tests-TestLibraries` (a library) and `Tests-Local` (empty in the W1 artifact) are left out.

Threads for Tests-ERM (compile / run / total): 1 thread 14.62 s / 0.86 s / 15.92 s; 4 threads 7.64 s / 0.37 s / 8.48 s;
12 threads 5.69 s / 0.31 s / 6.38 s. Peak memory 2.6 to 2.8 GB at every thread count.

Compile dominates: the demand compile of Base Application for the test codeunits of the bucket. The run is
sub-second except for Tests-Misc (5.4 s: 229 tests really run).

## "Outside the workspace" refusals (branch `bucket-demand`, 2026-10-14)

All 32 buckets in one process, before and after (same artifact, same flags). Before: 2939 tests refused as outside
the workspace (global table 1968, global codeunit 500, table 330, page 71, report 32). After: none. The cause was
never the demand-driven reach. A dependency file that does not parse takes all its objects out of the compile, and
name lookup then says "outside the workspace". The fixes are parser fixes (`Option` return type with no members,
`trigger Var::Event`, `ControlAddIn Name`, control add-in `event`), namespace-qualified data item tables, and the
message for platform tables with a dot in the name. The details are in ADR-013, "Outside the workspace on
Microsoft's test buckets". To find the next one, run a bucket with `AL_EAGER_DEPENDENCIES=1` (parses every file;
Tests-SMB: 3.4 GB peak, 8 s) and read the `parse:` lines; a normal run prints them for the files it reached.
