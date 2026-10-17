use super::*;

impl Session {
    /// The value a control shows: its binding, read now.
    pub(super) fn control_value(&mut self, st: &St, idx: usize) -> Result<Value, Signal> {
        let engine = self.engine();
        match page_of(&engine, st).controls[idx].binding.clone() {
            ir::Binding::Field(pos) => {
                if st.borrow().stale {
                    return unsupported(
                        "reading a row after CurrPage.Update on a page with OnFindRecord (the row it settles on is not pinned)",
                    );
                }
                Ok(self.page_rec(st).borrow().fields[pos].clone())
            }
            b @ (ir::Binding::Var(_) | ir::Binding::BaseVar(_)) => {
                let (slot, owner) = b
                    .var_slot(page_of(&engine, st).controls[idx].owner)
                    .expect("a variable binding");
                let inst = self.owner_instance(st, owner);
                let v = inst.borrow().globals[slot].clone();
                Ok(v)
            }
            ir::Binding::Expr(r) => self.control_routine(st, idx, r),
            // A label shows its caption (handlers/TestPageLabelControlContracts.al:Label_IsATestPageMember_WithItsCaption).
            ir::Binding::Caption => Ok(Value::Text(self.control_caption(st, idx)?.into())),
            ir::Binding::None => unsupported("a control without a source"),
        }
    }

    pub(super) fn control_as(&mut self, st: &St, idx: usize, to: &RtType) -> Result<Value, Signal> {
        if self.shows_no_row(st, idx) {
            return Ok(Value::default_for(to));
        }
        let v = self.control_value(st, idx)?;
        ops::convert(v, to)
    }

    /// A control bound to the page's record reads blank when the page shows no row: whatever its
    /// type, and on a field with an InitValue too (corpus testpart/BnrBlankPart.al:1-30).
    pub(super) fn shows_no_row(&self, st: &St, idx: usize) -> bool {
        let engine = self.engine();
        st.borrow().cursor == Cursor::None
            && matches!(page_of(&engine, st).controls[idx].binding, ir::Binding::Field(_))
    }

    /// The source field of a control bound to a record field.
    pub(super) fn control_field(&self, st: &St, idx: usize) -> Option<ir::Field> {
        let engine = self.engine();
        if !self.has_source(st) {
            return None;
        }
        let page = page_of(&engine, st);
        let ir::Binding::Field(pos) = page.controls[idx].binding else {
            return None;
        };
        let table = self.page_rec(st).borrow().table as usize;
        engine.program.objects[table]
            .table
            .as_ref()
            .map(|t| t.fields[pos].clone())
    }

