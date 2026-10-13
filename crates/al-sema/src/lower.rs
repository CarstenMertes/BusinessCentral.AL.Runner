//! Lowering resolved syntax to the typed IR (ADR-011). Anything the IR or the
//! builtins registry cannot express yet becomes a reason on the routine
//! (`Routine::unsupported` in `Lowered`), never a guess.

use crate::check::{CheckResult, Res};
use crate::members::Members;
use crate::types::Ty;
use crate::workspace::{ObjKind, Resolution, Workspace};
use al_builtins::value::RtType;
use al_ir as ir;
use al_syntax::{SyntaxKind, SyntaxNode, ast};
use std::collections::HashMap;

pub struct Lowered {
    pub program: ir::Program,
    /// Per object, per routine: why it cannot run (empty = it can).
    pub unsupported: Vec<Vec<Vec<String>>>,
    /// Per object, per routine: the routines it calls directly, as (object, routine).
    pub calls: Vec<Vec<Vec<(usize, usize)>>>,
}

pub fn lower(ws: &Workspace, members: &[Members], checked: &CheckResult) -> Lowered {
    let catalog = al_builtins::catalog::Catalog::load();
    // Routine numbering: every procedure and trigger node of an object, in source order.
    let routine_nodes: Vec<Vec<ast::Routine>> = (0..ws.objects.len())
        .map(|oi| {
            ws.object_ast(oi)
                .syntax()
                .descendants()
                .filter_map(ast::Routine::cast)
                .collect()
        })
        .collect();
    let routine_at: HashMap<(usize, usize), usize> = routine_nodes
        .iter()
        .enumerate()
        .flat_map(|(oi, rs)| {
            rs.iter()
                .enumerate()
                .map(move |(ri, r)| ((oi, usize::from(r.syntax().text_range().start())), ri))
        })
        .collect();
    let mut out = Lowered {
        program: ir::Program::default(),
        unsupported: Vec::new(),
        calls: Vec::new(),
    };
    for (oi, o) in ws.objects.iter().enumerate() {
        let obj = ws.object_ast(oi);
        let mut lw = Lowerer {
            ws,
            members,
            checked,
            catalog: &catalog,
            routine_at: &routine_at,
            oi,
            file: o.file,
            globals: HashMap::new(),
            locals: HashMap::new(),
            reasons: Vec::new(),
            calls: Vec::new(),
        };
        let mut globals = Vec::new();
        let mut global_reasons = Vec::new();
        for v in obj.var_decls() {
            let ty = v.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown);
            for n in v.names() {
                match lw.rt(&ty) {
                    Ok(rt) => {
                        lw.globals.insert(n.to_lowercase(), globals.len());
                        globals.push(ir::Slot { name: n, ty: rt });
                    }
                    Err(e) => global_reasons.push(format!("global `{n}`: {e}")),
                }
            }
        }
        let mut routines = Vec::new();
        let mut unsupported = Vec::new();
        let mut calls = Vec::new();
        for r in &routine_nodes[oi] {
            lw.reasons = global_reasons.clone();
            lw.calls.clear();
            let routine = lw.routine(r);
            unsupported.push(std::mem::take(&mut lw.reasons));
            calls.push(std::mem::take(&mut lw.calls));
            routines.push(routine);
        }
        out.program.objects.push(ir::Object {
            kind: o.kind.label().to_string(),
            id: o.id,
            name: o.name.clone(),
            globals,
            routines,
            properties: obj.properties().map(|p| (p.name(), p.value_text())).collect(),
        });
        out.unsupported.push(unsupported);
        out.calls.push(calls);
    }
    out
}

type L<T> = Result<T, String>;

struct Lowerer<'a> {
    ws: &'a Workspace,
    members: &'a [Members],
    checked: &'a CheckResult,
    catalog: &'a al_builtins::catalog::Catalog,
    routine_at: &'a HashMap<(usize, usize), usize>,
    oi: usize,
    file: usize,
    globals: HashMap<String, usize>,
    locals: HashMap<String, usize>,
    reasons: Vec<String>,
    calls: Vec<(usize, usize)>,
}

