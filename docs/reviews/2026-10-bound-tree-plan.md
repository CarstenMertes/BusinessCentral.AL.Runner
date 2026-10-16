# Bound tree plan (ADR-036, decision D6: build fully)

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

## Migration order (each step merges green)

Each step keeps old and new paths side by side. The proof for every step is the same:
1. `compile_dump` byte-identical (with and without dependencies), against the commit before the step.
2. `AL_CHECK_LINK=1` 0 differences, corpus 27/28/29; diagnostics identical on the corpus and must-not-compile.
3. `demand_vs_eager.sh`, `cache_vs_uncached.sh`, `ALRUNNER_CHECK_INCREMENTAL=1`: 0 differences.
4. `check.sh` green; `al-test-cli --all-versions` exit 0.
5. Instruction count per object of the cold corpus compile (`perf_budget`), recorded in the commit message.

Steps:

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
