//! Lowering of calls.
use super::*;

impl Lowerer<'_> {
    /// A call resolved on `node` (the CallExpr, or the callee for a call without parentheses).
    pub(crate) fn call(&mut self, node: &SyntaxNode, callee: &SyntaxNode, args: &[SyntaxNode]) -> L<ir::Expr> {
        if let Some(Res::DotNetMember(name)) = self.res(node).cloned() {
            let args = args.iter().map(|a| self.dotnet_arg(a)).collect::<L<Vec<_>>>()?;
            let op = if node.kind() == SyntaxKind::CallExpr {
                ir::DotNetOp::Call(name.clone())
            } else {
                ir::DotNetOp::Get(name.clone())
            };
            return self.dotnet_member(node, callee, &name, op, args);
        }
        // `this.Proc(...)` calls on the running instance, as the bare call does.
        let receiver_node = (callee.kind() == SyntaxKind::MemberExpr)
            .then(|| callee.children().next())
            .flatten()
            .filter(|r| !crate::check::is_this_keyword(r));
        match self.res(node).cloned() {
            Some(Res::Proc { object, offset }) if self.ws.objects[object].kind == ObjKind::Interface => {
                self.interface_call(object, offset, receiver_node.as_ref(), args)
            }
            Some(Res::Proc { object, offset }) => {
                let routine = self.routine_at.get(&(object, offset)).ok_or("procedure not found")?;
                let sig = self.members[object]
                    .procedures
                    .iter()
                    .find(|p| p.offset == offset)
                    .ok_or("procedure signature")?
                    .clone();
                let instance = match &receiver_node {
                    // `this.Proc()` runs on the current instance, like a bare call.
                    Some(r)
                        if r.kind() == SyntaxKind::NameRef
                            && r.text().to_string().trim().eq_ignore_ascii_case("this") =>
                    {
                        if object != self.oi {
                            return Err("`this` calls a procedure of another object".into());
                        }
                        None
                    }
                    Some(r)
                        if matches!(
                            self.ty_of(r),
                            Ty::Object(
                                ObjKind::Codeunit | ObjKind::Page | ObjKind::XmlPort | ObjKind::Report | ObjKind::Query,
                                _
                            ) | Ty::Record { .. }
                        ) =>
                    {
                        Some(Box::new(self.expr(r)?))
                    }
                    Some(_) => return Err("procedure calls on this receiver are not lowered yet".into()),
                    None if object == self.oi => {
                        self.declared_in_both_proc(object, offset)?;
                        None
                    }
                    // A page or report extension calling a procedure of the page or report it extends.
                    None if self.base_object().is_some_and(|(b, _, _)| b == object) => {
                        self.base_call_allowed(object, routine)?;
                        Some(Box::new(ir::Expr::BaseInstance))
                    }
                    // A table extension calling its table's procedure: on the same Rec.
                    None if self.table_of[object].is_some() && self.rec_slot.is_some() => Some(Box::new(
                        ir::Expr::Load(ir::Place::Global(self.rec_slot.expect("checked"))),
                    )),
                    None => return Err("implicit calls into another object are not lowered yet".into()),
                };
                let mut out = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let p = sig.params.get(i).ok_or("too many arguments")?;
                    if p.by_var {
                        out.push(self.by_var_arg(a)?);
                    } else {
                        let value = self.expr(a)?;
                        out.push(ir::Arg::Value(self.convert_to(&p.ty, self.ty_ref(a), value)?));
                    }
                }
                self.calls.push((object, routine));
                // The routine node is the one `routine_at` numbered, so no search by offset.
                let try_call = self
                    .routine_nodes
                    .get(object)
                    .get(routine)
                    .is_some_and(|r| r.attributes().any(|a| a.name().eq_ignore_ascii_case("tryfunction")));
                Ok(ir::Expr::Call {
                    object,
                    routine,
                    instance,
                    args: out,
                    try_call,
                })
            }
            // MaxStrLen reads the declared length of its argument's type, which values do not
            // carry (text/TestTextExtended.al MaxStrLen_*: 50 for Text[50], 2147483647 for Text).
            Some(Res::Builtin(id)) if id.starts_with("Text.MaxStrLen#") => {
                let arg = args.first().ok_or("MaxStrLen without an argument")?;
                match self.ty_of(arg) {
                    Ty::Text(Some(n)) | Ty::Code(Some(n)) => Ok(ir::Expr::Const(ir::Const::Integer(n as i32))),
                    Ty::Text(None) | Ty::Code(None) => Ok(ir::Expr::Const(ir::Const::Integer(i32::MAX))),
                    // A TextConst has the length 2048 (types/lt-variant/VaTTextConst.al:TextConst_MaxStrLen: Simple, Long, Empty and
                    // MultiLang). A Label answers 2048 too (text/lt-label-textbuilder/TxCLabel.al:Label_MaxLength).
                    Ty::Label => Ok(ir::Expr::Const(ir::Const::Integer(2048))),
                    other => Err(format!("MaxStrLen of a value of type {other}")),
                }
            }
            // ArrayLen reads the shape of its argument's type, which array values do not keep
            // (streams/TestArrayStreamContracts.al ArrayLen_*).
            Some(Res::Builtin(id)) if id.starts_with("System.ArrayLen#") => {
                let arg = args.first().ok_or("ArrayLen without an argument")?;
                let Ty::Array(dims, _) = self.ty_of(arg) else {
                    return Err("ArrayLen of something that is not an array variable".into());
                };
                // A one-dimensional array is measured when the call runs: a parameter holds the caller's array.
                if dims.len() == 1 && args.len() == 1 {
                    return Ok(ir::Expr::Builtin {
                        id: builtin_id(&id)?,
                        receiver: None,
                        args: vec![ir::Arg::Ref(self.place(arg)?)],
                        ret_used: true,
                    });
                }
                let dim = match args.get(1) {
                    Some(d) => match self.expr(d)? {
                        ir::Expr::Const(ir::Const::Integer(k)) => Some(k),
                        _ => return Err("ArrayLen with a dimension that is not a constant".into()),
                    },
                    None if dims.len() == 1 => Some(1),
                    // Without a dimension the total number of elements
                    // (streams/TestStreamsRuntimeContracts.al:ArrayLen_MultiDimensional_WithAndWithoutDimension).
                    None => {
                        let total: u32 = dims.iter().product();
                        return Ok(ir::Expr::Const(ir::Const::Integer(total as i32)));
                    }
                };
                match dim.and_then(|k| usize::try_from(k - 1).ok()).and_then(|k| dims.get(k)) {
                    Some(n) => Ok(ir::Expr::Const(ir::Const::Integer(*n as i32))),
                    None => Err("ArrayLen with a dimension outside the array (error text not pinned)".into()),
                }
            }
            Some(Res::Builtin(id)) => {
                if al_builtins::native::lookup(&id).is_none() {
                    return Err(match al_builtins::native::refusal(&id) {
                        Some(why) => format!("builtin {id}: {why} (docs/probes-needed.md)"),
                        None => format!("builtin {id} is not implemented"),
                    });
                }
                let sig = self
                    .catalog
                    .by_id(&id)
                    .ok_or_else(|| format!("{id} not in catalog"))?
                    .clone();
                // Evaluate into an Option variable: a variable has no OptionCaption, so a text that is neither an ordinal nor a
                // member name is a plain rejection (P86Eval.al:Eval Option, `Bet`, `-1`, `1,0`). A field may name a member by its
                // caption, which the value does not carry, so a field keeps the id that refuses it.
                let emit_id = if id == "System.Evaluate#1"
                    && args.first().is_some_and(|a| {
                        matches!(self.ty_of(a), Ty::Option(_))
                            && matches!(self.res(a), Some(Res::Local(_) | Res::Global(_)))
                    }) {
                    "System.EvaluateVariableOption#1".to_string()
                } else {
                    id.clone()
                };
                let receiver = match &receiver_node {
                    // `CurrReport` is the report whose code is running: the VM takes the running
                    // instance when a report builtin has no receiver.
                    Some(r)
                        if sig.instance
                            && sig.receiver == "Report"
                            && matches!(
                                self.ws.objects[self.oi].kind,
                                ObjKind::Report | ObjKind::ReportExtension
                            )
                            && r.text().to_string().trim().eq_ignore_ascii_case("CurrReport") =>
                    {
                        None
                    }
                    // `RequestOptionsPage` is the request page of the report whose code is running; it has
                    // no variable to take as a receiver.
                    Some(r)
                        if sig.instance
                            && sig.receiver == "RequestPage"
                            && matches!(
                                self.ws.objects[self.oi].kind,
                                ObjKind::Report | ObjKind::ReportExtension
                            )
                            && r.text().to_string().trim().eq_ignore_ascii_case("RequestOptionsPage") =>
                    {
                        None
                    }
                    // Enum metadata calls (`"Name".Names()`, `Var.Ordinals()`, `"Name".FromInteger(n)`)
                    // receive the enum itself: its object index, as an enum constant.
                    Some(r) if sig.receiver == "Enum" && sig.name != "AsInteger" => {
                        let Ty::Enum(name) = self.ty_of(r) else {
                            return Err(format!("{id}: the receiver is not an enum type"));
                        };
                        let object = self.resolve_obj(ObjKind::Enum, &name)?;
                        Some(Box::new(ir::Arg::Value(ir::Expr::Const(ir::Const::Enum(
                            object as u32,
                            0,
                        )))))
                    }
                    // A Label is its text: the variable holds none (reads lower to the constant).
                    Some(r) if sig.instance && sig.receiver == "Label" => Some(Box::new(ir::Arg::Value(self.expr(r)?))),
                    Some(r) if sig.instance => Some(Box::new(match self.place(r) {
                        Ok(p) => ir::Arg::Ref(p),
                        Err(_) => ir::Arg::Value(self.expr(r)?),
                    })),
                    // An unqualified record method in table code runs on the implicit Rec.
                    None if sig.instance && matches!(sig.receiver.as_str(), "Record" | "Query") => {
                        Some(Box::new(ir::Arg::Ref(ir::Place::Global(
                            self.rec_slot.ok_or("implicit Rec is not lowered for this object yet")?,
                        ))))
                    }
                    _ => None,
                };
                let mut out = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let p = sig
                        .params
                        .get(i)
                        .or_else(|| sig.params.last().filter(|p| p.variadic))
                        .ok_or("too many arguments")?;
                    // `Clear(X)` of a declared DotNet variable goes by place like any other variable.
                    let clears_dotnet =
                        id == "System.Clear#1" && matches!(self.ty_ref(a), Ty::DotNet(al) if !al.is_empty());
                    if let (false, Ty::DotNet(alias)) = (clears_dotnet, self.ty_of(a)) {
                        // `IsNull(DotNet)` takes the value as it is. A `Variant` parameter takes a
                        // primitive, an object or an enum as it is (`Format` of an object is its
                        // `ToString()`). A scalar parameter takes a DotNet result (no static type)
                        // when its run-time kind fits the parameter, as in an assignment (ADR-024
                        // section 2). Anything else is refused.
                        let scalar = match p.ty.as_str() {
                            "Text" | "Code" => Some(RtType::Text(None)),
                            "Integer" => Some(RtType::Integer),
                            "BigInteger" => Some(RtType::BigInteger),
                            "Decimal" => Some(RtType::Decimal),
                            "Boolean" => Some(RtType::Boolean),
                            "Char" => Some(RtType::Char),
                            "Byte" => Some(RtType::Byte),
                            "Date" => Some(RtType::Date),
                            "Time" => Some(RtType::Time),
                            "DateTime" => Some(RtType::DateTime),
                            "Duration" => Some(RtType::Duration),
                            "Guid" => Some(RtType::Guid),
                            _ => None,
                        };
                        let value = match p.ty.as_str() {
                            "DotNet" => self.expr(a)?,
                            // CopyStream takes a DotNet stream as it is; the VM copies the bytes
                            // through the host (ADR-024 section 3).
                            "InStream" | "OutStream" if !p.by_var && id.starts_with("System.CopyStream#") => {
                                self.expr(a)?
                            }
                            "Any" | "Variant" if !p.by_var => {
                                ir::Expr::DotNetOperand(Box::new(self.expr(a)?), ir::OperandUse::Variant)
                            }
                            _ => match scalar {
                                Some(rt) if !p.by_var && alias.is_empty() => ir::Expr::DotNetCoerce(
                                    rt,
                                    Box::new(ir::Expr::DotNetOperand(Box::new(self.expr(a)?), ir::OperandUse::Strict)),
                                ),
                                _ => {
                                    return Err(format!(
                                        "a DotNet value passed to builtin {id}: its type is only known at run time; assign it to a typed variable first"
                                    ));
                                }
                            },
                        };
                        out.push(ir::Arg::Value(value));
                        continue;
                    }
                    // A bare control name in a field position of a test page method.
                    if a.kind() == SyntaxKind::NameRef
                        && self.res(a).is_none()
                        && matches!(self.ty_ref(a), Ty::Builtin(b) if &**b == "TestField")
                        && receiver_node.as_ref().is_some_and(|r| {
                            matches!(
                                self.ty_of(r),
                                Ty::TestPage(_) | Ty::TestRequestPage(_) | Ty::TestPart(_)
                            )
                        })
                    {
                        let name = a
                            .children_with_tokens()
                            .filter_map(|e| e.into_token())
                            .find(|t| matches!(t.kind(), SyntaxKind::Ident | SyntaxKind::QuotedIdent))
                            .map(|t| ast::ident_text(&t))
                            .ok_or("control name")?;
                        let recv = receiver_node.clone().expect("checked");
                        out.push(ir::Arg::Value(self.control_named(&recv, &name)?));
                        continue;
                    }
                    // `Source.CopyFilter(FromField, Target."ToField")`: the target is a field of another
                    // record variable, passed as that record and the field's position.
                    if id == "Record.CopyFilter#1" && i == 1 {
                        let Some(Res::Field { table, id: fid }) = self.res(a).cloned() else {
                            return Err("CopyFilter to something that is not a field of a record".into());
                        };
                        self.readable_field_pos(table, fid)?;
                        let field = self.field_ref(table, fid);
                        let base = match a.kind() {
                            SyntaxKind::MemberExpr => self.place(&a.children().next().ok_or("member base")?)?,
                            _ => ir::Place::Global(
                                self.rec_slot.ok_or("implicit Rec is not lowered for this object yet")?,
                            ),
                        };
                        out.push(ir::Arg::Ref(base));
                        out.push(ir::Arg::Value(ir::Expr::Const(ir::Const::FieldPos(field as u32))));
                        continue;
                    }
                    // A parameter named Field... that is an Integer takes the number as a VALUE (`RecRef.Field(Buf."Field No.")`,
                    // `RecRef.FieldExist(R.Number)`); a field reference is passed for a parameter that names a field.
                    let field_param = (p.name.to_ascii_lowercase().starts_with("field") && p.ty != "Integer")
                        || p.name.eq_ignore_ascii_case("FromField")
                        || p.name.to_ascii_lowercase().starts_with("column");
                    match (field_param, self.res(a).cloned()) {
                        (true, Some(Res::Field { table, id })) => {
                            self.readable_field_pos(table, id)?;
                            let field = self.field_ref(table, id);
                            out.push(ir::Arg::Value(ir::Expr::Const(ir::Const::FieldPos(field as u32))));
                            continue;
                        }
                        // A query column is named by its position in the query's column list.
                        // The query is the table; the column's position in its list is the field number.
                        (true, Some(Res::QueryColumn { query, column })) => {
                            let field = self.field_ref(query, column as u32);
                            out.push(ir::Arg::Value(ir::Expr::Const(ir::Const::FieldPos(field as u32))));
                            continue;
                        }
                        _ => {}
                    }
                    // `Page.FindFirstField(Name, v)` names the control bare. The corpus only
                    // qualifies it (`ListPage."Integer Field"`, handlers/TestBCReportHandlerContracts.al:247);
                    // what a bare name means (the receiver's controls first?) is not pinned.
                    if p.ty == "TestField"
                        && a.kind() != SyntaxKind::MemberExpr
                        && !matches!(self.res(a), Some(Res::Local(_) | Res::Global(_)))
                    {
                        return Err(format!(
                            "the TestField argument `{}` is a bare control name: only the qualified form is pinned by a corpus test",
                            a.text()
                        ));
                    }
                    // Generic parameters convert to the collection's element types.
                    let elem = match (p.ty.as_str(), receiver_node.as_ref().map(|r| self.ty_of(r))) {
                        ("T", Some(Ty::List(el))) => Some(*el),
                        ("TKey", Some(Ty::Dictionary(k, _))) => Some(*k),
                        ("TValue", Some(Ty::Dictionary(_, v))) => Some(*v),
                        _ => None,
                    };
                    out.push(if p.by_var {
                        ir::Arg::Ref(self.place(a)?)
                    } else if let Some(el) = elem {
                        ir::Arg::Value(ir::Expr::Convert(self.rt(&el)?, Box::new(self.expr(a)?)))
                    // A Variant argument with a standard format number 3 to 8 raises an error that names the type
                    // 'Variant' (corpus types/lt-scalars/ScT70052.al:Integer_Numbers, n3 to n8, and the other ScT Fmt
                    // codeunits). The value in a Variant does not carry that, so the number is checked here.
                    } else if id.starts_with("System.Format#")
                        && i == 2
                        && args.len() == 3
                        && matches!(self.ty_of(&args[0]), Ty::Variant)
                        && matches!(self.ty_of(a), Ty::Integer)
                    {
                        let number = self.expr(a)?;
                        let literal = a.text().to_string().trim().parse::<i32>().ok();
                        let simple = matches!(self.res(a), Some(Res::Local(_) | Res::Global(_)));
                        if literal.is_none() && !simple {
                            return Err("Format of a Variant with a standard format number that is an expression: the error for a number the type lacks names the type 'Variant', which is checked only for a literal or a variable".into());
                        }
                        let mut guarded = number.clone();
                        for n in (3..=8).rev() {
                            guarded = ir::Expr::Ternary(
                                Box::new(ir::Expr::Binary(
                                    ir::BinOp::Eq,
                                    Box::new(number.clone()),
                                    Box::new(ir::Expr::Const(ir::Const::Integer(n))),
                                )),
                                Box::new(ir::Expr::Fail {
                                    message: format!(
                                        "Standard format number {n} does not exist for the type 'Variant'."
                                    ),
                                    code: "InvalidStandardFormat".into(),
                                }),
                                Box::new(guarded),
                            );
                        }
                        ir::Arg::Value(guarded)
                    } else if id.starts_with("System.Format#") && i == 0 {
                        ir::Arg::Value(self.format_option_field(args.len() == 1, a)?)
                    // The substituted values of StrSubstNo, Message and Error are `Format`ted: an Option
                    // field shows its caption (corpus xmlport/PWXmlPortFormatEvaluate.al:Import_Xml_Option,
                    // `%3` of a field with OptionCaption).
                    } else if i > 0
                        && (id.starts_with("Text.StrSubstNo#")
                            || id.starts_with("Dialog.Message#")
                            || id.starts_with("Dialog.Error#"))
                    {
                        ir::Arg::Value(self.format_option_field(true, a)?)
                    } else {
                        ir::Arg::Value(self.expr(a)?)
                    });
                }
                Ok(ir::Expr::Builtin {
                    id: builtin_id(&emit_id)?,
                    receiver,
                    args: out,
                    ret_used: true,
                })
            }
            // The checker refused a builtin of a newer runtime (ADR-015 3.3): say why.
            None if self.runtime_refusal(node).is_some() => Err(self.runtime_refusal(node).unwrap_or_default()),
            // `Window.Open(..)` in a report extension, where `Window` is a variable of the base report.
            None if self.base_member(callee).is_some() => Err(self.base_member(callee).unwrap_or_default()),
            // `CurrPage.<AddIn>.Method()`: refused when it runs, so code that never reaches it still runs
            // (Base Application's "Item Tracking Lines" calls its barcode add-in only after the client said it
            // is supported). Control add-ins are not modelled; no corpus test calls one.
            None if callee.kind() == SyntaxKind::MemberExpr
                && callee.children().next().and_then(|b| self.control_add_in(&b)).is_some() =>
            {
                let why = callee
                    .children()
                    .next()
                    .and_then(|b| self.control_add_in(&b))
                    .unwrap_or_default();
                Ok(ir::Expr::Refused(format!("call `{}`: {why}", node.text())))
            }
            None => Err(format!("call `{}` has no callee the checker resolved", node.text())),
            Some(r) => Err(format!(
                "call `{}` to {} is not lowered yet",
                node.text(),
                match r {
                    Res::Local(_) | Res::Global(_) => "a variable",
                    Res::Field { .. } | Res::QueryColumn { .. } => "a field",
                    Res::EnumValue { .. } | Res::OptionValue(_) => "an enum or option value",
                    Res::ObjectId(_) => "an object id",
                    _ => "this kind of callee",
                }
            )),
        }
    }

    /// The checker's AR1005 on this call, if any.
    pub(crate) fn runtime_refusal(&self, node: &SyntaxNode) -> Option<String> {
        let path = &self.ws.files[self.file].path;
        let at: usize = node.text_range().start().into();
        self.checked
            .diagnostics
            .iter()
            .find(|d| d.code == "AR1005" && d.offset == at && &d.file == path)
            .map(|d| format!("AR1005 {}", d.message))
    }
}

/// The [`ir::BuiltinId`] of a catalog overload id. Every catalog id has one (a test in al-builtins checks it), so
/// this fails only when `builtin_ids.rs` is stale.
pub(super) fn builtin_id(id: &str) -> Result<ir::BuiltinId, String> {
    ir::BuiltinId::of(id).ok_or_else(|| format!("builtin {id} has no builtin id (regenerate builtin_ids.rs)"))
}
