//! Lowering of expressions, places and constants.
use super::*;

impl Lowerer<'_> {
    /// The page or report that the page or report extension being lowered extends, with the noun
    /// for messages and whether it is a report (ADR-023).
    pub(crate) fn base_object(&self) -> Option<(usize, &'static str, bool)> {
        let (noun, report) = match self.ws.objects[self.oi].kind {
            ObjKind::PageExtension => ("page", false),
            ObjKind::ReportExtension => ("report", true),
            _ => return None,
        };
        Some((self.ws.extension_target(self.oi)?, noun, report))
    }

    /// Variable `name` (lower case) of the base object, seen from the extension being lowered
    /// (ADR-023): its slot among the base object's globals, and its type. Refused until the probes
    /// of ADR-023 pin the access.
    pub(crate) fn base_var(&self, name: &str) -> L<(usize, Ty, Option<String>)> {
        let (base, noun, report) = self.base_object().ok_or("a base variable outside an extension")?;
        let probe = if report { "R1" } else { "P1/P2" };
        if !(if report {
            crate::base_access::pinned("R1")
        } else {
            crate::base_access::pinned("P1") && crate::base_access::pinned("P2")
        }) {
            return Err(crate::base_access::unpinned_reason(&format!("`{name}`"), probe, noun));
        }
        let ast = self.ws.object_ast(base);
        let (_, decl) = crate::base_access::visible_vars(&ast)
            .into_iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("`{name}` is no protected variable of the {noun} it extends"))?;
        let ty = decl.type_ref().map(|t| Ty::from_type_ref(&t)).unwrap_or(Ty::Unknown);
        // Reference-like values (a record, a list, a dialog) are probe P5 (page) or R3 (report).
        let simple = matches!(
            ty,
            Ty::Integer
                | Ty::BigInteger
                | Ty::Decimal
                | Ty::Boolean
                | Ty::Char
                | Ty::Byte
                | Ty::Text(_)
                | Ty::Code(_)
                | Ty::Date
                | Ty::Time
                | Ty::DateTime
                | Ty::Duration
                | Ty::Guid
                | Ty::Option(_)
                | Ty::Enum(_)
                | Ty::Label
        );
        let kind_probe = if report { "R3" } else { "P5" };
        if !simple && !crate::base_access::pinned(kind_probe) {
            return Err(crate::base_access::unpinned_reason(
                &format!("`{name}` (a {ty} variable)"),
                kind_probe,
                noun,
            ));
        }
        // A control's source expression is probe P9.
        if !report && !crate::base_access::pinned("P9") && self.in_synthetic_routine() {
            return Err(crate::base_access::unpinned_reason(
                &format!("`{name}` in a control's source expression"),
                "P9",
                noun,
            ));
        }
        let items = if report {
            let l = self.layouts.report(base).ok_or("the base report has no layout")?;
            let mut seen = std::collections::HashSet::new();
            Some(l.items.iter().filter(|i| seen.insert(i.name.to_lowercase())).count())
        } else {
            None
        };
        let slot = crate::base_access::slot_of(&ast, items, name)
            .ok_or_else(|| format!("`{name}` has no slot in the {noun} it extends"))?;
        let label = decl.type_ref().as_ref().and_then(label_text);
        Ok((slot, ty, label))
    }

    /// A procedure of the extension with the name of a callable procedure of its base is compile
    /// error AL0440 in BC (corpus handlers/TestPageExtensionBaseAccessContracts.al:20), so it is
    /// refused: BC never runs such code.
    pub(crate) fn declared_in_both_proc(&self, object: usize, offset: usize) -> L<()> {
        if !crate::base_access::scope_enabled() {
            return Ok(());
        }
        let Some((base, noun, _)) = self.base_object() else {
            return Ok(());
        };
        let Some(name) = self.members[object]
            .procedures
            .iter()
            .find(|p| p.offset == offset)
            .map(|p| p.name.clone())
        else {
            return Ok(());
        };
        // Only the same parameter types are AL0440; a different signature is an overload (corpus
        // pageext/POPageExtOverload.al on the staging branch).
        let mine = ast::routines_in(self.ws.object_ast(object).syntax())
            .into_iter()
            .find(|r| usize::from(r.syntax().text_range().start()) == offset);
        let same_signature = |b: &ast::Routine| {
            mine.as_ref()
                .is_none_or(|m| crate::alcodes::signature(m) == crate::alcodes::signature(b))
        };
        if crate::base_access::visible_procs(&self.ws.object_ast(base), &name).any(|b| same_signature(&b)) {
            return Err(format!(
                "AL0440: the {noun} extension declares a method `{name}` that the {noun} it extends already defines (BC does not compile it)"
            ));
        }
        Ok(())
    }

    /// Whether extension code may call routine `routine` of its base object (ADR-023).
    pub(crate) fn base_call_allowed(&self, object: usize, routine: usize) -> L<()> {
        let (_, noun, report) = self.base_object().ok_or("a base call outside an extension")?;
        let (probe, ok) = if report {
            ("R1", crate::base_access::pinned("R1"))
        } else {
            ("P4", crate::base_access::pinned("P4"))
        };
        let node = self
            .routine_nodes
            .get(object)
            .get(routine)
            .ok_or("base procedure not found")?;
        let name = node.name();
        if !ok {
            return Err(crate::base_access::unpinned_reason(&format!("`{name}`"), probe, noun));
        }
        if !report && !crate::base_access::pinned("P10") && self.in_synthetic_routine() {
            return Err(crate::base_access::unpinned_reason(
                &format!("`{name}` in a control's source expression"),
                "P9/P10",
                noun,
            ));
        }
        for a in node.attributes() {
            let attr = a.name();
            if attr.eq_ignore_ascii_case("tryfunction") || attr.to_ascii_lowercase().ends_with("event") {
                return Err(format!(
                    "`{name}` is a [{attr}] procedure of the {noun} it extends: calling it from {noun} extension code is not lowered (ADR-023, refusals)"
                ));
            }
        }
        Ok(())
    }

    /// Whether the routine being lowered was made from a property expression (a control's source).
    pub(crate) fn in_synthetic_routine(&self) -> bool {
        self.routine_nodes
            .get(self.oi)
            .get(self.routine_idx)
            .is_some_and(|r| crate::pages::is_synthetic(&r.name()))
    }

    /// `CurrPage.Chart` where `Chart` is a `usercontrol` of this page: a control add-in, which the runtime
    /// does not model (no corpus test uses one; Base Application's chart pages pass theirs to a `var`
    /// parameter).
    pub(crate) fn control_add_in(&self, n: &SyntaxNode) -> Option<String> {
        let text = n.text().to_string();
        let name = text
            .trim()
            .strip_prefix("CurrPage.")?
            .trim()
            .trim_matches('"')
            .to_string();
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == ' ') {
            return None;
        }
        let object = self
            .ws
            .object_ast(self.oi)
            .syntax()
            .text()
            .to_string()
            .to_ascii_lowercase();
        let lower = name.to_ascii_lowercase();
        (object.contains(&format!("usercontrol({lower};")) || object.contains(&format!("usercontrol(\"{lower}\";")))
            .then(|| format!("`CurrPage.{name}` is a control add-in (usercontrol): control add-ins are not modelled"))
    }

    /// Why a name cannot be lowered in a page or report extension: it is a variable or procedure of
    /// the object the extension extends. Extension code runs on its own object, which holds none of
    /// the base object's variables or procedures (a bare control name binds to a base variable
    /// through `Binding::BaseVar`, corpus handlers/TestRequestPageControlErrorName.al). A report
    /// extension reaches the `protected` ones of its base report (Microsoft's "Mfg. Adjust Add. Rep.
    /// Currency" calls `Window.Open(..)` and `ExchangeAmtLCYToFCY(..)`).
    pub(crate) fn base_member(&self, n: &SyntaxNode) -> Option<String> {
        let noun = match self.ws.objects[self.oi].kind {
            ObjKind::PageExtension => "page",
            ObjKind::ReportExtension => "report",
            _ => return None,
        };
        let base = self.ws.extension_target(self.oi)?;
        let root = n
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .find(|t| matches!(t.kind(), SyntaxKind::Ident | SyntaxKind::QuotedIdent))?;
        let name = ast::ident_text(&root);
        let obj = self.ws.object_ast(base);
        let declared = obj
            .var_decls()
            .any(|v| v.names().iter().any(|x| x.eq_ignore_ascii_case(&name)))
            || obj.routines().any(|r| r.name().eq_ignore_ascii_case(&name));
        declared.then(|| {
            if crate::base_access::scope_enabled() {
                // The checker already resolves what extension code may name (ADR-023), so this one
                // is `local`, or a variable that is not `protected`.
                // AL0161 in BC (corpus handlers/TestPageExtensionBaseAccessContracts.al, header; the text
                // is the compiler's): BC never runs such code.
                let is_proc = obj.routines().any(|r| r.name().eq_ignore_ascii_case(&name))
                    && !obj.var_decls().any(|v| v.names().iter().any(|x| x.eq_ignore_ascii_case(&name)));
                let shown = if is_proc { format!("{name}()") } else { name.clone() };
                return format!(
                    "'{shown}' is inaccessible due to its protection level (a variable of the {noun} it extends that is not `protected`, or a `local` procedure; AL0161)"
                );
            }
            format!("`{name}` is a variable or procedure of the {noun} it extends: {noun} extension code on base {noun} members is not lowered yet (ADR-023: built, off until its probes ran)")
        })
    }

    pub(crate) fn constant(&self, n: &SyntaxNode) -> L<ir::Const> {
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
                // `0T` is the blank time, `000000T` is midnight
                // (types/TestFormatEvaluateTextContracts.al:Format_Time_StandardFormats).
                if d.chars().all(|c| c == '0') && d.len() < 6 {
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

    pub(crate) fn expr(&mut self, n: &SyntaxNode) -> L<ir::Expr> {
        use SyntaxKind::*;
        let kids: Vec<SyntaxNode> = n.children().collect();
        match n.kind() {
            Literal => Ok(ir::Expr::Const(self.constant(n)?)),
            // `this` in a codeunit is the running instance (corpus handlers/ProbeEventSubscriberOrder.al:
            // `BindSubscription(this)`).
            NameRef
                if crate::check::is_this_keyword(n)
                    && self.ws.objects[self.oi].kind == ObjKind::Codeunit
                    && !self.globals.contains_key("this") =>
            {
                Ok(ir::Expr::ThisInstance)
            }
            ParenExpr => self.expr(&kids[0]),
            PrefixExpr => {
                let op = match sig_tokens(n)[0].text().to_ascii_lowercase().as_str() {
                    "not" => ir::UnOp::Not,
                    "-" => ir::UnOp::Neg,
                    _ => ir::UnOp::Plus,
                };
                Ok(ir::Expr::Unary(
                    op,
                    Box::new(self.operand(&kids[0], ir::OperandUse::Value)?),
                ))
            }
            BinExpr if matches!(sig_tokens(n)[0].text().to_ascii_lowercase().as_str(), "is" | "as") => {
                self.interface_test(n, &kids)
            }
            // The parser recovered from text it did not accept and left one operand.
            BinExpr if kids.len() < 2 => {
                Err("binary expression with a missing operand (the parser did not accept it)".into())
            }
            BinExpr => {
                let op = sig_tokens(n)[0].text().to_ascii_lowercase();
                // How a settled DotNet operand may be used depends on the operator (ADR-024).
                let operand_use = match op.as_str() {
                    "+" | "=" | "<>" => ir::OperandUse::Enum,
                    "<" | "<=" | ">" | ">=" => ir::OperandUse::NoText,
                    _ => ir::OperandUse::Value,
                };
                if op == "in" {
                    self.no_dotnet_operand(&kids[0])?;
                }
                let l = if op == "in" {
                    self.expr(&kids[0])?
                } else {
                    self.operand(&kids[0], operand_use)?
                };
                if op == "in" {
                    let set = &kids[1];
                    if set.kind() != SetExpr {
                        return Err("`in` with a non-literal set is not lowered yet".into());
                    }
                    let elems = set.children().map(|e| self.set_elem(&e)).collect::<L<Vec<_>>>()?;
                    return Ok(ir::Expr::In(Box::new(l), elems));
                }
                let r = self.operand(&kids[1], operand_use)?;
                let dotnet =
                    matches!(self.ty_of(&kids[0]), Ty::DotNet(_)) || matches!(self.ty_of(&kids[1]), Ty::DotNet(_));
                let op = match op.as_str() {
                    "=" if dotnet => ir::BinOp::DotNetEq,
                    "<>" if dotnet => ir::BinOp::DotNetNe,
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
            TernaryExpr if kids.iter().any(|k| matches!(self.ty_of(k), Ty::DotNet(_))) => {
                Err("a DotNet value in a conditional expression: its type is only known at run time".into())
            }
            // `Text[i]` read: the character at i. The base is any text, a call result included
            // (corpus json/TestJsonXmlDeepContracts.al:71 `Format(10, 0, '<Integer>')[1]`).
            IndexExpr
                if matches!(kids.first().map(|k| self.ty_of(k)), Some(Ty::Text(_) | Ty::Code(_)))
                    && kids.len() == 2 =>
            {
                Ok(ir::Expr::CharAt(
                    Box::new(self.expr(&kids[0])?),
                    Box::new(self.operand(&kids[1], ir::OperandUse::Value)?),
                ))
            }
            IndexExpr if matches!(kids.first().map(|k| self.ty_of(k)), Some(Ty::DotNet(_))) => {
                Err("indexing a DotNet value is not lowered yet (call get_Item / GetValue instead)".into())
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
                // `Mode::Decompress` with `Mode: DotNet CompressionMode`: the type's static member.
                Some(Res::DotNetMember(name)) => {
                    let Ty::DotNet(alias) = self.ty_of(n) else {
                        return Err(format!("`{name}` is not a member of a DotNet value"));
                    };
                    let ty = self.ws.resolve_dotnet(self.file, &alias).ok_or_else(|| {
                        format!("DotNet '{alias}' has no dotnet {{ }} declaration in this app or its dependencies")
                    })?;
                    Ok(ir::Expr::DotNet {
                        ty: Some(ty),
                        receiver: Some(Box::new(self.expr(&kids[0])?)),
                        op: ir::DotNetOp::Get(name),
                        args: Vec::new(),
                    })
                }
                Some(Res::EnumValue { object, value }) => Ok(ir::Expr::Const(ir::Const::Enum(object as u32, value))),
                Some(Res::OptionValue(i)) => match self.ty_of(n) {
                    Ty::Option(m) => Ok(ir::Expr::Const(ir::Const::Option(i, m))),
                    Ty::Builtin(b) if al_builtins::sysenum::members(&b).is_some() => Ok(ir::Expr::Const(
                        ir::Const::Option(i, al_builtins::sysenum::option_members(&b).expect("checked")),
                    )),
                    _ => Err("option value without its option type".into()),
                },
                Some(Res::ObjectId(id)) => Ok(ir::Expr::Const(ir::Const::Integer(id))),
                _ => {
                    // `Enum::Member` of a system enum whose member list has no such member.
                    let text = n.text().to_string();
                    match text.split_once("::") {
                        Some((e, m)) if al_builtins::sysenum::members(e.trim()).is_some() => Err(format!(
                            "`{}::{}` is not a member of the system enum {} in the member list this runtime holds, which only holds the members a corpus probe lists: its ordinal is not pinned (docs/probes-needed.md)",
                            e.trim(),
                            m.trim().trim_matches('"'),
                            e.trim()
                        )),
                        _ => Err(format!("`{text}` is not lowered yet")),
                    }
                }
            },
            MemberExpr if self.is_control_access(n) => self.control(n),
            // `CurrPage.<Part>.Page`: the page the part shows.
            MemberExpr if self.is_curr_page_part_page(n) => {
                let part = n.children().next().ok_or("part page without a part")?;
                self.control(&part)
            }
            NameRef | MemberExpr | IndexExpr => match self.res(n).cloned() {
                Some(Res::Proc { .. }) | Some(Res::Builtin(_)) | Some(Res::DotNetMember(_)) => self.call(n, n, &[]),
                // `Q.Amount`: the current row's value of a query column.
                Some(Res::QueryColumn { query, column })
                    if n.kind() == MemberExpr
                        && query == self.oi
                        && kids[0].kind() == NameRef
                        && kids[0].text().to_string().trim().eq_ignore_ascii_case("this") =>
                {
                    // `this.Id` (corpus query/PRQueryOpenColumn.al `PeekThis`).
                    let layout = self.checked.queries[query].as_ref().ok_or("query layout")?;
                    if layout.query.columns[column].filter_element {
                        return Err("reading a filter element of a query (not pinned)".into());
                    }
                    let slot = *self.globals.get("currquery").ok_or("the running query")?;
                    Ok(ir::Expr::QueryColumn {
                        query: Box::new(ir::Expr::Load(ir::Place::Global(slot))),
                        column,
                    })
                }
                Some(Res::QueryColumn { query, column }) if n.kind() == MemberExpr => {
                    let layout = self.checked.queries[query].as_ref().ok_or("query layout")?;
                    if layout.query.columns[column].filter_element {
                        return Err("reading a filter element of a query (not pinned)".into());
                    }
                    Ok(ir::Expr::QueryColumn {
                        query: Box::new(self.expr(&kids[0])?),
                        column,
                    })
                }
                // A bare column name in the query's own code reads the running query's current row
                // (Base Application's "Summarized Stock By Item Trkg." does `X := Serial_No_;`; no corpus test
                // does, docs/probes-needed.md).
                Some(Res::QueryColumn { query, column }) if n.kind() == NameRef && query == self.oi => {
                    let layout = self.checked.queries[query].as_ref().ok_or("query layout")?;
                    if layout.query.columns[column].filter_element {
                        return Err("reading a filter element of a query (not pinned)".into());
                    }
                    let slot = *self.globals.get("currquery").ok_or("the running query")?;
                    Ok(ir::Expr::QueryColumn {
                        query: Box::new(ir::Expr::Load(ir::Place::Global(slot))),
                        column,
                    })
                }
                // `Database::A.B."Name"`: a namespace-qualified object id.
                Some(Res::ObjectId(id)) => Ok(ir::Expr::Const(ir::Const::Integer(id))),
                // `this.BaseVar` in a reportextension compiles and raises when it runs (types/PYThisExtensions.al:
                // ReportExtension_ThisBaseVariable_ReadInATrigger, BC 27.5 and 29.0). For a page extension BC aborts the
                // page with a text this corpus does not pin.
                Some(Res::ThisBaseGlobal(_)) => match (self.ws.objects[self.oi].kind, self.ws.objects[self.oi].id) {
                    (ObjKind::ReportExtension, Some(id)) => Ok(ir::Expr::Fail {
                        message: format!(
                            "The another object refers to a global variable on ReportExtension with ID {id} that no longer seems to exists. Try to recompile dependent extensions."
                        ),
                        code: "GlobalVariableNotFound".into(),
                    }),
                    _ => Err("`this.<variable of the base>` in this extension (what BC raises is not pinned)".into()),
                },
                Some(Res::Local(name)) if self.local_labels.contains_key(&name) => {
                    Ok(ir::Expr::Const(ir::Const::Text(self.local_labels[&name].clone())))
                }
                Some(Res::Global(name)) if self.global_labels.contains_key(&name) => {
                    Ok(ir::Expr::Const(ir::Const::Text(self.global_labels[&name].clone())))
                }
                // A Label of the base object is its text.
                Some(Res::BaseGlobal(name)) if self.base_var(&name)?.2.is_some() => Ok(ir::Expr::Const(
                    ir::Const::Text(self.base_var(&name)?.2.unwrap_or_default()),
                )),
                // `CurrFieldNo` in table code: a property of the implicit `Rec` the VM reads.
                Some(Res::Global(name)) if name == "currfieldno" && !self.globals.contains_key(&name) => {
                    let rec = self
                        .rec_slot
                        .ok_or("CurrFieldNo outside code that has an implicit Rec")?;
                    Ok(ir::Expr::Builtin {
                        id: "Record.CurrFieldNo#1".into(),
                        receiver: Some(Box::new(ir::Arg::Ref(ir::Place::Global(rec)))),
                        args: Vec::new(),
                        ret_used: true,
                    })
                }
                // `GetLocation(Code)."Bin Mandatory"`: a field of what a procedure returns. A call result is not a
                // place, so the base is evaluated and the field read off the record it gives. No corpus test
                // reads a field of a returned record (docs/probes-needed.md).
                Some(Res::Field { table, id })
                    if n.kind() == MemberExpr && kids.first().is_some_and(|b| b.kind() == SyntaxKind::CallExpr) =>
                {
                    let pos = self.readable_field_pos(table, id)?;
                    Ok(ir::Expr::FieldOf(Box::new(self.expr(&kids[0])?), pos))
                }
                _ => Ok(ir::Expr::Load(self.place(n)?)),
            },
            other => Err(format!("expression {other:?} `{}` is not lowered yet", n.text())),
        }
    }

    /// `TP.Ctl`: a member access on a test page or part that names a control.
    pub(crate) fn is_control_access(&self, n: &SyntaxNode) -> bool {
        let Some(recv) = n.children().next() else { return false };
        let request = matches!(self.ty_of(&recv), Ty::TestRequestPage(_));
        (matches!(self.ty_of(&recv), Ty::TestPage(_) | Ty::TestPart(_)) || request)
            && (matches!(self.ty_of(n), Ty::TestPart(_))
                || matches!(self.ty_of(n), Ty::Builtin(b) if b == "TestField" || b == "TestAction")
                // A data item of a request page.
                || (request && matches!(self.ty_of(n), Ty::TestFilter(_))))
    }

    /// `CurrPage.<Part>.Page`
    pub(crate) fn is_curr_page_part_page(&self, n: &SyntaxNode) -> bool {
        let mut kids = n.children();
        let (Some(part), Some(_)) = (kids.next(), kids.next()) else {
            return false;
        };
        let last_is_page = n
            .children()
            .last()
            .and_then(|m| {
                m.children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| matches!(t.kind(), SyntaxKind::Ident | SyntaxKind::QuotedIdent))
            })
            .is_some_and(|t| ast::ident_text(&t).eq_ignore_ascii_case("Page"));
        last_is_page
            && matches!(self.ty_of(&part), Ty::TestPart(_))
            && part.kind() == SyntaxKind::MemberExpr
            && part
                .children()
                .next()
                .is_some_and(|r| matches!(self.ty_of(&r), Ty::Object(ObjKind::Page, _)))
    }

    pub(crate) fn control(&mut self, n: &SyntaxNode) -> L<ir::Expr> {
        let recv = n.children().next().ok_or("control access without a receiver")?;
        let name = n
            .children()
            .last()
            .and_then(|m| {
                m.children_with_tokens()
                    .filter_map(|e| e.into_token())
                    .find(|t| matches!(t.kind(), SyntaxKind::Ident | SyntaxKind::QuotedIdent))
            })
            .map(|t| ast::ident_text(&t))
            .ok_or("control name")?;
        self.control_named(&recv, &name)
    }

    /// The control `name` of the test page (or part, or request page) that `recv` evaluates to.
    pub(crate) fn control_named(&mut self, recv: &SyntaxNode, name: &str) -> L<ir::Expr> {
        let (Ty::TestPage(page) | Ty::TestPart(page) | Ty::Object(ObjKind::Page, page) | Ty::TestRequestPage(page)) =
            self.ty_of(recv)
        else {
            return Err("control of a non-page".into());
        };
        let kind = if matches!(self.ty_of(recv), Ty::TestRequestPage(_)) {
            ObjKind::Report
        } else {
            ObjKind::Page
        };
        let page = self.resolve_obj(kind, &page)?;
        self.embeds.borrow_mut().insert(page);
        let layout = self.layouts.page_layout(page).ok_or("page layout")?;
        let control = layout
            .controls
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| format!("control `{name}` is not in the page layout"))?;
        let runtime = matches!(self.ty_of(recv), Ty::Object(ObjKind::Page, _));
        Ok(ir::Expr::Control {
            page: Box::new(self.expr(recv)?),
            control,
            runtime,
        })
    }

    /// An expression used as an operand. A `DotNet` value has no static type: it is settled when
    /// the code runs, from the kind the host returned (ADR-024).
    pub(crate) fn operand(&mut self, n: &SyntaxNode, how: ir::OperandUse) -> L<ir::Expr> {
        let e = self.expr(n)?;
        Ok(match self.ty_of(n) {
            Ty::DotNet(_) => ir::Expr::DotNetOperand(Box::new(e), how),
            _ => e,
        })
    }

    /// The left side of `in` keeps the static refusal: a membership test against a set of
    /// literals is not pinned for a settled value (ADR-024, refusals that stay).
    pub(crate) fn no_dotnet_operand(&self, n: &SyntaxNode) -> L<()> {
        match self.ty_of(n) {
            Ty::DotNet(_) => Err(format!(
                "`{}` is a DotNet value used as an operand: its type is only known at run time; assign it to a typed variable first",
                n.text().to_string().trim()
            )),
            _ => Ok(()),
        }
    }

    /// A condition: a DotNet result must be a Boolean.
    pub(crate) fn condition(&mut self, n: &SyntaxNode) -> L<ir::Expr> {
        let e = self.expr(n)?;
        Ok(match self.ty_of(n) {
            Ty::DotNet(a) if a.is_empty() => ir::Expr::DotNetCoerce(RtType::Boolean, Box::new(e)),
            _ => e,
        })
    }
}