fn key(file: usize, n: &SyntaxNode) -> (usize, usize, usize) {
    let r = n.text_range();
    (file, r.start().into(), r.end().into())
}

fn is_stmt(k: SyntaxKind) -> bool {
    use SyntaxKind::*;
    matches!(
        k,
        Block
            | AssignStmt
            | ExprStmt
            | IfStmt
            | CaseStmt
            | WhileStmt
            | RepeatStmt
            | ForStmt
            | ForeachStmt
            | ExitStmt
            | BreakStmt
            | AssertErrorStmt
            | WithStmt
            | ContinueStmt
    )
}

fn sig_tokens(n: &SyntaxNode) -> Vec<al_syntax::SyntaxToken> {
    n.children_with_tokens()
        .filter_map(|e| e.into_token())
        .filter(|t| !t.kind().is_trivia())
        .collect()
}

impl Lowerer<'_> {
    fn res(&self, n: &SyntaxNode) -> Option<&Res> {
        self.checked.resolutions.get(&key(self.file, n))
    }

    fn ty_of(&self, n: &SyntaxNode) -> Ty {
        self.checked
            .types
            .get(&key(self.file, n))
            .cloned()
            .unwrap_or(Ty::Unknown)
    }

    fn resolve_obj(&self, kind: ObjKind, name: &str) -> L<usize> {
        match self.ws.resolve_ref(self.file, kind, &[name.to_string()]) {
            Resolution::Found(i) => Ok(i),
            _ => Err(format!("{} `{name}` is outside the workspace", kind.label())),
        }
    }

    fn rt(&self, t: &Ty) -> L<RtType> {
        Ok(match t {
            Ty::Integer => RtType::Integer,
            Ty::BigInteger => RtType::BigInteger,
            Ty::Decimal => RtType::Decimal,
            Ty::Boolean => RtType::Boolean,
            Ty::Char => RtType::Char,
            Ty::Byte => RtType::Byte,
            Ty::Text(n) => RtType::Text(*n),
            Ty::Code(n) => RtType::Code(*n),
            Ty::Label => RtType::Text(None),
            Ty::Date => RtType::Date,
            Ty::Time => RtType::Time,
            Ty::DateTime => RtType::DateTime,
            Ty::Duration => RtType::Duration,
            Ty::DateFormula => RtType::DateFormula,
            Ty::Guid => RtType::Guid,
            Ty::Variant => RtType::Variant,
            Ty::Option(m) => RtType::Option(m.clone()),
            Ty::Enum(n) => RtType::Enum(self.resolve_obj(ObjKind::Enum, n)? as u32),
            Ty::Object(ObjKind::Codeunit, n) => RtType::Codeunit(self.resolve_obj(ObjKind::Codeunit, n)? as u32),
            Ty::List(t) => RtType::List(Box::new(self.rt(t)?)),
            Ty::Dictionary(k, v) => RtType::Dictionary(Box::new(self.rt(k)?), Box::new(self.rt(v)?)),
            Ty::Array(d, t) => RtType::Array(d.clone(), Box::new(self.rt(t)?)),
            Ty::Builtin(n) => RtType::Library(n.clone()),
            Ty::Record { .. } => return Err("records are not lowered yet".into()),
            other => return Err(format!("type {other} is not lowered yet")),
        })
    }

    fn routine(&mut self, r: &ast::Routine) -> ir::Routine {
        self.locals.clear();
        let mut locals = Vec::new();
        let mut by_var = Vec::new();
        let add = |lw: &mut Self, name: String, ty: Ty, locals: &mut Vec<ir::Slot>| match lw.rt(&ty) {
            Ok(rt) => {
                lw.locals.insert(name.to_lowercase(), locals.len());
                locals.push(ir::Slot { name, ty: rt });
            }
            Err(e) => {
                lw.reasons.push(format!("`{name}`: {e}"));
                lw.locals.insert(name.to_lowercase(), locals.len());
                locals.push(ir::Slot {
                    name,
                    ty: RtType::Variant,
                });
            }
        };
        for p in r.params() {
            by_var.push(p.is_var());
            let ty = p.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown);
            add(self, p.name(), ty, &mut locals);
        }
        let params = locals.len();
        let ret_ty = r.return_type().map(|t| Ty::from_type_ref(&t));
        let ret = match &ret_ty {
            Some(t) => match self.rt(t) {
                Ok(rt) => Some(rt),
                Err(e) => {
                    self.reasons.push(format!("return type: {e}"));
                    None
                }
            },
            None => None,
        };
        let mut ret_local = None;
        if let (Some(n), Some(t)) = (r.return_name(), &ret_ty) {
            ret_local = Some(locals.len());
            add(self, n, t.clone(), &mut locals);
        }
        for v in r.local_var_decls() {
            let ty = v.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown);
            for n in v.names() {
                add(self, n, ty.clone(), &mut locals);
            }
        }
        let body = match r.body() {
            Some(b) => self.stmts_of(&b),
            None => Vec::new(),
        };
        ir::Routine {
            name: r.name(),
            is_trigger: r.is_trigger(),
            attributes: r
                .attributes()
                .map(|a| ir::Attribute {
                    name: a.name(),
                    args: a
                        .args()
                        .map(|l| l.children().filter_map(|e| self.constant(&e).ok()).collect())
                        .unwrap_or_default(),
                })
                .collect(),
            params,
            by_var,
            locals,
            ret,
            ret_local,
            body,
        }
    }

    /// Statement children of a block-like node, flattened.
    fn stmts_of(&mut self, n: &SyntaxNode) -> Vec<ir::Stmt> {
        let mut out = Vec::new();
        for c in n.children().filter(|c| is_stmt(c.kind())) {
            out.extend(self.stmt(&c));
        }
        out
    }

    fn stmt_or_empty(&mut self, n: Option<&SyntaxNode>) -> Vec<ir::Stmt> {
        n.map(|n| self.stmt(n)).unwrap_or_default()
    }

    fn stmt(&mut self, n: &SyntaxNode) -> Vec<ir::Stmt> {
        use SyntaxKind::*;
        let kids: Vec<SyntaxNode> = n.children().collect();
        let r = match n.kind() {
            Block => return self.stmts_of(n),
            ExprStmt => self.expr(&kids[0]).map(ir::Stmt::Expr),
            AssignStmt => self.assign(n, &kids),
            IfStmt => {
                let else_at = sig_tokens(n)
                    .iter()
                    .find(|t| t.text().eq_ignore_ascii_case("else"))
                    .map(|t| t.text_range().start());
                let cond = self.expr(&kids[0]);
                let (mut then_s, mut else_s) = (Vec::new(), Vec::new());
                for k in kids.iter().skip(1) {
                    let lowered = self.stmt(k);
                    if else_at.is_some_and(|e| k.text_range().start() > e) {
                        else_s.extend(lowered);
                    } else {
                        then_s.extend(lowered);
                    }
                }
                cond.map(|c| ir::Stmt::If(c, then_s, else_s))
            }
            WhileStmt => {
                let cond = self.expr(&kids[0]);
                let body = self.stmt_or_empty(kids.get(1));
                cond.map(|c| ir::Stmt::While(c, body))
            }
            RepeatStmt => {
                let (until, body_nodes) = kids.split_last().expect("repeat has until");
                let mut body = Vec::new();
                for b in body_nodes {
                    body.extend(self.stmt(b));
                }
                self.expr(until).map(|u| ir::Stmt::Repeat(body, u))
            }
            ForStmt => {
                let down = sig_tokens(n).iter().any(|t| t.text().eq_ignore_ascii_case("downto"));
                let var = self.place(&kids[0]);
                let from = self.expr(&kids[1]);
                let to = self.expr(&kids[2]);
                let body = self.stmt_or_empty(kids.get(3));
                (|| {
                    Ok(ir::Stmt::For {
                        var: var?,
                        from: from?,
                        to: to?,
                        down,
                        body,
                    })
                })()
            }
            ForeachStmt => {
                let var = self.place(&kids[0]);
                let coll = self.expr(&kids[1]);
                let body = self.stmt_or_empty(kids.get(2));
                (|| {
                    Ok(ir::Stmt::Foreach {
                        var: var?,
                        collection: coll?,
                        body,
                    })
                })()
            }
            CaseStmt => self.case(&kids),
            ExitStmt => match kids.first() {
                None => Ok(ir::Stmt::Exit(None)),
                Some(e) => self.expr(e).map(|v| ir::Stmt::Exit(Some(v))),
            },
            BreakStmt => Ok(ir::Stmt::Break),
            AssertErrorStmt => Ok(ir::Stmt::AssertError(self.stmt_or_empty(kids.first()))),
            other => Err(format!("statement {other:?} is not lowered yet")),
        };
        match r {
            Ok(s) => vec![s],
            Err(e) => {
                self.reasons.push(e);
                Vec::new()
            }
        }
    }

    fn assign(&mut self, n: &SyntaxNode, kids: &[SyntaxNode]) -> L<ir::Stmt> {
        let place = self.place(&kids[0])?;
        let slot = self.rt(&self.ty_of(&kids[0]))?;
        let rhs = self.expr(&kids[1])?;
        let op = sig_tokens(n).into_iter().find(|t| {
            matches!(
                t.kind(),
                SyntaxKind::Assign
                    | SyntaxKind::PlusAssign
                    | SyntaxKind::MinusAssign
                    | SyntaxKind::StarAssign
                    | SyntaxKind::SlashAssign
            )
        });
        let value = match op.map(|t| t.kind()) {
            Some(SyntaxKind::PlusAssign) => {
                ir::Expr::Binary(ir::BinOp::Add, Box::new(ir::Expr::Load(place.clone())), Box::new(rhs))
            }
            Some(SyntaxKind::MinusAssign) => {
                ir::Expr::Binary(ir::BinOp::Sub, Box::new(ir::Expr::Load(place.clone())), Box::new(rhs))
            }
            Some(SyntaxKind::StarAssign) => {
                ir::Expr::Binary(ir::BinOp::Mul, Box::new(ir::Expr::Load(place.clone())), Box::new(rhs))
            }
            Some(SyntaxKind::SlashAssign) => {
                ir::Expr::Binary(ir::BinOp::Div, Box::new(ir::Expr::Load(place.clone())), Box::new(rhs))
            }
            _ => rhs,
        };
        Ok(ir::Stmt::Assign(place, ir::Expr::Convert(slot, Box::new(value))))
    }

    fn case(&mut self, kids: &[SyntaxNode]) -> L<ir::Stmt> {
        let scrutinee = self.expr(&kids[0])?;
        let mut branches = Vec::new();
        let mut otherwise = Vec::new();
        for k in &kids[1..] {
            match k.kind() {
                SyntaxKind::CaseBranch => {
                    let parts: Vec<SyntaxNode> = k.children().collect();
                    let mut labels = Vec::new();
                    let mut body = Vec::new();
                    for p in &parts {
                        if is_stmt(p.kind()) {
                            body.extend(self.stmt(p));
                        } else {
                            labels.push(self.set_elem(p)?);
                        }
                    }
                    branches.push((labels, body));
                }
                SyntaxKind::CaseElse => otherwise = self.stmts_of(k),
                _ => {}
            }
        }
        Ok(ir::Stmt::Case {
            scrutinee,
            branches,
            otherwise,
        })
    }

    fn set_elem(&mut self, n: &SyntaxNode) -> L<ir::SetElem> {
        if n.kind() != SyntaxKind::RangeExpr {
            return Ok(ir::SetElem::One(self.expr(n)?));
        }
        let dotdot = sig_tokens(n)
            .into_iter()
            .find(|t| t.kind() == SyntaxKind::DotDot)
            .expect("range has ..")
            .text_range()
            .start();
        let mut lo = None;
        let mut hi = None;
        for c in n.children() {
            let e = self.expr(&c)?;
            if c.text_range().start() < dotdot {
                lo = Some(e);
            } else {
                hi = Some(e);
            }
        }
        Ok(ir::SetElem::Range(lo, hi))
    }

    fn place(&mut self, n: &SyntaxNode) -> L<ir::Place> {
        match n.kind() {
            SyntaxKind::ParenExpr => self.place(&n.children().next().ok_or("empty parentheses")?),
            SyntaxKind::IndexExpr => {
                let kids: Vec<SyntaxNode> = n.children().collect();
                let base = self.place(&kids[0])?;
                let idx = kids[1..].iter().map(|k| self.expr(k)).collect::<L<Vec<_>>>()?;
                Ok(ir::Place::Index(Box::new(base), idx))
            }
            _ => match self.res(n) {
                Some(Res::Local(name)) => self
                    .locals
                    .get(name)
                    .map(|i| ir::Place::Local(*i))
                    .ok_or_else(|| format!("local `{name}`")),
                Some(Res::Global(name)) => self
                    .globals
                    .get(name)
                    .map(|i| ir::Place::Global(*i))
                    .ok_or_else(|| format!("implicit variable `{name}` is not lowered yet")),
                Some(Res::Field { .. }) => Err("record fields are not lowered yet".into()),
                _ => Err(format!("`{}` is not assignable", n.text())),
            },
        }
    }

    fn constant(&self, n: &SyntaxNode) -> L<ir::Const> {
        let t = sig_tokens(n).into_iter().next().ok_or("empty literal")?;
        let text = t.text();
        let bad = || format!("literal `{text}` is not lowered yet");
        Ok(match t.kind() {
            SyntaxKind::Integer => match text.parse::<i32>() {
                Ok(i) => ir::Const::Integer(i),
                Err(_) => return Err(bad()),
            },
            SyntaxKind::Decimal => ir::Const::Decimal(text.to_string()),
            SyntaxKind::StringLit => ir::Const::Text(text[1..text.len() - 1].replace("''", "'")),
            SyntaxKind::BigIntLit => ir::Const::BigInteger(text[..text.len() - 1].parse().map_err(|_| bad())?),
            SyntaxKind::Ident if text.eq_ignore_ascii_case("true") => ir::Const::Boolean(true),
            SyntaxKind::Ident if text.eq_ignore_ascii_case("false") => ir::Const::Boolean(false),
            SyntaxKind::DateLit => {
                let d = &text[..text.len() - 1];
                if d.chars().all(|c| c == '0') {
                    ir::Const::Date(0)
                } else if d.len() == 8 {
                    let (y, m, dd) = (
                        d[..4].parse().map_err(|_| bad())?,
                        d[4..6].parse().map_err(|_| bad())?,
                        d[6..].parse().map_err(|_| bad())?,
                    );
                    ir::Const::Date(al_builtins::format::day_from_civil(y, m, dd))
                } else {
                    return Err(bad());
                }
            }
            SyntaxKind::TimeLit => {
                let d = &text[..text.len() - 1];
                if d.chars().all(|c| c == '0') {
                    ir::Const::Time(0)
                } else {
                    let (hms, frac) = d.split_once('.').unwrap_or((d, "0"));
                    if hms.len() != 6 {
                        return Err(bad());
                    }
                    let h: i64 = hms[..2].parse().map_err(|_| bad())?;
                    let m: i64 = hms[2..4].parse().map_err(|_| bad())?;
                    let s: i64 = hms[4..].parse().map_err(|_| bad())?;
                    let ms: i64 = format!("{frac:0<3}")[..3].parse().map_err(|_| bad())?;
                    ir::Const::Time(((h * 60 + m) * 60 + s) * 1000 + ms + 1)
                }
            }
            SyntaxKind::DateTimeLit if text[..text.len() - 2].chars().all(|c| c == '0') => ir::Const::DateTime(0),
            _ => return Err(bad()),
        })
    }

    fn expr(&mut self, n: &SyntaxNode) -> L<ir::Expr> {
        use SyntaxKind::*;
        let kids: Vec<SyntaxNode> = n.children().collect();
        match n.kind() {
            Literal => Ok(ir::Expr::Const(self.constant(n)?)),
            ParenExpr => self.expr(&kids[0]),
            PrefixExpr => {
                let op = match sig_tokens(n)[0].text().to_ascii_lowercase().as_str() {
                    "not" => ir::UnOp::Not,
                    "-" => ir::UnOp::Neg,
                    _ => ir::UnOp::Plus,
                };
                Ok(ir::Expr::Unary(op, Box::new(self.expr(&kids[0])?)))
            }
            BinExpr => {
                let op = sig_tokens(n)[0].text().to_ascii_lowercase();
                let l = self.expr(&kids[0])?;
                if op == "in" {
                    let set = &kids[1];
                    if set.kind() != SetExpr {
                        return Err("`in` with a non-literal set is not lowered yet".into());
                    }
                    let elems = set.children().map(|e| self.set_elem(&e)).collect::<L<Vec<_>>>()?;
                    return Ok(ir::Expr::In(Box::new(l), elems));
                }
                let r = self.expr(&kids[1])?;
                let op = match op.as_str() {
                    "+" => ir::BinOp::Add,
                    "-" => ir::BinOp::Sub,
                    "*" => ir::BinOp::Mul,
                    "/" => ir::BinOp::Div,
                    "div" => ir::BinOp::IntDiv,
                    "mod" => ir::BinOp::Mod,
                    "and" => ir::BinOp::And,
                    "or" => ir::BinOp::Or,
                    "xor" => ir::BinOp::Xor,
                    "=" => ir::BinOp::Eq,
                    "<>" => ir::BinOp::Ne,
                    "<" => ir::BinOp::Lt,
                    "<=" => ir::BinOp::Le,
                    ">" => ir::BinOp::Gt,
                    ">=" => ir::BinOp::Ge,
                    other => return Err(format!("operator `{other}`")),
                };
                Ok(ir::Expr::Binary(op, Box::new(l), Box::new(r)))
            }
            TernaryExpr => Ok(ir::Expr::Ternary(
                Box::new(self.expr(&kids[0])?),
                Box::new(self.expr(&kids[1])?),
                Box::new(self.expr(&kids[2])?),
            )),
            CallExpr => {
                let args: Vec<SyntaxNode> = kids.get(1).map(|a| a.children().collect()).unwrap_or_default();
                self.call(n, &kids[0], &args)
            }
            ScopeExpr => match self.res(n).cloned() {
                Some(Res::EnumValue { object, value }) => Ok(ir::Expr::Const(ir::Const::Enum(object as u32, value))),
                Some(Res::OptionValue(i)) => match self.ty_of(n) {
                    Ty::Option(m) => Ok(ir::Expr::Const(ir::Const::Option(i, m))),
                    _ => Err("option value without its option type".into()),
                },
                Some(Res::ObjectId(id)) => Ok(ir::Expr::Const(ir::Const::Integer(id))),
                _ => Err(format!("`{}` is not lowered yet", n.text())),
            },
            NameRef | MemberExpr | IndexExpr => match self.res(n).cloned() {
                Some(Res::Proc { .. }) | Some(Res::Builtin(_)) => self.call(n, n, &[]),
                Some(Res::Field { .. }) => Err("record fields are not lowered yet".into()),
                _ => Ok(ir::Expr::Load(self.place(n)?)),
            },
            other => Err(format!("expression {other:?} `{}` is not lowered yet", n.text())),
        }
    }

    /// A call resolved on `node` (the CallExpr, or the callee for a call without parentheses).
    fn call(&mut self, node: &SyntaxNode, callee: &SyntaxNode, args: &[SyntaxNode]) -> L<ir::Expr> {
        let receiver_node = (callee.kind() == SyntaxKind::MemberExpr)
            .then(|| callee.children().next())
            .flatten();
        match self.res(node).cloned() {
            Some(Res::Proc { object, offset }) => {
                let routine = *self.routine_at.get(&(object, offset)).ok_or("procedure not found")?;
                let sig = self.members[object]
                    .procedures
                    .iter()
                    .find(|p| p.offset == offset)
                    .ok_or("procedure signature")?
                    .clone();
                let instance = match &receiver_node {
                    Some(r) if matches!(self.ty_of(r), Ty::Object(ObjKind::Codeunit, _)) => {
                        Some(Box::new(self.expr(r)?))
                    }
                    Some(_) => return Err("procedure calls on non-codeunit receivers are not lowered yet".into()),
                    None if object == self.oi => None,
                    None => return Err("implicit calls into another object are not lowered yet".into()),
                };
                let mut out = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let p = sig.params.get(i).ok_or("too many arguments")?;
                    if p.by_var {
                        out.push(ir::Arg::Ref(self.place(a)?));
                    } else {
                        out.push(ir::Arg::Value(ir::Expr::Convert(
                            self.rt(&p.ty)?,
                            Box::new(self.expr(a)?),
                        )));
                    }
                }
                self.calls.push((object, routine));
                Ok(ir::Expr::Call {
                    object,
                    routine,
                    instance,
                    args: out,
                })
            }
            Some(Res::Builtin(id)) => {
                if al_builtins::native::lookup(&id).is_none() {
                    return Err(format!("builtin {id} is not implemented"));
                }
                let sig = self
                    .catalog
                    .by_id(&id)
                    .ok_or_else(|| format!("{id} not in catalog"))?
                    .clone();
                let receiver = match &receiver_node {
                    Some(r) if sig.instance => Some(Box::new(match self.place(r) {
                        Ok(p) => ir::Arg::Ref(p),
                        Err(_) => ir::Arg::Value(self.expr(r)?),
                    })),
                    _ => None,
                };
                let mut out = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let p = sig
                        .params
                        .get(i)
                        .or_else(|| sig.params.last().filter(|p| p.variadic))
                        .ok_or("too many arguments")?;
                    if matches!(self.res(a), Some(Res::Field { .. })) {
                        return Err("field arguments are not lowered yet".into());
                    }
                    out.push(if p.by_var {
                        ir::Arg::Ref(self.place(a)?)
                    } else {
                        ir::Arg::Value(self.expr(a)?)
                    });
                }
                Ok(ir::Expr::Builtin {
                    id,
                    receiver,
                    args: out,
                })
            }
            other => Err(format!("call `{}` ({other:?}) is not lowered yet", node.text())),
        }
    }
}

