use super::*;
use al_builtins::page_op::PageMethod;
use al_builtins::record_op::RecordOp;

impl Session {
    /// An action with no `OnAction` trigger (its own or a page extension's) and no `RunObject` is not
    /// on the TestPage: naming it raises TestActionNotFound (handlers/TestPageLabelControlContracts.al:
    /// 131-145, BC 27.5). Whether other behaviour (a `RunPageLink` without a `RunObject`, a report
    /// action) also counts is not pinned.
    pub(super) fn action_without_behaviour(&self, st: &St, idx: usize) -> bool {
        let engine = self.engine();
        let ctl = &page_of(&engine, st).controls[idx];
        matches!(ctl.kind, ir::ControlKind::Action)
            && ctl.action_ref.is_none()
            && ctl.run_object.is_none()
            && !ctl.triggers.iter().any(|(n, _)| n.eq_ignore_ascii_case("onaction"))
            && !ctl
                .ext_triggers
                .iter()
                .any(|(n, _, _)| n.eq_ignore_ascii_case("onaction"))
    }

    pub(super) fn control_eliminated(&self, st: &St, idx: usize) -> bool {
        let engine = self.engine();
        let page = page_of(&engine, st);
        // Actions are not eliminated, whatever their Visible says (corpus
        // handlers/TestPageActionReachability_Tests.al:100-122).
        if matches!(
            page.controls[idx].kind,
            ir::ControlKind::Action | ir::ControlKind::ActionGroup
        ) {
            return false;
        }
        let mut at = Some(idx);
        while let Some(i) = at {
            if matches!(page.controls[i].prop("visible"), Some(ir::PropVal::Lit(v)) if v.trim().eq_ignore_ascii_case("false"))
            {
                return true;
            }
            at = page.controls[i].parent;
        }
        false
    }

    /// `TestField.DrillDown`, `AssistEdit` and `Lookup`: run the control's trigger of that name, the
    /// one it states itself or the one a page extension's `modify` block adds (corpus
    /// pageextensiontrigger/TestModifiedControlValidateTriggers_Tests.al: AModifyBlocksOnDrillDownRunsForTheModifiedControl,
    /// AModifyBlocksOnLookupRunsForTheModifiedControl, ABaseControlsOwnOnAssistEditRuns,
    /// AModifyBlocksOnAssistEditRunsForTheModifiedControl, OnAssistEditIsRaisedOnlyForTheControlItWasCalledOn).
    /// A DrillDown on a control with no trigger raises a fixed platform error (handlers/TestPageFieldDrillDown_Tests.al:
    /// FieldDrillDownWithNoTriggerIsRefused). What AssistEdit and Lookup do without a trigger (a table
    /// relation's lookup page) is not modelled.
    pub(super) fn control_navigate(&mut self, st: &St, idx: usize, method: PageMethod) -> Result<Value, Signal> {
        let engine = self.engine();
        let ctl = &page_of(&engine, st).controls[idx];
        let name = match method {
            PageMethod::DrillDown => "ondrilldown",
            PageMethod::AssistEdit => "onassistedit",
            _ => "onlookup",
        };
        let own = ctl.trigger(name).map(|r| (ctl.owner, r));
        let added: Vec<(usize, usize)> = ctl
            .ext_triggers
            .iter()
            .filter(|(n, _, _)| n == name)
            .map(|(_, e, r)| (*e, *r))
            .collect();
        let found: Vec<(Option<usize>, usize)> = own
            .into_iter()
            .chain(added.into_iter().map(|(e, r)| (Some(e), r)))
            .collect();
        let (owner, routine) = match found.as_slice() {
            [one] => *one,
            [] if method == PageMethod::DrillDown => {
                return Err(error("The NavDrilldownAction method is not supported."));
            }
            [] if method == PageMethod::Lookup => return self.field_lookup(st, idx),
            [] => return unsupported(format!("TestField.{method} on a control with no {name} trigger")),
            _ => {
                return unsupported(format!(
                    "TestField.{method} on a control with more than one {name} trigger"
                ));
            }
        };
        st.borrow_mut().refresh = false;
        let result = if method == PageMethod::Lookup {
            // OnLookup(var Text): Boolean. The text it writes back replaces the field's value, and
            // the Boolean says whether the user picked anything (corpus
            // handlers/TestPageFieldTableLookup.al:72 Lookup_ControlWithTrigger_WinsOverTheTableFieldsOnLookup,
            // handlers/TestPageModalHandler_Tests.al:132). The text the trigger starts with is the
            // control's current text (assumed, docs/probes-needed.md).
            let object = owner.unwrap_or(st.borrow().object as usize);
            let inst = self.owner_instance(st, owner);
            let current = self.control_value(st, idx)?;
            let shown = self.control_text(st, idx, &current)?;
            self.call(object, routine, inst, vec![Value::Text(shown.into())])
                .and_then(|(v, finals)| match (v, finals.first()) {
                    (Value::Boolean(true), Some(Value::Text(t))) => {
                        // Written back as the client would type it. The OnLookup of a page extension's modify block
                        // leaves a Code field in the case it wrote (`Text := 'FromExt'` reads back FromExt), the page's
                        // own trigger does not (FROMCTL) (lookups/W2Lookups.al:
                        // ModifyBlockOnLookup_ReturnsTrue, BC 27.5 and 29.0).
                        let r = self.control_set_value(st, idx, &mut [Value::Text(t.clone())]);
                        let binding = page_of(&self.engine(), st).controls[idx].binding.clone();
                        if let ir::Binding::Field(pos) = binding {
                            let rec = self.page_rec(st);
                            let is_code = matches!(rec.borrow().fields[pos], Value::Code(_));
                            if is_code && owner.is_some() && r.is_ok() {
                                rec.borrow_mut().fields[pos] = Value::Code(t.clone());
                            }
                        }
                        r
                    }
                    (Value::Boolean(true), other) => {
                        unsupported(format!("an OnLookup trigger whose Text is {}", other.describe()))
                    }
                    _ => Ok(Value::Null),
                })
        } else {
            self.owner_routine(st, owner, routine).map(|_| Value::Null)
        };
        if result.is_err() {
            st.borrow_mut().refresh = false;
        }
        let v = result?;
        self.run_refresh(st)?;
        Ok(v)
    }

