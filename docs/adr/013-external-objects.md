# ADR-013: Dependency apps and platform objects (objects outside the workspace)

- Status: Proposed
- Date: 2026-10-13

## Context

The corpus references System Application, Base Application, test-library and
platform objects whose source is not in the corpus (its manifest names System
Application, Base Application, System Application Test Library and
Permissions Mock as dependencies). Today the runtime treats every such
reference the same way:

- `al-sema/src/workspace.rs` (`resolve_references`) reports
  `AR0002 unresolved <kind> '<name>'`. `crates/al-sema/tests/expected-unresolved.txt`
  is the golden list: 202 distinct objects, 1062 references (table 112
  objects / 736 references, page 43 / 102, report 18 / 68, codeunit 12 / 81,
  enum 11 / 58, xmlport 4 / 12, query 2 / 5). `corpus_resolve.rs` fails if it
  changes, so it is already the backlog of this ADR.
- `al-sema/src/check.rs` does not guess: fields of an unresolved table have
  `Ty::Unknown`.
- `al-sema/src/lower.rs` (`resolve_obj`, `rt`) refuses a variable of an
  unresolved type, so the test is reported not run with
  `` `Var`: <kind> `X` is outside the workspace``.
- The CLI (`al-test-cli/src/main.rs`) loads exactly two apps, the corpus
  internals fixture and the corpus app (`runtime::load_app` reads `app.json`
  and `*.al`). The workspace model already has what dependencies need:
  `AppSource { id, name, dependencies, files }` and per-app visibility by
  dependency id (`visible_from`).

### Decision of the project owner (2026-10-13)

Fake AL objects of dependent apps are rejected, and so is hand-writing
Business Central / System Application objects. The direction, in the owner's
words: enable a way to either point the runtime at, or have it automatically
download, the dependent artifacts from the Microsoft feeds, extract the AL and
recompile it with the Rust compiler, and execute that compilation. The runtime
is still in spike mode and is not being built as a standalone tool like the
original AL Runner. Partial tables are out: real schemas come from the real
apps' source. Environment, identity, session, company and user answers mirror
what AL Runner (github.com/StefanMaron/BusinessCentral.AL.Runner) does.

## Survey: what is outside the workspace

`python3 tools/external_survey.py . <junit>` (added with this ADR) groups the
skip reasons of a run (HEAD c30ea3d, 1308/4836 pass) by first blocker. 516
tests (10.7 % of the corpus) stop at "outside the workspace": 453 on tables,
55 on codeunits, 8 on enums. A test is counted once, under the first thing that
stops it.

| Class | Objects | Tests | Examples (tests) |
|---|---:|---:|---|
| A. Program-metadata virtual tables | 19 | 203 | CodeUnit Metadata 23, Field 20, Page Control Field 19, AllObjWithCaption 18, AllObj 15, Table Metadata 12, Page Metadata 12, Report Layout List 12 |
| B. Permission tables | 7 | 59 | Expanded Permission 18, Metadata Permission Set 13, Permission Set 10, Aggregate Permission Set 7 |
| C. Generator / reference tables | 4 | 44 | Date 22, Integer 11, Time Zone 6, Windows Language 5 |
| D. Host, identity, platform bookkeeping tables | 12 | 72 | Record Link 18, All Profile 11, Session 10, Company 10, Active Session 9 |
| E. Base App business tables | 34 | 75 | Customer 9, Job 7, Dimension Correction 7, VAT Rate Change Setup 5, Location 5 |
| Codeunits | 9 | 55 | Temp Blob 28, Permissions Mock 6, Reten. Pol. Allowed Tables 5, Environment Information 4, Application Area Mgmt. Facade 4, User Permissions 3 |
| Enums | 3 | 8 | Printer Paper Kind 4, Job Queue Report Output Type 3 |

