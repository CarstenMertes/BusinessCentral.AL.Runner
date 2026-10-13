//! Tree-walking interpreter over the typed IR (ADR-011). `Engine` is the
//! immutable program (ADR-001); `Session` holds all mutable state.

use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::value::{Instance, RtType, Value};
use al_ir as ir;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;
use std::sync::Arc;

pub struct Engine {
    pub program: ir::Program,
    /// Per object, per routine: why it cannot run, if it cannot.
    pub blocked: Vec<Vec<Option<String>>>,
}

impl Engine {
    pub fn new(program: ir::Program, blocked: Vec<Vec<Option<String>>>) -> Arc<Engine> {
        Arc::new(Engine { program, blocked })
    }
}

pub struct Session {
    pub engine: Arc<Engine>,
    pub last_error: AlError,
    depth: usize,
    /// The one instance of each `SingleInstance = true` codeunit in this session.
    singletons: std::collections::HashMap<usize, Rc<RefCell<Instance>>>,
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
        }
    }

    /// A fresh instance of codeunit `object` with default globals.
    pub fn instance(&self, object: usize) -> Rc<RefCell<Instance>> {
        let globals = self.engine.program.objects[object]
            .globals
            .iter()
            .map(|g| Value::default_for(&g.ty))
            .collect();
        Rc::new(RefCell::new(Instance {
            object: object as u32,
            globals,
            initialized: true,
        }))
    }

    fn ensure_initialized(&self, inst: &Rc<RefCell<Instance>>) {
        let mut i = inst.borrow_mut();
        if !i.initialized {
            let obj = &self.engine.program.objects[i.object as usize];
            i.globals = obj.globals.iter().map(|g| Value::default_for(&g.ty)).collect();
            i.initialized = true;
        }
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
        let r = self.call_inner(object, routine, instance, args);
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
        let mut locals: Vec<Value> = r.locals.iter().map(|s| Value::default_for(&s.ty)).collect();
        for (i, a) in args.into_iter().enumerate().take(r.params) {
            locals[i] = ops::convert(a, &r.locals[i].ty)?;
        }
        let mut frame = Frame {
            object,
            routine,
            instance,
            locals,
        };
        let ret = match self.exec_block(&r.body, &mut frame) {
            Ok(()) => None,
            Err(Signal::Exit(v)) => v,
            Err(Signal::Break) => None,
            Err(e) => return Err(e),
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
            S::AssertError(body) => match self.exec_block(body, f) {
                Err(Signal::Error(e)) => {
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
            },
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
            E::Builtin { id, receiver, args } => {
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
    fn shared_instance(&mut self, i: Rc<RefCell<Instance>>) -> Rc<RefCell<Instance>> {
        let object = i.borrow().object as usize;
        let single = self.engine.program.objects[object]
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("SingleInstance") && v.trim().eq_ignore_ascii_case("true"));
        if !single {
            return i;
        }
        let engine = self.engine.clone();
        self.singletons
            .entry(object)
            .or_insert_with(|| {
                let globals = engine.program.objects[object]
                    .globals
                    .iter()
                    .map(|g| Value::default_for(&g.ty))
                    .collect();
                Rc::new(RefCell::new(Instance {
                    object: object as u32,
                    globals,
                    initialized: true,
                }))
            })
            .clone()
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
        }
    }

    fn write(&mut self, p: &ir::Place, v: Value, f: &mut Frame) -> Result<(), Signal> {
        match p {
            ir::Place::Local(i) => f.locals[*i] = v,
            ir::Place::Global(i) => f.instance.borrow_mut().globals[*i] = v,
            ir::Place::Index(base, idx) => {
                let el_ty = self.place_type(p, f);
                let (arr, flat) = self.element(base, idx, f)?;
                arr.borrow_mut()[flat] = ops::convert(v, &el_ty)?;
            }
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
    }
}

impl al_builtins::format::FormatCtx for Session {
    fn enum_name(&self, id: u32, value: i32) -> String {
        self.engine.program.objects[id as usize]
            .enum_values
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
