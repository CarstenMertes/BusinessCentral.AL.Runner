# ADR-026: Incremental compile (what it would take, what it would gain)

- Status: Proposed (needs an owner decision: `docs/agents/brief.md` forbids any cache beyond ADR-022). Options 4 and 2 were
  decided and built on 2026-10-15 (see "Results of 2026-10-15"); Option 3 stays unbuilt.
- Date: 2026-10-15
- Update 2026-10-15: for the warm server (ADR-028) the in-memory part of option 3 is built, for changes in the code of
  codeunits and tables (ADR-029). The recommendation above stands for stored results and for dependency sets.
- Related: ADR-013 (demand-driven compile), ADR-016 (no features that only hide cost), ADR-022 (compile cache), ADR-011 (IR)
- Changes: nothing yet. This ADR is measurement and analysis. No code was written.

## Question (owner, 2026-10-15)

"Would incremental compilation be possible? When one to four files out of a few thousand change, recompile only those.
Can it combine with the compile cache, and help when a dependency stack (for example Base Application tests) needs
recompiling?"

## Short answer

- **Possible, but the gain is small where the owner would expect it.** The app under test is 0.1 to 2 s of a run. The
  largest part of that (about 1 s of Tests-ERM) is one serial loop that can be made faster without any cache.
- **The expensive case is a dependency set that changes: 9 to 13 s wall, 25 s CPU.** Per-file reuse does not fix it,
  because the IR refers to objects and fields by position (ADR-022, "Can lowering be done per app?"). Reuse by app
  layer (a shared prefix of the dependency order) can fix the case where a late app changes. A Microsoft hotfix changes
  Base Application, which is early in the order, so layering does not help there.
- **Recommendation: do not build per-file or per-object incremental compile. Speed up the serial reference check first.
  Build layered dependency entries only when a real case needs them.**

## Measurements

Release build of `origin/main` (1996ce3) plus temporary timing lines (not committed). `alrunner test`, BC 27.5.46862.48827
artifacts, test data off (`--no-test-data`; it does not change compile), 12 cores shared with other agents. **The
machine was never quiet: 1-minute load average 14 to 23 during every number below.** Wall seconds move by a factor of
about 1.5 with load (the same Tests-ERM hit run gave compile 2.24 s at load 16 and 3.98 s at load 20 to 22), so read
the pairs and the ratios. Where it helps, CPU seconds are given.

### 1. The app under test, with the dependency cache hit (always compiled fresh)

Tests-ERM: 290 files, 21 MB, 276 test codeunits. Tests-SCM: 202 files. Small app: 1 file, depends on Base Application.
All three use the same 11 or 4 dependency apps from one cache entry.

| Phase (wall s) | Tests-ERM hit | Tests-SCM hit | Small app hit |
|---|---:|---:|---:|
| Parse of the app's files (all cores) | 0.17 to 0.24 (CPU 1.8 to 2.7) | 0.25 (CPU 2.4 to 2.7) | 0.00 |
| Merge + `resolve_references` (serial, workspace only) | 1.04 to 1.12 (1.8 at load 22) | 0.88 to 0.94 | 0.00 |
| Declared symbols (`members`) | not separate: built lazily per object inside check | same | same |
| Check of the workspace | 0.37 to 0.45 | 0.43 to 0.50 | 0.01 |
| Lower of the workspace (+ 14 or 0 re-lowered dependency blocks) | 0.29 to 0.52 | 0.37 to 0.82 | 0.08 |
| Read and decode the dependency blocks | 0.17 to 0.47 | 0.21 to 0.28 | 0.58 |
| "compile" line of `--timings` | **2.24 to 3.98** | **2.45 to 3.05** | **0.83** |
| Load average at start | 14 to 22 | 17 to 18 | 16 |

- **The app-under-test share (parse + resolve + check + lower) is about 1.9 s for Tests-ERM and 1.8 s for Tests-SCM
  (load 15 to 18).** The whole run is 14 to 21 s with tests, so this is 10 to 14 % of a full run.
- **`Workspace::resolve_references` is more than half of it** (`workspace.rs`, `resolve_references`): one serial loop
  over every workspace object that walks all its type references. It does not depend on the dependency set (0.004 s for
  the small app). It runs on every run, also when a selection compiles only one codeunit (Tests-ERM with one codeunit:
  load 1.28 s, check 0.65 s, lower 0.17 s, at load 16).
