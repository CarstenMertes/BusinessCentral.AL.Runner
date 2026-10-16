//! The static type of each expression of a routine (ADR-036, step 0).
//!
//! Lowering records a type for the expressions it has converted and leaves the rest `Unknown`. A reader that gets
//! `Unknown` behaves exactly as it would without the table. The table is a `Vec<ExprTy>` on [`Routine`]: one
//! entry per [`Expr`] node of the body in post-order (children in field order, then the node; statements in order;
//! the expressions inside a [`Place`] and an [`Arg`] included). An empty table means every node is `Unknown`.
//! [`ExprTypes`] pairs the walk with the table and answers by the address of a node.

use crate::*;
use serde::{Deserialize, Serialize};

/// The static type of an expression. Small and `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ExprTy {
    /// Not recorded.
    #[default]
    Unknown,
    Integer,
    BigInteger,
    Decimal,
    Boolean,
    Char,
    Byte,
    Text,
    Code,
    Date,
    Time,
    DateTime,
    Duration,
    Guid,
    Option,
    Enum,
    Record,
    /// Known, and none of the above.
    Other,
}

impl ExprTy {
    /// The type of a slot of this runtime type.
    pub fn of_rt(t: &al_builtins::value::RtType) -> ExprTy {
        use al_builtins::value::RtType as R;
        match t {
            R::Boolean => ExprTy::Boolean,
            R::Integer => ExprTy::Integer,
            R::BigInteger => ExprTy::BigInteger,
            R::Decimal => ExprTy::Decimal,
            R::Byte => ExprTy::Byte,
            R::Char => ExprTy::Char,
            R::Text(_) => ExprTy::Text,
            R::Code(_) => ExprTy::Code,
            R::Date => ExprTy::Date,
            R::Time => ExprTy::Time,
            R::DateTime => ExprTy::DateTime,
            R::Duration => ExprTy::Duration,
            R::Guid => ExprTy::Guid,
            R::Option(_) => ExprTy::Option,
            R::Enum(_) => ExprTy::Enum,
            R::Record(..) => ExprTy::Record,
            R::Variant => ExprTy::Unknown,
            _ => ExprTy::Other,
        }
    }
}

/// Calls `f` on every expression of `body` in post-order (the order of the table).
pub fn for_each_expr_post(body: &[Stmt], f: &mut dyn FnMut(&Expr)) {
    for s in body {
        stmt(s, f);
    }
}

/// The number of expression nodes in `body`.
pub fn count_exprs(body: &[Stmt]) -> usize {
    let mut n = 0;
    for_each_expr_post(body, &mut |_| n += 1);
    n
}

/// The number of expression nodes in `e`, itself included.
pub fn count_expr(e: &Expr) -> usize {
    let mut n = 0;
    expr(e, &mut |_| n += 1);
    n
}

/// The number of expression nodes in `p`.
pub fn count_place(p: &Place) -> usize {
    let mut n = 0;
    place(p, &mut |_| n += 1);
    n
}

fn stmts(b: &[Stmt], f: &mut dyn FnMut(&Expr)) {
    for s in b {
        stmt(s, f);
    }
}

fn stmt(s: &Stmt, f: &mut dyn FnMut(&Expr)) {
    match s {
        Stmt::Assign(p, e) => {
            place(p, f);
            expr(e, f);
        }
        Stmt::Expr(e) => expr(e, f),
        Stmt::If(c, a, b) => {
            expr(c, f);
            stmts(a, f);
            stmts(b, f);
        }
        Stmt::While(c, b) => {
            expr(c, f);
            stmts(b, f);
        }
        Stmt::Repeat(b, c) => {
            stmts(b, f);
            expr(c, f);
        }
        Stmt::For {
            var,
            from,
            to,
            down: _,
            body,
        } => {
            place(var, f);
            expr(from, f);
            expr(to, f);
            stmts(body, f);
        }
        Stmt::Foreach { var, collection, body } => {
            place(var, f);
            expr(collection, f);
            stmts(body, f);
        }
        Stmt::Case {
            scrutinee,
            branches,
            otherwise,
        } => {
            expr(scrutinee, f);
            for (elems, body) in branches {
                for e in elems {
                    set_elem(e, f);
                }
                stmts(body, f);
            }
            stmts(otherwise, f);
        }
        Stmt::Exit(e) => {
            if let Some(e) = e {
                expr(e, f);
            }
        }
        Stmt::AssertError(b) => stmts(b, f),
        Stmt::Break | Stmt::Pos(_) | Stmt::Assume(_) | Stmt::Continue => {}
    }
}

fn set_elem(e: &SetElem, f: &mut dyn FnMut(&Expr)) {
    match e {
        SetElem::One(x) => expr(x, f),
        SetElem::Range(a, b) => {
            if let Some(a) = a {
                expr(a, f);
            }
            if let Some(b) = b {
                expr(b, f);
            }
        }
    }
}

fn place(p: &Place, f: &mut dyn FnMut(&Expr)) {
    match p {
        Place::Local(_) | Place::Global(_) | Place::BaseGlobal(_) => {}
        Place::Index(base, idx) => {
            place(base, f);
            for e in idx {
                expr(e, f);
            }
        }
        Place::Field(base, _) => place(base, f),
    }
}

fn arg(a: &Arg, f: &mut dyn FnMut(&Expr)) {
    match a {
        Arg::Value(e) => expr(e, f),
        Arg::Ref(p) => place(p, f),
    }
}

fn args(a: &[Arg], f: &mut dyn FnMut(&Expr)) {
    for x in a {
        arg(x, f);
    }
}

