# ADR-034: Executable form, checked against the current interpreter

- Status: Accepted (2026-10-15, decided by the orchestrator on the owner's delegation; see docs/owner-review.md)
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

## Profile (step `profile-suites`, 2026-10-15)

Full numbers in `docs/reviews/2026-10-pageworks-profile.md`. Pageworks: dispatch 45 % (eval 23 %, exec_block 10 %),
value and Decimal handling 9 %, allocation 12 %, text 4.5 %, builtin lookup 3.7 %, record store 0.2 %. Tests-ERM:
dispatch 7 %, allocation about 65 % (libc included), record operations 77 % inclusive, of which 40 % is copying and
freeing whole table schemas (`validate_relation`, `records.rs`). The first profile of Pageworks found the install
codeunits re-run for every test codeunit (fixed). Reading: record handling comes first (point 1 below, ADR-035 and
the schema-clone fix); for computation-heavy AL, dispatch dominates, which favors the bytecode form (D4).

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

## Dual-run harness (step 2, implemented)

`al-test-cli --dual-run` and `alrunner test --dual-run` run every test codeunit twice on equal fresh sessions: once with
`Session::exec_mode` = `Tree`, once with `Bytecode` (`al_vm::ExecMode`; until the executable form runs, `Bytecode`
runs the tree walker, so the harness is tested by finding no difference between two runs of one interpreter). Both runs
read one standing clock. Compared per test: the outcome (pass, failure text, refusal reason), the call stack of an AL
error, the unprobed assumptions, and a hash of the rows of each table when the test ends (`MemStore::digest`); the first
difference of a test is named, with the first table whose rows differ. Every difference is printed and the run fails.
Traced (the nine tests of Tests-ERM codeunit 137617, and 131 in Tests-SCM): not an interpreter fault. Two tests of the
codeunit hit the CPU time limit (`--test-timeout-secs`, 20 s) and are stopped where the machine's speed puts them. The
tests after them share the session and start from the row-version and time-stamp counters the stopped test left, so
their system fields (`SystemRowVersion`, `SystemModifiedAt`) differ between two runs. The comparison therefore skips the
state of every test after the first one that either run stopped at the limit; outcome, error stack and assumptions are
still compared. Two runs of one interpreter now show 0 differences on Tests-ERM 137617 and on the corpus.

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