Members the corpus calls on the codeunits: `Temp Blob` `CreateOutStream` (33
calls), `CreateInStream` (25), `Length` (3); `Permissions Mock` `IsStarted`,
`Start`, `Stop`, `Set`, `ClearAssignments`, `Assign`; `Environment
Information` the six `Is*` predicates (the tests assert that the answers agree
with each other, not fixed values); `User Permissions` `IsSuper`; `Record Link
Management` `CopyLinks`. The survey script also prints the table members and
fields touched through declared variables.

### What the Microsoft artifacts contain (measured, BC 27.5.46862.56249)

All requests were made from this container. Only the entries named were
downloaded, and everything was extracted into `/tmp`, outside the repository.

| Fact | Evidence |
|---|---|
| Feed | `https://bcartifacts-exdbf9fwegejdqak.b02.azurefd.net/sandbox/<build>/<w1\|platform\|<country>>`, a ZIP served with `Accept-Ranges: bytes`. Version list `.../sandbox/indexes/w1.json` (90 KB, 1315 builds, `{Version, CreationTime}`). Reachable here: HEAD and ranged GET answered 200/206. The w1 artifact is 900 MB, the platform artifact 1.3 GB. |
| Other hosts | `api.nuget.org` answers 200. `bcartifacts.azureedge.net` (an older host) is refused by the proxy (502); AL Runner does not use it. |
| Range reads work | The ZIP central directory (125 entries for w1, 6917 for platform) was read with two ranged GETs; single entries with a 30-byte local-header GET plus one ranged GET, inflated locally. |
| The `platform` artifact has plain source | `Applications/<Name>/Source/<Name>.Source.zip`, a repository-layout zip with `app.json` and the `.al` files. Base Application 27 MB (8990 entries, 7868 `.al`, 100 MB of AL), System Application 9.6 MB (1264 `.al`, 6.5 MB), Business Foundation 0.4 MB (96 `.al`), plus `Application`, `System Application Test Library` (0.17 MB, depends on System Application and `Any`; holds `Azure AD Plan Test Library`), `TestFramework/TestLibraries/{Any,Assert,permissions mock}`, `TestFramework/TestRunner` and `BaseApp/Test/Tests-*.Source.zip`. What the corpus needs is about 40 MB. |
| The `w1` artifact has precompiled apps | `Extensions/Microsoft_*.app`: a NAVX header, then a ZIP holding ReadyToRun DLLs and a nested inner `.app`, which holds `src/*.al` (7868 files for Base Application; the outer wrapper holds none), `SymbolReference.json` (59 MB for Base Application) and `NavxManifest.xml`. Same source as the `Source.zip`. |
| Platform system tables have AL declarations | `platform` artifact, `ModernDev/pfiles/microsoft dynamics nav/270/al development environment/System.app` (0.5 MB, NAVX header then ZIP). 338 `.al` files declaring 217 tables (73 under `Virtual Tables`, 90 `Tenant Database Tables`, 28 `Application Database Tables`, 7 `Temporary Tables`) and 24 codeunits (the `*Triggers` event publishers, agent and feature helpers). It contains every system table the corpus misses (Date, Integer, AllObj, AllObjWithCaption, Session, Active Session, Company, User, Field, Key, Record Link, Permission Set, Code Coverage, CodeUnit Metadata, Time Zone, Windows Language, Expanded Permission, Aggregate Permission Set, Page Control Field). Only 28 of the 338 files contain a code body: declaring a virtual table and providing its rows are different things. |
| `DotNet` interop is real code in the apps | System Application declares 545 `DotNet` types and 134 of its files use them; Base Application 220 files. Examples: `Array`, `MemoryStream`, `Convert`, `Encoding`, `UserInfo`, and the platform helpers `NavUserAccountHelper` and `NavTenantSettingsHelper` (`Environment Information Impl.` reads the environment type through the latter). They have no AL source: they are .NET types in Microsoft's assemblies. |
| License signals | Most `.al` files start with a "Licensed under the MIT License" header (6748 of 7868 in Base Application, all but 7 in System Application, every platform `System.app` file looked at). The manifests say `ShowMyCode="False"`, `Target="OnPrem"`. See open question 1. |

