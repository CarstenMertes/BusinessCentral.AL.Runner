# Bound tree plan (ADR-036, decision D6: build fully)

**Not pursued (2026-10-16).** Steps 3 to 5 and the design sections below were not built. The profile showed no second
name resolution of the assumed size, the tree would raise the compile cost while it is built for a payoff of the 5 to 9 %
the position maps cost, and a replacement of the maps by per-routine chunks measured no net gain. See the amendment at
the end of `docs/adr/036-one-resolved-tree-from-check-to-lower.md`, which also lists what a later tree would reuse. What
was built: step 0 (the type table) and cheaper copies and hashing (results below). The rest of this file is kept as the
design record.

Status: plan only, no code yet. Author: rework lane C. Date: 2026-10-16.
Goal (owner-review, 2026-10-16): compile under 1,990,000 instructions per object (today about 2,014,000), then the
limit in `perf_budget` goes back down from 2,060,000.

## What the code does today

- `check/*` walks the CST per routine. It stores `types: PosMap<Ty>` and `resolutions: PosMap<Res>`, keyed by
  (file, start, end) (`check/mod.rs` `CheckResult`, `check/posmap.rs`, 16 shards, merged with one thread per shard).
- `Ty` names objects by string (`Record { table: String }`, `Enum(String)`, `Object(kind, String)`, ...).
  `Res::Local/Global/BaseGlobal/ThisBaseGlobal` carry lower-case names. `Res::Proc` carries (object index, byte offset).
- `lower/*` walks the CST again. It reads the maps through `Lowerer::res` and `Lowerer::ty_of` (`lower/convert.rs`;
  41 `ty_of` sites in `lower/`), resolves object names again through `resolve_obj` (16 sites), and looks locals and
  globals up by name in `HashMap<String, usize>`.
- Other readers of `CheckResult::types/resolutions`: `check/infer.rs` (writes), `check/tests.rs`,
  `al-test-cli/src/coverage.rs` (reads `resolutions`), `al-test-cli/examples/compile_dump.rs`.

## Bound tree types (`al-sema/src/bound.rs`, new)

One tree per routine, owned by the object's check result, not keyed by offsets.

- `BoundRoutine { key: LocalKey, locals: Vec<Slot>, body: Vec<BStmt>, error: Option<..> }`.
- `BExpr { ty: TyId, span: Span, kind: BKind }`. `Span` stays for diagnostics and source positions only.
- `BKind` variants, one per resolution result today: `Local(slot)`, `Global(slot)`, `BaseGlobal(slot)`,
  `Field { table: ObjRef, no }`, `Proc { callee: RoutineRef, args }`, `Builtin { id: BuiltinId, args }`,
  `EnumValue`, `OptionValue`, `ObjectId`, `QueryColumn`, `DotNetMember`, plus literals, operators, member access,
  indexing. It mirrors `Res` and the CST expression kinds, so each variant has an obvious source in `infer`.
- `BStmt`: assignment, call, if/case/loops, exit/error/with, one variant per statement kind lowering handles.
- Objects are named by `ObjKey` (ADR-033) or by the workspace index in the same check run. References inside a bound
  routine use the same per-object `Refs` idea as `SymObject`, so the tree is stable when files shift.
- `Error` node for a routine with a diagnostic; lowering refuses it as today (decision 4 of the ADR).

`Ty` changes first (step 1): object-carrying variants hold `ObjRef` (index plus `ObjKey`), display names come from the
workspace.

## Who reads it

| Pass | Today | After |
|---|---|---|
| `check/infer.rs`, `calls.rs`, `member.rs` | write `PosMap<Ty>`, `PosMap<Res>` | build `BExpr` (and still emit diagnostics) |
| `lower/expr.rs`, `call.rs`, `stmt.rs`, `convert.rs`, `dotnet.rs` | CST + `res`/`ty_of`/`resolve_obj` | `BExpr`/`BStmt` only |
| `lower/pages.rs`, `tables.rs` (CalcFormula, relations) | CST + maps | stay on CST until step 5; they hold few expression sites |
| `check/demand.rs` | reads `Res` to find reached routines | reads `BKind::Proc/Field/...` |
| `al-test-cli/coverage.rs`, `compile_dump.rs` | read `resolutions` | read bound trees |
| `incremental.rs` | recheck by file offsets | key by `LocalKey`; a routine's tree depends on its text and declarations read |

## Expression types for the bytecode compiler (lane A, ADR-034)

