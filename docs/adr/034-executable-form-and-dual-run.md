# ADR-034: Executable form, checked against the current interpreter

- Status: Proposed
- Date: 2026-10-15
- Related: ADR-011 decision 4 (tree walker until a budget needs more; superseded in part), ADR-031 (VM budget),
  ADR-032 (typed ids), ADR-033 (link step), ADR-009, review `docs/reviews/2026-10-compile-ir-review.md` (R2, R6)

## Context

The IR is both the cache format and the tree the interpreter walks. Interpreter optimizations (pre-resolved
builtins, constants as values, typed operations, slot layouts) cannot live in a cached, stable format. ADR-033 adds
a link step that builds the dense program at load time; that is the right place to build a form made for
execution.

The real AL suites are 5 to 6 times slower than BC (Pageworks: 88 s in BC's test codeunits, about 8 minutes here).
Nobody has profiled them yet, so the split between arithmetic, calls, builtins and record operations is not known.
A JIT is out of scope (review section 3.2: little gain on AL code, a second backend to keep equal, no WASM).

## Decision

1. **Profile first.** Before any executable-form code: profile Pageworks (the owner's repository, read only, a
   scratch copy) and Tests-ERM with test data, and record the split in this ADR. If record operations dominate,
   ADR-035 goes first.
2. **An executable form private to `al-vm`**, built from the dense `Program` at engine creation (`ExecProgram`; after
   ADR-033 that `Program` comes from the link step), never serialized:
   pre-resolved builtins (`NativeFn` per call site), constants as `Value`, routine metadata (ADR-031 item 2.5),
   locals templates, typed operations where the static types are known (Integer, Decimal, Boolean, Text
   concatenation, comparisons), positions in a side table instead of `Stmt::Pos` entries, inline caches for
   interface calls and event dispatch.
3. **The form is closure compilation** of the linked IR (each node becomes a pre-resolved closure). A register
   bytecode is the alternative; the choice is owner decision D4, after the profile. Both satisfy the rest of this
   ADR.
4. **The tree walker stays as the oracle.** A dual-run mode (`AL_DUAL_RUN=1`) runs every test with both and
   compares outcome, error text and code, call stack text, UI events, and the store after the test. A difference
   fails the run with the first diverging test.
5. **Per-routine fallback.** The linker compiles a routine to the new form only if it supports every node in it;
   otherwise the routine runs on the tree walker. Coverage grows node kind by node kind.
6. **Switch of the default** when the dual run shows 0 differences on the corpus (every major) and every bucket
   that runs today, and the ADR-031 budget shows the gain. The tree walker is kept for the dual run.

## What changes at which seam

- `al-vm`: a new module `exec/` (compiler from `Program` to `ExecProgram`, the closure runtime). `Session::call`
  dispatches per routine.
- `al-ir`: none (ADR-033 already split the forms). ADR-011 decision 4 is superseded: the trigger is the ADR-031
  budget plus the profile.
- `al-builtins`: none beyond ADR-032.

## Proof of zero behaviour change

- `AL_DUAL_RUN=1` on the corpus (every major) and on every bucket that runs today: 0 differences.
- Corpus and bucket outcomes identical with the new default (`tools/compare_junit.py`).
- Unit tests per node kind run on both forms (one test helper, two engines).

## Migration (each step merges green)

1. Profile and record the split (decision 1).
2. `ExecProgram` with the tree walker inside (no change); dual-run harness that compares two runs of the same
   engine (proves the harness).
3. Node kinds in order of the profile: constants and locals, Integer and Decimal arithmetic, calls, builtins,
   control flow, record field access, then the rest. Each step: dual run 0 differences.
4. Switch the default (decision 6).

## Consequences

- Unblocks: AL code at or near BC's speed on compute-heavy suites (expected 2 to 4 times on top of ADR-031,
  estimate).
- Blocked by: ADR-031 (budget) and the profile. Not blocked by ADR-033: the executable form is built from the dense
  `Program`, which the link step reproduces exactly, so it can start on today's `Program` and move behind `link()`
  later without change. ADR-032 makes it simpler (builtin ids).
- Cost: 15 to 25 agent-days. Two interpreters to keep equal; the dual run is the guard, and it must stay in CI
  (owner decision D5: every merge or nightly).

## Conformance

The dual-run job in CI; the ADR-031 budget.