### What AL Runner does (source: `/home/user/stefanmaron/businesscentral.al.runner`)

AL Runner is a C# tool that loads Microsoft's own `Microsoft.Dynamics.Nav.*`
DLLs and runs the precompiled application code. It does not recompile
Microsoft's AL. Where this ADR differs, the owner's direction has priority.

1. **Obtaining dependency apps.**
   - Feed and ranged ZIP reads: `AlRunner.Provisioning/ArtifactDownloader.cs`
     (`CdnBase` line 30; `BuildArtifactUrl`; `TryHeadContentLength`,
     `TryReadCentralDirectory` and `PlatformApps` read the central directory
     and fetch only the wanted entries). Version list `indexes/w1.json`
     (line 920). Network failures are classified in
     `AlRunner.Provisioning/NetworkDiagnosis.cs`; an unanswered probe holds the
     chosen version instead of falling back (`docs/provisioning-tiers.md`).
   - What it takes: from the `w1` artifact, `Extensions/` entries matching a
     curated five-app list (`W1PlatformAppPrefixes`, lines 646-659: Base
     Application, System Application, Business Foundation, Application,
     Application Test Library); from the `platform` artifact, the roughly 55
     service-tier DLLs and the test apps (`TestApps`). The AL compiler comes
     from `api.nuget.org` (`AlCompiler`).
   - Layout and cache: `~/.local/share/al-runner/artifacts/<build>/`
     (`AL_RUNNER_ARTIFACTS_ROOT`), `~/.cache/al-runner/` for compiled output,
     `ncl-cecil` and `install-baseline` (`AL_RUNNER_CACHE_ROOT`, `--cache`),
     `~/.bcartifacts.cache/sandbox/<build>/` (the VS Code AL cache, read for
     reuse), `--package-cache DIR` for local `.app` folders. Completeness
     predicates: `docs/provisioning.md`. Selection by app id and minimum
     version: `AlRunner/DependencyResolver.cs`.
2. **Source versus symbols versus DLL.** `docs/where-metadata-comes-from.md`
   and `docs/dependency-metadata-from-bc.md`: a dependency is served at one of
   four tiers (a committed precompiled sidecar, a ReadyToRun DLL inside the
   `.app`, a service-tier DLL, or, for packages that ship only source, a compile
   of `src/*.al`; `AppLoader.HasAlSource`, `AlRunner/AppLoader.cs:819`).
   Microsoft apps are served precompiled; their source is deliberately not
   compiled again because loading the Base Application floor costs about 70 s
   cold and 6 s warm, and compiling it measured 257 s and 8.8 GiB (that page).
   Table metadata for them comes from `SymbolReference.json`. Reading `.app`
   containers, including the `.NEA` runtime-package variant:
   `docs/runtime-packages.md`.
