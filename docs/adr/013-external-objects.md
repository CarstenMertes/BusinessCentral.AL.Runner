# ADR-013: Dependency apps and platform objects (objects outside the workspace)

- Status: Proposed (the project owner accepts it after reading this revision)
- Date: 2026-10-13
- Revised: 2026-10-13, to record the owner's answers to the five open questions

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

The same day the owner answered the five open questions that this ADR first
raised. The answers are recorded verbatim under "Answered by the project owner"
near the end. The decision section reflects them. One question remains, on
`DotNet` types.

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
| License signals | Most `.al` files start with a "Licensed under the MIT License" header (6748 of 7868 in Base Application, all but 7 in System Application, every platform `System.app` file looked at). The manifests say `ShowMyCode="False"`, `Target="OnPrem"`. The owner accepted downloading and compiling it with nothing committed (answer 1). |

### What AL Runner does (source: the checkout at `/home/stefan/Documents/Repos/community/BusinessCentral.AL.Runner`, commit 351c2813d, 2026-10-06; paths below are relative to it)

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

5. **Dependency version selection.** Two layers.
   - *Which BC build.* `--bc-version X` (an exact build or a prefix) or
     `--artifact-path DIR`; they are mutually exclusive (`AlRunner/Program.cs`
     lines 105 and 396-401). `BcArtifacts.SelectArtifactVersionDir`
     (`AlRunner/Infrastructure/BcArtifacts.cs:292`) lists the
     version-named directories under the artifacts root and takes the highest, or
     the highest whose dot-separated name starts with the prefix (`27.5` matches
     `27.5.x`, not `27.50`). No match is an exception that names the builds
     available and the download command; there is no silent fallback. With
     neither flag the default is bound to the engine the binary was built for
     (`Program.cs` lines 1068-1230): the exact build, then its minor, then its
     major. The cache is asked first, then the CDN when auto-provision is on (the
     default; `--no-auto-provision` opts out) (`ResolveProvisionTargetCore`,
     `BcArtifacts.cs:504`). A CDN probe that gets no answer holds its tier and
     does not fall to a looser one (`docs/provisioning-tiers.md`). The comment at
     `Program.cs:1068` gives the reason: the binary can only faithfully run its
     own major, so a stray download of another major must never become the
     default. The project's `app.json` is read as a floor: the higher of
     `application` and `platform` (`TryDeriveBcFloorFromProject`,
     `AlRunner/ProgramSupport/Provisioning.cs:449`; `ReadMinimumBcVersion`,
     `AlRunner/Infrastructure/InProcessAppPackager.cs:178`). The default never
     selects below it. An explicit `--bc-version` below it runs, with a warning
     (`AlRunner/Infrastructure/BcVersionFloor.cs`).
   - *Which package satisfies each dependency.* `AlRunner/DependencyResolver.cs`.
     Inputs: the roots are the `dependencies` of the project's `app.json`
     (`ReadDependencies`, `AlRunner/ProgramSupport/Dependencies.cs:261`) plus a
     default test-tool app; every chosen package then contributes its own
     `<Dependencies>` from `NavxManifest.xml` and its Platform and Application
     floors as extra roots (`AppLoader.ImplicitRoots`, line 233). The pool is
     every `.app` file under the search directories, indexed by app id and by
     (name, publisher), all versions kept (`EnsureIndexed`, line 583). The
     directories are the bundle's `.alpackages`, then the package-cache
     directories: `--package-cache`, or by default the VS Code AL symbol cache
     `~/.bcartifacts.cache/sandbox/<build>` and a curated symbols directory
     (`DefaultPackageCacheDirs`, `Provisioning.cs:197`), plus the runner's own
     `<artifacts>/<build>/platform-apps` and `test-apps`, which provisioning adds
     to the set (`Program.cs` lines 2203 and 2247). Rules (`TryFind`,
     line 322; `SelectBestVersion`, line 343): a lookup by app id is
     authoritative, and if candidates exist for the id but none meets the minimum
     it does not fall through to name and publisher; name and publisher are used
     only when the id is empty or unknown. A candidate below the declared version
     is dropped, because the version is a minimum. Among the rest, in order: a
     package built from source in this run, then the executable tier (R2R DLLs,
     then AL source or a sidecar DLL, then nothing), then the highest version
     (lines 453-479). For the five Microsoft platform apps the minimum is advisory
     when the winner cannot execute and a below-minimum package can (lines
     496-512). No match: an optional dependency or a Microsoft platform app is
     skipped with a printed line (lines 159-175); candidates that are all below the
     minimum raise `DependencyVersionMismatchException` listing the versions; no
     candidate raises `MissingDependencyException` naming the directories searched
     (lines 176-197). A cycle raises `InvalidOperationException` (line 202).
     The output is a post-order depth-first walk, so dependencies come before
     dependents (line 242). Caches: the candidate index lives in memory for one
     resolver and is never written to disk. Downloaded builds live under
     `~/.local/share/al-runner/artifacts/<build>/` (`AL_RUNNER_ARTIFACTS_ROOT`).
     Other caches sit under `~/.cache/al-runner/<name>` (`CacheRoots.cs`).
   - The corpus `app.json` declares platform, application and the Microsoft
     dependencies as `27.0.0.0`. To AL Runner that is a floor and nothing more. The
     build that runs is the one it selects in the first layer.
