use super::*;
use al_builtins::page_op::{PageMethod, PageOp};
use al_builtins::record_op::RecordOp;

impl Session {
    /// `TestField.SetValue`: activates the control, turns the value into what the control would
    /// have been typed with, validates the field (the table's OnValidate, then the page control's),
    /// and records a refusal in the control's ledger (corpus handlers/TestPageValidationError_Tests.al,
    /// TestPageReadOnlySetValue.al:1-30, TestPageBooleanSpelling_Tests.al).
    pub(super) fn control_set_value(&mut self, st: &St, idx: usize, args: &mut [Value]) -> Result<Value, Signal> {
        // A subscriber to OnBeforeValidateEvent / OnAfterValidateEvent of this control would run in BC; the client does
        // not raise them yet, so a write to such a control is refused (a page that is merely opened is not).
        {
            let engine = self.engine();
            let object = st.borrow().object as usize;
            let name = page_of(&engine, st).controls[idx].name.clone();
            // The element is the control's name or the name of the field it shows.
            let field_name = self.control_field(st, idx).map(|f| f.name.clone());
            let names: Vec<&str> = std::iter::once(name.as_str()).chain(field_name.as_deref()).collect();
            for ev in ["onbeforevalidateevent", "onaftervalidateevent"] {
                let compiled = engine
                    .events
                    .subscribers
                    .get(&(object, ev.to_string()))
                    .is_some_and(|v| {
                        v.iter()
                            .any(|s| s.element.is_empty() || names.iter().any(|n| s.element.eq_ignore_ascii_case(n)))
                    });
                if compiled
                    || names
                        .iter()
                        .any(|n| engine.events.missing_for(object, ev, Some(n)).is_some())
                {
                    return unsupported(format!(
                        "page event {ev} of control {name}: a subscriber listens, and the test client does not raise it yet"
                    ));
                }
            }
        }
        // SetValue through a part whose control says Editable = false writes anyway
        // (testpart/TestPartCloseLinkEditableContracts.al:
        // Parts_EnabledReadsTheControl_EditableFalseDoesNotBlockWrites). New() and Next() onto the
        // new-row line of such a part are not pinned and stay refused.
        // A validation through the page starts on fresh table globals, also when the line is saved
        // first because the focus leaves the key control (corpus record/PWTableGlobals.al:392-405,
        // PageInsert_NewThenSetTxtThenClose_ValidationBeforeInsert: OnInsert reads Counter 0 though
        // OnNewRecord set 9).
        if matches!(page_of(&self.engine(), st).controls[idx].binding, ir::Binding::Field(_)) && self.has_source(st) {
            let rec = self.page_rec(st);
            self.reset_table_globals(&rec);
        }
        self.activate(st, idx)?;
        // Only an Update() requested by this call's triggers refreshes at its end.
        st.borrow_mut().refresh = false;
        let engine = self.engine();
        let page = page_of(&engine, st);
        let ctl = &page.controls[idx];
        let current = self.control_value(st, idx)?;
        let Some(ty) = self.control_type(st, idx, &current) else {
            // A control bound to an expression on a page with no source table is pinned for the write itself
            // (misc/lt-probes-needed-7/PnGTempControl.al, pages/W1Controls.al:TempRecordControl_SetValue) but the page also
            // refreshes after an action there (AGCR), which is not modelled: refused.
            if matches!(ctl.binding, ir::Binding::Expr(_)) && ctl.trigger("__sourceset").is_some() {
                return unsupported(
                    "SetValue on a control bound to an expression of a page with no source table (the refresh after an action is not modelled)",
                );
            }
            return unsupported("SetValue on a control whose type is not known");
        };
        let rec_bound = matches!(ctl.binding, ir::Binding::Field(_));
        // OnBeforeValidate and OnAfterValidate of `modify` blocks wrap the control's own OnValidate
        // (corpus pageextensiontrigger/TestModifiedControlValidateTriggers_Tests.al:
        // ModifiedControlBeforeAndAfterValidateWrapTheBaseControlsOwnOnValidate). Only that
        // measured shape runs: a control bound to a record field whose table has no OnValidate of its own.
        let ext_validate = |name: &str| -> Vec<(usize, usize)> {
            ctl.ext_triggers
                .iter()
                .filter(|(n, _, _)| n == name)
                .map(|(_, e, r)| (*e, *r))
                .collect()
        };
        let (ext_before, ext_after) = (ext_validate("onbeforevalidate"), ext_validate("onaftervalidate"));
        if !ext_before.is_empty() || !ext_after.is_empty() {
            if !rec_bound {
                return unsupported("OnBeforeValidate/OnAfterValidate on a control bound to a variable (not pinned)");
            }
            if self.control_field(st, idx).is_some_and(|f| f.on_validate.is_some()) {
                return unsupported(
                    "OnBeforeValidate/OnAfterValidate on a control whose field has a table OnValidate (their order is not pinned)",
                );
            }
        }
        let caption = self.control_caption(st, idx)?;
        let fmt = al_builtins::pagefmt::ControlFormat {
            work_date: self.work_date_day(),
            today: Some(self.today_day_number()),
            ..self.control_format(st, idx)?
        };
        let arg = args.first().cloned().unwrap_or(Value::Null);
        let text = match &arg {
            Value::Text(t) | Value::Code(t) => t.clone(),
            other => {
                let converted = ops::convert(other.clone(), &ty)?;
                self.control_text(st, idx, &converted)?.into()
            }
        };
        let enum_engine = engine.clone();
        let by_text = move |id: u32, t: &str| {
            let o = &enum_engine.program.objects[id as usize];
            // An enum control resolves by caption only; a value without a caption shows its name.
            // The bare member name of a captioned value is rejected, as on a real service tier
            // (handlers/TestPageEnumVariableControl_Tests.al:50-56,86-90,137-140).
            o.enum_values
                .iter()
                .find(|(v, name)| {
                    let shown = o.enum_captions.iter().find(|(c, _)| c == v).map_or(name, |(_, c)| c);
                    shown.eq_ignore_ascii_case(t)
                })
                .map(|(v, _)| *v)
        };
        // Without a row on the page there is nothing to refresh (controls/W2ValueControls.al:Enum_RecordFieldControl_*).
        let no_row = self.shows_no_row(st, idx);
        let suffix = if rec_bound && !no_row {
            " (Select Refresh to discard errors)"
        } else {
            ""
        };
        let refuse_code = |msg: String, code: &str| -> Signal {
            let stored = format!("{msg}{suffix}");
            st.borrow_mut().errors.push((idx, stored.clone()));
            error_code(
                format!(
                    "Validation error for Field: {},  Message = '{stored}'",
                    page_of(&engine, st).controls[idx].name
                ),
                code,
            )
        };
        let refuse = |_: &mut Session, msg: String| -> Signal { refuse_code(msg, "") };
        // The bounds and the length of a Text variable raise a TestValidation error
        // (handlers/TestPageVariableMinMaxContracts.al:15-16, :119).
        let refuse_bound = |msg: String| -> Signal { refuse_code(msg, "TestValidation") };
        let parsed = match al_builtins::pagefmt::parse_control_text(&text, &ty, &fmt, &by_text) {
            Ok(v) => v,
            Err(al_builtins::pagefmt::ParseError::Invalid) => {
                let mut msg = format!("Your entry of '{text}' is not an acceptable value for '{caption}'.");
                // A Date adds how the client reads one (controls/W2ValueControls.al:Date_IsoSpellings and
                // session/W5WorkDateShorthand.al, BC 27.5 and 29.0); the text holds a CR, a backslash, a CR and
                // a backslash where one would expect line breaks.
                // An Integer variable adds why (controls/W2FindFieldAssign.al:ValueAssign_IntegerControl_TextAndNonNumber).
                if !rec_bound && matches!(ty, RtType::Integer) {
                    msg.push_str(&format!(" The value \"{text}\" can't be evaluated into type Integer."));
                }
                if matches!(ty, RtType::Date) {
                    msg.push_str(&format!(
                        " The format of the Date '{text}' does not match your device's date settings.\r\\\r\\Make sure that the day, month, and year are in the correct order and use the correct separator (such as - or /). Alternatively, you can always use the format 'yyyy-mm-dd'."
                    ));
                }
                // A control bound to a variable raises TestValidation (controls/W2ValueControls.al:Date_IsoSpellings,
                // Enum_VariableControl_*); one bound to a table field keeps the empty code.
                return Err(if rec_bound && !no_row {
                    refuse(self, msg)
                } else {
                    refuse_bound(msg)
                });
            }
            Err(al_builtins::pagefmt::ParseError::Unsupported) => {
                return unsupported(format!(
                    "SetValue of a {ty} control: its spelling is not modelled yet",
                    ty = ty.describe()
                ));
            }
        };
        // The entry is read before the row is looked for: a refused entry is a validation error, an acceptable one on
        // a page without a row is TestRowNotFound (controls/W2ValueControls.al:Enum_RecordFieldControl_CaptionAndNames).
        if self.shows_no_row(st, idx) {
            return Err(error_code("The row does not exist on the TestPage.", "TestRowNotFound"));
        }
        // A Text variable given too many characters (handlers/TestPageVariableMinMaxContracts.al:119).
        // A table field's overlong text is not pinned.
        if let (RtType::Text(Some(max)), Value::Text(t), false) = (&ty, &parsed, rec_bound) {
            let len = Value::text_len(t);
            if len > *max as usize {
                return Err(refuse_bound(format!(
                    "The length of the string is {len}, but it must be less than or equal to {max} characters. Value: {t}"
                )));
            }
        }
        let parsed = ops::convert(parsed, &ty)?;
        // MinValue / MaxValue are enforced on a control write, not on Rec.Validate (corpus
        // handlers/TestPageFieldMinMaxValue_Tests.al:20-110). A control bound to a variable has
        // its own bounds, with the same texts; a table field's adds the "Select Refresh" suffix
        // (handlers/TestPageVariableMinMaxContracts.al:60-130). The value shows as typed for an
        // integer and with two decimals for a decimal (2.60, 0.40; more decimals are not pinned).
        {
            let bound = |n: &str| match self.control_field(st, idx) {
                Some(f) => f
                    .properties
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(n))
                    .map(|(_, v)| v.trim().to_string()),
                None => match page_of(&engine, st).controls[idx].prop(n) {
                    Some(ir::PropVal::Lit(v)) => Some(v.trim().to_string()),
                    _ => None,
                },
            };
            let num = match &parsed {
                Value::Integer(i) => Some((rust_decimal::Decimal::from(*i), i.to_string())),
                Value::BigInteger(i) => Some((rust_decimal::Decimal::from(*i), i.to_string())),
                Value::Decimal(d) if d.normalize().scale() <= 2 => Some((*d, format!("{:.2}", d))),
                _ => None,
            };
            let beyond_pinned = matches!(&parsed, Value::Decimal(d) if d.normalize().scale() > 2);
            let min = bound("minvalue");
            let max = bound("maxvalue");
            let breaks = |b: &Option<String>, below: bool| match (b, &parsed) {
                (Some(b), Value::Decimal(_) | Value::Integer(_) | Value::BigInteger(_)) => {
                    b.parse::<rust_decimal::Decimal>().ok().is_some_and(|m| {
                        let v = match &parsed {
                            Value::Integer(i) => rust_decimal::Decimal::from(*i),
                            Value::BigInteger(i) => rust_decimal::Decimal::from(*i),
                            Value::Decimal(d) => *d,
                            _ => return false,
                        };
                        if below { v < m } else { v > m }
                    })
                }
                _ => false,
            };
            let (under, over) = (breaks(&min, true), breaks(&max, false));
            if (under || over) && beyond_pinned {
                return unsupported(
                    "SetValue outside MinValue/MaxValue with more than two decimals (how BC shows the value is not pinned)",
                );
            }
            if let Some((_, shown)) = num {
                if under {
                    let min = min.unwrap_or_default();
                    return Err(refuse_bound(format!(
                        "The value must be greater than or equal to {min}. Value: {shown}."
                    )));
                }
                if over {
                    let max = max.unwrap_or_default();
                    return Err(refuse_bound(format!(
                        "The value must be less than or equal to {max}. Value: {shown}."
                    )));
                }
            }
        }
        // A value equal to the one held still validates: OnValidate runs again (corpus
        // handlers/TestPageRuntimeContracts.al:179-194 TestPage_SetValue_OfTheValueAlreadyHeld_StillRunsOnValidate).
        let rec = if rec_bound { Some(self.page_rec(st)) } else { None };
        let before = rec.as_ref().map(|r| r.borrow().fields.clone());
        let was_dirty = st.borrow().dirty;
        // CurrPage.SaveRecord() inside the control's OnValidate must see the edit (corpus
        // handlers/TestPageSaveRecordInOnValidate_Tests.al:SaveRecordInOnValidatePersistsBeforeThePageCloses).
        st.borrow_mut().dirty = true;
        let result = (|| -> Result<(), Signal> {
            match ctl.binding.clone() {
                ir::Binding::Field(pos) => {
                    self.ret_used = false;
                    let rec = rec.clone().expect("rec-bound");
                    // The field's OnValidate sees its own number as CurrFieldNo
                    // (handlers/TestPageCurrFieldNo_Tests.al:36-60); the page's save later sees 0 (:105-123).
                    let number = {
                        let table = rec.borrow().table as usize;
                        self.engine.tables[table]
                            .as_ref()
                            .map_or(0, |t| t.fields[pos].id as i32)
                    };
                    self.pages.curr_field_no.push((Rc::downgrade(&rec), number));
                    // A validation through the page runs the table's OnValidate on fresh table
                    // globals, and leaves the page's record with fresh globals (ADR-021 decision 8;
                    // corpus probes TableGlobals_PageFieldValidate_*, TableGlobals_PageRec_ActionGlobalsAreLostAfterPageFieldValidation).
                    self.reset_table_globals(&rec);
                    let validated = self.record_op(
                        RecordOp::Validate,
                        &rec,
                        &mut [Value::Integer(pos as i32), parsed.clone()],
                    );
                    self.reset_table_globals(&rec);
                    self.pages.curr_field_no.pop();
                    validated?;
                }
                ir::Binding::Var(_) | ir::Binding::BaseVar(_) => {
                    let (slot, owner) = ctl.binding.var_slot(ctl.owner).expect("a variable binding");
                    let inst = self.owner_instance(st, owner);
                    inst.borrow_mut().globals[slot] = parsed.clone();
                }
                // A control bound to an expression that is a place (`Totals[1]."Inv. Discount Amount"`) is written through the
                // routine `pages::expand` made. Unprobed: assumption page-expression-control-setvalue.
                ir::Binding::Expr(_) if ctl.trigger("__sourceset").is_some() => {
                    let w = ctl.trigger("__sourceset").expect("checked");
                    al_builtins::assumptions::assume(
                        al_builtins::assumptions::Assumption::PageExpressionControlSetValue,
                    );
                    let owner = ctl.owner;
                    self.owner_routine_with(st, owner, w, vec![parsed.clone()])?;
                }
                _ => return unsupported("SetValue on a read-only control"),
            }
            for (e, r) in &ext_before {
                self.owner_routine(st, Some(*e), *r)?;
            }
            if let Some(r) = ctl.trigger("onvalidate") {
                self.control_routine(st, idx, r)?;
            }
            for (e, r) in &ext_after {
                self.owner_routine(st, Some(*e), *r)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                self.run_refresh(st)?;
                Ok(Value::Null)
            }
            Err(Signal::Error(e)) => {
                st.borrow_mut().refresh = false;
                st.borrow_mut().dirty = was_dirty;
                // The refused edit does not stay (corpus handlers/TestPageBooleanSpelling_Tests.al,
                // SetValue_TrueFalseSpelling_IsRefused: the stored value is untouched).
                if let (Some(rec), Some(before)) = (&rec, before) {
                    rec.borrow_mut().fields = before;
                } else if let Some((slot, owner)) = ctl.binding.var_slot(ctl.owner) {
                    let inst = self.owner_instance(st, owner);
                    inst.borrow_mut().globals[slot] = current.clone();
                } else if let (ir::Binding::Expr(_), Some(w)) = (&ctl.binding, ctl.trigger("__sourceset")) {
                    let _ = self.owner_routine_with(st, ctl.owner, w, vec![current.clone()]);
                }
                // An error of the write through an expression source (a text too long for the field) is a TestValidation
                // error, as for a variable (misc/lt-probes-needed-7/PnGTempControl.al:SetValueTooLongAndInvalid).
                if matches!(ctl.binding, ir::Binding::Expr(_)) {
                    return Err(refuse_bound(e.message));
                }
                Err(refuse(self, e.message))
            }
            Err(other) => Err(other),
        }
    }

