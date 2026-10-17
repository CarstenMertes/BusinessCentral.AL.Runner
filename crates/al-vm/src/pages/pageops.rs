use super::*;
use al_builtins::page_op::{PageMethod, PageOp, PageRecv};
use al_builtins::record_op::RecordOp;

impl Session {
    /// The `TestPage.*` / `TestField.*` methods (ADR-012).
    pub(crate) fn page_op(
        &mut self,
        method: PageOp,
        recv: Option<&mut Value>,
        args: &mut [Value],
    ) -> Result<Value, Signal> {
        // A TestPage begins a transaction of its own (see `in_page_transaction`). Page.Run and Page.RunModal with a handler that writes are allowed under None, and the
        // transaction the page began has ended afterwards
        // (codeunit/TestWriteTxNoneModelContracts.al:200-216 and :163-185).
        if recv.is_none() && self.no_transaction() {
            if matches!(method, PageOp::PageRunStatic | PageOp::PageRunModalStatic) {
                return self.in_page_transaction(|s| s.page_op_inner(method, recv, args));
            }
            return unsupported(format!("{method} under TransactionModel::None (not pinned)"));
        }
        if recv.is_some() && self.no_transaction() {
            return self.in_page_transaction(|s| s.page_op_inner(method, recv, args));
        }
        self.page_op_inner(method, recv, args)
    }