3. **Identity, environment, session, company, user.** The runner builds a
   skeleton `NavSession` by field-poking (`AlRunner/BcRuntime.cs`):
   - `UserId()` is `"TESTUSER"` (lines 1427-1458; `docs/scope.md` section 2);
     `UserSecurityId()` is `{C0A1BDFA-0000-0000-0000-545553545553}` (line 1441);
     the User table gets one matching row
     (`Patches/RecordPatches.UserSystemTable.cs`).
   - `CompanyName()` is `"My Company"` (line 1309; `docs/scope.md`); its company
     id is `{C0A1BDFA-0000-0000-0000-43524F4E5553}`; one Company row is seeded
     (`Patches/RecordPatches.CompanySystemTable.cs`).
   - `SessionId()` is 0; the Session table has exactly one row, the reading
     session, as BC's own provider returns, and Active Session one row
     (`Patches/RecordPatches.SessionVirtualTable.cs`,
     `RecordPatches.ActiveSessionSystemTable.cs`).
   - Culture is en-US (LCID 1033, around `BcRuntime.cs:1490`); the tenant id is
     `"default"` and the file-tenant sentinels are (empty Guid, `"STANDALONE"`)
     (around `BcRuntime.cs:2516`); a populated tenant is not implemented
     because it broke about 466 tests.
   - Environment: the tenant carries BC's default `EnvironmentType = Production`
     (`Patches/MetadataPatches.cs` lines 781-835), so `Environment Information`
     answers `IsProduction() = true`, `IsSandbox() = false`, `IsSaaS() = false`;
     `IsOnPrem()` is `ApplicationIdentifier() = 'NAV'`; service topology is
     always on-premises (`ProgramSupport/CliText.cs` lines 311-322,
     `docs/limitations.md#environment-type`). A test can switch the tenant to
     sandbox through the Microsoft test libraries, as on a service tier.
   - Permissions: everything is granted (a SUPER position for the session
     user); permission enforcement is a deliberate, permanent divergence, and
     asking about another user is refused with `RunnerOutOfScopeException`
     (`docs/scope.md` sections 2 and 3.17).
   - The corpus CI runs its tenant as a sandbox, so a corpus test cannot pin
     the runner's default (`docs/limitations.md` line 1150). The corpus tests
     that touch these answers (`session/`) assert agreement (`CompanyName()`
     equals `CurrentCompany()`, `IsProduction()` is `not IsSandbox()`), not
     fixed values, so the AL Runner defaults are compatible.
4. **Objects with no AL source.** `docs/scope.md` has four buckets: real code,
   faithful replacement, out of scope (`RunnerOutOfScopeException` naming the API
   and a section anchor) and in scope but not implemented. System tables are
   served two ways:
   - by Microsoft's own providers inside `Ncl.dll`: Date (2000000007) and
     Integer (2000000026) (`Infrastructure/NclCecilRewrite.Runtime.cs` around
     lines 962 and 1216; `docs/limitations.md` line 1273);
   - by the runner, one file per table under `AlRunner/Patches/`:
     `RecordPatches.AllObjVirtualTable.cs` and
     `RecordPatches.AllObjWithCaptionVirtualTable.cs` (rows from one shared
     inventory of the compiled objects; schema read from the platform
     `System.app` source, `docs/virtual-tables-allobj.md`),
     `RecordPatches.CodeunitMetadataVirtualTable.cs`,
     `...TableMetadataVirtualTable.cs`, `...PageMetadataVirtualTable.cs`,
     `...FieldVirtualTable.cs`, `...KeyVirtualTable.cs`,
     `...EventSubscriptionVirtualTable.cs`, `...PermissionSetSystemTable.cs`,
     `...RecordLinkTable.cs`, `...TimeZoneVirtualTable.cs`,
     `...WindowsLanguageVirtualTable.cs`, and so on. Each states what real BC
     answers (measured by decompiling the provider) and which columns it cannot
     fill; a shape nobody provided is refused, not emptied
     (`RecordPatches.VirtualTableShapeGap.cs`).
   - Platform behaviour that is .NET (the `NavTenantSettingsHelper` example
     above) is handled by patching the engine, or runs as the real BCL type.

### Experiments: what real source buys (measured, no code committed)

A temporary patch to `al-test-cli/src/main.rs` (reverted, not part of this
commit) loaded extra app directories ahead of the corpus apps; each was
extracted from the artifacts above into `/tmp`. Counts are the corpus run
against the BC 27.5 oracle.

| Run | Pass | Fail | Not run | Compile | Note |
|---|---:|---:|---:|---:|---|
| Baseline (main) | 1308 | 0 | 3528 | about 0.5 s | |
| + System Application, Business Foundation, Permissions Mock source (12.6 MB of AL in all) | 1312 | 0 | 3524 | 1.2 s (run 1.8 s) | Temp Blob resolves and its 3 direct tests pass |
| + Base Application source (113 MB of AL, 10 966 objects) | 1312 | 0 | 3524 | 16.5 s (run 80 s) | parse 1.9 s, workspace load 2.8 s, members 0.7 s, check 8.9 s (129 853 diagnostics, our gaps), lower 4.0 s; the rest of the 80 s is test execution, not profiled |
| + platform `System.app` declarations as ordinary tables | 1331 | **160** | 3345 | 1.2 s | virtual tables become empty stored tables: silently wrong answers |
| all of the above | 1331 | **157** | 3348 | 16.5 s (run 110 s) | 10 previously passing tests stop passing (not investigated) |

