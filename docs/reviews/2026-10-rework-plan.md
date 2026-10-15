# Rework plan (2026-10-15)

The owner paused feature work to do the rework from `docs/reviews/2026-10-compile-ir-review.md`. The corpus writer
on the laptop continues. This plan orders the work into steps, shows which step needs which, and schedules up to
four parallel Sonnet agents so that no two touch the same files at the same time.

The ADRs: 031 (VM speed budget and fast paths), 032 (typed ids), 033 (symbolic IR and link step), 034 (executable
form checked against the current interpreter), 035 (store keys and rows), 036 (one resolved tree from check to
lower), 037 (code structure rules). ADR-030 (value capture) is on another branch; it hooks the `Stmt::Pos` arm in
`al-vm/src/lib.rs`, so it must merge before lane A starts on `lib.rs`, or rebase onto it.

## 1. Steps

Each step merges green on its own: `check.sh`, corpus outcomes identical to before (`tools/compare_junit.py`, every
BC major), and the proof its ADR names.

| Step | What | ADR | Needs an accepted ADR? | Main files |
|---|---|---|---|---|
| `vm-budget` | Benchmark app and `perf_vm.rs` budget test | 031 | **No** (a test) | new `al-test-cli/tests/perf_vm.rs`, `tests/vm-bench/`, CI workflow |
| `profile-suites` | Profile Pageworks and Tests-ERM; record the split | 034 step 1 | **No** (read only) | none (writes the result into ADR-034) |
| `vm-fast-paths` | Integer fast path, identity convert at run time, hashed builtin lookup, constants once, routine facts once, `for` counter, `Catalog` `OnceLock` | 031 items 2.1, 2.3-2.7 | **No** (internals, ADR-009) | `al-builtins/src/{ops,native,catalog}.rs`, `al-vm/src/lib.rs` |
| `testkit` | `al-testkit` dev crate; `al-vm/tests` use it, fewer binaries | 037 item 5 | **No** (dev only) | new `crates/al-testkit`, `al-vm/tests/**` |
| `helpers-dedupe` | One `unquote`, `split_top`, month/day table, type-name renderer | 037 item 4 (part) | **No** (internal) | `al-sema/src/{pages,queries,reports,xmlports,relation}.rs`, `al-vm/src/{views,events}.rs`, `al-builtins/src/{format,formatstr,describe}.rs` |
| `split-lower` | `lower.rs` -> `lower/*.rs`, pure move | 037 item 3 | **No** (pure move) | `al-sema/src/lower.rs` |
| `split-check` | `check.rs` -> `check/*.rs`, pure move | 037 item 3 | **No** (pure move) | `al-sema/src/check.rs` |
| `store-facts-rows` | Table facts precomputed, `AccessContext` without `String` clones, shared rows | 035 steps 1-2 | **No** (internals behind the `Store` trait) | `al-data/src/memory.rs`, `al-vm/src/{records,companies,testdata}.rs` |
| `convert-at-lowering` | Lowering drops identity `Convert` | 031 item 2.2 | No | `al-sema/src/lower/convert.rs` (after `split-lower`) |
| `signal-box` | `Signal::Error(Box<AlError>)` (168 sites in 61 files) | 031 decision 3 | **Yes**, 031 | wide: `al-builtins`, `al-vm`, `al-testrun` |
| `objkind-ir` | `ObjKind` in `al-ir` (about 240 kind string literals) | 032 step 1 | **Yes**, 032 | wide: every crate |
| `builtin-id` | `BuiltinId` and one registry | 032 step 2 | Yes, 032 | `al-builtins/src/native.rs` + registries, `al-ir`, `al-sema/src/lower/call.rs`, `al-vm/src/lib.rs` |
| `split-vm` | `Session` sub-states; `interp.rs`, `ctx.rs` | 037 item 3 | No (move), but wide in `al-vm` | `al-vm/src/*.rs` |
| `split-pages-runtime` | `al-vm/pages.rs` and `al-testrun/runtime.rs` splits; one JUnit writer; CLIs through `al-testrun` | 037 items 3, 6 | No | `al-vm/src/pages*`, `al-testrun/src/runtime.rs`, `al-test-cli/src/output.rs`, `alrunner-cli/src/output.rs` |
| `record-op-enum` | `RecordOp`, `PageOp` | 032 step 3 | Yes, 032 | `al-vm/src/{records,recmeta,refs,pages/*}.rs`, `al-builtins/src/{native,refs}.rs` |
| `typed-facts` | `LibraryKind`, typed schema facts, `CompanyId` | 032 steps 4-6 | Yes, 032 | `al-builtins/src/value.rs`, `al-ir`, `al-sema/src/lower/*`, `al-data` |
| `where-parser` | One where-condition parser; view text at lowering | 037 item 4 | No (internal) | `al-sema/src/{calcformula,relation,pages,queries}.rs`, `al-vm/src/{views,pages/request}.rs` |
| `link-1-3` | `al_ir::sym`, `al-link`, contributions moved into link, positions only from link | 033 steps 1-3 | **Yes**, 033 | `al-ir`, new `crates/al-link`, `al-sema/src/lower/*` |
| `link-4-6` | Symbolic cache blocks, incremental with declaration changes, one compile path | 033 steps 4-6 | Yes, 033 | `al-cache`, `al-sema/src/{provided,incremental}.rs`, `al-testrun/src/compile_cache*` |
| `exec-form` | `ExecProgram`, dual run, node kinds, default switch | 034 | **Yes**, 034 (and D4) | new `al-vm/src/exec/`, `al-vm/src/interp.rs` |
| `store-keys` | Encoded keys | 035 step 3 | Yes, 035 | `al-data/src/memory.rs`, `al-builtins/src/filter.rs` (`key_cmp`) |
| `bound-tree` | Bound tree, construct by construct | 036 | **Yes**, 036 (and D6) | `al-sema/src/{check,lower}/*`, new `al-sema/src/bound.rs` |

