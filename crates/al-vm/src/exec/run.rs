//! The bytecode loop. Every instruction calls the helpers of its operation (`ops`, `Session::read`, `write`, ...).

use super::*;
use crate::{Frame, Session};
use al_builtins::describe::Describe;
use al_builtins::native::Signal;
use al_builtins::ops;
use al_builtins::value::Value;

/// The value in register `r`: a temporary is moved out (so it holds nothing alive afterwards), a local is cloned.
#[inline(always)]
fn take(f: &mut Frame, r: u16, nl: u16) -> Value {
    if r >= nl {
        std::mem::replace(&mut f.locals[r as usize], Value::Null)
    } else {
        f.locals[r as usize].clone()
    }
}

/// Whether a value owns nothing (no text, no handle): letting go of it needs no drop glue.
#[inline(always)]
fn plain(v: &Value) -> bool {
    matches!(
        v,
        Value::Null
            | Value::Boolean(_)
            | Value::Integer(_)
            | Value::BigInteger(_)
            | Value::Decimal(_)
            | Value::Byte(_)
            | Value::Char(_)
            | Value::Date(..)
            | Value::Time(_)
            | Value::DateTime(_)
            | Value::Duration(_)
            | Value::Guid(_)
            | Value::Enum(..)
    )
}

/// `*slot = v` without the call to the drop glue when the old value owns nothing.
#[inline(always)]
fn set(slot: &mut Value, v: Value) {
    if plain(slot) {
        std::mem::forget(std::mem::replace(slot, v));
    } else {
        *slot = v;
    }
}

/// The capture's id of the loop whose register is `r`.
fn cap_id(f: &Frame, r: u16) -> Option<u32> {
    match f.locals[r as usize] {
        Value::Integer(n) => u32::try_from(n).ok(),
        _ => None,
    }
}

/// Lets go of temporary `r` after an instruction read it by reference.
#[inline(always)]
fn release(f: &mut Frame, r: u16, nl: u16) {
    if r >= nl {
        let slot = &mut f.locals[r as usize];
        if !plain(slot) {
            *slot = Value::Null;
        }
    }
}

impl Session {
    /// Runs `chunk` on frame `f`. The frame's locals already hold the temporaries.
    pub(crate) fn run_chunk(&mut self, chunk: &Chunk, f: &mut Frame) -> Result<(), Signal> {
        self.run_range::<false>(chunk, f, 0, chunk.code.len()).map(|_| ())
    }