    /// How a control spells its value: the control's own properties, then the source field's.
    pub(super) fn control_format(&mut self, st: &St, idx: usize) -> Result<ControlFormat, Signal> {
        let engine = self.engine();
        let page = page_of(&engine, st);
        let ctl = &page.controls[idx];
        let field = self.control_field(st, idx);
        let from_field = |n: &str| {
            field.as_ref().and_then(|f| {
                f.properties
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(n))
                    .map(|(_, v)| v.clone())
            })
        };
        let lit = |n: &str| match ctl.prop(n) {
            Some(ir::PropVal::Lit(v)) => Some(v.clone()),
            _ => None,
        };
        let mut f = ControlFormat {
            decimal_places: lit("decimalplaces")
                .or_else(|| from_field("decimalplaces"))
                .and_then(|p| pagefmt::parse_decimal_places(&p)),
            ..Default::default()
        };
        // An auto-format expression carries a precision: `<Precision,4:4><Standard Format,0>`
        // (corpus autoformat/TestAutoFormat.al, TestAutoFormatExpressionTiming.al).
        if f.decimal_places.is_none() {
            let expr = match ctl.prop("autoformatexpression") {
                Some(ir::PropVal::Lit(v)) => Some(unquote(v)),
                Some(routine) => match self.prop_routine_value(st, idx, routine)? {
                    Some(Value::Text(t) | Value::Code(t)) => Some(t.into_string()),
                    _ => None,
                },
                None => None,
            };
            f.decimal_places = expr.as_deref().and_then(pagefmt::parse_precision);
            // AutoFormatType 1, 2, 4 and 5 (on the control, else on the table field): the places of an
            // amount or a unit amount, from the currency the expression names
            // (autoformat/PWAutoFormatTypes.al, see `auto_format_places`).
            if f.decimal_places.is_none() {
                let kind = lit("autoformattype").or_else(|| from_field("autoformattype"));
                if let Some(kind) = kind
                    .as_deref()
                    .map(str::trim)
                    .filter(|k| matches!(*k, "1" | "2" | "4" | "5"))
                {
                    let expr = expr
                        .or_else(|| from_field("autoformatexpression").map(|e| unquote(&e)))
                        .unwrap_or_default();
                    f.decimal_places = Some(self.auto_format_places(kind, &expr)?);
                }
            }
        }
        f.blank_zero = lit("blankzero")
            .or_else(|| from_field("blankzero"))
            .is_some_and(|v| is_true(&v));
        if let Some(b) = lit("blanknumbers").or_else(|| from_field("blanknumbers")) {
            f.blank = Blank::parse(&b).unwrap_or_default();
        }
        f.option_captions = lit("optioncaption")
            .or_else(|| from_field("optioncaption"))
            .map(|c| unquote(&c).split(',').map(str::to_string).collect());
        if f.option_captions.is_none() {
            // Without captions an option shows its member names, a blank member as nothing.
            f.option_captions = from_field("optionmembers").map(|c| {
                unquote(&c)
                    .split(',')
                    .map(|m| m.trim().trim_matches('"').to_string())
                    .collect()
            });
        }
        Ok(f)
    }

    /// The places a decimal control with `AutoFormatType` 1, 2, 4 or 5 shows (corpus autoformat/PWAutoFormatTypes.al,
    /// BC 27.5; the setup does not matter in any of the three setups probed, so General Ledger Setup is not read):
    /// 1 and 4 are amounts, 2 and 5 unit amounts. A blank expression gives 2:2 for an amount and 2:5 for a unit
    /// amount (:98-175). A currency code that exists gives the currency's "Amount Decimal Places" or
    /// "Unit-Amount Decimal Places" (:177-195), both empty gives 2:2 (:197-217), and a code that is no
    /// currency gives 2:5 (:263-290). A currency with only one of the two places empty is not pinned.
    pub(super) fn auto_format_places(&mut self, kind: &str, expr: &str) -> Result<(u32, u32), Signal> {
        let unit = matches!(kind, "2" | "5");
        if expr.trim().is_empty() {
            return Ok(if unit { (2, 5) } else { (2, 2) });
        }
        let engine = self.engine();
        let Some(oi) = engine
            .program
            .objects
            .iter()
            .position(|o| o.id == Some(4) && o.table.is_some())
        else {
            return unsupported(format!(
                "AutoFormatType {kind} with the currency `{expr}`: the Currency table is not in the program (not pinned)"
            ));
        };
        let Some(t) = engine.tables[oi].clone() else {
            return unsupported("AutoFormatType with a currency: the Currency table did not compile");
        };
        let field = |name: &str| t.fields.iter().position(|f| f.name.eq_ignore_ascii_case(name));
        let (Some(amount_at), Some(unit_at)) = (field("Amount Decimal Places"), field("Unit-Amount Decimal Places"))
        else {
            return unsupported("AutoFormatType with a currency: the Currency table has no decimal places fields");
        };
        let Ok(key) = ops::convert(Value::Text(expr.to_string().into()), &t.fields[0].ty) else {
            return Ok((2, 5));
        };
        let row = self
            .store
            .get(&self.cx, &t, &[key])
            .map_err(|e| Signal::Unsupported(format!("store error {e}")))?;
        let Some(row) = row else {
            return Ok((2, 5));
        };
        let text = |at: usize| {
            al_builtins::native::text_of(&row[at])
                .unwrap_or_default()
                .trim()
                .to_string()
        };
        let (amount, unit_places) = (text(amount_at), text(unit_at));
        if amount.is_empty() && unit_places.is_empty() {
            return Ok((2, 2));
        }
        let wanted = if unit { unit_places } else { amount };
        match pagefmt::parse_decimal_places(&wanted) {
            Some(p) if !wanted.is_empty() => Ok(p),
            _ => unsupported(format!(
                "AutoFormatType {kind} with the currency `{expr}`, whose places are `{wanted}` (one of the two empty is not pinned)"
            )),
        }
    }

    pub(super) fn control_text(&mut self, st: &St, idx: usize, v: &Value) -> Result<String, Signal> {
        // A Media control shows the id of its media, a MediaSet control the id of the first media
        // of its set; lowercase, no braces, empty when there is none
        // (handlers/TestPageFieldFormats_Tests.al:28, :628-640, TestPageFieldFormatsMediaFilled_Tests.al).
        if let (Some(RtType::Library(n)), Value::Guid(id)) = (self.control_type(st, idx, v), v) {
            let shown = if n.eq_ignore_ascii_case("Media") {
                Some(*id)
            } else if n.eq_ignore_ascii_case("MediaSet") {
                self.media.sets.borrow().get(id).and_then(|m| m.first().copied())
            } else {
                None
            };
            if n.eq_ignore_ascii_case("Media") || n.eq_ignore_ascii_case("MediaSet") {
                return Ok(match shown {
                    Some(g) if g != [0; 16] => al_builtins::pagefmt::guid_text(&g)
                        .trim_matches(['{', '}'])
                        .to_lowercase(),
                    _ => String::new(),
                });
            }
        }
        if let Value::Library(name, _) = v {
            return unsupported(format!("control text of a {name} value"));
        }
        // A control with HideValue = true shows no value (handlers/TestProbePageHandlerContracts.al:
        // TestPage_Members_ShowMandatoryHideValue_Defaults, `hidevalue_field_value` is empty).
        {
            let engine = self.engine();
            if let Some(ir::PropVal::Lit(l)) = page_of(&engine, st).controls[idx].prop("hidevalue") {
                if unquote(l).eq_ignore_ascii_case("true") {
                    return Ok(String::new());
                }
            }
        }
        let f = self.control_format(st, idx)?;
        let engine = self.engine();
        let caption = move |id: u32, ordinal: i32| {
            let o = &engine.program.objects[id as usize];
            o.enum_captions
                .iter()
                .find(|(v, _)| *v == ordinal)
                .or_else(|| o.enum_values.iter().find(|(v, _)| *v == ordinal))
                .map(|(_, c)| c.clone())
                .unwrap_or_else(|| ordinal.to_string())
        };
        let text = pagefmt::control_text(v, &f, &caption);
        // A DateTime control writes a plain space before AM / PM on every version (the corpus re-pinned it for the newer
        // service image: pages/lt-page-runtime/P82TestField.al `dt-value`, pages/lt-page-run-overloads-testpart/
        // P106TestFieldAsDateTime.al:P106_AsDateTime_Values; the older pin with a narrow no-break space is gone).
        Ok(text)
    }

    /// The type of the value a control holds.
    pub(super) fn control_type(&self, st: &St, idx: usize, v: &Value) -> Option<RtType> {
        if let Some(f) = self.control_field(st, idx) {
            return Some(f.ty);
        }
        let engine = self.engine();
        let ctl = &page_of(&engine, st).controls[idx];
        if let Some((slot, owner)) = ctl.binding.var_slot(ctl.owner) {
            let object = owner.unwrap_or(st.borrow().object as usize);
            return Some(engine.program.objects[object].globals[slot].ty.clone());
        }
        // A control bound to an expression has the type of the value the expression gives now (its routine returns a
        // Variant); only the plain kinds, and only when the expression can be written.
        if matches!(ctl.binding, ir::Binding::Expr(_)) && ctl.trigger("__sourceset").is_some() && self.has_source(st) {
            return match v {
                Value::Decimal(_) => Some(RtType::Decimal),
                Value::Integer(_) => Some(RtType::Integer),
                Value::BigInteger(_) => Some(RtType::BigInteger),
                Value::Boolean(_) => Some(RtType::Boolean),
                Value::Date(..) => Some(RtType::Date),
                Value::Text(_) => Some(RtType::Text(None)),
                Value::Code(_) => Some(RtType::Code(None)),
                _ => None,
            };
        }
        None
    }

    /// corpus handlers/TestPageDecimalAssertEquals_Tests.al:85, TestPageVariableControlErrorName.al:89:
    /// `AssertEquals for Field: <control> Expected = '<expected>', Actual = '<actual>'`.
    pub(super) fn control_assert_equals(&mut self, st: &St, idx: usize, args: &mut [Value]) -> Result<Value, Signal> {
        let actual = self.control_value(st, idx)?;
        let actual_text = if self.shows_no_row(st, idx) {
            String::new()
        } else {
            self.control_text(st, idx, &actual)?
        };
        let expected = args.first().cloned().unwrap_or(Value::Null);
        // A text is compared as written; any other value is converted to the field's type and
        // spelled through the control (corpus handlers/TestPageFieldFormats_Tests.al:1-40).
        let expected_text = match &expected {
            Value::Text(t) | Value::Code(t) => t.clone(),
            other => {
                let converted = match self.control_type(st, idx, &actual) {
                    Some(rt) => ops::convert(other.clone(), &rt)?,
                    None => other.clone(),
                };
                self.control_text(st, idx, &converted)?.into()
            }
        };
        if expected_text != actual_text {
            let engine = self.engine();
            let name = page_of(&engine, st).controls[idx].name.clone();
            // misc/lt-probes-needed-7/PnGTempControl.al:AssertEqualsThroughTheControls and pages/lt-page-runtime/P82TestField.al:
            // the error code is TestAssertEquals.
            return Err(error_code(
                format!("AssertEquals for Field: {name} Expected = '{expected_text}', Actual = '{actual_text}'"),
                "TestAssertEquals",
            ));
        }
        Ok(Value::Null)
    }

    /// The control's caption: its own `Caption`, else the source field's, else its name.
    pub(super) fn control_caption(&mut self, st: &St, idx: usize) -> Result<String, Signal> {
        let engine = self.engine();
        let ctl = &page_of(&engine, st).controls[idx];
        // CaptionClass `3,<text>` is a caption the platform resolves to <text>
        // (corpus autoformat/TestAutoFormatExpressionTiming.al, WorkingCaptionClass_OpensAndReadsItsCaption).
        let class = match ctl.prop("captionclass") {
            Some(ir::PropVal::Lit(c)) => Some(unquote(c)),
            Some(routine) => match self.prop_routine_value(st, idx, routine)? {
                Some(Value::Text(t) | Value::Code(t)) => Some(t.into_string()),
                _ => None,
            },
            None => None,
        };
        if let Some(c) = class {
            return self.caption_class_text(st, &c);
        }
        if let Some(ir::PropVal::Lit(c)) = ctl.prop("caption") {
            return Ok(unquote(c));
        }
        if let Some(f) = self.control_field(st, idx) {
            if let Some((_, c)) = f.properties.iter().find(|(k, _)| k.eq_ignore_ascii_case("caption")) {
                return Ok(unquote(c));
            }
            return Ok(f.name);
        }
        Ok(ctl.name.clone())
    }

    /// A boolean control property, evaluated now: a literal, a routine, or `default`.
    pub(super) fn prop_bool_now(&mut self, st: &St, idx: usize, name: &str, default: bool) -> Result<bool, Signal> {
        let engine = self.engine();
        match page_of(&engine, st).controls[idx].prop(name) {
            None => Ok(default),
            Some(ir::PropVal::Lit(v)) => Ok(is_true(v)),
            Some(routine) => match self.prop_routine_value(st, idx, routine)? {
                Some(v) => ops::truthy(&v),
                None => Ok(default),
            },
        }
    }

    /// The caption of the page in force: what `CurrPage.Caption := x` set, else its `Caption`
    /// property, else the page name.
    pub(super) fn page_caption(&self, st: &Rc<RefCell<PageState>>) -> String {
        if let Some(c) = &st.borrow().caption {
            return c.clone();
        }
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let o = &engine.program.objects[object];
        o.properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("Caption"))
            .map(|(_, v)| unquote(v))
            .unwrap_or_else(|| o.name.clone())
    }

    pub(super) fn page_editable(&self, st: &St) -> bool {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let props = &engine.program.objects[object].properties;
        // A list handed over in lookup mode is read-only, page-variable controls included
        // (corpus handlers/TestPageControlEditableRoutes.al:446 List_RunModal_LookupMode).
        st.borrow().mode != PageMode::View
            && !st.borrow().editable_off
            && !(st.borrow().lookup && self.is_list_page(st))
            && !props
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case("Editable") && v.trim().eq_ignore_ascii_case("false"))
    }

    /// Editable and Enabled follow the page state live (corpus
    /// handlers/TestPageControlPropertyLiveness_Tests.al:73, TestPageEditable_Tests.al).
    pub(super) fn control_editable(&mut self, st: &St, idx: usize) -> Result<bool, Signal> {
        // A control bound to a page variable stays editable on a read-only page (corpus
        // handlers/TestPageReadOnlySetValue.al:365 OpenView_PageVariableControl_StaysEditable).
        let engine = self.engine();
        let rec_bound = !matches!(
            page_of(&engine, st).controls[idx].binding,
            ir::Binding::Var(_) | ir::Binding::BaseVar(_)
        ) || (st.borrow().lookup && self.is_list_page(st))
            || st.borrow().editable_off
            // A request page that says `Editable = false` makes its variable-bound fields read No too
            // (handlers/PSRequestPageEditable.al Editable_RequestPageEditableFalse).
            || (self.is_request_page(st) && !self.page_editable(st));
        // A label is never editable (handlers/TestPageLabelControlContracts.al). A cuegroup with
        // `Editable = false` makes its fields read No (handlers/TestPageCueGroupFixedGridContracts.al:
        // CueGroup_SetValue_IsSilentlyIgnored).
        if matches!(page_of(&engine, st).controls[idx].binding, ir::Binding::Caption) {
            return Ok(false);
        }
        let mut cue = page_of(&engine, st).controls[idx].parent;
        while let Some(p) = cue {
            if page_of(&engine, st).controls[p].kind == ir::ControlKind::CueGroup
                && !self.prop_bool_now(st, p, "editable", true)?
            {
                return Ok(false);
            }
            cue = page_of(&engine, st).controls[p].parent;
        }
        Ok((!rec_bound || self.page_editable(st)) && self.prop_bool_now(st, idx, "editable", true)?)
    }

    /// Whether control `idx` sits inside a `cuegroup`.
    pub(super) fn in_cue_group(&self, st: &St, idx: usize) -> bool {
        let engine = self.engine();
        let page = page_of(&engine, st);
        let mut at = page.controls[idx].parent;
        while let Some(p) = at {
            if page.controls[p].kind == ir::ControlKind::CueGroup {
                return true;
            }
            at = page.controls[p].parent;
        }
        false
    }

    pub(super) fn control_enabled(&mut self, st: &St, idx: usize) -> Result<bool, Signal> {
        self.prop_bool_now(st, idx, "enabled", true)
    }

    pub(super) fn control_visible(&mut self, st: &St, idx: usize) -> Result<bool, Signal> {
        let engine = self.engine();
        let page = page_of(&engine, st);
        // Own Visible is the open-time value; a group's and a part's are live (a part: pages/lt-page-filtergroup-assumptions/
        // P99InvisiblePart.al:P99_FlagTrueInOnAfterGetCurrRecord) and so are the groups around the
        // control (corpus handlers/TestPageControlPropertyLiveness_Tests.al:56).
        let own = if matches!(
            page.controls[idx].kind,
            ir::ControlKind::Group | ir::ControlKind::CueGroup | ir::ControlKind::Part
        ) {
            self.prop_bool_now(st, idx, "visible", true)?
        } else {
            st.borrow().frozen_visible.get(idx).copied().unwrap_or(true)
        };
        let mut visible = own;
        let mut parent = page.controls[idx].parent;
        while let Some(p) = parent {
            visible &= self.prop_bool_now(st, p, "visible", true)?;
            parent = page.controls[p].parent;
        }
        Ok(visible)
    }
}
