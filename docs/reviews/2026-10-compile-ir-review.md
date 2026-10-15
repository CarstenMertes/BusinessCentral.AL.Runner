# Design and performance review: compile pipeline, IR, interpreter, code structure (2026-10-15)

- Reviewer: an independent agent, read-only on `main` (`e30f3524`). Measurements were taken in a scratch worktree.
- Scope: the owner's request ("the best solution even if it means extra work") plus the coordinator's addition
  (code structure and seams, area 5).
- The machine had a load average of 12 to 41 during every measurement. Wall times are given only as a rough
  guide. Comparisons use user-space instruction counts (`perf stat -e instructions:u`), which load does not change.

## Executive summary

1. The IR is used as both the cache format and the executable form. That is the root design problem. Every
   interpreter optimization changes the cache format, and every cache need (stable ids) fights the interpreter's
   need for dense indices.
2. ADR-011 decision 3, the **link step**, was never built. Lowering writes positional object, routine and field
   indices directly. This one gap is behind ADR-022 finding 1 and 2, the "workspace last" ordering, `relower_set`,
   and the limits of ADR-029 (no declaration changes).
3. The interpreter spends most of its time on overhead, not on AL semantics. One iteration of `s := (s + i) mod N`
   costs about **2 700 instructions**. Every Integer operation runs through `rust_decimal` with 128-bit division.
4. Three small fixes cut instructions by **30 %** over six microbenchmarks (up to 42 % on calls and builtins). The
   fixes are about 30 lines: an Integer fast path, identity conversion, and a hashed builtin lookup. Builtin
   lookup was a linear scan over about 750 string ids **on every builtin call**.
5. Check and lower resolve names twice. Check records `Ty` (objects by name) and `Res` (locals by name, procedures
   by byte offset) in hash maps keyed by node position. Lower walks the syntax tree again, looks those maps up and
   resolves object names again. About 45 % of compile CPU is tree walking, hashing and allocation. Semantic logic
   is about 12 %.
6. Too much is typed as strings across seams: object kinds (104 string compares), builtin ids (93 literal matches
   plus 27 `starts_with` in the VM), record methods (285 string match arms) and raw properties read at run time
   (128 `eq_ignore_ascii_case` in `al-vm`). Each new feature adds more of them.
7. The code structure costs every future agent time: `lower.rs` (7 479 lines), `al-vm/pages.rs` (4 703),
   `al-testrun/runtime.rs` (4 573), `check.rs` (3 607), a `Session` with 83 fields, 3 hand-written where-condition
   parsers, 7 copies of `unquote`, and 39 copies of the test compile helper.
8. A JIT is not worth it. A linked, typed executable form (closure-compiled tree or register bytecode) inside
   `al-vm`, plus a faster record store, should reach real BC's speed on most AL code.
9. Ranked by future cost: (1) symbolic IR plus link step plus executable form, (2) typed ids instead of strings,
   (3) one name resolution (a bound tree), (4) code structure splits. The interpreter quick wins come first in
   time because they are cheap. They are not first by future cost.
10. Every step can keep the zero-difference rules. The link step must produce today's `Program` exactly (assert
    equality). A new executable form runs next to the tree walker in a dual-run mode until it shows zero
    differences.

---

## 1. The compile pipeline

### 1.1 What it is today

`parse` (rowan CST, per file, parallel) -> `Workspace::load_apps` + `resolve_references` (per object, parallel since
ADR-026) -> `members` (lazy per object, merged with extensions) -> `check` (types and resolutions into
`PosMap<Ty>` and `PosMap<Res>`, keyed by `(file, start, end)`) -> `lower` (walks the CST again, reads the maps,
emits `al_ir::Program`) -> cache encode (postcard + zstd per object) or the engine.

There are four compile paths that must give the same program: eager (cache build), demand-driven (ADR-013),
cached (ADR-022, with re-lowering of touched blocks), and incremental (ADR-029). Each has a differential tool
(`tools/demand_vs_eager.sh`, `tools/cache_vs_uncached.sh`, `ALRUNNER_CHECK_INCREMENTAL`).

### 1.2 Findings

**F1.1 Names are resolved twice, and the handoff is by position hash map.** (high future cost)

- `check.rs:149-171` `CheckResult` holds `types: PosMap<Ty>` and `resolutions: PosMap<Res>`, keyed by
  `(file, start, end)` (`check.rs:39-60`).
- `Res::Local(String)`, `Res::Global(String)`, `Res::BaseGlobal(String)` carry **names**, not slots
  (`check.rs:202-240`). `Res::Proc { object, offset }` carries a **byte offset** into the source file.
- `Ty::Record { table: String }`, `Ty::Enum(String)`, `Ty::Object(kind, String)` carry **object names**
  (`types.rs:7-48`). Every `Ty` clone allocates.
- Lowering looks them up again: `ty_of` clones a `Ty` out of the map (`lower.rs:3316-3322`, 41 call sites), and
  `resolve_obj` resolves the name again through `Workspace::resolve_ref` with a freshly split `Vec<String>`
  (`lower.rs:3329-3334`, 21 call sites). `Lowerer` keeps `globals` and `locals` as `HashMap<String, usize>` by
  lower-case name (`lower.rs:2159-2172`).