## 2. Dependency graph

An arrow means "must merge before".

```
vm-budget ─────> vm-fast-paths ─> signal-box ─> split-vm ─┐
profile-suites ──────────────────────────────────────────┼─> exec-form
split-lower ─> split-check ─> objkind-ir ─> builtin-id ───┘
     │              │             │             │
     │              │             │             └──────────────┐
     │              │             ├─> link-1-3 ─> link-4-6      │
     │              │             │       └────────────────────┴─> bound-tree
     │              │             └─> typed-facts ─> store-keys
     │              └─> helpers-dedupe ─> where-parser
     └─> convert-at-lowering
testkit ─> split-pages-runtime ─> record-op-enum (also after objkind-ir)
                  └─> where-parser (vm side)
store-facts-rows ─> typed-facts (CompanyId)
```

Notes:

- `exec-form` does **not** need the link step: it is built from the dense `Program`, which `link()` reproduces
  exactly (ADR-034, Consequences). It needs the budget, the profile, the fast paths and `split-vm`.
- `objkind-ir`, `signal-box` and `split-vm` touch many files. They run in **sync windows** (section 3): the other
  lanes merge first and pause on files those steps touch, each window one to two days.
- `bound-tree` is last and largest. It must not overlap any other `al-sema` work.

## 3. Schedule for four lanes

A lane is one Sonnet agent in one worktree. A lane owns the files listed for its step; nobody else edits them until
the step merges. Durations are agent-days (estimates from the review).

### Phase 0: start today, no ADR needed (about 5 days)

| Lane | Steps in order | Owns |
|---|---|---|
| A (VM) | `vm-budget` (1) -> `vm-fast-paths` (3) | `al-builtins/src/{ops,native,catalog}.rs`, `al-vm/src/lib.rs`, new `perf_vm.rs` and `vm-bench/`, CI workflow |
| B (tests and tools) | `profile-suites` (0.5, read only, one heavy run at a time) -> `testkit` (2-3) | new `crates/al-testkit`, `al-vm/tests/**` |
| C (sema) | `split-lower` (1) -> `split-check` (1) -> `convert-at-lowering` (0.5) -> `helpers-dedupe`, sema side (1) | `al-sema/src/**` |
| D (data) | `store-facts-rows` (2-3) | `al-data/**`, `al-vm/src/{records,companies,testdata}.rs` |

Conflict check: A owns `al-vm/src/lib.rs`; D owns `records.rs`, `companies.rs`, `testdata.rs`. Table facts that
`lib.rs::default_value` reads move into D's step (ADR-031 item 2.5 note), so A does not touch them. B touches only
tests. C stays in `al-sema`. The VM side of `helpers-dedupe` (`al-vm/src/{views,events}.rs`) waits for phase 1.

### Sync window 1 (after phase 0 merges and ADR-031 and ADR-032 are accepted; 2 days)

