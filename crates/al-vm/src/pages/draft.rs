use super::*;
use al_builtins::page_op::PageMethod;
use al_builtins::record_op::RecordOp;

impl Session {
    /// `TestPage.New()` (corpus handlers/TestPageInsertAllowed_Tests.al:70 New_OnInsertAllowedFalsePage_IsDenied,
    /// handlers/TestPageOpenNewAfterGetCurrRecord.al New_OnAnOpenCard_RunsOnAfterGetCurrRecordForTheNewRecord).
    pub(super) fn new_record(&mut self, st: &St) -> Result<Value, Signal> {
        self.refuse_read_only_part(st, "New()")?;
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let insert_denied = engine.program.objects[object]
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("InsertAllowed") && v.trim().eq_ignore_ascii_case("false"));
        if insert_denied {
            return Err(error("New method failed because Insert is not allowed."));
        }
        match self.leave_row(st, PageMethod::New)? {
            Left::Stay => Ok(Value::Null),
            Left::Go => self.start_draft(st, true).map(|_| Value::Null),
        }
    }

    /// Saves the row the cursor is leaving. A started line whose insert fails does not raise:
    /// New(), Next() and Last() stay on it with one validation error on its key control; Previous()
    /// and First() move on and the insert fails again at Close()
    /// (corpus handlers/TestPageFailedInsert_Tests.al:1-30).
    pub(super) fn leave_row(&mut self, st: &St, how: PageMethod) -> Result<Left, Signal> {
        let r = self.save_row(st);
        match r {
            Err(Signal::Error(e)) if st.borrow().draft && st.borrow().dirty => match how {
                PageMethod::New | PageMethod::Next | PageMethod::Last => {
                    let key_ctl = self.key_control(st);
                    let mut s = st.borrow_mut();
                    if !s.refused {
                        s.errors.push((key_ctl, e.message));
                        s.refused = true;
                    }
                    Ok(Left::Stay)
                }
                PageMethod::Previous | PageMethod::First => {
                    let fields = self.page_rec(st).borrow().fields.clone();
                    let mut s = st.borrow_mut();
                    s.pending = Some(fields);
                    s.draft = false;
                    s.dirty = false;
                    Ok(Left::Go)
                }
                _ => Err(Signal::Error(e)),
            },
            Err(e) => Err(e),
            Ok(()) => Ok(Left::Go),
        }
    }

    /// The control bound to the first field of the primary key.
    pub(super) fn key_control(&self, st: &St) -> usize {
        if !self.has_source(st) {
            return 0;
        }
        let engine = self.engine();
        let page = page_of(&engine, st);
        let table = self.page_rec(st).borrow().table as usize;
        let key0 = engine.program.objects[table].table.as_ref().map(|t| t.keys[0][0]);
        page.controls
            .iter()
            .position(|c| matches!(c.binding, ir::Binding::Field(p) if Some(p) == key0))
            .unwrap_or(0)
    }

    /// xRec := Rec, the row as the client read it.
    pub(super) fn set_xrec(&mut self, st: &St) {
        if !self.has_source(st) {
            return;
        }
        let fields = self.page_rec(st).borrow().fields.clone();
        self.set_xrec_fields(st, fields);
    }

    /// xRec holds `fields`: the row as it was read.
    pub(super) fn set_xrec_fields(&mut self, st: &St, fields: Vec<Value>) {
        if !self.has_source(st) {
            return;
        }
        let object = st.borrow().object as usize;
        let Some(slot) = self.engine.program.objects[object].xrec_slot else {
            return;
        };
        let inst = st.borrow().instance.clone().expect("open page");
        if let Value::Record(x) = &inst.borrow().globals[slot] {
            x.borrow_mut().fields = fields;
        }
    }

    pub(super) fn page_xrec_fields(&self, st: &St) -> Vec<Value> {
        if !self.has_source(st) {
            return Vec::new();
        }
        let object = st.borrow().object as usize;
        let slot = self.engine.program.objects[object]
            .xrec_slot
            .expect("page with a source table");
        let inst = st.borrow().instance.clone().expect("open page");
        let v = inst.borrow().globals[slot].clone();
        match v {
            Value::Record(x) => x.borrow().fields.clone(),
            _ => Vec::new(),
        }
    }

    pub(super) fn same_values(a: &[Value], b: &[Value]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| ops::compare(x, y).map(|o| o.is_eq()).unwrap_or(false))
    }

    /// The stored row with `rec`'s primary key, whatever the view says.
    pub(super) fn stored_row(&mut self, rec: &Rec) -> Result<Option<Vec<Value>>, Signal> {
        let table = self.table(rec)?;
        let probe = Rc::new(RefCell::new(rec.borrow().clone()));
        probe.borrow_mut().filters.clear();
        let mut pk = table.primary_key(&rec.borrow().fields);
        self.ret_used = true;
        Ok(match self.record_op(RecordOp::Get, &probe, &mut pk)? {
            Value::Boolean(true) => Some(probe.borrow().fields.clone()),
            _ => None,
        })
    }

    /// A new record on the page: xRec stays on the row the cursor was on, Rec starts from the table's
    /// defaults, OnNewRecord runs with BelowxRec true (corpus
    /// handlers/TestPageCreateModeNewRecordArgs.al:243-270), then the row is populated
    /// (handlers/TestPageOpenNewAfterGetCurrRecord.al:1-12: OnAfterGetCurrRecord runs after OnNewRecord).
    pub(super) fn start_draft(&mut self, st: &St, keep_focus: bool) -> Result<(), Signal> {
        if !self.has_source(st) {
            return unsupported("a new record on a page without a source table");
        }
        let rec = self.page_rec(st);
        self.set_xrec(st);
        // A new line of a list starts below the last row the client holds: OnNewRecord sees that row as xRec, while the
        // insert sees the row the cursor was on (misc/W2BelowXRec.al:NewInTheMiddle_CursorOnFirstRow_KeyBetween,
        // NewInTheMiddle_CursorOnMiddleRow_KeyBefore, NewTwice_InTheMiddle: xRec=30 at OnNewRecord, 10 at OnInsertRecord).
        // A list longer than its window first reads on to the end of the view (the new line goes after the last row).
        let reads_on = {
            let s = st.borrow();
            keep_focus
                && s.host.is_none()
                && s.cursor == Cursor::Row
                && !s.draft
                && s.window_from_start
                && s.window.len() == Self::WINDOW
                && !s.window_eof
        } && self.is_list(st)
            && self.declares(st, "onaftergetrecord")
            && self.declares(st, "onaftergetcurrrecord")
            && !self.declares(st, "onfindrecord")
            && !self.declares(st, "onnextrecord");
        if reads_on {
            self.read_block_past_the_window(st, false)?;
        }
        let last_held = {
            let s = st.borrow();
            (keep_focus
                && s.host.is_none()
                && self.is_list(st)
                && s.cursor == Cursor::Row
                && !s.draft
                && s.window_from_start
                && (s.window.len() < Self::WINDOW || s.window_whole))
                .then(|| s.window.last().map(|w| w.stored.clone()))
                .flatten()
        };
        self.note_lower_key(st)?;
        // A list line started below a stored row: remember the row (see `PageState::after_new`).
        let anchor = (keep_focus && st.borrow().cursor == Cursor::Row && !st.borrow().draft)
            .then(|| rec.borrow().fields.clone());
        st.borrow_mut().after_new = anchor;
        let table = rec.borrow().table as usize;
        let fresh = self.initial_fields(table);
        rec.borrow_mut().fields = fresh;
        self.stamp_from_filters(st, true)?;
        let engine = self.engine();
        let page = page_of(&engine, st);
        let first = page.controls.iter().position(|c| c.kind == ir::ControlKind::Field);
        {
            let mut s = st.borrow_mut();
            s.cursor = if page.controls.iter().any(|c| c.kind == ir::ControlKind::Repeater) {
                Cursor::NewLine
            } else {
                Cursor::Row
            };
            s.draft = true;
            s.new_by_call = true;
            s.dirty = false;
            s.refused = false;
            if !keep_focus {
                s.focus = first;
                s.focus_in_row = true;
            } else {
                // Focus stays on the control of the row the cursor left
                // (corpus handlers/TestPageBlankKeyInsert.al, List_NewRow_FocusStays...).
                s.focus_in_row = false;
            }
        }
        let existed = self.stored_row(&rec)?.is_some();
        st.borrow_mut().draft_stored = existed;
        if let Some(last) = last_held {
            self.set_xrec_fields(st, last);
        }
        self.page_trigger(st, "onnewrecord", vec![Value::Boolean(true)])?;
        self.page_event(st, "onnewrecordevent", None, Some(true))?;
        if self.is_list(st) && st.borrow().host.is_none() {
            // xRec is the new record itself when the line becomes current (misc/lt-probes-needed-7/PnGNewRecord.al:
            // HandedRecordNoFilters `AGCR(,)`).
            self.set_xrec(st);
        }
        // A new row nobody has saved raises OnAfterGetCurrRecord but no OnAfterGetRecord (corpus
        // pageupdate/TestPageUpdateNewRow.al:OpenNew_TraceIsOneOnAfterGetCurrRecord).
        self.populate_expressions(st)?;
        self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
        self.page_event(st, "onaftergetcurrrecordevent", None, None)?;
        // Page code that inserts the new row itself (a template step) hands the page the stored row.
        if !existed && self.stored_row(&rec)?.is_some() {
            let mut s = st.borrow_mut();
            s.draft = false;
            s.cursor = Cursor::Row;
            drop(s);
            self.set_xrec(st);
        }
        Ok(())
    }

    /// Writes what the client holds for the current row: inserts a typed new record, modifies an
    /// edited one when a value really differs (corpus handlers/TestPageSameValueSetValue_Tests.al:1-30).
    pub(super) fn save_row(&mut self, st: &St) -> Result<(), Signal> {
        let (cursor, draft, dirty) = {
            let s = st.borrow();
            (s.cursor, s.draft, s.dirty)
        };
        if cursor == Cursor::None || !dirty || !self.has_source(st) {
            return Ok(());
        }
        if draft {
            return self.insert_draft(st);
        }
        let rec = self.page_rec(st);
        if Self::same_values(&rec.borrow().fields, &self.page_xrec_fields(st)) {
            st.borrow_mut().dirty = false;
            return Ok(());
        }
        // corpus handlers/TestPageTriggerEvents.al (ModifyRecordEvent_RunsAfterThePagesOwnOnModifyRecordTrigger)
        let keep = self.page_trigger_answer(st, "onmodifyrecord", vec![])?;
        if keep == Some(false) {
            return Ok(());
        }
        if !self.page_event(st, "onmodifyrecordevent", Some(true), None)? {
            return Ok(());
        }
        self.ret_used = false;
        // The page saves the row on fresh table globals: what an action set in them is gone (corpus
        // record/PWTableGlobals.al:408-431: OnModify reads Counter 0 though the action set 7).
        self.reset_table_globals(&rec);
        self.pages.page_xrec = Some(self.page_xrec_fields(st));
        let modified = self.record_op(RecordOp::Modify, &rec, &mut [Value::Boolean(true)]);
        self.pages.page_xrec = None;
        // The page keeps the row it read; when code behind it has changed that row since, whether the client has read it again
        // first is not pinned (no corpus page test reaches DB:RecordChanged), so the page's own save refuses rather than fails.
        if let Err(Signal::Error(e)) = &modified {
            if e.code == "DB:RecordChanged" {
                return unsupported(
                    "a page saves a row that code changed after the page read it (whether the client reads the row again first is not pinned)",
                );
            }
        }
        modified?;
        st.borrow_mut().dirty = false;
        self.set_xrec(st);
        Ok(())
    }

    /// OnInsertRecord decides whether the typed record is inserted at all (corpus
    /// handlers/TestPageRecordTriggers.al:76 OnInsertRecord_ReturningFalse_SuppressesTheInsert).
    pub(super) fn insert_draft(&mut self, st: &St) -> Result<(), Signal> {
        let rec = self.page_rec(st);
        // BelowxRec is Yes for a row added with New() at the end, as it is for OnNewRecord (corpus
        // handlers/TestPageRuntimeContracts.al:197-211 TestPage_New_AtTheEnd_PassesBelowxRecTrueToOnInsertRecord).
        // A row added in the middle is not pinned; it is given the same Yes as OnNewRecord gets.
        let anchor = st.borrow().after_new.clone();
        if let Some(anchor) = anchor.filter(|_| self.is_list(st) && st.borrow().host.is_none()) {
            // The insert sees the row the line was started below as xRec (misc/W2BelowXRec.al:NewInTheMiddle_*).
            self.set_xrec_fields(st, anchor);
        }
        // A line of a part is inserted while a stored host row may hold an edit of its own. The line goes in first, so the header's
        // OnModify sees it (testpart/ OkInvoke_HeaderOnModifySeesThePartRowSavedFirst); the host then reads its row again, because the
        // line's table code may have changed it, puts its own edit on top and saves (pages/lt-changed-by-other/
        // ChangedByOtherPlain.al:P182b_Plain_SaveRecordAction: `stored-after-line` is `descr=typed info=by line insert counter=1`, the
        // card shows `by line insert` and `1`). Only a stored host row is pinned; a new host row or one without a source stays as before.
        let host = st
            .borrow()
            .host
            .as_ref()
            .and_then(|(h, _)| h.upgrade())
            .filter(|h| self.has_source(h) && !h.borrow().draft && h.borrow().cursor == Cursor::Row);
        let go = self.page_trigger_answer(st, "oninsertrecord", vec![Value::Boolean(true)])?;
        if go == Some(false) {
            // A trigger that inserted the row itself and answered false leaves the page on a stored row: its next edit is
            // a modify (as for a template step in `start_draft`). Not pinned for this answer: Base Application's Item
            // Tracking Lines inserts into its temporary source this way (assumption page-oninsertrecord-false-after-code-insert).
            if !st.borrow().draft_stored && self.stored_row(&rec)?.is_some() {
                al_builtins::assumptions::assume(
                    al_builtins::assumptions::Assumption::PageOnInsertRecordFalseAfterCodeInsert,
                );
                {
                    let mut s = st.borrow_mut();
                    s.draft = false;
                    s.dirty = false;
                    s.cursor = Cursor::Row;
                }
                self.set_xrec(st);
            }
            return Ok(());
        }
        if !self.page_event(st, "oninsertrecordevent", Some(true), Some(true))? {
            return Ok(());
        }
        self.auto_split_key(st)?;
        self.ret_used = false;
        self.tx.immediate_errors = true;
        let inserted = self.record_op(RecordOp::Insert, &rec, &mut [Value::Boolean(true)]);
        self.tx.immediate_errors = false;
        inserted?;
        {
            let mut s = st.borrow_mut();
            s.draft = false;
            s.dirty = false;
            s.cursor = Cursor::Row;
        }
        self.set_xrec(st);
        if let Some(host) = &host {
            let hrec = self.page_rec(host);
            if let Some(stored) = self.stored_row(&hrec)? {
                let typed = hrec.borrow().fields.clone();
                let before = self.page_xrec_fields(host);
                let mut merged = stored;
                let mut edited = false;
                for (i, v) in typed.iter().enumerate() {
                    if i < before.len() && !Self::same_values(std::slice::from_ref(v), std::slice::from_ref(&before[i]))
                    {
                        merged[i] = v.clone();
                        edited = true;
                    }
                }
                hrec.borrow_mut().fields = merged;
                if edited && host.borrow().dirty {
                    self.save_row(host)?;
                } else {
                    self.set_xrec(host);
                }
            }
        }
        // The parts of a new host row link to it once it is stored, with the number its OnInsert
        // gave it (corpus handlers/TestPageOpenNewPartLink.al).
        if !st.borrow().parts.is_empty() {
            self.sync_parts(st)?;
        }
        Ok(())
    }

    /// Focus moves to control `idx`. On a new record of a page that inserts as you go, moving from
    /// a control of that row to a non-key control inserts it (corpus
    /// handlers/TestPageBlankKeyInsert.al:1-25 and its List_NewRow_Focus... arm: after New() the
    /// focus still sits on the old row's control).
    pub(super) fn activate(&mut self, st: &St, idx: usize) -> Result<(), Signal> {
        let (prev, in_row) = {
            let s = st.borrow();
            (s.focus, s.focus_in_row)
        };
        if prev == Some(idx) {
            return Ok(());
        }
        {
            let mut s = st.borrow_mut();
            s.focus = Some(idx);
            s.focus_in_row = true;
        }
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let delayed = engine.program.objects[object]
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("DelayedInsert") && v.trim().eq_ignore_ascii_case("true"));
        let page = page_of(&engine, st);
        let non_key = match page.controls[idx].binding {
            ir::Binding::Field(pos) => {
                let table = self.page_rec(st).borrow().table as usize;
                !engine.program.objects[table]
                    .table
                    .as_ref()
                    .is_some_and(|t| t.keys[0].contains(&pos))
            }
            _ => false,
        };
        if st.borrow().draft && in_row && prev.is_some() && !delayed && non_key {
            self.insert_draft(st)?;
        }
        Ok(())
    }
}
