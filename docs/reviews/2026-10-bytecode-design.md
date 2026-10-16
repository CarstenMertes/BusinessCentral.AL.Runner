# Design note: register bytecode for al-vm (ADR-034, decision D4)

Status: proposal for step `exec-form`, written before any code. Date: 2026-10-16.

## Goal

Cut the dispatch cost of the tree walker (Pageworks profile: dispatch 45 %, `eval` 23 %, `exec_block` 10 %,
`docs/reviews/2026-10-pageworks-profile.md`) without changing one observable result. The tree walker stays as the
oracle (ADR-034 decision 4). Nothing here is serialized; the form is built inside `al-vm` from today's
`al_ir::Program`.

## Principles

1. **Same helpers, new dispatch.** An instruction calls the same functions the tree walker calls (`ops::arith`,
   `ops::compare`, `ops::truthy`, `ops::convert`, `Session::read_record_field`, `Session::call`). The gain is the
   removal of recursive `eval`, `Result` plumbing per node, and `Box` chasing. Semantics are shared by construction.
2. **Per-routine fallback.** A routine compiles only if every node in it is supported. Otherwise it runs on the tree
   walker. Both forms enter and leave through `Session::call_inner`, so a compiled routine may call a fallback
   routine and the reverse.
3. **Capture, coverage and profile stay exact.** When a capture is on, the routine runs on the tree walker (capture
   is the exception path). Coverage and profile hooks run in the `Pos` instruction, the same code as `Stmt::Pos`.
4. **Each step merges green.** Fallback share is reported per step.

## Register model

A routine's frame today is `Frame { object, routine, instance, locals: Vec<Value> }`. The registers are
`locals` extended with temporaries:

- `r0 .. r(L-1)` are the routine's locals and parameters, unchanged (`L = routine.locals.len()`).
- `rL ..` are temporaries the compiler allocates (expression results), stack-like per statement, all `Value::Null`
  at entry.
- After the call, `call_inner` truncates to `r.params` as it does now, so temporaries never leak.

A register holds a `Value`. Operands are `u16` indexes (a routine with more than 65 535 registers is not compiled).
Reads of a local into an operation borrow the register (no clone); a result is written with a move into its
destination. Reading a local for storage elsewhere clones, as `Expr::Load` does now.

Globals (`Place::Global`, `BaseGlobal`), array elements and record fields are not registers. They are loaded into a
temporary by an instruction and stored back by another, with the same helpers as `Session::read` and `write`.

## Instruction set

Operands are registers (`a`, `b`, `d` = destination), constant indexes (`k`), or jump targets (`t`, an index into
`code`). One enum, 16 bytes or fewer per instruction (checked by a test).

Step 1 (first subset):

| Group | Instructions |
|---|---|
| Data | `Move d a`, `Const d k`, `LoadGlobal d i`, `StoreGlobal i a` |
| Arithmetic | `Arith op d a b` (the `ops::arith` path), `AddI d a b`, `SubI`, `MulI` (Integer fast path; falls to `ops::arith` on any other pair), `Neg d a`, `Not d a` |
| Compare | `Cmp op d a b` (`ops::compare`, `compare_order`), `LtI d a b` and `EqI` fast paths for Integer pairs |
| Convert | `Convert d a ty` (type table) |
| Control | `Jump t`, `JumpIf a t`, `JumpIfNot a t` (both use `ops::truthy`, so a non-Boolean raises the same error), `Return` and `ReturnValue a` |
| Loops | `ForInit`, `ForStep` (see below), back-edge `Jump` checks `check_stop` |
| Calls | `Call d routine argbase n`, `CallInstance`, `CallTry` (`try_call`) |
| Builtins | `Builtin d id argbase n flags` (the `NativeFn` per site is resolved once into a side table) |
| Position | `Pos p` |