- **With a selection, check and lower shrink** (only the selected codeunits compile), but parse and `resolve_references`
  do not. An edit-and-rerun loop on one codeunit of Tests-ERM spends most of its compile in those two.

### 2. Without the cache, for comparison (`--no-compile-cache`)

| | Tests-ERM | Tests-SCM | Small app |
|---|---:|---:|---:|
| load (scan of 9 983 / 9 564 dependency files + parse of the app) | 1.64 | 1.53 | 0.20 |
| demand check | 2.33 | 3.39 | 5.33 |
| lower | 3.53 | 1.63 | 2.13 |
| "compile" line | **8.81** | **7.35** | **8.26** |
| Load average at start | 17 | 18 | 17 |

### 3. A dependency set that must be built (cache miss)

The builder compiles every dependency object, eagerly: `lower_for_cache` (scan, check, lower+export), then
`al_cache::build` (encode and zstd).

| Set | Scan | Check (eager) | Lower + export | Encode + zstd | Wall of "built in" | Load avg |
|---|---:|---:|---:|---:|---:|---:|
| 11 apps, 9 919 objects (Tests-ERM closure) | 0.09 to 0.13 | 1.7 to 2.3 | 2.5 to 3.0 | 2.2 to 3.1 | **8.9** (5.1 to 8.9 over 4 runs) | 15 to 19 |
| 4 apps, 9 504 objects (System, System Application, Business Foundation, Base Application) | 0.14 | 2.6 | 3.9 | 2.7 | **8.2 to 10.7** | 16 to 19 |
| 5 apps (those 4 + a 1-file app "Dep Two") | 0.14 | 2.7 | 4.8 | 3.2 | **12.6** | 22 |
| Same 5 apps, one file added to "Dep Two" | 0.19 | 2.6 | 3.9 | 2.5 | **10.9** | 23 |

- **Compiling Base Application from source with no cache is this miss cost: 8 to 13 s wall, about 25 s CPU** for the
  whole `alrunner test` process of the small app (user 24.9 s, sys 2.5 s, 12 s wall). Base Application is 79 % of
  the objects (ADR-022).
- **Of the miss, parsing the dependency files is 4.1 s CPU** (9 919 files, lazy parse inside check and lower), so
  about one sixth of the build CPU. Check and lower are the rest, plus 2.2 to 3.1 s of encode.
- **A hit afterwards costs 0.38 to 0.83 s of compile** (the 5-app set: 0.38 s).

### 4. When one app in a set changes

Confirmed by experiment and by `compile_cache.rs`: `key_with` puts every app's source hash, and the hash of the
running executable, into one `Key`. **One added file in one small app of a 5-app set gave a new key and a full rebuild:
10.9 s wall** (second row of the 5-app measurement). Nothing of the Base Application layer is reused. A change to
`alrunner` itself (any commit) also invalidates every entry.

## Analysis: what an incremental compile needs in this codebase

### Which phases are per file and which are whole-app

| Phase | Unit | Depends on other files? | Code |
|---|---|---|---|
| Lex, preprocess, parse | file | Preprocessor symbols only. Page expression properties become routines (`expand_ranges`), still per file. | `workspace.rs` `parse_file` |
| Declaration scan (objects, extends, implements, subscribers) | file | No. | `scanned_file`, `FileIndex` |
| Name and extension indexes (`by_name`, `ext_of`, `subscribers_of`, `implementers_of`) | whole set | Yes: every object. Cheap (ms). | `index_extensions_and_subscribers` |
| `resolve_references` | object | Reads the index, per object. | `workspace.rs` |
| Members (fields, procedures, enum values) | object **merged with its extensions** | Yes: a table extension in file F changes the members of its base table and of everything that reads that table. Field order is "own fields, system fields, extension fields in workspace object order". | `members.rs` `merged_members` |
| Check | routine | Reads members of other objects, signatures of called procedures. | `check.rs` |
| Lower | object | Reads a small set of facts of other objects: table behind a page, page parts, routine numbering, query layout, who extends and implements (`ObjectFacts` in `provided.rs`). | `lower.rs` |
| Program assembly | whole set | Positions: objects and fields are positions in `Program::objects` and in the merged field list (about 35 places in `al-ir`). | ADR-022 findings 1 and 2 |

So **parse and scan are cleanly per file. Members, check and lower are per object but with cross-file inputs**, and the
IR output is positional. Adding or removing an object, or a field in an extension, shifts positions of everything
that comes later.

### What a dependency graph would have to hold