    pub(super) fn page_op_inner(
        &mut self,
        method: PageOp,
        recv: Option<&mut Value>,
        args: &mut [Value],
    ) -> Result<Value, Signal> {
        match recv {
            None if matches!(
                method,
                PageOp::PageGetBackgroundParameters | PageOp::PageSetBackgroundTaskResult
            ) =>
            {
                self.bg_static(method, args)
            }
            None if method == PageOp::PageRunStatic || method == PageOp::PageRunModalStatic => {
                let n = match args.first() {
                    Some(Value::Integer(n)) => *n,
                    other => return unsupported(format!("Page.Run with {} as the page", other.describe())),
                };
                let engine = self.engine();
                let record = match args.get(1) {
                    Some(Value::Record(r)) => Some(r.clone()),
                    None => None,
                    other => return unsupported(format!("Page.Run with {} as the record", other.describe())),
                };
                // A third argument is a field of the record (`Page.RunModal(Id, Rec, Rec.Field)`), arriving as its value;
                // the pinned case is the page with no handler (see `run_page_ui_with`).
                let field = match args.len() {
                    0..=2 => false,
                    3 => true,
                    _ => return unsupported("Page.Run with more than three arguments"),
                };
                // A FieldRef nobody assigned raises before anything opens (pages/lt-page-run-overloads-testpart/P106RunErrors.al:
                // P106_Err_FieldRefVariants, observation 'uninitialized').
                if let Some(Value::Library(_, lib)) = args.get(2) {
                    if !matches!(
                        &*lib.borrow(),
                        al_builtins::value::LibraryValue::FieldRef(..)
                            | al_builtins::value::LibraryValue::KeyFieldRef(..)
                    ) {
                        return Err(error_code(
                            "Microsoft.Dynamics.Nav.Runtime.NavFieldRef variable not initialized.",
                            "NotInitialized",
                        ));
                    }
                }
                // Id 0 with a record resolves the page from the record's table LookupPageId; a
                // table without one refuses like any unknown id (corpus
                // handlers/TestPageRunModalLookupPageId_Tests.al).
                let mut id = n as u32;
                if n == 0 {
                    if let Some(r) = &record {
                        let table = r.borrow().table as usize;
                        let lookup = engine.program.objects[table]
                            .properties
                            .iter()
                            .find(|(k, _)| k.eq_ignore_ascii_case("LookupPageId"))
                            .map(|(_, v)| v.trim().trim_matches('"').to_string());
                        if let Some(l) = lookup {
                            let found = engine.program.objects.iter().find(|o| {
                                o.is_kind(al_ir::ObjKind::Page)
                                    && (o.name.eq_ignore_ascii_case(&l) || o.id.is_some_and(|i| i.to_string() == l))
                            });
                            if let Some(i) = found.and_then(|o| o.id) {
                                id = i;
                            } else if names_uncompiled_page(&engine, &l) {
                                return unsupported(format!(
                                    "the LookupPageId of table {} is page {l}, which belongs to a dependency app and was not compiled (no code the workspace reaches uses it)",
                                    engine.program.objects[table].name
                                ));
                            }
                        }
                    }
                }
                let Some(object) = engine
                    .program
                    .objects
                    .iter()
                    .position(|o| o.is_kind(al_ir::ObjKind::Page) && o.id == Some(id))
                else {
                    // A page of a dependency app that nothing reached is not compiled (ADR-013); BC has it.
                    if engine
                        .program
                        .objects
                        .iter()
                        .any(|o| o.is_shell_of(al_ir::ObjKind::Page) && o.id == Some(id))
                    {
                        return unsupported(format!(
                            "page {n} belongs to a dependency app and was not compiled (no code the workspace reaches uses it)"
                        ));
                    }
                    return Err(self.missing_object("Page", i64::from(n)));
                };
                let st = Rc::new(RefCell::new(PageState::closed(object as u32)));
                // The static RunModal reports the lookup values, with or without a record: OK reads
                // LookupOK, Cancel LookupCancel (corpus handlers/TestPageModalHandlerStatic_Tests.al:48-72
                // for the record overload;
                // handlers/TestPageModalResultsAndTriggersContracts.al:206 RunModal_Dialog_StaticVersusVariable_ReportedAction
                // and :240 RunModal_Card_StaticWithAndWithoutRecord_VersusVariable_ReportedAction for the others). A page variable reports OK and Cancel.
                if method == PageOp::PageRunModalStatic {
                    st.borrow_mut().lookup = true;
                }
                // A record of another table than the page's is refused before anything opens, and so is a field number the table
                // lacks (a negative one or 0 goes on; pages/lt-page-run-overloads-testpart/P106RunErrors.al: Err_WrongTableRecord,
                // Err_FieldNoNotExisting).
                if let (Some(r), true) = (&record, self.has_source(&st)) {
                    self.page_instance(&st)?;
                    if self.page_rec(&st).borrow().table != r.borrow().table {
                        return Err(error_code("The table IDs do not match.", "TableIdMismatch"));
                    }
                    if let Some(Value::Integer(no)) = args.get(2) {
                        let table = self.table(r)?;
                        if *no > 0 && !table.fields.iter().any(|f| i64::from(f.id) == i64::from(*no)) {
                            return Err(error_code(
                                format!(
                                    "The supplied field number '{no}' cannot be found in the '{}' table.",
                                    self.caption(&table)
                                ),
                                "FieldNotFound",
                            ));
                        }
                    }
                }
                self.run_page_ui_with(&st, record, method == PageOp::PageRunModalStatic, field)
            }
            Some(Value::TestPage(st)) => {
                let st = st.clone();
                self.test_page_op(method, &st, args)
            }
            Some(Value::TestControl(c)) => {
                let c = c.clone();
                self.test_control_op(method, &c, args)
            }
            other => unsupported(format!("{method} on {}", other.describe())),
        }
    }