    /// Runs the instructions `start..end` and returns where it stopped: `end` when the code ran off its end, otherwise
    /// the target of a jump that left the range (a `break` or `continue` out of an `asserterror` body).
    pub(crate) fn run_range<const RANGED: bool>(
        &mut self,
        chunk: &Chunk,
        f: &mut Frame,
        start: usize,
        end: usize,
    ) -> Result<usize, Signal> {
        let nl = chunk.nlocals;
        let code = &chunk.code[..];
        let mut pc = start;
        /// A typed Integer arithmetic op: the operands are Integers by the compiler's proof; any other kind takes the generic path.
        macro_rules! int_op {
            ($op:ident, $d:expr, $a:expr, $b:expr) => {{
                let (d, a, b) = ($d, $a, $b);
                if let (Value::Integer(x), Value::Integer(y)) = (&f.locals[a as usize], &f.locals[b as usize]) {
                    let n = ops::int_arith(ops::Arith::$op, *x, *y)?;
                    f.locals[d as usize] = Value::Integer(n);
                } else {
                    self.typed_fallback(ir::BinOp::$op, &Value::Null, d, a, b, f, nl, false)?;
                }
            }};
        }

        macro_rules! cmp_op {
            ($op:ident, $cmp:tt, $d:expr, $a:expr, $b:expr) => {{
                let (d, a, b) = ($d, $a, $b);
                if let (Value::Integer(x), Value::Integer(y)) = (&f.locals[a as usize], &f.locals[b as usize]) {
                    let r = *x $cmp *y;
                    f.locals[d as usize] = Value::Boolean(r);
                } else {
                    self.typed_fallback(ir::BinOp::$op, &Value::Null, d, a, b, f, nl, false)?;
                }
            }};
        }

        macro_rules! jump_cmp {
            ($op:ident, $cmp:tt, $a:expr, $b:expr, $t:expr) => {{
                let (a, b) = ($a, $b);
                if let (Value::Integer(x), Value::Integer(y)) = (&f.locals[a as usize], &f.locals[b as usize]) {
                    if !(*x $cmp *y) {
                        pc = $t as usize;
                    }
                } else {
                    Self::note_typed_fallback();
                    let v = Self::binary_op(ir::BinOp::$op, &f.locals[a as usize], &f.locals[b as usize])?;
                    release(f, a, nl);
                    release(f, b, nl);
                    if !ops::truthy(&v)? {
                        pc = $t as usize;
                    }
                }
            }};
        }

        macro_rules! bool_op {
            ($op:ident, $bit:tt, $d:expr, $a:expr, $b:expr) => {{
                let (d, a, b) = ($d, $a, $b);
                if let (Value::Boolean(x), Value::Boolean(y)) = (&f.locals[a as usize], &f.locals[b as usize]) {
                    let r = *x $bit *y;
                    f.locals[d as usize] = Value::Boolean(r);
                } else {
                    self.typed_fallback(ir::BinOp::$op, &Value::Null, d, a, b, f, nl, false)?;
                }
            }};
        }
        while if RANGED {
            pc.wrapping_sub(start) < end - start
        } else {
            pc < end
        } {
            let op = code[pc];
            pc += 1;
            match op {
                Op::Move { d, a } => {
                    let v = f.locals[a as usize].clone();
                    set(&mut f.locals[d as usize], v);
                }
                Op::Const { d, k } => {
                    let v = self.constant_once(&chunk.consts[k as usize]);
                    set(&mut f.locals[d as usize], v);
                }
                Op::LoadGlobal { d, i } => {
                    let v = f.instance.borrow().globals[i as usize].clone();
                    set(&mut f.locals[d as usize], v);
                }
                Op::ReadPlace { d, p } => {
                    let v = self.read(&chunk.places[p as usize], f)?;
                    set(&mut f.locals[d as usize], v);
                }
                Op::StoreLocal { i, a, p } => {
                    let v = take(f, a, nl);
                    if matches!(v, Value::Record(_) | Value::Library(..)) {
                        self.write(&chunk.places[p as usize], v, f)?;
                    } else {
                        set(&mut f.locals[i as usize], v);
                    }
                }
                Op::StorePlace { p, a } => {
                    let v = take(f, a, nl);
                    self.write(&chunk.places[p as usize], v, f)?;
                }
                Op::Bin { op, d, a, b } => {
                    let v = Self::binary_op(op, &f.locals[a as usize], &f.locals[b as usize])?;
                    release(f, a, nl);
                    release(f, b, nl);
                    set(&mut f.locals[d as usize], v);
                }
                Op::ConstI { d, v } => f.locals[d as usize] = Value::Integer(v),
                Op::BinI { op, d, a, b } => {
                    use ir::BinOp as B;
                    let fast = if let (Value::Integer(x), Value::Integer(y)) =
                        (&f.locals[a as usize], &f.locals[b as usize])
                    {
                        let (x, y) = (*x, *y);
                        match op {
                            B::Add => x.checked_add(y).map(Value::Integer),
                            B::Sub => x.checked_sub(y).map(Value::Integer),
                            B::Mul => x.checked_mul(y).map(Value::Integer),
                            B::Mod if y != 0 => Some(Value::Integer(x.wrapping_rem(y))),
                            B::Eq => Some(Value::Boolean(x == y)),
                            B::Ne => Some(Value::Boolean(x != y)),
                            B::Lt => Some(Value::Boolean(x < y)),
                            B::Le => Some(Value::Boolean(x <= y)),
                            B::Gt => Some(Value::Boolean(x > y)),
                            B::Ge => Some(Value::Boolean(x >= y)),
                            _ => None,
                        }
                    } else {
                        None
                    };
                    let v = match fast {
                        Some(v) => v,
                        None => Self::binary_op(op, &f.locals[a as usize], &f.locals[b as usize])?,
                    };
                    release(f, a, nl);
                    release(f, b, nl);
                    set(&mut f.locals[d as usize], v);
                }
                Op::AddInt { d, a, b } => int_op!(Add, d, a, b),
                Op::SubInt { d, a, b } => int_op!(Sub, d, a, b),
                Op::MulInt { d, a, b } => int_op!(Mul, d, a, b),
                Op::DivInt { d, a, b } => int_op!(IntDiv, d, a, b),
                Op::ModInt { d, a, b } => int_op!(Mod, d, a, b),
                Op::AddIntK { d, a, k } => {
                    if let Value::Integer(x) = f.locals[a as usize] {
                        match x.checked_add(k) {
                            Some(n) => f.locals[d as usize] = Value::Integer(n),
                            None => return Err(ops::int_arith(ops::Arith::Add, x, k).unwrap_err()),
                        }
                    } else {
                        self.typed_fallback(ir::BinOp::Add, &Value::Integer(k), d, a, a, f, nl, true)?;
                    }
                }
                Op::EqInt { d, a, b } => cmp_op!(Eq, ==, d, a, b),
                Op::NeInt { d, a, b } => cmp_op!(Ne, !=, d, a, b),
                Op::LtInt { d, a, b } => cmp_op!(Lt, <, d, a, b),
                Op::LeInt { d, a, b } => cmp_op!(Le, <=, d, a, b),
                Op::GtInt { d, a, b } => cmp_op!(Gt, >, d, a, b),
                Op::GeInt { d, a, b } => cmp_op!(Ge, >=, d, a, b),
                Op::JumpUnlessEqInt { a, b, t } => jump_cmp!(Eq, ==, a, b, t),
                Op::JumpUnlessNeInt { a, b, t } => jump_cmp!(Ne, !=, a, b, t),
                Op::JumpUnlessLtInt { a, b, t } => jump_cmp!(Lt, <, a, b, t),
                Op::JumpUnlessLeInt { a, b, t } => jump_cmp!(Le, <=, a, b, t),
                Op::JumpUnlessGtInt { a, b, t } => jump_cmp!(Gt, >, a, b, t),
                Op::JumpUnlessGeInt { a, b, t } => jump_cmp!(Ge, >=, a, b, t),
                Op::JumpIfNotBool { a, t } => {
                    let Value::Boolean(c) = f.locals[a as usize] else {
                        Self::note_typed_fallback();
                        if !ops::truthy(&f.locals[a as usize])? {
                            pc = t as usize;
                        }
                        release(f, a, nl);
                        continue;
                    };
                    if !c {
                        pc = t as usize;
                    }
                }
                Op::NotBool { d, a } => {
                    let v = if let Value::Boolean(x) = f.locals[a as usize] {
                        Value::Boolean(!x)
                    } else {
                        Self::note_typed_fallback();
                        Self::unary_op(ir::UnOp::Not, take(f, a, nl))?
                    };
                    set(&mut f.locals[d as usize], v);
                }
                Op::AndBool { d, a, b } => bool_op!(And, &, d, a, b),
                Op::OrBool { d, a, b } => bool_op!(Or, |, d, a, b),
                Op::XorBool { d, a, b } => bool_op!(Xor, ^, d, a, b),
                Op::Un { op, d, a } => {
                    let v = Self::unary_op(op, take(f, a, nl))?;
                    set(&mut f.locals[d as usize], v);
                }
                Op::Convert { d, a, t } => {
                    let v = ops::convert(take(f, a, nl), &chunk.types[t as usize])?;
                    set(&mut f.locals[d as usize], v);
                }
                Op::CapHit { p } => self.cap_stmt_hit(p),
                Op::CapBegin { p } => self.cap_push_stmt(p),
                Op::CapEnd { s } => {
                    self.cap_pop_stmt();
                    let site = &chunk.cap_sites[s as usize];
                    self.cap_after_writes(site.pos, &site.writes, f);
                }
                Op::CapPop => self.cap_pop_stmt(),
                Op::CapLoopEnter { d } => {
                    f.locals[d as usize] = match self.cap_loop_enter() {
                        Some(n) => Value::Integer(n as i32),
                        None => Value::Null,
                    };
                }
                Op::CapLoopPass { a } => self.cap_loop_pass(cap_id(f, a)),
                Op::CapLoopExit { a } => self.cap_loop_exit(cap_id(f, a)),
                Op::IfaceResolve { r, s } => {
                    let recv = std::mem::replace(&mut f.locals[r as usize], Value::Null);
                    let (_, _, inst) = self.interface_target_value(recv, &chunk.ifaces[s as usize].targets)?;
                    f.locals[r as usize] = Value::Codeunit(inst);
                }
                Op::Fault { k } => {
                    return Err(match &chunk.faults[k as usize] {
                        FaultKind::Unsupported(why) => Signal::Unsupported(why.clone()),
                        FaultKind::Error { message, code } => Signal::Error(Box::new(al_builtins::native::AlError {
                            message: message.clone(),
                            code: code.clone(),
                            call_stack: String::new(),
                            ..Default::default()
                        })),
                    });
                }
                Op::DotNetClear => self.dotnet.clear_null_string(),
                Op::DotNetCoerce { d, a, t } => {
                    let v = take(f, a, nl);
                    let v = self.dotnet_coerce(v, &chunk.types[t as usize])?;
                    set(&mut f.locals[d as usize], v);
                }
                Op::DotNetOperand { d, a, h } => {
                    let v = take(f, a, nl);
                    let v = self.settle_operand(v, h)?;
                    set(&mut f.locals[d as usize], v);
                }
                Op::Special { s, d } => {
                    let v = self.run_special(&chunk.specials[s as usize], f, nl)?;
                    set(&mut f.locals[d as usize], v);
                }
                Op::Assume { a } => al_builtins::assumptions::assume(a),
                Op::Jump { t } => pc = t as usize,
                Op::JumpIfNot { a, t } => {
                    let c = ops::truthy(&f.locals[a as usize])?;
                    release(f, a, nl);
                    if !c {
                        pc = t as usize;
                    }
                }
                Op::CheckStop => self.check_stop()?,
                Op::SavePos { d } => {
                    f.locals[d as usize] = match self.call_stack.last() {
                        Some(top) => Value::Integer(top.pos as i32),
                        None => Value::Null,
                    };
                }
                Op::RestorePos { a } => {
                    if let Value::Integer(p) = f.locals[a as usize] {
                        self.restore_pos(Some(p as u32));
                    }
                }
                Op::Pos { p } => self.at_pos(p),
                Op::ForInit { s } => self.for_init(&chunk.fors[s as usize], f, nl)?,
                Op::ForTest { s, t } => {
                    let site = &chunk.fors[s as usize];
                    if let (Value::BigInteger(i), Value::BigInteger(e)) =
                        (&f.locals[site.i as usize], &f.locals[site.end as usize])
                    {
                        let go = if site.down { i >= e } else { i <= e };
                        if !go {
                            pc = t as usize;
                        }
                    }
                }
                Op::ForSet { s } => {
                    let site = &chunk.fors[s as usize];
                    match site.cap_loop {
                        None => self.for_set(site, f)?,
                        Some(r) => self.for_set_captured(site, r, f)?,
                    }
                }
                Op::ForNext { s } => self.for_next(&chunk.fors[s as usize], f)?,
                Op::Call { s, d } => {
                    let v = self.run_call(&chunk.calls[s as usize], f, nl)?;
                    set(&mut f.locals[d as usize], v);
                }
                Op::Builtin { s, d } => {
                    let v = self.run_builtin(&chunk.builtins[s as usize], f, nl)?;
                    set(&mut f.locals[d as usize], v);
                }
                Op::CharAt { d, a, b } => self.op_char_at(f, d, a, b, nl)?,
                Op::AppendText { s, old, a } => self.op_append(&chunk.appends[s as usize], f, old, a, nl)?,
                Op::JumpEq { a, b, t } => {
                    if ops::compare(&f.locals[a as usize], &f.locals[b as usize])? == std::cmp::Ordering::Equal {
                        pc = t as usize;
                    }
                }
                Op::JumpLt { a, b, t } => {
                    if ops::compare(&f.locals[a as usize], &f.locals[b as usize])? == std::cmp::Ordering::Less {
                        pc = t as usize;
                    }
                }
                Op::JumpGt { a, b, t } => {
                    if ops::compare(&f.locals[a as usize], &f.locals[b as usize])? == std::cmp::Ordering::Greater {
                        pc = t as usize;
                    }
                }
                Op::DotNet { s, d } => self.op_dotnet(&chunk.dotnets[s as usize], f, d, nl)?,
                Op::ThisInstance { d } => Self::op_this(f, d),
                Op::BaseInstance { d } => self.op_base(f, d)?,
                Op::Aux { s, d, a } => self.op_aux(&chunk.aux[s as usize], f, d, a, nl)?,
                Op::Assert { end: stop, from } => pc = self.op_assert(chunk, f, pc, stop as usize, from)?,
                Op::ForeachInit { s } => self.op_foreach_init(&chunk.foreachs[s as usize], f, nl)?,
                Op::ForeachNext { s, t } => {
                    if !self.op_foreach_next(&chunk.foreachs[s as usize], f)? {
                        pc = t as usize;
                    }
                }
                Op::IfaceCall { s, d } => {
                    let v = self.run_iface(&chunk.ifaces[s as usize], f, nl)?;
                    set(&mut f.locals[d as usize], v);
                }
                Op::Clear { a } => f.locals[a as usize] = Value::Null,
                Op::Ret => return Err(Signal::Exit(None)),
                Op::RetVal { a } => return Err(Signal::Exit(Some(take(f, a, nl)))),
                Op::RaiseBreak => return Err(Signal::Break),
                Op::RaiseContinue => return Err(Signal::Continue),
            }
        }
        Ok(pc)
    }

