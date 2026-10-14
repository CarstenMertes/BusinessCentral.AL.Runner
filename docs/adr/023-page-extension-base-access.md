# ADR-023: Page extension code that uses the base page's variables and procedures

- Status: Proposed
- Date: 2026-10-14
- Changes: how the lowering names a variable or procedure of the base page in extension code, and one field of `Instance`. Additive in the IR (`Place::BaseGlobal`, `Expr::BaseInstance`); no seam is removed. It needs the owner's decision because it adds to the IR and to `Instance` (CLAUDE.md rule 5).

## Context

AL lets a page extension use what its base page marks for it: a `protected var` variable and a
procedure that is not `local`. The runtime refuses it. Extension code lowers on the extension's own
object, whose globals and procedures are its own, so a name of the base page is "not assignable" or
unresolved. `Lowerer::base_page_member` (`al-sema/src/lower.rs`) detects the case and gives it a
reason: ``\`X\` is a variable or procedure of the page it extends: page extension code on base page
members is not lowered yet``.

**How much it costs.** On BC 27.5 build 46862.56320, the 32 Microsoft test buckets together
(`alrunner test`, no test data, 40326 tests; recipe in `docs/alrunner-buckets.md`) have **2962 tests**
that stop at this reason. Three names account for all of them: `Item` (2931), `ServiceOrderNoVisible` (19),
`EnableOpenRelatedEntity` (12). One routine is the first stop of almost every one: the control
source expression `__source_1112` of `Mfg. ItemAvailByLocationLines`, a page extension in Base
Application whose field names `Item."Qty. on Component Lines"`, where `Item` is a `protected var
Item: Record Item` of the page `Item Availability by Location Lines`. Any test that opens that page
(or a page that hosts it as a part) stops there, Tests-SCM first (about 335 tests of that bucket).

**What exists today.**
- A page extension runs on an instance of its own (`Session::page_instance`, `al-vm/src/pages.rs`),
  with the globals of the extension. It shares three slots with the page's instance: `Rec`, `xRec`
  and `CurrPage` (they hold the same `Rc` handle).
- A *control* of an extension that names a base variable already works: `Binding::BaseVar(slot)`
  reads and writes the slot of the base page's instance, "whoever owns the control" (`al-ir`). It
  was built for report request pages (`handlers/TestRequestPageControlErrorName.al`, documented in
  `docs/probes-needed.md` under "Request pages"). Only code is missing: statements and expressions
  in extension triggers and procedures, and control source expressions that are not a bare name
  (`Item."Qty. on Component Lines"`, `Format(BaseVar)`).
- The checker (`al-sema/src/check.rs`, `ObjKind::PageExtension` scope) puts `Rec` and `CurrPage`
  into the extension's scope and nothing of the base page's `var` section or procedures.

## What the corpus pins

Everything about the *extension's own* instance, and nothing about base access:

| Pin | Where |
|---|---|
| An extension's global lives on the extension's instance: assigned in the extension's `OnOpenPage`, read by a control it declares, rewritten by that control's `OnValidate` | `pageextensioncontrol/TestPageExtensionControlTrapped_Page.al:63-82`, `TestPageExtensionControlBinding_Page.al:56-72` |
| Extension code uses `Rec` (the page's record) for reads and writes, and its own local procedures | `handlers/TestPageExtensionActionInvoke_Page.al:70-91`, `pageextensiontrigger/TestPageExtensionPageTriggers_Page.al:35-60`, `pageextensiontrigger/TestModifiedControlValidateTriggers_Page.al:77-150`, `codeunit/TestCodeCoverageTableExtension.al:71-85` |
| An extension procedure publishes its own event | `extensionevents/ALTExtEvtFixtures.al:88-96` |
| A control of a reportextension that names a `protected var` of the base report binds to the base report's variable (read, write, parameters text); a bare name that is no variable of the base is refused | `handlers/TestRequestPageControlErrorName.al` (Ext5803Handler, cases 21-24) |
| Page and extension triggers run base first, then the extension (assumed for pages, pinned for reports) | `docs/probes-needed.md` "Page extensions" |

No file in the corpus contains `protected var`, and none has a page extension that calls a
procedure of its base page or reads a base variable from code. So the behavior below is **not
pinned**; it is what the AL language reference states and what Base Application's own source relies
on, and it needs probes on real BC before it is built (section "Probes").

## Options

**A. One instance for the page and its extensions.** The extension's globals are appended to the
page's global slots, and extension routines run on the page's `Instance`. Base variables are plain
slots of the same instance. Rejected: slot numbers of an extension then depend on every other
extension of the page that is loaded in the run (they are appended in application order), so the
lowering of one extension would depend on the set of apps, and the compile cache (ADR-022) could
no longer keep a per-object IR. It also changes what `Binding::Var(slot)` and `owner` mean for every
page that has extensions today (the 44 passing extension tests in `pageextension*`).

**B. Keep one instance per extension; give it a link to the base instance (recommended).** The
extension's own instance, globals and lowering stay as they are. Code that names a base variable
lowers to a new place that reads the base instance's slot; a call to a base procedure runs on the
base instance. This is the model `Binding::BaseVar` already uses for controls, extended from
controls to code.

**C. Copy base variables into the extension on each call and write them back.** Rejected: two
extensions of one page, a base trigger and the extension would each work on copies; a value changed
by a nested call is lost or overwritten. It is also slow for record and list variables.

## Decision (proposed): option B

1. **Names.** In a page extension, a name that the extension does not declare resolves, in this
   order, to: the extension's own variable or procedure; then a `protected var` variable of the base
   page; then a procedure of the base page that is not `local`; then the builtins (the order AL
   uses; to be confirmed by probe 6). A name of a `local` member of the base, or one that is
   declared in both, is refused with a reason that says so. The checker puts the base page's
   `protected` variables and its non-local procedures into the extension's scope with their types.
2. **IR (additive).**
   - `Place::BaseGlobal(usize)`: the global slot `n` of the *base page's instance*. It is
     valid only in routines of a page extension; the lowering never emits it elsewhere. Reading and
     writing work as for `Place::Global`, including `Index` and `Field` projections, record
     variables (ADR-021 `TableGlobals` live on the shared `RecordState`, so nothing changes) and
     `var` arguments.
   - `Expr::BaseInstance`: evaluates to the base page's instance. `Expr::Call { instance:
     Some(BaseInstance), object: <base page>, routine }` calls a base procedure on it.
   The slot number of a base variable is its position in the base object's globals, the number
   `Binding::BaseVar` uses today, computed from the base's syntax (declaration order), not from
   the base's lowered object, so extension lowering stays independent of the base's lowering
   (they run on different threads).
3. **VM.** `Instance` gets `base: Option<Weak<RefCell<Instance>>>` (`None` for everything that is
   not a page extension's instance). `page_instance` sets it when it builds each extension's
   instance, next to the `Rec`, `xRec` and `CurrPage` sharing it does today, and `adopt_instance`
   keeps it. `Place::BaseGlobal` and `Expr::BaseInstance` read it from the running frame's
   instance; a missing or dropped base is an internal error with a readable text, never a
   panic. A `Weak` link adds no `Rc` cycle (the page instance already points to its `PageState` through
   `CurrPage`, which points back to the extension instances).
4. **Demand-driven compile.** A reference to a base variable or procedure records a call edge to
   the base routine (for a procedure) and keeps the base page at the level the extension is
   used at (`Demand::activate` already raises an extended page's extensions and an extension's
   target at one level). Nothing else in demand changes. This is the part most likely to need
   care: the 2962 tests above reach the base through a *dependency* app.
5. **Refusals stay explicit.** Refused, each with its own reason: access to a `local` member; a
   name declared by two extensions of the same page (not a base member); a base procedure that is a
   `[TryFunction]`, an event publisher or has `by var` record parameters of a type the call path
   refuses today (same reasons as a call within one object); `this` in an extension; a table or
   report extension (a tableextension has the same problem for the table's `protected var`; see
   "Out of scope"); anything probes 1 to 8 do not confirm.
6. **Triggers.** No change: base triggers run first and then the extension's, as now. A base
   variable that a base trigger set is what the extension's trigger reads.

## Consequences

- Up to 2962 tests of Microsoft's buckets (without test data) can run past this reason. How many
  pass is another question: most then meet the next refusal (`Report.ObjectId`, `Format of a Record`,
  the `DocumentEntries` reportextensions; `docs/alrunner-buckets.md`).
- An extension's lowering depends on the base page's *syntax* (names, modifiers, declaration order)
  but not on its lowering. The compile cache (ADR-022) must hash the base page's source into the
  extension's key, which it already needs for `Binding::BaseVar` and for control merging.
- `Place` and `Expr` each get one variant. Code that matches them exhaustively (the checker of the
  call graph in `al-testrun/src/baseline.rs` `tables_written`, the lowering tests, the VM's `read`
  and `write`) gets one more arm; the compiler finds each.
- Cost when unused: one `Option<Weak>` per `Instance`; no check on the hot path of any other place.

## Probes (real BC; run on the main machine, not by an agent)

A corpus test file per row, in the style of `tableglobals/` (ADR-021): one claim per test, cited
by the implementation.

1. A page extension reads a base `protected var` (Integer, Text, Boolean, Record) that the base's `OnOpenPage` set.
2. A page extension writes a base `protected var`; the base's `OnAfterGetRecord` (or a base action) reads the new value.
3. Two extensions of one page: one writes, the other reads; and both read a value the base sets.
4. A page extension calls a non-local base procedure with a return value, with a `var` parameter, and one that changes a base variable.
5. A base `protected var` of type Record: the extension sets filters through it; the base sees them. Same for a `List` and a `Dictionary` (reference types).
6. Name resolution: an extension variable with the name of a `protected var` of the base (AL0 error or shadowing?), and an extension procedure with the name of a base procedure.
7. The state before the page runs: an extension's `OnInit` reads a base variable the base's `OnInit` has (or has not) set; which `OnInit` runs first.
8. `protected procedure` of the base: callable from the extension? And a base `local` variable: the compile error text (to refuse with the same text).
9. A control source expression that names a base variable (`Item."Qty. on Component Lines"`, `Format(BaseVar)`), through a TestPage, for a field of the base page's record and for a field that is not.
10. A base procedure called from an extension that runs a base trigger and reads `CurrPage` inside it.

## Decisions needed

1. **Accept option B** (instance link, `Place::BaseGlobal`, `Expr::BaseInstance`), or ask for A (one merged instance) with the cache cost described above.
2. **Probes first, or build first.** Everything in "Decision" except the refusals is unpinned. Build first, record what is assumed in `docs/probes-needed.md` and refuse nothing the language reference allows; or probe first (about ten tests) and build exactly what they pin.
3. **Scope.** Page extensions only, as here; or in the same step table extensions (`protected var` of a table, ADR-021 already gives the table's globals a lifetime) and report extensions (a reportextension's code reading the base report's `protected var`; controls already work).

## Out of scope

Table extensions and report extensions (above), `Codeunit` access modifiers, and `internal` members across apps.

## Conformance

On acceptance: the probe tests above (corpus branch), the existing `an_extension_naming_a_variable_of_its_base_page_is_refused_with_that_reason` in `al-sema/src/lower.rs` replaced by tests that lower and run the same code, one VM test per decision 3 rule in `al-vm/tests/pageext.rs`, and the 2962-test measurement repeated with the recipe in `docs/alrunner-buckets.md` (report the new refusal reasons).
