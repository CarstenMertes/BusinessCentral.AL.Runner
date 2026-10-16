//! The static type of an IR expression, as far as the bytecode compiler may rely on it to pick a typed instruction.
//!
//! One interface, [`ExprTypes`]. Today its source is [`DeclaredTypes`]: the declared types of the locals, constants and
//! `Convert` results, combined by the rules of `ops::arith` and `ops::compare`. The checker already knows every
//! expression's type; when lowering (ADR-036) emits it, a second implementation of the trait replaces this one and
//! the instruction selection in `compile.rs` does not change. `None` means "not known": the compiler then emits the
//! generic instruction, so a missing type costs speed and never a result.

use al_builtins::value::RtType;
use al_ir as ir;

/// The types a typed instruction exists for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ty {
    Integer,
    Boolean,
}

pub(crate) trait ExprTypes<'a> {
    /// The type `e` always evaluates to, or `None` when that is not known.
    fn expr_ty(&self, e: &'a ir::Expr) -> Option<Ty>;
}

/// Types read from what the routine declares.
pub(crate) struct DeclaredTypes<'a> {
    pub locals: Vec<RtType>,
    /// The types lowering recorded (`al_ir::types`, ADR-036); consulted first. `Unknown` everywhere until lowering fills it.
    pub lowered: al_ir::types::ExprTypes<'a>,
}

/// The typed-instruction type of a declared type, if it has one.
pub(crate) fn of_rt(t: &RtType) -> Option<Ty> {
    match t {
        RtType::Integer => Some(Ty::Integer),
        RtType::Boolean => Some(Ty::Boolean),
        _ => None,
    }
}

impl<'a> ExprTypes<'a> for DeclaredTypes<'a> {
    fn expr_ty(&self, e: &'a ir::Expr) -> Option<Ty> {
        use al_ir::types::ExprTy;
        match self.lowered.of(e) {
            ExprTy::Integer => return Some(Ty::Integer),
            ExprTy::Boolean => return Some(Ty::Boolean),
            ExprTy::Unknown => {}
            // Known, and not a type that has a typed instruction.
            _ => return None,
        }
        use ir::BinOp as B;
        use ir::Expr as E;
        match e {
            E::Const(ir::Const::Integer(_)) => Some(Ty::Integer),
            E::Const(ir::Const::Boolean(_)) => Some(Ty::Boolean),
            E::Load(ir::Place::Local(i)) => self.locals.get(*i).and_then(of_rt),
            E::Convert(t, _) => of_rt(t),
            E::Unary(ir::UnOp::Not, a) if self.expr_ty(a) == Some(Ty::Boolean) => Some(Ty::Boolean),
            E::Binary(op, l, r) => match (self.expr_ty(l)?, self.expr_ty(r)?) {
                (Ty::Integer, Ty::Integer) => match op {
                    B::Add | B::Sub | B::Mul | B::IntDiv | B::Mod => Some(Ty::Integer),
                    B::Eq | B::Ne | B::Lt | B::Le | B::Gt | B::Ge => Some(Ty::Boolean),
                    _ => None,
                },
                (Ty::Boolean, Ty::Boolean) => match op {
                    B::And | B::Or | B::Xor => Some(Ty::Boolean),
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        }
    }
}