6. **Install baseline.** The code is `AlRunner/InstallTriggerRunner.cs`,
   `AlRunner/CompanyInitializer.cs`, `AlRunner/TestExecutor.cs` (lines 540-870) and
   `AlRunner/Patches/RecordPatches.InstallBaseline.cs`.
   - *Which apps, in what order.* Every loaded dependency assembly in dependency
     order (`DependencyLoader.LoadAll` returns them dependencies first), then the
     project's own test assembly last (`InstallTriggerRunner.RunAll`, line 99).
     Within an app, every codeunit that carries a `OnInstallAppPerCompany` or
     `OnInstallAppPerDatabase` subscriber to the system codeunit 2000000010
     "Extension Triggers" (`Scan` and `InstallTriggerMethod`). For each codeunit,
     per-company runs first, then per-database (`FireAll`, line 144).
   - *Install, Upgrade, Company-Initialize.* Only install triggers run. A search
     of `InstallTriggerRunner.cs` and `TestExecutor.cs` finds no call to an upgrade
     trigger. Base Application codeunit 2 `Company-Initialize` runs once, after the
     dependency install triggers and before the dependency snapshot, and only if
     that codeunit exists in the loaded apps (`CompanyInitializer.EnsureCompanyInitialized`,
     line 128). If it aborts, the rows it wrote are kept, and the abort is a
     run-level result with exit code 2 (`docs/partial-company-initialization.md`).
   - *When it runs.* Once per app group (one project and its dependency closure),
     before the first test (`TestExecutor.Run`).
   - *Steps in `TestExecutor.Run`.* Reset per-test state. Seed the Published
     Application rows of the dependency apps, the session User row, the Company
     row, and the Access Control row that makes the user SUPER, because dependency
     install code reads them. Run the dependency install triggers
     (`RunDependenciesOnly`). Run `Company-Initialize`. Capture the "dependency plus
     company" snapshot. Then seed the project's own Published Application row and
     the Active Session row, run the project's own install triggers
     (`RunTestAssemblyOnly`), and capture the baseline.
   - *Snapshot.* The rows of the in-memory table providers are deep-copied, plus
     isolated storage, record links and the auto-increment counters
     (`InstallBaselineSnapshot`). The self-populating virtual tables (AllObj,
     Field, the metadata tables) are not snapshotted; they are derived again on
     every access. The dependency-plus-company snapshot is cached in memory per
     key, and on disk at `~/.cache/al-runner/install-baseline/<sha256>.bin`
     (`InstallBaselineDiskCache.cs`). The key is the dependency assembly set, the
     app symbol state, the subscriber assemblies and the test-data options
     (`CurrentInstallBaselineCacheKey`, line 460), plus the runner build for the
     disk copy. A disk copy is not written when `Company-Initialize` aborted.
   - *Restore.* Under the default `TestIsolation = Codeunit`, the baseline is
     restored at every test-codeunit boundary (`RestoreInstallBaseline`,
     `TestExecutor.cs:939`). Under `Test` isolation it is restored before every
     test. The code comment explains why: real BC rolls each test back but keeps
     committed install data.

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

## Decision (proposed, revised 2026-10-13 after the owner's answers)