Re-estimate of the 516 "outside the workspace" tests:

- With real System Application, Business Foundation, Permissions Mock and Base
  Application source, 125 of the 516 move past that blocker and the 391 that
  remain are all system tables (47 objects, classes A to D, plus `Azure AD Plan
  Test Library`, which is in the System Application Test Library source, not
  loaded in this run). Every other codeunit, enum and Base App business table
  in the survey resolves.
- With the platform `System.app` declarations as well, 507 of 516 (98 %) pass
  that blocker; the other 9 are the test-library codeunit.
- Realised passes are small because the next blockers are independent. With
  source alone only the 3 `Temp Blob` tests pass (+4 in total). With the
  platform declarations 31 of the 516 pass, 157 fail (empty virtual tables) and
  about 320 stop at an unimplemented builtin or construct, most often
  `Database.CompanyName`, `NavApp.GetCurrentModuleInfo`, `Record.FieldNo`,
  `System.DMY2Date`, `Record.AddLink`, `Database.ServiceInstanceId`,
  `Report.SaveAs`, `RecordRef.GetTable` and `TestPage`. The 157 failures are the
  tests that become passes once the platform tables get row providers.

Two findings drive the decision:

1. The failures prove that a declared schema without behaviour is worse than
   refusal: a platform table whose provider does not exist must be refused at
   compile time, never stored and returned empty (hard rule 4, ADR-009).
2. Compiling Base Application costs 16.5 s here, far over the 1 s budget the
   corpus has, but the tests reach a tiny part of it. Refusals must stay
   explicit and only reachable code may matter, which the static
   `blocked_reason` call-graph check (`al-test-cli/src/runtime.rs`) already does
   for execution; the compile cost needs the same treatment.

## Decision (proposed)

1. **Dependency apps come from Microsoft's artifacts, as source, compiled by
   our front end.** The app set is resolved from the oracle's BC build.
   - *Local path.* `--deps <dir>` (repeatable): a directory holding any of
     `*.Source.zip`, `*.app` (plain or R2R-wrapped; the nested inner `.app` is
     unwrapped), `System.app`, or already extracted source trees. This is the
     offline and CI-cache route.
   - *Automatic download.* When a required app is not found, download it from
     the `platform` artifact of the pinned build by ranged ZIP reads, only the
     entries needed: `Applications/*/Source/*.Source.zip` for the apps in the
     corpus manifest and their dependencies, `System.app` and the TestFramework
     libraries. About 40 MB for the current corpus out of a 1.3 GB artifact; the
     `w1` artifact is not needed. `--no-download` makes a missing app an error
     naming the app, the build and the URL.
   - *Pinning.* `oracle/oracle.toml` records the artifact build the oracle ran
     on (`bc_artifact = "27.5.<build>"`), so the dependency source is the code
     the oracle executed. The junit does not record it today (open question 3).
   - *Integrity.* Every extracted entry is checked against the CRC-32 in the
     ZIP central directory; the cache keeps the entry bytes as downloaded and
     records build, entry name, size and CRC.
2. **Never in the repository.** Microsoft source and the downloaded zips live in
   a cache directory outside the repo (default `~/.cache/al-runtime/artifacts/<build>/`,
   `AL_RUNTIME_CACHE` to move it) and in a CI cache keyed by build. A test
   (`no_microsoft_source_in_repo`) fails if any tracked file carries the
   Microsoft copyright header.
