use super::*;

impl Session {
    /// Runs a block. `CAP` is whether a capture watches the routine (`capture.rs`): the two instances are two
    /// functions, so a run without a capture executes code that has no trace of it.
    pub(crate) fn exec_block<const CAP: bool>(&mut self, stmts: &[ir::Stmt], f: &mut Frame) -> Result<(), Signal> {
        if CAP {
            return self.exec_block_captured(stmts, f);
        }
        for s in stmts {
            self.exec::<false>(s, f)?;
        }
        Ok(())
    }

    /// [`Session::exec_block`] for the instance the routine running now needs.
    pub(crate) fn exec_block_dyn(&mut self, stmts: &[ir::Stmt], f: &mut Frame) -> Result<(), Signal> {
        if self.capturing {
            self.exec_block::<true>(stmts, f)
        } else {
            self.exec_block::<false>(stmts, f)
        }
    }

    pub(crate) fn exec<const CAP: bool>(&mut self, s: &ir::Stmt, f: &mut Frame) -> Result<(), Signal> {
        use ir::Stmt as S;
        match s {
            S::Assume(a) => {
                al_builtins::assumptions::assume(*a);
                Ok(())
            }
            S::Assign(p, e) => {
                // `S += x` and `S := S + x` on a text variable append in place when nothing else holds the text.
                if let (ir::Place::Local(_) | ir::Place::Global(_), ir::Expr::Convert(RtType::Text(max), inner)) =
                    (p, e)
                {
                    if let ir::Expr::Binary(ir::BinOp::Add, l, r) = &**inner {
                        if matches!(&**l, ir::Expr::Load(lp) if lp == p) {
                            return self.append_text(p, *max, r, f);
                        }
                    }
                }
                let v = self.eval(e, f)?;
                self.write(p, v, f)
            }
            S::Expr(e) => self.eval(e, f).map(|_| ()),
            S::If(c, t, e) => {
                if ops::truthy(&self.eval(c, f)?)? {
                    self.exec_block::<CAP>(t, f)
                } else {
                    self.exec_block::<CAP>(e, f)
                }
            }
            S::While(c, body) => {
                let here = self.call_stack.last().map(|t| t.pos);
                let lid = if CAP { self.cap_loop_enter() } else { None };
                while ops::truthy(&self.eval(c, f)?)? {
                    self.check_stop()?;
                    if CAP {
                        self.cap_loop_pass(lid);
                    }
                    match self.exec_block::<CAP>(body, f) {
                        Err(Signal::Break) => break,
                        Err(Signal::Continue) => {}
                        r => r?,
                    }
                    // The condition belongs to the loop's line, not to the body's last statement.
                    self.restore_pos(here);
                }
                if CAP {
                    self.cap_loop_exit(lid);
                }
                Ok(())
            }
            S::Repeat(body, until) => {
                let here = self.call_stack.last().map(|t| t.pos);
                let lid = if CAP { self.cap_loop_enter() } else { None };
                loop {
                    self.check_stop()?;
                    if CAP {
                        self.cap_loop_pass(lid);
                    }
                    match self.exec_block::<CAP>(body, f) {
                        Err(Signal::Break) => break,
                        Err(Signal::Continue) => {}
                        r => r?,
                    }
                    self.restore_pos(here);
                    if ops::truthy(&self.eval(until, f)?)? {
                        break;
                    }
                }
                if CAP {
                    self.cap_loop_exit(lid);
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
                let (from, to) = (self.eval(from, f)?, self.eval(to, f)?);
                // A loop over a Boolean (`for Stepwise := false to true do`, 27 Microsoft codeunits) runs for false and
                // then for true, the variable holding the Boolean each round (registry entry for-boolean). Not pinned.
                if matches!(from, Value::Boolean(_)) || matches!(to, Value::Boolean(_)) {
                    al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::ForBoolean);
                }
                // Option bounds of an Integer loop take its type as an assignment does: the ordinal (Microsoft's
                // "Library - Manufacturing" loops `for Day := FromDay to ToDay` with Option parameters; pinned by the
                // corpus Option_InCaseAndLoopBounds, types/ for_to_option).
                let bound = |v: Value| match v {
                    Value::Option(o, _) | Value::Enum(_, o) => Value::Integer(o),
                    Value::Boolean(b) => Value::Integer(i32::from(b)),
                    v => v,
                };
                if matches!(from, Value::Enum(..)) || matches!(to, Value::Enum(..)) {
                    al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::ForOptionVariable);
                }
                let (from, to) = (bound(from), bound(to));
                let (start, end) = (int(&from)?, int(&to)?);
                let mut i = start;
                let lid = if CAP { self.cap_loop_enter() } else { None };
                // The type of the counter variable does not change while the loop runs: found once.
                let slot_ty = self.place_type(var, f);
                let option_like = matches!(slot_ty, RtType::Option(_) | RtType::Enum(_));
                let is_boolean = matches!(slot_ty, RtType::Boolean);
                let is_integer = matches!(slot_ty, RtType::Integer);
                while if *down { i >= end } else { i <= end } {
                    self.check_stop()?;
                    if CAP {
                        self.cap_loop_pass(lid);
                    }
                    if option_like {
                        al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::ForOptionVariable);
                    }
                    let counter = if is_boolean {
                        al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::ForBoolean);
                        Value::Boolean(i != 0)
                    } else if is_integer && let Ok(n) = i32::try_from(i) {
                        // What the conversion of a BigInteger into an Integer gives, without the Decimal in between; a
                        // counter beyond Integer takes the general path, which reports it.
                        Value::Integer(n)
                    } else {
                        ops::convert(Value::BigInteger(i), &slot_ty)?
                    };
                    self.write(var, counter, f)?;
                    if CAP {
                        self.cap_loop_var(var, f);
                    }
                    match self.exec_block::<CAP>(body, f) {
                        Err(Signal::Break) => break,
                        Err(Signal::Continue) => {}
                        r => r?,
                    }
                    // The counter is read back from the variable, which may be an Option (an Option field as the
                    // loop variable, Sales Price Calc. Mgt.): its ordinal.
                    i = int(&bound(self.read(var, f)?))?;
                    i += if *down { -1 } else { 1 };
                }
                if CAP {
                    self.cap_loop_exit(lid);
                }
                Ok(())
            }
            S::Foreach { var, collection, body } => {
                let coll = self.eval(collection, f)?;
                // A .NET collection, or a DotNet variable that holds none (ADR-024 section 4).
                let dotnet_var = matches!(coll, Value::Null)
                    && matches!(collection, ir::Expr::Load(p) if matches!(self.place_type(p, f), RtType::DotNet(_)));
                if matches!(coll, Value::DotNet(_)) || dotnet_var {
                    return self.foreach_dotnet(var, coll, collection, body, f);
                }
                let items: Vec<Value> = match coll {
                    Value::List(l) => l.borrow().clone(),
                    // The nodes an XPath query found (corpus handlers/TestReportFlowFieldColumnAutoCalc.al).
                    other => match al_builtins::json::array_items(&other)
                        .or_else(|| al_builtins::xml::node_list_items(&other))
                    {
                        Some(items) => items,
                        None => return Err(Signal::Unsupported(format!("foreach over {}", other.describe()))),
                    },
                };
                let lid = if CAP { self.cap_loop_enter() } else { None };
                for item in items {
                    if CAP {
                        self.cap_loop_pass(lid);
                    }
                    self.write(var, item, f)?;
                    if CAP {
                        self.cap_loop_var(var, f);
                    }
                    match self.exec_block::<CAP>(body, f) {
                        Err(Signal::Break) => break,
                        Err(Signal::Continue) => {}
                        r => r?,
                    }
                }
                if CAP {
                    self.cap_loop_exit(lid);
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
                        return self.exec_block::<CAP>(body, f);
                    }
                }
                self.exec_block::<CAP>(otherwise, f)
            }
            S::Exit(e) => {
                let v = match e {
                    Some(e) => Some(self.eval(e, f)?),
                    None => None,
                };
                Err(Signal::Exit(v))
            }
            S::Break => Err(Signal::Break),
            S::Continue => Err(Signal::Continue),
            S::Pos(p) => {
                if let Some(top) = self.call_stack.last_mut() {
                    top.pos = *p;
                }
                if profile::ON.load(std::sync::atomic::Ordering::Relaxed) {
                    self.profile_sample();
                }
                if self.coverage.code_coverage {
                    self.coverage_hit(*p);
                }
                if let Some((counts, touched)) = &mut self.coverage.coverage_counts {
                    let i = *p as usize;
                    if counts.len() <= i {
                        counts.resize(self.engine.program.positions.len().max(i + 1), 0);
                    }
                    if counts[i] == 0 {
                        touched.push(*p);
                    }
                    counts[i] += 1;
                }
                Ok(())
            }
            S::AssertError(body) => {
                // A caught error rolls the transaction back to the last commit, also inside a modal
                // page handler: the write pending before the page opened and the handler's own
                // earlier writes are lost with it
                // (record/TestValidateRaisesAndHandlerAsserterrorContracts.al:122-143,
                // Asserterror_InAModalHandler_RollsBackToTheLastCommit).
                match self.exec_block::<CAP>(body, f) {
                    Err(Signal::Error(mut e)) => {
                        // The caught error rolls the transaction back to the last commit
                        // (record/TestRecordRenameRollback.al).
                        self.rollback_transaction();
                        // The routine that catches it is the last frame of its call stack.
                        self.add_top_frame(&mut e);
                        self.last_error = *e;
                        Ok(())
                    }
                    // Text and code: types/TestFormatEvaluateTextContracts.al:Asserterror_NothingRaised_ReportsAssertError.
                    Ok(()) => Err(Signal::Error(Box::new(AlError {
                        message: "An error was expected inside an ASSERTERROR statement.".into(),
                        code: "AssertError".into(),
                        call_stack: String::new(),
                    }))),
                    Err(other) => Err(other),
                }
            }
        }
    }

    pub(crate) fn restore_pos(&mut self, pos: Option<u32>) {
        if let (Some(p), Some(top)) = (pos, self.call_stack.last_mut()) {
            top.pos = p;
        }
    }

    pub(crate) fn matches_any(&mut self, v: &Value, labels: &[ir::SetElem], f: &mut Frame) -> Result<bool, Signal> {
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

    pub(crate) fn eval(&mut self, e: &ir::Expr, f: &mut Frame) -> Result<Value, Signal> {
        use ir::Expr as E;
        match e {
            E::Const(c) => Ok(self.constant_once(c)),
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
                    B::Lt => Value::Boolean(ops::compare_order(&l, &r)? == Ordering::Less),
                    B::Le => Value::Boolean(ops::compare_order(&l, &r)? != Ordering::Greater),
                    B::Gt => Value::Boolean(ops::compare_order(&l, &r)? == Ordering::Greater),
                    B::Ge => Value::Boolean(ops::compare_order(&l, &r)? != Ordering::Less),
                    B::DotNetEq => Value::Boolean(dotnet::dotnet_equal(&l, &r)?),
                    B::DotNetNe => Value::Boolean(!dotnet::dotnet_equal(&l, &r)?),
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
                        // A procedure of a reportextension called through the report variable runs on the extension's
                        // instance of that variable, which keeps the extension's globals and added data items
                        // (reportext/W3RpExtProcedures.al:ExtensionProcedure_*).
                        Value::Codeunit(i)
                            if self.engine.program.objects[*object].report_ext.is_some()
                                && i.borrow().object as usize != *object =>
                        {
                            self.report_extension_instance(&i, *object)?
                        }
                        Value::Codeunit(i) => self.shared_instance(i),
                        // A table procedure runs with the record as its Rec, on the record
                        // variable's own globals (ADR-021).
                        Value::Record(rc) => {
                            let (inst, saved) = self.enter_record(*object, rc, None)?;
                            let r = self.finish_call(*object, *routine, inst.clone(), args, *try_call, f);
                            Self::leave_record(&inst, saved);
                            return r;
                        }
                        // A query procedure runs with the query it was called on as its own state.
                        Value::Query(q) => self.query_instance(*object, q),
                        // The page a part shows (`CurrPage.<Part>.Page`), or a `Page` variable, which
                        // is constructed at first use.
                        Value::TestPage(st) => {
                            let i = self.construct_page(&st)?;
                            // A procedure a page extension declares runs on the extension's instance.
                            let ext = st
                                .borrow()
                                .ext_instances
                                .iter()
                                .find(|(e, _)| e == object)
                                .map(|(_, i)| i.clone());
                            self.shared_instance(ext.unwrap_or(i))
                        }
                        other => {
                            return Err(Signal::Unsupported(format!(
                                "a procedure call on a {} value",
                                value_kind(&other)
                            )));
                        }
                    },
                };
                self.finish_call(*object, *routine, inst, args, *try_call, f)
            }
            E::BaseInstance => Ok(Value::Codeunit(self.base_instance(f)?)),
            E::ThisInstance => Ok(Value::Codeunit(f.instance.clone())),
            E::Refused(why) => Err(Signal::Unsupported(why.clone())),
            E::Fail { message, code } => Err(Signal::Error(Box::new(al_builtins::native::AlError {
                message: message.clone(),
                code: code.clone(),
                call_stack: String::new(),
            }))),
            E::InterfaceCall {
                receiver,
                targets,
                args,
            } => {
                let (object, routine, inst) = self.interface_target(receiver, targets, f)?;
                self.finish_call(object, routine, inst, args, false, f)
            }
            E::InterfaceTest { value, objects, cast } => {
                let v = self.eval(value, f)?;
                self.interface_test(v, objects, *cast)
            }
            E::EnumImpl {
                value,
                by_ordinal,
                unknown,
            } => {
                let v = self.eval(value, f)?;
                self.enum_implementation(v, by_ordinal, *unknown)
            }
            E::Control { page, control, runtime } => {
                let p = self.eval(page, f)?;
                self.page_control(p, *control, *runtime)
            }
            E::FieldOf(base, pos) => {
                al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::FieldOfProcedureResult);
                let record = self.eval(base, f)?;
                self.read_record_field(record, *pos)
            }
            E::QueryColumn { query, column } => {
                let q = self.eval(query, f)?;
                self.query_column(q, *column)
            }
            E::DotNet { ty, receiver, op, args } => self.eval_dotnet(*ty, receiver.as_deref(), op, args, f),
            E::CharAt(text, index) => {
                let t = self.eval(text, f)?;
                let Some(t) = al_builtins::native::text_of(&t) else {
                    return Err(Signal::Unsupported(format!("indexing {} as a text", t.describe())));
                };
                let i = int(&self.eval(index, f)?)?;
                // types/TestProbeScalarContracts.al:TextChar_AssignByIndex: position 0 and below raise, the position after
                // the last character reads as the character 0; further positions are not pinned.
                if i < 1 {
                    return al_builtins::native::raise(
                        format!(
                            "The value of INDEXING parameter 1 is outside of the permitted range. The current value is: {i}. The permitted range is: from 1 to 2,147,483,647."
                        ),
                        "OutsidePermittedRange",
                    );
                }
                if t.is_ascii() {
                    // No copy of a long text for each character read.
                    return match t.as_bytes().get(i as usize - 1) {
                        Some(u) => Ok(Value::Char(u16::from(*u))),
                        None if i as usize == t.len() + 1 => Ok(Value::Char(0)),
                        None => Err(Signal::Unsupported(
                            "a text read more than one position past its length (not pinned)".into(),
                        )),
                    };
                }
                let units: Vec<u16> = t.encode_utf16().collect();
                match units.get(i as usize - 1) {
                    Some(u) => Ok(Value::Char(*u)),
                    None if i as usize == units.len() + 1 => Ok(Value::Char(0)),
                    None => Err(Signal::Unsupported(
                        "a text read more than one position past its length (not pinned)".into(),
                    )),
                }
            }
            E::DotNetCoerce(t, e) => {
                self.dotnet.clear_null_string();
                let v = self.eval(e, f)?;
                self.dotnet_coerce(v, t)
            }
            E::DotNetOperand(e, how) => {
                self.dotnet.clear_null_string();
                let v = self.eval(e, f)?;
                self.settle_operand(v, *how)
            }
            E::Builtin {
                id,
                receiver,
                args,
                ret_used,
            } => {
                use al_builtins::builtin_id::{Class, ids};
                let id = *id;
                // Event subscriptions are session state: the VM runs them (dispatch.rs).
                if let ids::SESSION_BINDSUBSCRIPTION_1 | ids::SESSION_UNBINDSUBSCRIPTION_1 = id {
                    let Some(ir::Arg::Value(e)) = args.first() else {
                        return Err(Signal::Unsupported(format!("{id} with this argument")));
                    };
                    let cu = self.eval(e, f)?;
                    return self.subscription(id == ids::SESSION_BINDSUBSCRIPTION_1, cu, *ret_used);
                }
                // `CurrFieldNo` is a property of the implicit Rec, not a catalog method: the lowerer
                // emits this id with the Rec as receiver.
                if id == ids::RECORD_SETCURRFIELDNO_1 {
                    let (Some(ir::Arg::Ref(p)), Some(ir::Arg::Value(e))) = (receiver.as_deref(), args.first()) else {
                        return Err(Signal::Unsupported("CurrFieldNo := without a record".into()));
                    };
                    let Value::Record(rec) = self.read(p, f)? else {
                        return Err(Signal::Unsupported("CurrFieldNo of something that is no record".into()));
                    };
                    let Value::Integer(n) = self.eval(e, f)? else {
                        return Err(Signal::Unsupported("CurrFieldNo := a value that is no Integer".into()));
                    };
                    match self
                        .pages
                        .curr_field_no
                        .iter_mut()
                        .rev()
                        .find(|(w, _)| std::ptr::eq(w.as_ptr(), Rc::as_ptr(&rec)))
                    {
                        Some(entry) => entry.1 = n,
                        None => {
                            // Entries of dropped records are dead; forget them before adding one.
                            self.pages.curr_field_no.retain(|(w, _)| w.strong_count() > 0);
                            self.pages.curr_field_no.push((Rc::downgrade(&rec), n));
                        }
                    }
                    return Ok(Value::Null);
                }
                if id == ids::RECORD_CURRFIELDNO_1 {
                    let Some(ir::Arg::Ref(p)) = receiver.as_deref() else {
                        return Err(Signal::Unsupported("CurrFieldNo without a record".into()));
                    };
                    let Value::Record(rec) = self.read(p, f)? else {
                        return Err(Signal::Unsupported("CurrFieldNo of something that is no record".into()));
                    };
                    return Ok(Value::Integer(self.curr_field_of(&rec)));
                }
                // `ClearAll()` resets every global variable of the running object to its default;
                // locals are untouched (docs/probes-needed.md "ClearAll"). In a codeunit with a TableNo,
                // a table and a page the record stays as it is and so does the page itself
                // (session/TestClearAllTransactionTypeContracts.al:224-285, BC 27.5). Any other object
                // kind is refused.
                if id == ids::SYSTEM_CLEARALL_1 {
                    let object = f.instance.borrow().object as usize;
                    let o = &self.engine.program.objects[object];
                    let keep: Vec<usize> = match o.kind {
                        al_ir::ObjKind::Codeunit | al_ir::ObjKind::Table => o.rec_slot.into_iter().collect(),
                        al_ir::ObjKind::Page => o
                            .rec_slot
                            .into_iter()
                            .chain(o.page.as_ref().map(|p| p.curr_page_slot))
                            .collect(),
                        _ => {
                            return Err(Signal::Unsupported(format!("ClearAll in a {} (not pinned)", o.kind)));
                        }
                    };
                    let tys: Vec<RtType> = o.globals.iter().map(|g| g.ty.clone()).collect();
                    let fresh: Vec<Value> = tys.iter().map(|t| self.default_value(t)).collect();
                    let mut inst = f.instance.borrow_mut();
                    for (i, v) in fresh.into_iter().enumerate() {
                        if !keep.contains(&i) {
                            inst.globals[i] = v;
                        }
                    }
                    return Ok(Value::Null);
                }
                if id == ids::SESSION_APPLICATIONAREA_1 {
                    // `ApplicationArea()` reads the session's areas, `ApplicationArea(Text)` sets them
                    // (corpus pageapplicationarea/TestPageApplicationAreaControlRemoval.al).
                    return match args.first() {
                        None => Ok(Value::Text(self.application_areas.clone().into())),
                        Some(ir::Arg::Value(e)) => match self.eval(e, f)? {
                            Value::Text(t) | Value::Code(t) => {
                                self.application_areas = t.to_string();
                                Ok(Value::Null)
                            }
                            other => Err(Signal::Unsupported(format!("ApplicationArea({})", other.describe()))),
                        },
                        Some(_) => Err(Signal::Unsupported("ApplicationArea with this argument".into())),
                    };
                }
                let class = id.class();
                if class != Class::Other && class != Class::InStreamRead
                    || id == ids::REQUESTPAGE_UPDATE_1
                    || id == ids::DATABASE_ISINWRITETRANSACTION_1
                {
                    let recv = match receiver.as_deref() {
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
                    if id == ids::DATABASE_ISINWRITETRANSACTION_1 {
                        return Ok(Value::Boolean(self.in_write_transaction()));
                    }
                    if class == Class::Xmlport {
                        return self.xmlport_builtin(id, recv, &values, *ret_used);
                    }
                    if id == ids::REQUESTPAGE_UPDATE_1 {
                        return self.request_page_update(&values);
                    }
                    if class == Class::Report {
                        self.ret_used = *ret_used;
                        return self.report_builtin(id, recv, &values, f.instance.clone());
                    }
                    if class == Class::FilterPageBuilder {
                        return self.filter_page_builder_op(id, recv, &values, *ret_used);
                    }
                    return self.codeunit_run(id, recv, &values, *ret_used);
                }
                let native =
                    al_builtins::native::by_id(id).ok_or_else(|| Signal::Unsupported(format!("builtin {id}")))?;
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
                self.ret_used = *ret_used;
                let ret = native(self, recv.as_mut(), &mut values)?;
                if let (Some(ir::Arg::Ref(p)), Some(v)) = (receiver.as_deref(), recv) {
                    self.write(p, v, f)?;
                }
                for (a, v) in args.iter().zip(values) {
                    if let ir::Arg::Ref(p) = a {
                        let v = if class == Class::InStreamRead {
                            self.fit_stream_read(p, v, f)?
                        } else {
                            v
                        };
                        self.write(p, v, f)?;
                    }
                }
                Ok(ret)
            }
        }
    }

    /// What `InStream.Read` stores in a Text or Code variable: a Code takes the characters
    /// upper-cased and cut to its length without an error (streams/ProbeJsonAndInStreamBytes.al:
    /// InStreamRead_Text_ReturnValueIsLength reads `toolongvalueforcode` into a Code[10] and gets
    /// `TOOLONGVAL`). A Text that is too short for the characters is not pinned.
    pub(crate) fn fit_stream_read(&mut self, p: &ir::Place, v: Value, f: &mut Frame) -> Result<Value, Signal> {
        match (&v, self.place_type(p, f)) {
            (Value::Code(t), RtType::Code(max)) => {
                let up = t.to_uppercase();
                Ok(Value::Code(match max {
                    Some(m) => up.chars().take(m as usize).collect(),
                    None => up.into(),
                }))
            }
            (Value::Text(t), RtType::Text(Some(m))) if t.chars().count() > m as usize => Err(Signal::Unsupported(
                "InStream.Read of more characters than the Text variable holds (not pinned)".into(),
            )),
            _ => Ok(v),
        }
    }

    /// What `CurrFieldNo` reads on `rec`: the field number a TestPage write is validating on this
    /// record, 0 anywhere else (handlers/TestPageCurrFieldNo_Tests.al:36-123).
    pub(crate) fn curr_field_of(&self, rec: &Rc<RefCell<RecordState>>) -> i32 {
        self.pages
            .curr_field_no
            .iter()
            .rev()
            .find(|(w, _)| std::ptr::eq(w.as_ptr(), Rc::as_ptr(rec)))
            .map_or(0, |(_, n)| *n)
    }

    /// Evaluates the arguments of a resolved call and runs it on `inst`, writing back the
    /// by-reference arguments.
    pub(crate) fn finish_call(
        &mut self,
        object: usize,
        routine: usize,
        inst: Rc<RefCell<Instance>>,
        args: &[ir::Arg],
        try_call: bool,
        f: &mut Frame,
    ) -> Result<Value, Signal> {
        let mut values = Vec::with_capacity(args.len());
        for a in args {
            values.push(match a {
                ir::Arg::Value(e) => self.eval(e, f)?,
                ir::Arg::Ref(p) => self.read(p, f)?,
            });
        }
        let result = self.call(object, routine, inst, values);
        if let Err(Signal::Error(_)) = &result {
            if let Some(finals) = self.error_finals.take() {
                for (a, v) in args.iter().zip(finals) {
                    if let ir::Arg::Ref(p) = a {
                        self.write(p, v, f)?;
                    }
                }
            }
        }
        let (ret, finals) = match (result, try_call) {
            (Err(Signal::Error(mut e)), true) => {
                self.add_top_frame(&mut e);
                self.last_error = *e;
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

    /// The instance a call on codeunit value `i` runs on: the session's
    /// singleton for `SingleInstance = true` codeunits, `i` itself otherwise.
    pub(crate) fn shared_instance(&mut self, i: Rc<RefCell<Instance>>) -> Rc<RefCell<Instance>> {
        let object = i.borrow().object as usize;
        let single = self.engine.program.objects[object]
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("SingleInstance") && v.trim().eq_ignore_ascii_case("true"));
        if !single {
            return i;
        }
        if !self.singletons.contains_key(&object) {
            let inst = self.instance(object);
            self.singletons.insert(object, inst);
        }
        self.singletons[&object].clone()
    }

    /// The instance of the page or report that the running extension extends (ADR-023). A missing
    /// or dropped link is reported, never a panic.
    pub(crate) fn base_instance(&self, f: &Frame) -> Result<Rc<RefCell<Instance>>, Signal> {
        f.instance
            .borrow()
            .base
            .as_ref()
            .and_then(std::rc::Weak::upgrade)
            .ok_or_else(|| {
                Signal::Unsupported(
                    "internal error: extension code named a member of its base object, but the extension's instance \
                     has no live link to the base object's instance"
                        .into(),
                )
            })
    }

    /// The declared type of a local, a global, or an element of an array of those, by reference.
    pub(crate) fn place_ty<'a>(&'a self, p: &ir::Place, f: &Frame) -> Option<&'a RtType> {
        match p {
            ir::Place::Local(i) => Some(&self.engine.program.objects[f.object].routines[f.routine].locals[*i].ty),
            ir::Place::Global(i) => {
                let o = f.instance.borrow().object as usize;
                Some(&self.engine.program.objects[o].globals[*i].ty)
            }
            ir::Place::Index(base, _) => match self.place_ty(base, f)? {
                RtType::Array(_, el) => Some(el),
                _ => None,
            },
            _ => None,
        }
    }

    pub(crate) fn place_type(&self, p: &ir::Place, f: &Frame) -> RtType {
        if let Some(t) = self.place_ty(p, f) {
            return t.clone();
        }
        let r = &self.engine.program.objects[f.object].routines[f.routine];
        match p {
            ir::Place::Local(i) => r.locals[*i].ty.clone(),
            ir::Place::Global(i) => {
                let o = f.instance.borrow().object as usize;
                self.engine.program.objects[o].globals[*i].ty.clone()
            }
            ir::Place::BaseGlobal(i) => match self.base_instance(f) {
                Ok(b) => {
                    let o = b.borrow().object as usize;
                    self.engine.program.objects[o].globals[*i].ty.clone()
                }
                Err(_) => RtType::Variant,
            },
            ir::Place::Index(base, _) => match self.place_type(base, f) {
                RtType::Array(_, el) => *el,
                other => other,
            },
            ir::Place::Field(base, pos) => match self.place_type(base, f) {
                RtType::Record(t, _) => self.engine.tables[t as usize]
                    .as_ref()
                    .map_or(RtType::Variant, |td| td.fields[*pos].ty.clone()),
                _ => RtType::Variant,
            },
        }
    }

    /// `P := P + R` for a Text variable `P`: the text is taken out of the variable, `R` is appended to it (in
    /// place when no other value shares it, so a loop of `S += x` is linear), and it is put back. The left
    /// operand is the value before `R` is evaluated, as in the general path; the length limit and the
    /// call-stack marker rule are the general path's (`ops::convert`, `ops::arith`).
    pub(crate) fn append_text(
        &mut self,
        p: &ir::Place,
        max: Option<u32>,
        r: &ir::Expr,
        f: &mut Frame,
    ) -> Result<(), Signal> {
        let old = self.read(p, f)?;
        let rv = self.eval(r, f)?;
        let v = match (old, &rv) {
            (Value::Text(mut t), Value::Text(x) | Value::Code(x))
                if !t.has_marker()
                    && !x.has_marker()
                    && max.is_none_or(|m| t.utf16_len() + x.utf16_len() <= m as usize) =>
            {
                // Let go of the variable's hold so the append can happen in place.
                match p {
                    ir::Place::Local(i) => f.locals[*i] = Value::Null,
                    ir::Place::Global(i) => f.instance.borrow_mut().globals[*i] = Value::Null,
                    _ => unreachable!("a local or a global"),
                }
                t.push_str(x);
                ops::convert(Value::Text(t), &RtType::Text(max))?
            }
            (old, rv) => ops::convert(ops::arith(ops::Arith::Add, &old, rv)?, &RtType::Text(max))?,
        };
        self.write(p, v, f)
    }

    pub(crate) fn read(&mut self, p: &ir::Place, f: &mut Frame) -> Result<Value, Signal> {
        match p {
            ir::Place::Local(i) => Ok(f.locals[*i].clone()),
            ir::Place::Global(i) => Ok(f.instance.borrow().globals[*i].clone()),
            ir::Place::BaseGlobal(i) => {
                let base = self.base_instance(f)?;
                let v = base.borrow().globals.get(*i).cloned();
                v.ok_or_else(|| base_slot_missing(*i))
            }
            ir::Place::Index(base, idx) => {
                let (arr, flat) = self.element(base, idx, f)?;
                let v = arr.borrow()[flat].clone();
                Ok(v)
            }
            ir::Place::Field(base, pos) => {
                let record = self.read(base, f)?;
                self.read_record_field(record, *pos)
            }
        }
    }

    /// The value of field `pos` of the record `record` holds (the read of `Rec.Field`, also of a field of a procedure's
    /// result).
    pub(crate) fn read_record_field(&mut self, record: Value, pos: usize) -> Result<Value, Signal> {
        let pos = &pos;
        match record {
            Value::Record(rc) => {
                self.connection_check(&rc)?;
                let r = rc.borrow();
                if !self.engine.unevaluable_init.is_empty() && !r.touched {
                    // The buffer of a record nothing has written is built by this read, which
                    // evaluates the InitValues (record/TestDateTimeInitValue.Codeunit.al:
                    // DateTimeInitValue_Table_UntouchedRecordRaisesOnReadNotOnInit).
                    if let Some(message) = self.engine.unevaluable_init.get(&(r.table, *pos)) {
                        return Err(Signal::Error(Box::new(AlError {
                            message: message.clone(),
                            code: String::new(),
                            call_stack: String::new(),
                        })));
                    }
                }
                if !self.engine.unevaluable_init.is_empty() && r.touched {
                    let engine = self.engine.clone();
                    if let Some(message) = engine.unevaluable_init.get(&(r.table, *pos)) {
                        let pristine = self.initial_fields(r.table as usize);
                        if ops::compare(&r.fields[*pos], &pristine[*pos]).ok() == Some(std::cmp::Ordering::Equal) {
                            return Err(Signal::Unsupported(format!(
                                "a read of the field whose InitValue BC cannot evaluate (`{message}`), in a record where another field was written (not pinned)"
                            )));
                        }
                    }
                }
                let v = r.fields[*pos].clone();
                if r.load.partial {
                    drop(r);
                    rc.borrow_mut().load.touch(*pos);
                }
                Ok(v)
            }
            other => Err(Signal::Unsupported(format!("field of {}", other.describe()))),
        }
    }

    pub(crate) fn write(&mut self, p: &ir::Place, v: Value, f: &mut Frame) -> Result<(), Signal> {
        // `:=` between FilterPageBuilders copies the control collection and shares the records
        // (corpus filterpage/TestFilterPageBuilder.al:FilterPageBuilder_Assignment_*).
        let v = match v {
            Value::Library(al_builtins::library_kind::LibraryKind::FilterPageBuilder, cell) => Value::Library(
                al_builtins::library_kind::LibraryKind::FilterPageBuilder,
                filterpage::copy_for_assignment(&cell),
            ),
            v => v,
        };
        // `:=` between records copies the fields into the target record (ADR-010).
        if let Value::Record(src) = &v {
            if !matches!(p, ir::Place::Field(..)) {
                if let Ok(Value::Record(dst)) = self.read(p, f) {
                    if !Rc::ptr_eq(src, &dst) {
                        // A copied record holds copies of its Blobs (value semantics). The corpus
                        // does not tell a copy from a shared Blob (record/TestFormatRecordContracts.al
                        // only formats the copy): docs/probes-needed.md.
                        let fields: Vec<Value> = src
                            .borrow()
                            .fields
                            .iter()
                            .map(|f| match f {
                                Value::Blob(b) => {
                                    let b = b.borrow();
                                    Value::Blob(std::sync::Arc::new(al_builtins::stream::BlobCell::new(
                                        al_builtins::stream::BlobBuf {
                                            bytes: b.bytes.clone(),
                                            write_pos: 0,
                                            // A copy shows the bytes (and a stream over it reads them, streams/
                                            // TestProbeStreamContracts.al:RecordAssign_CopiesBlobBytes), but CalcFields
                                            // fetches the Blob from the stored row again (types/ProbeTypeFormsAndFilters.al:
                                            // FormatRecord_AssignmentCopiesBlobBytes, BC 27.5).
                                            loaded: true,
                                            refetch: true,
                                            persisted: false,
                                        },
                                    )))
                                }
                                other => other.clone(),
                            })
                            .collect();
                        let load = src.borrow().load.clone();
                        let mut d = dst.borrow_mut();
                        d.fields = fields;
                        // The copy holds what the source's buffer held, if that is known.
                        d.load.buffer = load.buffer;
                        d.load.partial = load.partial;
                    }
                    return Ok(());
                }
            }
        }
        if let ir::Place::Field(base, pos) = p {
            let ty = self.place_type(p, f);
            let v = ops::convert(v, &ty)?;
            return match self.read(base, f)? {
                Value::Record(rc) => {
                    self.connection_check(&rc)?;
                    let mut r = rc.borrow_mut();
                    r.fields[*pos] = v;
                    r.touched = true;
                    if r.load.partial {
                        r.load.written(*pos);
                    }
                    Ok(())
                }
                other => Err(Signal::Unsupported(format!("field of {}", other.describe()))),
            };
        }
        // A record assigned to a Variant is a by-value copy: a temporary record's copy is an empty table of its own
        // (recordref/PWRecordMisc.al:TempRecord_AssignedToVariantVariable and :TempRecord_VariantModifiedAfterAssign).
        let v = match &v {
            Value::Record(rc) if matches!(self.place_type(p, f), RtType::Variant) => self.record_by_value(rc),
            _ => v,
        };
        match p {
            ir::Place::Local(i) => f.locals[*i] = v,
            ir::Place::Global(i) => f.instance.borrow_mut().globals[*i] = v,
            ir::Place::BaseGlobal(i) => {
                let base = self.base_instance(f)?;
                let mut b = base.borrow_mut();
                match b.globals.get_mut(*i) {
                    Some(slot) => *slot = v,
                    None => return Err(base_slot_missing(*i)),
                }
            }
            ir::Place::Index(base, idx) if matches!(self.place_type(base, f), RtType::Text(_) | RtType::Code(_)) => {
                return self.write_char(base, idx, v, f);
            }
            ir::Place::Index(base, idx) => {
                let el_ty = self.place_type(p, f);
                let (arr, flat) = self.element(base, idx, f)?;
                arr.borrow_mut()[flat] = ops::convert(v, &el_ty)?;
            }
            ir::Place::Field(..) => unreachable!("handled above"),
        }
        Ok(())
    }

    /// `Text[i] := c`: sets one character of a text or code, at a position inside it or the next one within
    /// the declared length (handlers/TestPageFieldFormats_Tests.al:58-66,
    /// types/TestCharIndexAssignContracts.al:CharacterAssignedIntoAPosition); a position outside that and a value
    /// that is not one UTF-16 character are refused.
    pub(crate) fn write_char(
        &mut self,
        base: &ir::Place,
        idx: &[ir::Expr],
        v: Value,
        f: &mut Frame,
    ) -> Result<(), Signal> {
        // A Code takes the character upper-cased (types/TestCharIndexAssignContracts.al:CharacterAssignedIntoAPosition:
        // `Letter[1] := 'a'` gives `A`, `Ten[3] := 'z'` gives `ABZDE`).
        let (max, is_code) = match self.place_type(base, f) {
            RtType::Text(m) => (m, false),
            RtType::Code(m) => (m, true),
            _ => (None, false),
        };
        let [e] = idx else {
            return Err(Signal::Unsupported("a text indexed with several positions".into()));
        };
        let i = int(&self.eval(e, f)?)?;
        let unit = match int(&v) {
            Ok(c) => u16::try_from(c).ok().filter(|u| !(0xD800..0xE000).contains(u)),
            Err(_) => None,
        };
        let (Value::Text(s) | Value::Code(s)) = self.read(base, f)? else {
            return Err(Signal::Unsupported(
                "assigning a character into a value that is no text".into(),
            ));
        };
        let mut units: Vec<u16> = s.encode_utf16().collect();
        let Some(unit) = unit else {
            return Err(Signal::Unsupported(format!(
                "assigning {v} to a character of a text (only a UTF-16 character is pinned)",
                v = v.describe()
            )));
        };
        // A position outside 1 to the length plus one raises (types/TestProbeScalarContracts.al:TextChar_AssignByIndex:
        // 5, 0 and -1 on 'abc'); the position after the last one appends, and past the declared length it raises the
        // length error of an assignment.
        if i < 1 || (i as usize) > units.len() + 1 {
            return al_builtins::native::raise(
                format!(
                    "The value of INDEXING parameter 1 is outside of the permitted range. The current value is: {i}. The permitted range is: from 1 to {}.",
                    units.len()
                ),
                "OutsidePermittedRange",
            );
        }
        let i = i as usize - 1;
        if i == units.len() {
            units.push(unit);
        } else {
            units[i] = unit;
        }
        if let Some(m) = max.filter(|m| units.len() > *m as usize) {
            return al_builtins::native::raise(
                format!(
                    "The length of the string is {}, but it must be less than or equal to {m} characters. Value: {}",
                    units.len(),
                    String::from_utf16_lossy(&units)
                ),
                "StringLengthExceeded",
            );
        }
        let text = String::from_utf16(&units)
            .map_err(|_| Signal::Unsupported("a text that ends up with a lone surrogate".into()))?;
        if is_code {
            return self.write(base, Value::Code(ops::upper_code(&text).into()), f);
        }
        self.write(base, Value::Text(text.into()), f)
    }

    /// The array behind `base` and the flat position of `idx` (1-based, row-major).
    pub(crate) fn element(
        &mut self,
        base: &ir::Place,
        idx: &[ir::Expr],
        f: &mut Frame,
    ) -> Result<(Cells, usize), Signal> {
        // The dimensions of an array variable (a local or a global, the usual case) are read where they are
        // declared; `place_type` would clone the type, and its dimension list, on every element access.
        let mut small = [0u32; 4];
        let mut n = usize::MAX;
        {
            let declared = match base {
                ir::Place::Local(i) => Some(&self.engine.program.objects[f.object].routines[f.routine].locals[*i].ty),
                ir::Place::Global(i) => {
                    let o = f.instance.borrow().object as usize;
                    Some(&self.engine.program.objects[o].globals[*i].ty)
                }
                _ => None,
            };
            if let Some(RtType::Array(d, _)) = declared {
                if d.len() <= small.len() {
                    small[..d.len()].copy_from_slice(d);
                    n = d.len();
                }
            }
        }
        let big: Vec<u32>;
        let dims: &[u32] = if n != usize::MAX {
            &small[..n]
        } else {
            let RtType::Array(d, _) = self.place_type(base, f) else {
                return Err(Signal::Unsupported("indexing a non-array".into()));
            };
            big = d;
            &big
        };
        // `A[x]` of a two-dimensional array is a row; what BC does with one (Clear, assignment, an argument) is
        // not pinned (docs/probes-needed.md). It used to be read as the cell at flat position x.
        if idx.len() != dims.len() {
            return Err(Signal::Unsupported(format!(
                "a row of a {}-dimensional array (indexed by {} of its dimensions): what BC does with it is not pinned (docs/probes-needed.md)",
                dims.len(),
                idx.len()
            )));
        }
        let arr = match self.read(base, f)? {
            Value::Array(a) => a,
            other => return Err(Signal::Unsupported(format!("indexing {}", other.describe()))),
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
        // A parameter declared with more elements than the argument has (Tests-ERM codeunit 134831 passes
        // an `array[2]` to `var DimensionValue: array[9]`): the stored cells are the argument's. What BC does
        // is not pinned by the corpus, so it is refused rather than indexed out of range.
        if flat >= arr.borrow().len() {
            return Err(Signal::Unsupported(format!(
                "array element {} of an argument that has {} elements, passed to a parameter declared with {}; not pinned (docs/probes-needed.md)",
                flat + 1,
                arr.borrow().len(),
                dims.iter().map(|d| d.to_string()).collect::<Vec<_>>().join(", ")
            )));
        }
        Ok((arr, flat))
    }
}