1. **Dependency apps are Microsoft's source, downloaded into a cache and
   compiled by our front end.** Licensing is settled by the owner's answer: the
   source is downloaded into a local cache and a CI cache, compiled, and nothing
   is committed.
   - *Local path.* `--deps <dir>` (repeatable): a directory holding any of
     `*.Source.zip`, `*.app` (plain or R2R-wrapped; the nested inner `.app` is
     unwrapped), `System.app`, or already extracted source trees. This is the
     offline and CI-cache route. It plays the role of AL Runner's
     `--package-cache`.
   - *Automatic download.* When no package in the search set satisfies a
     dependency, download it from the `platform` artifact of the selected build
     (decision 2) by ranged ZIP reads, only the entries needed:
     `Applications/*/Source/*.Source.zip` for the apps in the manifest and their
     dependencies, `System.app` and the TestFramework libraries. About 40 MB for
     the current corpus out of a 1.3 GB artifact; the `w1` artifact is not
     needed. `--no-download` makes a missing app an error naming the app, the
     build and the URL.
   - *Integrity.* Every extracted entry is checked against the CRC-32 in the ZIP
     central directory; the cache keeps the entry bytes as downloaded and
     records build, entry name, size and CRC.

2. **Version selection is driven by the project's `app.json` and mirrors AL
   Runner.** The runtime is not corpus-centric. It works the same for any
   project. Two layers, as in AL Runner (item 5 of the AL Runner section above).

   *Layer A: which BC build supplies the Microsoft apps.*
   1. An explicit `--bc-version PREFIX` (an exact build, or a dot-segment prefix
      such as `27.5`) wins. The newest matching build in the artifact cache is
      used; if there is none, it is downloaded.
   2. Otherwise the target is the major of the project's floor. The floor is the
      higher of `application` and `platform` in `app.json`
      (`ReadMinimumBcVersion`). Within that major, the newest cached build is
      used, and if none is cached, the newest published build is downloaded.
      A stray build of another major never becomes the default.
   3. The default never selects a build below the floor. An explicit
      `--bc-version` below the floor runs, with a printed warning.
   4. No match is an error. It names the request, the builds available and the
      cache directory. It never falls back silently.

   AL Runner's default is "the build the engine was compiled for", which has no
   equivalent here because we have no .NET engine. Step 2 is therefore our
   adaptation of AL Runner's "stay within the engine's major" rule. Everything
   else in Layer A is AL Runner's behaviour.

   *Layer B: which package satisfies each dependency.* This is
   `DependencyResolver` unchanged in rules:
   1. The pool is every app package under the search directories: the project's
      `.alpackages`, then `--deps` directories, then the selected build's
      packages in the cache. Index by app id and by (name, publisher). Keep every
      version.
   2. Roots are the `dependencies` of `app.json`. The platform floors add the
      implicit roots Microsoft/System and Microsoft/Application. Each chosen
      package's own declared dependencies are followed.
   3. Lookup by app id is authoritative when the id is known. If candidates exist
      for that id but none meets the minimum, do not fall through to name and
      publisher. Fall back to name and publisher only when the id is empty or
      unknown.
   4. A declared `version` is a minimum. Among candidates at or above it, the
      highest version wins.
   5. Output is a post-order walk: dependencies before dependents. A cycle is an
      error naming the path.
   6. Candidates all below the minimum: an error naming the app, the required
      minimum and the versions found. No candidate at all: an error naming the
      app, the version, the directories searched, and the way to supply it.
      An optional dependency that is missing is skipped with a printed line.
   7. The five Microsoft platform apps (Base Application, System Application,
      Business Foundation, Application, System) ship one package per build. For
      them the minimum is advisory when the selected build's package exists but
      is below it. The run uses that package and prints a note. AL Runner does
      the same (`SelectBestVersion`).

   Differences from AL Runner, on purpose. AL Runner skips a missing Microsoft
   platform app with a warning, because it has the DLLs. We cannot run without
   the source, so a missing one is an error. AL Runner ranks R2R packages above
   source above symbol-only ones. We have one kind of executable package, source
   with bodies. A package with no AL source ranks below one that has it (this
   keeps a symbol-only copy from shadowing the real one). Source-built workspace
   packages do not exist here yet.

   *Keeping corpus runs comparable to the oracle.* The oracle junit comes from
   BC 27.5 (`oracle/oracle.toml`, `bc_version = "27.5"`), but the corpus
   `app.json` only says `27.0.0.0` for platform, application and the Microsoft
   dependencies. Run through the generic rules, the floor alone would select the
   newest 27.x build. That might not be 27.5, and any other build can change
   results. The corpus is therefore handled by the harness, not by the runtime:
   - In corpus mode (`al-test-cli --root`), `al-test-cli` reads `bc_version`
     from `oracle/oracle.toml` (read only; `oracle/` is never edited) and passes
     it as the explicit `--bc-version`. The runtime crates never read
     `oracle.toml`. Any other project gets the generic rules.
   - The corpus checkout already carries a `.alpackages` for the app
     (`corpus/tests/al-language/.alpackages`: System Application, Base
     Application, Business Foundation, Application at 27.5.46862.48827, with full
     source; checked 2026-10-13, 7866 `.al` files in Base Application, 5638 with
     a `begin` block). It is in the pool as the project's own `.alpackages`. With
     `--bc-version 27.5` the newest cached 27.5 build wins over it. With
     `--no-download` and an empty cache it is the fallback, so an offline corpus
     run is possible with no download.
   - Every run prints the full build it selected and writes it into the junit
     and the pass-rate report (`bc_build`). A build change that flips a case then
     shows in the ratchet as a regression with its cause visible.
   - Optional, not required: ask the corpus CI to record its full build number in
     its junit. Once it does, the harness passes the exact build. That is a
     corpus-repo issue under hard rule 2; this repository does not edit
     `oracle/`.

   The residual risk is builds inside one minor, such as 27.5.46862.48827 against
   27.5.46862.53931. AL Runner measured differences between BC minors as dozens of tests in one
   large project and as none on the corpus (`docs/provisioning-tiers.md`). It did
   not measure builds inside one minor. We treat same-minor skew as probably small,
   always visible, and not a blocker.

