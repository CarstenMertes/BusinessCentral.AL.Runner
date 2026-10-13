# Status and roadmap

As of `main` on 2026-10-13. Oracle: BC 27.5, corpus `cf6e546`, CI run 38306086475.
Regenerate the numbers with
`./target/release/al-test-cli --root . --markdown pass-rate.md`; CI uploads
the same report as a `pass-rate-<date>` artifact.

## BC versions (ADR-015)

One oracle and one ratchet per BC major. `al-test-cli --root .` runs the newest;
`--bc-version N` picks one; `--all-versions` runs all three and compares them.

| BC major | Oracle leg | Baseline | Passing | Version gates active |
|---|---|---|---:|---|
| 27 | 27.5 | `metrics/27/passing.txt` | 2885 | none |
| 28 | 28.5 | `metrics/28/passing.txt` | 2885 | none |
| 29 | 29.0 | `metrics/29/passing.txt` | 2885 | Integer sum into BigInteger FlowField; min and max over SystemRowVersion |

The numbers in the rest of this file are from before the version work and are
refreshed after merging.

## Overall

**2412 / 4836 pass (49.9%), 0 fail, 2424 not run.** Compiling the corpus
takes about 0.5 s cold, and the whole run, compile plus every test, about 1 s.

| Area | Total | Pass | Not run | Rate |
|---|---:|---:|---:|---:|
| xml | 90 | 89 | 1 | 98.9% |
| testpageteardown | 105 | 105 | 0 | 100% |
| text | 132 | 126 | 6 | 95.5% |
| testpageclose | 36 | 34 | 2 | 94.4% |
| streams | 47 | 44 | 3 | 93.6% |
| json | 117 | 108 | 9 | 92.3% |
| autoformat | 39 | 36 | 3 | 92.3% |
| recordref | 108 | 99 | 9 | 91.7% |
| collections | 84 | 71 | 13 | 84.5% |
| error-handling | 81 | 62 | 19 | 76.5% |
| tableextension | 31 | 22 | 9 | 71.0% |
| types | 350 | 236 | 114 | 67.4% |
| pageupdate | 32 | 20 | 12 | 62.5% |
| codeunit | 245 | 149 | 96 | 60.8% |
| fieldref | 126 | 68 | 58 | 54.0% |
| record | 1018 | 533 | 485 | 52.4% |
| tableextensiontrigger | 13 | 6 | 7 | 46.2% |
| handlers | 1115 | 497 | 618 | 44.6% |
| session | 427 | 80 | 347 | 18.7% |
| testpart | 80 | 71 | 9 | 88.8% |
| query | 129 | 0 | 129 | 0% |
| permissions | 58 | 0 | 58 | 0% |
| pageapplicationarea | 76 | 0 | 76 | 0% |
| xmlport | 40 | 25 | 15 | 62.5% |
| other areas (17) | 257 | 17 | 240 | |

## What is implemented

- **Front end.** Full lossless parse of the corpus. Namespaces and `using`s,
  extensions, preprocessor symbols, overload resolution, implicit
  conversions.
- **Language.** All statement forms. TryFunction. Labels. Interfaces through
  codeunit variables. List and Dictionary. Option and enum. Date, Time,
  DateTime and Duration arithmetic. Format and Evaluate for the pinned forms.
- **Records.** CRUD, triggers, filters (SetRange, SetFilter, filter groups,
  marks), keys, Validate with TableRelation checks, Rename with propagation to
  related tables, FlowFields and FlowFilters (all CalcFormula methods),
  CalcSums, SetAutoCalcFields, AutoIncrement, Truncate, temporary records,
  SystemId and SystemRowVersion, transactions (Commit, `asserterror`
  rollback, test-boundary commit).
- **RecordRef, FieldRef, KeyRef and RecordId.**
- **Events.** Integration, business and internal publishers. Static and Manual
  subscribers, BindSubscription, table events in BC's pinned order, table
  extension triggers, Isolated events.
- **Data formats.** JSON, XML DOM with simple XPath, Blob streams, text
  functions, TextBuilder, BigText, SecretText.
- **XmlPorts (Format = Xml).** `Import` and `Export` (instance and static),
  `SetSource`, `SetDestination`, `SetTableView`; table, field, text elements and
  attributes, nested table elements with `LinkFields`, `AutoSave`, `AutoUpdate`,
  `AutoReplace`, `UseTemporary`, and the triggers the corpus uses. The 15
  remaining xmlport tests need dependency apps (ADR-013).
- **Test runner.** Install codeunits. `[HandlerFunctions]` with Message,
  Confirm, StrMenu, page and modal page handlers.