- Profile of the corpus compile (`--example phases`, perf, 2 822 objects): rowan tree navigation 22.6 %, lexer
  and parser 19.7 % (the example parses several times), allocation, copy and drop 13.4 %, hash maps 8.5 %
  (`PosMap<Ty>::get` 2.7 %, `PosMap<Res>::get` 2.5 %), lowering logic 6.0 %, checker logic 4.2 %.
- Consequence: two walks of the CST, each creating rowan cursor nodes. Two places to implement each language
  feature. A class of bugs where check and lower disagree. And no unit smaller than "an object from source" for
  incremental work, because the check output is keyed by byte positions in a file.

**F1.2 Positional output.** (highest future cost; see F2.1)

**F1.3 Four compile paths.** Each new language feature has to be right in eager, demand, cached and incremental
compile. The differential tools catch differences, but they cost minutes per run and agent time per difference.
The paths exist mainly because the IR is positional and whole-program (F2.1): a block cannot be reused unless
the whole object order and field order around it is the same.

**F1.4 Work repeated per compile that should be once per process.**

- `Catalog::load()` (`al-builtins/src/catalog.rs:65`) parses all 1 725 signature lines on every call, with no
  `OnceLock`. It has about 34 call sites in `src` (`lower.rs` 9, `check.rs` 4). Every compile in every test pays
  it.
- The executable is hashed with sha256 for the cache key on every run. That is about 6 to 30 ms. It showed as
  1.5 to 13 % of the samples of a short benchmark run. Hash it once per binary (store the hash next to it, or
  use a build id).

**F1.5 Parallelism.** Parse, `resolve_references`, check (demand rounds) and lower are parallel. The serial parts
left are the ones ADR-026 lists (union of `PosMap` shards, program assembly, positions). The bigger blocker
for parallelism and incrementality is not threads. It is that lowering needs whole-program facts in positional
form (`Shared` in `lower.rs:1281`).

**F1.6 Decode is allocation-bound because the IR is full of strings.** ADR-022 measured 28.9 MB of postcard
becoming 280 MB of heap. Strings are two thirds of the raw bytes. In the IR: `Expr::Builtin { id: String }`,
`Slot::name`, `RtType::Library(String)`, `RtType::Option(Vec<String>)`, `Const::Decimal(String)`,
`Const::Option(i32, Vec<String>)`, `Object::kind: String`, `Object::properties: Vec<(String, String)>`,
`Field::relation_table: Option<String>`. Typed ids (F5.2) shrink both the file and decode time.

### 1.3 Is the phase split right?

The phases are right. The interfaces between them are wrong:

- check -> lower should be a **bound tree** (a typed HIR per routine): every name already a slot, field number,
  object id or builtin id, every node typed. Lowering then reads that tree, not the CST and not hash maps. This
  is the standard design (Roslyn's bound tree, rustc's THIR).
- lower -> VM should be a **symbolic IR** (stable references) plus a **link** step that builds the dense,
  executable form. ADR-011 already says so (decision 3). It was not built.

---

## 2. The IR design

### 2.1 Findings

**F2.1 No link step: object, routine and field references are positions.** (highest future cost)

- `Expr::Call { object: usize, routine: usize }`, `RtType::Record(u32, _)` and about 35 other places index
  `Program::objects` (ADR-022 finding 1). `Place::Field(_, usize)` and `Const::FieldPos` index the merged field
  list, whose order depends on which extensions are in the workspace (ADR-022 finding 2).