Later steps add record field access, `Case`, `In`, `Foreach`, `AssertError`, interface calls, `Expr::DotNet` family,
`Control`, `QueryColumn`, and the remaining place forms. Each stays a fallback until implemented.

Arguments for calls and builtins go in consecutive temporaries starting at `argbase`; by-reference arguments
(`Arg::Ref`) are written back after the call exactly as `finish_call` and the builtin path do today. The write-back
list is part of the instruction's side-table entry.

### Loops and short circuits

AL evaluates both operands of `and`/`or` (docs/probes-needed.md); the compiler emits both and a plain `Arith` on
Booleans, never a jump. `while`, `repeat`, `for` become jumps. `break` and `continue` become jumps to the loop's
exit and next-round labels; the tree walker's `Signal::Break` and `Signal::Continue` do not occur inside compiled
code. `exit` becomes `Return` (or `ReturnValue`). A `break` or `continue` inside a construct the first subset does
not cover (`AssertError`, `Foreach`) makes the whole routine fall back, so no signal ever crosses the two forms.

`for` mirrors `S::For`: bounds evaluated once, the Option/Enum/Boolean bound handling and the assumption markers
are done by one `ForInit` instruction that calls the shared helper the tree walker will also call (it is extracted,
not copied, so both forms keep one definition). `ForStep` stores the counter and tests the bound.

### Positions

`Stmt::Pos(p)` becomes `Pos p`. A statement after a loop restores the loop's position (`restore_pos`) exactly as the
tree walker does: the compiler emits the same `Pos` re-set at the loop's condition. The set of `Pos` instructions is
the same as the set of `Stmt::Pos` in the IR, which the dual run checks via the call-stack texts.

## How values, errors and Ctx calls work

- **Values.** `Value` is unchanged. Registers own values. No unsafe code; instructions that need two registers
  mutably use `split_at_mut` or take the operand out with `std::mem::take`.
- **Errors.** `run` returns `Result<(), Signal>`, the same type as `exec_block`. `Return` ends the loop and hands
  `Signal::Exit(v)` to `call_inner`, which already turns it into the return value. So `call_inner` needs no new
  contract; it only selects the form. An `AlError` raised in an instruction is returned at once, and the error's
  call-stack frame is added by `call_inner` as today.
- **Ctx / natives.** A `Builtin` instruction evaluates arguments into registers, copies them into a `Vec<Value>`
  (the `&mut [Value]` the `NativeFn` wants; a reusable scratch buffer later), calls the native with
  `self as &mut dyn Ctx`, writes by-reference arguments back, and stores the result. The VM-run builtins
  (`BuiltinId::class()` and the `ids::` constants in `lib.rs` `eval`) are handled by the same function the tree
  walker uses; that function is extracted from `eval` into `fn builtin_call(&mut self, id, receiver, args, ret_used,
  f)` taking already-evaluated values where possible. Step 1 supports only builtins that go through `native::by_id`
  with plain arguments; the VM-run builtins (subscriptions, ClearAll, Codeunit.Run, ...) stay fallback.
- **Calls.** `Call` evaluates arguments, then calls `Session::call(object, routine, instance, args)` (the same entry
  `finish_call` uses), so depth limit, frames, try functions, commit behavior, trap scopes, coverage and error
  frames are all in one place.

## Where the code lives

- `al-vm/src/exec/mod.rs`: `ExecProgram` (lazily filled `OnceLock<Option<Chunk>>` per routine, so unused Microsoft
  routines cost nothing), `Chunk`, `Op`, the run loop.
- `al-vm/src/exec/compile.rs`: IR to `Chunk`; returns `None` for an unsupported node. Counts why (a histogram by node
  kind) for the fallback report.
- `al-vm/src/interp.rs`: `call_inner` asks `engine.exec.chunk(object, routine)`; with a chunk and no capture it runs
  `exec::run`, else `exec_block`.
- An environment switch `AL_EXEC=tree|bytecode|dual` selects the mode (default `tree` until the last step); `dual`
  is the hook lane D's harness uses.

