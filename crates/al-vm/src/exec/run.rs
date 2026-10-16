//! The bytecode loop. Every instruction does what the matching tree-walker node does, through the same helpers.

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
                Op::Un { op, d, a } => {
                    let v = Self::unary_op(op, take(f, a, nl))?;
                    set(&mut f.locals[d as usize], v);
                }
                Op::Convert { d, a, t } => {
                    let v = ops::convert(take(f, a, nl), &chunk.types[t as usize])?;
                    set(&mut f.locals[d as usize], v);
                }
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
                Op::ForSet { s } => self.for_set(&chunk.fors[s as usize], f)?,
                Op::ForNext { s } => self.for_next(&chunk.fors[s as usize], f)?,
                Op::Call { s, d } => {
                    let v = self.run_call(&chunk.calls[s as usize], f, nl)?;
                    set(&mut f.locals[d as usize], v);
                }
                Op::Builtin { s, d } => {
                    let v = self.run_builtin(&chunk.builtins[s as usize], f, nl)?;
                    set(&mut f.locals[d as usize], v);
                }
                Op::TreeExpr { e, d } => {
                    if counting() {
                        STATS.tree_exprs_run.fetch_add(1, Ordering::Relaxed);
                        note_tree(&chunk.expr_kinds[e as usize], false);
                    }
                    let v = self.eval(&chunk.exprs[e as usize], f)?;
                    set(&mut f.locals[d as usize], v);
                }
                Op::TreeStmt { s, brk, cont } => {
                    if counting() {
                        STATS.tree_stmts_run.fetch_add(1, Ordering::Relaxed);
                        note_tree(chunk.stmt_kinds[s as usize], true);
                    }
                    match self.exec::<false>(&chunk.stmts[s as usize], f) {
                        Ok(()) => {}
                        Err(Signal::Break) if brk != NONE => pc = brk as usize,
                        Err(Signal::Continue) if cont != NONE => pc = cont as usize,
                        Err(e) => return Err(e),
                    }
                }
                Op::CharAt { d, a, b } => self.op_char_at(f, d, a, b, nl)?,
                Op::AppendText { s, old, a } => self.op_append(&chunk.appends[s as usize], f, old, a, nl)?,
                Op::MatchAny { a, l, t } => {
                    if !self.op_match(&chunk.labels[l as usize], f, a)? {
                        pc = t as usize;
                    }
                }
                Op::InSet { d, a, l } => self.op_in(&chunk.labels[l as usize], f, d, a, nl)?,
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
        match self.run_range::<true>(chunk, f, pc, stop) {
            Ok(at) if at == stop => Err(Signal::Error(Box::new(al_builtins::native::AlError {
                message: "An error was expected inside an ASSERTERROR statement.".into(),
                code: "AssertError".into(),
                call_stack: String::new(),
            }))),
            Ok(at) => Ok(at),
            Err(Signal::Error(mut e)) => {
                self.rollback_transaction();
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

    #[inline(never)]
    fn op_foreach_init(&mut self, site: &ForeachSite, f: &mut Frame, nl: u16) -> Result<(), Signal> {
        let coll = take(f, site.coll, nl);
        let items = Self::foreach_items(coll)?;
        f.locals[site.items as usize] = Value::Array(std::rc::Rc::new(std::cell::RefCell::new(items)));
        f.locals[site.at as usize] = Value::Integer(0);
        Ok(())
    }

    #[inline(never)]
    fn op_foreach_next(&mut self, site: &ForeachSite, f: &mut Frame) -> Result<bool, Signal> {
        let Value::Integer(i) = f.locals[site.at as usize] else {
            return Err(Signal::Unsupported("a foreach loop that is not set up".into()));
        };
        let item = match &f.locals[site.items as usize] {
            Value::Array(items) => items.borrow().get(i as usize).cloned(),
            _ => None,
        };
        let Some(item) = item else { return Ok(false) };
        f.locals[site.at as usize] = Value::Integer(i + 1);
        self.write(&site.var, item, f)?;
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
        self.finish_call(object, routine, inst, &site.args, Some(values), false, f)
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
    fn op_match(&mut self, labels: &[ir::SetElem], f: &mut Frame, a: u16) -> Result<bool, Signal> {
        let v = f.locals[a as usize].clone();
        self.matches_any(&v, labels, f)
    }

    #[inline(never)]
    fn op_in(&mut self, labels: &[ir::SetElem], f: &mut Frame, d: u16, a: u16, nl: u16) -> Result<(), Signal> {
        let v = take(f, a, nl);
        let hit = self.matches_any(&v, labels, f)?;
        set(&mut f.locals[d as usize], Value::Boolean(hit));
        Ok(())
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
            AuxSite::FieldOf { pos } => {
                al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::FieldOfProcedureResult);
                self.read_record_field(v, *pos)?
            }
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
            Some(values),
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
