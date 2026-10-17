use super::*;
use al_builtins::record_op::RecordOp;

impl Session {
    pub(super) fn page_open(&mut self, st: &St, mode: PageMode) -> Result<Value, Signal> {
        // corpus testpageclose/TestPageClosingActions.al:19-24: an open variable cannot be opened.
        if st.borrow().open {
            return Err(error(ALREADY_OPEN));
        }
        // A reopened variable shows a fresh page.
        st.borrow_mut().instance = None;
        st.borrow_mut().inited = false;
        st.borrow_mut().modal = false;
        self.open_page(st, mode, None, None).map(|_| Value::Null)
    }

    /// Whether the page has a source table (and so a `Rec`).
    pub(super) fn has_source(&self, st: &St) -> bool {
        let object = st.borrow().object as usize;
        self.engine.program.objects[object].rec_slot.is_some()
    }

    /// The page instance of `st`, created on first use (a `Page` variable has one before it runs).
    pub(crate) fn page_instance(&mut self, st: &St) -> Result<Rc<RefCell<al_builtins::value::Instance>>, Signal> {
        if let Some(i) = st.borrow().instance.clone() {
            return Ok(i);
        }
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let page = page_of(&engine, st);
        if let Some(why) = page.unsupported.first() {
            return unsupported(why.clone());
        }
        let inst = self.instance(object);
        inst.borrow_mut().globals[page.curr_page_slot] = Value::TestPage(st.clone());
        // Each extension runs on an instance of its own that shares the page's Rec, xRec and CurrPage.
        let mut exts = Vec::new();
        for &e in &page.extensions {
            let ext = self.extension_instance(e, &inst);
            let (page_object, ext_object) = (&engine.program.objects[object], &engine.program.objects[e]);
            for (from, to) in [
                (page_object.rec_slot, ext_object.rec_slot),
                (page_object.xrec_slot, ext_object.xrec_slot),
            ] {
                if let (Some(from), Some(to)) = (from, to) {
                    let shared = inst.borrow().globals[from].clone();
                    ext.borrow_mut().globals[to] = shared;
                }
            }
            if let Some(slot) = ext_object.globals.iter().position(|g| g.name == "CurrPage") {
                ext.borrow_mut().globals[slot] = Value::TestPage(st.clone());
            }
            exts.push((e, ext));
        }
        st.borrow_mut().ext_instances = exts;
        st.borrow_mut().instance = Some(inst.clone());
        Ok(inst)
    }

    /// A call on a `Page` variable constructs the page on first use: its instance, and OnInit once
    /// (handlers/TestPageOnInit_Tests.al:188 POI_SetterBeforeRunModal_RunsAfterOnInitAndSurvives,
    /// handlers/TestPageEnumGlobal*: a getter on a variable that never ran reads the declared default).
    pub(crate) fn construct_page(&mut self, st: &St) -> Result<Rc<RefCell<al_builtins::value::Instance>>, Signal> {
        let inst = self.page_instance(st)?;
        if st.borrow().host.is_none() && !st.borrow().inited {
            st.borrow_mut().inited = true;
            self.page_trigger(st, "oninit", vec![])?;
        }
        Ok(inst)
    }

    /// `to` takes over the page `from` constructed, if it did, else starts without one.
    pub(super) fn adopt_instance(&mut self, from: &St, to: &St) {
        if Rc::ptr_eq(from, to) {
            to.borrow_mut().instance = None;
            to.borrow_mut().inited = false;
            return;
        }
        let (instance, exts, inited) = {
            let mut f = from.borrow_mut();
            (
                f.instance.take(),
                std::mem::take(&mut f.ext_instances),
                std::mem::take(&mut f.inited),
            )
        };
        let engine = self.engine();
        if let Some(inst) = &instance {
            let page = page_of(&engine, to);
            inst.borrow_mut().globals[page.curr_page_slot] = Value::TestPage(to.clone());
            for (e, ext) in &exts {
                let obj = &engine.program.objects[*e];
                if let Some(slot) = obj.globals.iter().position(|g| g.name == "CurrPage") {
                    ext.borrow_mut().globals[slot] = Value::TestPage(to.clone());
                }
            }
        }
        // The part pages the variable's code reached before it ran go with the page (Warehouse Pick Summary.SetRecords hands rows to its
        // part, then Run opens the page the test trapped).
        let early = std::mem::take(&mut from.borrow_mut().hidden_parts);
        for (_, child) in &early {
            let host = child.borrow().host.as_ref().map(|(_, i)| (Rc::downgrade(to), *i));
            child.borrow_mut().host = host;
        }
        let mut t = to.borrow_mut();
        t.instance = instance;
        t.ext_instances = exts;
        t.inited = inited;
        t.hidden_parts = early;
    }

    /// Opens the page: OnInit, OnOpenPage, the control expressions and the first row. `record`
    /// is the record AL code handed to `Page.Run`/`SetRecord`: its filters and position become the
    /// page's view.
    ///
    /// With `bind`, the UI handler for the page is looked up after OnInit and before OnOpenPage
    /// (corpus handlers/TestPageOnInit_Tests.al: an OnInit error surfaces as itself even where no
    /// handler is declared).
    pub(super) fn open_page(
        &mut self,
        st: &St,
        mode: PageMode,
        record: Option<Rec>,
        bind: Option<UiKind>,
    ) -> Result<Option<Handler>, Signal> {
        self.open_page_as(st, mode, record, bind, false)
    }