- `grep -rn "fn link"` finds no link step in `al-vm`, `al-ir` or `al-sema`. ADR-011 decision 3 ("Link turns
  object references into indices") is not implemented.
- What this already cost: design A of ADR-022 instead of per-app entries; ordering workspace apps last;
  `relower_set`; `Routine::shift_positions`; ADR-029 refusing any declaration change; ADR-026 judging per-object
  incremental compile "too costly".
- AL has stable identities that make this easy: an object is `(kind, id)` (unique in a tenant), a field is
  `(table id, field number)` (unique including extensions), an enum value is its ordinal. Procedures need a key:
  `(object, name, overload ordinal)` or a signature hash.

**F2.2 One IR for two jobs.** The IR is the serialized cache format **and** the tree the interpreter walks.
That blocks both sides:

- Pre-resolving a builtin to a function pointer, folding a constant into a `Value`, or caching a table schema on
  a node cannot be stored in a cache.
- Stable ids that the cache needs are slow for the interpreter if it has to look them up at run time.

The fix is two forms: a symbolic IR (stable, serializable, the seam ADR-011 meant) and an executable form built
by the link step inside `al-vm` (dense, never serialized, free to change with ADR-009's freedom).

**F2.3 Node shapes are wide.** Measured with `size_of` (release): `Stmt` 224 bytes, `Expr` 80, `Value` 32,
`RtType` 32, `Signal` 72, `Result<Value, Signal>` 72. Every `Stmt::Pos` marker costs 224 bytes. Every `eval`
returns 72 bytes, and the annotated profile of `Session::eval` shows its top samples on 16-byte moves of
these results through a stack frame of more than 480 bytes (`0x1e0(%rsp)`).

**F2.4 Builtins are strings at run time.** `Expr::Builtin { id: String }` is matched in `Session::eval` against
about 10 literal ids and 6 `starts_with` prefixes (`al-vm/src/lib.rs:1363-1487`), then looked up with
`native::lookup(id)`. That function was a **linear scan** over about 750 `(&str, fn)` pairs, then a `HashMap`
(`al-builtins/src/native.rs:555-561`). After the call, every `Arg::Ref` checks `id.starts_with("InStream.Read")`.

**F2.5 Constants are rebuilt on every evaluation.** `constant()` (`al-vm/src/lib.rs:1996-2011`) parses
`Const::Decimal(String)` with `str::parse` each time, allocates a new `String` and `Arc` for each text constant,
and clones `Vec<String>` into a new `Arc<[String]>` for each Option constant.

**F2.6 Conversions are not resolved statically.** `convert_to` emits `Expr::Convert` for every assignment, also
Integer into Integer (`lower.rs:3493-3500`). At run time `ops::convert` turns any number into a `Decimal` first
(`al-builtins/src/ops.rs:54-80`), including Integer into Integer.

**F2.7 Per-call work that belongs to the routine, not the call** (`al-vm/src/lib.rs:742-890`):
`engine.events.publishers.contains_key(&(object, routine))` (a `HashMap` lookup on every call),
`r.attributes.iter().any(|a| a.name.eq_ignore_ascii_case("TryFunction"))`, a filter for `CommitBehavior`,
`errorcollect::declares_collect(&r.attributes)`, `default_value` for each local (for a record local it scans the
table's `properties` strings twice for `TableType`, `lib.rs:546-563`), and a new `Vec<Value>` for the locals.

### 2.2 Would a lower-level form pay off?

Yes, and the measurements show where:

| Benchmark (one test codeunit each, no dependencies) | Iterations | Wall today | Instructions per iteration today | After 3 small fixes | Change |
|---|---:|---:|---:|---:|---:|
| `IntLoop`: `s := (s + i) mod 1000003` | 3 000 000 | 0.65 s | 2 739 | 1 657 | -40 % |
| `DecimalLoop`: `d := d + i * 1.5 - 0.25` | 1 000 000 | 0.24 s | 2 838 | 2 728 | -4 % |
| `CallLoop`: `s := Add(s, i)` (local procedure) | 1 000 000 | 0.47 s | 6 168 | 3 552 | -42 % |
| `TextLoop`: `Format`, `+`, `CopyStr`, `StrLen`, `StrPos` | 300 000 | 0.22 s | 9 830 | 9 090 | -8 % |
| `BuiltinLoop`: `Round(Abs(i / 3), 0.01) + Power(2, 3)` | 500 000 | 0.49 s | 13 372 | 7 784 | -42 % |
| `RecordLoop`: temp table, Insert, FindSet/Next, Get/Modify | 105 000 record ops | 0.32 s | ~29 400 per op | ~27 700 | -6 % |
| All six | | | 30.2 G total | 21.0 G total | **-30 %** |

Method: `perf stat -e instructions:u` of `alrunner test <app> --no-test-data`, one codeunit per app; an empty test
costs 43 M instructions, subtracted. The three fixes (scratch only, not committed, about 30 lines):

1. `ops::arith`: when both sides are `Integer`, use `checked_add`, `checked_sub`, `checked_mul`, `checked_div`,
   `checked_rem`, with the same division-by-zero and overflow errors.
2. `ops::convert`: `Integer -> Integer`, `Decimal -> Decimal`, `BigInteger -> BigInteger` return the value.
3. `native::lookup`: a `HashMap<&str, NativeFn>` built once.

For comparison, .NET runs `s := (s + i) mod N` in a few instructions per iteration. Even after the fixes, the
interpreter is two to three orders of magnitude slower on pure arithmetic. On the owner's Pageworks suite the
gap is about 5.5x (88 s in BC, about 8 minutes here), because real code spends most of its time in builtins and
record operations, where BC is slow too. **Nobody has profiled Pageworks yet.** That profile must come before
the choice in R6.

### 2.3 What it would take to move there incrementally

See R1, R2 and R6. The key property: the link step first produces **exactly today's `Program`** (equality is
testable on the corpus and on Microsoft's apps). Only after that does the executable form diverge from it, and
then behind a dual-run mode.

---

## 3. The interpreter's hot paths

### 3.1 Findings (from `perf record` of the six benchmarks)

- **Dispatch.** `Session::eval` is one function with every arm inline. Its self time is 20 to 35 % on every
  benchmark. Most of it is moving 72-byte `Result`s (F2.3). `exec_block` adds 5 to 13 %.
- **Numbers.** Integer arithmetic and conversion go through `rust_decimal` (`Decimal::trunc`, `fract`, `rem`,
  `__divti3` are 12 % of `IntLoop`). The `for` loop converts its counter through `Value::BigInteger` and
  `ops::convert` on every iteration and clones the slot's `RtType` with `place_type` (`lib.rs:1022-1024`).
- **Builtins.** `native::lookup` was 33 % of `BuiltinLoop` and 7 % of every other benchmark (the test framework
  uses builtins too).
- **Calls.** `call` + `finish_call` + conversions: about 2 600 instructions per call after the fixes (CallLoop
  minus IntLoop). See F2.7.
- **Text.** `TextLoop` spends its time in `malloc` and `free` (10 %), SipHash on `&str` (7 %, the `derived()`
  `HashMap` with the default hasher), and `StrSearcher::new`. Text is now copy-on-write (good). Constants still
  allocate (F2.5).
- **Records.** `key_cmp` + `ops::compare` 18 %, `Vec<Value>` clones 9 %, `im` B-tree lookup 3 %, malloc/free 6 %.
  Per record operation: `record_op` matches the method name as a string against long lists
  (`records.rs:894-939`); scans all fields for `init_text` (`records.rs:940-941`); `cx_for` clones company and
  user `String`s into an `AccessContext` (`companies.rs:75-95`); tables are keyed by `(String, u32)` in
  `MemStore` (`al-data/src/memory.rs:26`); a record with an ad-hoc key deep-clones the whole `TableDef` on each
  operation (`records.rs:55-59`).
- **Value.** 32 bytes is acceptable. `Library(String, Rc<..>)` and `DateFormula(String)` set the size. An interned
  library type id would make it 24.
- **Events.** A `HashMap` lookup per call to find publishers (`lib.rs:763`). Subscribers are keyed by
  `(usize, String)` with a lower-case event name (`events.rs:38`).

### 3.2 What "as fast as real BC" needs

Real BC compiles AL to C#, then to IL, then the .NET JIT makes machine code. Integer arithmetic is native.
`Decimal` is `System.Decimal` (slow, 128-bit software arithmetic, as `rust_decimal` here). Text is .NET
`string` (immutable, UTF-16). Record operations go through the NST data layer and SQL, which is slow.

So for real AL code:

| Kind of work | BC | Here today | What closes the gap |
|---|---|---|---|
| Integer, Boolean, control flow | native | 1 000+ instructions per operation | typed instructions; no `Decimal` on Integer paths |
| Decimal arithmetic | software decimal | software decimal plus overhead | typed instructions; constants pre-built |
| Calls | .NET call plus NST frame setup | about 2 600 instructions | routine flags precomputed; locals from a template; no `HashMap` per call |
| Builtins (text, format, dates) | .NET library | library plus string dispatch | pre-resolved builtin id or fn pointer |
| Record operations | NST plus SQL round trip | in memory, about 28 000 instructions | typed keys, shared rows, interned company, precomputed flags |

An honest view of the options:

- **Quick wins** (R3): 30 % measured, 2 to 3x expected once all of F2.4 to F2.7 are done. A few agent-days.
- **Executable form inside `al-vm`** (R6): closure compilation of the linked IR (each node becomes a
  pre-resolved boxed closure or a compact typed node) or a register bytecode. Typical gains over a naive tree
  walker are 3 to 10x. Inline caches help on interface calls, record field access and event dispatch.
- **A JIT with Cranelift**: it would beat bytecode only on pure arithmetic loops. AL code rarely is that. It needs
  unboxed typed values, deoptimization for errors (every AL error has an exact text and call stack), and a second
  backend to keep equal to the interpreter. That is months of work and a permanent maintenance cost. It does not
  fit WASM hosts (ADR-006, ADR-016). **Not recommended.** Revisit only if a profile of real suites shows more
  than half the time in arithmetic after R6.

---

## 4. Risks that will cost more later (ranked by future cost)

Ranked by how fast the cost of fixing them grows. The interpreter quick wins are not on this list: they cost the
same next year. They come first in the sequence (section 7) because they are cheap.

1. **Positional IR with no link step (F2.1, F2.2).** The cost grows with every IR variant and every consumer.
   Today about 35 places in `al-ir` and about 80 enum variants a remap would touch. Each new feature adds more.
   It blocks per-app cache entries, incremental compile of declaration changes, reuse of blocks across
   workspaces, and every interpreter optimization that should not change the cache format.
2. **Strings where ids belong (F5.2).** 93 builtin id literals and 27 prefix checks in `al-vm`, 285 record and
   page method string arms, 104 object-kind string compares, 128 runtime property scans. Each new builtin or
   object kind adds more. Turning them into enums later means touching all of them at once.
3. **Two name resolutions (F1.1).** Every new language construct is built twice, in `check.rs` and `lower.rs`.
   The longer this lasts, the more the two drift.
4. **God files and god types (F5.1).** Merge conflicts between parallel agents grow with file size. `Session`
   with 83 fields is touched by almost every feature.
5. **Four compile paths (F1.3).** Each new feature must be right four times. This shrinks once (1) is done.
6. **Duplicated parsers and helpers (F5.3).** Each copy drifts; a fix in one is missed in the others.
7. **No interpreter performance budget.** CI has budgets for compile and parse only (`docs/architecture.md`,
   Budgets). A change that makes the VM 20 % slower passes CI. This is cheap to fix and should be fixed first.

---

## 5. Code structure and seams

### 5.1 Size and responsibilities

| File | Lines | Problem |
|---|---:|---|
| `al-sema/src/lower.rs` | 7 479 | One `impl Lowerer` from line 2369 to about 5490; 1 680 lines of inline tests at the end. Statements, expressions, calls, pages, reports, xmlports, DotNet and cache assembly in one file. |
| `al-vm/src/pages.rs` | 4 703 | The whole test client; 31 runtime property reads (`PageType` at 1317, 1458, 1985, 2623). |
| `al-testrun/src/runtime.rs` | 4 573 | Run orchestration, cache, baseline, isolation and about 1 720 lines of inline tests. |
| `al-sema/src/check.rs` | 3 607 | Checker plus the `PosMap` machinery plus the type fitting of catalog signatures. |
| `al-builtins/src/xml.rs` | 3 542 | XML, XPath, and XmlPort helpers. |
| `al-vm/src/lib.rs` | 2 267 | Engine, Session (83 fields), the interpreter, and the `Ctx` implementation for builtins. |

`Session` holds the store, transactions, UI handlers, page clients, events, coverage, DotNet state, error
collection, reports and test-runner state. Almost every feature adds a field to it. Splitting it into owned
sub-states (`TxState`, `UiState`, `PageClient`, `EventState`, `Coverage`, `DotNetState`, `ReportState`) lets
features be built without touching each other.

### 5.2 Strings that should be types

- `ir::Object::kind: String` (`al-ir/src/lib.rs:160`). `al-sema` has `ObjKind` with 16 variants, but the IR drops
  it. So `al-vm`, `al-testrun` and the CLIs compare strings: 104 sites (`"codeunit"` 13, `"table"` 7). Some kinds
  are invented with `format!` (`"uncompiled report"`, `lower.rs:1515`) and compared in `al-vm/reports.rs:374,455`.
- Builtin ids: a `BuiltinId` enum (`u16`) generated from the catalog would replace `Expr::Builtin { id: String }`,
  the three registries (11 static slices chained in `native.rs:511-524`, 12 `native_for` derivers, and
  `platform_native::lookup`), and the 93 literal matches in `al-vm`.
- Record and page methods: `record_op(method: &str, ..)` (`records.rs:894`), 57 callers, 51 with literals; 285
  match arms on method-name literals across `al-vm`. A `RecordOp` enum.
- `RtType::Library(String)` and `Value::Library(String, ..)`: 26 name-guarded matches. A `LibraryKind` enum.
- `Object::properties: Vec<(String, String)>` read at run time: `TableType` on every record default value
  (`lib.rs:546-563`), `EventSubscriberInstance`, `PageType`, `FieldClass`. Lowering should turn the properties the
  VM reads into typed fields of the schema.

### 5.3 Duplicated implementations

- Where-condition grammar (`field = const(..)`, `filter(..)`, `field(..)`): three hand-written parsers in `al-sema`
  (`calcformula.rs:248-265`, `relation.rs:46` and `193-229`, `pages.rs:875-918`).
- View text (`SORTING`, `ORDER`, `WHERE`) parsed **at run time** in two places (`al-vm/views.rs:270`,
  `al-vm/pages/request.rs` about 530-570). `property_view` (`views.rs:399`) rewrites raw property strings at run
  time instead of at lowering.
- `split_top`: 5 copies. `unquote`: 7 private copies, while `al-syntax/ast.rs:88 unquote_ident` exists. Month and
  day name tables: 3 copies. Value or type to type-name text: 6 functions.
- JUnit writer: `al-test-cli/src/output.rs:61-88` and `alrunner-cli/src/output.rs:241`. A `coverage.rs` in three
  crates with different meanings.
- `al-test-cli` calls `al_sema::check::check` directly (`coverage.rs:67`, `mnc.rs:178`), past `al-testrun`.

### 5.4 How many places a new thing touches

- **A new AL value type:** `Value` (28 variants) has 3 967 references in 94 files; `RtType` 586 in 39; `Ty` 608
  in 6. `Duration` appears in 23 files across 4 crates. Expected for a value model, but `Ty` and `RtType` are two
  parallel type systems: one by name, one by index (F1.1).
- **A new builtin:** catalog line, one of three registries, often a string-keyed `Ctx` method, and often a VM
  special case in `lib.rs:1363-1487` or a lowering special case.
- **A new object kind:** `ObjKind` in `al-sema` (362 references in 12 files), then strings everywhere after the IR.
- **A new BC version gate:** `al-version/gates.rs` (enum, `ALL`, `since_bc`, `evidence`). 7 of 16 gates are never
  referenced outside that file. `transactions.rs:145` and `lib.rs:2132` read `t.bc.major` directly (default 29),
  although `gates.rs:4` says code never compares a version.

### 5.5 Tests

- 1 077 inline `#[test]` in 113 source files; 1 698 in `tests/` across 132 files.
- `al-vm/tests` has 52 files, so 52 test binaries, each linking the whole stack. 44 define their own compile and
  run helper; `fn run` is copied 39 times. There is no shared `common` module or test-kit crate.
- `al-test-cli/tests/corpus_harness.rs` starts the binary 15 times against the corpus; line 70 runs
  `--all-versions` and is not ignored.
- `Catalog::load()` without a `OnceLock` adds its parse to every compile in every test (F1.4).

### 5.6 What makes the code hard for future agents to change safely

1. Large files with many owners: merges in `lower.rs` and `al-vm/src/lib.rs` are the norm.
2. Seams typed as strings: the compiler does not tell an agent where a new kind or builtin must be handled.
3. Two type systems and two name resolutions: an agent must change both and keep them equal.
4. Four compile paths that must agree.
5. Test helpers copied 39 times: a change to how a test compiles touches dozens of files.

---

## 6. Recommendations (ranked by future cost)

Each item says what, why, the expected gain and how it was estimated, the cost in agent-days, the risks, and how
to do it step by step under the zero-difference rules (corpus ratchet, `demand_vs_eager`, `cache_vs_uncached`,
`ALRUNNER_CHECK_INCREMENTAL`, `baseline_rule_check`).

### R1. Symbolic IR and a link step (build ADR-011 decision 3)

- **What.** Lowering writes stable references: objects as `(ObjKind, id)` (or a name key for objects without an
  id), routines as `(object, routine key)`, fields as `(table id, field number)`, controls as `(page, control
  name)`. A new `link` step in `al-vm` (or a small `al-link` crate) builds today's dense `Program` from it.
- **Why.** It removes the root cause behind ADR-022 findings 1 and 2, the "workspace last" order, `relower_set`,
  `shift_positions`, and ADR-029's "declarations must not change". Cache blocks become a function of their own
  object plus declarations, so per-app entries (ADR-022 design B) and per-object incremental compile become a
  matter of keys, not remaps.
- **Gain.** Not a speed gain by itself. It enables: a Base Application hotfix reusing all unchanged blocks
  (ADR-026 table: 7.7 to 12.4 s rebuild down to the changed objects); ADR-029 extended to declaration changes;
  one less compile path (cached equals eager by construction). Estimate from ADR-022 section 7 and ADR-026.
- **Cost.** 10 to 15 agent-days. ADR-022 estimated 4 to 5 weeks for a *remap* (design B). A link step is
  smaller, because lowering emits keys and nothing has to be remapped after the fact.
- **Risks.** Wrong-field or wrong-object bugs. Merged field order (own fields, system fields, extension fields in
  workspace order) must be reproduced by the linker exactly.
- **Incremental path.**
  1. Add a `Ref` type for each reference kind next to the positional fields, filled by lowering. Keep both.
  2. Write `link(symbolic) -> Program`. Assert `link(symbolic) == today's Program` on the corpus, Tests-ERM,
     NP Retail and all three BC majors. This is a pure equality, so zero differences is exact, not statistical.
  3. Switch the cache to store symbolic blocks (cache format bump; ADR-022 amendment).
  4. Remove the positional fields from the symbolic form. The VM keeps reading the linked `Program`.

### R2. Split the IR into a symbolic form and an executable form

- **What.** The symbolic IR (R1) stays the seam, serialized, stable. The executable form is private to `al-vm`,
  built by the link step, never serialized: pre-resolved builtins (fn pointer or `BuiltinId`), constants as
  `Value`, routine flags (`is_try`, `commit_behavior`, `collects_errors`, `is_publisher`) as fields, record local
  templates, typed schema flags (temporary, CRM), positions as a side table instead of `Stmt::Pos` entries.
- **Why.** It ends the conflict in F2.2. Interpreter work no longer changes the cache format; cache work no longer
  slows the interpreter.
- **Gain.** Enables R3 to be finished and R6 to happen. ADR-022's decode cost (allocation-bound, 280 MB) drops
  when the symbolic form loses its strings.
- **Cost.** 5 to 8 agent-days on top of R1 (most is moving the R3 items into the link step).
- **Risks.** Low. The executable form is internal (ADR-009).
- **Incremental path.** Start as `ExecProgram { program: Arc<Program>, routines: Vec<Vec<RoutineMeta>>,
  builtins: Vec<NativeFn>, consts: Vec<Value> }` built in `Engine::with_config`. Move one thing at a time.

### R3. Interpreter quick wins (do these first)

- **What.**
  1. Integer fast path in `ops::arith` and the comparison operators. (measured)
  2. Identity conversions return the value; lowering drops `Convert` when source and target `RtType` are equal.
     (measured at run time)
  3. Builtin lookup by hash map now, by pre-resolved id after R2. Move the VM special cases
     (`lib.rs:1363-1487`) into the registry so `eval` has one path. (measured)
  4. Constants built once (`Decimal` parsed at lowering into a stored decimal, `Arc<[String]>` shared for
     Options, `Txt` shared for text).
  5. `Signal::Error(Box<AlError>)`: `Result<Value, Signal>` from 72 to about 40 bytes.
  6. Routine flags computed once (F2.7); a `bool` on the routine for "is a publisher".
  7. `for` loops: an Integer counter path without `BigInteger -> convert -> write -> read`; no `place_type`
     clone per iteration.
  8. `Catalog::load()` behind a `OnceLock`. Executable hash computed once per binary.
- **Gain.** Items 1 to 3: **-30 % instructions** over the six benchmarks, -40 to -42 % on Integer loops, calls and
  builtins (measured, table in section 2.2). Items 4 to 7: expected a further 30 to 50 % on the same loops,
  estimated from the profile shares (`eval` moves 20 to 35 %, `constant` and `from_str` 6 % of `DecimalLoop`,
  `convert` still 13 % of `IntLoop`). Total expected: 2 to 3x on compute-heavy code.
- **Cost.** 3 to 5 agent-days.
- **Risks.** Overflow and division-by-zero texts must stay the same; `Mod` and `div` signs must match `Decimal`
  results (they do for `i32`). Each item cites the corpus test that pins the behaviour (hard rule 3).
- **Incremental path.** One item per commit, each with a red/green unit test and a full corpus run. Add the
  benchmark below (R4) first, so each item shows its gain.

### R4. An interpreter performance budget in CI

- **What.** Commit the six benchmarks (appendix A) as an app under `crates/al-test-cli/tests/` or `tools/`, and an
  `#[ignore]`d budget test that counts instructions per iteration (or wall time under `flock`, as the existing
  budgets do). Add a Pageworks-like real-code benchmark when one can be shared.
- **Why.** Today a 20 % slower VM passes CI. ADR-011 decision 4 ties bytecode to budgets that do not exist for
  compute-heavy code.
- **Cost.** 1 agent-day. **Risk:** noise on a loaded runner; instruction counts avoid most of it.

### R5. Typed ids across seams

- **What.** `ObjKind` in the IR instead of `kind: String`. A generated `BuiltinId` enum. A `RecordOp` enum (and
  `PageOp`). A `LibraryKind` enum. Typed schema properties instead of `Vec<(String, String)>` lookups at run time.
  Interned company id in `MemStore`.
- **Why.** F5.2. The compiler then tells an agent every place a new kind or builtin must be handled.
- **Gain.** Removes about 600 string-compare sites; faster decode (fewer strings in the cache); removes the
  string dispatch costs in R3 item 3 for good.
- **Cost.** 6 to 10 agent-days, done one enum at a time.
- **Risks.** Low; mechanical. A catalog that changes per BC version (ADR-015) needs the enum to cover the union
  of versions, with availability per target.
- **Incremental path.** One enum per branch. Each is a pure refactor: corpus, buckets and all differential tools
  must show zero differences.

### R6. An executable form that is fast (after R1, R2, R3)

- **What.** First profile Pageworks and two Microsoft buckets to see the real split between arithmetic, calls,
  builtins and record operations. Then choose:
  - **Closure compilation** of the linked IR (each node becomes a pre-resolved closure with its operands typed).
    Cheapest step from a tree walker; keeps the code shape.
  - Or a **register bytecode** with typed instructions and inline caches for interface calls, record field
    reads and event dispatch.
  Not a JIT (section 3.2).
- **Gain.** Typical published results for this step are 3 to 10x over a naive tree walker. Expected for AL code:
  2 to 4x on top of R3, less where record operations dominate (then R7 matters more).
- **Cost.** 15 to 25 agent-days.
- **Risks.** A second interpreter to keep equal. Mitigation: keep the tree walker as the oracle; add a dual-run
  mode (`AL_DUAL_RUN=1`) that runs each test with both and compares outcomes, store and error texts. Run it on the
  corpus and the buckets until zero differences, then switch the default.
- **Incremental path.** Per routine: the linker compiles a routine to the new form if every node is supported, else
  the routine stays on the tree walker. Coverage grows routine kind by routine kind.

### R7. Record store representation

- **What.** Rows as `Rc<[Value]>` (copy on write) instead of `Vec<Value>` clones; primary keys encoded once into a
  byte string that compares with `memcmp` (collation folded in at encoding); tables keyed by `(CompanyId, u32)`;
  `AccessContext` by id, not `String`s; precomputed per-table flags (`has_unevaluable_init`, temporary, CRM); no
  `TableDef` clone for ad-hoc keys.
- **Why.** `RecordLoop` costs about 28 000 instructions per operation; `key_cmp`, `compare` and `Vec` clones are a
  third of it.
- **Gain.** Expected 2 to 3x on record-heavy code (from the profile shares: compare + clone + malloc about 40 %).
- **Cost.** 5 to 8 agent-days.
- **Risks.** Sort order is observable (ADR-009). The key encoding must reproduce `key_cmp` exactly; test it as a
  property test against `key_cmp` on random values and on the corpus.
- **Incremental path.** Behind the `Store` trait (ADR-003). The `al-data` contract suite plus the corpus.

### R8. One name resolution: a bound tree between check and lower

- **What.** The checker produces, per routine, a typed tree with every name resolved to a slot, field number,
  object id or builtin id. Lowering consumes it, not the CST and not `PosMap`. `Ty` refers to objects by id.
- **Why.** F1.1: two walks, two resolutions, hash maps keyed by byte offsets. Every language feature is built
  twice. It also gives incremental compile a unit (a routine's bound tree) that does not depend on file offsets.
- **Gain.** Compile: tree walking, hashing and allocation are about 45 % of compile CPU; the second walk and the
  map lookups are an estimated 15 to 25 % of check plus lower (estimate from the profile categories, not
  measured separately). The larger gain is maintenance.
- **Cost.** 20 to 30 agent-days. This is the largest item.
- **Risks.** It moves most of `lower.rs` and `check.rs`. Must be done with R1's equality check in place:
  `Program` from the new path equals `Program` from the old path, object by object.
- **Incremental path.** First give `Ty` an object id next to the name (cheap, removes the 21 `resolve_obj` calls
  in lowering). Then `Res::Local(slot)` instead of names. Then statement kinds one by one, with the old lowering
  as the fallback for kinds not moved yet.

### R9. Code structure

- **What.**
  1. Split `Session` into sub-states (section 5.1). Split `lower.rs` into `lower/{stmt,expr,call,pages,reports,
     xmlports,dotnet,assemble}.rs`; same for `check.rs` and `al-vm/pages.rs`. Move inline test modules of more
     than 300 lines into `tests/`.
  2. One where-condition parser in `al-sema`; view text parsed at lowering, not at run time; one `unquote`, one
     `split_top`, one type-name renderer.
  3. One test-kit crate (`al-testkit`) with the compile and run helpers; merge `al-vm/tests` into a few binaries.
  4. One JUnit writer; `al-test-cli` goes through `al-testrun` only.
  5. A file-size rule in `check.sh` (for example, warn above 2 000 lines of non-test code).
- **Why.** Section 5.6. It lowers the merge and review cost of every future change.
- **Gain.** Not measurable in seconds; test build and link time drop with fewer test binaries.
- **Cost.** 6 to 10 agent-days, spread out. Each split is a pure move.
- **Risks.** Merge conflicts with agents working in the same files. Do each split when the file has no open
  branches, in one short commit.

### R10. Fewer compile paths (after R1)

- **What.** With symbolic blocks, the cached path and the eager path are the same function. Demand only decides
  which objects to lower. Incremental compile reuses blocks by key.
- **Gain.** Removes `relower_set` and most of `cache_vs_uncached`. Each new feature is built once.
- **Cost.** 5 to 8 agent-days after R1.

### Summary table

| Rank (future cost) | Item | Agent-days | Gain | Order in time |
|---:|---|---:|---|---:|
| 1 | R1 symbolic IR and link step | 10-15 | per-app cache, incremental for declaration changes, enables R2 and R10 | 3 |
| 2 | R2 symbolic and executable forms | 5-8 | ends cache vs speed conflict | 4 |
| 3 | R5 typed ids across seams | 6-10 | about 600 string sites gone; safer changes | 2 (start) |
| 4 | R8 bound tree, one resolution | 20-30 | one place per feature; 15-25 % of check+lower (estimate) | 7 |
| 5 | R9 code structure | 6-10 | lower merge and review cost | ongoing |
| 6 | R10 fewer compile paths | 5-8 | one path per feature | 6 |
| 7 | R6 fast executable form | 15-25 | 2-4x on top of R3 (estimate) | 5 |
| 8 | R7 record store | 5-8 | 2-3x on record-heavy code (estimate) | 5 (parallel) |
| 9 | R3 interpreter quick wins | 3-5 | -30 % measured; 2-3x expected | 1 |
| 10 | R4 VM performance budget | 1 | guards all of the above | 1 |

---

## 7. Proposed sequence of ADRs

1. **ADR-030: Interpreter performance budget and fast paths.** R4 and R3. Mostly internal (ADR-009), but it sets a
   new CI budget and changes `Signal` (a seam). Small.
2. **ADR-031: Typed identities across seams.** R5: `ObjKind` in the IR, `BuiltinId`, `RecordOp`, `LibraryKind`,
   typed schema properties. Amends ADR-008 (builtins keyed by overload id: the id becomes an enum generated from the
   catalog) and ADR-010 (`Library` kind).
3. **ADR-032: Symbolic IR and the link step.** R1 and R2. Builds ADR-011 decision 3; amends ADR-022 (block
   format, per-app entries become possible) and ADR-029 (declaration changes). States the equality proof.
4. **ADR-033: Executable form and dual-run mode.** R6. Supersedes ADR-011 decision 4 ("tree walker until budgets
   need more") with the budget from ADR-030 as the trigger. Requires the Pageworks and bucket profile first.
5. **ADR-034: Store keys and rows.** R7. Amends ADR-003 and ADR-004 only in internals; the trait stays.
6. **ADR-035: Bound tree between check and lower.** R8. The largest change; after ADR-032 so the equality check
   exists.
7. **ADR-036: Code structure rules.** R9: module splits, the test-kit crate, the file-size rule, one parser per
   grammar. Can be accepted any time; the splits happen when files are quiet.

---

## Appendix A: the benchmark app

`app.json`: id range 70000-70099, runtime 14.0, no dependencies. A temporary table 70000 "Bench Row" (`No.`
Integer key, `Amount` Decimal, `Description` Text[100], `Qty` Integer, `TableType = Temporary`). One test
codeunit per benchmark for profiling (a test codeunit is the isolation unit, so `--filter` still runs every method
of the codeunit). Run as `alrunner test <folder> --no-test-data`.

```al
[Test] procedure IntLoop()      // for i := 1 to 3000000 do s := (s + i) mod 1000003;
[Test] procedure DecimalLoop()  // for i := 1 to 1000000 do d := d + i * 1.5 - 0.25;
[Test] procedure CallLoop()     // for i := 1 to 1000000 do s := Add(s, i);   Add: exit((a + b) mod 1000003)
[Test] procedure TextLoop()     // 300000 x: t := 'Item ' + Format(i mod 97); n += StrLen(CopyStr(t, 2, 3)); if StrPos(t, '5') > 0 then n += 1;
[Test] procedure BuiltinLoop()  // for i := 1 to 500000 do d += Round(Abs(i / 3), 0.01) + Power(2, 3);
[Test] procedure RecordLoop()   // 50000 Init/Insert; SetRange(Qty, 3) + FindSet/Next sum; 50000 Get/Modify
```

## Appendix B: the experiment (not committed)

```rust
// al-builtins/src/ops.rs, start of `arith`
if let (V::Integer(a), V::Integer(b)) = (l, r) {
    let r = match op {
        Arith::Add => a.checked_add(*b), Arith::Sub => a.checked_sub(*b), Arith::Mul => a.checked_mul(*b),
        Arith::IntDiv | Arith::Mod if *b == 0 => return raise("Attempted to divide by zero.", "AL:DivideByZero"),
        Arith::IntDiv => a.checked_div(*b), Arith::Mod => a.checked_rem(*b), Arith::Div => None,
    };
    if let Some(r) = r { return Ok(V::Integer(r)); }
    if op != Arith::Div { return raise("Arithmetic operation resulted in an overflow.", "AL:Overflow"); }
}
// al-builtins/src/ops.rs, `convert`: (Integer, T::Integer) | (Decimal, T::Decimal) | (BigInteger, T::BigInteger) => v
// al-builtins/src/native.rs, `lookup`: a OnceLock<HashMap<&'static str, NativeFn>> over `all()` before the scan
```

The corpus was not run with these changes. They are evidence for the size of the gain, not a finished patch.
