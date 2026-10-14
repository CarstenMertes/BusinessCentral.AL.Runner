//! Demand-driven checking of dependency apps (ADR-013 decision 6).
//!
//! Microsoft's apps hold ten thousand objects and a test reaches a few hundred of
//! them. Checking and lowering all of it costs tens of seconds, so the checker starts
//! from the workspace's own code and follows what that code uses:
//!
//! * A routine that is *reached* has its body checked, and the procedures it calls
//!   are reached in turn.
//! * An object is *full* when reached code runs it: a codeunit it calls, a table it
//!   writes to. Everything BC would run implicitly for it is reached too: the triggers
//!   of a table, the subscribers to its events, the codeunits that implement an
//!   interface or enum it is used through.
//! * A table is *read* when reached code only reads it (a record variable, `Get`,
//!   `FindSet`, field access, `CalcFields`). Reading fires no trigger and no event,
//!   so nothing runs implicitly; the tables it relates to are lowered with it.
//! * An object is *schema* when only the shape of another object depends on it (a
//!   `TableRelation` target, a FlowField's source table, a page named by
//!   `Page::"X"`): it is lowered, its routines are not reached.
//!
//! Anything the analysis does not reach is not compiled. The workspace then stops
//! resolving it (`Workspace::restrict_to`), so code that needs it after all is
//! refused with "outside the workspace" instead of running half-present (hard rule 4).

use super::*;
use crate::workspace::Resolution;
use std::collections::HashSet;
use std::rc::Rc;

/// Base Application's `Company-Initialize`, which a new company runs once.
pub const COMPANY_INITIALIZE: u32 = 2;

/// An object nothing in the compile refers to.
pub const NONE: u8 = 0;
/// Lowered, with its routines stubbed.
pub const SCHEMA: u8 = 1;
/// A table reached code reads.
pub const READ: u8 = 2;
/// Run by reached code.
pub const FULL: u8 = 3;

/// What the workspace's code reaches in its dependency apps.
#[derive(Debug, Clone, Default)]
pub struct Demand {
    /// Per object: `NONE`, `SCHEMA`, `READ` or `FULL`. Objects of workspace apps are not tracked.
    pub level: Vec<u8>,
    /// The routines of dependency objects whose bodies were checked: (object, routine offset).
    pub reached: HashSet<(usize, usize)>,
}

impl Demand {
    /// Whether dependency object `i` is part of the compile at all.
    pub fn active(&self, i: usize) -> bool {
        self.level[i] != NONE
    }

    pub fn reached(&self, object: usize, offset: usize) -> bool {
        self.reached.contains(&(object, offset))
    }
}

pub(super) struct State {
    level: Vec<u8>,
    reached: HashSet<(usize, usize)>,
    queue: Vec<(usize, usize)>,
    /// Objects whose level just rose, with the new level.
    fresh: Vec<(usize, u8)>,
    /// Per table: the write operations (`INSERT`, ...) reached code performs on it.
    ops: Vec<u8>,
    /// Tables with a write operation not processed yet.
    fresh_ops: Vec<(usize, u8)>,
    /// The write operations (any table, any app) whose `Global Triggers` subscribers are reached.
    global_ops: u8,
    /// Write operations not yet processed for `Global Triggers`.
    fresh_global: u8,
    ctxs: HashMap<usize, Rc<ObjCtx>>,
    routines: HashMap<usize, Rc<Vec<ast::Routine>>>,
}

impl State {
    pub(super) fn new(objects: usize) -> State {
        State {
            level: vec![NONE; objects],
            reached: HashSet::new(),
            queue: Vec::new(),
            fresh: Vec::new(),
            ops: vec![0; objects],
            fresh_ops: Vec::new(),
            global_ops: 0,
            fresh_global: 0,
            ctxs: HashMap::new(),
            routines: HashMap::new(),
        }
    }
}

/// Write operations on a table, each of which fires its own triggers and events.
pub const INSERT: u8 = 1;
pub const MODIFY: u8 = 2;
pub const DELETE: u8 = 4;
pub const RENAME: u8 = 8;
pub const VALIDATE: u8 = 16;
pub const ALL_OPS: u8 = INSERT | MODIFY | DELETE | RENAME | VALIDATE;

/// The table triggers an operation runs, and the events it raises.
const OP_TRIGGERS: [(u8, &str, [&str; 2]); 5] = [
    (INSERT, "OnInsert", ["OnBeforeInsertEvent", "OnAfterInsertEvent"]),
    (MODIFY, "OnModify", ["OnBeforeModifyEvent", "OnAfterModifyEvent"]),
    (DELETE, "OnDelete", ["OnBeforeDeleteEvent", "OnAfterDeleteEvent"]),
    (RENAME, "OnRename", ["OnBeforeRenameEvent", "OnAfterRenameEvent"]),
    (
        VALIDATE,
        "OnValidate",
        ["OnBeforeValidateEvent", "OnAfterValidateEvent"],
    ),
];

