//! Tree-walking interpreter over the typed IR (ADR-011). `Engine` is the
//! immutable program (ADR-001); `Session` holds all mutable state.

use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::value::{Instance, RecordState, RtType, Value};
use al_ir as ir;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;
use std::sync::Arc;

mod autoinc;
mod dispatch;
pub mod events;
mod flowfields;
mod pages;
mod recmeta;
mod records;
mod refs;
mod relations;
mod rename;
mod sessionops;
mod ui;
mod views;
pub use ui::{UiHandler, UiKind};

pub struct Engine {
    pub program: ir::Program,
    /// Per object, per routine: why it cannot run, if it cannot.
    pub blocked: Vec<Vec<Option<String>>>,
    /// Store schema per table object.
    pub tables: Vec<Option<al_data::TableDef>>,
    /// Event publishers and their subscribers, resolved by name.
    pub events: events::Registry,
}

impl Engine {
    pub fn new(program: ir::Program, blocked: Vec<Vec<Option<String>>>) -> Arc<Engine> {
        let tables = program
            .objects
            .iter()
            .enumerate()
            .map(|(i, o)| {
                o.table.as_ref().map(|t| al_data::TableDef {
                    object: i as u32,
                    name: o.name.clone(),
                    fields: t
                        .fields
                        .iter()
                        .map(|f| al_data::FieldDef {
                            id: f.id,
                            name: f.name.clone(),
                            ty: f.ty.clone(),
                            stored: f.stored,
                        })
                        .collect(),
                    keys: t.keys.clone(),
                    per_company: t.per_company,
                })
            })
            .collect();
        let events = events::Registry::build(&program);
        Arc::new(Engine {
            program,
            blocked,
            tables,
            events,
        })
    }
}

pub struct Session {
    pub engine: Arc<Engine>,
    pub last_error: AlError,
    depth: usize,
    /// Page handler functions running right now (`[PageHandler]`, `[ModalPageHandler]`).
    pub(crate) page_handler_depth: usize,
    /// Number sequences by (company, name); a global sequence has an empty company.
    pub(crate) sequences: std::collections::HashMap<(String, String), sessionops::Sequence>,
    /// The one instance of each `SingleInstance = true` codeunit in this session.
    singletons: std::collections::HashMap<usize, Rc<RefCell<Instance>>>,
    pub(crate) store: al_data::memory::MemStore,
    /// Private tables of temporary records, by temp id.
    pub(crate) temps: std::collections::HashMap<u64, al_data::memory::MemStore>,
    next_temp: u64,
    pub(crate) next_guid: u64,
    pub(crate) next_row_version: i64,
    /// Last AutoIncrement value handed out per table (and company). It is not part of the
    /// store: a rollback undoes rows but not the counter (corpus
    /// record/TestAutoIncrementExplicitKey.al:154).
    pub(crate) autoinc: std::collections::HashMap<(String, u32), i64>,
    /// The counter each table had before the first Truncate of the open transaction; a
    /// rollback puts it back (corpus record/TestAutoIncrementEdges.al:36-170).
    pub(crate) truncate_undo: std::collections::HashMap<(String, u32), i64>,
    /// Records inside an OnValidate (by handle) with the field values from before the outermost
    /// Validate, which nested Validates of the same record see as xRec (corpus
    /// record/TestXRecContracts.al:133).
    pub(crate) validating: Vec<(usize, Vec<Value>)>,
    /// Try functions on the call stack (Truncate is refused inside them).
    pub(crate) try_depth: usize,
    pub(crate) cx: al_data::AccessContext,
    /// Whether the builtin being called has its result used (ADR-011 `ret_used`).
    pub(crate) ret_used: bool,
    /// Manual event subscriptions made by `BindSubscription`.
    pub(crate) bindings: Vec<dispatch::Binding>,
    /// The parameters of the routine that just failed with an error, as it left them:
    /// the caller writes its `var` arguments back before it handles the error.
    pub(crate) error_finals: Option<Vec<Value>>,
    /// The highest rowversion handed out when the last `Commit` ran: rows at or below it are durable.
    pub(crate) committed_row_version: i64,
    /// Handler functions bound to the running test method (None outside one).
    pub(crate) ui: Option<ui::UiScope>,
    /// The TestPage variable whose `Trap()` waits for the next plain `Page.Run` (ADR-012).
    /// The variable-bound control values each SaveValues page stored when it closed.
    pub(crate) saved_values: std::collections::HashMap<u32, Vec<(usize, Value)>>,
    pub(crate) trap: Option<Rc<RefCell<al_builtins::value::PageState>>>,
}

