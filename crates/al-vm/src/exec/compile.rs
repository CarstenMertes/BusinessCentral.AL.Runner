//! IR to bytecode. Returns `None` only when the routine needs more registers than an instruction can name.

use super::*;
use al_builtins::builtin_id::{Class, ids};
use al_builtins::value::RtType;
use types::{DeclaredTypes, ExprTypes, Ty, of_rt};

/// Compiles `r`.
pub(crate) fn compile(r: &ir::Routine) -> Option<Chunk> {
    let nlocals = u16::try_from(r.locals.len()).ok()?;
    let mut c = Compiler {
        locals_ty: r.locals.iter().map(|l| l.ty.clone()).collect(),
        tys: DeclaredTypes {
            locals: r.locals.iter().map(|l| l.ty.clone()).collect(),
        },
        chunk: Chunk {
            code: Vec::new(),
            consts: Vec::new(),
            places: Vec::new(),
            types: Vec::new(),
            exprs: Vec::new(),
            stmts: Vec::new(),
            expr_kinds: Vec::new(),
            stmt_kinds: Vec::new(),
            calls: Vec::new(),
            builtins: Vec::new(),
            fors: Vec::new(),
            labels: Vec::new(),
            appends: Vec::new(),
            dotnets: Vec::new(),
            aux: Vec::new(),
            foreachs: Vec::new(),
            ifaces: Vec::new(),
            nlocals,
            temps: 0,
            tree_nodes: 0,
        },
        next: nlocals,
        max: nlocals,
        loops: Vec::new(),
    };
    c.block(&r.body).ok()?;
    c.chunk.temps = c.max - nlocals;
    if std::env::var_os("AL_EXEC_DUMP").is_some() {
        eprintln!("routine {} ({} locals, {} temps)", r.name, nlocals, c.chunk.temps);
        for (i, op) in c.chunk.code.iter().enumerate() {
            eprintln!("  {i:3} {op:?}");
        }
    }
    Some(c.chunk)
}

struct LoopCtx {
    /// Jumps to patch to the loop's exit and to its next round.
    brk: Vec<usize>,
    cont: Vec<usize>,
}

struct Compiler {
    /// The declared type of each local.
    locals_ty: Vec<RtType>,
    /// Where the static types come from; the instruction selection asks only this.
    tys: DeclaredTypes,
    chunk: Chunk,
    next: u16,
    max: u16,
    loops: Vec<LoopCtx>,
}

type R<T> = Result<T, ()>;

/// Evaluating `e` cannot write a local or a global: no call, no builtin, no tree node.
fn pure(e: &ir::Expr) -> bool {
    use ir::Expr as E;
    match e {
        E::Const(_) | E::Load(ir::Place::Local(_) | ir::Place::Global(_)) => true,
        E::Unary(_, a) | E::Convert(_, a) => pure(a),
        E::Binary(_, a, b) => pure(a) && pure(b),
        _ => false,
    }
}

/// The builtins the tree walker handles itself, with operands of their own kind, before anything else; they stay on
/// the tree walker.
fn special(id: al_builtins::builtin_id::BuiltinId) -> bool {
    matches!(
        id,
        ids::SESSION_BINDSUBSCRIPTION_1
            | ids::SESSION_UNBINDSUBSCRIPTION_1
            | ids::RECORD_SETCURRFIELDNO_1
            | ids::RECORD_CURRFIELDNO_1
            | ids::SYSTEM_CLEARALL_1
            | ids::SESSION_APPLICATIONAREA_1
    )
}

/// The builtins the VM runs itself on evaluated operands (`Session::vm_class_call`).
fn vm_class(id: al_builtins::builtin_id::BuiltinId) -> bool {
    let class = id.class();
    class != Class::Other && class != Class::InStreamRead
        || matches!(id, ids::REQUESTPAGE_UPDATE_1 | ids::DATABASE_ISINWRITETRANSACTION_1)
}

/// An expression that cannot fail and writes nothing.
fn simple(e: &ir::Expr) -> bool {
    matches!(
        e,
        ir::Expr::Const(_) | ir::Expr::Load(ir::Place::Local(_) | ir::Place::Global(_))
    )
}