3. **Never in the repository.** Microsoft source and the downloaded zips live in
   a cache directory outside the repo (default
   `~/.cache/al-runtime/artifacts/<build>/`, `AL_RUNTIME_CACHE` to move it) and in
   a CI cache keyed by build. A test (`no_microsoft_source_in_repo`) fails if any
   tracked file carries the Microsoft copyright header. The corpus submodule's
   own `.alpackages` is the corpus repository's content, not ours; the test
   skips `corpus/`.

4. **Platform apps are apps.** The platform `System.app` is loaded as an app with
   its real id and is an implicit dependency of every app (the corpus manifest
   does not list it; BC always provides it). Dependency apps are marked in
   `AppSource` (`kind`, `version`) so that no test is discovered or run in them
   and diagnostics name the app.

5. **The install baseline mirrors AL Runner.** Item 6 of the AL Runner section
   above describes what it does. For this runtime:
   1. It runs once per run, before the first test, for the loaded dependency
      apps and then for the project's own app.
   2. Order: seed the platform rows install code reads (Published Application
      rows for the dependency apps, the session User, the Company, the Access
      Control row that makes the user SUPER). Then each dependency app's
      `Subtype = Install` codeunits in dependency order; within a codeunit
      `OnInstallAppPerCompany`, then `OnInstallAppPerDatabase`. Then the Base
      Application codeunit 2 `Company-Initialize`, once, if Base Application is
      loaded. Snapshot (ADR-004). Then the project's own rows (its Published
      Application row, the Active Session row) and its own install codeunits.
      Snapshot again.
   3. Only install triggers run. Upgrade triggers do not, as in AL Runner.
   4. Each test codeunit (and each test, under `TestIsolation = Test`) starts
      from that snapshot. Self-populating virtual tables (AllObj, Field and the
      metadata tables) are not part of it; they are computed on read.
   5. An install trigger that fails is reported with the codeunit and trigger
      name, and the failure is not swallowed. A failed `Company-Initialize` keeps
      its partial rows and is recorded as a run-level result, as in AL Runner
      (`docs/partial-company-initialization.md` there).
   6. An install trigger that cannot run yet, because a platform table it
      touches has no provider, refuses the tests that depend on that app. It is
      never skipped silently (hard rule 4).
   7. The baseline is extra roots for decision 6. Install code and
      `Company-Initialize` reach a large part of Base Application. The compile
      cost of that is measured in the Base Application increment, not assumed.
      A persistent copy of the snapshot is an optimisation (AL Runner keeps one,
      keyed by dependency set, runner build and BC version).