- **Pages (ADR-012).** TestPage on Card and List pages: lifecycle,
  navigation, SetValue and validation, actions, page triggers,
  `CurrPage.Update`, `Page.Run` and `RunModal`, lookup mode, SaveValues,
  control-text formatting. Parts and `TestPart` (SubPageLink, SubPageView,
  Provider, navigation, new rows with AutoSplitKey, `CurrPage.<Part>.Page`
  calls), `TestFilter`, `Page.SetRecord`, SourceTableTemporary pages.

## Largest remaining blockers

Each test is counted under the first thing that stops it.

| Tests | Blocker | Plan |
|---:|---|---|
| ~550 | Objects outside the workspace: tables (452), codeunits (51), pages (46) | ADR-013 (Proposed): compile Microsoft's real apps from the artifact feed; native providers for system tables |
| 23 | Tests that use parts but stop on something else: `DMY2Date` (7), `Session.ApplicationArea` (5), RunObject action (4), the Integer system table (4), page extensions (3) | Each is counted under its own blocker |
| 121 | Query objects | Not started |
| 96 | Request and filter pages | ADR-012 follow-up |
| 94 | Page extensions | ADR-012 follow-up |
| 73 | Global Triggers codeunit events (Base App) | Arrives with ADR-013 |
| 73 | RunObject actions | ADR-012 follow-up |
| 58 | `[TransactionModel]` | Not started; corpus pins it (record/TestTransactionModelAutoRollback.al) |
| 43 | Guarded `Codeunit.Run` (`if Codeunit.Run(...)`) | Not started |
| ~50 | Single builtins that remain: Session.ApplicationArea, TestFilter, FilterPageBuilder and Report.Run (page and report infrastructure), Today, CurrentDateTime and WorkDate (host clock), UserSecurityId and SessionId (host identity), Record.SetLoadFields (partial records), error collection (`[ErrorBehavior]`) | Needs the objects or host values named; see `docs/probes-needed.md` for what the done ones assume |
| 31 | SystemCreatedAt/By and SystemModifiedAt/By are not stamped | Needs the clock and identity host values (ADR-013 identity table) |

## Open decisions for the project owner

ADR-013 (Proposed, revised 2026-10-13) now waits on one question and on the
owner accepting the revision.

1. **`DotNet` types in Microsoft's AL.** May the runtime contain Rust code that
   stands in for a .NET class declared in Microsoft's AL? Options: no (refuse
   every `DotNet` use); yes for standard-library classes only (`Convert`,
   `Encoding`, `MemoryStream`, ...); yes for those and for Microsoft server
   classes (`NavTenantSettingsHelper`, `NavUserAccountHelper`, ...). At most 20
   to 28 corpus tests are affected. Real cases are in `docs/dotnet-examples.md`.
   Until decided, a `DotNet` call is refused.

Answered on 2026-10-13 and recorded in ADR-013:

- Licensing: download Microsoft's app source into a local cache and a CI cache,
  compile it, commit nothing.
- Native layer: AL that exists is taken as AL and never shimmed. Platform
  tables with no AL body get native row providers.
- Version selection: driven by the project's `app.json`, using the same
  mechanism as AL Runner. Not corpus-specific. The corpus harness passes the
  oracle's BC minor.
- Budget: demand-driven compilation. A persistent cache is an optimisation.
- Install triggers and Company-Initialize: mirror AL Runner.

## Known issues and debt

- `GetLastErrorCallStack` returns a marker that refuses to be read; real call
  stacks need source lines in the IR.
- Red/green discipline slipped in places during the parallel build-out (the
  first cut of event dispatch, later FlowField formula tests, some TestPage
  fixes). Those tests were checked by mutation, but were never seen red.
  Review them when the code is next touched.
- About 135 assumptions in `docs/probes-needed.md` wait for probes on real BC.
  They include the exact "Unhandled UI" text, JSON and XML `WriteTo`
  formatting, and the `Format` of Time and DateTime.
- `al-host` is still a placeholder. Clock, identity and network go through it
  once ADR-013 lands.

## Next targets after the corpus (owner, 2026-10-13)

Start these when the corpus is green or nearly green and nothing big is left:

1. **Microsoft's own Base App test buckets.** AL Runner has a skill for this:
   `BusinessCentral.AL.Runner/.claude/skills/running-ms-test-buckets/SKILL.md`.
   It measures how many of Microsoft's own test cases this runtime can run.
   The tests need CRONUS demo data (`--test-data`), not an empty database.
2. **CRONUS demo data in native Rust.** `bcdb` (repo `BusinessCentral.BakReader`, C#) reads BC
   data straight from a `.bak` or `.bacpac` file, with no SQL Server. AL Runner uses it
   for `--test-data`. Port it to Rust: here at first, possibly in that repo later.
   This gives speed and native support for loading CRONUS into the store.