## Dual run (lane D)

`Session::exec_mode` is `Tree`, `Bytecode` or `Dual`. In `Dual` the harness runs each test codeunit twice on
equal fresh sessions, one per form, and compares: outcome, error text and code, call-stack text, messages and UI
events, and the store after the test. The first divergence names the test. This note defines only the switch.

## Plan and exit criteria per step

1. **Subset.** `ExecProgram`, compiler for locals, constants, arithmetic, compares, branches, loops, calls, simple
   builtins, `Pos`; fallback otherwise. Gate: `check.sh`; `--all-versions` 0 outcome differences with
   `AL_EXEC=bytecode`; perf_vm deltas and fallback share reported (routines compiled, and by executed calls).
2. **Dual-run harness** (lane D).
3. **Widen** node kinds in profile order until fallback by executed calls is near zero. Per node kind: a test run on
   both forms.
4. **Default switch** when the dual run shows 0 differences on the corpus and every running bucket and
   `perf_vm` shows the gain.

## Risks

- `try_call`, `AssertError`, and error finals (`error_finals`) are subtle; they stay in `call_inner` or fall back.
- Temporaries in `locals` could show up in code that iterates `f.locals` (capture, `end_trap_scope`). Capture runs
  only on the tree walker; `end_trap_scope` is checked by a test that a routine with temporaries traps like one
  without.
- Two forms to keep equal: the dual run is the guard (ADR-034, owner decision D5).

## Typed instructions (design note, 2026-10-16, lane A; proposed, not built)

Goal: where the compiler knows both operand types, emit an op that does no run-time type dispatch. The generic op stays
as the fallback, so a wrong or missing type never changes a result. Direction and later steps: `docs/ideas.md`, "Faster
than BC at raw computation" (typed instructions, inline caches, superinstructions, Cranelift).

### What exists today, and how much a typed op can win

`BinI` already has an Integer fast path (two `Value::Integer` tags tested, `checked_*`, else `Session::binary_op`).
So the gain of an `AddInt` is not "skip the generic op". It is smaller and has three parts:

1. No tag test and no fallback branch on the hot path (the type is proven, so the op reads `i32` directly).
2. No `release` of the operands and no drop of the old destination: an op that knows its registers hold plain values
   does `locals[d] = Value::Integer(x)` with no drop glue (today `set()` still tests the old value).
3. It is the form a Cranelift translation needs: a register with a proven type becomes an SSA value of that type.

Expected size: Int loop 640 instructions per iteration should fall by roughly a quarter to a third; calls and records
barely move. I will measure after the first op family and report, not promise.

### Where the types come from

The IR carries no expression types. It has only: `Convert(RtType, e)`, `Const`, and the declared `RtType` of locals
(`Chunk.locals_ty`, already used for `For`/`Foreach`), globals, and routine returns. So the compiler infers a type with
a pure function `static_ty(&Expr) -> Option<Ty>` (`Ty` = Integer, BigInteger, Decimal, Boolean, Text, Code, Date, Time,
DateTime, Char, Option, ...), conservative: `None` means unknown and the generic op is emitted.

- `Const(Integer/Decimal/Boolean/Text)`: its own type.
- `Load(Local i)`: `locals_ty[i]` if it is a scalar. Not for `Variant`, not for a `var` parameter (a by-reference
  place may hold another value kind; check how `read` resolves it), not for temporaries until step 3 below.
- `Load(Global i)`: needs the global's type; the compiler sees only the routine today, so globals stay generic in step 1.
- `Convert(t, _)`: `t`. `Binary` of two typed operands: by the same rules as `ops::arith` (Integer op Integer is
  Integer except `/`, which is Decimal; Integer with Decimal is Decimal; comparisons are Boolean).
- Calls and builtins: the declared return type, only if the callee's `RtType` is available at compile time. Not in step 1.
- Temporaries: the register of a typed op's result has a known type, kept in a compile-time map register to `Ty` for the
  life of the temporary.

