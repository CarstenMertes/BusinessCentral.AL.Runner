# ADR-033: Symbolic IR and the link step

- Status: Accepted (2026-10-15, decided by the orchestrator on the owner's delegation; see docs/owner-review.md)
- Date: 2026-10-15
- Related: ADR-011 decision 3 (link, never built), ADR-022 (compile cache, findings 1 to 3 of section 7), ADR-026
  (incremental compile analysis), ADR-029 (incremental compile of code changes), ADR-032 (typed ids), review
  `docs/reviews/2026-10-compile-ir-review.md` (R1, R2, R10)

## Context

ADR-011 decision 3 says: "Link turns object references into indices into the Engine's object tables". No link step
exists. Lowering writes positions directly:

- objects as positions in `Program::objects` (`Expr::Call::object`, `RtType::Record(u32, _)` and about 35 other
  places, ADR-022 finding 1);
- routines as positions in `Object::routines`;
- fields as positions in the merged field list, whose order depends on which extensions are compiled
  (`Place::Field(_, usize)`, `Const::FieldPos`, keys, relations, page bindings; ADR-022 finding 2);
- page controls as positions in the merged control list;
- other apps' contributions merged into an object's IR (extension fields, controls, triggers, implementers,
  enum values; ADR-022 finding 3).

What this already cost: one cache entry per dependency set with workspace apps ordered last; `relower_set`;
`Routine::shift_positions`; ADR-029 refusing every declaration change; ADR-026 judging per-object incremental
compile too costly; four compile paths (eager, demand, cached, incremental) with a differential tool each.

AL has stable identities: an object is `(kind, id)` or, for kinds without an id, `(kind, qualified name)`; a field is
`(table id, field number)`, unique including extensions; an enum value is its ordinal; a control has a name unique in
its page including extensions.

## Decision

1. **Two forms.** Lowering produces a **symbolic IR**: every reference to another object, routine, field, control or
   enum value is a stable key. A **link** step turns a set of symbolic objects into today's dense `al_ir::Program`.
   The VM keeps reading the dense `Program` (ADR-034 later gives it its own executable form).
2. **Keys.**
   - `ObjKey = (ObjKind, ObjName)`, where `ObjName` is the object id or, for kinds without an id, the qualified
     name in lower case. (Owner decision D1 in the plan: id or name for objects that have both.)
   - `RoutineKey = (ObjKey, routine name in lower case, overload ordinal among same-named routines in declaration
     order)`; triggers by trigger name and owning section (field number, control name). (Owner decision D2.)
   - `FieldKey = (table ObjKey, field number)`. `ControlKey = (page ObjKey, control name)`. Enum values by ordinal.
3. **Per-object purity.** A symbolic object holds only what its own source and the declarations it reads give it.
   Contributions of other objects (extension fields, controls, triggers, implementers, enum values, subscribers) are
   not merged into it; the link step merges them, in the order today's lowering uses ("own fields, system fields,
   extension fields in workspace object order"; extension controls by their anchors).
4. **What a cache block holds.** A symbolic object plus the list of declarations it read (object keys and member
   keys). A block is valid while its own source hash and the declarations it read are unchanged. This replaces
   `embeds` and `relower_set`.
5. **Where link lives.** A new crate `al-link` (core crate, no OS access, ADR-006), between `al-sema` and `al-vm`.

## What changes at which seam

- `al-sema -> al-ir`: lowering emits `al_ir::sym::*` types (new module of `al-ir`); `al_ir::Program` stays as the
  linked form.
- `al-cache`: blocks hold symbolic objects. Format bump. Amends ADR-022: per-app entries (its design B) become
  possible without a remap; whether to switch is owner decision D3.
- `al-sema` incremental (ADR-029): an update re-lowers the changed objects and re-links. Declaration changes become
  possible: the objects whose read list names a changed declaration are re-lowered.
- `al-vm`: none in this ADR.

## Proof of zero behaviour change

The link step must produce **exactly today's `Program`**. That is a pure equality, so zero differences is exact:

- A test helper `assert_same_program(old, linked)` (structural equality of `al_ir::Program`, with a readable first
  difference). It runs in unit tests and behind `AL_CHECK_LINK=1` in `al-test-cli` and `alrunner`.
- Equality on: the corpus apps (every major), Tests-ERM, Tests-SCM, NP Retail (scratch copy, ADR-026), the eager
  compile of the Microsoft apps for BC 27, 28 and 29.
- After the cache switches to symbolic blocks: `tools/cache_vs_uncached.sh` and `tools/demand_vs_eager.sh` 0
  differences; `ALRUNNER_CHECK_INCREMENTAL=1` 0 differences on the ADR-029 edit sets.
- Corpus and bucket outcomes identical (`tools/compare_junit.py`).

## Migration (each step merges green)

1. `al_ir::sym` types and the `al-link` crate with `link()`; lowering fills the symbolic keys **next to** the
   positions it writes today. `AL_CHECK_LINK=1` compares. Nothing reads the symbolic form yet.
2. Move one contribution at a time out of the object and into link: extension fields, then controls, then enum
   values, then implementers and interface targets, then subscribers. After each: equality on every input above.
3. Lowering stops writing positions; `link()` is the only producer of `Program`. Delete `shift_positions`.
4. Cache format: symbolic blocks plus read lists. Delete `relower_set` and `embeds`. Cache format bump.
5. Incremental compile (ADR-029) accepts declaration changes, by read lists. Differential on the ADR-029 edit sets
   plus declaration edits (a new procedure, a new field in a table extension, a new object).
6. Demand-driven compile only decides which objects to lower; eager and cached share one path (review R10).

## Consequences

- Unblocks: ADR-034 (an executable form built by link), per-app cache entries (D3), incremental compile of
  declaration changes, reuse of dependency blocks across workspaces, fewer compile paths.
- Blocks: feature work in `lower.rs` conflicts with steps 1 to 3. Do them during the pause.
- Cost: 10 to 15 agent-days (review R1 and R2).
- Risk: wrong field or object after link. The equality proof covers every input that exists; the corpus covers
  behaviour.

## Conformance

`assert_same_program` in tests; `AL_CHECK_LINK=1` in CI on the corpus and one bucket; the existing differential
tools.
