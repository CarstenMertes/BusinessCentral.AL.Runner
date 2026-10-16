//! Lowering of routines and statements.
use super::*;

impl Lowerer<'_> {
    pub(crate) fn routine(&mut self, r: &ast::Routine) -> ir::Routine {
        #[cfg(test)]
        if r.name() == "InjectedLowerPanic" {
            panic!("injected lowering panic");
        }
        // A routine the compiler made up (a page expression) has no source.
        self.emit_pos = self.lines.is_some() && !crate::pages::is_synthetic(&r.name());
        let pos = match r.keyword_offset().filter(|_| self.emit_pos) {
            Some(k) => self.add_pos(k, k + 1).unwrap_or(ir::NO_POS),
            None => ir::NO_POS,
        };
        let end_pos = match r.end_offset().filter(|_| self.emit_pos) {
            Some(k) => self.add_pos(k, k + 1).unwrap_or(ir::NO_POS),
            None => ir::NO_POS,
        };
        let begin_pos = match r.begin_offset().filter(|_| self.emit_pos) {
            Some(k) => self.add_pos(k, k + 1).unwrap_or(ir::NO_POS),
            None => ir::NO_POS,
        };
        self.locals.clear();
        self.local_labels.clear();
        self.tys.clear();
        self.tys_on = true;
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
        self.ret_decl = ret_ty.clone();
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
        self.ret_ty = ret.clone();
        let mut ret_local = None;
        if let (Some(n), Some(t)) = (r.return_name(), &ret_ty) {
            ret_local = Some(locals.len());
            add(self, n, t.clone(), &mut locals);
        }
        for v in r.local_var_decls() {
            let ty = v.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown);
            let label = v.type_ref().as_ref().and_then(label_text);
            for n in v.names() {
                if let Some(l) = &label {
                    self.local_labels.insert(n.to_lowercase(), l.clone());
                }
                add(self, n, ty.clone(), &mut locals);
            }
        }
        let mut body = match r.body() {
            Some(b) => self.stmts_of(&b),
            None => Vec::new(),
        };
        // The service tier implements a `[Native]` procedure; its AL body is empty. Running that body
        // would answer the default value of the return type, which is not what the platform answers.
        // A few have an implementation here (`platform_native`, pinned by the corpus); the others are refused.
        let native = (body.is_empty() && r.attributes().any(|a| a.name().eq_ignore_ascii_case("native")))
            .then(|| {
                let types: Vec<String> = r
                    .params()
                    .map(|p| p.type_ref().map(|t| t.type_name().to_lowercase()).unwrap_or_default())
                    .collect();
                al_builtins::platform_native::find(&self.ws.objects[self.oi].name, &r.name(), &types)
            })
            .flatten()
            .and_then(|entry| ir::BuiltinId::of(entry.id));
        if let Some(id) = native {
            let call = ir::Expr::Builtin {
                id,
                receiver: None,
                args: (0..params)
                    .map(|i| ir::Arg::Value(ir::Expr::Load(ir::Place::Local(i))))
                    .collect(),
                ret_used: ret_ty.is_some(),
            };
            body = vec![if ret_ty.is_some() {
                ir::Stmt::Exit(Some(call))
            } else {
                ir::Stmt::Expr(call)
            }];
        } else if body.is_empty() && r.attributes().any(|a| a.name().eq_ignore_ascii_case("native")) {
            self.reasons.push(format!(
                "[Native] procedure `{}` is implemented by the platform and has no AL body (what it answers is not pinned)",
                r.name()
            ));
        }
        let expr_tys = self.take_table(&body);
        self.tys_on = false;
        let field = r
            .syntax()
            .ancestors()
            .filter_map(ast::Section::cast)
            .find(|s| s.name().eq_ignore_ascii_case("field"))
            .and_then(|s| {
                let id: u32 = s.args().first()?.parse().ok()?;
                self.table_of[self.oi].and_then(|t| self.field_pos(t, id))
            });
        let container = r.syntax().ancestors().find_map(ast::Section::cast).map(|sec| {
            let arg = sec.args().first().cloned().unwrap_or_default();
            (
                sec.name().to_ascii_lowercase(),
                arg.trim().trim_matches('"').to_string(),
            )
        });
        ir::Routine {
            name: r.name(),
            field,
            container,
            is_trigger: r.is_trigger(),
            attributes: self.attributes_of(r),
            params,
            by_var,
            locals,
            ret,
            ret_local,
            body,
            pos,
            end_pos,
            begin_pos,
            expr_tys,
        }
    }

    /// The first argument of `Format`. An Option field with an `OptionCaption` formats as the
    /// caption, not the member name (corpus fieldref/TestPrecompiledFieldOptionCaption.al:47-55,
    /// a Base Application table whose caption differs from its member). The value keeps its
    /// ordinal and takes the caption list. Other Format forms on such a field are not pinned.
    pub(crate) fn format_option_field(&mut self, plain: bool, arg: &SyntaxNode) -> L<ir::Expr> {
        let value = self.expr(arg)?;
        let Some(Res::Field { table, id: field }) = self.res(arg).cloned() else {
            return Ok(value);
        };
        let Some(f) = self.members[table].fields.iter().find(|f| f.id == field) else {
            return Ok(value);
        };
        let (Ty::Option(names), Some((_, text))) = (
            &f.ty,
            f.properties
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("OptionCaption")),
        ) else {
            return Ok(value);
        };
        let captions: Vec<String> = match crate::members::first_string_literal(text) {
            Some(list) => list.split(',').map(str::to_string).collect(),
            None => return Err(format!("Format of option field {} with OptionCaption `{text}`", f.name)),
        };
        if &captions == names {
            return Ok(value);
        }
        // Every form of Format reads the caption list (text/TestProbeTextDateFormatContracts.al:
        // Format_OptionFieldAndOptionVariable): standard formats 2 and 9 the ordinal, a length cuts or pads the caption.
        let _ = plain;
        if captions.len() != names.len() {
            return Err(format!(
                "Format of option field {} whose OptionCaption has another number of entries than its members is not pinned",
                f.name
            ));
        }
        Ok(ir::Expr::Convert(RtType::Option(captions), Box::new(value)))
    }

    pub(crate) fn attributes_of(&self, r: &ast::Routine) -> Vec<ir::Attribute> {
        r.attributes()
            .map(|a| ir::Attribute {
                text: a.syntax().text().to_string(),
                name: a.name(),
                args: a
                    .args()
                    .map(|l| l.children().filter_map(|e| self.constant(&e).ok()).collect())
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// A routine of a dependency app that nothing reached: its name and attributes (so event
    /// subscribers stay registered), no body, and a reason that refuses anything that runs it.
    pub(crate) fn stub(&mut self, r: &ast::Routine) -> ir::Routine {
        self.refused(
            r,
            "not compiled: no code reached it while compiling the workspace's dependencies (ADR-013)".into(),
        )
    }

    /// A routine without a body, and `reason` that refuses anything that runs it.
    pub(crate) fn refused(&mut self, r: &ast::Routine, reason: String) -> ir::Routine {
        self.reasons.push(reason);
        ir::Routine {
            name: r.name(),
            field: None,
            container: None,
            is_trigger: r.is_trigger(),
            attributes: self.attributes_of(r),
            params: 0,
            by_var: Vec::new(),
            locals: Vec::new(),
            ret: None,
            ret_local: None,
            body: Vec::new(),
            pos: ir::NO_POS,
            end_pos: ir::NO_POS,
            begin_pos: ir::NO_POS,
            expr_tys: Vec::new(),
        }
    }

    /// Records the position of the code at `start..end` of the file being lowered and returns its id.
    pub(crate) fn add_pos(&mut self, start: usize, end: usize) -> Option<u32> {
        let lines = self.lines.as_ref()?;
        let (line, col) = lines.locate(start);
        let (end_line, _) = lines.locate(end.saturating_sub(1).max(start));
        let p = ir::SrcPos {
            file: lines.file,
            line,
            col,
            end_line,
            object: self.oi as u32,
            routine: self.routine_idx as u32,
        };
        self.positions.push(p);
        Some(self.positions.len() as u32 - 1)
    }

    /// The marker for statement `n`, when the routine being lowered has positions.
    pub(crate) fn mark(&mut self, n: &SyntaxNode) -> Option<ir::Stmt> {
        if !self.emit_pos {
            return None;
        }
        let (start, end) = significant_span(n)?;
        self.add_pos(start, end).map(ir::Stmt::Pos)
    }

    /// Statement children of a block-like node, flattened.
    pub(crate) fn stmts_of(&mut self, n: &SyntaxNode) -> Vec<ir::Stmt> {
        let mut out = Vec::new();
        for c in n.children().filter(|c| is_stmt(c.kind())) {
            out.extend(self.stmt(&c));
        }
        out
    }

    pub(crate) fn stmt_or_empty(&mut self, n: Option<&SyntaxNode>) -> Vec<ir::Stmt> {
        n.map(|n| self.stmt(n)).unwrap_or_default()
    }

    /// Notes that the lowering of the statement being lowered rests on assumption `a`: the statement is
    /// preceded by `Stmt::Assume(a)`, so a test that runs it is reported as flagged.
    pub(crate) fn assume(&mut self, a: al_builtins::assumptions::Assumption) {
        if let Some(frame) = self.assumed.last_mut() {
            if !frame.contains(&a) {
                frame.push(a);
            }
        }
    }

    pub(crate) fn stmt(&mut self, n: &SyntaxNode) -> Vec<ir::Stmt> {
        self.assumed.push(Vec::new());
        let mark = self.tys.len();
        let mut out = self.stmt_plain(n);
        if self.tys_on && n.kind() != SyntaxKind::Block {
            self.seal_stmts(mark, &out);
        }
        let marks = self.assumed.pop().unwrap_or_default();
        if !marks.is_empty() && !out.is_empty() {
            // After the position marker, which belongs to the statement that follows it.
            let at = usize::from(matches!(out.first(), Some(ir::Stmt::Pos(_))));
            for (i, a) in marks.into_iter().enumerate() {
                out.insert(at + i, ir::Stmt::Assume(a));
            }
        }
        out
    }

    pub(crate) fn stmt_plain(&mut self, n: &SyntaxNode) -> Vec<ir::Stmt> {
        use SyntaxKind::*;
        let kids: Vec<SyntaxNode> = n.children().collect();
        let r = match n.kind() {
            Block => return self.stmts_of(n),
            // A try function called as a statement behaves like a normal method
            // (doc: TryFunction attribute), so its errors propagate.
            // `continue;` (a statement of the AL language that Base Application's Inventory Adjustment uses): the
            // checker sees a bare name that is no variable, procedure or field.
            ExprStmt
                if kids[0].kind() == NameRef
                    && kids[0].text().to_string().trim().eq_ignore_ascii_case("continue")
                    && self.res(&kids[0]).is_none() =>
            {
                Ok(ir::Stmt::Continue)
            }
            ExprStmt => self.expr(&kids[0]).map(|e| match e {
                ir::Expr::Builtin { id, receiver, args, .. } => ir::Stmt::Expr(ir::Expr::Builtin {
                    id,
                    receiver,
                    args,
                    ret_used: false,
                }),
                ir::Expr::Call {
                    object,
                    routine,
                    instance,
                    args,
                    ..
                } => ir::Stmt::Expr(ir::Expr::Call {
                    object,
                    routine,
                    instance,
                    args,
                    try_call: false,
                }),
                e => ir::Stmt::Expr(e),
            }),
            AssignStmt => self.assign(n, &kids),
            IfStmt => {
                let else_at = sig_tokens(n)
                    .iter()
                    .find(|t| t.text().eq_ignore_ascii_case("else"))
                    .map(|t| t.text_range().start());
                let cond = self.condition(&kids[0]);
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
                let cond = self.condition(&kids[0]);
                let body = self.stmt_or_empty(kids.get(1));
                cond.map(|c| ir::Stmt::While(c, body))
            }
            RepeatStmt => {
                let (until, body_nodes) = kids.split_last().expect("repeat has until");
                let mut body = Vec::new();
                for b in body_nodes {
                    body.extend(self.stmt(b));
                }
                self.condition(until).map(|u| ir::Stmt::Repeat(body, u))
            }
            ForStmt => {
                let down = sig_tokens(n).iter().any(|t| t.text().eq_ignore_ascii_case("downto"));
                let var = self.place(&kids[0]);
                let from = self.operand(&kids[1], ir::OperandUse::Value);
                let to = self.operand(&kids[2], ir::OperandUse::Value);
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
                Some(e) => self.expr(e).and_then(|v| {
                    // A DotNet result must fit the return type exactly (ADR-014). An interface
                    // result takes what an interface variable takes: a codeunit that implements
                    // it, an enum, or an interface it extends (codeunit/TestInterfaceRuntimeContracts.al:
                    // Interface_VarParameterAndReturnValue_CarryTheReference, `exit(Impl)`).
                    if let Some(decl @ Ty::Object(ObjKind::Interface, _)) = self.ret_decl.clone() {
                        return self
                            .convert_to(&decl, self.ty_ref(e), v)
                            .map(|v| ir::Stmt::Exit(Some(v)));
                    }
                    Ok(ir::Stmt::Exit(Some(match (self.ty_ref(e), &self.ret_ty) {
                        (Ty::DotNet(a), Some(rt)) if a.is_empty() && !matches!(rt, RtType::DotNet(_)) => {
                            ir::Expr::DotNetCoerce(rt.clone(), Box::new(v))
                        }
                        _ => v,
                    })))
                }),
            },
            BreakStmt => Ok(ir::Stmt::Break),
            AssertErrorStmt => Ok(ir::Stmt::AssertError(self.stmt_or_empty(kids.first()))),
            other => Err(format!("statement {other:?} is not lowered yet")),
        };
        match r {
            Ok(s) => match self.mark(n) {
                Some(m) => vec![m, s],
                None => vec![s],
            },
            Err(e) => {
                self.reasons.push(e);
                Vec::new()
            }
        }
    }

    pub(crate) fn assign(&mut self, n: &SyntaxNode, kids: &[SyntaxNode]) -> L<ir::Stmt> {
        // `CurrFieldNo := FieldNo(X)` in table code sets the field number of the implicit Rec. Base Application's
        // `SelltoContactLookup` does it before it validates a field. The corpus pins reading CurrFieldNo
        // (handlers/TestPageCurrFieldNo_Tests.al); the assignment is not pinned (docs/probes-needed.md).
        if kids[0].kind() == SyntaxKind::NameRef
            && matches!(self.res(&kids[0]), Some(Res::Global(g)) if g == "currfieldno")
            && !self.globals.contains_key("currfieldno")
        {
            if !sig_tokens(n).iter().any(|t| t.kind() == SyntaxKind::Assign) {
                return Err("a compound assignment to CurrFieldNo is not lowered (not pinned)".into());
            }
            let rec = self
                .rec_slot
                .ok_or("CurrFieldNo outside code that has an implicit Rec")?;
            let value = self.expr(&kids[1])?;
            let value = self.convert_to(&Ty::Integer, self.ty_ref(&kids[1]), value)?;
            return Ok(ir::Stmt::Expr(ir::Expr::Builtin {
                id: ir::BuiltinId::of("Record.SetCurrFieldNo#1").ok_or("Record.SetCurrFieldNo#1 has no builtin id")?,
                receiver: Some(Box::new(ir::Arg::Ref(ir::Place::Global(rec)))),
                args: vec![ir::Arg::Value(value)],
                ret_used: false,
            }));
        }
        // `X.Property := value` on a DotNet value (ADR-014).
        if let Some(Res::DotNetMember(name)) = self.res(&kids[0]).cloned() {
            if kids[0].kind() == SyntaxKind::MemberExpr {
                if sig_tokens(n).iter().any(|t| {
                    matches!(
                        t.kind(),
                        SyntaxKind::PlusAssign
                            | SyntaxKind::MinusAssign
                            | SyntaxKind::StarAssign
                            | SyntaxKind::SlashAssign
                    )
                }) {
                    return Err("compound assignment to a DotNet property is not lowered yet".into());
                }
                let value = self.dotnet_arg(&kids[1])?;
                return self
                    .dotnet_member(&kids[0], &kids[0], &name, ir::DotNetOp::Set(name.clone()), vec![value])
                    .map(ir::Stmt::Expr);
            }
        }
        // `FieldRef.Value := x` is the setter form of the `Value` method
        // (corpus recordref/TestRecordRefCRUD.al:20-23): not a place, a builtin call.
        if let Some(Res::Builtin(id)) = self.res(&kids[0]).cloned() {
            let sig = self
                .catalog
                .by_id(&id)
                .filter(|s| is_property_setter(&s.receiver, &s.name))
                .cloned();
            let member = kids[0].kind() == SyntaxKind::MemberExpr;
            // Without a receiver the member is a global function (`WorkDate := d`) or a Record
            // member of the implicit Rec of table, page and dataitem code (`FilterGroup := 2`).
            let implicit = !member
                && sig
                    .as_ref()
                    .is_some_and(|s| !s.instance || (s.receiver == "Record" && self.rec_slot.is_some()));
            if let Some(sig) = sig.filter(|_| member || implicit) {
                // A test page control's setter form is run by the test client, not a native builtin.
                if al_builtins::native::lookup(&id).is_none() && !id.starts_with("TestField.") {
                    return Err(match al_builtins::native::refusal(&id) {
                        Some(why) => format!("builtin {id}: {why} (docs/probes-needed.md)"),
                        None => format!("builtin {id} is not implemented"),
                    });
                }
                let compound = sig_tokens(n).iter().any(|t| {
                    matches!(
                        t.kind(),
                        SyntaxKind::PlusAssign
                            | SyntaxKind::MinusAssign
                            | SyntaxKind::StarAssign
                            | SyntaxKind::SlashAssign
                    )
                });
                if compound {
                    return Err("compound assignment to a setter-form builtin is not lowered yet".into());
                }
                let receiver = if member {
                    let recv = kids[0].children().next().ok_or("member base")?;
                    // `CurrReport` is the running report: the VM takes it when a report builtin has
                    // no receiver (as for the call form). A static member (`System.WorkDate`)
                    // has none.
                    let current_report = sig.receiver == "Report"
                        && matches!(
                            self.ws.objects[self.oi].kind,
                            ObjKind::Report | ObjKind::ReportExtension
                        )
                        && recv.text().to_string().trim().eq_ignore_ascii_case("CurrReport");
                    if !sig.instance || current_report {
                        None
                    } else {
                        Some(Box::new(match self.place(&recv) {
                            Ok(p) => ir::Arg::Ref(p),
                            Err(_) => ir::Arg::Value(self.expr(&recv)?),
                        }))
                    }
                } else if sig.instance {
                    let rec = self.rec_slot.ok_or("implicit Rec is not lowered for this object yet")?;
                    Some(Box::new(ir::Arg::Ref(ir::Place::Global(rec))))
                } else {
                    None
                };
                let rhs = self.expr(&kids[1])?;
                return Ok(ir::Stmt::Expr(ir::Expr::Builtin {
                    id: super::call::builtin_id(&id)?,
                    receiver,
                    args: vec![ir::Arg::Value(rhs)],
                    ret_used: false,
                }));
            }
        }
        // `GlobalLanguage := Id` is the other spelling of `GlobalLanguage(Id)` (System Application's
        // "Reten. Pol. Telemetry Impl." writes it so; the setter is pinned by
        // session/TestSessionSettings.al:534): a builtin call, not a place.
        if let Some(Res::Builtin(id)) = self.res(&kids[0]).cloned() {
            if id == "System.GlobalLanguage#1" && kids[0].kind() != SyntaxKind::MemberExpr {
                if sig_tokens(n).iter().any(|t| {
                    t.kind() != SyntaxKind::Assign
                        && matches!(
                            t.kind(),
                            SyntaxKind::PlusAssign
                                | SyntaxKind::MinusAssign
                                | SyntaxKind::StarAssign
                                | SyntaxKind::SlashAssign
                        )
                }) {
                    return Err("compound assignment to GlobalLanguage is not lowered yet".into());
                }
                let rhs = self.expr(&kids[1])?;
                return Ok(ir::Stmt::Expr(ir::Expr::Builtin {
                    id: super::call::builtin_id(&id)?,
                    receiver: None,
                    args: vec![ir::Arg::Value(rhs)],
                    ret_used: false,
                }));
            }
        }
        let place = self.place(&kids[0])?;
        let target_ty = self.ty_of(&kids[0]);
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
        let op_kind = op.map(|t| t.kind());
        let value = match op_kind {
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
        let value = match op_kind {
            Some(SyntaxKind::Assign) | None => self.convert_to(&target_ty, self.ty_ref(&kids[1]), value)?,
            Some(_) => ir::Expr::Convert(self.rt(&target_ty)?, Box::new(value)),
        };
        Ok(ir::Stmt::Assign(place, value))
    }

    pub(crate) fn case(&mut self, kids: &[SyntaxNode]) -> L<ir::Stmt> {
        let scrutinee = self.operand(&kids[0], ir::OperandUse::Value)?;
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

    pub(crate) fn set_elem(&mut self, n: &SyntaxNode) -> L<ir::SetElem> {
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

    /// The argument for a `var` parameter: the variable it names. Base Application passes the implicit
    /// `CurrFieldNo` to event publishers that declare `var CallingFieldNo: Integer` (`Sales Header`,
    /// `OnBeforeShouldSearchForCustomerByName`). `CurrFieldNo` is no variable, so the callee gets a copy of its
    /// value and what it assigns to its parameter stays there. Whether BC writes the assignment back to
    /// `CurrFieldNo` is not pinned (docs/probes-needed.md).
    pub(crate) fn by_var_arg(&mut self, a: &SyntaxNode) -> L<ir::Arg> {
        match self.place(a) {
            Ok(p) => Ok(ir::Arg::Ref(p)),
            Err(_)
                if a.kind() == SyntaxKind::NameRef
                    && a.text().to_string().trim().eq_ignore_ascii_case("CurrFieldNo") =>
            {
                Ok(ir::Arg::Value(self.expr(a)?))
            }
            Err(e) => Err(e),
        }
    }

    pub(crate) fn place(&mut self, n: &SyntaxNode) -> L<ir::Place> {
        match n.kind() {
            SyntaxKind::ParenExpr => self.place(&n.children().next().ok_or("empty parentheses")?),
            SyntaxKind::IndexExpr => {
                let kids: Vec<SyntaxNode> = n.children().collect();
                // `A[x] [y]` on a multi-dimensional array is `A[x, y]`: the chain becomes one index.
                if kids[0].kind() == SyntaxKind::IndexExpr && matches!(self.ty_ref(&kids[0]), Ty::Array(..)) {
                    if let ir::Place::Index(root, mut idx) = self.place(&kids[0])? {
                        self.assume(al_builtins::assumptions::Assumption::ChainedArrayIndex);
                        for k in &kids[1..] {
                            idx.push(self.expr(k)?);
                        }
                        return Ok(ir::Place::Index(root, idx));
                    }
                }
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
                // An extension variable named like a base variable is its own (corpus
                // handlers/TestPageExtensionBaseAccessContracts.al:20, Shadow).
                Some(Res::Global(name)) => self
                    .globals
                    .get(name)
                    .map(|i| ir::Place::Global(*i))
                    .ok_or_else(|| format!("implicit variable `{name}` is not lowered yet")),
                Some(Res::BaseGlobal(name)) => {
                    let (slot, _, _) = self.base_var(name)?;
                    Ok(ir::Place::BaseGlobal(slot))
                }
                Some(Res::Field { table, id }) => {
                    let (table, id) = (*table, *id);
                    // A temporary record of a platform table is an ordinary temporary table: the
                    // columns the platform provider pins do not matter (record/
                    // TestPlatformTablesWritableContracts.al:27-48).
                    let temporary = n.kind() == SyntaxKind::MemberExpr
                        && n.children()
                            .next()
                            .is_some_and(|b| matches!(self.ty_ref(&b), Ty::Record { temporary: true, .. }));
                    if temporary {
                        self.field_pos(table, id).ok_or_else(|| "field position".to_string())?;
                    } else {
                        self.readable_field_pos(table, id)?;
                    }
                    let field = self.field_ref(table, id);
                    let base = match n.kind() {
                        SyntaxKind::MemberExpr => self.place(&n.children().next().ok_or("member base")?)?,
                        _ => ir::Place::Global(self.rec_slot.ok_or("implicit Rec is not lowered for this object yet")?),
                    };
                    Ok(ir::Place::Field(Box::new(base), field))
                }
                _ if n.kind() == SyntaxKind::NameRef && n.text().to_string().trim().eq_ignore_ascii_case("this") => Err(
                    "`this` passed by reference (`BindSubscription(this)`) is not lowered: no corpus test passes the running instance as an argument"
                        .into(),
                ),
                _ => Err(self
                    .base_member(n)
                    .or_else(|| self.control_add_in(n))
                    .unwrap_or_else(|| format!("`{}` is not assignable", n.text()))),
            },
        }
    }
}