/// Nested AL calls allowed before the session gives up. BC's own limit and
/// its error are not pinned by the corpus, so exceeding it is Unsupported.
pub const MAX_CALL_DEPTH: usize = 400;

impl Session {
    pub fn new(engine: Arc<Engine>) -> Session {
        Session {
            engine,
            last_error: AlError::default(),
            depth: 0,
            singletons: Default::default(),
            store: Default::default(),
            temps: Default::default(),
            next_temp: 0,
            next_guid: 0,
            next_row_version: 0,
            autoinc: Default::default(),
            truncate_undo: Default::default(),
            validating: Vec::new(),
            try_depth: 0,
            cx: al_data::AccessContext {
                company: "CRONUS".into(),
                user: "USER".into(),
            },
            ret_used: true,
            bindings: Vec::new(),
            error_finals: None,
            committed_row_version: 0,
            page_handler_depth: 0,
            sequences: Default::default(),
            ui: None,
            saved_values: std::collections::HashMap::new(),
            trap: None,
        }
    }

    /// Makes every change so far durable (and a Truncate of this transaction final).
    pub fn commit_transaction(&mut self) {
        al_data::Store::commit(&mut self.store);
        self.truncate_undo.clear();
        // Rows at or below this rowversion are durable: a buffer older than one of them is stale.
        self.committed_row_version = self.next_row_version;
    }

    /// Discards every change since the last commit. The AutoIncrement counters are not
    /// part of the data and stay (corpus record/TestAutoIncrementExplicitKey.al:154), except
    /// those a Truncate of this transaction changed.
    pub fn rollback_transaction(&mut self) {
        al_data::Store::rollback(&mut self.store);
        for (slot, value) in std::mem::take(&mut self.truncate_undo) {
            self.autoinc.insert(slot, value);
        }
    }

    /// The session's database, for runners that manage transactions around tests.
    pub fn store_mut(&mut self) -> &mut al_data::memory::MemStore {
        &mut self.store
    }

    /// The initial value of a slot of type `t`.
    pub fn default_value(&mut self, t: &RtType) -> Value {
        match t {
            RtType::Record(table, temporary) => {
                // A table declared `TableType = Temporary` is temporary for every variable
                // (corpus record 60198 TempOnly_IsTemporary_AlwaysTrue).
                let temporary = &(*temporary
                    || self.engine.program.objects[*table as usize]
                        .properties
                        .iter()
                        .any(|(k, v)| {
                            k.eq_ignore_ascii_case("TableType") && v.trim().eq_ignore_ascii_case("Temporary")
                        }));
                let temp_id = temporary.then(|| {
                    self.next_temp += 1;
                    self.next_temp
                });
                Value::Record(Rc::new(RefCell::new(RecordState {
                    table: *table,
                    temporary: *temporary,
                    temp_id,
                    fields: self.initial_fields(*table as usize),
                    filters: Vec::new(),
                    filter_group: 0,
                    marks: Vec::new(),
                    marked_only: false,
                    read_isolation: 0,
                    // A record in a test run is Validated (corpus record/TestSecurityFilteringContracts.al:161-175).
                    security_filtering: 2,
                    key: 0,
                    ascending: true,
                    field_descending: false,
                    position: None,
                    auto_calc: Vec::new(),
                })))
            }
            other => Value::default_for(other),
        }
    }

    /// A fresh instance of codeunit `object` with default globals.
    pub fn instance(&mut self, object: usize) -> Rc<RefCell<Instance>> {
        let tys: Vec<RtType> = self.engine.program.objects[object]
            .globals
            .iter()
            .map(|g| g.ty.clone())
            .collect();
        let globals = tys.iter().map(|t| self.default_value(t)).collect();
        Rc::new(RefCell::new(Instance {
            object: object as u32,
            globals,
            initialized: true,
        }))
    }