Lane A needs each expression's static type after lowering. Decision (coordinator, 2026-10-16): the types come from the
lowering itself, built together with the expression, never matched up afterwards. (Filling a table from `ty_of` in
`expr()` completion order was rejected: IR nodes made inline, reordered or dropped by lowering make the table misalign
without notice.)

**Interface (al-ir, `al_ir::types`; lane A codes against it now).**

```rust
/// Static type of an expression, small and `Copy`. `Unknown` = not recorded; the reader uses the generic instruction.
pub enum ExprTy { Unknown, Integer, BigInteger, Decimal, Boolean, Char, Byte, Text, Code, Date, Time, DateTime,
                  Duration, Guid, Option, Enum, Record, Other }
pub struct ExprTypes<'r> { .. }
impl<'r> ExprTypes<'r> {
    pub fn new(routine: &'r Routine) -> Self;     // one walk of the body
    pub fn of(&self, e: &Expr) -> ExprTy;         // e must be a node of `routine.body`; Unknown otherwise
}
```

`ExprTypes::of` is the `expr_ty` lane A wraps. A reader that gets `Unknown` must behave exactly as without the table
(so lane A's own conservative inference can stay behind the same function and be dropped kind by kind).

**Storage.** `Routine::expr_tys: Vec<ExprTy>` (`#[serde(default)]`), one entry per `Expr` node of the body in
post-order (children in field order, then the node; statements in order; `Place` index expressions and `Arg`s
included). An empty table means every node is `Unknown`. `ExprTypes::new` pairs the walk with the table by index, so
its lookup by node address is valid while the routine is not mutated (a linked `Program` is not).

**Construction (the sound rule).** Every lowering function that returns an `ir::Expr` returns `(ir::Expr, ExprTy)` and
pushes the entries of the whole subtree it built, in post-order, onto `Lowerer::tys` as it builds it. A node made
inline (a `Convert` wrapper, a folded constant) pushes its own entry where it is made. A function not converted yet
is wrapped: it discards the entries pushed since it started and pushes `Unknown` for every node of what it returned
(`node_count`). So a not-converted parent makes its subtree `Unknown`; it never misaligns. At the end of a routine the
count must equal the body's node count (`debug_assert` always, test on the corpus), and `compile_dump` prints the table.

**Order of conversion** (what lane A's typed ops need most): (1) `Const` and `Load` of a local or global, Integer and
Boolean; (2) `Binary`/`Unary`/compare of Integer and Boolean, with the `Convert` the lowering inserts; (3) Decimal and
Text/Code arithmetic and compares; (4) `Call`/`Builtin` results; (5) the rest (Record, field, control, DotNet,
interface). After each slice: `compile_dump` identical except the table section, expression counts match, instructions
per object not higher, and the share of non-`Unknown` entries on the corpus is reported.

**Cache.** `Routine` gains the field, so the cache `FORMAT` is bumped; blocks grow by about one byte per expression
(measured in slice 1).

## Migration order (each step merges green)

Each step keeps old and new paths side by side. The proof for every step is the same:
1. `compile_dump` byte-identical (with and without dependencies), against the commit before the step.
2. `AL_CHECK_LINK=1` 0 differences, corpus 27/28/29; diagnostics identical on the corpus and must-not-compile.
3. `demand_vs_eager.sh`, `cache_vs_uncached.sh`, `ALRUNNER_CHECK_INCREMENTAL=1`: 0 differences.
4. `check.sh` green; `al-test-cli --all-versions` exit 0.
5. Instruction count per object of the cold corpus compile (`perf_budget`), recorded in the commit message.

Steps:

0. **Expression types after lowering** (section above), converted in slices. First, because lane A waits on it. It is the first part of the bound tree: the types are built with the expressions.
1. **`Ty` with object refs.** `Ty::Record/Enum/Object/TestPage/...` carry the resolved object next to the name.
   Lowering drops `resolve_obj` at the 16 sites that start from a `Ty`. Expected win: the second name resolution.
2. **Slots for names.** `Res::Local(slot)`, `Res::Global(slot)`, `BaseGlobal`; lowering drops its name maps.
3. **Expressions, construct by construct**, each a separate merge. Order, simplest first, so the pattern settles:
   literals and constants; locals/globals/names; field access and member access; operators and conversions;
   builtin calls (`BuiltinId`); procedure calls; interface and enum expansions; DotNet members; page-control and
   query-column expressions. Lowering uses the bound expression when the node has one, the CST path otherwise.
4. **Statements**, same way: assignment, call statement, if, case, loops, exit/error, with, `begin..end`.
5. **Remove `PosMap` and the CST path of lowering**; `CheckResult::types/resolutions` go. `pages.rs` and `tables.rs`
   sites move here if any remain.

## Measurement and the stop rule

- After steps 1 and 2, measure instructions per object. The review puts tree walking, hashing and allocation at about
  45 percent of CPU; steps 1 and 2 remove the string hashing and the second resolution, so most of the gain should
  come there. If steps 1 and 2 already reach under 1,990,000, report it and let the owner decide whether to go on
  (D6 said build fully; the coordinator confirms).
- Each later step must not raise the count; a step that does is reworked or dropped.

## Risks

- `check.rs` and `lower.rs` are the files every feature touches. No other lane may change `al-sema/src/{check,lower}/*`
  while steps 3 to 5 run (rework plan, section 3). Steps 1 and 2 are small enough to merge between feature merges.
- Two implementations of one construct exist during step 3 and 4. Each construct's merge removes the old path for it,
  so the double code never exceeds one construct.
- Bound trees held for all routines raise peak memory. Demand-driven check keeps only reached routines; measure peak
  memory on Tests-ERM after step 3 (limit: no more than 5 percent above today).

## Size

Steps 1 and 2: about 4 agent-days. Steps 3 and 4: 12 to 18. Step 5: 2 to 3. Matches the ADR's 20 to 30 total.

## Needs the coordinator's decision

- Whether to stop after steps 1 and 2 if the budget is met there (see stop rule). Otherwise nothing blocks coding.

## Slice 2 result (2026-10-16)

- Built: `lower/tys.rs`. `Lowerer::tys` holds one record (type, kind, number of expressions directly inside, size) per
  node in post-order. `expr()` seals each node, `convert_to` seals the `Convert` it makes, `stmt()` checks the records
  of each statement, `routine()` keeps the table when the records tile the body. A record is kept only when the kinds
  and arities of the records below it match the node; otherwise the subtree is `Unknown`. In debug builds
  (all tests) every record is also compared with its node (`exact`).
- Filled (the type of the value the interpreter produces): `Const` (Boolean, Integer, BigInteger, Decimal, Text);
  `Load` of a local, global or base global of type Integer, BigInteger, Decimal, Boolean, Char, Byte; `Unary Not`
  (Boolean), `Unary Neg` of Integer; `Binary`: comparisons, `and`/`or`/`xor` (Boolean), `+ - * div mod` of two
  Integers (Integer); `Convert` to Integer, BigInteger, Decimal, Boolean, Char, Byte. Everything else is `Unknown`:
  field loads, calls and builtin results, Date/Time/Text/Code variables, `/`, Option, Enum, Record.
- Measured (corpus, no dependencies): 15,566 of 15,566 routines have a table; 145,650 of 534,370 nodes typed (27 %;
  107,798 of them Text constants). With the Microsoft apps: 421,390 of 1,686,563 nodes typed.
- Cost: cold compile 2,058,982 -> 2,105,730 instructions per object (+2.3 %). The records cost about 1.5 % and the
  per-statement and `Convert` checks the rest. It is over the limit of 2,060,000 (the tree before this slice was
  already 1,000 under it). Steps 1 and 2 (no second name resolution, no `ty_of` clones) have to bring it back.

## Result of the groundwork before step 3 (2026-10-16)

The profile of the cold compile showed no large second name resolution; the cost sat in copies and hashing. Changes
(none alters the lowered output; `compile_dump` is byte-identical, hashes included), instructions per object:

| change | per object |
|---|---|
| slice 2 | 2,105,730 |
| line starts by memchr, type lookups without a copy | 2,087,492 |
| `Ty` names are `Arc<str>` (a clone shares the name) | 2,064,651 |
| Fx-hashed maps in the check scope and lowering (`Fx` hash of a string reads 8 bytes at a time) | 2,045,937 |
| one copy of the name in `lookup_name` | 2,038,005 |
| Fx-hashed maps in the checker, workspace, reports and xmlports | 2,009,388 |
| candidate list of a call borrowed, not copied | 1,958,345 |

The limit in `perf_budget` is back at 1,990,000. Steps 3 to 5 (the bound tree, `PosMap` removed) are still to do and
are no longer needed for the budget.