For each object: (a) the files it was parsed from, (b) the members it read (`object, member`), (c) the lower-time facts
it read, (d) the objects it embeds (ADR-022 already stores `embeds` for this purpose, and `relower_set` already
re-lowers cached blocks when the workspace touches an `embeds` object). A change then invalidates: the objects in the
changed files, every object that read a changed member or fact, and the group of objects that extend each other.
A signature change reaches every caller. A new field in a table extension reaches every object that reads that table,
plus all positions after it.

The record of what each check and lower **read** does not exist today. The checker and lowerer read through
`MemberTable` and `Shared` without logging it. Adding the log is the main cost: about 30 to 50 read sites, and it must
be complete (see risks).

### What could be reused, and what it is worth

| Reusable piece | Key | Saves (measured) | Notes |
|---|---|---|---|
| Parse tree per file | content hash | Tests-ERM 0.17 s wall (CPU 1.9 s) per run; 4.1 s CPU of a dependency build | Needs a serialized rowan tree. Parse runs at 50 MB/s or faster (perf budget), so a decode may not beat it. Not worth a cache. |
| Declared symbols per object | object source + extensions' hashes | Small (members are lazy and cheap; 42 to 69 ms to decode all of a stack, ADR-022) | Already inside ADR-022 entries for dependencies. |
| `resolve_references` result per file | content hash + index digest | About 1.0 s of Tests-ERM | **Better fixed by making the loop cheaper (see Option 4).** |
| Checked and lowered IR per object | source hash + read log | Tests-ERM check + lower 0.7 to 1.0 s; a dependency miss 6 s CPU+ | Positional IR: blocks of a changed workspace shift. Needs the read log. |

### The gain, from the measurements

For the **app under test** (X = the compile of the app, Y = the compile of four changed files plus dependents):

- Tests-ERM hit run: X = parse 0.17 + resolve 1.05 + check 0.4 + lower 0.35 = **about 2.0 s** (load 15 to 17).
- Y: parse, resolve, check and lower scale with files, 4 of 290 = 1.4 %: about 0.03 s. Dependents are unknown: a
  changed procedure signature in a shared test library file can reach tens of files, and a changed table extension
  field reaches every object reading that table. A fair range is **Y = 0.05 to 0.5 s**.
- **Gain = X - Y = about 1.5 to 1.9 s per run**, against a full run of 14 to 21 s (7 to 13 %). With a codeunit
  selection the run is 3 to 5 s and the gain is the same 1.5 s, which is 30 to 50 % of that run. The small app gains
  nothing (its whole compile is 0.1 s).
- It would need a **persisted per-file/per-object result of the app under test**, which is the exception to the
  owner's decision "workspace apps compile on every run" (ADR-022 decision 1) and to the brief's "do not add another
  cache". Without persistence there is nothing to reuse between runs (no watch mode, ADR-016).

For a **dependency set that changes**:

- Today: 9 to 13 s wall for any change in any app (table 4).
- Per-object reuse across versions of one app (a Base Application hotfix touches few files): blocked by positional IR.
  A new or removed object or extension field shifts everything after it, so blocks would need a remap pass at decode,
  or symbolic references. The gain is at most the ~9 s of one miss, once per hotfix per machine. CI builds the entry
  for CI runners anyway (ADR-022).
- Layered entries (Option 2 below): a change in app k of the dependency order rebuilds only apps k..n. The measured
  build cost is about 0.9 ms of wall per object at load 15 to 19 (8.9 s for 9 919 objects); a customer library of 300
  objects would then cost about 0.3 s to build as a layer instead of 9 to 13 s for the whole set. This is an
  estimate from that ratio, not a measurement of a layered build.

## Options

1. **Do nothing.** Cost: none. The dependency miss stays 9 to 13 s once per set and per `alrunner` build; the app
   under test stays about 2 s for a Tests-ERM-sized app.
2. **Layered dependency entries (prefix cache).** One entry per prefix of the dependency order, instead of one per
   set. Apps are ordered by a canonical topological order (ties broken by app id), so the System/Base prefix is the
   same for every run. Layer k stores the objects of app k with positions that assume the prefix before it. A run takes
   the longest prefix that hits, compiles the rest as ordinary dependency apps (eager, as `lower_for_cache` does), and
   stores the new layers.
   - Reuses the existing `compile_loaded_cached` / `dependency_prefix` / `relower_set` machinery. Layer k can touch
     (extend) objects of earlier layers, so those earlier blocks have to be re-lowered with layer k, the same way the
     workspace does it today, and layer k's entry carries the overriding blocks.
   - Helps: a changed or newly installed library app late in the order (a partner library, a customer base
     extension, a test library). Estimated 9 to 13 s down to 0.3 to 2 s.
   - Does not help: a Base Application hotfix (it is early, so every later layer is stale), nor a change to `alrunner`
     (the executable hash is in every key).
   - Cost: about 2 weeks. Risk: medium. Key construction and layer overrides are new places for a stale entry.