6. **Compilation is demand-driven.** This is the decision, as the owner asked
   ("anything is allowed as long as the test results speak for themselves").
   The workspace indexes every dependency object by id and name with a cheap
   declaration scan, parses a file only when one of its objects is needed, builds
   member tables as today, and checks and lowers only routines reachable from the
   roots. The roots are the test codeunits being run and the install baseline
   (decision 5), including event subscribers and the triggers of tables touched.
   Unreachable code is never type-checked, so unimplemented builtins in it cost
   nothing. Reachable ones stay explicit refusals on the test that reaches them.
   The official AL compiler is all-or-nothing, so this is a deliberate
   difference. The owner allows it: "anything is allowed as long as the test
   results speak for themselves". The corpus pass list is that judge. A persistent on-disk cache of the parsed and indexed form of each
   dependency, keyed by the digest of its source entries plus the digest of our
   binary, is an optimisation. Add it only if the measured warm start needs it. It
   is not a prerequisite. Lowered IR is not cached in the first increment: other
   apps' extensions merge into a table's members, so the unit of caching would
   have to be redesigned first. Budgets: the corpus-only cold compile stays at
   1 s. With dependencies, the warm start (cache present, nothing downloaded)
   targets 1 s plus the cost of what the run reaches, reported as a metric. A
   cold start with download is reported, not budgeted.

7. **Platform-native layer: only where there is no AL.** The owner's rule: "if AL
   objects in the system app exist, we are not allowed to shim them. So anything
   that exists as AL will need to be taken as AL. Anything that is a service
   internal structure, like metadata tables for example, we have to create
   ourselves."

   | What | How |
   |---|---|
   | An object that has AL source with a body (any codeunit, table, page, report in System Application, Business Foundation, Base Application, the test libraries) | Compiled and run as AL. Never replaced natively. |
   | A platform table declared in the platform `System.app` (Date, Integer, AllObj, Session, Company, User, permissions, the metadata tables, ...) | The schema is the real declaration, taken as AL. The rows come from a native table provider, registered by table id. |
   | A platform codeunit or event publisher with no body | The runtime raises it natively. |
   | A `DotNet` type | Undecided. Refused until the owner decides. See the open question and `docs/dotnet-examples.md`. |

   The registry (the ADR-008 mechanism, a second id namespace) is the only way a
   platform table becomes usable. A platform table (any table from the platform
   `System.app`, or with an id of 2000000000 or above) with no registry entry is
   refused at compile time on the test that touches it, with a diagnostic naming
   table and id: `platform table 'Date' (2000000007) has no provider`. The refusal
   is reported through `Lowered::unsupported`, as unimplemented builtins are.
   Provider kinds, mirroring AL Runner's choices: generated rows (Date, Integer,
   Time Zone), rows derived from the compiled apps (AllObj, AllObjWithCaption and
   the metadata tables), single seeded rows (Company, User, Session, Active
   Session, Published Application, the SUPER Access Control row, with the values
   in decision 9) and stored platform tables (Record Link, the permission tables)
   that use the ordinary Store with their real declaration and need an explicit
   registry entry because their platform triggers are native.

   A test enforces the boundary: for every registry entry, no loaded app supplies
   an AL object with a body under the same id or name.

8. **`DotNet`: refused until decided.** Any use of a `DotNet` variable raises
   `Signal::Unsupported` naming the .NET type when it is first called. This is the
   safe default under hard rule 4. `docs/dotnet-examples.md` has the examples
   and the three options.

9. **Identity answers mirror AL Runner exactly.** They are host configuration
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
   source. Its `DotNet NavTenantSettingsHelper` is covered by the open question.

## Seams: what changes and what does not

Changes:

- **`al-host` (ADR-006).** The trait side only: a `DependencySource` capability
  (list the packages and entries of a build, read an entry with its CRC) and the
  identity configuration above. No OS access in `al-host`.