    /// `Lookup()` on a control with no OnLookup trigger of its own: the source table field's
    /// OnLookup runs on the page's row (corpus handlers/TestPageFieldTableLookup.al:55
    /// Lookup_ControlWithoutTrigger_RunsTheTableFieldsOnLookup); else a field with a TableRelation
    /// opens the related table's lookup page modally, and a confirmed pick is written back
    /// (handlers/TestPageFieldRelationLookup_Tests.al:104, :122, :142); a field with neither does
    /// nothing (:168).
    pub(super) fn field_lookup(&mut self, st: &St, idx: usize) -> Result<Value, Signal> {
        let Some(field) = self.control_field(st, idx) else {
            return unsupported("TestField.Lookup on a control that is not bound to a table field");
        };
        if let Some((object, routine)) = field.on_lookup {
            let rec = self.page_rec(st);
            st.borrow_mut().refresh = false;
            self.call_on_record(object, routine, rec, None, Vec::new())?;
            // The control now shows what the trigger wrote, as an edit of the row (assumed,
            // docs/probes-needed.md).
            st.borrow_mut().dirty = true;
            self.run_refresh(st)?;
            return Ok(Value::Null);
        }
        if !field.relation {
            return Ok(Value::Null);
        }
        let engine = self.engine();
        let [arm] = field.relation_arms.as_slice() else {
            return unsupported("Lookup over a TableRelation with several arms or an unreadable one (not pinned)");
        };
        let (Some(table), true, Some(wheres)) = (arm.table, arm.conditions.is_empty(), &arm.wheres) else {
            return unsupported("Lookup over a conditional TableRelation or one into a table outside the workspace");
        };
        // The page is the related table's LookupPageId, else its DrillDownPageId; a table with
        // neither still shows a form, which nobody can answer (handlers/TestPageFieldRelationLookup_Tests.al:253).
        let table_obj = &engine.program.objects[table];
        let page_of_table = ["LookupPageId", "DrillDownPageId"].iter().find_map(|prop| {
            let name = table_obj
                .properties
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(prop))
                .map(|(_, v)| v.trim().trim_matches('"').to_string())?;
            engine.program.objects.iter().position(|o| {
                o.is_kind(al_ir::ObjKind::Page)
                    && (o.name.eq_ignore_ascii_case(&name) || o.id.is_some_and(|i| i.to_string() == name))
            })
        });
        // A page of a dependency app that nothing reached is not compiled (ADR-013); BC has it, and
        // it would open.
        if page_of_table.is_none() {
            for prop in ["LookupPageId", "DrillDownPageId"] {
                let named = table_obj
                    .properties
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(prop))
                    .map(|(_, v)| v.trim().trim_matches('"').to_string());
                if let Some(name) = named.filter(|n| names_uncompiled_page(&engine, n)) {
                    return unsupported(format!(
                        "the {prop} of table {} is page {name}, which belongs to a dependency app and was not compiled (no code the workspace reaches uses it)",
                        table_obj.name
                    ));
                }
            }
        }
        let Some(page_object) = page_of_table else {
            // An `asserterror` around this call is the only way a test sees the error. How far it
            // rolls back when the page's operation, not the test, raised it is not pinned: with
            // writes pending from before the call the run is refused (the corpus test seeds its
            // host row without a Commit, handlers/TestPageFieldRelationLookup_Tests.al:253).
            if self.store.has_uncommitted() {
                return unsupported(
                    "Lookup over a relation to a table with no lookup page, with writes pending (the extent of an asserterror rollback around a TestPage call is not pinned)",
                );
            }
            return Err(error("Unhandled UI: ModalPage"));
        };
        if !wheres.is_empty() {
            return unsupported(
                "Lookup over a TableRelation with a where() clause (the lookup page's filter is not pinned)",
            );
        }
        let current = self.control_value(st, idx)?;
        if !Self::same_values(
            std::slice::from_ref(&current),
            std::slice::from_ref(&Value::default_for(&field.ty)),
        ) {
            return unsupported(
                "Lookup over a TableRelation from a field that already holds a value (the page's position is not pinned)",
            );
        }
        let related = Rc::new(RefCell::new(PageState::closed(page_object as u32)));
        related.borrow_mut().lookup = true;
        self.page_instance(&related)?;
        let blank = Rc::new(RefCell::new(self.page_rec(&related).borrow().clone()));
        let outcome = self.run_page_ui(&related, Some(blank), true)?;
        let confirmed = matches!(&outcome, Value::Option(n, members)
            if members.get(*n as usize).is_some_and(|m| m == "LookupOK"));
        if !confirmed {
            return Ok(Value::Null);
        }
        let picked = self.page_rec(&related);
        let table_def = self.table(&picked)?;
        let pos = arm.field.unwrap_or(table_def.keys[0][0]);
        let value = picked.borrow().fields[pos].clone();
        st.borrow_mut().refresh = false;
        self.control_set_value(st, idx, &mut [value])?;
        Ok(Value::Null)
    }

    /// The page a list's `CardPageId` names: `None` when it names none.
    pub(super) fn list_card_page(&self, st: &St) -> Result<Option<usize>, Signal> {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let Some(card) = engine.program.objects[object]
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("CardPageId"))
            .map(|(_, v)| v.trim().trim_matches('"').to_string())
        else {
            return Ok(None);
        };
        engine
            .program
            .objects
            .iter()
            .position(|o| {
                o.is_kind(al_ir::ObjKind::Page)
                    && (o.name.eq_ignore_ascii_case(&card) || o.id.is_some_and(|i| i.to_string() == card))
            })
            .map(Some)
            .ok_or_else(|| Signal::Unsupported("CardPageId names a page outside the workspace".into()))
    }

    /// Whether the card a list's Edit action would open can be edited. Only `Editable = false`
    /// is pinned (corpus handlers/TestPageModeSwitchAction_Tests.al:
    /// ListWhoseCardIsReadOnlyLeavesTheEditActionVisibleButNotEnabled); any other restriction is refused.
    pub(super) fn card_allows_edit(&self, card: usize) -> Result<bool, Signal> {
        let engine = self.engine();
        let prop = |n: &str| {
            engine.program.objects[card]
                .properties
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(n))
                .map(|(_, v)| v.trim().to_ascii_lowercase())
        };
        if prop("ModifyAllowed").is_some_and(|v| v != "true") {
            return unsupported("the Edit action of a list whose card sets ModifyAllowed (not pinned)");
        }
        match prop("Editable").as_deref() {
            None | Some("true") => Ok(true),
            Some("false") => Ok(false),
            Some(_) => unsupported("the Edit action of a list whose card computes Editable (not pinned)"),
        }
    }

    /// The built-in Edit or View action of a list page: opens the page named by the list's
    /// `CardPageId` on the current row, editable or read-only, as a `RunObject` action would.
    pub(super) fn list_card_action(&mut self, st: &St, edit: bool) -> Result<Value, Signal> {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let Some(card) = engine.program.objects[object]
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("CardPageId"))
            .map(|(_, v)| v.trim().trim_matches('"').to_string())
        else {
            return unsupported("the built-in Edit/View action of a list page with no CardPageId (not pinned)");
        };
        let Some(target) = engine.program.objects.iter().position(|o| {
            o.is_kind(al_ir::ObjKind::Page)
                && (o.name.eq_ignore_ascii_case(&card) || o.id.is_some_and(|i| i.to_string() == card))
        }) else {
            return unsupported("CardPageId names a page outside the workspace");
        };
        if st.borrow().cursor != Cursor::Row {
            return unsupported("the built-in Edit/View action of a list that stands on no stored row (not pinned)");
        }
        let ro = ir::RunObject {
            kind: ir::RunKind::Page,
            object: Some(target),
            on_rec: true,
            mode: if edit { ir::RunMode::Edit } else { ir::RunMode::View },
            link: None,
            unsupported: None,
        };
        st.borrow_mut().refresh = false;
        self.run_object_page(st, &ro)?;
        Ok(Value::Null)
    }

    /// `TestAction.Invoke`: runs the action's OnAction when it is enabled; a disabled action
    /// does nothing and raises nothing (corpus handlers/TestPageActionEnabledSourceField_Tests.al,
    /// TPAE_ActionEnabledBoolField_FalseRow_IsDisabledAndItsTriggerIsSkipped).
    pub(super) fn action_invoke(&mut self, st: &St, idx: usize) -> Result<Value, Signal> {
        // An actionref invokes the action it stands for.
        let idx = match page_of(&self.engine(), st).controls[idx].action_ref.clone() {
            None => idx,
            Some(ir::ActionRef::Target(t)) => t,
            Some(ir::ActionRef::Unresolved(why)) => return unsupported(why),
        };
        if !self.control_enabled(st, idx)? {
            return Ok(Value::Null);
        }
        let engine = self.engine();
        // A client sends the row it stands on to the server before it runs an action (corpus
        // handlers/TestPageAutoSplitKey_Tests.al:TestPage_ActionInvoke_OnANewSubpageRow_*).
        let was_dirty = st.borrow().dirty;
        if st.borrow().host.is_some() {
            self.save_row(st)?;
        }
        st.borrow_mut().refresh = false;
        // An edit the client sends before the action (a control SetValue left the page dirty) is
        // saved, and the saved row is read again: OnAfterGetRecord of the base runs before the
        // action (corpus handlers/TestPageExtensionBaseAccessContracts.al:338-342, where the base
        // copies a variable an extension control set into a variable the action logs).
        // A page with OnAfterGetRecord reads that row as a clean page does, and nothing more
        // (pageext/PCCurrRecordReads.al:Card_DirtyPage_ReadsAfterEachAction and
        // List_DirtyPage_ReadsAfterEachAction: one OnAfterGetRecord before the action body, no
        // OnAfterGetCurrRecord); the reads after it follow below.
        let dirty_reread_only = was_dirty
            && self.declares(st, "onaftergetrecord")
            && (!self.declares(st, "onfindrecord") && !self.declares(st, "onnextrecord")
                || self.declares(st, "onfindrecord") && self.declares(st, "onaftergetcurrrecord"))
            && self.has_source(st)
            && st.borrow().open
            && st.borrow().cursor == Cursor::Row
            && !st.borrow().draft
            && page_of(&engine, st).controls[idx].run_object.is_none()
            && page_of(&engine, st).controls[idx].trigger("onaction").is_some();
        if dirty_reread_only && st.borrow().host.is_none() {
            // The edit reaches the server with the action, so the rows read after it hold it
            // (Name reads 'dirty' after the action in the same tests).
            self.save_row(st)?;
        }
        if was_dirty && !self.declares(st, "onfindrecord") && !self.declares(st, "onnextrecord") && !dirty_reread_only {
            st.borrow_mut().refresh = true;
            self.run_refresh(st)?;
            st.borrow_mut().refresh = false;
        }
        // The client reads the row it stands on before an action runs (corpus
        // pageext/PCCurrRecordReads.al:Card_ReadsAfterEachAction and List_ReadsAfterEachAction: one
        // OnAfterGetRecord before the action's own trigger, only when the row is not dirty, which has
        // its re-read above).
        // A list that serves its rows from OnFindRecord and OnNextRecord does the same (misc/W2FindRecordUpdate.al: during_action
        // `AGR(20);ActionBegin;ActionEnd;`).
        // (misc/W2FindRecordUpdate.al for a list; pages/lt-update-on-find/UpdateOnFindRecord.al for a card and a list that
        // declare OnAfterGetCurrRecord: `AGR:2;act:Update(false)` first.)
        let find_list =
            self.declares(st, "onfindrecord") && (self.is_list(st) || self.declares(st, "onaftergetcurrrecord"));
        let reads_rows = (!was_dirty || dirty_reread_only)
            && self.declares(st, "onaftergetrecord")
            && (find_list || !self.declares(st, "onnextrecord") && !self.declares(st, "onfindrecord"))
            && self.has_source(st)
            && st.borrow().open
            && st.borrow().cursor == Cursor::Row
            && !st.borrow().draft
            && page_of(&engine, st).controls[idx].run_object.is_none()
            && page_of(&engine, st).controls[idx].trigger("onaction").is_some();
        if reads_rows {
            let rec = self.page_rec(st);
            if self.stored_row(&rec)?.is_some() {
                self.populate_current(st)?;
            }
        }
        // RunObject wins over OnAction when an action declares both (handlers/TestPageRunObjectContracts.al:371-390).
        if page_of(&engine, st).controls[idx].run_object.is_some() {
            return self.run_object_action(st, idx);
        } else if let Some(r) = page_of(&engine, st).controls[idx].trigger("onaction") {
            let before = (self.has_source(st) && st.borrow().cursor == Cursor::Row && !st.borrow().draft)
                .then(|| self.page_rec(st).borrow().fields.clone());
            let r = self.control_routine(st, idx, r);
            if r.is_err() {
                st.borrow_mut().refresh = false;
            }
            r?;
            // What the action left in Rec is saved when the client leaves the row (corpus
            // record/PWTableGlobals.al:408-431, PageModify_AfterActionChangedRec_NoValidation).
            if let Some(before) =
                before.filter(|_| st.borrow().cursor == Cursor::Row && !st.borrow().draft && !st.borrow().refresh)
            {
                let rec = self.page_rec(st);
                // A change the action saved itself (`Rec.Modify()` in the action) is not saved again: the
                // page runs no OnModify for it (misc/W2TableGlobals.al:Modify_ActionCallsModifyItself).
                let saved_by_action = self
                    .stored_row(&rec)?
                    .is_some_and(|stored| Self::same_values(&rec.borrow().fields, &stored));
                if !Self::same_values(&rec.borrow().fields, &before) && !saved_by_action {
                    st.borrow_mut().dirty = true;
                    self.save_row(st)?;
                }
            }
            self.settle_after_action(st)?;
            // The client reads the current row again after every action, so OnAfterGetCurrRecord of
            // the page and of its extensions runs again (corpus
            // handlers/TestPageExtensionBaseMembersContracts.al:195-218: "ext AfterGetCurrRecord"
            // after the extension's action and after the base's). A page that serves its rows from
            // OnFindRecord is left as `CurrPage.Update` alone re-reads it (re-anchoring not pinned).
            let find_page = self.declares(st, "onfindrecord") || self.declares(st, "onnextrecord");
            let find_pinned = find_page
                && reads_rows
                && self.declares(st, "onfindrecord")
                && !self.is_list(st)
                && self.declares(st, "onaftergetcurrrecord");
            if find_pinned {
                // pages/lt-update-on-find/UpdateOnFindRecord.al:P168_List_Update*, P168_Card_Update*: the reads come before
                // the refresh's `Find:=`.
                self.read_page_again(st)?;
                st.borrow_mut().refresh = true;
                self.run_refresh(st)?;
            } else if !find_page {
                // The reads the page does when it opens come first, then the refresh's own: a card
                // reads its row twice, a list the rows of its window as it opens plus one (corpus
                // pageext/PCCurrRecordReads.al:Card_ReadsAfterEachAction, List_ReadsAfterEachAction,
                // pinned for a card and a list of 3 rows with the cursor on the first).
                if reads_rows {
                    self.read_page_again(st)?;
                }
                st.borrow_mut().refresh = true;
            }
            if !find_pinned {
                self.run_refresh(st)?;
            }
        }
        Ok(Value::Null)
    }

    /// The reads of an action that did not call `CurrPage.Update`, before the refresh's own (see
    /// `action_invoke`).
    pub(super) fn read_page_again(&mut self, st: &St) -> Result<(), Signal> {
        if !st.borrow().open || st.borrow().cursor != Cursor::Row || st.borrow().draft {
            return Ok(());
        }
        let rec = self.page_rec(st);
        if self.stored_row(&rec)?.is_none() {
            return Ok(());
        }
        if !self.is_list(st) {
            return self.populate_current(st);
        }
        let key = self.row_key(&rec)?;
        if self.declares(st, "onaftergetcurrrecord") && self.declares(st, "onaftergetrecord") {
            return self.read_list_again_from_row(st, &rec, &key);
        }
        let first = {
            let s = st.borrow();
            s.window_from_start
                && s.window.len() < Self::WINDOW
                && s.window.first().is_some_and(|w| Self::same_key(&w.key, &key))
        };
        if !first {
            return self.read_list_again_from_row(st, &rec, &key);
        }
        st.borrow_mut().landing_first = true;
        let r = self.populate_window(st);
        st.borrow_mut().landing_first = false;
        r?;
        st.borrow_mut().window_from_start = true;
        Ok(())
    }

    /// The reads of an action on a list whose cursor is not on the first row: the row again, the rows from it on (up to 20),
    /// the row again when the view goes on past it, the rows back to the first one, and the row twice more before the refresh's
    /// own read. Pinned by misc/lt-probes-needed-6/PnF05ActionReads.al:Action_List_LaterRow (row 2 and 3 of 3 rows: 7 and 6
    /// reads after the action) and :Action_List_ManyRows (row 6 of 25: 29 reads), and, with the row identities, by
    /// misc/lt-probes-needed-7/PnGNewRecord.al:ListPendingNewRowSavedBeforeAction (last row of three: `P1;P1;N2;N1;P1;P1;C`)
    /// and misc/lt-probes-needed-7/PnGListUpdate.al:ListModifyWithAndWithoutUpdate (`U2;U2;U3;U2;U1;U2;U2;C`). Only a page that declares both row triggers and serves its rows from the table is
    /// pinned; the identities of the rows beyond the window held are the table order.
    fn read_list_again_from_row(&mut self, st: &St, rec: &Rec, key: &[Value]) -> Result<(), Signal> {
        let ok = self.declares(st, "onaftergetrecord") && st.borrow().window_from_start;
        if !ok {
            // Other shapes are not pinned: the refresh alone stays (docs/probes-needed.md).
            return Ok(());
        }
        let find = self.declares(st, "onfindrecord");
        let mut pk = key.to_vec();
        self.populate_current(st)?;
        if find {
            // The forward reads start with `=>` (pages/lt-update-on-find/UpdateOnFindRecord.al:P168_List_UpdateFalse).
            self.nav_find(st, "=>")?;
        }
        self.populate_current(st)?;
        let mut moved = false;
        for _ in 1..Self::WINDOW {
            if !self.nav_next(st, 1)? {
                break;
            }
            moved = true;
            self.populate_current(st)?;
        }
        if moved {
            self.ret_used = true;
            self.record_op(RecordOp::Get, rec, &mut pk)?;
        }
        // Is there a row before it? Asked of the table: the page's own OnNextRecord is asked when the client steps.
        self.ret_used = true;
        let has_before =
            matches!(self.record_op(RecordOp::Next, rec, &mut [Value::Integer(-1)])?, Value::Integer(n) if n != 0);
        self.ret_used = true;
        self.record_op(RecordOp::Get, rec, &mut pk)?;
        let mut more_rows = moved;
        if has_before {
            more_rows = true;
            if moved {
                // The client reads the row again when it steps back onto it from the rows past it; the first row
                // has no step back (Action_List_FirstRow reads 6 times after the action, Action_List_LaterRow 7).
                self.populate_current(st)?;
            }
        }
        if has_before || find {
            while self.nav_next(st, -1)? {
                self.populate_current(st)?;
            }
        }
        self.ret_used = true;
        self.record_op(RecordOp::Get, rec, &mut pk)?;
        // A view of one row has no step back onto it (a list of one row opens with RRC, not RRRC; Action_List_FewRows).
        if more_rows {
            self.populate_current(st)?;
        }
        Ok(())
    }

    /// The re-read `CurrPage.Update` asked for, once the call that made it has returned: the
    /// stored current row loads again and raises OnAfterGetRecord and OnAfterGetCurrRecord; a new
    /// row nobody has saved has nothing to re-read (corpus pageupdate/TestPageUpdateNewRow.al:1-30,
    /// TestPageUpdateGoneRow.al StoredRow_OnValidateUpdateFalse_Trace). The row is read from the
    /// table unless the client holds edits of it that are not saved yet.
    pub(super) fn run_refresh(&mut self, st: &St) -> Result<(), Signal> {
        if !std::mem::take(&mut st.borrow_mut().refresh) {
            return Ok(());
        }
        if !st.borrow().open || !self.has_source(st) || st.borrow().cursor != Cursor::Row || st.borrow().draft {
            return Ok(());
        }
        // A page that serves its rows from OnFindRecord asks the trigger to settle the client on a row
        // again (corpus pageupdate/TestPageUpdateGoneRow.al:390 pins the Which strings of a deleted
        // row only). Which row that is, is not modelled: nothing that shows it may be read before the
        // cursor moves, and the page's row triggers must not matter.
        if self.declares(st, "onfindrecord") || self.declares(st, "onnextrecord") {
            if self.declares(st, "onaftergetrecord") || self.declares(st, "onaftergetcurrrecord") {
                // The client asks OnFindRecord for the current row (`=`), then reads it as it does after
                // any move. Assumed (update-refresh-onfindrecord): Base Application's document pages do this
                // under Microsoft's tests; a page that holds edits or has only OnNextRecord stays refused.
                // A list refills its window around the row (misc/W2FindRecordUpdate.al:Update_True_OnOpenedPage pins that
                // trace); only a card, which has the one row, is served.
                if self.is_list(st) && self.declares(st, "onfindrecord") {
                    return self.refresh_list_with_find(st);
                }
                if !self.declares(st, "onfindrecord") || st.borrow().dirty || self.is_list(st) {
                    let why = if !self.declares(st, "onfindrecord") {
                        "only OnNextRecord"
                    } else if st.borrow().dirty {
                        "unsaved edits"
                    } else {
                        "a list"
                    };
                    return unsupported(format!(
                        "CurrPage.Update on a page with OnFindRecord and row triggers (re-anchoring not pinned; {why})"
                    ));
                }
                al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::UpdateRefreshOnFindRecord);
                if !self.nav_find(st, "=")? {
                    return unsupported(
                        "OnFindRecord does not find the current row after CurrPage.Update (not pinned)",
                    );
                }
                let r = (|| {
                    self.populate_current(st)?;
                    self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
                    self.page_event(st, "onaftergetcurrrecordevent", None, None)
                })();
                self.torn_down_on_error(st, r)?;
                st.borrow_mut().refresh = false;
                return self.sync_parts(st);
            }
            st.borrow_mut().stale = true;
            return Ok(());
        }
        let rec = self.page_rec(st);
        if self.stored_row(&rec)?.is_none() {
            return Ok(());
        }
        if !st.borrow().dirty {
            self.ret_used = true;
            let mut pk = self.table(&rec)?.primary_key(&rec.borrow().fields);
            self.record_op(RecordOp::Get, &rec, &mut pk)?;
        }
        let r = (|| {
            self.populate_current(st)?;
            self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
            self.page_event(st, "onaftergetcurrrecordevent", None, None)
        })();
        self.torn_down_on_error(st, r)?;
        st.borrow_mut().refresh = false;
        // The parts read their rows again after the host's own triggers (corpus
        // testpart/TestPartTriggerOrderAndRefresh.al:HostCurrPageUpdate_ReReadsThePart:
        // "host.Action;host.AGCR(2);part.AGCR(2);", and no part OnOpenPage).
        self.sync_parts(st)?;
        Ok(())
    }

    /// `CurrPage.Update` on a list that serves its rows from OnFindRecord, with or without OnNextRecord (corpus misc/W2FindRecordUpdate.al:
    /// Update_False_OnOpenedPage, Update_True_OnOpenedPage, Update_StoredRowChangedBehindThePage, Update_True_AfterRecChangedInMemory,
    /// all one trace for a table of 3 rows with the cursor on the middle one): the row is read again, `=>` finds the row to
    /// start from, the client reads on to the end of the view, puts the first of those rows back, steps back to the start of
    /// the view reading each row it moves to, puts the row back, asks `=` for it and reads it as after a move. Only that shape
    /// is pinned, with both triggers; the same steps are taken for other shapes, among them a list with OnFindRecord alone, where
    /// `Next` moves the record (assumption update-refresh-list-onfindrecord), and a view that fills
    /// a window stays refused.
    fn refresh_list_with_find(&mut self, st: &St) -> Result<(), Signal> {
        al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::UpdateRefreshListOnFindRecord);
        let rec = self.page_rec(st);
        self.page_trigger(st, "onaftergetrecord", vec![])?;
        if !self.nav_find(st, "=>")? {
            return unsupported("OnFindRecord finds no row after CurrPage.Update on a list (not pinned)");
        }
        let mut forward = Vec::new();
        loop {
            if forward.len() + 1 >= Self::WINDOW {
                return unsupported("CurrPage.Update on a list whose view fills a window (not pinned)");
            }
            let key = self.row_key(&rec)?;
            let stored = rec.borrow().fields.clone();
            self.populate_current(st)?;
            forward.push(al_builtins::value::WindowRow {
                key,
                stored,
                shown: rec.borrow().fields.clone(),
            });
            if !self.nav_next(st, 1)? {
                break;
            }
        }
        let first = forward[0].clone();
        rec.borrow_mut().fields = first.stored.clone();
        // On the last row the client has not moved, so it puts nothing back (pages/lt-update-on-find/UpdateOnFindRecord.al:
        // P168_List_UpdateOnLastRow: `Next:1;Next:-1;AGR:2`).
        if forward.len() > 1 {
            self.page_trigger(st, "onaftergetrecord", vec![])?;
        }
        let mut before = Vec::new();
        while self.nav_next(st, -1)? {
            if before.len() + forward.len() + 1 >= Self::WINDOW {
                return unsupported("CurrPage.Update on a list whose view fills a window (not pinned)");
            }
            let key = self.row_key(&rec)?;
            let stored = rec.borrow().fields.clone();
            self.populate_current(st)?;
            before.push(al_builtins::value::WindowRow {
                key,
                stored,
                shown: rec.borrow().fields.clone(),
            });
        }
        before.reverse();
        rec.borrow_mut().fields = first.stored.clone();
        self.page_trigger(st, "onaftergetrecord", vec![])?;
        if !self.nav_find(st, "=")? {
            return unsupported("OnFindRecord does not find the current row after CurrPage.Update (not pinned)");
        }
        let r = (|| {
            self.populate_current(st)?;
            self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
            self.page_event(st, "onaftergetcurrrecordevent", None, None)
        })();
        self.torn_down_on_error(st, r)?;
        before.extend(forward);
        st.borrow_mut().window = before;
        st.borrow_mut().window_from_start = true;
        st.borrow_mut().refresh = false;
        self.sync_parts(st)
    }

    /// What the page does once an action returns: a page that showed no row may show the one the
    /// action inserted; a row the action deleted closes a card, and moves a list to the next row,
    /// else the previous one, else none (corpus pageupdate/TestPageUpdateGoneRow.al:1-30).
    pub(super) fn settle_after_action(&mut self, st: &St) -> Result<(), Signal> {
        if !self.has_source(st) {
            return Ok(());
        }
        self.show_row_page_code_positioned(st)?;
        if !st.borrow().open || st.borrow().cursor != Cursor::Row || st.borrow().draft {
            return Ok(());
        }
        let rec = self.page_rec(st);
        if self.stored_row(&rec)?.is_some() {
            return Ok(());
        }
        if !self.is_list(st) {
            let parts: Vec<St> = st.borrow().parts.iter().map(|(_, c)| c.clone()).collect();
            if parts
                .iter()
                .any(|c| self.declares(c, "onclosepage") || self.declares(c, "onqueryclosepage"))
            {
                return unsupported("a card closed by a deleted row whose parts declare close triggers (not pinned)");
            }
            self.page_trigger(st, "onclosepage", vec![])?;
            self.mark_closed(st);
            return Ok(());
        }
        if self.declares(st, "onfindrecord") {
            return self.settle_gone_row_with_find(st, &rec);
        }
        self.ret_used = true;
        let moved = matches!(
            self.record_op(RecordOp::Next, &rec, &mut [Value::Integer(1)])?,
            Value::Integer(1)
        ) || matches!(
            self.record_op(RecordOp::Next, &rec, &mut [Value::Integer(-1)])?,
            Value::Integer(-1)
        );
        if moved {
            let r = self.arrive(st);
            self.torn_down_on_error(st, r)?;
        } else {
            // No stored row is left: the list shows its blank line, which raises
            // OnAfterGetCurrRecord three times after an action that called CurrPage.Update (corpus
            // pageupdate/TestPageUpdateGoneRow.al:List_DeletedByAction_OnlyRow_ShowsNoStoredRow);
            // what an action that did not call it raises is not pinned.
            if !st.borrow().refresh {
                return unsupported("a list whose only row an action deleted, without CurrPage.Update");
            }
            st.borrow_mut().refresh = false;
            let table = rec.borrow().table as usize;
            let fresh = self.initial_fields(table);
            rec.borrow_mut().fields = fresh;
            st.borrow_mut().cursor = Cursor::None;
            for _ in 0..3 {
                self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
                self.page_event(st, "onaftergetcurrrecordevent", None, None)?;
            }
        }
        Ok(())
    }

    /// A list that serves its rows from OnFindRecord lost its current row to an action. The client asks
    /// the trigger again (corpus pageupdate/TestPageUpdateGoneRow.al:390 List_DeletedByAction_OnFindRecord_PicksTheRow,
    /// :423 List_DeletedByAction_PassThroughFind_MiddleRow, :456 _LastRow, :483 _OnlyRow):
    /// * when it knows other rows of the list: `=` (the gone row), `=>` (from there on), and `=` again for
    ///   the row it settled on, which is the one `=>` answered, else the row before the gone one;
    /// * when it knows no other row: `=>` and `=><` only, and then it shows the blank line, raising
    ///   OnAfterGetCurrRecord once before the two calls and twice after.
    ///
    /// "Knows other rows" is read as: the rows it populated hold more than the gone one. That reading is
    /// only checked against these four shapes (docs/probes-needed.md); a list the client knows as
    /// one row while the view holds more is refused.
    pub(super) fn settle_gone_row_with_find(&mut self, st: &St, rec: &Rec) -> Result<(), Signal> {
        let gone = self.row_key(rec)?;
        let window = st.borrow().window.clone();
        let at = window.iter().position(|w| Self::same_key(&w.key, &gone));
        if window.len() > 1 {
            let Some(at) = at else {
                return unsupported("a list row an action deleted, which the client had not populated (not pinned)");
            };
            self.nav_find(st, "=")?;
            let after = self.nav_find(st, "=>")?;
            if !after {
                if at == 0 {
                    return unsupported("a deleted first row whose OnFindRecord finds no row after it (not pinned)");
                }
                rec.borrow_mut().fields = window[at - 1].stored.clone();
            }
            if !self.nav_find(st, "=")? {
                return unsupported("OnFindRecord does not find the row the client settled on");
            }
            st.borrow_mut().window = Vec::new();
            st.borrow_mut().window_from_start = false;
            st.borrow_mut().cursor = Cursor::Row;
            let r = self.arrive(st);
            return self.torn_down_on_error(st, r);
        }
        // The client knows no other row: nothing may be left in the view for that to be true.
        let probe = Rc::new(RefCell::new(rec.borrow().clone()));
        self.ret_used = true;
        if matches!(
            self.record_op(RecordOp::FindFirst, &probe, &mut [])?,
            Value::Boolean(true)
        ) {
            return unsupported(
                "a list whose only populated row an action deleted, with more rows in its view (not pinned)",
            );
        }
        let table = rec.borrow().table as usize;
        let fresh = self.initial_fields(table);
        rec.borrow_mut().fields = fresh;
        st.borrow_mut().cursor = Cursor::None;
        st.borrow_mut().window = Vec::new();
        st.borrow_mut().window_from_start = false;
        self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
        self.page_event(st, "onaftergetcurrrecordevent", None, None)?;
        if self.nav_find(st, "=>")? || self.nav_find(st, "=><")? {
            return unsupported("OnFindRecord finds a row in a view the page showed as empty (not pinned)");
        }
        for _ in 0..2 {
            self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
            self.page_event(st, "onaftergetcurrrecordevent", None, None)?;
        }
        st.borrow_mut().refresh = false;
        Ok(())
    }

    /// A page that showed no row shows one again once page code has inserted a row the view
    /// admits and left Rec on it; code that only changes the buffer, sets a key it never
    /// inserts, or reads a row the view hides shows nothing (corpus
    /// testpart/BnrBlankPart.al:1-30, NoRow_Action_* arms).
    pub(super) fn show_row_page_code_positioned(&mut self, st: &St) -> Result<(), Signal> {
        if !st.borrow().open || st.borrow().cursor != Cursor::None {
            return Ok(());
        }
        let rec = self.page_rec(st);
        let table = self.table(&rec)?;
        let probe = Rc::new(RefCell::new(rec.borrow().clone()));
        let pk = table.primary_key(&rec.borrow().fields);
        for (n, &f) in table.keys[0].iter().enumerate() {
            let mut p = probe.borrow_mut();
            p.filters.retain(|x| x.field != f);
            p.filters.push(al_builtins::filter::FieldFilter {
                field: f,
                expr: al_builtins::filter::FilterExpr::Range(Some(pk[n].clone()), Some(pk[n].clone())),
                group: 0,
            });
        }
        self.ret_used = true;
        if matches!(
            self.record_op(RecordOp::FindFirst, &probe, &mut [])?,
            Value::Boolean(true)
        ) {
            let fields = probe.borrow().fields.clone();
            // The probe replaced the view's filters on key fields with the key; the row must still
            // pass the filters the page has (a Get of a row the view hides shows nothing:
            // testpart/BnrBlankPart.al NoRow_Action_GetOfARowTheFilterHides_StaysBlank).
            if !rec.borrow().filters.iter().all(|f| f.expr.matches(&fields[f.field])) {
                return Ok(());
            }
            rec.borrow_mut().fields = fields;
            st.borrow_mut().cursor = Cursor::Row;
            let r = self.arrive(st);
            self.torn_down_on_error(st, r)?;
        }
        Ok(())
    }
}