3. **Per-object incremental check and lower with a read log and a dependency graph**, for the dependency set and the
   app under test. Needs the read log (30 to 50 sites), a stable-reference IR or a remap pass at decode, and a
   persisted result for the app under test.
   - Helps: a Base Application hotfix (few files change: miss goes from 9 to 13 s to maybe 1 to 3 s), and the app
     under test (gain 1.5 to 1.9 s per run, table above).
   - Cost: 6 to 10 weeks. Risk: high. A missed read is a stale result that no test will show until it is wrong.
4. **Make the serial work cheaper (no cache).** `resolve_references` is a serial loop of about 1.0 s on Tests-ERM
   (0.9 s on Tests-SCM), for 290 files. It can run per file on all cores (the lookups it makes are reads of
   an index that is built before the loop). It can also be skipped for objects that no selected test reaches. Gain:
   about 0.8 s of the 2.0 s. This is "make the work itself cheaper" (brief, Performance), needs no ADR and no cache.
   Cost: days. Risk: low (the diagnostics list must stay in the same order; the existing parallel merge already shows
   how).
5. **Per-file parse cache.** Rejected: parse is 0.17 s wall for Tests-ERM and 4.1 s of CPU for a full dependency
   build; a serialized tree must be decoded (allocation-bound, ADR-022 section 2), so it is unlikely to win.

## Decision (proposed)

1. **Do not build per-file or per-object incremental compile** (Option 3) now. The gain on the app under test is
   1.5 to 1.9 s of runs that last 3 to 21 s, it needs a new persisted result of the code under test, and a missed read
   gives stale results.
2. **Do Option 4 first**, under the brief's performance rule: parallelize `resolve_references`. It takes the biggest
   piece of the app under test without storing anything. Re-measure with `--timings` on a quiet machine.
3. **Keep Option 2 (layered entries) as the next step, built only when a real case shows up**: a project that edits a
   library app which sits under the tests, in the dependency list. If the owner wants it earlier, accept this ADR with
   Option 2 and ask for its own design ADR, since it changes the key and the entry layout of ADR-022 (a cache format
   change, `al_cache::FORMAT` rises).
4. **The rule from ADR-022 and the owner's zero-difference decision stays for any cache work**: the cached and the
   uncached run give the same result for every test (`tools/cache_vs_uncached.sh run`, no known differences).

## Consequences

- No code change and no format change from accepting this ADR as written, except that Option 4 becomes the next
  performance item.
- The first-run cost of a changed dependency set stays 9 to 13 s until Option 2 is built.
- Every `alrunner` build rebuilds every entry, as today.

## Risks (for Options 2 and 3, if the owner chooses them)

- **Stale reuse.** AL Runner v2 restored stale rows from a key that missed an input (ADR-022). The same would happen
  if one read of the checker or lowerer is missing from the log (Option 3) or if a layer's key leaves out an override
  of an earlier layer (Option 2). Mitigation: derive the log in the code that reads, not beside it; run the
  differential below on every change; fail closed (rebuild) on any doubt.
- **Position shifts.** Adding or removing an object changes positions after it. Layers avoid this by fixing the prefix.
  Per-object reuse across versions does not, without a remap or symbolic IR (a seam change, ADR needed).
- **Zero difference.** `tools/cache_vs_uncached.sh run` must show 0 differences, as for ADR-022. Add a mode that
  changes 1 to 4 files and compares the incremental run with a full compile of the same sources (`Program`, refusals,
  call graph, and every test outcome).
- **Test isolation of timing.** The numbers here were taken at load 14 to 23. The ratios hold, the seconds do not.
  Repeat on a quiet machine before building.

## Conformance

For Option 4: the diagnostics order test for a workspace with unresolved references, the corpus ratchet, and
`al-test-cli --timings` before and after on Tests-ERM. For Options 2 and 3: the differential above, a test that
changes one file of a library app in the middle of a set, and `tools/cache_vs_uncached.sh run` with 0 differences.

## Results of 2026-10-15 (owner decision: build Options 4 and 2; branch `layered-cache`)