/// A copy of an argument list with the value expressions replaced by a constant: after a call only the places of
/// by-reference arguments are used.
fn blank(args: &[ir::Arg]) -> Vec<ir::Arg> {
    args.iter()
        .map(|a| match a {
            ir::Arg::Value(_) => ir::Arg::Value(ir::Expr::Const(ir::Const::Integer(0))),
            ir::Arg::Ref(p) => ir::Arg::Ref(p.clone()),
        })
        .collect()
}

impl Compiler {
    fn emit(&mut self, op: Op) -> usize {
        self.chunk.code.push(op);
        self.chunk.code.len() - 1
    }

    fn here(&self) -> u32 {
        self.chunk.code.len() as u32
    }

    fn patch(&mut self, at: usize, to: u32) {
        match &mut self.chunk.code[at] {
            Op::Jump { t }
            | Op::JumpIfNot { t, .. }
            | Op::JumpIfNotBool { t, .. }
            | Op::JumpUnlessEqInt { t, .. }
            | Op::JumpUnlessNeInt { t, .. }
            | Op::JumpUnlessLtInt { t, .. }
            | Op::JumpUnlessLeInt { t, .. }
            | Op::JumpUnlessGtInt { t, .. }
            | Op::JumpUnlessGeInt { t, .. }
            | Op::MatchAny { t, .. }
            | Op::ForeachNext { t, .. }
            | Op::ForTest { t, .. } => *t = to,
            Op::TreeStmt { .. } => unreachable!("tree statements are patched through patch_tree"),
            other => unreachable!("not a jump: {other:?}"),
        }
    }

    fn alloc(&mut self) -> R<u16> {
        let r = self.next;
        self.next = self.next.checked_add(1).ok_or(())?;
        self.max = self.max.max(self.next);
        Ok(r)
    }

    fn k(&mut self, c: &ir::Const) -> u32 {
        self.chunk.consts.push(c.clone());
        (self.chunk.consts.len() - 1) as u32
    }

    fn place(&mut self, p: &ir::Place) -> u32 {
        self.chunk.places.push(p.clone());
        (self.chunk.places.len() - 1) as u32
    }

    fn is_local(&self, r: u16) -> bool {
        r < self.chunk.nlocals
    }

    // ---- expressions ----

    /// The register that holds the value of `e`, which may be a local itself: the caller must not let anything write
    /// that local before it reads the register.
    fn expr_any(&mut self, e: &ir::Expr) -> R<u16> {
        if let ir::Expr::Load(ir::Place::Local(i)) = e {
            return u16::try_from(*i).map_err(|_| ());
        }
        let d = self.alloc()?;
        self.expr_into(e, d)?;
        Ok(d)
    }

    /// The register that holds a copy of the value of `e` (a temporary), safe against later writes to locals.
    fn expr_copy(&mut self, e: &ir::Expr) -> R<u16> {
        let r = self.expr_any(e)?;
        if self.is_local(r) {
            let d = self.alloc()?;
            self.emit(Op::Move { d, a: r });
            Ok(d)
        } else {
            Ok(r)
        }
    }

    /// Operands `a` then `b`: `a` must be a copy when evaluating `b` could write a local.
    fn pair(&mut self, a: &ir::Expr, b: &ir::Expr) -> R<(u16, u16)> {
        let ra = if pure(b) { self.expr_any(a)? } else { self.expr_copy(a)? };
        let rb = self.expr_any(b)?;
        Ok((ra, rb))
    }

    fn aux(&mut self, site: AuxSite, d: u16, a: u16) {
        self.chunk.aux.push(site);
        let s = (self.chunk.aux.len() - 1) as u32;
        self.emit(Op::Aux { s, d, a });
    }

    fn tree_expr(&mut self, e: &ir::Expr, d: u16) {
        self.chunk.exprs.push(e.clone());
        self.chunk.expr_kinds.push(match e {
            ir::Expr::Builtin { id, .. } => format!("Builtin {id}"),
            other => expr_kind(other).to_string(),
        });
        let idx = (self.chunk.exprs.len() - 1) as u32;
        self.chunk.tree_nodes += 1;
        self.emit(Op::TreeExpr { e: idx, d });
    }