- **A new non-core crate `al-deps`** (std::fs, an HTTP client, a ZIP reader, the
  cache, and the version selection of decision 2) implements it; `al-test-cli`
  depends on it. Downloads and file access live here. The core crates
  (`al-syntax`, `al-sema`, `al-ir`, `al-builtins`, `al-vm`, `al-data`) keep doing
  none. The selection rules are pure functions over a list of candidate
  packages, so they are tested without the network.
- **`al-sema::workspace`:** `AppSource` gains `kind` (workspace or dependency)
  and `version`; loading takes an object index so files are parsed lazily; the
  platform `System.app` is an implicit dependency. Name resolution, visibility
  and extension merging are unchanged.
- **`al-sema::lower` / `check`:** an entry-point-driven compile (check and lower
  only what is reachable from given routines) beside the whole-workspace one.
  The corpus-only path keeps its behaviour.
- **`al-builtins` (ADR-008):** the registry gains a second id namespace for
  platform table providers.
- **`al-data` (ADR-003):** table providers (rows computed on read) need a new
  cursor source behind the Store trait. That is an amendment to ADR-003, not made
  here; it gets its own ADR before the first provider is written.
- **CLI:** `--deps DIR` (repeatable), `--bc-version PREFIX`, `--no-download`,
  `--cache-dir DIR`, `AL_RUNTIME_CACHE`. In corpus mode `al-test-cli` derives
  `--bc-version` from `oracle/oracle.toml`. `oracle/` is not edited.

Does not change: the IR and its serialisation (ADR-011), the value model
(ADR-010), `Ctx` (natives receive it as builtins do), the Session/Engine split
(ADR-001), snapshots (ADR-004), hard rules 1 to 6, the oracle and the ratchet
(ADR-007).

## First increment

Goal: prove the pipeline on the apps that cost least, with no row provider and
no `DotNet` shim yet.

