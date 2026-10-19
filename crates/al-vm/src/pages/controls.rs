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
                // A subscriber of a declared and not compiled app, or one that names the field when the control
                // has another name, or one for every element, would run in BC and is not reached here.
                // Only a control bound to a record field is served: its events carry the page's Rec.
                let record_bound = matches!(page_of(&engine, st).controls[idx].binding, ir::Binding::Field(_));
                let unreached = engine
                    .events
                    .subscribers
                    .get(&(object, ev.to_string()))
                    .is_some_and(|v| {
                        v.iter().any(|s| {
                            s.element.is_empty()
                                || (names.iter().any(|n| s.element.eq_ignore_ascii_case(n))
                                    && (!record_bound || !s.element.eq_ignore_ascii_case(&name)))
                        })
                    });
                if unreached
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
            if matches!(ctl.binding, ir::Binding::Expr(_))
                && ctl.trigger("__sourceset").is_some()
                && !self.is_request_page(st)
            {
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
            // A whole number handed to a text or code control is its digits (WhseItemTrackingPageHandler of SCM RTAM Item Tracking-II sets
            // `Serial No.` to `TrackingQuantity2 + AssignTracking`, a Decimal); unprobed (assumption setvalue-whole-number-into-text-control).
            // Beyond 999 Format adds a thousands separator, which is not pinned for a control, so that stays refused.
            Value::Integer(_) | Value::BigInteger(_) | Value::Decimal(_)
                if matches!(ty, RtType::Text(_) | RtType::Code(_)) =>
            {
                let n = match &arg {
                    Value::Integer(i) => Some(i64::from(*i)),
                    Value::BigInteger(i) => Some(*i),
                    Value::Decimal(d) if d.fract().is_zero() => i64::try_from(d.trunc()).ok(),
                    _ => None,
                };
                match n {
                    Some(n) if n.abs() < 1000 => {
                        al_builtins::assumptions::assume(
                            al_builtins::assumptions::Assumption::SetValueWholeNumberIntoTextControl,
                        );
                        n.to_string().into()
                    }
                    _ => {
                        return unsupported(format!(
                            "SetValue of {} on a text control (how BC writes it is not pinned)",
                            arg.describe()
                        ));
                    }
                }
            }
            // An Enum handed to a text control (Analysis Types' "Item Ledger Entry Type Filter".SetValue(Enum)): the generic conversion
            // raises TypeConversion for a Variant holding an Enum, which is not what a control does; how the client writes it is not pinned.
            Value::Enum(..) if matches!(ty, RtType::Text(_) | RtType::Code(_)) => {
                return unsupported("SetValue of an Enum on a text control (how BC writes it is not pinned)");
            }
            other => {
                let converted = ops::convert(other.clone(), &ty)?;
                self.control_text(st, idx, &converted)?.into()
            }
        };
        if let RtType::Enum(id) = &ty {
            self.note_enum(*id);
        }
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
        // A list shows no Refresh hint (misc/lt-probes-needed-3/PnC07TestPage.al:GetValidationError_AfterFailedValidation, ValueAssign_Number_FromText_And_Decimal);
        // a card does (handlers/TestPageErrTeardown_Tests.al, TestPageBooleanSpelling_Tests.al:210).
        let suffix = if rec_bound && !no_row && !self.is_list(st) {
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
        // A blank value in a control bound to a NotBlank field is refused with a TestValidation error naming the caption
        // (pages/lt-page-presentation-properties/P119PageFields.al:P119_NotBlankPageField; Tests-SCM-Planning
        // ManufacturerCodeCannotBeBlank asserts the code). The text is pinned on a card with a row; on a list or a page
        // without a row it is not.
        if rec_bound
            && text.is_empty()
            && self.control_field(st, idx).is_some_and(|f| {
                f.properties
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("NotBlank") && v.trim().eq_ignore_ascii_case("true"))
            })
        {
            if no_row || self.is_list(st) {
                return unsupported(
                    "SetValue of a blank value on a NotBlank control of a list or of a page without a row (the client's text is not pinned)",
                );
            }
            return Err(refuse_bound(format!("{caption} must be filled in. Enter a value.")));
        }
        // A Numeric page field takes digits only (pages/lt-page-presentation-properties/P119PageNumeric.al:P119_PageNumericSetValue:
        // letters are refused with a TestValidation error). Other non-digits are not pinned.
        if matches!(ty, RtType::Text(_) | RtType::Code(_))
            && matches!(page_of(&engine, st).controls[idx].prop("numeric"), Some(ir::PropVal::Lit(v)) if v.trim().eq_ignore_ascii_case("true"))
            && !text.chars().all(|c| c.is_ascii_digit())
        {
            if !text.chars().any(|c| c.is_ascii_alphabetic()) {
                return unsupported(
                    "SetValue on a Numeric field with a character that is neither a digit nor a letter (not pinned)",
                );
            }
            return Err(refuse_bound(format!(
                "Your entry of '{text}' is not an acceptable value for '{caption}'. Only numeric characters are allowed in field."
            )));
        }
        // CharAllowed on a page field or on the table field it shows: a Text or Code entry may hold only the characters from the first to
        // the second of the property's two characters, case sensitive (pages/lt-page-controls-aa/AaFieldProps.al:AA_CharAllowed_PageField
        // line 67 `az` and `09`, AA_CharAllowed_TableField line 100). The error names the first character outside it. The table field's
        // property restricts the page only: Validate and assignment in code accept anything (same file, `validate-upper`, `assign`).
        // Pinned on a card with a row and a two-character property only.
        if matches!(ty, RtType::Text(_) | RtType::Code(_)) {
            let page_prop = match page_of(&engine, st).controls[idx].prop("charallowed") {
                Some(ir::PropVal::Lit(v)) => Some(unquote(v)),
                _ => None,
            };
            let field_prop = self.control_field(st, idx).and_then(|f| {
                f.properties
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("CharAllowed"))
                    .map(|(_, v)| unquote(v))
            });
            if let Some(allowed) = page_prop.or(field_prop) {
                let range: Vec<char> = allowed.chars().collect();
                if range.len() != 2 || no_row || self.is_list(st) {
                    return unsupported(
                        "SetValue on a field with CharAllowed that is not two characters, on a list or on a page without a row (not pinned)",
                    );
                }
                if let Some(bad) = text.chars().find(|c| !(range[0]..=range[1]).contains(c)) {
                    return Err(refuse_bound(format!(
                        "The character {bad} is not valid in this field. You can specify the following characters: {allowed}."
                    )));
                }
            }
        }
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
                // A Decimal says what was wrong (misc/lt-probes-needed-3/PnC07TestPage.al:ValueAssign_Number_FromText_And_Decimal, a list).
                if rec_bound && self.is_list(st) && matches!(ty, RtType::Decimal) {
                    msg.push_str(&format!(" '{text}' is not a part of a valid decimal expression."));
                }
                if matches!(ty, RtType::Date) {
                    msg.push_str(&format!(
                        " The format of the Date '{text}' does not match your device's date settings.\\\\Make sure that the day, month, and year are in the correct order and use the correct separator (such as - or /). Alternatively, you can always use the format 'yyyy-mm-dd'."
                    ));
                }
                // A DateTime adds that the date is not valid (pages/lt-page-run-overloads-testpart/P106TestFieldAsDateTime.al:
                // P106_AsDateTime_Card, an ISO text on a table field).
                if matches!(ty, RtType::DateTime) {
                    msg.push_str(" The date is not valid.");
                }
                // A control bound to a variable raises TestValidation (controls/W2ValueControls.al:Date_IsoSpellings,
                // Enum_VariableControl_*); one bound to a table field keeps the empty code.
                return Err(
                    // An Enum control raises TestValidation on a card with a row too (pages/lt-page-enum-control/
                    // PageEnumControlCaptionless.al:P161_EnumControl_SetValueSpellings_Captionless `spaceOrdinal`).
                    // A refused text on an Option control is a TestValidation error on a card with a row: the empty text (pages/lt-testpage-assertequals-option/
                    // TestPageAssertEqualsOption.al:SetValue_BlankOption_AsEmptyText) and `Nope`, `5`, `2`, `Tw` (handlers/N582TestPageEdges.al:156-186).
                    if rec_bound
                        && !no_row
                        && !matches!(ty, RtType::DateTime | RtType::Enum(_))
                        && !matches!(ty, RtType::Option(_))
                        && !self.is_list(st)
                    {
                        refuse(self, msg)
                    } else {
                        refuse_bound(msg)
                    },
                );
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
        // A table field's overlong text is a TestValidation error too (P81PageValidateEvents.al:TestPage_SetValue_NameTooLong).
        if let (RtType::Text(Some(max)), Value::Text(t)) = (&ty, &parsed) {
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
            // The page field's own MinValue / MaxValue count on a control bound to a table field too (pages/
            // lt-page-presentation-properties/P119PageNumeric.al:P119_PageMinMax); then the value shows as the control shows it.
            let page_bound = |n: &str| match page_of(&engine, st).controls[idx].prop(n) {
                Some(ir::PropVal::Lit(v)) => Some(v.trim().to_string()),
                _ => None,
            };
            // Pinned for a Decimal field only: an Integer field with the same page properties takes 11 above a MaxValue of 10
            // without an error (handlers/TestPageValidationGetError_Tests.al:GetValidationError_IndexPastLast `setValueOutOfRange`).
            let on_the_page = self.control_field(st, idx).is_some()
                && matches!(parsed, Value::Decimal(_))
                && (page_bound("minvalue").is_some() || page_bound("maxvalue").is_some());
            let bound = |n: &str| match (
                on_the_page.then(|| page_bound(n)).flatten(),
                self.control_field(st, idx),
            ) {
                (Some(v), _) => Some(v),
                (None, Some(f)) => f
                    .properties
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(n))
                    .map(|(_, v)| v.trim().to_string()),
                (None, None) => page_bound(n),
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
                let shown = if on_the_page && (under || over) {
                    self.control_text(st, idx, &parsed)?.to_string()
                } else {
                    shown
                };
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
        // A control bound to a page variable does not edit the row: nothing for SaveRecord to write (Planning Worksheet's batch
        // name control calls SaveRecord from OnValidate on a DelayedInsert page; Tests-SCM-Planning
        // OrderTrackingForCalcRegenPlanSalesUsingForecastLFLItem).
        if rec_bound {
            st.borrow_mut().dirty = true;
        }
        let from_page_trigger = std::cell::Cell::new(false);
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
            // The page's events around the control's own trigger (corpus P81PageValidateEvents.al).
            let element = page_of(&self.engine(), st).controls[idx].name.clone();
            if rec_bound {
                self.page_validate_event(st, "onbeforevalidateevent", &element)?;
            }
            if let Some(r) = ctl.trigger("onvalidate") {
                if let Err(e) = self.control_routine(st, idx, r) {
                    from_page_trigger.set(true);
                    return Err(e);
                }
            }
            for (e, r) in &ext_after {
                self.owner_routine(st, Some(*e), *r)?;
            }
            if rec_bound {
                self.page_validate_event(st, "onaftervalidateevent", &element)?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                // A SetValue that goes through clears the control's refused entries (pages/lt-f3-pages/F3ValidationAddinFilter.al:
                // GetValidationError_ErrorThenCorrected, line 272: the page answers an empty text afterwards).
                st.borrow_mut().errors.retain(|(i, _)| *i != idx);
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
                // An error of a control's own OnValidate on a list is the bare message, with no Refresh hint (misc/lt-probes-needed-3/
                // PnC07TestPage.al:GetValidationError_AfterFailedValidation and ValueAssign_ValidationError); on a card it carries it
                // (handlers/TestPageErrTeardown_Tests.al, codeunit 60836). Whether the list or something else in that page decides is
                // not probed (docs/probes-needed.md).
                if from_page_trigger.get() && self.is_list(st) {
                    st.borrow_mut().errors.push((idx, e.message.clone()));
                    return Err(error_code(
                        format!(
                            "Validation error for Field: {},  Message = '{}'",
                            page_of(&engine, st).controls[idx].name,
                            e.message
                        ),
                        "TestValidation",
                    ));
                }
                // An error of the table's validation (its trigger, the length of the text) carries TestValidation
                // (record/lt-loadfields-pagesource-validate/P81PageValidateEvents.al:TestPage_SetValue_NegativeQty_TableTriggerRaises,
                // TestPage_SetValue_NameTooLong).
                Err(refuse_bound(e.message))
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
                // The client reads the row of a card before Cancel, as before any action (pages/lt-page-runtime/P82PageRun.al:
                // RunModal_NumberAndRecord_Cancel `OnAfterGetRecord(C);OnAfterGetRecord(C);OnQueryClosePage(LookupCancel)`); OK reads nothing
                // (RunModal_Number_OnCard).
                if name == "Cancel"
                    && self.has_source(st)
                    && !self.is_list(st)
                    && !self.is_request_page(st)
                    && st.borrow().cursor == Cursor::Row
                    && !st.borrow().draft
                    && self.declares(st, "onaftergetrecord")
                {
                    self.populate_current(st)?;
                    // A page with OnAfterGetCurrRecord runs it after that read (errors/lt-ms-refusals-129/P129Handlers.al:
                    // P129_TwoTestPageHandlers_SecondPageOpenedByAnActionOfTheFirstHandler: `card AGR 1;card AGCR 1;card OnQueryClosePage`).
                    if self.declares(st, "onaftergetcurrrecord") {
                        self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
                        self.page_event(st, "onaftergetcurrrecordevent", None, None)?;
                    }
                }
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
                self.refresh_host_after_part_edit(st)?;
                Ok(r)
            }
            PageMethod::Value => {
                if self.shows_no_row(st, idx) {
                    return Ok(Value::Text(String::new().into()));
                }
                let v = self.control_value(st, idx)?;
                // An Enum control that refused an entry shows the ordinal, not the caption (controls/W2ValueControls.al:
                // Enum_VariableControl_*, `value` 0 after a refused `Alpha`, BC 27.5 and 29.0).
                // An Option control does the same after the empty text was refused (pages/lt-testpage-assertequals-option/
                // TestPageAssertEqualsOption.al:SetValue_BlankOption_AsEmptyText, `afterEmpty` [1]); other refused option texts are not pinned.
                if let Value::Enum(_, ordinal) | Value::Option(ordinal, _) = &v {
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
                    Some(RtType::Enum(id)) => {
                        self.note_enum(id);
                        engine.program.objects[id as usize]
                            .enum_values
                            .iter()
                            .map(|(v, _)| *v)
                            .collect()
                    }
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
                    // A label shows no mandatory mark (misc/lt-probes-needed-7/PnGControlKinds.al:LabelValueAndCaption `label-mandatory = No`).
                    None if matches!(page_of(&engine, st).controls[idx].binding, ir::Binding::Caption) => {
                        Ok(Value::Boolean(false))
                    }
                    // ShowMandatory without the property is Yes when another control of the page shows the same table field with
                    // ShowMandatory = true, else No (misc/lt-probes-needed-7/PnGFindField.al:HideValueAndShowMandatoryLiterals:
                    // `PlainGrpCtl` Yes next to `MandatoryCtl` on the same field, `NameCtl` No; pages/lt-page-runtime/P82TestField.al:
                    // Properties_DynamicVisible_And_ShowMandatory `no-showmandatory` No; handlers/TestProbePageHandlerContracts.al:
                    // TestPage_Members_ShowMandatoryHideValue_Defaults `name_showmandatory` Yes next to `ShowMandatoryField`). For
                    // a control bound to something else than a table field the answer is not pinned.
                    None if m == PageMethod::ShowMandatory => {
                        let page = page_of(&engine, st);
                        match &page.controls[idx].binding {
                            ir::Binding::Field(pos) => Ok(Value::Boolean(page.controls.iter().any(|c| {
                                matches!(&c.binding, ir::Binding::Field(p) if p == pos)
                                    && matches!(c.prop("showmandatory"), Some(ir::PropVal::Lit(l)) if unquote(l).eq_ignore_ascii_case("true"))
                            }))),
                            _ => unsupported("ShowMandatory of a control that is no table field, without the property (not pinned)"),
                        }
                    }
                    None => Ok(Value::Boolean(false)),
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
                self.refresh_host_after_part_edit(st)?;
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
