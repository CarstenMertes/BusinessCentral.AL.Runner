# ADR-036: One resolved tree from check to lower

- Status: Accepted (2026-10-15, decided by the orchestrator on the owner's delegation; see docs/owner-review.md). Amended 2026-10-16: the bound tree (decisions 1 and 3, migration steps 3 to 5) is not pursued; see the amendment at the end.
- Date: 2026-10-15
- Related: ADR-011 (typed IR), ADR-013 (demand-driven check), ADR-029 (incremental compile), ADR-033 (symbolic IR),
  review `docs/reviews/2026-10-compile-ir-review.md` (F1.1, R8)

## Context

The checker and the lowering both walk the CST and both resolve names:

- The checker stores `PosMap<Ty>` and `PosMap<Res>` keyed by `(file, start offset, end offset)`
  (`al-sema/src/check.rs:39-171`). `Ty` names objects by string (`types.rs:7-48`). `Res::Local(String)`,
  `Res::Global(String)` carry names; `Res::Proc { object, offset }` a byte offset.
- The lowering walks the CST again, clones `Ty` out of the map (`ty_of`, 41 sites), resolves object names again
  (`resolve_obj`, 21 sites), and keeps locals and globals as `HashMap<String, usize>` by lower-case name.
- In a profile of the corpus compile, tree walking, hashing and allocation are about 45 % of the CPU; checker and
  lowering logic about 12 %.

Every language construct is built twice, in `check.rs` and `lower.rs`, and the two can disagree. The check output is
keyed by byte offsets, so it cannot be reused when a file changes.

## Decision

1. **The checker builds a bound tree** (`al-sema/src/bound.rs`): per routine, a typed tree in which every name is
   resolved to a local slot, global slot, `FieldKey`, `RoutineKey`, `ObjKey` (ADR-033) or `BuiltinId` (ADR-032), and
   every expression has its type.
2. **`Ty` names objects by `ObjKey`**, not by string. Display names come from the workspace when a message needs one.
3. **Lowering reads the bound tree only**, not the CST and not `PosMap`. `PosMap` is removed when the last
   construct moves.
4. **Diagnostics and AL compiler codes** stay in the checker; the bound tree of a routine with an error holds an
   `Error` node, and lowering refuses it as today.
5. **Incremental unit.** A routine's bound tree depends on its own text and the declarations it read (ADR-033 read
   lists), not on file offsets.

## What changes at which seam

- `al-sema` inside: `check.rs` produces bound trees; `lower.rs` consumes them. `CheckResult::types` and
  `resolutions` are replaced by `CheckResult::bound`.
- `al-test-cli` code that calls `al_sema::check::check` directly (`coverage.rs`, `mnc.rs`) moves to the bound tree.
- `al-ir`, `al-vm`: none.

## Proof of zero behaviour change

- `Program` from the new path equals `Program` from the old path (`assert_same_program`, ADR-033), object by object,
  on the corpus (every major), Tests-ERM, Tests-SCM, NP Retail and the eager compile of the Microsoft apps.
- Diagnostics identical (text, code, position) on the corpus, `must-not-compile`, and the buckets.
- `tools/demand_vs_eager.sh`, `tools/cache_vs_uncached.sh`, `ALRUNNER_CHECK_INCREMENTAL=1`: 0 differences.

## Migration (each step merges green)

1. `Ty` carries `ObjKey` next to the name; lowering uses it and drops `resolve_obj` (21 sites).
2. `Res::Local(slot)` and `Res::Global(slot)`; lowering drops its name maps.
3. The bound tree for expressions, construct by construct (literals, names, members, calls, builtins, operators);
   lowering takes a bound expression where one exists and the CST path otherwise.
4. Statements, the same way.
5. Remove `PosMap` and the CST path of lowering.

## Consequences

- Unblocks: one implementation per language construct; per-routine incremental compile; faster check and lower
  (estimated 15 to 25 % of those phases, not measured separately).
- Blocked by: ADR-033 (keys and the equality helper); ADR-032 (`BuiltinId`).
- Cost: 20 to 30 agent-days, the largest item. Owner decision D6: build it all, or stop after step 2.
- Risk: it moves most of `check.rs` and `lower.rs`. The equality proof covers it; feature work in those files must
  stay paused while steps 3 to 5 run.

## Conformance

`assert_same_program` and diagnostic equality in CI on the corpus; the differential tools.

## Amendment 2026-10-16: the bound tree is not pursued

**What was measured.** A profile of the cold compile (`perf record -e instructions:u` on `perf_budget`, BC corpus pin 12)
showed no second name resolution of the size the review assumed ("about 45 % of CPU"). Parsing and the syntax tree
are about 40 % of the instructions; the two position maps (`PosMap<Ty>`, `PosMap<Res>`: inserts, lookups, rehash,
copies) are 5 to 9 %. Lowering reads the check's answers once; resolution already happens once.

**What was done instead (merged).** Steps 1 and 2 as cheaper changes: `Ty` names are `Arc<str>` (a copy shares the
name), the string-keyed maps of the check and the lowering use FxHash, a call's candidate list is borrowed, not cloned.
With the type-table of ADR-036 step 0 included, the cold compile went from 2,105,730 to 1,958,345 instructions per
object, under the old limit.

**What was tried and dropped.** Replacing `PosMap` by per-routine sorted chunks (the check appends to an open buffer;
a routine's end sorts it into a chunk; lookups binary-search the chunk, with a hint kept by the lowering). The share of
the instructions spent in the maps fell from 9.2 % to 4.7 %, but the cold compile did not get cheaper: 608,370 to 628,000
instructions per KiB of source with one thread per phase (+3.2 %), 627,128 to 632,886 with all threads (+0.9 %). The
rest of the compile grew by more than the maps saved, and the cause was not found. A single shard in place of 16 gave
-0.3 %. The chunk code is not kept in the repository.

**Decision.** The tree (migration steps 3 to 5) is superseded: it would change where an answer is stored, not how often
it is computed, its payoff is the 5 to 9 % of the maps, it costs 20 to 30 agent-days and raises the budget while it is
built, and the native-code path (Cranelift) starts from the IR or the bytecode, not from a tree. Decisions 2 (types
name objects without copies) and 4 and 5 stay as principles; the check stays the only place that resolves.

### What a later bound tree would reuse

Carries over unchanged:

- The check's answers by node position (`CheckResult::types`, `resolutions`): a tree is built from them, node by node,
  in one walk of the routine; nothing else resolves.
- `Ty` with shared names, and (when it lands) object keys in place of names.
- The type table of the lowered IR (`Routine::expr_tys`, `al_ir::types::ExprTypes`, `lower/tys.rs`): one type per IR
  expression in post-order, with the check that every record is the record of its node (kind, number of children,
  size). A tree node would hold the same type directly, and the table would then be dropped.
- The compile-dump equality (`compile_dump` byte-identical, `AL_CHECK_LINK`, `demand_vs_eager`, `cache_vs_uncached`)
  as the proof of each step.

A tree would add:

- Nodes with children and slots (local and global variable numbers instead of names), so lowering no longer asks the
  CST anything; the `Cst` fallback by position in the design of `docs/reviews/2026-10-bound-tree-plan.md` for the
  construct-by-construct migration.
- A per-routine node index that does not depend on byte offsets, for an incremental unit that survives edits above
  the routine (today the maps are keyed by offset, so an edit shifts every key below it).
- A place for facts that are not types or resolutions (conversions to insert, the `Stmt::Assume` marks, refusal reasons).