fn expr(e: &Expr, f: &mut dyn FnMut(&Expr)) {
    match e {
        Expr::Const(_) | Expr::BaseInstance | Expr::ThisInstance | Expr::Refused(_) | Expr::Fail { .. } => {}
        Expr::Load(p) => place(p, f),
        Expr::Unary(_, x) | Expr::Convert(_, x) | Expr::DotNetCoerce(_, x) | Expr::DotNetOperand(x, _) => expr(x, f),
        Expr::Binary(_, a, b) | Expr::CharAt(a, b) => {
            expr(a, f);
            expr(b, f);
        }
        Expr::In(x, set) => {
            expr(x, f);
            for s in set {
                set_elem(s, f);
            }
        }
        Expr::Ternary(c, a, b) => {
            expr(c, f);
            expr(a, f);
            expr(b, f);
        }
        Expr::Call {
            object: _,
            routine: _,
            instance,
            args: a,
            try_call: _,
        } => {
            if let Some(i) = instance {
                expr(i, f);
            }
            args(a, f);
        }
        Expr::InterfaceCall {
            receiver,
            targets: _,
            args: a,
        } => {
            expr(receiver, f);
            args(a, f);
        }
        Expr::InterfaceTest {
            value,
            objects: _,
            cast: _,
        } => expr(value, f),
        Expr::EnumImpl {
            value,
            by_ordinal: _,
            unknown: _,
        } => expr(value, f),
        Expr::Control {
            page,
            control: _,
            runtime: _,
        } => expr(page, f),
        Expr::FieldOf(x, _) => expr(x, f),
        Expr::QueryColumn { query, column: _ } => expr(query, f),
        Expr::DotNet {
            ty: _,
            receiver,
            op: _,
            args: a,
        } => {
            if let Some(r) = receiver {
                expr(r, f);
            }
            args(a, f);
        }
        Expr::Builtin {
            id: _,
            receiver,
            args: a,
            ret_used: _,
        } => {
            if let Some(r) = receiver {
                arg(r, f);
            }
            args(a, f);
        }
    }
    f(e);
}

/// The types of the expressions of one routine, found by the address of the node.
///
/// A node that is not part of the routine's body, or any node when the routine has no table (or one of the wrong
/// length), is `Unknown`. The routine must not change while an `ExprTypes` of it is in use.
pub struct ExprTypes {
    by_addr: std::collections::HashMap<usize, ExprTy>,
}

impl ExprTypes {
    /// One walk of the body.
    pub fn new(routine: &Routine) -> ExprTypes {
        let mut by_addr = std::collections::HashMap::new();
        let table = &routine.expr_tys;
        if !table.is_empty() {
            let mut i = 0;
            let mut found = Vec::new();
            for_each_expr_post(&routine.body, &mut |e| {
                found.push(e as *const Expr as usize);
                i += 1;
            });
            if table.len() == found.len() {
                by_addr.reserve(found.len());
                for (addr, ty) in found.into_iter().zip(table.iter()) {
                    if *ty != ExprTy::Unknown {
                        by_addr.insert(addr, *ty);
                    }
                }
            }
        }
        ExprTypes { by_addr }
    }

    /// The type of `e`, a node of the routine's body.
    pub fn of(&self, e: &Expr) -> ExprTy {
        self.by_addr
            .get(&(e as *const Expr as usize))
            .copied()
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn int(v: i32) -> Expr {
        Expr::Const(Const::Integer(v))
    }

    fn routine(body: Vec<Stmt>, expr_tys: Vec<ExprTy>) -> Routine {
        Routine {
            name: "R".into(),
            field: None,
            container: None,
            is_trigger: false,
            attributes: vec![],
            params: 0,
            by_var: vec![],
            locals: vec![],
            ret: None,
            ret_local: None,
            pos: NO_POS,
            end_pos: NO_POS,
            begin_pos: NO_POS,
            body,
            expr_tys,
        }
    }

    #[test]
    fn the_walk_is_post_order_and_counts_places_and_args() {
        let e = Expr::Binary(
            BinOp::Add,
            Box::new(int(1)),
            Box::new(Expr::Load(Place::Index(Box::new(Place::Local(0)), vec![int(2)]))),
        );
        let body = vec![Stmt::Assign(Place::Local(0), e)];
        let mut seen = Vec::new();
        for_each_expr_post(&body, &mut |e| {
            seen.push(match e {
                Expr::Const(_) => "const",
                Expr::Load(_) => "load",
                Expr::Binary(..) => "binary",
                _ => "other",
            })
        });
        assert_eq!(seen, ["const", "const", "load", "binary"]);
        assert_eq!(count_exprs(&body), 4);
    }

    #[test]
    fn a_table_is_found_by_node_and_a_table_of_the_wrong_length_is_ignored() {
        let body = vec![Stmt::Expr(Expr::Binary(BinOp::Add, Box::new(int(1)), Box::new(int(2))))];
        let r = routine(body, vec![ExprTy::Integer, ExprTy::Integer, ExprTy::Integer]);
        let t = ExprTypes::new(&r);
        let Stmt::Expr(Expr::Binary(_, a, _)) = &r.body[0] else {
            unreachable!()
        };
        assert_eq!(t.of(a), ExprTy::Integer);
        assert_eq!(t.of(&int(1)), ExprTy::Unknown, "a node of another tree");
        let short = routine(r.body.clone(), vec![ExprTy::Integer]);
        let Stmt::Expr(e) = &short.body[0] else { unreachable!() };
        assert_eq!(ExprTypes::new(&short).of(e), ExprTy::Unknown);
        let none = routine(r.body.clone(), vec![]);
        let Stmt::Expr(e) = &none.body[0] else { unreachable!() };
        assert_eq!(ExprTypes::new(&none).of(e), ExprTy::Unknown);
    }
}
