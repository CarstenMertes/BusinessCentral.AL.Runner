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