    /// Emits the typed instruction for `l op r` when both operands are proven Integer (or both Boolean) and one exists.
    fn typed_binary(&mut self, op: ir::BinOp, l: &ir::Expr, r: &ir::Expr, d: u16) -> R<bool> {
        use ir::BinOp as B;
        let (Some(tl), Some(tr)) = (self.tys.expr_ty(l), self.tys.expr_ty(r)) else {
            return Ok(false);
        };
        if (tl, tr) == (Ty::Boolean, Ty::Boolean) {
            if !matches!(op, B::And | B::Or | B::Xor) {
                return Ok(false);
            }
            let (a, b) = self.pair(l, r)?;
            self.emit(match op {
                B::And => Op::AndBool { d, a, b },
                B::Or => Op::OrBool { d, a, b },
                _ => Op::XorBool { d, a, b },
            });
            return Ok(true);
        }
        if (tl, tr) != (Ty::Integer, Ty::Integer) {
            return Ok(false);
        }
        if matches!(op, B::Add | B::Sub) {
            if let ir::Expr::Const(ir::Const::Integer(k)) = r {
                if op == B::Add || *k != i32::MIN {
                    let a = self.expr_any(l)?;
                    let k = if op == B::Add { *k } else { -*k };
                    self.emit(Op::AddIntK { d, a, k });
                    return Ok(true);
                }
            }
        }
        if !matches!(
            op,
            B::Add | B::Sub | B::Mul | B::IntDiv | B::Mod | B::Eq | B::Ne | B::Lt | B::Le | B::Gt | B::Ge
        ) {
            return Ok(false);
        }
        let (a, b) = self.pair(l, r)?;
        self.emit(match op {
            B::Add => Op::AddInt { d, a, b },
            B::Sub => Op::SubInt { d, a, b },
            B::Mul => Op::MulInt { d, a, b },
            B::IntDiv => Op::DivInt { d, a, b },
            B::Mod => Op::ModInt { d, a, b },
            B::Eq => Op::EqInt { d, a, b },
            B::Ne => Op::NeInt { d, a, b },
            B::Lt => Op::LtInt { d, a, b },
            B::Le => Op::LeInt { d, a, b },
            B::Gt => Op::GtInt { d, a, b },
            _ => Op::GeInt { d, a, b },
        });
        Ok(true)
    }

    /// Evaluates the condition `c` and emits the jump taken when it is false, to be patched; returns the jump's index.
    /// An Integer compare is fused with the jump.
    fn cond_jump(&mut self, c: &ir::Expr, t: u32) -> R<usize> {
        use ir::BinOp as B;
        if let ir::Expr::Binary(op @ (B::Eq | B::Ne | B::Lt | B::Le | B::Gt | B::Ge), l, r) = c {
            if (self.tys.expr_ty(l), self.tys.expr_ty(r)) == (Some(Ty::Integer), Some(Ty::Integer)) {
                let (a, b) = self.pair(l, r)?;
                return Ok(self.emit(match op {
                    B::Eq => Op::JumpUnlessEqInt { a, b, t },
                    B::Ne => Op::JumpUnlessNeInt { a, b, t },
                    B::Lt => Op::JumpUnlessLtInt { a, b, t },
                    B::Le => Op::JumpUnlessLeInt { a, b, t },
                    B::Gt => Op::JumpUnlessGtInt { a, b, t },
                    _ => Op::JumpUnlessGeInt { a, b, t },
                }));
            }
        }
        let boolean = self.tys.expr_ty(c) == Some(Ty::Boolean);
        let a = self.expr_any(c)?;
        Ok(self.emit(if boolean {
            Op::JumpIfNotBool { a, t }
        } else {
            Op::JumpIfNot { a, t }
        }))
    }