/// The operations a Record method performs, if it writes.
pub(super) fn record_ops(method: &str) -> u8 {
    match method.to_ascii_lowercase().as_str() {
        "insert" => INSERT,
        "modify" => MODIFY,
        "delete" | "deleteall" => DELETE,
        "rename" => RENAME,
        "validate" => VALIDATE,
        "modifyall" => MODIFY | VALIDATE,
        _ => 0,
    }
}

const EVENT_ATTRS: [&str; 3] = ["IntegrationEvent", "BusinessEvent", "InternalEvent"];

impl Checker<'_> {
    /// Raises dependency object `i` to `level`.
    fn raise(&self, i: usize, level: u8) {
        if !self.ws.is_dependency(i) {
            return;
        }
        if let Some(st) = self.demand.borrow_mut().as_mut() {
            if st.level[i] == NONE {
                if let Some(tx) = &self.prefetch {
                    let _ = tx.send(self.ws.objects[i].file);
                }
            }
            if st.level[i] < level {
                st.level[i] = level;
                st.fresh.push((i, level));
            }
        }
    }

    /// Reached code uses object `i` the way its kind is usually used: a table is read, a
    /// codeunit, enum, interface or query runs, a page or report is only named.
    pub(super) fn touch(&self, i: usize) {
        let level = match self.ws.objects[i].kind {
            ObjKind::Table => READ,
            ObjKind::Codeunit | ObjKind::Enum | ObjKind::EnumExtension | ObjKind::Interface | ObjKind::Query => FULL,
            _ => SCHEMA,
        };
        self.raise(i, level);
    }

    /// Reached code runs object `i`: drives a page through the test client.
    pub(super) fn touch_full(&self, i: usize) {
        self.raise(i, FULL);
    }

    /// Reached code writes to table `i` (`ops`): that runs its triggers and the subscribers
    /// to its events for those operations.
    pub(super) fn touch_ops(&self, i: usize, ops: u8) {
        if ops == 0 {
            return;
        }
        // Any write, to a table of any app, raises the events of the platform's `Global Triggers`
        // codeunit, whose subscribers can sit in a dependency app.
        if let Some(st) = self.demand.borrow_mut().as_mut() {
            let new = ops & !st.global_ops;
            if new != 0 {
                st.global_ops |= new;
                st.fresh_global |= new;
            }
        }
        if !self.ws.is_dependency(i) {
            return;
        }
        self.raise(i, FULL);
        if let Some(st) = self.demand.borrow_mut().as_mut() {
            let new = ops & !st.ops[i];
            if new != 0 {
                st.ops[i] |= new;
                st.fresh_ops.push((i, new));
            }
        }
    }

    /// The shape of reached code's objects depends on `i`.
    pub(super) fn touch_schema(&self, i: usize) {
        self.raise(i, SCHEMA);
    }

    /// Every object a variable of type `t` can reach: `t` is used by code that runs.
    pub(super) fn touch_ty(&self, file: usize, t: &Ty) {
        if self.demand.borrow().is_none() {
            return;
        }
        match t {
            Ty::Record { table, .. } | Ty::TestFilter(table) => {
                self.resolve_at(file, ObjKind::Table, table);
            }
            Ty::Object(kind, name) => {
                self.resolve_at(file, *kind, name);
            }
            Ty::Enum(name) => {
                self.resolve_at(file, ObjKind::Enum, name);
            }
            Ty::TestPage(name) | Ty::TestPart(name) => {
                if let Some(i) = self.resolve_at(file, ObjKind::Page, name) {
                    self.touch_full(i);
                }
            }
            Ty::TestRequestPage(name) => {
                if let Some(i) = self.resolve_at(file, ObjKind::Report, name) {
                    self.touch_full(i);
                }
            }
            Ty::List(t) | Ty::Array(_, t) => self.touch_ty(file, t),
            Ty::Dictionary(k, v) => {
                self.touch_ty(file, k);
                self.touch_ty(file, v);
            }
            _ => {}
        }
    }

    /// Queues routine `offset` of object `object` to be checked, if it is a dependency's.
    fn reach(&self, object: usize, offset: usize) {
        if !self.ws.is_dependency(object) {
            return;
        }
        self.touch(object);
        if let Some(st) = self.demand.borrow_mut().as_mut() {
            if st.reached.insert((object, offset)) {
                st.queue.push((object, offset));
            }
        }
    }

    /// The install baseline's roots (ADR-013 decision 5): every install codeunit of a dependency
    /// app, with its install triggers, and Base Application's `Company-Initialize`.
    pub(super) fn reach_baseline(&self) {
        for i in 0..self.ws.objects.len() {
            let o = &self.ws.objects[i];
            if !self.ws.is_dependency(i) || o.kind != ObjKind::Codeunit || !self.ws.usable(i) {
                continue;
            }
            if o.install {
                self.touch_full(i);
                for r in self.routines_of(i).iter() {
                    if r.is_trigger()
                        && (r.name().eq_ignore_ascii_case("OnInstallAppPerCompany")
                            || r.name().eq_ignore_ascii_case("OnInstallAppPerDatabase"))
                    {
                        self.reach(i, usize::from(r.syntax().text_range().start()));
                    }
                }
            } else if o.id == Some(COMPANY_INITIALIZE) && o.name.eq_ignore_ascii_case("Company-Initialize") {
                self.reach_run(i);
            }
        }
    }

    /// Reaches the procedures the routine just checked calls.
    pub(super) fn take_calls(&mut self) {
        let calls = std::mem::take(&mut self.calls);
        if self.demand.borrow().is_some() {
            for (object, offset) in calls {
                self.reach(object, offset);
            }
        }
    }

    /// `Codeunit.Run` on object `i`, or `Codeunit::"X"` written where it can be run: its
    /// `OnRun` trigger runs.
    pub(super) fn reach_run(&self, i: usize) {
        if self.demand.borrow().is_none() || !self.ws.is_dependency(i) || !self.ws.usable(i) {
            return;
        }
        self.touch_full(i);
        for r in self.routines_of(i).iter() {
            if r.is_trigger() && r.name().eq_ignore_ascii_case("OnRun") {
                self.reach(i, usize::from(r.syntax().text_range().start()));
            }
        }
    }

    fn routines_of(&self, i: usize) -> Rc<Vec<ast::Routine>> {
        if let Some(st) = self.demand.borrow().as_ref() {
            if let Some(r) = st.routines.get(&i) {
                return r.clone();
            }
        }
        let routines = Rc::new(ast::routines_in(self.ws.object_ast(i).syntax()));
        if let Some(st) = self.demand.borrow_mut().as_mut() {
            st.routines.insert(i, routines.clone());
        }
        routines
    }

    fn reach_all(&self, i: usize) {
        for r in self.routines_of(i).iter() {
            self.reach(i, usize::from(r.syntax().text_range().start()));
        }
    }

    fn reach_triggers(&self, i: usize) {
        for r in self.routines_of(i).iter() {
            if r.is_trigger() {
                self.reach(i, usize::from(r.syntax().text_range().start()));
            }
        }
    }

    /// Reaches the `k`-th event subscriber routine of object `o`.
    fn reach_subscriber(&self, o: usize, k: usize) {
        if !self.ws.usable(o) {
            return;
        }
        self.touch(o);
        let routines = self.routines_of(o);
        let routine = routines
            .iter()
            .filter(|r| r.attributes().any(|a| a.name().eq_ignore_ascii_case("EventSubscriber")))
            .nth(k);
        if let Some(r) = routine {
            self.reach(o, usize::from(r.syntax().text_range().start()));
        }
    }

    /// What using object `i` at `level` sets off.
    fn activate(&mut self, i: usize, level: u8) {
        if !self.ws.usable(i) {
            return;
        }
        let kind = self.ws.objects[i].kind;
        // An extension in use needs what it extends; an extended object needs its extensions
        // (their fields, enum values and controls are part of it).
        if let Some(t) = self.ws.extension_target(i) {
            self.raise(t, level);
        }
        for &e in self.ws.extensions_of(i) {
            self.raise(e, level);
        }
        match kind {
            ObjKind::Table => self.activate_table(i, level),
            ObjKind::TableExtension => {}
            // The test client drives a page: any of its routines can run, and so can the
            // operations on its table, so all of them are reached (the call graph the lowering
            // builds for a TestPage assumes the same).
            ObjKind::Page | ObjKind::PageExtension => {
                self.activate_page_shape(i);
                if level == FULL {
                    self.reach_all(i);
                    self.activate_source_table(i);
                }
            }
            ObjKind::Report | ObjKind::ReportExtension | ObjKind::XmlPort if level == FULL => {
                self.reach_triggers(i);
                self.activate_source_table(i);
            }
            ObjKind::Query => {
                if level == FULL {
                    self.reach_triggers(i);
                }
            }
            ObjKind::Enum | ObjKind::EnumExtension => {
                if level == FULL {
                    self.activate_enum(i);
                }
            }
            ObjKind::Interface if level == FULL => self.activate_interface(i),
            _ => {}
        }
    }

    fn activate_table(&mut self, t: usize, level: u8) {
        let file = self.ws.objects[t].file;
        let fields = self.members[t].fields.clone();
        for f in &fields {
            let decl_file = f.declared_in.map_or(file, |d| self.ws.objects[d].file);
            // A field of an enum type lowers to the enum; without it the field would go untyped.
            if let Ty::Enum(name) = &f.ty {
                self.resolve_at(decl_file, ObjKind::Enum, name);
            }
            if level < READ {
                continue;
            }
            // Validating and calculating read the tables a field relates to.
            if let Some((_, text)) = f
                .properties
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("TableRelation"))
            {
                for arm in crate::relation::parse(text).unwrap_or_default() {
                    for n in [arm.path.len(), arm.path.len().saturating_sub(1)] {
                        if n == 0 {
                            continue;
                        }
                        if let Resolution::Found(r) =
                            self.ws
                                .resolve_relation_target(decl_file, ObjKind::Table, &arm.path[..n])
                        {
                            self.touch_schema(r);
                            break;
                        }
                    }
                }
            }
            if let Some(text) = &f.calc_formula {
                if let Ok(formula) = crate::calcformula::parse(text) {
                    if let Resolution::Found(r) =
                        self.ws
                            .resolve_ref(decl_file, ObjKind::Table, std::slice::from_ref(&formula.table))
                    {
                        self.touch_schema(r);
                    }
                }
            }
        }
        // The table's events can fire whatever reached code does (a rename reaches related
        // tables, a RecordRef writes without a typed variable). Subscribers not reached
        // by an operation are lowered as stubs, so that if one does fire it refuses.
        for (o, _) in self.ws.subscribers_of(t) {
            self.touch_schema(o);
        }
    }

    /// Table `t` is written to (`ops`): reach the triggers and the subscribers those operations run.
    fn activate_ops(&mut self, t: usize, ops: u8) {
        if !self.ws.usable(t) {
            return;
        }
        for (op, trigger, events) in OP_TRIGGERS {
            if ops & op == 0 {
                continue;
            }
            for o in std::iter::once(t).chain(self.ws.extensions_of(t).iter().copied()) {
                if !self.ws.usable(o) {
                    continue;
                }
                self.touch_ops_object(o);
                for r in self.routines_of(o).iter() {
                    // A table extension's `modify(Field)` triggers bracket a field's OnValidate.
                    let ext_field_trigger = op == VALIDATE
                        && (r.name().eq_ignore_ascii_case("OnBeforeValidate")
                            || r.name().eq_ignore_ascii_case("OnAfterValidate"));
                    if r.is_trigger() && (r.name().eq_ignore_ascii_case(trigger) || ext_field_trigger) {
                        self.reach(o, usize::from(r.syntax().text_range().start()));
                    }
                }
            }
            for (o, k) in self.ws.subscribers_of(t) {
                let event = &self.ws.objects[o].subscribers[k].event;
                if events.iter().any(|e| event.eq_ignore_ascii_case(e)) {
                    self.reach_subscriber(o, k);
                }
            }
        }
    }

    /// Writes (`ops`) raise the events of the platform codeunit `Global Triggers`: reach the
    /// subscribers in dependency apps. The setup events are asked for every table; the others
    /// are those the runtime raises for the operation (the global events of Insert, Modify and
    /// Delete are not published by BC, record/TestTriggerDispatchOrder.al).
    fn activate_global(&mut self, ops: u8) {
        let mut events: Vec<&str> = vec!["getglobaltabletriggermask", "getdatabasetabletriggersetup"];
        for (op, names) in [
            (INSERT, &["ondatabaseinsert"][..]),
            (MODIFY, &["ondatabasemodify"][..]),
            (DELETE, &["ondatabasedelete"][..]),
            (RENAME, &["onglobalrename", "ondatabaserename"][..]),
        ] {
            if ops & op != 0 {
                events.extend(names.iter().copied());
            }
        }
        for (o, k) in self.ws.global_trigger_subscribers() {
            let event = &self.ws.objects[o].subscribers[k].event;
            if events.iter().any(|e| event.eq_ignore_ascii_case(e)) {
                self.reach_subscriber(o, k);
            }
        }
    }

    /// An extension whose triggers run is used.
    fn touch_ops_object(&self, o: usize) {
        self.raise(o, FULL);
    }

    /// A page lowers with its source table and the pages it shows as parts; without them it
    /// would be refused as pointing outside the workspace.
    fn activate_page_shape(&mut self, i: usize) {
        let obj = self.ws.object_ast(i);
        let file = self.ws.objects[i].file;
        let source = obj
            .properties()
            .find(|p| p.name().eq_ignore_ascii_case("SourceTable"))
            .map(|p| p.value_text().trim().trim_matches('"').to_string());
        if let Some(t) = source.and_then(|name| self.resolve_quiet(file, ObjKind::Table, &name)) {
            self.touch_schema(t);
        }
        for sec in ast::sections_in(obj.syntax()) {
            if sec.name().eq_ignore_ascii_case("part") {
                if let Some(p) = sec
                    .args()
                    .get(1)
                    .and_then(|n| self.resolve_quiet(file, ObjKind::Page, n))
                {
                    self.touch_schema(p);
                }
            }
        }
    }

    /// A page, report or XML port runs against the table it is built on.
    fn activate_source_table(&mut self, i: usize) {
        let obj = self.ws.object_ast(i);
        let file = self.ws.objects[i].file;
        let source = obj
            .properties()
            .find(|p| p.name().eq_ignore_ascii_case("SourceTable"))
            .map(|p| p.value_text().trim().trim_matches('"').to_string());
        if let Some(name) = source {
            if let Some(t) = self.resolve_at(file, ObjKind::Table, &name) {
                if matches!(self.ws.objects[i].kind, ObjKind::Page | ObjKind::PageExtension) {
                    self.touch_ops(t, ALL_OPS);
                }
            }
        }
        for sec in ast::sections_in(obj.syntax()) {
            let n = sec.name().to_ascii_lowercase();
            if matches!(n.as_str(), "dataitem" | "tableelement") {
                if let Some(table) = sec.args().get(1) {
                    self.resolve_at(file, ObjKind::Table, table);
                }
            }
            // A part opens with its host.
            if n == "part" {
                if let Some(page) = sec.args().get(1) {
                    if let Some(p) = self.resolve_at(file, ObjKind::Page, page) {
                        self.touch_full(p);
                    }
                }
            }
        }
    }

    /// An enum used through an interface runs the codeunits its values implement.
    fn activate_enum(&mut self, e: usize) {
        let impls = self.members[e].enum_impls.clone();
        for im in impls {
            let file = self.ws.objects[im.declared_in].file;
            let iface = self.ws.resolve_ref(file, ObjKind::Interface, &im.interface);
            let codeunit = self.ws.resolve_ref(file, ObjKind::Codeunit, &im.codeunit);
            if let (Resolution::Found(iface), Resolution::Found(cu)) = (iface, codeunit) {
                self.touch(iface);
                self.touch(cu);
                self.reach_implementation(iface, cu);
            }
        }
    }

    /// Every codeunit that implements interface `iface` is a candidate for its calls.
    fn activate_interface(&mut self, iface: usize) {
        let name = self.ws.objects[iface].name.clone();
        let candidates: Vec<usize> = self.ws.implementers_of(&name).to_vec();
        for cu in candidates {
            let o = &self.ws.objects[cu];
            let implements_it = o.implements.iter().any(|parts| {
                matches!(self.ws.resolve_ref(o.file, ObjKind::Interface, parts), Resolution::Found(i) if i == iface)
            });
            if implements_it && o.kind == ObjKind::Codeunit {
                self.touch(cu);
                self.reach_implementation(iface, cu);
            }
        }
    }

    /// Reaches the procedures of `cu` that implement procedures of `iface`.
    fn reach_implementation(&self, iface: usize, cu: usize) {
        if !self.ws.usable(cu) {
            return;
        }
        let wanted: Vec<String> = self.members[iface]
            .procedures
            .iter()
            .map(|p| p.name.to_lowercase())
            .collect();
        for r in self.routines_of(cu).iter() {
            if wanted.contains(&r.name().to_lowercase()) {
                self.reach(cu, usize::from(r.syntax().text_range().start()));
            }
        }
    }

    /// Checks one reached routine, and queues what it uses.
    fn check_reached(&mut self, object: usize, offset: usize) {
        if !self.ws.usable(object) {
            return;
        }
        let cached = self.demand.borrow().as_ref().and_then(|s| s.ctxs.get(&object).cloned());
        let ctx = match cached {
            Some(c) => c,
            None => {
                let c = Rc::new(self.object_ctx(object));
                if let Some(st) = self.demand.borrow_mut().as_mut() {
                    st.ctxs.insert(object, c.clone());
                }
                c
            }
        };
        let routines = self.routines_of(object);
        let Some(r) = routines
            .iter()
            .find(|r| usize::from(r.syntax().text_range().start()) == offset)
        else {
            return;
        };
        self.check_routine_in(&ctx, r);
        self.take_calls();
        // A reached event publisher reaches its subscribers.
        let is_publisher = r
            .attributes()
            .any(|a| EVENT_ATTRS.iter().any(|e| a.name().eq_ignore_ascii_case(e)));
        if is_publisher {
            let name = r.name();
            for (o, k) in self.ws.subscribers_of(object) {
                if self.ws.objects[o].subscribers[k].event.eq_ignore_ascii_case(&name) {
                    self.reach_subscriber(o, k);
                }
            }
        }
    }

    /// Runs the demand loop to its end and reports what it reached.
    pub(super) fn run_demand(&mut self) -> Option<Demand> {
        self.demand.borrow().as_ref()?;
        // The stored platform tables are part of every tenant (the Company, the User, the links of
        // a record): compile their shape so the platform can seed and write them.
        for i in 0..self.ws.objects.len() {
            let stored = self.ws.platform_table(i)
                && self.ws.objects[i]
                    .id
                    .and_then(al_builtins::platform::provider)
                    == Some(al_builtins::platform::Provider::Stored);
            if stored && self.ws.usable(i) {
                self.raise(i, READ);
            }
        }
        loop {
            let fresh = self.demand.borrow_mut().as_mut().and_then(|s| s.fresh.pop());
            if let Some((i, level)) = fresh {
                self.activate(i, level);
                continue;
            }
            let op = self.demand.borrow_mut().as_mut().and_then(|s| s.fresh_ops.pop());
            if let Some((t, ops)) = op {
                self.activate_ops(t, ops);
                continue;
            }
            let global = self
                .demand
                .borrow_mut()
                .as_mut()
                .map(|s| std::mem::take(&mut s.fresh_global))
                .unwrap_or(0);
            if global != 0 {
                self.activate_global(global);
                continue;
            }
            let next = self.demand.borrow_mut().as_mut().and_then(|s| s.queue.pop());
            if let Some((o, off)) = next {
                self.check_reached(o, off);
                continue;
            }
            break;
        }
        let st = self.demand.borrow_mut().take()?;
        Some(Demand {
            level: st.level,
            reached: st.reached,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::lower::{Lowered, compile_apps};
    use crate::workspace::{AppKind, AppSource, Workspace};

    /// A workspace app `main` over a dependency app `dep`, compiled with demand-driven dependencies.
    fn compile(main: &str, dep: &[(&str, &str)]) -> (Workspace, Lowered) {
        let app = |id: &str, deps: &[&str], files: Vec<(String, String)>| AppSource {
            id: id.into(),
            name: id.into(),
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
            files,
        };
        compile_apps(
            vec![
                app("main", &["dep"], vec![("main.al".into(), main.into())]),
                app(
                    "dep",
                    &[],
                    dep.iter().map(|(p, s)| (p.to_string(), s.to_string())).collect(),
                ),
            ],
            vec![AppKind::Workspace, AppKind::Dependency],
            &Default::default(),
        )
    }

    fn object(l: &Lowered, name: &str) -> usize {
        l.program
            .objects
            .iter()
            .position(|o| o.name == name)
            .unwrap_or_else(|| panic!("no object {name}"))
    }

    /// The reasons routine `name` of object `obj` cannot run (empty: it runs).
    fn reasons(l: &Lowered, obj: &str, name: &str) -> Vec<String> {
        let o = object(l, obj);
        let r = l.program.objects[o]
            .routines
            .iter()
            .position(|r| r.name == name)
            .unwrap_or_else(|| panic!("no routine {name}"));
        l.unsupported[o][r].clone()
    }

    fn compiled(l: &Lowered, obj: &str, name: &str) -> bool {
        reasons(l, obj, name).iter().all(|r| !r.contains("not compiled"))
    }

    #[test]
    fn only_the_dependency_routines_the_workspace_calls_are_compiled() {
        let (_, l) = compile(
            "codeunit 1 Main { procedure Go() var D: Codeunit Lib; begin D.Used(); end; }",
            &[(
                "lib.al",
                "codeunit 10 Lib { procedure Used() begin Inner(); end; local procedure Inner() begin end; procedure Unused() begin Other(); end; local procedure Other() begin end; }",
            )],
        );
        assert!(compiled(&l, "Lib", "Used"));
        assert!(compiled(&l, "Lib", "Inner"), "what a reached routine calls is reached");
        assert!(!compiled(&l, "Lib", "Unused"));
        assert!(!compiled(&l, "Lib", "Other"));
        // The stub says why it cannot run.
        assert!(reasons(&l, "Lib", "Unused")[0].contains("not compiled"));
    }

    #[test]
    fn an_unsupported_dependency_routine_nothing_reaches_does_not_block_anything() {
        // `Rare` uses a builtin that does not exist; nothing reaches it, so nothing is refused.
        let (_, l) = compile(
            "codeunit 1 Main { procedure Go() var D: Codeunit Lib; begin D.Fine(); end; }",
            &[(
                "lib.al",
                "codeunit 10 Lib { procedure Fine() begin end; procedure Rare() var T: Text; begin T := NoSuchBuiltin(1); end; }",
            )],
        );
        assert!(reasons(&l, "Lib", "Fine").is_empty());
        assert!(!compiled(&l, "Lib", "Rare"));
        assert!(reasons(&l, "Main", "Go").is_empty(), "{:?}", reasons(&l, "Main", "Go"));
    }

    #[test]
    fn a_reached_routine_that_cannot_run_blocks_what_reaches_it() {
        let (_, l) = compile(
            "codeunit 1 Main { procedure Go() var D: Codeunit Lib; begin D.Rare(); end; }",
            &[(
                "lib.al",
                "codeunit 10 Lib { procedure Rare() var T: Text; begin T := NoSuchBuiltin(1); end; }",
            )],
        );
        assert!(compiled(&l, "Lib", "Rare"));
        assert!(!reasons(&l, "Lib", "Rare").is_empty());
        let go = l.program.objects[object(&l, "Main")]
            .routines
            .iter()
            .position(|r| r.name == "Go")
            .unwrap();
        let lib = object(&l, "Lib");
        let rare = l.program.objects[lib]
            .routines
            .iter()
            .position(|r| r.name == "Rare")
            .unwrap();
        assert!(l.calls[object(&l, "Main")][go].contains(&(lib, rare)));
    }

    #[test]
    fn objects_nothing_refers_to_are_not_compiled_and_stay_out_of_reach() {
        let (ws, l) = compile(
            "codeunit 1 Main { procedure Go() var R: Record Used; begin R.FindFirst(); end; }",
            &[
                ("used.al", "table 10 Used { fields { field(1; K; Code[10]) { } } }"),
                (
                    "other.al",
                    "table 11 Other { fields { field(1; K; Code[10]) { } } trigger OnInsert() begin end; }",
                ),
            ],
        );
        let used = &l.program.objects[object(&l, "Used")];
        let other = &l.program.objects[object(&l, "Other")];
        assert!(used.table.is_some());
        assert!(
            other.table.is_none() && other.routines.is_empty(),
            "an object nothing refers to is a shell"
        );
        // The workspace no longer resolves it either: code that needs it after all is refused.
        let fi = ws.files.iter().position(|f| f.path == "main.al").unwrap();
        assert_eq!(
            ws.resolve_ref(fi, crate::workspace::ObjKind::Table, &["Other".to_string()]),
            crate::workspace::Resolution::Missing
        );
    }

    #[test]
    fn reading_a_table_runs_no_trigger_and_writing_runs_the_trigger_of_that_operation() {
        let dep = [(
            "t.al",
            "table 10 T { fields { field(1; K; Code[10]) { } } \
             trigger OnInsert() begin end; trigger OnModify() begin end; trigger OnDelete() begin end; }",
        )];
        let (_, read) = compile(
            "codeunit 1 Main { procedure Go() var R: Record T; begin R.FindFirst(); end; }",
            &dep,
        );
        for t in ["OnInsert", "OnModify", "OnDelete"] {
            assert!(!compiled(&read, "T", t), "{t} must not run when the table is only read");
        }
        let (_, write) = compile(
            "codeunit 1 Main { procedure Go() var R: Record T; begin R.Insert(true); end; }",
            &dep,
        );
        assert!(compiled(&write, "T", "OnInsert"));
        assert!(
            !compiled(&write, "T", "OnModify"),
            "only the operation used runs its trigger"
        );
        assert!(!compiled(&write, "T", "OnDelete"));
    }

    #[test]
    fn writing_a_table_reaches_its_event_subscribers_for_that_operation_only() {
        let dep = [
            ("t.al", "table 10 T { fields { field(1; K; Code[10]) { } } }"),
            (
                "s.al",
                "codeunit 20 Subs {\n\
                 [EventSubscriber(ObjectType::Table, Database::T, 'OnAfterInsertEvent', '', false, false)]\n\
                 local procedure OnIns() begin end;\n\
                 [EventSubscriber(ObjectType::Table, Database::T, 'OnAfterModifyEvent', '', false, false)]\n\
                 local procedure OnMod() begin end; }",
            ),
        ];
        let (_, l) = compile(
            "codeunit 1 Main { procedure Go() var R: Record T; begin R.Insert(); end; }",
            &dep,
        );
        assert!(compiled(&l, "Subs", "OnIns"));
        assert!(!compiled(&l, "Subs", "OnMod"));
        // Reading only: neither subscriber is reached.
        let (_, l) = compile(
            "codeunit 1 Main { procedure Go() var R: Record T; begin R.FindFirst(); end; }",
            &dep,
        );
        assert!(!compiled(&l, "Subs", "OnIns"));
    }

    #[test]
    fn a_reached_event_publisher_reaches_the_subscribers_of_that_event() {
        let dep = [
            (
                "p.al",
                "codeunit 10 Pub { procedure Do() begin OnThing(); OnOther(); end;\n\
                 [IntegrationEvent(false, false)] local procedure OnThing() begin end;\n\
                 [IntegrationEvent(false, false)] local procedure OnOther() begin end; }",
            ),
            (
                "s.al",
                "codeunit 20 Subs {\n\
                 [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnThing', '', false, false)]\n\
                 local procedure A() begin end;\n\
                 [EventSubscriber(ObjectType::Codeunit, Codeunit::Pub, 'OnNever', '', false, false)]\n\
                 local procedure B() begin end; }",
            ),
        ];
        let (_, l) = compile(
            "codeunit 1 Main { procedure Go() var P: Codeunit Pub; begin P.Do(); end; }",
            &dep,
        );
        assert!(compiled(&l, "Subs", "A"));
        assert!(!compiled(&l, "Subs", "B"), "the publisher never raises OnNever");
    }

    #[test]
    fn a_write_anywhere_reaches_the_global_triggers_subscribers_of_that_operation() {
        // `Global Triggers` (2000000002) is a platform codeunit no app declares here; the runtime
        // raises its events around writes, so a dependency's subscribers to them are reached by a
        // write to any table, the workspace's own included.
        let dep = [(
            "s.al",
            "codeunit 20 Subs {\n\
             [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Global Triggers\", 'GetDatabaseTableTriggerSetup', '', false, false)]\n\
             local procedure Setup(TableId: Integer; var OnDatabaseInsert: Boolean; var OnDatabaseModify: Boolean; var OnDatabaseDelete: Boolean; var OnDatabaseRename: Boolean) begin end;\n\
             [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Global Triggers\", 'OnDatabaseInsert', '', false, false)]\n\
             local procedure Ins(RecRef: RecordRef) begin end;\n\
             [EventSubscriber(ObjectType::Codeunit, Codeunit::\"Global Triggers\", 'OnDatabaseModify', '', false, false)]\n\
             local procedure Mod(RecRef: RecordRef) begin end; }",
        )];
        let (_, l) = compile(
            "table 5 Mine { fields { field(1; K; Code[10]) { } } }\n\
             codeunit 1 Main { procedure Go() var R: Record Mine; begin R.Insert(); end; }",
            &dep,
        );
        assert!(compiled(&l, "Subs", "Setup"));
        assert!(compiled(&l, "Subs", "Ins"));
        assert!(
            !compiled(&l, "Subs", "Mod"),
            "only the operations used raise their events"
        );
        let (_, l) = compile(
            "table 5 Mine { fields { field(1; K; Code[10]) { } } }\n\
             codeunit 1 Main { procedure Go() var R: Record Mine; begin R.FindFirst(); end; }",
            &dep,
        );
        assert!(
            l.program.objects[object(&l, "Subs")].routines.is_empty(),
            "reading raises nothing, so the subscribers are not compiled"
        );
    }

    #[test]
    fn an_interface_used_reaches_the_codeunits_that_implement_it() {
        let dep = [
            ("i.al", "interface IThing { procedure Run(); }"),
            (
                "c.al",
                "codeunit 10 Impl implements IThing { procedure Run() begin Helper(); end; local procedure Helper() begin end; procedure Extra() begin end; }",
            ),
        ];
        let (_, l) = compile(
            "codeunit 1 Main { procedure Go() var I: Interface IThing; begin I.Run(); end; }",
            &dep,
        );
        assert!(compiled(&l, "Impl", "Run"));
        assert!(compiled(&l, "Impl", "Helper"));
        assert!(
            !compiled(&l, "Impl", "Extra"),
            "only the interface's procedures are called through it"
        );
    }

    #[test]
    fn a_table_related_to_a_used_one_is_lowered_with_it_but_runs_nothing() {
        let dep = [
            (
                "a.al",
                "table 10 A { fields { field(1; K; Code[10]) { } field(2; Ref; Code[10]) { TableRelation = B.K; } } }",
            ),
            (
                "b.al",
                "table 11 B { fields { field(1; K; Code[10]) { } } trigger OnInsert() begin end; }",
            ),
        ];
        let (_, l) = compile(
            "codeunit 1 Main { procedure Go() var R: Record A; begin R.FindFirst(); end; }",
            &dep,
        );
        let b = &l.program.objects[object(&l, "B")];
        assert!(
            b.table.is_some(),
            "the related table is lowered so the relation resolves"
        );
        assert!(!compiled(&l, "B", "OnInsert"));
        let a = l.program.objects[object(&l, "A")].table.as_ref().unwrap();
        assert!(a.fields[1].relation_arms.iter().all(|arm| arm.table.is_some()));
    }

    #[test]
    fn a_dependency_app_without_a_reference_from_the_workspace_compiles_nothing() {
        let (_, l) = compile(
            "codeunit 1 Main { procedure Go() begin end; }",
            &[
                ("lib.al", "codeunit 10 Lib { procedure P() begin end; }"),
                ("t.al", "table 11 T { }"),
            ],
        );
        assert!(l.program.objects[object(&l, "Lib")].routines.is_empty());
        assert!(l.dependency[object(&l, "Lib")] && !l.dependency[object(&l, "Main")]);
    }
}