    // ---- controls -----------------------------------------------------------------------

    pub(super) fn test_control_op(
        &mut self,
        method: PageOp,
        c: &ControlRef,
        args: &mut [Value],
    ) -> Result<Value, Signal> {
        self.require_open(&c.page)?;
        let st = &c.page;
        let m = method.method();
        let idx = match &c.target {
            ControlTarget::Control(i) => *i,
            ControlTarget::BuiltIn(name) if matches!(name.as_str(), "Edit" | "View") => {
                // On a card the built-in Edit and View switch the open page's mode in place; both stay
                // visible, and only the one the current mode does not already have is enabled (corpus
                // handlers/TestPageModeSwitchAction_Tests.al:Claim 1 and 2). A list opens its card
                // instead, like a RunObject action with RunPageOnRec, answered by a page handler (corpus
                // handlers/TestPageListViewEditAction_Tests.al:ListViewActionOpensTheCardOnTheListsCurrentRowReadOnly,
                // ListEditActionOpensTheCardOnTheListsCurrentRowEditable). Whether the action is visible or
                // enabled on a list is not pinned.
                if self.is_list(st) {
                    // Both actions are visible on a list whether or not it names a card; View is
                    // enabled, Edit only when the list names a card that can be edited; a disabled
                    // action does nothing, and so does View on a list with no card (corpus
                    // handlers/TestPageModeSwitchAction_Tests.al:
                    // PlainListWithoutCardPageIdOffersBothActionsAndEnablesOnlyView,
                    // ListWhoseCardIsReadOnlyLeavesTheEditActionVisibleButNotEnabled,
                    // ListWithAnEditableCardPageIdEnablesBothActions).
                    let edit = name == "Edit";
                    let card = self.list_card_page(st)?;
                    // A list without a card switches the mode of its own page, as a card does: opened for view,
                    // Edit is enabled and View is not; opened for edit, the other way round. A list that names a
                    // card with no row to open it for has both disabled (misc/W2ListActions.al:NoCard_WithRows,
                    // List_NoRows, BC 27.5 and 29.0).
                    let no_rows = st.borrow().cursor == Cursor::None;
                    let editable = st.borrow().mode != PageMode::View;
                    let enabled = match (edit, card) {
                        (_, None) => edit == !editable,
                        (_, Some(_)) if no_rows => false,
                        (false, _) => true,
                        (true, Some(target)) => self.card_allows_edit(target)?,
                    };
                    return match m {
                        PageMethod::Visible => Ok(Value::Boolean(true)),
                        PageMethod::Enabled => Ok(Value::Boolean(enabled)),
                        PageMethod::Invoke if !enabled || card.is_none() => Ok(Value::Null),
                        PageMethod::Invoke => self.list_card_action(st, edit),
                        other => unsupported(format!(
                            "TestAction.{other} of the built-in {name} action of a list page"
                        )),
                    };
                }
                let editable = st.borrow().mode != PageMode::View;
                let applies = (name == "Edit") == !editable;
                return match m {
                    PageMethod::Visible => Ok(Value::Boolean(true)),
                    PageMethod::Enabled => Ok(Value::Boolean(applies)),
                    PageMethod::Invoke => {
                        if applies {
                            let mode = if name == "Edit" { PageMode::Edit } else { PageMode::View };
                            st.borrow_mut().mode = mode;
                            // The parts follow the host (corpus handlers/TestPageControlEditableRoutes.al:
                            // Part_HostOpenEdit_ThenViewAction).
                            self.set_part_modes(st, mode);
                        }
                        Ok(Value::Null)
                    }
                    other => unsupported(format!("TestAction.{other}")),
                };
            }
            ControlTarget::Filter => {
                return self.test_filter_op(m, st, args);
            }
            ControlTarget::BuiltIn(name) if m == PageMethod::Invoke => {
                // OK and Yes confirm, Cancel and No cancel; a refused save keeps the page open
                // (corpus testpageclose/TestPageClosingActions.al).
                return self.close_with(st, name, true, true);
            }
            // The built-in OK and Cancel are visible (report/TestProbeReportContracts.al:RequestPage_InvokeActionNotFound).
            ControlTarget::BuiltIn(_) if m == PageMethod::Visible => return Ok(Value::Boolean(true)),
            _ => return unsupported(format!("{method} on a built-in control")),
        };
        // The data items and actions of a report's request page.
        if self.is_request_page(st) {
            let kind = page_of(&self.engine(), st).controls[idx].kind.clone();
            match kind {
                ir::ControlKind::DataItem => return self.request_item_op(method, st, idx, args),
                ir::ControlKind::Action if m == PageMethod::Invoke => return self.request_action_invoke(st, idx),
                _ => {}
            }
        }
        // The compiler emits no call for a `Value()` whose result is thrown away: nothing is
        // evaluated, on a control that is on the page or not (corpus
        // pageapplicationarea/TestPageDiscardedFieldValueCall.al: RemovedControl_DiscardedValueCall_RaisesNothing,
        // FoundControl_DiscardedValueCall_RaisesNothing).
        if m == PageMethod::Value && args.is_empty() && !self.ret_used {
            return Ok(Value::Null);
        }
        // A control whose Visible is the literal false, or that sits in a group whose Visible is,
        // is not on the runtime page at all (corpus handlers/TestPageFieldVisibleGroup_Tests.al:1-30).
        // Neither is one that application areas removed.
        self.check_areas_pinned(st, idx)?;
        if self.control_eliminated(st, idx) || self.area_removed(st, idx) || self.action_without_behaviour(st, idx) {
            let engine = self.engine();
            let ctl = &page_of(&engine, st).controls[idx];
            let what = match ctl.kind {
                ir::ControlKind::Action | ir::ControlKind::SystemAction => "action",
                _ => "field",
            };
            // The id in the text is generated from the declaring object and the control name
            // (see `control_id`); it matches the ids the corpus pins for Base Application controls
            // (pageapplicationarea/TestPageApplicationAreaPrecompiledControlRemoval.al:57, :92, :124)
            // and for a part (testpart/TestTestPart.al:233). The code of a field is pinned by
            // handlers/TestPageRuntimeContracts.al:260; that of an action is TestActionNotFound
            // (handlers/TestPageLabelControlContracts.al:141-145).
            let ctl_name = ctl.name.clone();
            let owner = ctl.owner;
            let object = st.borrow().object as usize;
            let declaring = self.object_id_of(owner.unwrap_or(object) as u32);
            let id = control_id(declaring, &ctl_name);
            return Err(match what {
                "action" => error_code(
                    format!("The action with ID = {id} is not found on the page."),
                    "TestActionNotFound",
                ),
                _ => error_code(
                    format!("The field with ID = {id} is not found on the page."),
                    "TestFieldNotFound",
                ),
            });
        }
        // Only Invoke of an actionref is pinned: it runs the action it stands for. A found actionref
        // reads Visible true when neither it nor its target states a Visible (corpus
        // pageapplicationarea/TestPageApplicationAreaExtensionActionPartRemoval.al:
        // ExtActionRef_TargetEnabled_IsFound, ExtActionRef_TargetNotEnabled_IsNotFound).
        if m != PageMethod::Invoke {
            let engine = self.engine();
            let page = page_of(&engine, st);
            let ctl = &page.controls[idx];
            if let Some(action_ref) = &ctl.action_ref {
                let target_plain = match action_ref {
                    ir::ActionRef::Target(t) => page.controls[*t].prop("visible").is_none(),
                    ir::ActionRef::Unresolved(_) => false,
                };
                if m == PageMethod::Visible && target_plain && ctl.prop("visible").is_none() {
                    return Ok(Value::Boolean(true));
                }
                // handlers/TestPageRunObjectContracts.al:392-404: Visible and Enabled of an actionref answer Yes.
                let target_enabled_plain = match action_ref {
                    ir::ActionRef::Target(t) => page.controls[*t].prop("enabled").is_none(),
                    ir::ActionRef::Unresolved(_) => false,
                };
                if m == PageMethod::Enabled && target_enabled_plain && ctl.prop("enabled").is_none() {
                    return Ok(Value::Boolean(true));
                }
                return unsupported(format!("TestAction.{m} of an actionref"));
            }
        }
        match m {
            // `Ctl.Value := x` is the setter form of the property, and does what SetValue does
            // (corpus handlers/ProbeTestPageForms.al:109-131: OnValidate runs with the assigned text).
            PageMethod::Value if !args.is_empty() && self.in_cue_group(st, idx) => Ok(Value::Null),
            PageMethod::Value if !args.is_empty() => {
                let r = self.control_set_value(st, idx, args)?;
                self.resync_parts_linked_by(st, idx)?;
                Ok(r)
            }
            PageMethod::Value => {
                if self.shows_no_row(st, idx) {
                    return Ok(Value::Text(String::new().into()));
                }
                let v = self.control_value(st, idx)?;
                // An Enum control that refused an entry shows the ordinal, not the caption (controls/W2ValueControls.al:
                // Enum_VariableControl_*, `value` 0 after a refused `Alpha`, BC 27.5 and 29.0).
                if let Value::Enum(_, ordinal) = &v {
                    if st.borrow().errors.iter().any(|(i, _)| *i == idx) {
                        return Ok(Value::Text(ordinal.to_string().into()));
                    }
                }
                let text = self.control_text(st, idx, &v)?;
                Ok(Value::Text(text.into()))
            }
            PageMethod::AsInteger => self.control_as(st, idx, &RtType::Integer),
            PageMethod::AsDecimal => self.control_as(st, idx, &RtType::Decimal),
            PageMethod::AsBoolean => self.control_as(st, idx, &RtType::Boolean),
            PageMethod::AsDate => self.control_as(st, idx, &RtType::Date),
            PageMethod::AsTime => self.control_as(st, idx, &RtType::Time),
            PageMethod::AsDateTime => self.control_as(st, idx, &RtType::DateTime),
            PageMethod::AssertEquals => self.control_assert_equals(st, idx, args),
            PageMethod::Caption => Ok(Value::Text(self.control_caption(st, idx)?.into())),
            PageMethod::OptionCount | PageMethod::GetOption => {
                let current = self.control_value(st, idx)?;
                let engine = self.engine();
                let members: Vec<i32> = match self.control_type(st, idx, &current) {
                    Some(RtType::Option(names)) => (0..names.len() as i32).collect(),
                    Some(RtType::Enum(id)) => engine.program.objects[id as usize]
                        .enum_values
                        .iter()
                        .map(|(v, _)| *v)
                        .collect(),
                    _ => return unsupported(format!("TestField.{m} on a control that is not an option")),
                };
                if m == PageMethod::OptionCount {
                    return Ok(Value::Integer(members.len() as i32));
                }
                let i = match args.first() {
                    Some(Value::Integer(i)) => *i,
                    _ => return unsupported("GetOption without an index"),
                };
                let Some(ordinal) = i
                    .checked_sub(1)
                    .and_then(|n| usize::try_from(n).ok())
                    .and_then(|n| members.get(n))
                else {
                    return unsupported(format!("GetOption({i}) outside the options (error text not pinned)"));
                };
                let v = match self.control_type(st, idx, &current) {
                    Some(RtType::Option(names)) => Value::Option(*ordinal, names.into()),
                    Some(RtType::Enum(id)) => Value::Enum(id, *ordinal),
                    _ => unreachable!(),
                };
                Ok(Value::Text(self.control_text(st, idx, &v)?.into()))
            }
            // The properties as written; a computed one is not read (docs only).
            PageMethod::HideValue | PageMethod::ShowMandatory => {
                let name = if m == PageMethod::HideValue {
                    "hidevalue"
                } else {
                    "showmandatory"
                };
                let engine = self.engine();
                match page_of(&engine, st).controls[idx].prop(name) {
                    // ShowMandatory is true when the page says nothing (handlers/TestProbePageHandlerContracts.al:
                    // TestPage_Members_ShowMandatoryHideValue_Defaults); HideValue is false.
                    None => Ok(Value::Boolean(m == PageMethod::ShowMandatory)),
                    Some(ir::PropVal::Lit(l)) => Ok(Value::Boolean(unquote(l).eq_ignore_ascii_case("true"))),
                    Some(_) => unsupported(format!("TestField.{m} of a computed property")),
                }
            }
            PageMethod::Editable => Ok(Value::Boolean(self.control_editable(st, idx)?)),
            PageMethod::Enabled => Ok(Value::Boolean(self.control_enabled(st, idx)?)),
            PageMethod::Visible => Ok(Value::Boolean(self.control_visible(st, idx)?)),
            PageMethod::Invoke => self.action_invoke(st, idx),
            PageMethod::DrillDown | PageMethod::AssistEdit | PageMethod::Lookup => self.control_navigate(st, idx, m),
            // SetValue on a field of a cuegroup does nothing and raises nothing
            // (handlers/TestPageCueGroupFixedGridContracts.al:CueGroup_SetValue_IsSilentlyIgnored).
            PageMethod::SetValue if self.in_cue_group(st, idx) => Ok(Value::Null),
            PageMethod::SetValue => {
                let r = self.control_set_value(st, idx, args)?;
                self.resync_parts_linked_by(st, idx)?;
                Ok(r)
            }
            PageMethod::Activate => {
                self.activate(st, idx)?;
                Ok(Value::Null)
            }
            PageMethod::ValidationErrorCount => {
                let n = st.borrow().errors.iter().filter(|(i, _)| *i == idx).count();
                Ok(Value::Integer(n as i32))
            }
            PageMethod::GetValidationError => {
                let i = match args.first() {
                    Some(Value::Integer(i)) => *i,
                    None => 1,
                    _ => return unsupported("GetValidationError with a non-integer index"),
                };
                let s = st.borrow();
                let mine: Vec<&String> = s.errors.iter().filter(|(c, _)| *c == idx).map(|(_, t)| t).collect();
                // An index past the control's ledger escapes AL as an untrappable CLR exception
                // (corpus handlers/TestPageValidationError_Tests.al:1-40), so it is not modelled.
                match usize::try_from(i - 1).ok().and_then(|n| mine.get(n)) {
                    Some(t) => Ok(Value::Text((*t).clone().into())),
                    None => unsupported("GetValidationError past the ledger"),
                }
            }
            other => unsupported(format!("TestField.{other}")),
        }
    }
}