An unsound type would be a wrong result, so the dual run is the check, plus one rule: a typed op re-checks nothing but
is only emitted from a proven type; in debug builds it `debug_assert!`s the tag, so the test suite and the dual run on
the corpus trap an unsound inference instead of reading garbage. A local of declared type Integer must hold
`Value::Integer` at every read. I will verify this (default values on frame set-up, `Clear`, `Evaluate`, by-reference
write-back) and list each place where it can hold `Null` before any local is treated as typed. If one exists, that
local stays generic.

### Which ops, in order

Step 1 (Integer, Boolean and compare/jump; the loop kernels):
- `AddInt`, `SubInt`, `MulInt`, `ModInt`, `DivIntInt` (the `div` operator), each `d, a, b`; `AddIntK` and `SubIntK`
  with an immediate (counters, indexes).
- `CmpInt{Eq,Ne,Lt,Le,Gt,Ge}` producing a Boolean, and fused `JumpIfInt{Lt,...}` that skip the Boolean register
  (superinstruction, step 3 of the vision, cheap once types exist).
- `JumpIfFalseBool`, `NotBool`, `AndBool`/`OrBool`/`XorBool` (non-short-circuit, as AL's `and`/`or` evaluate both sides today;
  keep whatever `binary_op` does).
- `MoveInt`-style copies are not needed: `Move` of a plain value is already a copy; only the typed result matters.

Step 2 (Decimal, Text, Date):
- `AddDec`, `SubDec`, `MulDec`, `CmpDec*`; `CmpText*`; `ConcatText` (`Text + Text`, reusing the append buffer of
  `AppendText` where the left side is dead); `CmpDate*`.
- Date and Time arithmetic stay generic (rare, and the rules are subtle).

Step 3: typed temporaries and typed calls/builtin returns, then fused load-and-compare on record fields.

### Semantics stay identical: overflow, Decimal, text

The rule is that a typed op calls the same helper or the same expression as the generic path, never a copy of its logic.

- **Integer.** Overflow, divide by zero and `mod` use the one function body `ops::arith` uses for `(Integer, Integer)`.
  I will extract that into `#[inline(always)] pub fn int_arith(op, a: i32, b: i32) -> Result<i32, Signal>` in
  `al-builtins/src/ops.rs`; `arith`, `BinI` and the typed ops all call it. `MIN mod -1` is 0, errors are the two
  existing texts, so a test that runs each of the five operators on a grid of edge values (MIN, MAX, -1, 0, 1) through
  `arith` and through the typed op and compares both result and error is the guard. (I own al-builtins text functions; the
  `ops.rs` edit is a pure move and I will tell lane D/owner of that file in the hand-back.)
- **Decimal.** `rust_decimal`-style rounding, scale and overflow behavior stay inside `arith_general`. `AddDec` etc.
  call one extracted `dec_arith(op, a: &Decimal, b: &Decimal)` taken verbatim from the Decimal branch of
  `arith_general`; there is no second implementation. If the Decimal branch is not separable without changing behavior,
  Decimal stays generic and I say so.
- **Compare.** Integer and Decimal compares call `a.cmp(b)` exactly as the first arms of `ops::compare` do. Text
  compares keep `collation::compare`; `CmpText` is generic compare with the type test removed, not a byte compare.
- **Text concat.** `ConcatText` keeps the rule "an empty side leaves the other shared" and the maximum-length
  `fit` after the `Convert`; it removes only the `text_like` dispatch.
- **Errors and position.** Every typed op that can fail reports through the same `Signal` the generic op raises, at the
  same statement position (`Pos` ops are unchanged), so error text, code and call stack match.

### Proof and risk

- Red/green: each op family starts with a test pair. The edge-value grid test is red until the helper exists. A test
  that a typed op is emitted (chunk disassembly count) is red until the compiler emits it. A regression guard where
  behavior is unchanged is labeled as such in the commit message.