The owner decided that a change in a developer's own code must never recompile Base Application, and asked for
Option 4 (parallel `resolve_references`), Option 2 (layers, in ADR-022 "Owner decision 2026-10-15") and a measurement of a
large partner app. The machine was never quiet (1-minute load average 14 to 36 from other agents' builds), so ratios
and pairs are given, not single numbers. Each figure has its load average.

### Option 4: `resolve_references` runs per object on all cores

The loop is the same code, run with rayon over the objects and collected in object order, so the list of diagnostics is
the list the serial loop gave (a test with 300 files and five unresolved references each checks the exact list ten
times; the final sort by file and offset is unchanged). The time of the call alone, serial against parallel:

| Workspace | Serial | Parallel | Load average |
|---|---:|---:|---:|
| NP Retail (7 234 + 425 files), two runs each | 0.92, 1.06 s | 0.15, 0.32 s | 15 to 16 |
| Tests-ERM (290 files), best of three | 0.81 s | 0.51 s | 24 |
| Tests-ERM, five interleaved pairs | 1.85, 3.48, 4.49, 1.64, 4.74 s | 2.04, 1.63, 0.97, 0.96, 0.97 s | 22 to 36 |

At a load of 24 on 12 cores a parallel loop gets only the cores the other agents leave free, so Tests-ERM shows less
than NP Retail. On a quiet machine the call is bounded by the 12 cores: about 1 s of CPU. No cache is involved.

### Option 2: layers

Built, see ADR-022. One changed file in a partner library over the 4 Microsoft apps: 6.0 s before, 1.4 to 2.3 s after
(load 14 to 16); the Microsoft layer is not touched. The ADR-026 measurement of 10.9 s was for the same kind of case at
load 23.

### A large implementation app: NP Retail (NaviPartner, `npcore-partners`; the code is read in place, a scratch copy
is edited, nothing of it is committed)

NP Retail is 7 234 files and 7 234 objects, 152 MB with its media. Its test app has 425 files, 313 test codeunits and
4 813 tests. The build is BC 27.5.46862.48827. Two ways to run it:

**1. NP Retail as the app under test** (`alrunner test Application Test`, selection of one codeunit, `--timings`). The
workspace compile by phase, wall seconds, with the Microsoft layer a cache hit:

| Phase | Before this work | After (parallel `resolve_references`) |
|---|---:|---:|
| Load (read, parse 7 659 files, `resolve_references`) | 1.55 (of which `resolve_references` 0.92) | 0.63 (of which 0.15) |
| Check of the workspace | 0.93 | 0.69 |
| Lower of the workspace | 0.98 | 0.97 |
| **Workspace compile** | **3.5** | **2.3** |
| Load average | 16 | 15 |