One lane at a time, the others idle or on read-only work (ADR review, the next step's design notes):

1. `objkind-ir` (lane C, 1 day): every crate.
2. `signal-box` (lane A, 1 day): `al-builtins`, `al-vm`, `al-testrun`.

### Phase 1 (about 6 days)

| Lane | Steps in order | Owns |
|---|---|---|
| A (VM) | `builtin-id` (2-3) | `al-builtins/src/native.rs` and registries, `al-ir` (`Expr::Builtin` only), `al-sema/src/lower/call.rs`, `al-vm/src/lib.rs` |
| B (tests and tools) | `split-pages-runtime` (2-3) -> `record-op-enum` (2) | `al-vm/src/{pages*,recmeta,refs}.rs`, `al-testrun/src/runtime.rs`, CLI output files |
| C (sema) | `helpers-dedupe` vm side + `where-parser` (2-3) | `al-sema/src/{calcformula,relation,pages,queries,reports,xmlports}.rs`, `al-vm/src/{views,events,pages/request}.rs` (request.rs handed over by B after its split) |
| D (data) | `typed-facts` (3-4) | `al-builtins/src/value.rs`, `al-ir` (schema fields), `al-sema/src/lower/{shared,assemble}.rs`, `al-data` |

Conflict check: `al-ir` is shared by A (`Expr::Builtin`) and D (schema fields): different items in one file;
merge A first, D rebases (small). `records.rs` moves from D to B for `record-op-enum`; D's `typed-facts` does not
touch it. `pages/request.rs`: B splits, then hands it to C.

### Sync window 2 (after ADR-033 and ADR-034 are accepted; 1-2 days)

`split-vm` (lane A): `Session` sub-states touch every `al-vm` module.

### Phase 2 (about 10 days)

| Lane | Steps in order | Owns |
|---|---|---|
| A (VM) | `exec-form` (15-25, continues into phase 3) | new `al-vm/src/exec/`, `al-vm/src/interp.rs` |
| B (tests and tools) | `link-4-6` after C's `link-1-3` step 3 merges (5-8); until then: the dual-run harness for A (`al-testrun`, 2) | `al-cache`, `al-sema/src/{provided,incremental}.rs`, `al-testrun/src/compile_cache*` |
| C (sema) | `link-1-3` (8-10) | `al-ir` (new `sym` module), new `crates/al-link`, `al-sema/src/lower/*` |
| D (data) | `store-keys` (3) -> then free for `bound-tree` preparation (design notes, read only) | `al-data/src/memory.rs`, `al-builtins/src/filter.rs` |

### Phase 3 (about 20 to 30 days)

| Lane | Steps | Owns |
|---|---|---|
| A | `exec-form` continues (node kinds, dual run, default switch) | `al-vm/src/exec/**` |
| C (+ D) | `bound-tree` (20-30). Two agents only if they split by construct with a clear owner per file (`check/infer.rs` vs `lower/expr.rs` and `lower/stmt.rs`) | `al-sema/src/**` |
| B | Feature work may resume outside `al-sema` and `al-vm/src/exec/`, as the owner decides | |

### Rules for every lane

- Merge `main` into the branch before handing back (as today). Regenerate baselines after merging; do not hand-merge
  `metrics/<major>/passing.txt`. Each step is expected to keep the baselines unchanged.
- Heavy runs (buckets, `demand_vs_eager`, profiles) go through `systemd-run --user --scope -q --slice=al-work.slice -p
  MemoryMax=8G`, one at a time on the machine.
- A step that finds a behaviour difference stops and reports it. It does not fix behaviour inside a rework step.

## 4. What can start today without an ADR

- `vm-budget` (a new ignored test and a CI step).
- `profile-suites` (read only).
- `vm-fast-paths` except `signal-box` (internals; ADR-009).
- `testkit` (dev-only crate).
- `split-lower`, `split-check` (pure moves).
- `helpers-dedupe` on the `al-sema` side (`unquote`, `split_top`, month/day tables, type names).
- `store-facts-rows` (internals behind the `Store` trait).

The where-condition parser merge needs no ADR either, but it waits for the splits (phase 1).

## 5. Decisions for the owner

- **D1 Object key in the symbolic IR (ADR-033).** (a) `(kind, id)` where an id exists, else name: survives renames.
  (b) Always the qualified name: survives renumbering. Recommended: (a).
- **D2 Routine key (ADR-033).** (a) Name plus overload ordinal: simple, but a new overload above shifts later keys.
  (b) Name plus parameter-type signature: stable under edits, more work. Recommended: (b).
- **D3 Cache entries after the link step (ADR-022, ADR-033).** (a) Keep layers as built. (b) Per-app entries. Recommended:
  keep layers now, measure after `link-4-6`, then decide.
- **D4 Executable form (ADR-034).** (a) Closure compilation: smaller step, keeps the code shape. (b) Register bytecode:
  more speed headroom, more work. Decide after `profile-suites`. Recommended now: (a).
- **D5 Dual run in CI (ADR-034).** (a) On every merge: safest, adds one corpus run per CI job. (b) Nightly with the
  buckets. Recommended: (a) on the corpus, (b) on the buckets.
- **D6 Bound tree scope (ADR-036).** (a) Build fully, 20 to 30 agent-days. (b) Stop after steps 1-2 (object ids in
  types, slots in resolutions), about 4 days. Recommended: (b) now, (a) after the link step.
- **D7 File-size rule (ADR-037).** (a) `check.sh` warns above 2 000 lines. (b) Warns at 2 000, fails at 3 000, with a
  shrinking exception list. Recommended: (b).
- **D8 Scope of the pause.** (a) No feature work at all. (b) Fixes allowed outside `al-sema`, `al-vm/src/lib.rs`
  and the files a lane owns. Recommended: (b), checked against the lane table.