1. `al-deps`: version selection (decision 2) as pure functions with tests; local
   path and download resolution of the `platform` artifact; `Source.zip` and
   `System.app` reading; cache and CRC check; `AppSource` `kind`/`version`; the
   CLI flags; corpus mode passes `bc_version` from `oracle.toml`; the selected
   build is printed and written to the junit. Tests use a tiny fixture zip written
   by the test (not Microsoft's) and a loopback range server.
2. Load System Application, Business Foundation, Permissions Mock, the System
   Application Test Library and the platform `System.app` for the corpus run,
   with the platform-table refusal (decision 7) in place so the run reports 0
   fail. Expected from the measurements: pass 1308 to about 1312, the tests that
   move past "outside the workspace" appear under their next blocker, and the 160
   silent failures are refusals instead.
3. Demand-driven compile and the warm-start metric, then Base Application.
   Expected: Base App business tables resolve for their 75 tests; no new passes
   are promised, because their next blockers are builtins.
4. The ADR-003 amendment for table providers, then providers in the order the
   survey gives: Date and Integer (33 tests), Company, User, Session and Active
   Session (31, with the identity values), AllObj and AllObjWithCaption (33), the
   metadata tables (about 170). The Published Application and Access Control
   providers come in this step too, because install code reads them.
5. The install baseline (decision 5), once the providers in step 4 that install
   code reads exist. Until then an app whose install code needs a missing
   provider refuses its dependent tests.

## Rejected options

- **(b) Bundled hand-written AL shims for selected platform objects.** Rejected
  by the project owner: AL objects of dependent apps are not to be faked or
  hand-written. (A throwaway `Temp Blob` shim was used earlier to measure the
  idea; it made 3 tests pass and showed that the cost was in the next blockers,
  not the missing object. Dropped, together with the attribute design that went
  with it.)
- **(a) Native Rust implementations of dependent-app objects** such as `Temp
  Blob`. Rejected by the owner's answer 2 for any object that has AL source. Kept
  only for platform tables and codeunits that have none (decision 7).
- **(c) Symbols only (`SymbolReference.json`).** Gives the surface but no
  behaviour, and Base Application's is 59 MB of JSON; the source gives both. May
  still serve as a verifier of the parsed schema in a test.
- **(d) Keep refusing.** Leaves 516 tests dark, including core language tests
  that only use `Temp Blob` as a vehicle (`types/TestInterfaceVarCodeunitOut.al`).
- **(e) Pin the Microsoft build in the runtime to the oracle's build.** Rejected
  by the owner's answer 3: the runtime must work for any project.
- **(f) Whole-workspace compile of dependency apps with a persistent cache first.**
  Not chosen: the owner accepts demand-driven compilation and calls caching an
  optimisation (answer 4).

## Consequences

- The runtime executes Microsoft's AL, not an imitation. Every gap in our front
  end and VM that Base Application reaches becomes visible as an explicit
  refusal on the test that reaches it; the whole app produces 129 853 check
  diagnostics today, of which only the reachable fraction matters.
- A first run needs the network or a populated cache; CI needs an artifact cache
  step keyed by build.
- Corpus results depend on the BC build selected. The harness pins it to the
  oracle's minor, prints the full build, and records it in the junit. Same-minor
  build drift is possible until the corpus CI records its exact build.
- The install baseline and demand-driven compile interact. Install code and
  `Company-Initialize` are compile roots, so the cost of the baseline is a
  measured budget item.
- Platform behaviour is written once per table in Rust, with the real
  declaration as its schema, each with a corpus pin or a probe (hard rule 3).
- `DotNet` uses are refused until the owner decides. Tests that never run a
  DotNet line are unaffected.
- The warm-start budget for dependencies is new and must be measured, not
  assumed.

## Conformance

- `al-deps` tests: a fixture zip reads the same by range and whole; a CRC
  mismatch is an error; `--no-download` with a missing app fails naming it; the
  cache layout is stable.
- Version selection tests, named after AL Runner's resolver cases: the highest
  version at or above the minimum wins; an id match is authoritative and does not
  fall through to name; candidates all below the minimum give an error listing
  them; a missing app gives an error naming the directories searched; a cycle is
  an error; output is dependencies first; a Microsoft platform app below its
  minimum uses the selected build's package with a note; the default build is
  never below the app.json floor; an explicit `--bc-version` below it warns;
  a package with no AL source never shadows one that has it.
- Install baseline tests: dependency apps before the project app; per-company
  before per-database; upgrade triggers do not run; a failing trigger is reported
  with its name and not swallowed; a restored snapshot is independent of the live
  store; self-populating virtual tables are not snapshotted.
- `al-sema` tests: a platform table without a registry entry is refused with its
  name and id; a dependency app's tests are not discovered; an unreachable
  unsupported routine in a dependency does not block a test that does not reach it;
  a `DotNet` call raises `Signal::Unsupported` naming the type.
- Registry boundary test: no registry entry has an AL object with a body under the
  same id or name in any loaded app.
- `no_microsoft_source_in_repo`: no tracked file carries the Microsoft copyright
  header (the corpus submodule is excluded).
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

## Answered by the project owner (2026-10-13)

1. **Licensing.** "yes." Download Microsoft's app source into a local cache and a
   CI cache, compile it, commit nothing. (Decision 1, 3.)
2. **Native layer.** "if AL objects in the system app exist, we are not allowed to
   shim them. So anything that exists as AL will need to be taken as AL. Anything
   that is a service internal structure, like metadata tables for example, we have
   to create ourselves." Virtual and system tables with no AL counterpart get
   native row providers. `DotNet`: the owner needs more information and examples.
   (Decision 7, 8.)
3. **Pinning.** The runtime must not be corpus-centric. It should work for any
   project. The project's `app.json` dependencies (minimum versions) drive
   selection, using the same mechanism AL Runner uses today. (Decision 2.)
4. **Budget.** The official AL compiler is all-or-nothing. Lazy or demand-driven
   compilation is welcome, and "anything is allowed as long as the test results
   speak for themselves". (Decision 6.)
5. **Install triggers.** Mirror AL Runner. (Decision 5.)

## Open question for the project owner

1. **`DotNet` types in Microsoft's AL.** May the runtime contain Rust code that
   stands in for a .NET class declared in Microsoft's AL, and for which classes?
   The options, with real cases, are in `docs/dotnet-examples.md`:
   - No. Refuse every `DotNet` use. At most 20 to 28 corpus tests stay not run.
   - Yes, for standard-library classes only (`Convert`, `Encoding`,
     `MemoryStream`, `Array`, ...).
   - Yes, for both standard-library classes and Microsoft server classes
     (`NavTenantSettingsHelper`, `NavUserAccountHelper`, `ALConfigSettings`, ...),
     each backed by the identity configuration or a probe on real BC.

   Until the owner decides, decision 8 applies: refuse.