    fn expr_into(&mut self, e: &ir::Expr, d: u16) -> R<()> {
        use ir::Expr as E;
        let mark = self.next;
        match e {
            E::Const(ir::Const::Integer(v)) => {
                self.emit(Op::ConstI { d, v: *v });
            }
            E::Const(c) => {
                let k = self.k(c);
                self.emit(Op::Const { d, k });
            }
            E::Load(ir::Place::Local(i)) => {
                let a = u16::try_from(*i).map_err(|_| ())?;
                self.emit(Op::Move { d, a });
            }
            E::Load(ir::Place::Global(i)) => {
                self.emit(Op::LoadGlobal { d, i: *i as u32 });
            }
            E::Load(p) => {
                let p = self.place(p);
                self.emit(Op::ReadPlace { d, p });
            }
            E::Unary(ir::UnOp::Not, a) if self.tys.expr_ty(a) == Some(Ty::Boolean) => {
                let a = self.expr_any(a)?;
                self.emit(Op::NotBool { d, a });
            }
            E::Unary(op, a) => {
                let a = self.expr_any(a)?;
                self.emit(Op::Un { op: *op, d, a });
            }
            E::Binary(op, l, r) if self.typed_binary(*op, l, r, d)? => {}
            E::Binary(op, l, r) => {
                let (a, b) = self.pair(l, r)?;
                use ir::BinOp as B;
                if matches!(
                    op,
                    B::Add | B::Sub | B::Mul | B::Mod | B::Eq | B::Ne | B::Lt | B::Le | B::Gt | B::Ge
                ) {
                    self.emit(Op::BinI { op: *op, d, a, b });
                } else {
                    self.emit(Op::Bin { op: *op, d, a, b });
                }
            }
            // A conversion to the type the operand is proven to have changes nothing.
            E::Convert(t, a) if of_rt(t).is_some() && of_rt(t) == self.tys.expr_ty(a) => {
                self.expr_into(a, d)?;
            }
            E::Convert(t, a) => {
                let a = self.expr_any(a)?;
                self.chunk.types.push(t.clone());
                let t = (self.chunk.types.len() - 1) as u32;
                self.emit(Op::Convert { d, a, t });
            }
            E::Call {
                object,
                routine,
                instance,
                args,
                try_call,
            } => {
                let instance = match instance {
                    Some(i) => Some(self.expr_copy(i)?),
                    None => None,
                };
                let arg_regs = self.arg_regs(args)?;
                self.chunk.calls.push(CallSite {
                    object: *object,
                    routine: *routine,
                    instance,
                    arg_regs,
                    args: blank(args),
                    try_call: *try_call,
                });
                let s = (self.chunk.calls.len() - 1) as u32;
                self.emit(Op::Call { s, d });
            }
            E::Builtin {
                id,
                receiver,
                args,
                ret_used,
            } if !special(*id) && (vm_class(*id) || al_builtins::native::by_id(*id).is_some()) => {
                let vm = vm_class(*id);
                let native = if vm { None } else { al_builtins::native::by_id(*id) };
                let n = args.len();
                let mut regs = Vec::with_capacity(n + 1);
                let mut all: Vec<&ir::Arg> = Vec::with_capacity(n + 1);
                if let Some(r) = receiver.as_deref() {
                    all.push(r);
                }
                all.extend(args.iter());
                for (i, a) in all.iter().enumerate() {
                    let rest_pure = all[i + 1..].iter().all(|b| arg_pure(b));
                    regs.push(self.arg_reg(a, rest_pure)?);
                }
                let receiver_reg = if receiver.is_some() { Some(regs.remove(0)) } else { None };
                self.chunk.builtins.push(BuiltinSite {
                    id: *id,
                    vm_class: vm,
                    native,
                    class: id.class(),
                    receiver: receiver_reg,
                    receiver_arg: receiver.as_deref().map(|a| blank(std::slice::from_ref(a)).remove(0)),
                    arg_regs: regs,
                    args: blank(args),
                    ret_used: *ret_used,
                });
                let s = (self.chunk.builtins.len() - 1) as u32;
                self.emit(Op::Builtin { s, d });
            }
            E::CharAt(text, index) => {
                let (a, b) = self.pair(text, index)?;
                self.emit(Op::CharAt { d, a, b });
            }
            E::In(v, set) => {
                let a = self.expr_copy(v)?;
                self.chunk.labels.push(set.clone());
                let l = (self.chunk.labels.len() - 1) as u32;
                self.emit(Op::InSet { d, a, l });
            }
            E::DotNet { ty, receiver, op, args } => {
                let recv = match receiver.as_deref() {
                    Some(r) => Some(self.expr_copy(r)?),
                    None => None,
                };
                let arg_regs = self.arg_regs(args)?;
                self.chunk.dotnets.push(DotNetSite {
                    ty: *ty,
                    op: op.clone(),
                    recv,
                    arg_regs,
                    args: blank(args),
                });
                let s = (self.chunk.dotnets.len() - 1) as u32;
                self.emit(Op::DotNet { s, d });
            }
            E::InterfaceCall {
                receiver,
                targets,
                args,
            } if args.iter().all(|a| {
                matches!(a, ir::Arg::Value(v) if simple(v))
                    || matches!(a, ir::Arg::Ref(ir::Place::Local(_) | ir::Place::Global(_)))
            }) =>
            {
                // The target is resolved before the arguments are evaluated; plain operands cannot tell the order.
                let recv = self.expr_copy(receiver)?;
                let arg_regs = self.arg_regs(args)?;
                self.chunk.ifaces.push(IfaceSite {
                    targets: targets.clone(),
                    recv,
                    arg_regs,
                    args: blank(args),
                });
                let s = (self.chunk.ifaces.len() - 1) as u32;
                self.emit(Op::IfaceCall { s, d });
            }
            E::ThisInstance => {
                self.emit(Op::ThisInstance { d });
            }
            E::BaseInstance => {
                self.emit(Op::BaseInstance { d });
            }
            E::Ternary(c, a, b) => {
                let jf = self.cond_jump(c, 0)?;
                self.expr_into(a, d)?;
                let jend = self.emit(Op::Jump { t: 0 });
                let else_at = self.here();
                self.patch(jf, else_at);
                self.expr_into(b, d)?;
                let end = self.here();
                self.patch(jend, end);
            }
            E::InterfaceTest { value, objects, cast } => {
                let a = self.expr_any(value)?;
                self.aux(
                    AuxSite::InterfaceTest {
                        objects: objects.clone(),
                        cast: *cast,
                    },
                    d,
                    a,
                );
            }
            E::EnumImpl {
                value,
                by_ordinal,
                unknown,
            } => {
                let a = self.expr_any(value)?;
                self.aux(
                    AuxSite::EnumImpl {
                        by_ordinal: by_ordinal.clone(),
                        unknown: *unknown,
                    },
                    d,
                    a,
                );
            }
            E::Control { page, control, runtime } => {
                let a = self.expr_any(page)?;
                self.aux(
                    AuxSite::Control {
                        control: *control,
                        runtime: *runtime,
                    },
                    d,
                    a,
                );
            }
            E::FieldOf(base, pos) => {
                let a = self.expr_any(base)?;
                self.aux(AuxSite::FieldOf { pos: *pos }, d, a);
            }
            E::QueryColumn { query, column } => {
                let a = self.expr_any(query)?;
                self.aux(AuxSite::QueryColumn { column: *column }, d, a);
            }
            other => self.tree_expr(other, d),
        }
        // Temporaries used inside `e` are free again; `d` itself stays allocated by the caller.
        self.next = mark;
        Ok(())
    }