A second pair at a higher load gave 4.1 s before and 5.2 s after (load 16 to 20; the check and lower phases moved by a
factor of two with the load, `resolve_references` alone did not rise). Reading the dependency layer adds 0.2 to 0.7 s
(blocks) and the install baseline 0.4 to 0.8 s. The workspace has 6 AL0118 errors ("The name 'Run' does not exist in
the current context", six `__Legacy` scanner codeunits) that stop its run; they are a finding for another task.

**2. NP Retail as a dependency of the test app** (the case of an implementation app in `.alpackages`): 17 183
dependency objects, 7 234 of them NP Retail's. Rebuild after one changed procedure in one NP Retail file:

| | One entry per set (before) | Layers (after) |
|---|---:|---:|
| Rebuild, wall | 14.8, 24.5, 25.6, 29.0 s (load 17 to 25) | 11.2, 11.9, 12.4 s (load 22 to 23); 7.7 s (load 16) |
| Of which Microsoft's apps | the same as the rest | none (a hit) |
| Cold build | 17.0, 18.8 s (load 21 to 26) | 20.6 to 28.6 s (load 23 to 25) |

The layer is 42 % of the objects and costs 1.0 to 1.5 ms per object plus about 1.3 s fixed, so **a large partner app is
helped by about a half, not by a factor of ten.** A library of 100 to 300 objects takes 1.4 to 2.3 s.

### What a per-object incremental compile of the workspace would save (estimate, nothing implemented)

Method: the phases scale with the number of files for parse, `resolve_references`, check and lower. For one to four
changed files of 7 659, the own work is about 0.05 % of the workspace compile, 1 to 5 ms. The dependents are the
unknown (ADR-026 table above): a body-only change has none, a changed signature of a widely used procedure or a new
field in a table extension reaches the objects that read it, which for NP Retail can be hundreds. At 1 to 3 ms per
object, 100 dependents cost 0.1 to 0.3 s. The fixed part that stays: the dependency layers' blocks (0.2 to 0.7 s), the
install baseline (0.4 to 0.8 s).

| Case | Full compile | Incremental, estimated | Saved |
|---|---:|---:|---:|
| NP Retail as the app under test, 1 to 4 files changed | 2.3 to 4.1 s | 0.1 to 0.5 s | 2 to 3.5 s per run |
| That, as a share of a run | | | 20 to 40 % of a run that selects one codeunit (5 to 10 s); 3 to 5 % of the full 4 813-test run (50 to 80 s) |
| NP Retail as a dependency layer, 1 to 4 files changed | 7.7 to 12.4 s | 1.4 to 2 s (the 1.3 s fixed cost stays) | 6 to 10 s per rebuild |

The second row is the larger win for a project like NP Retail, and it needs more than per-object reuse: the layer file
would have to be updated in place (positions of unchanged objects stay if no object is added or removed and no
extension field is added), which is Option 3's read log again. The cheaper steps on the same layer that are not built:
skip the read and scan of the layers below (the `.alci` index already holds the scan) and decode no blocks of the layers
below that are not needed (the fixed 1.3 s), and use the cores better in the layer build (check 1.1 s, lower 1.8 s,
drop 0.65 s and encode 1.8 s for NP Retail's layer ran one after the other).

## Prior art: Microsoft RAD (research of 2026-10-15)

Source: Microsoft Learn and the microsoft/AL issue tracker. No Microsoft code was read or copied.

- **RAD is incremental publishing, not an incremental whole-app compiler.** The AL extension for VS Code records the
  changed objects of the project in `.vscode/rad.json` (added, modified, removed, by file) on each build. A RAD publish
  compiles that delta as its own unit against the app that is already published and installs it. Microsoft says it
  "is an interim state and doesn't replace a full publish"; the app that ships is always a full build.
  [Learn: Work with RAD](https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/devenv-rad-publishing),
  [microsoft/AL #7976](https://github.com/microsoft/AL/issues/7976).
- **RAD does not recompile dependents of a changed object.** It trusts the server to catch a break. Our checker has no
  server, so it would need the read log of Option 3 or a full compile (inference from the sources above).
- **The compiler reuse is in the editor process.** `al.incrementalBuild` lets a build finish the language server's
  background compilation. It is not a cross-process cache, so a cold command-line run has nothing to reuse.
  [Learn: AL extension configuration](https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/devenv-al-extension-configuration),
  [microsoft/AL #6277](https://github.com/microsoft/AL/issues/6277).
- **Dependencies are handled per app.** Symbols come from `.app` files. In a workspace the project references are
  resolved from the live project, and publishing a base app uninstalls and recompiles the apps that depend on it as
  whole apps. ALTool's `graph extract-all` is incremental per app, keyed by a content hash. This matches the layers of
  this ADR (one unit per app, a change rebuilds the app and the apps above it).
  [Learn: project references](https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/devenv-work-workspace-projects-references),
  [Learn: ALTool](https://learn.microsoft.com/en-us/dynamics365/business-central/dev-itpro/developer/devenv-al-tool).
- **What RAD leaves out on purpose:** translations, permissions, report layouts, table data and web service
  definitions; object ID renames are not detected.
- **Its bug list is a list of test cases for a differential check** of any per-object incremental compile here (each
  one a case that changes one or a few files and is compared with a full compile): deleted objects, renamed or moved
  files and case-only renames (stale or duplicate objects, [#6214](https://github.com/microsoft/AL/issues/6214),
  [#6077](https://github.com/microsoft/AL/issues/6077)), a reverted file (git revert, [#5561](https://github.com/microsoft/AL/issues/5561)),
  page extensions ([#5594](https://github.com/microsoft/AL/issues/5594)), report request pages
  ([#8222](https://github.com/microsoft/AL/issues/8222)), permission objects ([#7804](https://github.com/microsoft/AL/issues/7804)),
  a bogus "removed" entry ([#5686](https://github.com/microsoft/AL/issues/5686)), and test codeunits not found after a delta
  publish ([#7227](https://github.com/microsoft/AL/issues/7227)).
- **Consequence for this ADR:** Microsoft made the same split as the measurements here. Reuse for the app being edited
  lives in the editor; across apps the unit is the app. That supports layers (Option 2, built) over per-object reuse
  across versions (Option 3, not built).