- `--dual-run` 0 differences on the corpus (27, 28, 29) and Pageworks (812 passing); `perf_vm` per benchmark with
  lowered limits; Pageworks instructions before and after, same machine.
- Risks: (1) an unsound static type, covered by the debug assert and the dual run; (2) a second code path for each
  operator, covered by sharing the helpers; (3) code size of `run_range` (one loop, hot icache): measure, and put cold
  arms `#[inline(never)]`; (4) typed ops are only worth it if a later Cranelift step uses them, so step 1 is kept small
  (Integer and Boolean) and the numbers decide whether step 2 goes ahead.
- Question for the owner before step 2: Decimal typed ops pay off only when Decimal arithmetic is hot, which perf_vm's
  Decimal loop shows (1780 instructions per iteration); Pageworks (QR, fonts) is Integer and Boolean heavy.

## Retiring the tree walker (2026-10-16, lane A)

State after this step: no statement and no expression of the IR needs the tree walker.

**What runs on bytecode now that did not.**
- Value capture and loop tracking (ADR-030) have a captured form of each tracked routine (`compile(.., true)`,
  `ExecProgram::captured_chunk`). It adds `CapHit`, `CapBegin`, `CapEnd`, `CapPop`, `CapLoopEnter`, `CapLoopPass`,
  `CapLoopExit` and the loop id carried by `ForSite` and `ForeachSite`, and calls the same `cap_*` functions in the same
  order as `exec_block_captured`. A `break`, `continue`, `exit`, an error and an `asserterror` that catches one close the
  statements they leave (`CapPop`, `CapFrame::stmt_base`, a depth restore in `op_assert`).
- `Foreach` over any collection (globals, routine results, .NET enumerators: `foreach_dotnet_start` and
  `foreach_dotnet_next` are the tree walker's loop split in two), `Stmt::Assume`, the interface call with computed
  arguments (`IfaceResolve` resolves the target before the arguments run), `DotNetCoerce`, `DotNetOperand`,
  `Bind/UnbindSubscription`, `CurrFieldNo`, `ClearAll`, `ApplicationArea` (`Special`), and `Refused`, `Fail` and a builtin
  nothing implements (`Fault`).
- Every routine compiles at its first call (the hot threshold is gone). On Pageworks the instruction count is the same
  with and without it (556.3 G and 556.5 G): compiling a routine that runs once costs about what the tree walker costs
  to run it.

**Proof.** `crates/al-testrun/tests/capture_modes.rs` runs four programs under `AL_EXEC=tree` and `AL_EXEC=bytecode` in two
processes and compares the whole capture (values, loops, iterations, statements, messages); breaking one instruction
makes it fail. The capture output of Pageworks (the four `VectorMatchesReference` tests, 109 MB) and of Tests-ERM
(`TestPmntDisc*`) is byte for byte equal. `--dual-run`: 0 differences on the corpus (27, 28, 29) and Pageworks.
`AL_EXEC_REPORT=1`: no tree node on the corpus, Tests-ERM and Pageworks.

**What the tree walker is still used for.**
1. `AL_EXEC=tree` and `AL_EXEC=dual`: the oracle of the dual run (a CI step) and the fallback switch.
2. A routine that needs more than 65535 registers (`compile` returns `None`): never seen in the corpus, Tests-ERM or
   Pageworks.
3. Code that only the tree walker reaches: `exec_block_captured` and `exec_block_dyn` (capture on the tree walker),
   `foreach_dotnet` (its loop), `Session::eval` and `exec` themselves, and the
   `ExecMode::Tree` branches in `call_inner`.

Deleting the tree walker means dropping the oracle of the dual run, so it needs the ADR-034 amendment. What would replace
the oracle: the corpus, whose expected results come from real BC, and Pageworks; the dual run protects against a
difference between two forms that no longer both exist.
