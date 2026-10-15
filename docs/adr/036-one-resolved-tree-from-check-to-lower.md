# ADR-036: One resolved tree from check to lower

- Status: Accepted (2026-10-15, decided by the orchestrator on the owner's delegation; see docs/owner-review.md)
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