    pub(super) fn test_page_op(&mut self, method: PageOp, st: &St, args: &mut [Value]) -> Result<Value, Signal> {
        let name = method.method();
        let r = match self.test_page_op_inner(method, st, args) {
            // `CurrPage` and a `Page` variable are the page's own code, not a TestPage the test closed: BC does not raise
            // "The TestPage is not open." for them. What a method of a page that is not open (any more) does is not pinned, so
            // the call is refused instead of failing with the TestPage error (Posted Sales Document Lines.CopyLineToDoc reads
            // `CurrPage.PostedShpts.PAGE` after its RunModal returned).
            // `Run` and `RunModal` only pass on what their handler raised (handlers/TestPageModalSelfClose_Tests.al:104).
            Err(Signal::Error(e))
                if e.code == "TestPageNotOpened"
                    && method.recv() == PageRecv::Page
                    && !matches!(name, PageMethod::Run | PageMethod::RunModal) =>
            {
                return unsupported(format!(
                    "{} called by the code of a page that is not open (what it does then is not pinned)",
                    method.method()
                ));
            }
            r => r?,
        };
        // The client runs the page's background tasks before the call returns (corpus
        // handlers/TestPageBgTask_Tests.al:EnqueueBackgroundTask_CompletesBeforeOpenViewReturns).
        self.bg_drain(st)?;
        // A move of the host re-points its parts; a move of a part re-points the parts that take
        // their link from it (corpus testpart/TestPartProviderMove.al).
        let moved = match name {
            PageMethod::First
            | PageMethod::Last
            | PageMethod::Next
            | PageMethod::Previous
            | PageMethod::GoToKey
            | PageMethod::GoToRecord
            | PageMethod::FindFirstField
            | PageMethod::FindNextField
            | PageMethod::FindPreviousField => !matches!(r, Value::Boolean(false)),
            PageMethod::New => true,
            // CurrPage.Update leaves a host's parts as they are: they are not opened again (corpus
            // handlers/TestPageNoSrcPart_Tests.al:DirectOpen_NoSrcHost_HostWriteIsVisibleThroughTheSameInstance);
            // whether they read their rows again is docs/probes-needed.md.
            _ => false,
        };
        if moved && st.borrow().open {
            if st.borrow().host.is_some() {
                self.sync_dependents(st)?;
            } else {
                self.sync_parts(st)?;
            }
        }
        Ok(r)
    }

