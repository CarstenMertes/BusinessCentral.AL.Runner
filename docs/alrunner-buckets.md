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
| Tests-SMB | off | 6 | 6 | 1018 | 0.14 s | 3.17 s | 1810 MB |
| Tests-SMB | on | 10 | 3 | 1017 | 0.19 s | 3.64 s | 1818 MB |
| Tests-ERM | off | 224 | 121 | 8988 | 0.24 s | 4.54 s | 2840 MB |
| Tests-ERM | on | 431 | 9 | 8893 | 2.18 s | 6.18 s | 2916 MB |

With test data Tests-SMB loads 2953 rows from 97 tables and Tests-ERM 15350 rows from 165 tables; no table is
refused. The data costs nothing before a table is touched, which is why the run phase grows only with the tests that
now get far enough to run (ERM: 2.18 s for 431 passing tests instead of 0.24 s).

### Failures with test data

Every failure of the two buckets, ranked by `--top-failures`. Triage rule (the AL Runner v2 skill): a failure
is a data-recipe difference when the runtime did the right thing for the company it was handed. None of these are.

| Count | Message (numbers and quoted values folded) | Bucket | Cause |
|---:|---|---|---|
| 3 | `The Office Add-in does not exist. ... Application ID=<name>` | SMB | Runtime gap. The test that creates the default add-ins (`CreateDefaultAddinsWhenPageRuns`) is not run (`Uri.Scheme` is a DotNet value used as an operand), and three later tests of the codeunit rely on its rows. The backup holds no Office Add-in row |
| 4 | `An error was expected inside an ASSERTERROR statement.` | ERM | Runtime. `Validate` of `Price List Line."Amount Type"` raises nothing (2 tests); validating `Bank Account` with an export code of the wrong direction raises nothing (2 tests); not investigated further, probably a TableRelation filter that `Validate` does not enforce |
| 3 | `The record in table <Positive Pay Entry / Purch. Comment Line Archive> already exists ...` | ERM | Runtime. `CreateGuid` is deterministic with a counter in the last 8 bytes, so `CopyStr(Format(CreateGuid()), 1, 20)` is `{00000000-0000-4000-` for every Guid. Both tables are empty in the backup |
| 1 | `Assert.ExpectedError failed. Expected: Bal. Account Type must not be   in Payment Registration Setup. Actual: ... must have a value ...` | ERM | Runtime. The text of a TestField error on an Option whose blank member is a space |
| 1 | `The User Personalization does not exist. ... User SID=<name>` | ERM | Runtime. The platform table has no row for the test user |

Without test data Tests-ERM failed 121 tests; the first 30 failures are missing setup data (`General Posting Setup`
must contain records, `Sales & Receivables Setup`, `Tax Setup` and `Purchases & Payables Setup` do not exist, a date
calculation on an undefined date).

## Measured (4 threads each for check, lowering and tests; one run per bucket; under `MemoryMax=3G`)