    fn arg_regs(&mut self, args: &[ir::Arg]) -> R<Vec<u16>> {
        let mut regs = Vec::with_capacity(args.len());
        for (i, a) in args.iter().enumerate() {
            let rest_pure = args[i + 1..].iter().all(arg_pure);
            regs.push(self.arg_reg(a, rest_pure)?);
        }
        Ok(regs)
    }

    /// The register of one call or builtin argument. A local is used in place when the arguments after it cannot write
    /// it; the register stays allocated until the instruction runs (the caller resets the temporaries per statement).
    fn arg_reg(&mut self, a: &ir::Arg, rest_pure: bool) -> R<u16> {
        match a {
            ir::Arg::Value(e) => {
                if rest_pure {
                    let r = self.expr_any(e)?;
                    self.keep(r);
                    Ok(r)
                } else {
                    let r = self.expr_copy(e)?;
                    self.keep(r);
                    Ok(r)
                }
            }
            ir::Arg::Ref(ir::Place::Local(i)) if rest_pure => u16::try_from(*i).map_err(|_| ()),
            ir::Arg::Ref(p) => {
                let d = self.alloc()?;
                let p = self.place(p);
                self.emit(Op::ReadPlace { d, p });
                Ok(d)
            }
        }
    }

    /// Keeps a temporary allocated past the end of the expression that made it.
    fn keep(&mut self, r: u16) {
        if !self.is_local(r) && r >= self.next {
            self.next = r + 1;
            self.max = self.max.max(self.next);
        }
    }

    // ---- statements ----