3. **Platform apps are apps.** The platform `System.app` is loaded as an app with
   its real id and is an implicit dependency of every app (the corpus manifest
   does not list it; BC always provides it). Dependency apps are marked in
   `AppSource` (`kind`, `version`) so that no test is discovered or run in them,
   diagnostics name the app, and their `Install` triggers and `Company-Initialize`
   can run in dependency order into a snapshot (ADR-004) that every test starts
   from, the way AL Runner's install baseline works
   (`RecordPatches.InstallBaseline.cs`). The first increment runs it only if a
   corpus test needs it (open question 5).
4. **Compilation is demand-driven, then cached if still needed.** The workspace
   indexes every dependency object by id and name (a cheap declaration scan,
   then lazy parse by file), builds member tables as today, and checks and
   lowers only routines reachable from the test codeunits being run (the
   reference closure over the resolved call graph, including event subscribers
   and triggers of touched tables). Unreachable code is never type-checked, so
   unimplemented builtins in it cost nothing, and reachable ones stay explicit
   refusals on the test that reaches them. If the warm budget is still missed,
   add an on-disk cache of the parsed and indexed form of each dependency, keyed
   by the digest of its source entries plus the digest of our binary (any
   compiler change invalidates it). Lowered IR is not cached in the first
   increment: other apps' extensions (the corpus extends Base App tables) merge
   into a table's members, so the unit of caching would have to be redesigned
   first. Budgets: the corpus-only cold compile stays at 1 s; with dependencies
   the warm start (cache present, nothing downloaded) targets 1 s plus the cost
   of what the run reaches, reported as a metric; a cold start with download is
   reported, not budgeted.
5. **Platform-native layer for what has no AL.** Objects that only exist as
   engine behaviour are implemented natively in Rust and registered by object id
   or type name (the ADR-008 registry mechanism, a second id namespace). This is
   not a fake of a dependent app's AL object: the schema of every platform table
   is the real declaration from `System.app`; what we write is the row provider
   and the platform behaviour behind it. Three kinds:
   - **Table providers** by table id, mirroring AL Runner's choices: generated
     rows (Date, Integer, Time Zone), rows derived from the compiled apps
     (AllObj, AllObjWithCaption and the metadata tables) and single seeded rows
     (Company, User, Session, Active Session, with the values below). Stored
     platform tables (Record Link, the permission tables) use the ordinary Store
     with their real declaration and need an explicit registry entry saying so,
     because their platform triggers are native.
   - **`DotNet` types** by name (`Array`, `MemoryStream`, `Convert`, `Encoding`,
     `NavTenantSettingsHelper`, `NavUserAccountHelper`, ...), only those the
     reachable code calls.
   - **Platform codeunit members** that the source implements with a native call.

   The registry is the only way a platform table or `DotNet` type becomes usable.
   A platform table (any table from the platform `System.app`, or with an id of
   2000000000 or above) with no registry entry is refused at compile time on the
   test that touches it, with a diagnostic naming table and id: `platform table
   'Date' (2000000007) has no provider`. The same holds for a `DotNet` type. The
   refusal is reported through `Lowered::unsupported`, as unimplemented builtins
   are.
6. **Identity answers mirror AL Runner exactly.** They are host configuration
   (ADR-005, `al-host`) with these defaults:

   | Answer | Value | AL Runner source |
   |---|---|---|
   | `UserId()` | `TESTUSER` | `BcRuntime.cs:1433` |
   | `UserSecurityId()` | `{C0A1BDFA-0000-0000-0000-545553545553}` | `BcRuntime.cs:1441` |
   | `CompanyName()` | `My Company` | `BcRuntime.cs:1309` |
   | Company id | `{C0A1BDFA-0000-0000-0000-43524F4E5553}` | `BcRuntime.cs` (company seed) |
   | `SessionId()`, Session `Connection ID` | `0` | `RecordPatches.SessionVirtualTable.cs` |
   | Session / Active Session | one row, the running session | same file, `ActiveSessionSystemTable.cs` |
   | Culture | en-US (1033) | `BcRuntime.cs` (cultureSettings seed) |
   | Environment type | Production: `IsProduction()` true, `IsSandbox()` false, `IsSaaS()` false | `MetadataPatches.cs:781-835`, `CliText.cs:311-322` |
   | Service topology | on-premises, also after a test switches to SaaS | `docs/limitations.md#environment-type` |
   | Permissions | all granted; enforcement not modelled; another user refused | `docs/scope.md` section 2 |

   The `Environment Information` codeunit stays the real System Application
   source; only the `DotNet NavTenantSettingsHelper` it calls is native.