impl Session {
    /// The value of a constant node. The ones that cost something to build are built once per session.
    #[inline(always)]
    pub(crate) fn constant_once(&mut self, c: &ir::Const) -> Value {
        match c {
            ir::Const::Integer(i) => Value::Integer(*i),
            ir::Const::Boolean(b) => Value::Boolean(*b),
            ir::Const::Decimal(_) | ir::Const::Text(_) | ir::Const::Option(..) => self.constant_cached(c),
            other => constant(other),
        }
    }

    #[inline(never)]
    pub(crate) fn constant_cached(&mut self, c: &ir::Const) -> Value {
        let key = c as *const ir::Const as usize;
        if let Some(v) = self.constants.get(&key) {
            return v.clone();
        }
        let v = constant(c);
        self.constants.insert(key, v.clone());
        v
    }
}

pub(crate) fn constant(c: &ir::Const) -> Value {
    match c {
        ir::Const::Boolean(b) => Value::Boolean(*b),
        ir::Const::Integer(i) => Value::Integer(*i),
        ir::Const::BigInteger(i) => Value::BigInteger(*i),
        ir::Const::Decimal(s) => Value::Decimal(s.parse().expect("decimal literal from the lowerer")),
        ir::Const::Text(s) => Value::Text(s.clone().into()),
        ir::Const::Date(d) => Value::Date(*d, false),
        ir::Const::Time(t) => Value::Time(*t),
        ir::Const::DateTime(t) => Value::DateTime(*t),
        ir::Const::Option(i, m) => Value::Option(*i, m.clone().into()),
        ir::Const::Enum(o, v) => Value::Enum(*o, *v),
        ir::Const::DateFormula(f) => Value::DateFormula(f.clone()),
        ir::Const::FieldPos(p) => Value::Integer(*p as i32),
    }
}