    /// `asserterror`: runs the body `pc..stop`; returns where the code goes on.
    #[inline(never)]
    fn op_assert(&mut self, chunk: &Chunk, f: &mut Frame, pc: usize, stop: usize, from: u16) -> Result<usize, Signal> {
        let nl = chunk.nlocals;
        let open = self.cap_stmt_depth();
        match self.run_range::<true>(chunk, f, pc, stop) {
            Ok(at) if at == stop => Err(Signal::Error(Box::new(al_builtins::native::AlError {
                message: "An error was expected inside an ASSERTERROR statement.".into(),
                code: "AssertError".into(),
                call_stack: String::new(),
                ..Default::default()
            }))),
            Ok(at) => Ok(at),
            Err(Signal::Error(mut e)) => {
                self.rollback_transaction();
                // The statements the error left open are closed.
                self.cap_stmt_truncate(open);
                self.add_top_frame(&mut e);
                self.last_error = *e;
                // Temporaries of the body that the error left behind hold nothing alive.
                for r in from.max(nl) as usize..f.locals.len() {
                    f.locals[r] = Value::Null;
                }
                Ok(stop)
            }
            Err(other) => Err(other),
        }
    }

    /// A typed instruction met an operand of another kind: the compiler's type was wrong (or a local held a value it should
    /// not). The result is the generic one; the count is in `AL_EXEC_REPORT`.
    #[cold]
    #[inline(never)]
    fn note_typed_fallback() {
        if counting() {
            STATS.typed_fallbacks.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The generic form of a typed binary instruction. `k_form`: `b` is the constant in `k` (an `AddIntK`).
    #[cold]
    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn typed_fallback(
        &self,
        op: ir::BinOp,
        k: &Value,
        d: u16,
        a: u16,
        b: u16,
        f: &mut Frame,
        nl: u16,
        k_form: bool,
    ) -> Result<(), Signal> {
        Self::note_typed_fallback();
        let v = if k_form {
            Self::binary_op(op, &f.locals[a as usize], k)?
        } else {
            Self::binary_op(op, &f.locals[a as usize], &f.locals[b as usize])?
        };
        release(f, a, nl);
        if !k_form {
            release(f, b, nl);
        }
        set(&mut f.locals[d as usize], v);
        Ok(())
    }

    #[inline(never)]
    fn run_special(&mut self, site: &SpecialSite, f: &mut Frame, nl: u16) -> Result<Value, Signal> {
        use al_builtins::builtin_id::ids;
        match site.id {
            ids::SESSION_BINDSUBSCRIPTION_1 | ids::SESSION_UNBINDSUBSCRIPTION_1 => {
                let cu = site.arg.map_or(Value::Null, |r| take(f, r, nl));
                self.subscription(site.id == ids::SESSION_BINDSUBSCRIPTION_1, cu, site.ret_used)
            }
            ids::RECORD_SETCURRFIELDNO_1 => {
                let rec = match &site.place {
                    Some(p) => self.read(p, f)?,
                    None => Value::Null,
                };
                let n = site.arg.map_or(Value::Null, |r| take(f, r, nl));
                self.set_curr_field_no(rec, n)
            }
            ids::RECORD_CURRFIELDNO_1 => {
                let rec = match &site.place {
                    Some(p) => self.read(p, f)?,
                    None => Value::Null,
                };
                self.curr_field_no(rec)
            }
            ids::SYSTEM_CLEARALL_1 => self.clear_all(f),
            _ => {
                let set = site.arg.map(|r| take(f, r, nl));
                self.application_area(set)
            }
        }
    }

    #[inline(never)]
    fn op_foreach_init(&mut self, site: &ForeachSite, f: &mut Frame, nl: u16) -> Result<(), Signal> {
        let coll = take(f, site.coll, nl);
        // A .NET collection, or a DotNet variable that holds none (ADR-024 section 4); its enumerator takes the place of
        // the items and the loop is not tracked (ADR-030).
        let dotnet_var = matches!(coll, Value::Null)
            && matches!(&site.collection, Some(ir::Expr::Load(p)) if matches!(self.place_type(p, f), al_builtins::value::RtType::DotNet(_)));
        if matches!(coll, Value::DotNet(_)) || dotnet_var {
            let en = self.foreach_dotnet_start(coll, site.collection.as_ref(), f)?;
            f.locals[site.items as usize] = en;
            if let Some(r) = site.cap_loop {
                f.locals[r as usize] = Value::Null;
            }
            return Ok(());
        }
        if let Value::List(l) = &coll {
            f.locals[site.ver as usize] = Value::BigInteger(l.borrow().version as i64);
            f.locals[site.watch as usize] = coll.clone();
        } else {
            f.locals[site.watch as usize] = Value::Null;
        }
        let items = Self::foreach_items(coll)?;
        f.locals[site.items as usize] = Value::Array(std::rc::Rc::new(std::cell::RefCell::new(items)));
        f.locals[site.at as usize] = Value::Integer(0);
        if let Some(r) = site.cap_loop {
            f.locals[r as usize] = match self.cap_loop_enter() {
                Some(n) => Value::Integer(n as i32),
                None => Value::Null,
            };
        }
        Ok(())
    }

    #[inline(never)]
    fn op_foreach_next(&mut self, site: &ForeachSite, f: &mut Frame) -> Result<bool, Signal> {
        if let Value::DotNet(_) = &f.locals[site.items as usize] {
            let en = f.locals[site.items as usize].clone();
            let Some(item) = self.foreach_dotnet_next(&site.var, &en, f)? else {
                return Ok(false);
            };
            self.write(&site.var, item, f)?;
            return Ok(true);
        }
        let Value::Integer(i) = f.locals[site.at as usize] else {
            return Err(Signal::Unsupported("a foreach loop that is not set up".into()));
        };
        if let (Value::List(l), Value::BigInteger(v)) = (&f.locals[site.watch as usize], &f.locals[site.ver as usize]) {
            if l.borrow().version as i64 != *v {
                return Err(Signal::Error(Box::new(al_builtins::native::AlError {
                    message: "Collection was modified; enumeration operation may not execute.".into(),
                    code: "AL:InvalidOperation".into(),
                    call_stack: String::new(),
                    ..Default::default()
                })));
            }
        }
        let item = match &f.locals[site.items as usize] {
            Value::Array(items) => items.borrow().get(i as usize).cloned(),
            _ => None,
        };
        let Some(item) = item else { return Ok(false) };
        f.locals[site.at as usize] = Value::Integer(i + 1);
        if let Some(r) = site.cap_loop {
            self.cap_loop_pass(cap_id(f, r));
        }
        self.write(&site.var, item, f)?;
        if site.cap_loop.is_some() {
            self.cap_loop_var(&site.var, f);
        }
        Ok(true)
    }

    #[inline(never)]
    fn run_iface(&mut self, site: &IfaceSite, f: &mut Frame, nl: u16) -> Result<Value, Signal> {
        let recv = take(f, site.recv, nl);
        let (object, routine, inst) = self.interface_target_value(recv, &site.targets)?;
        let mut values = Vec::with_capacity(site.arg_regs.len());
        for &r in &site.arg_regs {
            values.push(take(f, r, nl));
        }
        self.finish_call(object, routine, inst, &site.args, values, false, f)
    }

    #[inline(never)]
    fn op_char_at(&mut self, f: &mut Frame, d: u16, a: u16, b: u16, nl: u16) -> Result<(), Signal> {
        let v = {
            let Some(t) = al_builtins::native::text_of(&f.locals[a as usize]) else {
                return Err(Signal::Unsupported(format!(
                    "indexing {} as a text",
                    f.locals[a as usize].describe()
                )));
            };
            let i = crate::int(&f.locals[b as usize])?;
            Self::char_at(&t, i)?
        };
        release(f, a, nl);
        release(f, b, nl);
        set(&mut f.locals[d as usize], v);
        Ok(())
    }

    #[inline(never)]
    fn op_append(&mut self, site: &AppendSite, f: &mut Frame, old: u16, a: u16, nl: u16) -> Result<(), Signal> {
        let old = take(f, old, nl);
        let rv = take(f, a, nl);
        self.append_finish(&site.place, site.max, old, rv, f)
    }

    #[inline(never)]
    fn op_dotnet(&mut self, site: &DotNetSite, f: &mut Frame, d: u16, nl: u16) -> Result<(), Signal> {
        let v = self.run_dotnet(site, f, nl)?;
        set(&mut f.locals[d as usize], v);
        Ok(())
    }

    #[inline(never)]
    fn op_this(f: &mut Frame, d: u16) {
        set(&mut f.locals[d as usize], Value::Codeunit(f.instance.clone()));
    }

    #[inline(never)]
    fn op_base(&mut self, f: &mut Frame, d: u16) -> Result<(), Signal> {
        let v = Value::Codeunit(self.base_instance(f)?);
        set(&mut f.locals[d as usize], v);
        Ok(())
    }

    #[inline(never)]
    fn op_aux(&mut self, site: &AuxSite, f: &mut Frame, d: u16, a: u16, nl: u16) -> Result<(), Signal> {
        let v = take(f, a, nl);
        let r = match site {
            AuxSite::InterfaceTest { objects, cast } => self.interface_test(v, objects, *cast)?,
            AuxSite::EnumImpl { by_ordinal, unknown } => self.enum_implementation(v, by_ordinal, *unknown)?,
            AuxSite::Control { control, runtime } => self.page_control(v, *control, *runtime)?,
            // A field read off what a procedure returns is the field of the returned copy (corpus record/
            // lt-record-conversion-assumptions/RecordResultField.al).
            AuxSite::FieldOf { pos } => self.read_record_field(v, *pos)?,
            AuxSite::QueryColumn { column } => self.query_column(v, *column)?,
        };
        set(&mut f.locals[d as usize], r);
        Ok(())
    }

    #[inline(never)]
    fn for_init(&mut self, site: &ForSite, f: &mut Frame, nl: u16) -> Result<(), Signal> {
        let from = take(f, site.from, nl);
        let to = take(f, site.to, nl);
        let (start, end) = Self::for_limits(from, to)?;
        f.locals[site.i as usize] = Value::BigInteger(start);
        f.locals[site.end as usize] = Value::BigInteger(end);
        // A loop over an empty range still gives the counter the start value
        // (misc/lt-system-functions/ControlFlow.al:ForLoop_VariableAfterLoop: `for i := 5 to 4` leaves 5).
        if if site.down { start < end } else { start > end } {
            self.for_set(site, f)?;
        }
        Ok(())
    }

    /// `for_set` of a loop a capture tracks: the pass starts, the counter is set, and its value is a record.
    #[inline(never)]
    fn for_set_captured(&mut self, site: &ForSite, r: u16, f: &mut Frame) -> Result<(), Signal> {
        self.cap_loop_pass(cap_id(f, r));
        self.for_set(site, f)?;
        self.cap_loop_var(&site.var, f);
        Ok(())
    }

    #[inline(never)]
    fn for_set(&mut self, site: &ForSite, f: &mut Frame) -> Result<(), Signal> {
        let Value::BigInteger(i) = f.locals[site.i as usize] else {
            return Err(Signal::Unsupported("a for loop counter that is not set".into()));
        };
        if let (Some(v), Ok(n)) = (site.int_local, i32::try_from(i)) {
            set(&mut f.locals[v as usize], Value::Integer(n));
            return Ok(());
        }
        let slot_ty = self.place_type(&site.var, f);
        let kind = crate::interp::ForKind::of(&slot_ty);
        if kind.option_like {
            al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::ForOptionVariable);
        }
        let counter = Self::for_counter(kind, i, &slot_ty)?;
        self.write(&site.var, counter, f)
    }

    #[inline(never)]
    fn for_next(&mut self, site: &ForSite, f: &mut Frame) -> Result<(), Signal> {
        if let Some(v) = site.int_local {
            if let Value::Integer(n) = f.locals[v as usize] {
                f.locals[site.i as usize] = Value::BigInteger(i64::from(n) + if site.down { -1 } else { 1 });
                return Ok(());
            }
        }
        let back = Self::for_bound(self.read(&site.var, f)?);
        let i = crate::int(&back)?;
        f.locals[site.i as usize] = Value::BigInteger(i + if site.down { -1 } else { 1 });
        Ok(())
    }

    #[inline(never)]
    fn run_call(&mut self, site: &CallSite, f: &mut Frame, nl: u16) -> Result<Value, Signal> {
        let instance = site.instance.map(|r| take(f, r, nl));
        let mut values = Vec::with_capacity(site.arg_regs.len());
        for &r in &site.arg_regs {
            values.push(take(f, r, nl));
        }
        self.call_site(
            site.object,
            site.routine,
            instance,
            &site.args,
            values,
            site.try_call,
            f,
        )
    }

    #[inline(never)]
    fn run_dotnet(&mut self, site: &DotNetSite, f: &mut Frame, nl: u16) -> Result<Value, Signal> {
        let recv = match site.recv {
            Some(r) => take(f, r, nl),
            None => Value::Null,
        };
        let mut values = Vec::with_capacity(site.arg_regs.len());
        let mut places = Vec::with_capacity(site.arg_regs.len());
        for (&r, a) in site.arg_regs.iter().zip(&site.args) {
            values.push(take(f, r, nl));
            places.push(match a {
                ir::Arg::Ref(p) => Some(p.clone()),
                ir::Arg::Value(_) => None,
            });
        }
        self.dotnet_apply(site.ty, recv, values, places, &site.op, f)
    }

    #[inline(never)]
    fn run_builtin(&mut self, site: &BuiltinSite, f: &mut Frame, nl: u16) -> Result<Value, Signal> {
        let recv = site.receiver.map(|r| take(f, r, nl));
        let mut values = Vec::with_capacity(site.arg_regs.len());
        for &r in &site.arg_regs {
            values.push(take(f, r, nl));
        }
        if site.vm_class {
            return self.vm_class_call(site.id, site.class, recv, values, site.ret_used, f);
        }
        let Some(native) = site.native else {
            return Err(Signal::Unsupported(format!("builtin {}", site.id)));
        };
        if site.id == al_builtins::builtin_id::ids::SYSTEM_CLEAR_1 {
            // `Clear` of a Variant empties it, whatever it holds (types/lt-variant/VaTIs*.al:Is_Empty_Variant `V := 5; Clear(V)`
            // has no type flag left); the native clears a value to the default of its own kind.
            if let Some(ir::Arg::Ref(p)) = site.args.first() {
                if matches!(self.place_type(p, f), al_builtins::value::RtType::Variant) {
                    drop(values);
                    self.write(p, Value::Null, f)?;
                    return Ok(Value::Null);
                }
            }
        }
        self.run_native(
            native,
            site.class,
            site.receiver_arg.as_ref(),
            &site.args,
            recv,
            values,
            site.ret_used,
            f,
        )
    }
}