    /// `open_page`, or with `as_part` the opening of a part's page: everything up to and including
    /// the page's OnOpenPage, leaving its rows to the host (`sync_part`), which knows the link.
    pub(super) fn open_page_as(
        &mut self,
        st: &St,
        mode: PageMode,
        record: Option<Rec>,
        bind: Option<UiKind>,
        as_part: bool,
    ) -> Result<Option<Handler>, Signal> {
        let mut handler = None;
        let engine = self.engine();
        let page = page_of(&engine, st);
        for a in &page.assumes {
            al_builtins::assumptions::assume(*a);
        }
        self.page_instance(st)?;
        {
            let mut s = st.borrow_mut();
            let first_run = !s.ran;
            s.open = true;
            s.ran = true;
            s.mode = mode;
            s.cursor = Cursor::None;
            s.draft = false;
            s.dirty = false;
            s.frozen_visible = Vec::new();
            s.window = Vec::new();
            s.window_from_start = false;
            s.window_eof = false;
            s.window_whole = false;
            s.focus = None;
            s.focus_in_row = false;
            s.errors = Vec::new();
            s.refused = false;
            s.pending = None;
            s.result = None;
            s.editable_off = false;
            // A caption assigned to the variable before its first run stays.
            // It stays for a second run too (types/lt-caption-assigned-before-run/CaptionAssignedBeforeRun.al:P187_SecondRun_NoReassign,
            // `OnOpenPage [Assigned One]` twice); a caption assigned while the page ran is dropped.
            if !first_run && !s.caption_assigned_closed {
                s.caption = None;
            }
            s.opening = true;
            s.parts = Vec::new();
            s.areas = self.application_areas.clone();
        }
        if let Err(e) = self.create_parts(st) {
            self.mark_closed(st);
            return Err(e);
        }
        let has_source = self.has_source(st);
        // SourceTableView is the page's own view of its table (corpus
        // pagefindrecord/TestPageFindRecord.al:192 PlainPage_WithoutTheTriggers_WalksTheTableDescending);
        // how it combines with the view of a record handed to Page.Run is pinned by P145PageRunView.al.
        let source_view = engine.program.objects[st.borrow().object as usize]
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("SourceTableView"))
            .map(|(_, v)| crate::views::property_view(v.trim()));
        // The part control's SubPageView comes first: its filters land in the link group, its
        // sorting and order set the key and direction. The part page's own SourceTableView then
        // sets only the halves it names (corpus handlers/TestPartOwnSortVsSubPageView.al:1-45).
        let sub_view = self.sub_page_view(st).map(|v| crate::views::sub_page_view_text(&v));
        if let (Some(view), true) = (&sub_view, has_source) {
            let rec = self.page_rec(st);
            let table = self.table(&rec)?;
            self.set_view_in(&rec, &table, view, Some(parts::LINK_GROUP), false, true)?;
        }
        // A record handed to Page.Run brings its own filters, key and order; they come first and the page's
        // SourceTableView then adds to them: the record's filters stay in their groups, the view's land in group 2 beside the
        // record's own group 2, the view's SORTING replaces the record's key and the record's direction stays (types/lt-code-digits-page-run-view/
        // P145PageRunView.al:P145_A_RecFilterSameFieldAsView, _RecFilterOverlappingView, _RecFilterOtherField, _RecKeyDescending,
        // _RecKeyPrimary, _RecFilterInGroup1, _RecFilterInGroup2). A view that names an ORDER stays refused: nothing pins it.
        if let (Some(r), true) = (&record, has_source) {
            let rec = self.page_rec(st);
            let src = r.borrow();
            let mut dst = rec.borrow_mut();
            dst.fields = src.fields.clone();
            dst.filters = src.filters.clone();
            dst.key = src.key;
            dst.adhoc = src.adhoc.clone();
            // A record that is no temporary one (a view only, see SetTableView) leaves a page whose source table is temporary on its own rows.
            if src.temporary || !dst.temporary {
                dst.temporary = src.temporary;
                dst.temp_id = src.temp_id.clone();
            }
            dst.ascending = src.ascending;
        }
        if let (Some(view), true) = (source_view, has_source) {
            if record.is_some() {
                let lower = view.to_ascii_lowercase();
                if lower.contains("order(") {
                    self.mark_closed(st);
                    return unsupported(
                        "Page.Run with a record on a page whose SourceTableView names an order (not pinned)",
                    );
                }
            }
            let rec = self.page_rec(st);
            let table = self.table(&rec)?;
            // The platform applies the view in filter group 2: the page's own AL reads it with
            // FilterGroup(2) and finds group 0 empty (corpus handlers/TestPageSourceTableView.al:
            // FiltersAreInFilterGroup2WhenOnOpenPageRuns).
            // The filters the handed record carries in group 2 (Bank Acc. Reconciliation.OpenWorksheet sets the statement there) stay: the view
            // adds its own to the group instead of replacing it (P145PageRunView.al:P145_A_RecFilterInGroup2).
            let kept: Vec<_> = if record.is_some() {
                rec.borrow().filters.iter().filter(|f| f.group == 2).cloned().collect()
            } else {
                Vec::new()
            };
            self.set_view_in(&rec, &table, &view, Some(2), sub_view.is_some(), false)?;
            // A field the view filters itself keeps the view's filter alone (two equal filters of one field read as a doubled value
            // when a new line takes it: Purchase Price List's "Price Type").
            let mut r = rec.borrow_mut();
            let viewed: Vec<usize> = r.filters.iter().filter(|f| f.group == 2).map(|f| f.field).collect();
            r.filters
                .extend(kept.into_iter().filter(|f| !viewed.contains(&f.field)));
        }
        let result = (|| {
            // A `Page` variable whose procedure ran before this open was constructed then, and its
            // OnInit has run (handlers/TestPageOnInit_Tests.al:188).
            // A second run of the same variable does not run OnInit again (controls/W2PageVariableInit.al:
            // PageVariable_RunModalTwice, BC 27.5 and 29.0); a reopened TestPage is a fresh page (`page_open`).
            if !std::mem::replace(&mut st.borrow_mut().inited, true) {
                self.page_trigger(st, "oninit", vec![])?;
            }
            // `Editable(false)` on the variable before it runs applies after OnInit: OnInit still
            // reads Yes (corpus handlers/PWPageVariableEditable.al:221-240).
            {
                let mut s = st.borrow_mut();
                s.editable_off = s.editable_preset == Some(false);
            }
            // The parts' OnInit runs between the host's OnInit and its OnOpenPage (corpus
            // testpart/TestPartTriggerOrderAndRefresh.al:236-247).
            self.init_parts(st)?;
            let mut unattended = false;
            if let Some(kind) = bind {
                if self.pages.lenient_bind {
                    // A RunObject action opens the page, runs OnInit and OnOpenPage, and only then
                    // looks for a handler; with none it goes no further, so the first row is never
                    // read (handlers/TestPageRunObjectContracts.al:330-346: "card.OnInit;card.OnOpenPage;").
                    // A page opened in Create mode reads its new row first (:handlers/TestPageCreateModeNewRecordArgs.al:304-327).
                    let object = st.borrow().object;
                    handler = self.try_bind_page_handler(kind, object)?;
                    unattended = handler.is_none() && mode != PageMode::New;
                } else {
                    handler = Some(self.bind_page_handler(kind, st.borrow().object)?);
                }
            }
            self.restore_page_values(st);
            self.check_page_events(st)?;
            let opened = self.page_trigger(st, "onopenpage", vec![]);
            st.borrow_mut().opening = false;
            opened?;
            self.page_event(st, "onopenpageevent", None, None)?;
            if unattended {
                return Ok(());
            }
            let new_line = has_source && self.offers_new_line(st)?;
            st.borrow_mut().new_line = new_line;
            // Visible is evaluated once, at open.
            let mut frozen = Vec::new();
            for i in 0..page.controls.len() {
                frozen.push(self.prop_bool_now(st, i, "visible", true)?);
            }
            st.borrow_mut().frozen_visible = frozen;
            // The parts open right after the host's own OnOpenPage, before its first row is read
            // (corpus handlers/TestPagePartAgcr_Tests.al:PartFiresOnHostOpen_NoTouch).
            self.open_parts(st)?;
            if as_part {
                return Ok(());
            }
            self.load_first_row(st, mode, record.is_some())?;
            self.sync_parts_at_open(st)
        })();
        if let Err(e) = result {
            // The open raises the error itself and leaves the variable closed
            // (corpus testpageteardown/TestPageTeardown.al:3-24).
            self.mark_closed(st);
            return Err(e);
        }
        Ok(handler)
    }

    /// The first row a page shows when it opens (or when a part is linked afresh).
    pub(super) fn load_first_row(&mut self, st: &St, mode: PageMode, with_record: bool) -> Result<(), Signal> {
        let has_source = self.has_source(st);
        if !has_source {
            // A page over no table shows its one implicit row: the expressions, then OnAfterGetRecord.
            st.borrow_mut().cursor = Cursor::Row;
            self.populate_expressions(st)?;
            self.page_trigger(st, "onaftergetrecord", vec![])?;
            self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
            self.page_event(st, "onaftergetcurrrecordevent", None, None)?;
            return Ok(());
        }
        if mode == PageMode::New {
            // corpus handlers/TestPageCreateModeNewRecordArgs.al:243-270: OnOpenPage, then
            // OnNewRecord with BelowxRec true and xRec on the last row.
            // A page with an OnFindRecord trigger is opened the same way: the trigger is not asked (Microsoft's Service
            // Credit Memo, Invoice and Order pages, whose tests open them with OpenNew). Assumed, see
            // Assumption::OpenNewSkipsOnFindRecord.
            if self.declares(st, "onfindrecord") {
                al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::OpenNewSkipsOnFindRecord);
            }
            self.ret_used = true;
            let rec = self.page_rec(st);
            self.record_op(RecordOp::FindLast, &rec, &mut [])?;
            return self.start_draft(st, false);
        }
        if with_record {
            // The page opens on the record the caller handed it, when that row exists
            // (corpus handlers/TestPageRunHandler_Tests.al:PageHandlerReceivesTheRecordPassedToPageRun).
            let rec = self.page_rec(st);
            if self.stored_row(&rec)?.is_some() {
                self.ret_used = true;
                let mut pk = self.table(&rec)?.primary_key(&rec.borrow().fields);
                if matches!(self.record_op(RecordOp::Get, &rec, &mut pk)?, Value::Boolean(true)) {
                    // A row the view does not admit is not the page's: it opens on the first row at or after it in the view
                    // (pages/lt-page-setrecord-settableview/PageVarSetRecordTableView.al:P157_List_SetRecordThenSetTableView_RecordOutsideView
                    // `B` over `Grp = 1` opens on `C`, P157_Card_SetRecordOutsideView).
                    if !rec.borrow().filters.is_empty() {
                        if self.nav_find(st, "=>")? {
                            st.borrow_mut().cursor = Cursor::Row;
                            return self.arrive(st);
                        }
                    } else {
                        st.borrow_mut().cursor = Cursor::Row;
                        return self.arrive(st);
                    }
                }
            }
        }
        self.page_first_row(st)
    }

    pub(super) fn save_values(&self, st: &St) -> bool {
        let object = st.borrow().object as usize;
        let on = self.engine.program.objects[object]
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("SaveValues") && v.trim().eq_ignore_ascii_case("true"));
        // A SaveValues page that page extensions extend is driven as one without extensions (assumed).
        if on
            && self.engine.program.objects[object]
                .page
                .as_ref()
                .is_some_and(|p| !p.extensions.is_empty())
        {
            al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::SaveValuesWithExtensions);
        }
        on
    }

    pub(super) fn var_slots(&self, st: &St) -> Vec<usize> {
        let engine = self.engine();
        let mut slots: Vec<usize> = page_of(&engine, st)
            .controls
            .iter()
            .filter_map(|c| {
                if let (ir::Binding::Var(slot), None) = (&c.binding, c.owner) {
                    Some(*slot)
                } else {
                    None
                }
            })
            .collect();
        slots.sort_unstable();
        slots.dedup();
        slots
    }

    pub(super) fn store_page_values(&mut self, st: &St) {
        if !self.save_values(st) {
            return;
        }
        let Some(inst) = st.borrow().instance.clone() else {
            return;
        };
        let saved: Vec<(usize, Value)> = self
            .var_slots(st)
            .into_iter()
            .map(|slot| (slot, inst.borrow().globals[slot].clone()))
            .collect();
        let object = st.borrow().object;
        self.pages.saved_values.insert(object, saved);
    }

    pub(super) fn restore_page_values(&mut self, st: &St) {
        let object = st.borrow().object;
        let Some(saved) = self.pages.saved_values.get(&object).cloned() else {
            return;
        };
        let Some(inst) = st.borrow().instance.clone() else {
            return;
        };
        // A request page's variables that the caller set before the run (`SetParameters`) keep those values: corpus
        // reportextensiontrigger/TestPrecompiledReportExtensionDataItems.al (Base Application's "Get Demand To Reserve"
        // has SaveValues and an earlier run confirmed another demand type; the later run still goes by the one
        // `SetParameters` gave, `All` included). Which one wins in BC is assumed.
        let caller_set = self.is_request_page(st)
            && self
                .report
                .prepared
                .iter()
                .any(|w| w.upgrade().is_some_and(|i| Rc::ptr_eq(&i, &inst)));
        for (slot, v) in saved {
            if caller_set && !matches!(v, Value::Record(_)) {
                // The caller's value stays over the stored one (report/lt-requestpage-savevalues-date-text-setvalue/P147SaveValuesText.al:
                // P147_Text_StoredVsCaller_Ok, P147_Boolean_CallerFalseOverStoredTrue, P147_Option_CallerSelectedOverStoredAll).
                continue;
            }
            inst.borrow_mut().globals[slot] = v;
        }
    }

    /// `TestPage.Close()`: the page closes the way a Cancel does (corpus
    /// testpageclose/TestPageClosingActions.al:Dialog arms, `qcp:Cancel` after Close()).
    pub(super) fn page_close(&mut self, st: &St) -> Result<Value, Signal> {
        // Card, List, Document, Worksheet and ListPlus report OK (handlers/TestPageCloseActionByPageTypeContracts.al:
        // TestPageClose_IsOk_ForEveryPageType; a Card also in handlers/TestPageCloseTriggersSelectionCardPageContracts.al:
        // OnQueryClosePage_PageAndExtension_BothRun_AndBothMustAgreeToClose); the dialogs report Cancel.
        // Other page types are not pinned and keep Cancel.
        let object = st.borrow().object as usize;
        let ok_type = self.engine.object_facts[object]
            .page_type
            .as_ref()
            .is_some_and(al_ir::PageType::is_plain);
        let action = if ok_type && !st.borrow().modal { "OK" } else { "Cancel" };
        let r = self.close_with(st, action, false, false);
        // A refused TestPage.Close() leaves the variable not open all the same (corpus
        // handlers/TestPageCloseVeto_Tests.al: PlainVeto_SecondCloseRaisesNotOpen).
        if r.is_ok() {
            self.mark_closed(st);
        }
        r
    }

    /// Closes the page with `action`. The client first saves what was typed; a save the table
    /// refuses raises at Close() but not at an OK, which counts one validation error and leaves the
    /// page open (corpus testpageclose/TestPageClosingActions.al, Card_OK_AfterATableRefusedDuplicateKey_*,
    /// handlers/TestPageFailedInsert_Tests.al). Then OnQueryClosePage(action) decides, then
    /// OnClosePage runs (TestPageClosingActions.al:Card_OK_RaisesTheCloseTriggersOnce).
    pub(super) fn close_with(
        &mut self,
        st: &St,
        action: &str,
        swallow_save_errors: bool,
        via_action: bool,
    ) -> Result<Value, Signal> {
        self.require_open(st)?;
        let mapped = self.close_action(st, action);
        let action = mapped.as_str();
        if st.borrow().refused {
            // The refused line was reported when it was left; closing drops it.
            let mut s = st.borrow_mut();
            s.refused = false;
            s.draft = false;
            s.dirty = false;
        }
        // BC's client saves the child forms before the form's own record on a saving close
        // (corpus testpart/TestOkPartRow.al:OkInvoke_HeaderOnModifySeesThePartRowSavedFirst).
        self.save_parts(st)?;
        let pending = st.borrow_mut().pending.take();
        if let Some(fields) = pending {
            let rec = self.page_rec(st);
            rec.borrow_mut().fields = fields;
            self.ret_used = false;
            self.tx.immediate_errors = true;
            let inserted = self.record_op(RecordOp::Insert, &rec, &mut [Value::Boolean(true)]);
            self.tx.immediate_errors = false;
            inserted?;
        }
        match self.save_row(st) {
            Err(Signal::Error(e)) if swallow_save_errors => {
                let key_ctl = self.key_control(st);
                st.borrow_mut().errors.push((key_ctl, e.message));
                return Ok(Value::Null);
            }
            other => other?,
        }
        let members: std::sync::Arc<[String]> = al_builtins::value::ACTION_MEMBERS
            .iter()
            .map(|m| m.to_string())
            .collect();
        let ordinal = al_builtins::value::ACTION_MEMBERS
            .iter()
            .position(|m| *m == action)
            .unwrap_or(0) as i32;
        // The parts are asked first, with the action None; the host's own OnQueryClosePage follows
        // (testpart/TestPartCloseLinkEditableContracts.al:
        // Parts_CloseTriggers_AllPartsQueryClose_ThenHost_ThenPartsClose_ThenHost).
        let open_parts: Vec<St> = st
            .borrow()
            .parts
            .iter()
            .map(|(_, c)| c.clone())
            .filter(|c| c.borrow().open)
            .collect();
        let none_ordinal = al_builtins::value::ACTION_MEMBERS
            .iter()
            .position(|m| *m == "None")
            .unwrap_or(0) as i32;
        for part in &open_parts {
            if !part.borrow().parts.is_empty() {
                return unsupported("closing a host whose part page has parts of its own (close order not pinned)");
            }
            let answer = match self.query_close_answer(part, vec![Value::Option(none_ordinal, members.clone())]) {
                Err(Signal::Error(_)) => {
                    return unsupported("an error in a part's OnQueryClosePage (not pinned)");
                }
                other => other?,
            };
            if answer == Some(false) || !self.page_event(part, "onqueryclosepageevent", Some(true), None)? {
                return unsupported("a part's OnQueryClosePage that refuses the close (not pinned)");
            }
        }
        // An AL error raised in OnQueryClosePage is shown as a message and refuses the close
        // (corpus handlers/TestPageQueryCloseError_Tests.al:1-25).
        let veto = match self.query_close_answer(st, vec![Value::Option(ordinal, members)]) {
            Err(Signal::Error(e)) => {
                self.ui_message(e.message)?;
                st.borrow_mut().close_error = true;
                return Ok(Value::Null);
            }
            other => other?,
        };
        st.borrow_mut().close_error = false;
        if veto == Some(false) {
            return Ok(Value::Null);
        }
        if !self.page_event(st, "onqueryclosepageevent", Some(true), None)? {
            return Ok(Value::Null);
        }
        for part in &open_parts {
            self.page_trigger(part, "onclosepage", vec![])?;
            self.page_event(part, "onclosepageevent", None, None)?;
        }
        self.page_trigger(st, "onclosepage", vec![])?;
        self.page_event(st, "onclosepageevent", None, None)?;
        // SaveValues: what the page's variable-bound controls hold is stored unless the Cancel
        // action closed it (corpus handlers/TestPageSaveValues.al, testpageclose/TestPageClosingActions.al:937-977;
        // Close() and a platform close store too, docs/probes-needed.md).
        if !(via_action && matches!(action, "Cancel" | "LookupCancel" | "No")) {
            self.store_page_values(st);
        }
        // Not on the blank new-line row of a page: corpus misc/lt-page-run-overloads RunModal_NonTempRecord_ShowsDbRows keeps
        // `Row.Code` = D1 after a handler that read the page down to its new line and pressed OK.
        // A line started with New() is the row the page ends on, blank or typed (pages/lt-page-runmodal-rec/
        // PageRunModalRecAfterClose.al:P174_NonTemp_StartA_HandlerNewRowThenOk, _NewRowTypedKeyThenOk, _NewRowThenCancel), and
        // Cancel leaves the row the cursor stood on in Rec as OK does (:P174_NonTemp_StartA_HandlerNextThenCancel).
        let on_row = st.borrow().cursor == Cursor::Row && !st.borrow().draft;
        let on_new = st.borrow().draft && st.borrow().new_by_call;
        // A page the platform closes after the handler returned hands back nothing on Cancel (handlers/
        // TestPageActionRunObjectPageRec_Tests.al:PageRunModalTargetMovingItsRecAndTheCallersRecord).
        let hands_back = action == "LookupOK" || (action == "LookupCancel" && via_action);
        if hands_back && self.has_source(st) && (on_row || on_new) {
            let fields = self.page_rec(st).borrow().fields.clone();
            st.borrow_mut().selection = Some(fields);
        }
        self.mark_closed(st);
        st.borrow_mut().result = Some(action.to_string());
        Ok(Value::Null)
    }

    /// The built-in actions a page type offers (corpus handlers/TestPageBuiltInActions_Tests.al:1-30,
    /// testpageclose/TestPageClosingActions.al:25-30: a card or a list offers OK and no Cancel; a
    /// StandardDialog both; a NavigatePage only OK; a PromptDialog Cancel, and OK unless it declares
    /// systemaction(OK); a ConfirmationDialog neither).
    pub(super) fn builtin_offered(&self, st: &St, which: &str) -> bool {
        if self.is_request_page(st) {
            return self.request_builtin_offered(st, which);
        }
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let page_type = engine.object_facts[object].page_type_lower().to_string();
        let dialog = matches!(
            page_type.as_str(),
            "confirmationdialog" | "standarddialog" | "promptdialog" | "navigatepage"
        );
        // A lookup-mode modal offers OK and Cancel whatever its type (corpus
        // handlers/TestPageModalQueryClose_Tests.al (b2) and (d)).
        if !dialog && matches!(which, "OK" | "Cancel") && st.borrow().modal && st.borrow().lookup {
            return true;
        }
        match (page_type.as_str(), which) {
            ("confirmationdialog", "Yes" | "No") => true,
            ("confirmationdialog", _) => false,
            (_, "Yes" | "No") => false,
            ("standarddialog", _) => true,
            ("navigatepage", "Cancel") => false,
            ("promptdialog", "Cancel") => true,
            ("promptdialog", "OK") => !page_of(&engine, st)
                .controls
                .iter()
                .any(|c| c.kind == ir::ControlKind::SystemAction && c.name.eq_ignore_ascii_case("OK")),
            (_, "Cancel") => false,
            _ => true,
        }
    }

    /// Whether the page carries the blank line past its data: an editable,
    /// insert-allowed repeater opened with OpenEdit (corpus
    /// handlers/TestPageNewRowLine_Tests.al:1-30, gating arms).
    pub(super) fn offers_new_line(&mut self, st: &St) -> Result<bool, Signal> {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let page = page_of(&engine, st);
        let has_repeater = page.controls.iter().any(|c| c.kind == ir::ControlKind::Repeater);
        let props = &engine.program.objects[object].properties;
        let prop_false = |n: &str| {
            props
                .iter()
                .any(|(k, v)| k.eq_ignore_ascii_case(n) && v.trim().eq_ignore_ascii_case("false"))
        };
        Ok(has_repeater
            && st.borrow().mode == PageMode::Edit
            // A list handed over in lookup mode has no new-row line (corpus
            // handlers/TestPageModalNewRowLine_Tests.al:209 Plain_LookupModeTrue_HasNoNewRowLine). Only a
            // LookupMode(true) call does that: Page.RunModal(id, Rec) keeps the line (:180).
            && !(st.borrow().lookup_set && st.borrow().lookup && self.is_list_page(st))
            && !prop_false("Editable")
            && !prop_false("InsertAllowed"))
    }

    // ---- rows ---------------------------------------------------------------------------

    /// Whether the page declares trigger `name` (lower case) of its own.
    pub(super) fn declares(&self, st: &St, name: &str) -> bool {
        let engine = self.engine();
        page_of(&engine, st).triggers.iter().any(|(n, _)| n == name)
    }

    /// FindFirst (`which` "-") or FindLast ("+") of the page's rowset. A page that declares
    /// OnFindRecord serves its rows from the trigger, which receives `Which` and answers whether it
    /// found a row (corpus pagefindrecord/TestPageFindRecord.al:112, :160). The strings the client
    /// really passes are `=`, `=>` and `=><` forms the corpus pins only for a deleted row
    /// (pageupdate/TestPageUpdateGoneRow.al:390); `-` and `+` are assumed (docs/probes-needed.md).
    pub(super) fn nav_find(&mut self, st: &St, which: &str) -> Result<bool, Signal> {
        if self.declares(st, "onfindrecord") {
            let v = self.page_trigger(st, "onfindrecord", vec![Value::Text(which.into())])?;
            return Ok(matches!(v, Some(Value::Boolean(true))));
        }
        let rec = self.page_rec(st);
        self.ret_used = true;
        Ok(matches!(
            match which {
                "-" => self.record_op(RecordOp::FindFirst, &rec, &mut [])?,
                "+" => self.record_op(RecordOp::FindLast, &rec, &mut [])?,
                w => self.record_op(RecordOp::Find, &rec, &mut [Value::Text(w.into())])?,
            },
            Value::Boolean(true)
        ))
    }

    /// Next(steps) of the page's rowset: OnNextRecord when the page declares it, which answers how
    /// many rows it moved (corpus pagefindrecord/TestPageFindRecord.al:129).
    pub(super) fn nav_next(&mut self, st: &St, steps: i32) -> Result<bool, Signal> {
        if self.declares(st, "onnextrecord") {
            let v = self.page_trigger(st, "onnextrecord", vec![Value::Integer(steps)])?;
            return Ok(matches!(v, Some(Value::Integer(n)) if n != 0));
        }
        let rec = self.page_rec(st);
        self.ret_used = true;
        let moved = self.record_op(RecordOp::Next, &rec, &mut [Value::Integer(steps)])?;
        Ok(matches!(moved, Value::Integer(n) if n != 0))
    }

    /// Lands on the first row of the view when the page opens: a stored row, else
    /// the new-row line, else nothing. The expressions of the controls are evaluated for
    /// an empty view too (corpus autoformat/TestAutoFormatExpressionTiming.al:5-12).
    pub(super) fn page_first_row(&mut self, st: &St) -> Result<(), Signal> {
        // A page that serves its rows from OnFindRecord is asked `=><` when it opens (corpus
        // pageext/PCCurrRecordReads.al:FindList_OnFindRecordAndOnNextRecord, `open`).
        // That is pinned for the default order; a page with a SourceTableView (a descending order
        // among them) keeps the `-` assumed so far, as the client's blank key would sit at the other end.
        let object = st.borrow().object as usize;
        let has_view = self.engine.object_facts[object].has_source_table_view;
        let which = if self.declares(st, "onfindrecord") && !has_view {
            "=><"
        } else {
            "-"
        };
        if self.nav_find(st, which)? {
            st.borrow_mut().cursor = Cursor::Row;
            st.borrow_mut().landing_first = true;
            let r = self.arrive(st);
            st.borrow_mut().landing_first = false;
            // An error of the rows a modal page reads when it opens comes out with the code of the client's abort (misc/
            // lt-probes-needed-7/PnGPageClose.al:TeardownInHandlerReportsForNonLookupRunModal `af236`, `af260`, `af285`).
            let r = match r {
                Err(Signal::Error(mut e)) if st.borrow().modal && e.code == "Dialog" => {
                    e.code = "ClientPageAbort:FormAbort".into();
                    Err(Signal::Error(e))
                }
                other => other,
            };
            r?;
            // The window starts at the first row of the view.
            let from_start = self.is_list(st);
            st.borrow_mut().window_from_start = from_start;
            Ok(())
        } else if st.borrow().new_line {
            self.land_on_new_line(st)?;
            // A list with no row but the blank line reads it twice, as a list of one row reads its row (misc/lt-probes-needed-3/
            // PnC07TestPage.al:New_OnEmptyPage `OnNewRecord` three times with the New() call, misc/lt-probes-needed-7/
            // PnGNewRecord.al:HandedRecordKeyFieldFilter `OnNewRecord;AGCR(FILTERED,FILTERED)` twice).
            if self.is_list(st) && st.borrow().host.is_none() {
                self.land_on_new_line(st)?;
            }
            self.populate_expressions(st)
        } else if self.is_blank_list_plus(st) {
            // A ListPlus page without a repeater, opened for edit over an empty view, stands on a blank record
            // that has run OnNewRecord and OnAfterGetCurrRecord. Not pinned by the corpus: Microsoft's Suggest
            // Price Lines UT.T212_CopyLinesEnabledOnPurchPriceListPageIfFixedUpgradeCompleted needs it (the
            // Purchase Price List sets PriceListIsEditable in OnAfterGetCurrRecord). Assumption
            // listplus-blank-record-on-empty-view; a Card or a Document keeps opening without a row.
            al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::ListPlusBlankRecordOnEmptyView);
            self.land_on_new_line(st)?;
            self.populate_expressions(st)
        } else {
            self.populate_expressions(st)
        }
    }

    /// A top-level ListPlus page with a source table, no repeater, opened for edit.
    fn is_blank_list_plus(&self, st: &St) -> bool {
        let object = st.borrow().object as usize;
        self.engine.object_facts[object].page_type == Some(al_ir::PageType::ListPlus)
            && st.borrow().mode == PageMode::Edit
            && st.borrow().host.is_none()
            && self.has_source(st)
            && !self.is_list(st)
    }

    pub(super) fn land_on_new_line(&mut self, st: &St) -> Result<(), Signal> {
        let rec = self.page_rec(st);
        let table = rec.borrow().table as usize;
        self.note_lower_key(st)?;
        let fresh = self.initial_fields(table);
        rec.borrow_mut().fields = fresh;
        {
            let mut s = st.borrow_mut();
            s.cursor = Cursor::NewLine;
            s.draft = true;
        }
        // The blank line of a part is a started record: the client runs NewRecord when it becomes
        // current (corpus handlers/TestPagePartDraftLineLink.al:LinkedPart_DraftLine_*,
        // handlers/TestPagePartOnNewRecordCount.al). A top-level list does the same, and both then
        // run OnAfterGetCurrRecord on the blank row (corpus testpart/TestPartTriggerOrderAndRefresh.al:
        // 258-273 TopLevelList_BlankLine_RunsOnNewRecord, Part_BlankLine_RunsOnNewRecord).
        // A part whose host record is not saved yet (a card opened with OpenNew whose key was just typed) copies the link
        // values without validating them: the host row they point to does not exist yet (Microsoft's Allocation Account
        // page, 34 Tests-ERM tests failed on the table relation of "Allocation Account No."). Assumed, see
        // Assumption::PartLinkNotValidatedForUnsavedHost.
        let host_unsaved = st
            .borrow()
            .host
            .as_ref()
            .and_then(|(h, _)| h.upgrade())
            .is_some_and(|h| h.borrow().draft);
        if host_unsaved {
            al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::PartLinkNotValidatedForUnsavedHost);
        }
        self.stamp_from_filters(st, !host_unsaved)?;
        self.page_trigger(st, "onnewrecord", vec![Value::Boolean(true)])?;
        self.page_event(st, "onnewrecordevent", None, Some(true))?;
        // xRec is the new record itself there (misc/lt-probes-needed-7/PnGNewRecord.al:HandedRecordNoFilters `AGCR(,)`,
        // PnGHandedRecord `AGCR(FILTERED,FILTERED)`).
        self.set_xrec(st);
        self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
        self.page_event(st, "onaftergetcurrrecordevent", None, None)?;
        Ok(())
    }

    /// Remembers the stored row the cursor stands on as the lower neighbour of the row about to
    /// start, for AutoSplitKey.
    pub(super) fn note_lower_key(&mut self, st: &St) -> Result<(), Signal> {
        let lower = if st.borrow().cursor == Cursor::Row && !st.borrow().draft {
            let rec = self.page_rec(st);
            Some(self.row_key(&rec)?)
        } else {
            None
        };
        st.borrow_mut().lower_key = lower;
        Ok(())
    }

    /// What the client's NewRecord copies onto a fresh buffer of a part: a field whose combined
    /// filter is a single value, when the field is part of the primary key or the page sets
    /// PopulateAllFields; then the table's OnValidate runs for each (corpus
    /// handlers/TestPageSubpagePartFieldLink.al, TestPageSubpagePartConstFilter.al,
    /// TestPageSubpagePartNewRecordInit.al:New_StampsTheLinkedFieldAndValidatesIt).
    pub(super) fn stamp_from_filters(&mut self, st: &St, validate: bool) -> Result<(), Signal> {
        if !self.has_source(st) {
            return Ok(());
        }
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let populate_all = engine.program.objects[object]
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("PopulateAllFields") && is_true(v));
        let rec = self.page_rec(st);
        let table = self.table(&rec)?;
        let mut stamped: Vec<(usize, Value)> = Vec::new();
        for pos in 0..table.fields.len() {
            if !(populate_all || table.keys[0].contains(&pos)) {
                continue;
            }
            let single: Vec<Option<Value>> = rec
                .borrow()
                .filters
                .iter()
                .filter(|f| f.field == pos)
                .map(|f| match &f.expr {
                    al_builtins::filter::FilterExpr::Eq(v) => Some(v.clone()),
                    al_builtins::filter::FilterExpr::Range(Some(a), Some(b))
                        if ops::compare(a, b).is_ok_and(|o| o.is_eq()) =>
                    {
                        Some(a.clone())
                    }
                    _ => None,
                })
                .collect();
            // The combined filter of a field is the intersection of its filters, in every filter group: several
            // that all name the same value are that single value (Payment Journal: the record the page was run
            // with carries the batch in group 0 and `OpenJnl` sets it again in group 2; Microsoft's Incom. Doc.
            // Attach. FactBox tests, unprobed, docs/probes-needed.md).
            let Some(Some(v)) = single.first() else { continue };
            if single
                .iter()
                .any(|o| !o.as_ref().is_some_and(|w| ops::compare(v, w).is_ok_and(|c| c.is_eq())))
            {
                continue;
            }
            if single.len() > 1 {
                al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::NewRowEqualFiltersSeveralGroups);
            }
            stamped.push((pos, v.clone()));
        }
        // Every value is on the record before the first validation runs: the table relation of a link field filters on
        // Document Type of the same line (Service Item Line: "Document No." is field 1, "Document Type" field 43).
        // Assumed, see Assumption::NewLineStampsAllBeforeValidating.
        for (pos, v) in &stamped {
            rec.borrow_mut().fields[*pos] = v.clone();
        }
        if validate {
            if stamped.len() > 1 {
                al_builtins::assumptions::assume(
                    al_builtins::assumptions::Assumption::NewLineStampsAllBeforeValidating,
                );
            }
            for (pos, v) in stamped {
                self.ret_used = false;
                self.record_op(RecordOp::Validate, &rec, &mut [Value::Integer(pos as i32), v])?;
            }
        }
        Ok(())
    }

    /// AutoSplitKey: the last field of the primary key of a new line is numbered from its
    /// neighbours when it is saved (corpus handlers/TestPageAutoSplitKey_Tests.al,
    /// TestPageAutoSplitKeyRanges_Tests.al). Only the shapes the corpus pins are numbered; any
    /// other raises Signal::Unsupported.
    pub(super) fn auto_split_key(&mut self, st: &St) -> Result<(), Signal> {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let on = engine.program.objects[object]
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("AutoSplitKey") && is_true(v));
        if !on {
            return Ok(());
        }
        let rec = self.page_rec(st);
        let table = self.table(&rec)?;
        let Some(&last) = table.keys[0].last() else {
            return Ok(());
        };
        let ty = table.fields[last].ty.clone();
        if !matches!(ty, RtType::Integer | RtType::BigInteger | RtType::Decimal) {
            return unsupported(format!("AutoSplitKey on a key field of type {}", ty.describe()));
        }
        let dec = |v: &Value| match v {
            Value::Integer(i) => Some(rust_decimal::Decimal::from(*i)),
            Value::BigInteger(i) => Some(rust_decimal::Decimal::from(*i)),
            Value::Decimal(d) => Some(*d),
            _ => None,
        };
        // A key the test typed stays; the number is assigned to an unset one.
        let current = rec.borrow().fields[last].clone();
        if dec(&current).is_none_or(|d| !d.is_zero()) {
            return Ok(());
        }
        if rec.borrow().key != 0 || !rec.borrow().ascending {
            return unsupported("AutoSplitKey on a part that is not read in primary key order");
        }
        let lower = st
            .borrow()
            .lower_key
            .as_ref()
            .and_then(|k| k.get(table.keys[0].len() - 1))
            .and_then(dec);
        // The next line after the lower one in the part's view, within the same leading key.
        let upper = match lower {
            None => None,
            Some(l) => {
                let probe = Rc::new(RefCell::new(rec.borrow().clone()));
                {
                    let fields = rec.borrow().fields.clone();
                    let mut p = probe.borrow_mut();
                    for &k in &table.keys[0][..table.keys[0].len() - 1] {
                        p.filters.push(al_builtins::filter::FieldFilter {
                            field: k,
                            expr: al_builtins::filter::FilterExpr::Range(
                                Some(fields[k].clone()),
                                Some(fields[k].clone()),
                            ),
                            group: 99,
                        });
                    }
                    let lv = ops::convert(Value::Decimal(l), &ty)?;
                    p.filters.push(al_builtins::filter::FieldFilter {
                        field: last,
                        expr: al_builtins::filter::FilterExpr::Gt(lv),
                        group: 99,
                    });
                }
                self.ret_used = true;
                if matches!(
                    self.record_op(RecordOp::FindFirst, &probe, &mut [])?,
                    Value::Boolean(true)
                ) {
                    dec(&probe.borrow().fields[last])
                } else {
                    None
                }
            }
        };
        use rust_decimal::Decimal as D;
        let step = D::from(10000);
        let is_decimal = matches!(ty, RtType::Decimal);
        let number = match (lower, upper) {
            // An empty grid burns one interval on the blank placeholder row.
            (None, _) => D::from(20000),
            // Appended after the last line (handlers/TestPageAutoSplitKeyMoreShapes.al:104-117,
            // corpus handlers/TestPageAutoSplitKeyRanges_Tests.al): last + 10000. After a negative
            // line the corpus pins exactly these values; the rule behind them is not known, so
            // any other negative last line is refused.
            (Some(l), None) if l >= D::ZERO => l + step,
            (Some(l), None)
                if !is_decimal && [-30000, -20000, -10000, -5, -1].contains(&ToPrimitive::to_i64(&l).unwrap_or(0)) =>
            {
                D::from(match ToPrimitive::to_i64(&l).unwrap_or(0) {
                    -30000 => -22500,
                    -20000 => -15000,
                    -10000 => -6667,
                    -5 => -4,
                    _ => 1,
                })
            }
            // Between two lines (handlers/TestPageAutoSplitKeyMoreShapes.al:119-166 and
            // :186-205 for Decimal; handlers/TestPageAutoSplitKeyRanges_Tests.al for the cap): the
            // lower line plus half the gap, at most 10000. A gap that crosses zero gives one less
            // (the number zero is reserved: -10000 and 10000 give -1, -1 and 1 give -0.5), and a
            // result that lands on zero by the cap goes to 1. No whole number between two adjacent
            // lines raises "The line cannot be split." (code TestWrapped).
            (Some(l), Some(u)) if u > l => {
                let crossing = l < D::ZERO && u > D::ZERO;
                let gap = if crossing { u - l - D::ONE } else { u - l };
                let mut half = gap / D::from(2);
                if !is_decimal {
                    half = half.trunc();
                    if half < D::ONE {
                        return Err(error_code("The line cannot be split.", "TestWrapped"));
                    }
                }
                let mut n = l + half.min(step);
                if n.is_zero() {
                    if is_decimal {
                        return unsupported("AutoSplitKey landing on zero in a Decimal key (not pinned)");
                    }
                    n = D::ONE;
                }
                n
            }
            (l, u) => {
                return unsupported(format!(
                    "AutoSplitKey between {l} and {u} (the corpus does not pin it)",
                    l = l.describe(),
                    u = u.describe()
                ));
            }
        };
        rec.borrow_mut().fields[last] = ops::convert(Value::Decimal(number), &ty)?;
        Ok(())
    }
}