    fn block(&mut self, stmts: &[ir::Stmt]) -> R<()> {
        for s in stmts {
            let mark = self.next;
            self.stmt(s)?;
            self.next = mark;
        }
        Ok(())
    }

    fn tree_stmt(&mut self, s: &ir::Stmt) {
        self.chunk.stmts.push(s.clone());
        self.chunk.stmt_kinds.push(stmt_kind(s));
        let idx = (self.chunk.stmts.len() - 1) as u32;
        self.chunk.tree_nodes += 1;
        let at = self.emit(Op::TreeStmt {
            s: idx,
            brk: NONE,
            cont: NONE,
        });
        if let Some(l) = self.loops.last_mut() {
            // Patched with the loop's targets when it ends.
            l.brk.push(at | TREE_BRK);
            l.cont.push(at | TREE_CONT);
        }
    }

    fn patch_all(&mut self, list: Vec<usize>, to: u32) {
        for at in list {
            if at & TREE_BRK != 0 || at & TREE_CONT != 0 {
                let i = at & !(TREE_BRK | TREE_CONT);
                if let Op::TreeStmt { brk, cont, .. } = &mut self.chunk.code[i] {
                    if at & TREE_BRK != 0 {
                        *brk = to;
                    } else {
                        *cont = to;
                    }
                }
            } else {
                self.patch(at, to);
            }
        }
    }