    /// A record passed by value is a copy (ADR-010). A temporary record's copy
    /// gets an independent, empty table of its own (corpus
    /// record/TestTempTableContracts.al:TempVar_PassedByValue_CallerNotAffected).
    pub(crate) fn record_by_value(&mut self, rc: &Rc<RefCell<RecordState>>) -> Value {
        let mut st = rc.borrow().clone();
        if st.temporary {
            self.next_temp += 1;
            st.temp_id = Some(self.next_temp);
        }
        Value::Record(Rc::new(RefCell::new(st)))
    }

    pub(crate) fn ensure_initialized(&mut self, inst: &Rc<RefCell<Instance>>) {
        if inst.borrow().initialized {
            return;
        }
        let object = inst.borrow().object as usize;
        let fresh = self.instance(object);
        let globals = std::mem::take(&mut fresh.borrow_mut().globals);
        let mut i = inst.borrow_mut();
        i.globals = globals;
        i.initialized = true;
    }

    /// Calls routine `routine` of `object` on `instance`. Returns the return
    /// value and the final values of all parameters (for `var` write-back).
    pub fn call(
        &mut self,
        object: usize,
        routine: usize,
        instance: Rc<RefCell<Instance>>,
        args: Vec<Value>,
    ) -> Result<(Value, Vec<Value>), Signal> {
        if let Some(Some(reason)) = self.engine.blocked.get(object).and_then(|o| o.get(routine)) {
            return Err(Signal::Unsupported(reason.clone()));
        }
        if self.depth >= MAX_CALL_DEPTH {
            return Err(Signal::Unsupported(format!(
                "call depth over {MAX_CALL_DEPTH} (BC's limit not pinned)"
            )));
        }
        self.depth += 1;
        let r = if self.engine.events.publishers.contains_key(&(object, routine)) {
            self.publish(object, routine, instance, args)
        } else {
            self.call_inner(object, routine, instance, args)
        };
        self.depth -= 1;
        r
    }

    fn call_inner(
        &mut self,
        object: usize,
        routine: usize,
        instance: Rc<RefCell<Instance>>,
        args: Vec<Value>,
    ) -> Result<(Value, Vec<Value>), Signal> {
        self.ensure_initialized(&instance);
        let engine = self.engine.clone();
        let r = &engine.program.objects[object].routines[routine];
        let mut locals: Vec<Value> = Vec::with_capacity(r.locals.len());
        for s in &r.locals {
            locals.push(self.default_value(&s.ty));
        }
        for (i, a) in args.into_iter().enumerate().take(r.params) {
            locals[i] = match (a, r.by_var.get(i).copied().unwrap_or(false)) {
                (Value::Record(rc), false) => self.record_by_value(&rc),
                (a, _) => ops::convert(a, &r.locals[i].ty)?,
            };
        }
        let mut frame = Frame {
            object,
            routine,
            instance,
            locals,
        };
        let is_try = r.attributes.iter().any(|a| a.name.eq_ignore_ascii_case("TryFunction"));
        self.try_depth += usize::from(is_try);
        let outcome = self.exec_block(&r.body, &mut frame);
        self.try_depth -= usize::from(is_try);
        self.end_trap_scope(&frame, r.params, &r.by_var);
        let ret = match outcome {
            Ok(()) => None,
            Err(Signal::Exit(v)) => v,
            Err(Signal::Break) => None,
            Err(e) => {
                // Variables set before the error stay set for the caller that catches it
                // (error-handling/TestTryFunctionContracts.al, codeunit/TestScopeIsolationContracts.al).
                if matches!(e, Signal::Error(_)) {
                    self.error_finals = Some(frame.locals[..r.params].to_vec());
                }
                return Err(e);
            }
        };
        let value = match (&r.ret, ret, r.ret_local) {
            (Some(t), Some(v), _) => ops::convert(v, t)?,
            (Some(_), None, Some(l)) => frame.locals[l].clone(),
            (Some(t), None, None) => Value::default_for(t),
            (None, _, _) => Value::Null,
        };
        frame.locals.truncate(r.params);
        Ok((value, frame.locals))
    }

