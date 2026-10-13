# Corpus inventory

Corpus commit `cf6e546` (the `corpus` submodule pin). Oracle: CI run
38306086475, BC 27.5 — **4836 cases, 0 failures**. Regenerate the per-area
numbers with `python3 tools/inventory.py corpus/tests/al-language oracle/junit-27.5.xml`.

## Apps in the corpus

| App | `.al` files | Role |
|---|---:|---|
| `tests/al-language` (Target = Cloud) | 1330 | The test app. Everything below refers to it. |
| `tests/al-language-internals-fixture` | 8 | A dependency of the test app (CI publishes it first). Must be loaded as a second app: some `codeunit/` tests call its objects across the app boundary (`ALT Internal Codeunit`, `ALT Cross Compute`). |
| `tests/al-language-onprem` (Target = OnPrem) | 9 | Not in the 27.5 oracle (separate CI job). Out of scope. |
| `tests/al-language-isolation-probe` | 5 | Not in the CI test run. Out of scope. |

The original brief's "519 files / 273 test codeunits" is out of date: the test app
now has 1330 files and 643 test codeunits.

## First-task answers

**Assert codeunit.** The corpus ships its own `codeunit 60021 Assert`
(`_fixtures/Assert.al`), modelled on Microsoft's Library Assert but written in
plain AL. No native shim is needed: the runtime executes it like any other
codeunit. It does lean on `Variant` type inspection, `RecordRef`, `GetLastErrorText`,
`GetLastErrorCode` and `StrPos`, so those are on the M2 critical path.

**Handler attributes** (count of uses, whole test app):

| Attribute | Uses | Attribute | Uses |
|---|---:|---|---:|
| `HandlerFunctions` | 356 | `ConfirmHandler` | 22 |
| `ModalPageHandler` | 185 | `SessionSettingsHandler` | 5 |
| `RequestPageHandler` | 158 | `HttpClientHandler` | 3 |
| `PageHandler` | 84 | `StrMenuHandler` | 2 |
| `MessageHandler` | 31 | `SendNotificationHandler` | 2 |

Other attributes the VM must understand: `Test` (4924), `EventSubscriber` (121),
`TryFunction` (79), `IntegrationEvent` (66), `TransactionModel` (25),
`CommitBehavior` (16), `ErrorBehavior` (8), `NonDebuggable` (8), `Scope` (5),
`BusinessEvent` (4), `InternalEvent` (1), `TestFilterField` (1).

In the M2–M4 areas handlers are rare (`codeunit` 4, `record` 14, `session` 5,
`types` 1); `MessageHandler` / `ConfirmHandler` are the ones needed early.

**Preprocessor symbols.** Only `BC29PLUS` (6× `#if`, 1× `#if not`) and one
`BC143PLUS`. No `BC28PLUS` guard is currently used. Emulating 27.5 means
`BC27PLUS` defined, everything else undefined (ADR-007).

**Fixtures per area** (top references): every area uses `ALTFixtureCleanup`.
`record` is dominated by `ALT Universal` (433), then `ALT Trigger Log`,
`ALT Triggered`, `ALT Parent`/`ALT Child`, `ALT Link Host`, `ALT Base`,
`ALT Keyed`. `codeunit` uses `ALT Event Publisher`, `ALT Manual Event Sub`,
`IALTCompute`/`ALTDouble`. `streams` uses `ALT Blob`. `json`, `xml`, `text`
use no table fixtures beyond cleanup.

## Cases per area (oracle, BC 27.5)

| Milestone | Areas | Cases |
|---|---|---:|
| M2 pure semantics | types 350, text 132, collections 84, codeunit 245, error-handling 81 | 892 |
| M3 record engine | record 1018, recordref 108, fieldref 126 | 1252 |
| M4 stretch | json 117, xml 90, streams 47, session 427 | 681 |
| Out of scope | handlers 1115, query 129, xmlport 40, profile 16, page*/testpage*/test* ~540, permissions 58, install 32, … | 2011 |

`handlers/` alone is 23% of the corpus; the tier-1 ceiling is therefore
2825 / 4836 = 58% overall, and 100% of each in-scope milestone.

## Dependencies outside the corpus (tier-1 risk)

The test app declares dependencies on Base Application, System Application,
System Application Test Library and Permissions Mock. Declared variables
referring to objects the corpus does not define, in the in-scope areas:

| Area | External objects referenced |
|---|---|
| types | `Codeunit "Temp Blob"` (System App, 11), `Enum "Printer Paper Kind"` (8) |
| codeunit | `Record "Code Coverage"` and friends (system tables), `Record "CodeUnit Metadata"` |
| error-handling | `Codeunit "Temp Blob"` (1) |
| record | system / virtual tables: `CodeUnit Metadata` 31, `Record Link` 26, `Date` 25, `Field` 23, `AllObjWithCaption` 21, `Page Control Field` 19, `AllObj` 19, `Table Metadata` 12, `Page Metadata` 12, `Integer` 10 |
| recordref, fieldref | a few Base App tables (`Customer`, `Item`, `G/L Entry`, `Dimension Correction`, …), ~20 uses |
| session | `Session`, `Active Session` (system tables), `Environment Information`, `Application Area Mgmt. Facade` (System App) |
| text, collections, json, xml, streams | none |

Implications:
1. M2 needs a native `Temp Blob` (or the System App source) — small.
2. M3 needs the **platform virtual tables** (`Integer`, `Date`, `Field`,
   `AllObj*`, `*Metadata`, `Record Link`). These are platform-defined, not
   Base App, and belong in tier 1. Expect them to be a visible chunk of the
   M3 curve.
3. The handful of Base App table uses in `fieldref`/`recordref` are tier 3.
   Those cases will stay red (compile-time "unsupported" per hard rule 4) and
   are listed as such in the report rather than silently dropped.