/// Load, check and lower in one go.
pub fn compile(
    apps: Vec<crate::workspace::AppSource>,
    symbols: &std::collections::BTreeSet<String>,
) -> (Workspace, Lowered) {
    let ws = Workspace::load(apps, symbols);
    let members = crate::members::build(&ws);
    let checked = crate::check::check(&ws, &members, &al_builtins::catalog::Catalog::load());
    let lowered = lower(&ws, &members, &checked);
    (ws, lowered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::AppSource;

    fn lower_src(src: &str) -> (Workspace, Lowered) {
        compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("f.al".into(), src.into())],
            }],
            &Default::default(),
        )
    }

    fn routine<'a>(l: &'a Lowered, name: &str) -> (usize, &'a ir::Routine) {
        l.program.objects[0]
            .routines
            .iter()
            .enumerate()
            .find(|(_, r)| r.name == name)
            .expect("routine")
    }

    #[test]
    fn slots_for_globals_params_and_locals() {
        let (_, l) = lower_src(
            "codeunit 50100 C { var G: Integer; procedure P(A: Text; var B: Decimal) R: Boolean var L: Code[10]; begin end; }",
        );
        let o = &l.program.objects[0];
        assert_eq!(
            o.globals,
            vec![ir::Slot {
                name: "G".into(),
                ty: RtType::Integer
            }]
        );
        let (_, r) = routine(&l, "P");
        assert_eq!(r.params, 2);
        assert_eq!(r.by_var, vec![false, true]);
        let names: Vec<_> = r.locals.iter().map(|s| (s.name.as_str(), s.ty.clone())).collect();
        assert_eq!(
            names,
            vec![
                ("A", RtType::Text(None)),
                ("B", RtType::Decimal),
                ("R", RtType::Boolean),
                ("L", RtType::Code(Some(10)))
            ]
        );
        assert_eq!(r.ret, Some(RtType::Boolean));
        assert_eq!(r.ret_local, Some(2));
    }

    #[test]
    fn assignment_converts_to_the_slot_type() {
        let (_, l) = lower_src("codeunit 50100 C { procedure P() var D: Decimal; begin D := 1; end; }");
        let (_, r) = routine(&l, "P");
        assert_eq!(
            r.body,
            vec![ir::Stmt::Assign(
                ir::Place::Local(0),
                ir::Expr::Convert(RtType::Decimal, Box::new(ir::Expr::Const(ir::Const::Integer(1))))
            )]
        );
    }

    #[test]
    fn compound_assignment_reads_the_place() {
        let (_, l) = lower_src("codeunit 50100 C { procedure P() var I: Integer; begin I += 2; end; }");
        let (_, r) = routine(&l, "P");
        let ir::Stmt::Assign(ir::Place::Local(0), ir::Expr::Convert(_, e)) = &r.body[0] else {
            panic!("{:?}", r.body)
        };
        assert_eq!(
            **e,
            ir::Expr::Binary(
                ir::BinOp::Add,
                Box::new(ir::Expr::Load(ir::Place::Local(0))),
                Box::new(ir::Expr::Const(ir::Const::Integer(2)))
            )
        );
    }

    #[test]
    fn builtin_calls_carry_the_overload_id() {
        let (_, l) = lower_src("codeunit 50100 C { procedure P() var T: Text; begin T := Format(1); end; }");
        let (_, r) = routine(&l, "P");
        let ir::Stmt::Assign(_, ir::Expr::Convert(_, e)) = &r.body[0] else {
            panic!()
        };
        assert!(
            matches!(&**e, ir::Expr::Builtin { id, receiver: None, args } if id == "System.Format#1" && args.len() == 1)
        );
    }

    #[test]
    fn procedure_calls_and_call_graph() {
        let (_, l) = lower_src(
            "codeunit 50100 C { procedure Q(var X: Integer) begin end; procedure P() var I: Integer; begin Q(I); end; }",
        );
        let (qi, _) = routine(&l, "Q");
        let (pi, p) = routine(&l, "P");
        assert_eq!(
            p.body,
            vec![ir::Stmt::Expr(ir::Expr::Call {
                object: 0,
                routine: qi,
                instance: None,
                args: vec![ir::Arg::Ref(ir::Place::Local(0))]
            })]
        );
        assert_eq!(l.calls[0][pi], vec![(0, qi)]);
    }

    #[test]
    fn unimplemented_builtin_makes_the_routine_unsupported() {
        let (_, l) = lower_src("codeunit 50100 C { procedure P() begin Sleep(1); end; procedure Ok() begin end; }");
        let (pi, _) = routine(&l, "P");
        let (oi, _) = routine(&l, "Ok");
        assert_eq!(
            l.unsupported[0][pi],
            vec!["builtin System.Sleep#1 is not implemented".to_string()]
        );
        assert!(l.unsupported[0][oi].is_empty());
    }

    #[test]
    fn control_flow_and_asserterror() {
        let (_, l) = lower_src(
            "codeunit 50100 C { procedure P() var I: Integer; begin if I = 0 then I := 1 else exit; asserterror Error('x'); end; }",
        );
        let (pi, p) = routine(&l, "P");
        assert!(l.unsupported[0][pi].is_empty(), "{:?}", l.unsupported[0][pi]);
        assert!(matches!(p.body[0], ir::Stmt::If(_, ref t, ref e) if t.len() == 1 && e == &vec![ir::Stmt::Exit(None)]));
        assert!(matches!(p.body[1], ir::Stmt::AssertError(ref b) if b.len() == 1));
    }

    #[test]
    fn enum_and_option_constants() {
        let (_, l) = lower_src(
            "enum 50900 E { value(0; A) { } value(7; B) { } }\ncodeunit 50100 C { procedure P() var X: Enum E; O: Option P,Q; begin X := E::B; O := O::Q; end; }",
        );
        let r = l.program.objects[1].routines.iter().find(|r| r.name == "P").unwrap();
        assert!(
            matches!(&r.body[0], ir::Stmt::Assign(_, ir::Expr::Convert(_, e)) if **e == ir::Expr::Const(ir::Const::Enum(0, 7)))
        );
        assert!(
            matches!(&r.body[1], ir::Stmt::Assign(_, ir::Expr::Convert(_, e)) if **e == ir::Expr::Const(ir::Const::Option(1, vec!["P".into(), "Q".into()])))
        );
    }
}