    fn stmt(&mut self, s: &ir::Stmt) -> R<()> {
        use ir::Stmt as S;
        match s {
            S::Pos(p) => {
                self.emit(Op::Pos { p: *p });
            }
            S::Assign(p, e) => self.assign(p, e)?,
            S::Expr(e) => {
                let d = self.alloc()?;
                self.expr_into(e, d)?;
                self.emit(Op::Clear { a: d });
            }
            S::If(c, t, e) => {
                let jf = self.cond_jump(c, 0)?;
                self.block(t)?;
                if e.is_empty() {
                    let end = self.here();
                    self.patch(jf, end);
                } else {
                    let jend = self.emit(Op::Jump { t: 0 });
                    let else_at = self.here();
                    self.patch(jf, else_at);
                    self.block(e)?;
                    let end = self.here();
                    self.patch(jend, end);
                }
            }
            S::While(c, body) => {
                let saved = self.alloc()?;
                self.emit(Op::SavePos { d: saved });
                let top = self.here();
                let mark = self.next;
                let jf = self.cond_jump(c, 0)?;
                self.next = mark;
                self.emit(Op::CheckStop);
                self.loops.push(LoopCtx {
                    brk: Vec::new(),
                    cont: Vec::new(),
                });
                self.block(body)?;
                let l = self.loops.pop().ok_or(())?;
                let cont = self.here();
                self.emit(Op::RestorePos { a: saved });
                self.emit(Op::Jump { t: top });
                let end = self.here();
                self.patch(jf, end);
                self.patch_all(l.cont, cont);
                self.patch_all(l.brk, end);
            }
            S::Repeat(body, until) => {
                let saved = self.alloc()?;
                self.emit(Op::SavePos { d: saved });
                let top = self.here();
                self.emit(Op::CheckStop);
                self.loops.push(LoopCtx {
                    brk: Vec::new(),
                    cont: Vec::new(),
                });
                self.block(body)?;
                let l = self.loops.pop().ok_or(())?;
                let cont = self.here();
                self.emit(Op::RestorePos { a: saved });
                let mark = self.next;
                self.cond_jump(until, top)?;
                self.next = mark;
                let end = self.here();
                self.patch_all(l.cont, cont);
                self.patch_all(l.brk, end);
            }
            S::For {
                var,
                from,
                to,
                down,
                body,
            } => {
                let i = self.alloc()?;
                let end_reg = self.alloc()?;
                let (fr, tr) = self.pair(from, to)?;
                let int_local = match var {
                    ir::Place::Local(v) if matches!(self.locals_ty.get(*v), Some(RtType::Integer)) => {
                        u16::try_from(*v).ok()
                    }
                    _ => None,
                };
                self.chunk.fors.push(ForSite {
                    var: var.clone(),
                    down: *down,
                    from: fr,
                    to: tr,
                    i,
                    end: end_reg,
                    int_local,
                });
                let site = (self.chunk.fors.len() - 1) as u32;
                self.emit(Op::ForInit { s: site });
                let top = self.here();
                let test = self.emit(Op::ForTest { s: site, t: 0 });
                self.emit(Op::CheckStop);
                self.emit(Op::ForSet { s: site });
                self.loops.push(LoopCtx {
                    brk: Vec::new(),
                    cont: Vec::new(),
                });
                let mark = self.next;
                self.block(body)?;
                self.next = mark;
                let l = self.loops.pop().ok_or(())?;
                let cont = self.here();
                self.emit(Op::ForNext { s: site });
                self.emit(Op::Jump { t: top });
                let end = self.here();
                self.patch(test, end);
                self.patch_all(l.cont, cont);
                self.patch_all(l.brk, end);
            }
            S::Exit(None) => {
                self.emit(Op::Ret);
            }
            S::Exit(Some(e)) => {
                let a = self.expr_any(e)?;
                self.emit(Op::RetVal { a });
            }
            S::Break => match self.loops.last_mut() {
                Some(_) => {
                    let at = self.emit(Op::Jump { t: 0 });
                    if let Some(l) = self.loops.last_mut() {
                        l.brk.push(at);
                    }
                }
                None => {
                    self.emit(Op::RaiseBreak);
                }
            },
            S::Continue => match self.loops.last_mut() {
                Some(_) => {
                    let at = self.emit(Op::Jump { t: 0 });
                    if let Some(l) = self.loops.last_mut() {
                        l.cont.push(at);
                    }
                }
                None => {
                    self.emit(Op::RaiseContinue);
                }
            },
            S::Case {
                scrutinee,
                branches,
                otherwise,
            } => {
                let a = self.expr_copy(scrutinee)?;
                let mut ends = Vec::new();
                for (labels, body) in branches {
                    self.chunk.labels.push(labels.clone());
                    let l = (self.chunk.labels.len() - 1) as u32;
                    let miss = self.emit(Op::MatchAny { a, l, t: 0 });
                    self.emit(Op::Clear { a });
                    self.block(body)?;
                    ends.push(self.emit(Op::Jump { t: 0 }));
                    let next = self.here();
                    self.patch(miss, next);
                }
                self.emit(Op::Clear { a });
                self.block(otherwise)?;
                let end = self.here();
                for j in ends {
                    self.patch(j, end);
                }
            }
            S::AssertError(body) => {
                let at = self.emit(Op::Assert {
                    end: 0,
                    from: self.next,
                });
                self.block(body)?;
                let end = self.here();
                if let Op::Assert { end: e, .. } = &mut self.chunk.code[at] {
                    *e = end;
                }
            }
            S::Foreach { var, collection, body }
                if match collection {
                    ir::Expr::Load(ir::Place::Local(i)) => {
                        matches!(self.locals_ty.get(*i), Some(RtType::List(_) | RtType::Library(_)))
                    }
                    // A built-in method returns a library value (XmlElement.GetChildNodes, JsonArray ...), never a .NET one.
                    ir::Expr::Builtin { .. } => true,
                    _ => false,
                } =>
            {
                let coll = self.expr_copy(collection)?;
                let items = self.alloc()?;
                let at = self.alloc()?;
                self.chunk.foreachs.push(ForeachSite {
                    var: var.clone(),
                    coll,
                    items,
                    at,
                });
                let site = (self.chunk.foreachs.len() - 1) as u32;
                self.emit(Op::ForeachInit { s: site });
                let top = self.here();
                let next = self.emit(Op::ForeachNext { s: site, t: 0 });
                self.loops.push(LoopCtx {
                    brk: Vec::new(),
                    cont: Vec::new(),
                });
                let mark = self.next;
                self.block(body)?;
                self.next = mark;
                let l = self.loops.pop().ok_or(())?;
                self.emit(Op::Jump { t: top });
                let end = self.here();
                self.emit(Op::Clear { a: items });
                self.patch(next, end);
                self.patch_all(l.cont, top);
                self.patch_all(l.brk, end);
            }
            S::Foreach { .. } => self.tree_stmt(s),
            S::Assume(_) => self.tree_stmt(s),
        }
        Ok(())
    }