    fn exec_block(&mut self, stmts: &[ir::Stmt], f: &mut Frame) -> Result<(), Signal> {
        for s in stmts {
            self.exec(s, f)?;
        }
        Ok(())
    }

    fn exec(&mut self, s: &ir::Stmt, f: &mut Frame) -> Result<(), Signal> {
        use ir::Stmt as S;
        match s {
            S::Assign(p, e) => {
                let v = self.eval(e, f)?;
                self.write(p, v, f)
            }
            S::Expr(e) => self.eval(e, f).map(|_| ()),
            S::If(c, t, e) => {
                if ops::truthy(&self.eval(c, f)?)? {
                    self.exec_block(t, f)
                } else {
                    self.exec_block(e, f)
                }
            }
            S::While(c, body) => {
                while ops::truthy(&self.eval(c, f)?)? {
                    match self.exec_block(body, f) {
                        Err(Signal::Break) => break,
                        r => r?,
                    }
                }
                Ok(())
            }
            S::Repeat(body, until) => {
                loop {
                    match self.exec_block(body, f) {
                        Err(Signal::Break) => break,
                        r => r?,
                    }
                    if ops::truthy(&self.eval(until, f)?)? {
                        break;
                    }
                }
                Ok(())
            }
            S::For {
                var,
                from,
                to,
                down,
                body,
            } => {
                let start = int(&self.eval(from, f)?)?;
                let end = int(&self.eval(to, f)?)?;
                let mut i = start;
                while if *down { i >= end } else { i <= end } {
                    let slot_ty = self.place_type(var, f);
                    self.write(var, ops::convert(Value::BigInteger(i), &slot_ty)?, f)?;
                    match self.exec_block(body, f) {
                        Err(Signal::Break) => break,
                        r => r?,
                    }
                    i = int(&self.read(var, f)?)?;
                    i += if *down { -1 } else { 1 };
                }
                Ok(())
            }
            S::Foreach { var, collection, body } => {
                let items: Vec<Value> = match self.eval(collection, f)? {
                    Value::List(l) => l.borrow().clone(),
                    other => return Err(Signal::Unsupported(format!("foreach over {other:?}"))),
                };
                for item in items {
                    self.write(var, item, f)?;
                    match self.exec_block(body, f) {
                        Err(Signal::Break) => break,
                        r => r?,
                    }
                }
                Ok(())
            }
            S::Case {
                scrutinee,
                branches,
                otherwise,
            } => {
                let v = self.eval(scrutinee, f)?;
                for (labels, body) in branches {
                    if self.matches_any(&v, labels, f)? {
                        return self.exec_block(body, f);
                    }
                }
                self.exec_block(otherwise, f)
            }
            S::Exit(e) => {
                let v = match e {
                    Some(e) => Some(self.eval(e, f)?),
                    None => None,
                };
                Err(Signal::Exit(v))
            }
            S::Break => Err(Signal::Break),
            S::AssertError(body) => {
                // Inside a UI handler, with writes pending from before the page opened, the extent
                // of the rollback is not pinned: handlers/TestPageEnumVariableControl_Tests.al
                // RunModal_EnumGlobalControl_SetValueRejectsTheBareMemberName deletes in
                // Initialize() without a Commit, runs a modal page whose handler does
                // `asserterror` on a rejected SetValue, and still sees the delete afterwards on
                // real BC. Rolling back to the last commit would undo it.
                let pending_in_handler = self.in_ui_handler() && self.store.has_uncommitted();
                match self.exec_block(body, f) {
                    Err(Signal::Error(_)) if pending_in_handler => Err(Signal::Unsupported(
                        "asserterror in a UI handler with writes pending from before it ran: the extent \
                         of the rollback is not pinned"
                            .into(),
                    )),
                    Err(Signal::Error(e)) => {
                        // The caught error rolls the transaction back to the last commit
                        // (record/TestRecordRenameRollback.al).
                        self.rollback_transaction();
                        self.last_error = e;
                        Ok(())
                    }
                    // The message is not pinned by the corpus yet (docs/probes-needed.md).
                    Ok(()) => Err(Signal::Error(AlError {
                        message: "An error was expected inside an ASSERTERROR statement.".into(),
                        code: "Dialog".into(),
                        call_stack: String::new(),
                    })),
                    Err(other) => Err(other),
                }
            }
        }
    }