## Seams: what changes and what does not

Changes:

- **`al-host` (ADR-006).** The trait side only: a `DependencySource` capability
  (list the packages and entries of a build, read an entry with its CRC) and the
  identity configuration above. No OS access in `al-host`.
- **A new non-core crate `al-deps`** (std::fs, an HTTP client, a ZIP reader, the
  cache) implements it; `al-test-cli` depends on it. Downloads and file access
  live here. The core crates (`al-syntax`, `al-sema`, `al-ir`, `al-builtins`,
  `al-vm`, `al-data`) keep doing none.
- **`al-sema::workspace`:** `AppSource` gains `kind` (workspace or dependency)
  and `version`; loading takes an object index so files are parsed lazily; the
  platform `System.app` is an implicit dependency. Name resolution, visibility
  and extension merging are unchanged.
- **`al-sema::lower` / `check`:** an entry-point-driven compile (check and lower
  only what is reachable from given routines) beside the whole-workspace one.
  The corpus-only path keeps its behaviour.
- **`al-builtins` (ADR-008):** the registry gains a second id namespace for
  platform providers, `DotNet` types and platform members.
- **`al-data` (ADR-003):** table providers (rows computed on read) need a new
  cursor source behind the Store trait. That is an amendment to ADR-003, not made
  here; it gets its own ADR before the first provider is written.
- **CLI:** `--deps DIR` (repeatable), `--bc-artifact BUILD`, `--no-download`,
  `--cache-dir DIR`, `AL_RUNTIME_CACHE`. `oracle/oracle.toml` gains the artifact
  build.

Does not change: the IR and its serialisation (ADR-011), the value model
(ADR-010), `Ctx` (natives receive it as builtins do), the Session/Engine split
(ADR-001), snapshots (ADR-004), hard rules 1 to 6, the oracle and the ratchet
(ADR-007).

## First increment

Goal: prove the pipeline on the apps that cost least, with no row provider and
no `DotNet` type yet.