| Bucket | Tests | Passed | Failed | Not run | Compile | Run | Tests/s | Peak memory |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
| Tests-Bank | 667 | 5 | 0 | 662 | 3.00 s | 0.12 s | 5750 | 1438 MB |
| Tests-Cash Flow | 399 | 21 | 2 | 376 | 3.53 s | 0.12 s | 3416 | 1497 MB |
| Tests-Cost Accounting | 562 | 11 | 5 | 546 | 2.62 s | 0.08 s | 7043 | 1280 MB |
| Tests-CRM integration | 961 | 9 | 6 | 946 | 3.04 s | 0.19 s | 5022 | 1468 MB |
| Tests-Data Exchange | 399 | 6 | 0 | 393 | 3.22 s | 0.11 s | 3564 | 1447 MB |
| Tests-Dimension | 976 | 15 | 15 | 946 | 4.46 s | 0.10 s | 10239 | 1808 MB |
| Tests-ERM | 9333 | 142 | 12 | 9179 | 7.64 s | 0.37 s | 25210 | 2824 MB |
| Tests-Fixed Asset | 462 | 3 | 0 | 459 | 3.43 s | 0.10 s | 4447 | 1431 MB |
| Tests-General Journal | 825 | 4 | 1 | 820 | 3.57 s | 0.09 s | 8745 | 1625 MB |
| Tests-Graph | 77 | 0 | 0 | 77 | 2.99 s | 0.10 s | 790 | 1349 MB |
| Tests-Integration | 365 | 10 | 0 | 355 | 3.22 s | 0.16 s | 2292 | 1539 MB |
| Tests-Job | 1289 | 5 | 0 | 1284 | 3.84 s | 0.11 s | 11999 | 1622 MB |
| Tests-Marketing | 737 | 2 | 3 | 732 | 3.49 s | 0.10 s | 7452 | 1573 MB |
| Tests-Misc | 3245 | 229 | 90 | 2926 | 5.62 s | 5.43 s | 598 | 2219 MB |
| Tests-Monitor Sensitive Fields | 19 | 0 | 1 | 18 | 2.27 s | 0.06 s | 314 | 1163 MB |
| Tests-Permissions | 105 | 1 | 2 | 102 | 2.55 s | 0.08 s | 1297 | 1526 MB |
| Tests-Physical Inventory | 157 | 33 | 5 | 119 | 2.11 s | 0.08 s | 2087 | 1286 MB |
| Tests-Prepayment | 567 | 0 | 0 | 567 | 2.51 s | 0.11 s | 5138 | 1510 MB |
| Tests-Rapid Start | 385 | 57 | 0 | 328 | 2.77 s | 0.28 s | 1378 | 1358 MB |
| Tests-Report | 781 | 3 | 0 | 778 | 2.67 s | 0.09 s | 8977 | 1583 MB |
| Tests-Resource | 346 | 4 | 0 | 342 | 2.51 s | 0.06 s | 6147 | 1415 MB |
| Tests-Reverse | 198 | 0 | 0 | 198 | 1.57 s | 0.05 s | 3920 | 1298 MB |
| Tests-SCM-Assembly | 1764 | 0 | 0 | 1764 | 2.46 s | 0.07 s | 23630 | 1700 MB |
| Tests-SCM-Manufacturing | 1179 | 2 | 0 | 1177 | 3.04 s | 0.06 s | 18285 | 1637 MB |
| Tests-SCM-Service | 1917 | 13 | 0 | 1904 | 3.12 s | 0.13 s | 14384 | 1866 MB |
| Tests-SCM | 8295 | 26 | 1 | 8268 | 5.39 s | 0.19 s | 44460 | 2576 MB |
| Tests-SINGLESERVER | 922 | 19 | 2 | 901 | 3.57 s | 0.19 s | 4910 | 1775 MB |
| Tests-SMB | 1030 | 3 | 4 | 1023 | 3.77 s | 0.09 s | 10925 | 1770 MB |
| Tests-Upgrade | 12 | 6 | 0 | 6 | 2.45 s | 0.09 s | 136 | 1144 MB |
| Tests-User | 73 | 0 | 0 | 73 | 3.32 s | 0.11 s | 694 | 1561 MB |
| Tests-VAT | 1197 | 5 | 24 | 1168 | 3.64 s | 0.08 s | 14903 | 1743 MB |
| Tests-Workflow | 1056 | 17 | 2 | 1037 | 3.05 s | 0.07 s | 14933 | 1709 MB |
| Sum of the 32 buckets | 40300 | 651 | 175 | 39474 | | | | |

All 32 buckets in one process (6 GB cap, 4 threads): 40300 tests, load 0.44 s, compile 15.96 s, run 4.48 s
(8992 tests/s), total 21.17 s, peak memory 5785 MB. `Tests-TestLibraries` (a library) and `Tests-Local` (empty
in the W1 artifact) are left out.

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