    fn matches_any(&mut self, v: &Value, labels: &[ir::SetElem], f: &mut Frame) -> Result<bool, Signal> {
        for l in labels {
            let hit = match l {
                ir::SetElem::One(e) => ops::compare(v, &self.eval(e, f)?)? == Ordering::Equal,
                ir::SetElem::Range(lo, hi) => {
                    let above = match lo {
                        Some(lo) => ops::compare(v, &self.eval(lo, f)?)? != Ordering::Less,
                        None => true,
                    };
                    let below = match hi {
                        Some(hi) => ops::compare(v, &self.eval(hi, f)?)? != Ordering::Greater,
                        None => true,
                    };
                    above && below
                }
            };
            if hit {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn eval(&mut self, e: &ir::Expr, f: &mut Frame) -> Result<Value, Signal> {
        use ir::Expr as E;
        match e {
            E::Const(c) => Ok(constant(c)),
            E::Load(p) => self.read(p, f),
            E::Convert(t, e) => {
                let v = self.eval(e, f)?;
                ops::convert(v, t)
            }
            E::Unary(op, e) => {
                let v = self.eval(e, f)?;
                match op {
                    ir::UnOp::Not => Ok(Value::Boolean(!ops::truthy(&v)?)),
                    ir::UnOp::Plus => Ok(v),
                    ir::UnOp::Neg => ops::arith(ops::Arith::Sub, &Value::Integer(0), &v).map(|r| match (&v, r) {
                        (Value::Decimal(_), r) => r,
                        (_, r) => r,
                    }),
                }
            }
            E::Binary(op, l, r) => {
                // AL evaluates both operands of and/or (no short circuit):
                // docs/probes-needed.md.
                let l = self.eval(l, f)?;
                let r = self.eval(r, f)?;
                use ir::BinOp as B;
                Ok(match op {
                    B::Add => ops::arith(ops::Arith::Add, &l, &r)?,
                    B::Sub => ops::arith(ops::Arith::Sub, &l, &r)?,
                    B::Mul => ops::arith(ops::Arith::Mul, &l, &r)?,
                    B::Div => ops::arith(ops::Arith::Div, &l, &r)?,
                    B::IntDiv => ops::arith(ops::Arith::IntDiv, &l, &r)?,
                    B::Mod => ops::arith(ops::Arith::Mod, &l, &r)?,
                    B::And => Value::Boolean(ops::truthy(&l)? & ops::truthy(&r)?),
                    B::Or => Value::Boolean(ops::truthy(&l)? | ops::truthy(&r)?),
                    B::Xor => Value::Boolean(ops::truthy(&l)? ^ ops::truthy(&r)?),
                    B::Eq => Value::Boolean(ops::compare(&l, &r)? == Ordering::Equal),
                    B::Ne => Value::Boolean(ops::compare(&l, &r)? != Ordering::Equal),
                    B::Lt => Value::Boolean(ops::compare(&l, &r)? == Ordering::Less),
                    B::Le => Value::Boolean(ops::compare(&l, &r)? != Ordering::Greater),
                    B::Gt => Value::Boolean(ops::compare(&l, &r)? == Ordering::Greater),
                    B::Ge => Value::Boolean(ops::compare(&l, &r)? != Ordering::Less),
                })
            }
            E::In(v, set) => {
                let v = self.eval(v, f)?;
                Ok(Value::Boolean(self.matches_any(&v, set, f)?))
            }
            E::Ternary(c, a, b) => {
                if ops::truthy(&self.eval(c, f)?)? {
                    self.eval(a, f)
                } else {
                    self.eval(b, f)
                }
            }
            E::Call {
                object,
                routine,
                instance,
                args,
                try_call,
            } => {
                let inst = match instance {
                    None => f.instance.clone(),
                    Some(e) => match self.eval(e, f)? {
                        Value::Codeunit(i) => self.shared_instance(i),
                        // A table procedure runs with the record as its Rec.
                        Value::Record(rc) => self.record_instance(*object, rc, None)?,
                        other => return Err(Signal::Unsupported(format!("call on {other:?}"))),
                    },
                };
                let mut values = Vec::with_capacity(args.len());
                for a in args {
                    values.push(match a {
                        ir::Arg::Value(e) => self.eval(e, f)?,
                        ir::Arg::Ref(p) => self.read(p, f)?,
                    });
                }
                let result = self.call(*object, *routine, inst, values);
                if let Err(Signal::Error(_)) = &result {
                    if let Some(finals) = self.error_finals.take() {
                        for (a, v) in args.iter().zip(finals) {
                            if let ir::Arg::Ref(p) = a {
                                self.write(p, v, f)?;
                            }
                        }
                    }
                }
                let (ret, finals) = match (result, try_call) {
                    (Err(Signal::Error(e)), true) => {
                        self.last_error = e;
                        return Ok(Value::Boolean(false));
                    }
                    (Ok((_, finals)), true) => (Value::Boolean(true), finals),
                    (r, _) => r?,
                };
                for (a, v) in args.iter().zip(finals) {
                    if let ir::Arg::Ref(p) = a {
                        self.write(p, v, f)?;
                    }
                }
                Ok(ret)
            }
            E::Control { page, control } => {
                let p = self.eval(page, f)?;
                self.page_control(p, *control)
            }
            E::Builtin {
                id,
                receiver,
                args,
                ret_used,
            } => {
                // Event subscriptions are session state: the VM runs them (dispatch.rs).
                if let "Session.BindSubscription#1" | "Session.UnbindSubscription#1" = id.as_str() {
                    let Some(ir::Arg::Value(e)) = args.first() else {
                        return Err(Signal::Unsupported(format!("{id} with this argument")));
                    };
                    let cu = self.eval(e, f)?;
                    return self.subscription(id.starts_with("Session.Bind"), cu, *ret_used);
                }
                let native =
                    al_builtins::native::lookup(id).ok_or_else(|| Signal::Unsupported(format!("builtin {id}")))?;
                let mut recv = match receiver.as_deref() {
                    Some(ir::Arg::Value(e)) => Some(self.eval(e, f)?),
                    Some(ir::Arg::Ref(p)) => Some(self.read(p, f)?),
                    None => None,
                };
                let mut values = Vec::with_capacity(args.len());
                for a in args {
                    values.push(match a {
                        ir::Arg::Value(e) => self.eval(e, f)?,
                        ir::Arg::Ref(p) => self.read(p, f)?,
                    });
                }
                self.ret_used = *ret_used;
                let ret = native(self, recv.as_mut(), &mut values)?;
                if let (Some(ir::Arg::Ref(p)), Some(v)) = (receiver.as_deref(), recv) {
                    self.write(p, v, f)?;
                }
                for (a, v) in args.iter().zip(values) {
                    if let ir::Arg::Ref(p) = a {
                        self.write(p, v, f)?;
                    }
                }
                Ok(ret)
            }
        }
    }

    /// The instance a call on codeunit value `i` runs on: the session's
    /// singleton for `SingleInstance = true` codeunits, `i` itself otherwise.
    pub(crate) fn shared_instance(&mut self, i: Rc<RefCell<Instance>>) -> Rc<RefCell<Instance>> {
        let object = i.borrow().object as usize;
        let single = self.engine.program.objects[object]
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("SingleInstance") && v.trim().eq_ignore_ascii_case("true"));
        if !single {
            return i;
        }
        if !self.singletons.contains_key(&object) {
            let inst = self.instance(object);
            self.singletons.insert(object, inst);
        }
        self.singletons[&object].clone()
    }

    fn place_type(&self, p: &ir::Place, f: &Frame) -> RtType {
        let r = &self.engine.program.objects[f.object].routines[f.routine];
        match p {
            ir::Place::Local(i) => r.locals[*i].ty.clone(),
            ir::Place::Global(i) => {
                let o = f.instance.borrow().object as usize;
                self.engine.program.objects[o].globals[*i].ty.clone()
            }
            ir::Place::Index(base, _) => match self.place_type(base, f) {
                RtType::Array(_, el) => *el,
                other => other,
            },
            ir::Place::Field(base, pos) => match self.place_type(base, f) {
                RtType::Record(t, _) => self.engine.tables[t as usize]
                    .as_ref()
                    .map_or(RtType::Variant, |td| td.fields[*pos].ty.clone()),
                _ => RtType::Variant,
            },
        }
    }

    fn read(&mut self, p: &ir::Place, f: &mut Frame) -> Result<Value, Signal> {
        match p {
            ir::Place::Local(i) => Ok(f.locals[*i].clone()),
            ir::Place::Global(i) => Ok(f.instance.borrow().globals[*i].clone()),
            ir::Place::Index(base, idx) => {
                let (arr, flat) = self.element(base, idx, f)?;
                let v = arr.borrow()[flat].clone();
                Ok(v)
            }
            ir::Place::Field(base, pos) => match self.read(base, f)? {
                Value::Record(rc) => Ok(rc.borrow().fields[*pos].clone()),
                other => Err(Signal::Unsupported(format!("field of {other:?}"))),
            },
        }
    }

    fn write(&mut self, p: &ir::Place, v: Value, f: &mut Frame) -> Result<(), Signal> {
        // `:=` between records copies the fields into the target record (ADR-010).
        if let Value::Record(src) = &v {
            if !matches!(p, ir::Place::Field(..)) {
                if let Ok(Value::Record(dst)) = self.read(p, f) {
                    if !Rc::ptr_eq(src, &dst) {
                        let fields = src.borrow().fields.clone();
                        if fields.iter().any(|f| matches!(f, Value::Blob(_))) {
                            // Whether a copied record shares or copies its Blob is not pinned.
                            return Err(Signal::Unsupported("record assignment with a Blob field".into()));
                        }
                        dst.borrow_mut().fields = fields;
                    }
                    return Ok(());
                }
            }
        }
        if let ir::Place::Field(base, pos) = p {
            let ty = self.place_type(p, f);
            let v = ops::convert(v, &ty)?;
            return match self.read(base, f)? {
                Value::Record(rc) => {
                    rc.borrow_mut().fields[*pos] = v;
                    Ok(())
                }
                other => Err(Signal::Unsupported(format!("field of {other:?}"))),
            };
        }
        match p {
            ir::Place::Local(i) => f.locals[*i] = v,
            ir::Place::Global(i) => f.instance.borrow_mut().globals[*i] = v,
            ir::Place::Index(base, idx) => {
                let el_ty = self.place_type(p, f);
                let (arr, flat) = self.element(base, idx, f)?;
                arr.borrow_mut()[flat] = ops::convert(v, &el_ty)?;
            }
            ir::Place::Field(..) => unreachable!("handled above"),
        }
        Ok(())
    }

    /// The array behind `base` and the flat position of `idx` (1-based, row-major).
    fn element(&mut self, base: &ir::Place, idx: &[ir::Expr], f: &mut Frame) -> Result<(Cells, usize), Signal> {
        let RtType::Array(dims, _) = self.place_type(base, f) else {
            return Err(Signal::Unsupported("indexing a non-array".into()));
        };
        let arr = match self.read(base, f)? {
            Value::Array(a) => a,
            other => return Err(Signal::Unsupported(format!("indexing {other:?}"))),
        };
        let mut flat = 0usize;
        for (d, e) in dims.iter().zip(idx) {
            let i = int(&self.eval(e, f)?)?;
            if i < 1 || i > i64::from(*d) {
                return Err(Signal::Unsupported(
                    "array index out of bounds (error text not pinned yet)".into(),
                ));
            }
            flat = flat * (*d as usize) + (i as usize - 1);
        }
        Ok((arr, flat))
    }
}

/// The shared storage of an array value.
type Cells = Rc<RefCell<Vec<Value>>>;

struct Frame {
    object: usize,
    routine: usize,
    instance: Rc<RefCell<Instance>>,
    locals: Vec<Value>,
}

fn int(v: &Value) -> Result<i64, Signal> {
    match v {
        Value::Integer(i) => Ok(i64::from(*i)),
        Value::BigInteger(i) => Ok(*i),
        Value::Byte(b) => Ok(i64::from(*b)),
        Value::Char(c) => Ok(i64::from(*c)),
        other => Err(Signal::Unsupported(format!("{other:?} used as an integer"))),
    }
}

fn constant(c: &ir::Const) -> Value {
    match c {
        ir::Const::Boolean(b) => Value::Boolean(*b),
        ir::Const::Integer(i) => Value::Integer(*i),
        ir::Const::BigInteger(i) => Value::BigInteger(*i),
        ir::Const::Decimal(s) => Value::Decimal(s.parse().expect("decimal literal from the lowerer")),
        ir::Const::Text(s) => Value::Text(s.clone()),
        ir::Const::Date(d) => Value::Date(*d, false),
        ir::Const::Time(t) => Value::Time(*t),
        ir::Const::DateTime(t) => Value::DateTime(*t),
        ir::Const::Option(i, m) => Value::Option(*i, m.clone().into()),
        ir::Const::Enum(o, v) => Value::Enum(*o, *v),
        ir::Const::FieldPos(p) => Value::Integer(*p as i32),
    }
}

impl al_builtins::format::FormatCtx for Session {
    /// The caption a value declares, else its name (types/TestEnumCaption.al).
    fn enum_name(&self, id: u32, value: i32) -> String {
        let o = &self.engine.program.objects[id as usize];
        if let Some((_, c)) = o.enum_captions.iter().find(|(v, _)| *v == value) {
            return c.clone();
        }
        o.enum_values
            .iter()
            .find(|(v, _)| *v == value)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| value.to_string())
    }
}

impl al_builtins::native::Ctx for Session {
    fn last_error(&self) -> AlError {
        self.last_error.clone()
    }
    fn set_last_error(&mut self, e: AlError) {
        self.last_error = e;
    }
    fn record(&mut self, method: &str, rec: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
        match rec {
            Some(Value::Record(rc)) => {
                let rc = rc.clone();
                self.record_op(method, &rc, args)
            }
            other => Err(Signal::Unsupported(format!("record method {method} on {other:?}"))),
        }
    }
    fn page(&mut self, method: &str, recv: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
        self.page_op(method, recv, args)
    }