1. `al-deps`: local-path and download resolution of the `platform` artifact,
   `Source.zip` and `System.app` reading, cache and CRC check; `AppSource`
   `kind`/`version`; the CLI flags; the `oracle.toml` pin. Tests use a tiny
   fixture zip written by the test (not Microsoft's) and a loopback range server.
2. Load System Application, Business Foundation, Permissions Mock, the System
   Application Test Library and the platform `System.app` for the corpus run,
   with the platform-table refusal (decision 5) in place so the run reports 0
   fail. Expected from the measurements: pass 1308 to about 1312, the tests that
   move past "outside the workspace" appear under their next blocker, and the 160
   silent failures are refusals instead.
3. Demand-driven compile and the warm-start metric, then Base Application.
   Expected: Base App business tables resolve for their 75 tests; no new passes
   are promised, because their next blockers are builtins.
4. The ADR-003 amendment for table providers, then providers in the order the
   survey gives: Date and Integer (33 tests), Company, User, Session and Active
   Session (31, with the identity values), AllObj and AllObjWithCaption (33), the
   metadata tables (about 170). `DotNet` types as reachable code asks.

## Rejected options

- **(b) Bundled hand-written AL shims for selected platform objects.** Rejected
  by the project owner: AL objects of dependent apps are not to be faked or
  hand-written. (A throwaway `Temp Blob` shim was used earlier to measure the
  idea; it made 3 tests pass and showed that the cost was in the next blockers,
  not the missing object. Dropped, together with the attribute design that went
  with it.)
- **(a) Native Rust implementations of dependent-app objects** such as `Temp
  Blob`. Rejected for the same reason for objects that have AL source; kept only
  for objects and types that have none (decision 5), which is open question 2.
- **(c) Symbols only (`SymbolReference.json`).** Gives the surface but no
  behaviour, and Base Application's is 59 MB of JSON; the source gives both. May
  still serve as a verifier of the parsed schema in a test.
- **(d) Keep refusing.** Leaves 516 tests dark, including core language tests
  that only use `Temp Blob` as a vehicle (`types/TestInterfaceVarCodeunitOut.al`).

## Consequences

- The runtime executes Microsoft's AL, not an imitation. Every gap in our front
  end and VM that Base Application reaches becomes visible as an explicit
  refusal on the test that reaches it; the whole app produces 129 853 check
  diagnostics today, of which only the reachable fraction matters.
- A first run needs the network or a populated cache; CI needs an artifact cache
  step keyed by build.
- Platform behaviour is written once per table or type in Rust, with the real
  declaration as its schema, each with a corpus pin or a probe (hard rule 3).
- The warm-start budget for dependencies is new and must be measured, not
  assumed.

## Conformance

- `al-deps` tests: a fixture zip reads the same by range and whole; a CRC
  mismatch is an error; `--no-download` with a missing app fails naming it; the
  cache layout is stable.
- `al-sema` tests: a platform table without a registry entry is refused with its
  name and id; a dependency app's tests are not discovered; an unreachable
  unsupported routine in a dependency does not block a test that does not reach it.
- `no_microsoft_source_in_repo`: no tracked file carries the Microsoft copyright
  header.
- `corpus_resolve.rs`: `expected-unresolved.txt` is regenerated with the
  dependencies present and then holds only what no artifact supplies.
- The corpus run (`metrics/passing.txt`) is the integration test.

## Side finding: namespace-qualified in-workspace names are mis-reported

Unrelated to this decision, and worth a separate fix. 5 of the 516 are not
external: a name qualified by a namespace the corpus itself declares
(`ALLanguage.Coverage.TxnLocal.Shipment Method` 3 tests,
`...RelationNameCollision.Shipping Agent` 1, `...PxqLocal.PXQ Table` 1) reaches
`lower.rs::resolve_obj` as one string and does not resolve. Separately,
`Workspace::resolve_ref` has a last tier that accepts a candidate in any
namespace, which the corpus relies on (no corpus file writes `using
System.Utilities;` yet uses `Codeunit "Temp Blob"`); dependency apps resolve
through the same path.

## Open questions for the project owner

1. **Licensing.** Microsoft's packages carry `ShowMyCode="False"` in their
   manifests, yet the source is inside them and most files carry an MIT license
   header. Is downloading it into a local cache and compiling it, with nothing
   committed to the repository, acceptable under your reading of the terms, and
   may the CI cache hold it?
2. **Native platform layer.** The system tables (Date, Integer, AllObj, Session,
   Company, User, permissions, metadata) and the `DotNet` types have no AL body,
   only a real AL declaration of the schema. Is writing native row providers and
   `DotNet` shims for them in Rust, behind the real declarations and refusing
   when absent, within "do not fake AL objects from dependent apps"? The
   boundary proposed: native only where there is no AL body to run, never for an
   object whose body we can compile.
3. **Pinning.** The oracle junit does not record the BC build it ran on. Should
   the corpus CI be asked to record the artifact build (a corpus change), or
   should `oracle.toml` pin the latest 27.5 build and treat a mismatch as a
   warning?
4. **Dependency budget.** Is "1 s warm start plus what the run reaches" an
   acceptable budget for runs that include Base Application, or should the cold
   compile cost be hidden behind a persistent cache from the start?
5. **Install triggers.** Should dependency apps' `Install` triggers and
   `Company-Initialize` run into the baseline snapshot (as AL Runner does and BC
   does when it installs apps), or stay out until a corpus test needs the rows
   they create?