    pub(super) fn test_page_op_inner(&mut self, method: PageOp, st: &St, args: &mut [Value]) -> Result<Value, Signal> {
        match method.method() {
            // `TestPart.Visible` / `.Enabled` read the part control of the host: a part that is
            // reachable at all is visible (corpus testpart/TestTestPart.al:TestPart_Visible_*).
            // A part whose Visible expression came out false answers No (misc/lt-probes-needed-7/PnGPartTriggers.al:
            // HiddenPartExpression).
            PageMethod::Visible if method.recv() == PageRecv::TestPart => {
                self.require_open(st)?;
                let host = st.borrow().host.clone();
                match host.and_then(|(h, i)| h.upgrade().map(|h| (h, i))) {
                    Some((h, i)) => Ok(Value::Boolean(self.control_visible(&h, i)?)),
                    None => Ok(Value::Boolean(true)),
                }
            }
            PageMethod::Enabled if method.recv() == PageRecv::TestPart => {
                self.require_open(st)?;
                let host = st.borrow().host.clone();
                match host.and_then(|(h, i)| h.upgrade().map(|h| (h, i))) {
                    Some((h, i)) => Ok(Value::Boolean(self.control_enabled(&h, i)?)),
                    None => unsupported("TestPart.Enabled on a page that is not a part"),
                }
            }
            // The id is a compiler-generated control id; the corpus pins only that a table field
            // number is not one (testpart/TestTestPart.al:TestPart_GetField_TakesAControlIdNotATableFieldNo).
            PageMethod::Filter => {
                self.require_open(st)?;
                Ok(self.filter_handle(st))
            }
            PageMethod::GetField => {
                self.require_open(st)?;
                match args.first() {
                    // Code: handlers/TestPageRuntimeContracts.al:274-275 (page and part).
                    Some(Value::Integer(n)) => Err(error_code(
                        format!("The field with ID = {n} is not found on the page."),
                        "TestFieldNotFound",
                    )),
                    _ => unsupported("TestPart.GetField without an id"),
                }
            }
            PageMethod::OpenEdit => self.page_open(st, PageMode::Edit),
            PageMethod::OpenView => self.page_open(st, PageMode::View),
            PageMethod::OpenNew => self.page_open(st, PageMode::New),
            PageMethod::Close if method.recv() != PageRecv::Page => self.page_close(st),
            m @ (PageMethod::First | PageMethod::Last | PageMethod::Next | PageMethod::Previous) => {
                self.require_open(st)?;
                self.page_move(st, m)
            }
            PageMethod::GoToKey => {
                self.require_open(st)?;
                self.page_go_to_key(st, args)
            }
            m @ (PageMethod::FindFirstField | PageMethod::FindNextField | PageMethod::FindPreviousField) => {
                self.require_open(st)?;
                self.page_find_field(st, m, args)
            }
            // A page that is not a tree has nothing to expand (docs only; the corpus calls them,
            // TestPageAdvanced.al:104, :283, asserting nothing about the answer).
            PageMethod::Expand => {
                self.require_open(st)?;
                Ok(Value::Null)
            }
            PageMethod::IsExpanded => {
                self.require_open(st)?;
                Ok(Value::Boolean(false))
            }
            PageMethod::SaveAsXml => self.request_save_as_xml(st, args),
            PageMethod::SaveAsPdf => self.request_save_as_render(st, args, "Pdf"),
            PageMethod::SaveAsExcel => self.request_save_as_render(st, args, "Excel"),
            m @ (PageMethod::OK | PageMethod::Cancel | PageMethod::Yes | PageMethod::No) => {
                self.require_open(st)?;
                if !self.builtin_offered(st, m.name()) {
                    // Code TestActionNotFound (pages/lt-page-run-overloads-testpart/P106RunInstance.al:P106_Inst_RunModal_List `af100`).
                    return Err(error_code(
                        format!("The built-in action = {m} is not found on the page."),
                        "TestActionNotFound",
                    ));
                }
                Ok(Value::TestControl(Rc::new(ControlRef {
                    page: st.clone(),
                    target: ControlTarget::BuiltIn(m.to_string()),
                })))
            }
            m @ (PageMethod::Edit | PageMethod::View) => {
                self.require_open(st)?;
                Ok(Value::TestControl(Rc::new(ControlRef {
                    page: st.clone(),
                    target: ControlTarget::BuiltIn(m.to_string()),
                })))
            }
            // `ObjectId(UseNames)`: `Page <id>`, or with names (the default) `Page <Caption>`, the object name when
            // the page has no Caption. Taken from the probed report form (handlers/PWRefusedBuiltins.al:78-107,
            // BC 27.5; the page form is pinned by handlers/PRPageObjectId.al). It reads the object, so
            // a page that has not run answers too.
            PageMethod::ObjectId if method.recv() == PageRecv::Page => {
                let names = match args.first() {
                    None => true,
                    Some(Value::Boolean(b)) => *b,
                    Some(other) => return unsupported(format!("Page.ObjectId with {}", other.describe())),
                };
                let engine = self.engine();
                let o = &engine.program.objects[st.borrow().object as usize];
                if !names {
                    return Ok(Value::Text(format!("Page {}", o.id.unwrap_or(0)).into()));
                }
                let caption = match o.properties.iter().find(|(k, _)| k.eq_ignore_ascii_case("Caption")) {
                    Some((_, v)) => match crate::recmeta::string_literal(v) {
                        Some(c) => c,
                        None => return unsupported("Page.ObjectId of a page whose Caption is not a literal"),
                    },
                    None => o.name.clone(),
                };
                Ok(Value::Text(format!("Page {caption}").into()))
            }
            // `CurrPage.Caption := x` sets the page's caption for as long as it stays open; without an
            // argument it answers the caption in force (handlers/TestPageCaption_Page.al:44).
            PageMethod::Caption if method.recv() == PageRecv::Page => {
                // The caption read on a Page variable before the page runs is the page's own (Edit Dimension Set Entries.SetFormCaption
                // reads `CurrPage.Caption` before Dimension Management runs the page); an assignment still needs an open page
                // (assumption caption-read-before-the-page-runs).
                if args.is_empty() && !st.borrow().open {
                    al_builtins::assumptions::assume(
                        al_builtins::assumptions::Assumption::CaptionReadBeforeThePageRuns,
                    );
                    return Ok(Value::Text(self.page_caption(st).into()));
                }
                self.require_open(st)?;
                match args.first() {
                    Some(v) => match al_builtins::native::text_of(v) {
                        Some(t) => {
                            st.borrow_mut().caption = Some(t.to_string());
                            Ok(Value::Null)
                        }
                        None => unsupported(format!("CurrPage.Caption with {}", v.describe())),
                    },
                    None => Ok(Value::Text(self.page_caption(st).into())),
                }
            }
            // `ObjectId(UseNames)` (CurrPage and a Page variable alike, open or not): `Page <id>`, or with names, the
            // default, `Page <Caption>` and the object name when the page has no Caption (handlers/PRPageObjectId.al).
            PageMethod::ObjectId if method.recv() == PageRecv::Page => {
                let names = match args.first() {
                    None => true,
                    Some(v) => ops::truthy(v)?,
                };
                let engine = self.engine();
                let o = &engine.program.objects[st.borrow().object as usize];
                if !names {
                    return Ok(Value::Text(format!("Page {}", o.id.unwrap_or(0)).into()));
                }
                let caption = o
                    .properties
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("Caption"))
                    .map(|(_, v)| unquote(v))
                    .unwrap_or_else(|| o.name.clone());
                Ok(Value::Text(format!("Page {caption}").into()))
            }
            PageMethod::Caption => {
                self.require_open(st)?;
                Ok(Value::Text(self.test_page_caption(st)?.into()))
            }
            // `CurrPage.PromptMode := PromptMode::Prompt` states what a PromptDialog already starts in:
            // the pages of handlers/TbaPromptDeclared.Page.al and TbaPromptBare.Page.al do it in OnOpenPage
            // and the tests around them (TestPageBuiltInActions_Tests.al (a)-(j)) see an ordinary dialog.
            // The other modes are not pinned.
            PageMethod::PromptMode => match args.first() {
                Some(Value::Option(n, members)) if members.get(*n as usize).is_some_and(|m| m == "Prompt") => {
                    Ok(Value::Null)
                }
                Some(Value::Enum(..)) | Some(Value::Option(..)) => {
                    unsupported("CurrPage.PromptMode other than Prompt (not pinned)")
                }
                other => unsupported(format!("CurrPage.PromptMode with {}", other.describe())),
            },
            PageMethod::Editable if method.recv() == PageRecv::Page => {
                // A Page variable that has not run (Base App sets `Languages.Editable := false` before
                // `RunModal`): no "not open" error. It reads true until set, and what it was set to is
                // what the page opens with, for Run and RunModal alike
                // (handlers/TestPageVariableEditableContracts.al).
                if !st.borrow().open {
                    return match args.first() {
                        Some(v) => {
                            let on = ops::truthy(v)?;
                            st.borrow_mut().editable_preset = Some(on);
                            Ok(Value::Null)
                        }
                        // What it was set to, else the page's own `Editable` property (corpus
                        // handlers/PWPageVariableEditable.al:187-205: a page with `Editable = false`
                        // reads No before it runs).
                        None => {
                            let engine = self.engine();
                            let object = st.borrow().object as usize;
                            let own = !engine.program.objects[object].properties.iter().any(|(k, v)| {
                                k.eq_ignore_ascii_case("Editable") && v.trim().eq_ignore_ascii_case("false")
                            });
                            Ok(Value::Boolean(st.borrow().editable_preset.unwrap_or(own)))
                        }
                    };
                }
                match args.first() {
                    // CurrPage.Editable(false) in OnOpenPage makes the page, page-variable controls
                    // included, read-only; Editable(true) there does not widen a View page. Calls
                    // from later triggers leave TestPage.Editable() alone (corpus
                    // handlers/TestPageControlEditableRoutes.al:536-555, TestPageEditable_Tests.al:150-180;
                    // their effect on Rec-bound controls is docs/probes-needed.md).
                    Some(v) => {
                        if st.borrow().opening && !ops::truthy(v)? {
                            st.borrow_mut().editable_off = true;
                        }
                        Ok(Value::Null)
                    }
                    None => Ok(Value::Boolean(self.page_editable(st))),
                }
            }
            PageMethod::Editable => {
                self.require_open(st)?;
                Ok(Value::Boolean(self.page_editable(st)))
            }
            PageMethod::Trap => {
                // The next plain Page.Run of this page type opens into this variable
                // (corpus handlers/TestPageRunHandler_Tests.al:TrapReceivesTheNonModalPageAndItStaysOpen).
                st.borrow_mut().trapping = true;
                // A second Trap() on the same page, from this or another variable, replaces the first.
                let object = st.borrow().object;
                self.pages
                    .traps
                    .retain(|t| !Rc::ptr_eq(t, st) && t.borrow().object != object);
                self.pages.traps.push(st.clone());
                Ok(Value::Null)
            }
            PageMethod::LookupMode => match args.first() {
                Some(v) => {
                    st.borrow_mut().lookup = ops::truthy(v)?;
                    st.borrow_mut().lookup_set = true;
                    Ok(Value::Null)
                }
                None => Ok(Value::Boolean(st.borrow().lookup)),
            },
            // `P.SetRecord(Rec)` hands the page variable the record it shows when it runs (corpus
            // handlers/TestPageSetRecord.al).
            PageMethod::SetRecord => match args.first() {
                Some(Value::Record(r)) => {
                    // Inside a Page-typed modal handler the page holds the record already
                    // (handlers/PSModalPageHandlerTyped.al: GetRecord answers the record SetRecord handed).
                    if st.borrow().typed_handler && self.has_source(st) {
                        let rec = self.page_rec(st);
                        rec.borrow_mut().fields = r.borrow().fields.clone();
                        return Ok(Value::Null);
                    }
                    let kept = st.borrow().set_record.clone().filter(|_| st.borrow().set_view);
                    if let Some(view) = kept.filter(|v| v.borrow().table == r.borrow().table) {
                        view.borrow_mut().fields = r.borrow().fields.clone();
                        return Ok(Value::Null);
                    }
                    let copy = Rc::new(RefCell::new(r.borrow().clone()));
                    let mut s = st.borrow_mut();
                    s.set_record = Some(copy);
                    s.set_view = false;
                    Ok(Value::Null)
                }
                other => unsupported(format!("Page.SetRecord with {}", other.describe())),
            },
            // `CurrPage.SetSelectionFilter(var Rec)` on a list the test client stands on a row of: the record is
            // filtered to that row (handlers/TestPageCloseTriggersSelectionCardPageContracts.al:239-250). A
            // record that already has filters, and a page on no row, are not pinned.
            PageMethod::SetSelectionFilter => {
                let (Some(Value::Record(dst)), Some(inst)) = (args.first(), st.borrow().instance.clone()) else {
                    return unsupported("Page.SetSelectionFilter of a page that never ran (not pinned)");
                };
                let object = st.borrow().object as usize;
                let Some(slot) = self.engine.program.objects[object].rec_slot else {
                    return unsupported("Page.SetSelectionFilter of a page without a source table (not pinned)");
                };
                let Value::Record(src) = inst.borrow().globals[slot].clone() else {
                    return unsupported("Page.SetSelectionFilter: the page's Rec is no record");
                };
                if src.borrow().table != dst.borrow().table {
                    return unsupported("Page.SetSelectionFilter into a record of another table (not pinned)");
                }
                if st.borrow().cursor != Cursor::Row {
                    return unsupported("Page.SetSelectionFilter of a page on no row (not pinned)");
                }
                if !dst.borrow().filters.is_empty() {
                    return unsupported("Page.SetSelectionFilter into a record that has filters (not pinned)");
                }
                let fields = src.borrow().fields.clone();
                dst.borrow_mut().fields = fields;
                self.record_op(RecordOp::SetRecFilter, dst, &mut [])?;
                Ok(Value::Null)
            }
            // Which record a page variable reports after it ran is not pinned: no corpus test calls GetRecord
            // (docs/probes-needed.md). Refused when reached, like SetSelectionFilter.
            // `P.GetRecord(var Rec)` after the page ran: the row it stood on when it closed, the picked row after
            // OK and the last positioned row after Cancel too, without filters
            // (handlers/TestPageCloseTriggersSelectionCardPageContracts.al:250-285).
            PageMethod::GetRecord => {
                let (Some(Value::Record(dst)), Some(inst)) = (args.first(), st.borrow().instance.clone()) else {
                    return unsupported("Page.GetRecord of a page that never ran (not pinned)");
                };
                let object = st.borrow().object as usize;
                let Some(slot) = self.engine.program.objects[object].rec_slot else {
                    return unsupported("Page.GetRecord of a page without a source table (not pinned)");
                };
                let Value::Record(src) = inst.borrow().globals[slot].clone() else {
                    return unsupported("Page.GetRecord: the page's Rec is no record");
                };
                if src.borrow().table != dst.borrow().table {
                    return unsupported("Page.GetRecord into a record of another table (not pinned)");
                }
                if st.borrow().cursor != Cursor::Row {
                    return unsupported("Page.GetRecord of a page that closed on no row (not pinned)");
                }
                let fields = src.borrow().fields.clone();
                dst.borrow_mut().fields = fields;
                Ok(Value::Null)
            }
            // `P.SetTableView(Rec)` hands the page variable the view of Rec (filters and sorting) but not its
            // position, so the page opens on its first row (corpus handlers/TestPageModalNewRowLine_Tests.al:
            // ReEditable_LookupModeTrue_HasNoNewRowLine, Plain_LookupModeFalse_HasTheNewRowLine).
            PageMethod::SetTableView => match args.first() {
                Some(Value::Record(r)) => {
                    let copy = Rc::new(RefCell::new(r.borrow().clone()));
                    let table = copy.borrow().table as usize;
                    // A record SetRecord handed the page earlier stays the position the page opens on: SetRecord gives the
                    // position and SetTableView the filters (Microsoft's VendEntry-Apply Posted Entries calls both with the
                    // same record and its page reads Rec in OnOpenPage; not pinned, docs/probes-needed.md).
                    let kept = st.borrow().set_record.as_ref().map(|p| p.borrow().fields.clone());
                    let fields = match kept {
                        Some(f) if f.len() == copy.borrow().fields.len() => {
                            al_builtins::assumptions::assume(
                                al_builtins::assumptions::Assumption::SetTableViewKeepsSetRecordPosition,
                            );
                            f
                        }
                        _ => self.initial_fields(table),
                    };
                    copy.borrow_mut().fields = fields;
                    let mut s = st.borrow_mut();
                    s.set_record = Some(copy);
                    s.set_view = true;
                    Ok(Value::Null)
                }
                other => unsupported(format!("Page.SetTableView with {}", other.describe())),
            },
            PageMethod::EnqueueBackgroundTask => {
                self.require_open(st)?;
                self.bg_enqueue(st, args)
            }
            PageMethod::CancelBackgroundTask => {
                self.require_open(st)?;
                self.bg_cancel(st, args)
            }
            PageMethod::RunPageBackgroundTask => {
                self.require_open(st)?;
                self.bg_run_page_task(st, args)
            }
            PageMethod::Run => {
                let record = st.borrow().set_record.clone();
                self.run_page_ui(st, record, false)
            }
            PageMethod::RunModal => {
                let record = st.borrow().set_record.clone();
                self.run_page_ui(st, record, true)
            }
            PageMethod::Close if method.recv() == PageRecv::Page => {
                self.require_open(st)?;
                let a = self.close_action(st, "Close");
                self.close_with(st, &a, false, false)
            }
            PageMethod::New => {
                self.require_open(st)?;
                self.new_record(st)
            }
            PageMethod::ValidationErrorCount => {
                self.require_open(st)?;
                Ok(Value::Integer(st.borrow().errors.len() as i32))
            }
            PageMethod::GetValidationError => {
                self.require_open(st)?;
                let i = match args.first() {
                    Some(Value::Integer(i)) => *i,
                    _ => return unsupported("GetValidationError without an index"),
                };
                let s = st.borrow();
                match usize::try_from(i - 1).ok().and_then(|n| s.errors.get(n)) {
                    Some((_, t)) => Ok(Value::Text(t.clone().into())),
                    // Index 0 is pinned (handlers/TestPageRuntimeContracts.al:224-236: text and code);
                    // an index past the last error is assumed to raise the same (docs/probes-needed.md).
                    None => Err(error_code("Index out of bounds.", "IndexOutOfBounds")),
                }
            }
            // A part the page's own code created without opening it (a literal `Visible = false`, `InvisiblePartForPageCode`) has no
            // client to refresh: `CurrPage.Update` in its procedure does nothing. Base Application's Approval FactBox ends
            // `UpdateApprovalEntriesFromSourceRecord` with it (assumption hidden-part-update-is-noop).
            PageMethod::Update if st.borrow().host.is_some() && !st.borrow().open => {
                al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::HiddenPartUpdateIsNoop);
                Ok(Value::Null)
            }
            PageMethod::Update | PageMethod::SaveRecord => {
                self.require_open(st)?;
                // CurrPage.Update() without an argument saves (the parameter defaults to true).
                let save = (method == PageOp::PageSaveRecord) || !matches!(args.first(), Some(Value::Boolean(false)));
                if !(method == PageOp::PageSaveRecord) {
                    // The refresh lands after the trigger that asked for it returns (corpus
                    // pageupdate/TestPageUpdate.al: ValidateBegin;ValidateEnd;HostAGCR).
                    st.borrow_mut().refresh = true;
                }
                if !save {
                    return Ok(Value::Null);
                }
                // Page code changed Rec: SaveRecord writes it whether or not the client typed anything
                // (corpus handlers/TestPageTriggerEvents.al, ModifyRecordEvent_PageDrivenSave_*).
                // Page code that inserted the new row itself and copied it into Rec hands the page
                // that row (corpus handlers/TestPageOpenNewAfterGetCurrRecord.al:44-63).
                if st.borrow().draft && !st.borrow().draft_stored && {
                    let rec = self.page_rec(st);
                    self.stored_row(&rec)?.is_some()
                } {
                    let mut s = st.borrow_mut();
                    s.draft = false;
                    s.dirty = false;
                    drop(s);
                    self.set_xrec(st);
                    return Ok(Value::Null);
                }
                if st.borrow().cursor == Cursor::Row {
                    st.borrow_mut().dirty = true;
                }
                self.save_row(st)?;
                Ok(Value::Null)
            }
            PageMethod::GoToRecord => {
                self.require_open(st)?;
                let Some(Value::Record(r)) = args.first() else {
                    return unsupported("GoToRecord without a record");
                };
                let table = self.table(r)?;
                let mut key: Vec<Value> = table.primary_key(&r.borrow().fields);
                self.page_go_to_key(st, &mut key)
            }
            other => unsupported(format!("TestPage.{other}")),
        }
    }

    // ---- opening and closing ------------------------------------------------------------
}