    fn ref_op(&mut self, id: &str, receiver: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
        Session::ref_op(self, id, receiver, args)
    }
    fn session_op(&mut self, id: &str, args: &mut [Value]) -> Result<Value, Signal> {
        Session::session_op(self, id, args)
    }
    fn enum_members(&self, id: u32) -> Option<Vec<(i32, String)>> {
        Some(self.engine.program.objects[id as usize].enum_values.clone())
    }
    fn commit(&mut self) -> Result<(), Signal> {
        self.commit_transaction();
        Ok(())
    }
    fn ret_used(&self) -> bool {
        self.ret_used
    }
    fn ui_message(&mut self, text: String) -> Result<(), Signal> {
        Session::ui_message(self, text)
    }
    fn ui_confirm(&mut self, question: String, default: bool) -> Result<bool, Signal> {
        Session::ui_confirm(self, question, default)
    }
    fn ui_str_menu(&mut self, options: String, default: i32, instruction: String) -> Result<i32, Signal> {
        Session::ui_str_menu(self, options, default, instruction)
    }
    fn new_guid(&mut self) -> [u8; 16] {
        // Deterministic ids (ADR-006: no OS randomness in the core).
        self.next_guid += 1;
        let mut g = [0u8; 16];
        g[8..].copy_from_slice(&self.next_guid.to_be_bytes());
        g[6] = 0x40;
        g
    }
}

#[cfg(test)]
mod conformance {
    /// ADR-001: the compiled program is shared across threads; sessions are not.
    #[test]
    fn engine_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<super::Engine>();
    }
}