    fn assign(&mut self, p: &ir::Place, e: &ir::Expr) -> R<()> {
        // `S += x` and `S := S + x` on a text variable append in place: the tree walker's routine does it.
        if let (ir::Place::Local(_) | ir::Place::Global(_), ir::Expr::Convert(RtType::Text(_), inner)) = (p, e) {
            if let ir::Expr::Binary(ir::BinOp::Add, l, _) = &**inner {
                if matches!(&**l, ir::Expr::Load(lp) if lp == p) {
                    let ir::Expr::Convert(RtType::Text(max), inner) = e else {
                        unreachable!("matched above")
                    };
                    let ir::Expr::Binary(_, _, r) = &**inner else {
                        unreachable!("matched above")
                    };
                    // The old value is read before `x` is evaluated, as the tree walker does.
                    let old = self.alloc()?;
                    let pi = self.place(p);
                    self.emit(Op::ReadPlace { d: old, p: pi });
                    let a = self.expr_any(r)?;
                    self.chunk.appends.push(AppendSite {
                        place: p.clone(),
                        max: *max,
                    });
                    let s = (self.chunk.appends.len() - 1) as u32;
                    self.emit(Op::AppendText { s, old, a });
                    return Ok(());
                }
            }
        }
        match p {
            // A local of a typed kind takes a proven value of that kind straight into its register.
            ir::Place::Local(i)
                if self.locals_ty.get(*i).and_then(of_rt).is_some_and(|t| {
                    let inner = if let ir::Expr::Convert(_, x) = e { x } else { e };
                    self.tys.expr_ty(inner) == Some(t)
                }) =>
            {
                let i = u16::try_from(*i).map_err(|_| ())?;
                let inner = if let ir::Expr::Convert(_, x) = e { x } else { e };
                self.expr_into(inner, i)?;
            }
            ir::Place::Local(i) => {
                let i = u16::try_from(*i).map_err(|_| ())?;
                let a = self.expr_any(e)?;
                let pi = self.place(p);
                self.emit(Op::StoreLocal { i, a, p: pi });
            }
            _ => {
                let a = self.expr_any(e)?;
                let pi = self.place(p);
                self.emit(Op::StorePlace { p: pi, a });
            }
        }
        Ok(())
    }
}

const TREE_BRK: usize = 1 << 40;
const TREE_CONT: usize = 1 << 41;

fn arg_pure(a: &ir::Arg) -> bool {
    match a {
        ir::Arg::Value(e) => pure(e),
        ir::Arg::Ref(ir::Place::Local(_) | ir::Place::Global(_)) => true,
        ir::Arg::Ref(_) => false,
    }
}

fn expr_kind(e: &ir::Expr) -> &'static str {
    use ir::Expr as E;
    match e {
        E::Const(_) => "Const",
        E::Load(_) => "Load",
        E::Unary(..) => "Unary",
        E::Binary(..) => "Binary",
        E::In(..) => "In",
        E::Ternary(..) => "Ternary",
        E::Convert(..) => "Convert",
        E::Call { .. } => "Call",
        E::InterfaceCall { .. } => "InterfaceCall",
        E::BaseInstance => "BaseInstance",
        E::ThisInstance => "ThisInstance",
        E::Refused(_) => "Refused",
        E::Fail { .. } => "Fail",
        E::InterfaceTest { .. } => "InterfaceTest",
        E::EnumImpl { .. } => "EnumImpl",
        E::Control { .. } => "Control",
        E::FieldOf(..) => "FieldOf",
        E::QueryColumn { .. } => "QueryColumn",
        E::DotNet { .. } => "DotNet",
        E::CharAt(..) => "CharAt",
        E::DotNetCoerce(..) => "DotNetCoerce",
        E::DotNetOperand(..) => "DotNetOperand",
        E::Builtin { .. } => "Builtin",
    }
}

fn stmt_kind(s: &ir::Stmt) -> &'static str {
    use ir::Stmt as S;
    match s {
        S::Assign(..) => "Assign",
        S::Expr(_) => "Expr",
        S::If(..) => "If",
        S::While(..) => "While",
        S::Repeat(..) => "Repeat",
        S::For { .. } => "For",
        S::Foreach { .. } => "Foreach",
        S::Case { .. } => "Case",
        S::Exit(_) => "Exit",
        S::Break => "Break",
        S::AssertError(_) => "AssertError",
        S::Pos(_) => "Pos",
        S::Assume(_) => "Assume",
        S::Continue => "Continue",
    }
}
