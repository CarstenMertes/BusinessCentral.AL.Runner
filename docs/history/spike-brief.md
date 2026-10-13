# Spike brief (historical)

> This is the original brief that started the project as a 2–4 week
> feasibility spike. The spike answered its question with **Go** on its first
> day of agent work (see `spike-report.md`); the project is now being built as
> a product. Current docs: `README.md`, `docs/architecture.md`,
> `docs/status.md`. Kept unchanged below for the record.

# Handoff: AL Runtime in Rust — Spike

> Corpus counts in this brief predate the current corpus; see
> `corpus-inventory.md` for current numbers. Oracle source and emulated
> version are fixed in ADR-007.

## Purpose

Find out, within a fixed timebox, whether agents can build a Rust runtime that executes AL with real fidelity, using the existing oracles. The spike exists to measure feasibility, not to produce a product.

**Timebox:** 2–4 weeks
**Scope:** Tier 1 — pure AL language semantics plus an in-memory record engine. No Base App.

## Question the spike must answer

> Does the corpus pass rate climb steadily while the architecture holds, or do agents plateau on undocumented semantics?

The decision at the end is one of:
- **Go:** continue to Base App type-checking.
- **Adjust:** fix the oracle or harness, then re-run.
- **Stop.**

## Oracles

| Oracle | Role | Location |
|---|---|---|
| AL language test corpus | Acceptance spec, verified against real BC Cloud on every commit. | github.com/StefanMaron/BusinessCentral.AL.Language.Tests |
| Real BC container | Ground truth for any behavior the corpus does not cover. Agents write a probe test, run it on BC, and add it to the corpus. | github.com/StefanMaron/MsDyn365Bc.On.Linux |
| AL Runner | Cheap second oracle and future trace oracle. Not required for tier 1. | github.com/StefanMaron/BusinessCentral.AL.Runner |

**Order of authority:** real BC, then AL Runner, then the Rust runtime.

## Hard rules

1. **Red/green TDD, no exceptions.** Every behavior starts as a failing test. Every bug fix starts with a test that reproduces it. Nothing merges with the suite red, except corpus tests that are explicitly tracked as not yet implemented.
2. **Never weaken an oracle.** Corpus tests are not edited to make the runtime pass. If real BC disagrees with the corpus, raise it as a corpus issue. Do not patch it locally.
3. **No guessing at semantics.** If behavior is unclear, write a probe test, run it on real BC, and add it to the corpus. Then implement.
4. **Unsupported means a compile-time error.** Builtins that are not implemented must fail at compile time with a clear diagnostic, never silently at runtime.
5. **Architecture changes go through an ADR** (`docs/adr/`). Agents fill in implementations; seams change only through a recorded decision.
6. **Observable equivalence (ADR-009).** As long as the observable outcome is the same, the runtime may improve on the original AL language and BC stack.

## Architecture

### Pipeline

```
AL source → lexer → preprocessor (#if, token level) → lossless CST (rowan)
          → semantic model (resolve, type-check, extension merge)
          → typed IR → bytecode (serializable, content-hashed, cached)
          → link (symbolic object refs → per-session tables, event registry)
          → VM (sync, tree-walk or simple register bytecode)
```

### Crates

| Crate | Responsibility |
|---|---|
| `al-syntax` | Lexer, preprocessor, error-tolerant lossless parser |
| `al-sema` | Workspace of apps and objects, name and overload resolution, implicit conversions, extension merging |
| `al-builtins` | Declarative catalog of every builtin, keyed by type + method + signature, with Rust implementations. The type checker and the VM both read from it. |
| `al-ir` | Typed IR, bytecode, serialization (serde) |
| `al-vm` | Interpreter, value model, control flow (`Error` / `Exit` as `Result` signals), events |
| `al-data` | Store trait, in-memory store, filter engine, keys, FlowFields |
| `al-host` | Capability traits (UI, clock, identity, network, blob storage) and a test host |
| `al-test-cli` | Runs `[Test]` codeunits and emits results comparable with the corpus CI |

### Seams

See `docs/adr/001`–`006`.

### Value model notes

- Text and Code are values. List, Dictionary, and the Json types are references.
- A Record is a handle holding a field buffer, filters, and the current key. `:=` copies fields only; `Copy` also copies filters.
- Decimal uses `rust_decimal`. Text keeps UTF-16 length semantics.

### In-memory data engine

- Tables are persistent copy-on-write maps (`im` or `rpds`), one index per key.
- A snapshot is a root pointer, so Commit, rollback-on-Error, and per-test reset are O(1).
- SetFilter strings compile to a predicate tree. Key-prefix ranges are extracted from it so lookups can seek.
- Temporary records get a private store per variable.

## Contract suites

Every trait seam gets a conformance suite of plain Rust tests that any implementation must pass. The in-memory store turns its suite green first. Future stores (SQLite, Postgres) must pass the same suite.

## Milestones

| # | Milestone | Done when |
|---|---|---|
| 0 | Harness | The corpus checks out, the test CLI runs, and every corpus test is reported, all red. The pass-rate metric is in CI. |
| 1 | Front end | The parser handles every corpus file losslessly (round-trip test). The fixtures and test codeunits type-check. |
| 2 | Pure semantics | `types/`, `text/`, `collections/`, `codeunit/`, `error-handling/` are green. |
| 3 | Record engine | `record/`, `recordref/`, `fieldref/` are green. |
| 4 | Stretch | `json/`, `xml/`, `streams/`, `session/`. |

Out of scope: `query/`, `xmlport/`, `handlers/` (TestPage, reports), `profile/`, and Base App.

## Metrics, tracked daily

- Corpus pass rate, overall and per area.
- Regressions: tests that went from green to red.
- Probe tests added to the corpus (oracle growth).
- Builtins catalog coverage.
- Performance budgets as tests: cold compile of the corpus ≤ 1 s; median test ≤ 1 ms; full corpus run ≤ 5 s.

## Risks to watch

- **Undocumented semantics** — Format/Evaluate, Decimal rounding, record assignment. Mitigation: hard rule 3.
- **Architecture drift.** Mitigation: ADR gate and contract suites.
- **Code quality degrading over time.** Mitigation: periodic human review of one crate per week.
- **Licensing** (later, for Base App).

## End-of-spike report

- Pass-rate curve per area over time.
- Where agents got stuck, and why.
- Probe tests that changed earlier assumptions.
- Recommendation: go, adjust, or stop. If go, include an estimate for reaching "Base App type-checks with zero errors".
