//! The test client for pages (ADR-012): the part of BC's test framework that
//! opens a page, moves on its rows, reads and writes its controls and closes
//! it again. The page itself runs as ordinary AL (triggers, procedures,
//! property routines) on an `Instance` in this session.
//!
//! Every rule below cites the corpus test that pins it; behaviour no test
//! pins raises `Signal::Unsupported`.

use crate::{Engine, Session, UiKind};
use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::pagefmt::{self, Blank, ControlFormat};
use al_builtins::value::{ControlRef, ControlTarget, Cursor, PageMode, PageState, RecordState, RtType, Value};
use al_ir as ir;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

type St = Rc<RefCell<PageState>>;
type Rec = Rc<RefCell<RecordState>>;

/// corpus testpageteardown/TestPageTeardown.al:227, testpageclose/TestPageClosingActions.al:338
const NOT_OPEN: &str = "The TestPage is not open.";
/// corpus testpageclose/TestPageClosingActions.al:339
const ALREADY_OPEN: &str = "The TestPage is already open.";

enum Left {
    Go,
    Stay,
}

fn error(message: impl Into<String>) -> Signal {
    Signal::Error(AlError {
        message: message.into(),
        code: String::new(),
        call_stack: String::new(),
    })
}

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn page_of<'a>(engine: &'a Engine, st: &St) -> &'a ir::Page {
    let object = st.borrow().object as usize;
    engine.program.objects[object]
        .page
        .as_ref()
        .expect("a TestPage is typed by a page object")
}

/// `'text'` → `text`; anything else as written.
fn unquote(s: &str) -> String {
    let t = s.trim();
    match t.strip_prefix('\'').and_then(|x| x.strip_suffix('\'')) {
        Some(inner) => inner.replace("''", "'"),
        None => t.to_string(),
    }
}

fn is_true(s: &str) -> bool {
    s.trim().eq_ignore_ascii_case("true")
}

type Handler = (usize, usize, Rc<RefCell<al_builtins::value::Instance>>);

mod filter;
mod parts;
mod request;
pub(crate) use request::RequestClosed;
mod runobject;

impl Session {
    /// `TP.Ctl`: a handle on control `control` of the test page `page`.
    pub(crate) fn page_control(&mut self, page: Value, control: usize) -> Result<Value, Signal> {
        match page {
            Value::TestPage(p) => {
                // A part control reads as the page it shows (a `TestPart`).
                let is_part = {
                    let engine = self.engine();
                    page_of(&engine, &p).controls[control].kind == ir::ControlKind::Part
                };
                if is_part {
                    return self.part_page(&p, control).map(Value::TestPage);
                }
                Ok(Value::TestControl(Rc::new(ControlRef {
                    page: p,
                    target: ControlTarget::Control(control),
                })))
            }
            other => Err(Signal::Unsupported(format!("control of {other:?}"))),
        }
    }

    fn engine(&self) -> Arc<Engine> {
        self.engine.clone()
    }

    /// The page's `Rec`.
    fn page_rec(&self, st: &St) -> Rec {
        let object = st.borrow().object as usize;
        let slot = self.engine.program.objects[object]
            .rec_slot
            .expect("page with a source table");
        let inst = st.borrow().instance.clone().expect("open page");
        let v = inst.borrow().globals[slot].clone();
        match v {
            Value::Record(r) => r,
            other => panic!("Rec is {other:?}"),
        }
    }

    fn require_open(&self, st: &St) -> Result<(), Signal> {
        if st.borrow().open { Ok(()) } else { Err(error(NOT_OPEN)) }
    }

    /// Runs page trigger `name` (lower case) if the page declares it.
    fn page_trigger(&mut self, st: &St, name: &str, args: Vec<Value>) -> Result<Option<Value>, Signal> {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let Some(r) = page_of(&engine, st)
            .triggers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, r)| *r)
        else {
            return Ok(None);
        };
        let inst = st.borrow().instance.clone().expect("open page");
        let result = self.call(object, r, inst, args).map(|(v, _)| Some(v));
        self.end_page_call(st);
        result
    }

    /// A page-driven call ends: the client takes the page's view back, which carries the key and
    /// `Ascending` but not a per-field `SetAscending` (corpus handlers/TestPageViewRoundTripRoutes.al,
    /// Action_SetAscending_ReadInTheSameTrigger_IsDescending against ..._ReadByALaterAction).
    fn end_page_call(&mut self, st: &St) {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        if engine.program.objects[object].rec_slot.is_none() {
            return;
        }
        if st.borrow().instance.is_some() {
            self.page_rec(st).borrow_mut().field_descending = false;
        }
    }

    /// A page trigger that answers Boolean: `Some(answer)` when the trigger exists and can state one,
    /// `None` when it is absent or never executes `exit(value)`. A trigger that falls off its end
    /// without an exit does not veto (corpus pageupdate/PageUpdateNewRowCard.Page.al: OnInsertRecord
    /// assigns the key and exits nothing, and the row is saved; handlers/TrtCardNoInsert.Page.al
    /// exits false and it is not).
    fn page_trigger_answer(&mut self, st: &St, name: &str, args: Vec<Value>) -> Result<Option<bool>, Signal> {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let routine = page_of(&engine, st)
            .triggers
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, r)| *r);
        let states_answer = routine.is_some_and(|r| has_value_exit(&engine.program.objects[object].routines[r].body));
        let v = self.page_trigger(st, name, args)?;
        Ok(match v {
            Some(Value::Boolean(b)) if states_answer => Some(b),
            _ => None,
        })
    }

    /// Raises the platform event `event` that BC publishes after the page's own trigger of that name
    /// (corpus handlers/TestPageTriggerEvents.al:1-20): the page's own trigger runs first, the event is
    /// the platform's last word. `allow` is the `var Allow...` parameter some of them carry; the
    /// returned flag is what the subscribers left in it.
    fn page_event(&mut self, st: &St, event: &str, allow: Option<bool>, below: Option<bool>) -> Result<bool, Signal> {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        if !engine.events.is_subscribed(object, event) {
            return Ok(allow.unwrap_or(true));
        }
        if engine.program.objects[object].rec_slot.is_none() {
            return unsupported(format!("page event {event} on a page without a source table"));
        }
        let rec = Value::Record(self.page_rec(st));
        let xobj = &engine.program.objects[object];
        let xrec = {
            let inst = st.borrow().instance.clone().expect("open page");
            let v = xobj.xrec_slot.map(|x| inst.borrow().globals[x].clone());
            v.unwrap_or(Value::Null)
        };
        let mut names = vec!["Rec".to_string()];
        let mut args = vec![rec];
        match event {
            "onopenpageevent" | "onclosepageevent" | "onaftergetcurrrecordevent" => {}
            "onqueryclosepageevent" => {
                names.push("AllowClose".into());
                args.push(Value::Boolean(true));
            }
            "onnewrecordevent" => {
                names.extend(["BelowxRec".into(), "xRec".into()]);
                args.extend([Value::Boolean(below.unwrap_or(false)), xrec]);
            }
            "oninsertrecordevent" => {
                names.extend(["BelowxRec".into(), "xRec".into(), "AllowInsert".into()]);
                args.extend([Value::Boolean(below.unwrap_or(false)), xrec, Value::Boolean(true)]);
            }
            "onmodifyrecordevent" => {
                names.extend(["xRec".into(), "AllowModify".into()]);
                args.extend([xrec, Value::Boolean(true)]);
            }
            other => return unsupported(format!("page event {other}")),
        }
        let call = crate::dispatch::EventCall {
            object,
            event,
            element: None,
            isolated: false,
            names: &names,
            sender: None,
        };
        self.raise(&call, &mut args)?;
        Ok(match (allow, args.last()) {
            (Some(_), Some(Value::Boolean(b))) => *b,
            _ => true,
        })
    }

    /// Refuses a page some subscriber listens to for an event the client does not raise yet.
    fn check_page_events(&self, st: &St) -> Result<(), Signal> {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        const RAISED: [&str; 7] = [
            "onopenpageevent",
            "onqueryclosepageevent",
            "onclosepageevent",
            "onaftergetcurrrecordevent",
            "onnewrecordevent",
            "oninsertrecordevent",
            "onmodifyrecordevent",
        ];
        for ((o, ev), subs) in &engine.events.subscribers {
            if *o == object
                && !subs.is_empty()
                && ev.starts_with("on")
                && ev.ends_with("event")
                && !RAISED.contains(&ev.as_str())
            {
                // Events a page publishes itself (IntegrationEvent) are ordinary routines.
                let published = engine
                    .events
                    .publishers
                    .values()
                    .any(|p| p.object == object && p.event == *ev);
                if !published {
                    return unsupported(format!("page event {ev} is not raised by the test client yet"));
                }
            }
        }
        Ok(())
    }

    /// Calls routine `r` of the page.
    fn page_routine(&mut self, st: &St, r: usize) -> Result<Value, Signal> {
        let object = st.borrow().object as usize;
        let inst = st.borrow().instance.clone().expect("open page");
        let result = self.call(object, r, inst, vec![]).map(|(v, _)| v);
        self.end_page_call(st);
        result
    }

    /// The `TestPage.*` / `TestField.*` methods (ADR-012).
    pub(crate) fn page_op(
        &mut self,
        method: &str,
        recv: Option<&mut Value>,
        args: &mut [Value],
    ) -> Result<Value, Signal> {
        // A TestPage begins a transaction of its own (see `in_page_transaction`); running a page
        // with no TestPage, from a test with no transaction, is not pinned.
        if recv.is_none() && self.no_transaction() {
            return unsupported(format!("{method} under TransactionModel::None (not pinned)"));
        }
        if recv.is_some() && self.no_transaction() {
            return self.in_page_transaction(|s| s.page_op_inner(method, recv, args));
        }
        self.page_op_inner(method, recv, args)
    }

    fn page_op_inner(&mut self, method: &str, recv: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
        match recv {
            None if method == "Page.RunStatic" || method == "Page.RunModalStatic" => {
                let n = match args.first() {
                    Some(Value::Integer(n)) => *n,
                    other => return unsupported(format!("Page.Run with {other:?} as the page")),
                };
                let engine = self.engine();
                let record = match args.get(1) {
                    Some(Value::Record(r)) => Some(r.clone()),
                    None => None,
                    other => return unsupported(format!("Page.Run with {other:?} as the record")),
                };
                if args.len() > 2 {
                    return unsupported("Page.Run with a field argument");
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
                                o.kind == "page"
                                    && (o.name.eq_ignore_ascii_case(&l) || o.id.is_some_and(|i| i.to_string() == l))
                            });
                            if let Some(i) = found.and_then(|o| o.id) {
                                id = i;
                            }
                        }
                    }
                }
                let Some(object) = engine
                    .program
                    .objects
                    .iter()
                    .position(|o| o.kind == "page" && o.id == Some(id))
                else {
                    return Err(error(format!("You tried to invoke the Page object with the ID {n}")));
                };
                let st = Rc::new(RefCell::new(PageState::closed(object as u32)));
                // RunModal(id, Record) is the lookup overload: OK reads LookupOK, Cancel LookupCancel
                // (corpus handlers/TestPageModalHandlerStatic_Tests.al:48-72).
                if method == "Page.RunModalStatic" && record.is_some() {
                    st.borrow_mut().lookup = true;
                }
                self.run_page_ui(&st, record, method == "Page.RunModalStatic")
            }
            Some(Value::TestPage(st)) => {
                let st = st.clone();
                self.test_page_op(method, &st, args)
            }
            Some(Value::TestControl(c)) => {
                let c = c.clone();
                self.test_control_op(method, &c, args)
            }
            other => unsupported(format!("{method} on {other:?}")),
        }
    }

    fn test_page_op(&mut self, method: &str, st: &St, args: &mut [Value]) -> Result<Value, Signal> {
        let name = method.split_once('.').map(|(_, m)| m).unwrap_or(method);
        let r = self.test_page_op_inner(method, st, args)?;
        // A move of the host re-points its parts; a move of a part re-points the parts that take
        // their link from it (corpus testpart/TestPartProviderMove.al).
        let moved = match name {
            "First" | "Last" | "Next" | "Previous" | "GoToKey" | "GoToRecord" | "FindFirstField" | "FindNextField"
            | "FindPreviousField" => !matches!(r, Value::Boolean(false)),
            "New" => true,
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

    fn test_page_op_inner(&mut self, method: &str, st: &St, args: &mut [Value]) -> Result<Value, Signal> {
        match method.split_once('.').map(|(_, m)| m).unwrap_or(method) {
            // `TestPart.Visible` / `.Enabled` read the part control of the host: a part that is
            // reachable at all is visible (corpus testpart/TestTestPart.al:TestPart_Visible_*).
            "Visible" if method.starts_with("TestPart.") => {
                self.require_open(st)?;
                Ok(Value::Boolean(true))
            }
            "Enabled" if method.starts_with("TestPart.") => {
                self.require_open(st)?;
                let host = st.borrow().host.clone();
                match host.and_then(|(h, i)| h.upgrade().map(|h| (h, i))) {
                    Some((h, i)) => Ok(Value::Boolean(self.control_enabled(&h, i)?)),
                    None => unsupported("TestPart.Enabled on a page that is not a part"),
                }
            }
            // The id is a compiler-generated control id; the corpus pins only that a table field
            // number is not one (testpart/TestTestPart.al:TestPart_GetField_TakesAControlIdNotATableFieldNo).
            "Filter" => {
                self.require_open(st)?;
                Ok(self.filter_handle(st))
            }
            "GetField" => {
                self.require_open(st)?;
                match args.first() {
                    Some(Value::Integer(n)) => Err(error(format!("The field with ID = {n} is not found on the page."))),
                    _ => unsupported("TestPart.GetField without an id"),
                }
            }
            "OpenEdit" => self.page_open(st, PageMode::Edit),
            "OpenView" => self.page_open(st, PageMode::View),
            "OpenNew" => self.page_open(st, PageMode::New),
            "Close" if !method.starts_with("Page.") => self.page_close(st),
            m @ ("First" | "Last" | "Next" | "Previous") => {
                self.require_open(st)?;
                self.page_move(st, m)
            }
            "GoToKey" => {
                self.require_open(st)?;
                self.page_go_to_key(st, args)
            }
            m @ ("FindFirstField" | "FindNextField" | "FindPreviousField") => {
                self.require_open(st)?;
                self.page_find_field(st, m, args)
            }
            // A page that is not a tree has nothing to expand (docs only; the corpus calls them,
            // TestPageAdvanced.al:104, :283, asserting nothing about the answer).
            "Expand" => {
                self.require_open(st)?;
                Ok(Value::Null)
            }
            "IsExpanded" => {
                self.require_open(st)?;
                Ok(Value::Boolean(false))
            }
            m @ ("OK" | "Cancel" | "Yes" | "No") => {
                self.require_open(st)?;
                if !self.builtin_offered(st, m) {
                    return Err(error(format!("The built-in action = {m} is not found on the page.")));
                }
                Ok(Value::TestControl(Rc::new(ControlRef {
                    page: st.clone(),
                    target: ControlTarget::BuiltIn(m.to_string()),
                })))
            }
            m @ ("Edit" | "View") => {
                self.require_open(st)?;
                Ok(Value::TestControl(Rc::new(ControlRef {
                    page: st.clone(),
                    target: ControlTarget::BuiltIn(m.to_string()),
                })))
            }
            "Caption" => {
                self.require_open(st)?;
                let engine = self.engine();
                let object = st.borrow().object as usize;
                let o = &engine.program.objects[object];
                let cap = o
                    .properties
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("Caption"))
                    .map(|(_, v)| unquote(v))
                    .unwrap_or_else(|| o.name.clone());
                Ok(Value::Text(cap))
            }
            "Editable" if method.starts_with("Page.") => {
                self.require_open(st)?;
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
            "Editable" => {
                self.require_open(st)?;
                Ok(Value::Boolean(self.page_editable(st)))
            }
            "Trap" => {
                // The next plain Page.Run of this page type opens into this variable
                // (corpus handlers/TestPageRunHandler_Tests.al:TrapReceivesTheNonModalPageAndItStaysOpen).
                st.borrow_mut().trapping = true;
                self.trap = Some(st.clone());
                Ok(Value::Null)
            }
            "LookupMode" => match args.first() {
                Some(v) => {
                    st.borrow_mut().lookup = ops::truthy(v)?;
                    Ok(Value::Null)
                }
                None => Ok(Value::Boolean(st.borrow().lookup)),
            },
            // `P.SetRecord(Rec)` hands the page variable the record it shows when it runs (corpus
            // handlers/TestPageSetRecord.al).
            "SetRecord" => match args.first() {
                Some(Value::Record(r)) => {
                    let copy = Rc::new(RefCell::new(r.borrow().clone()));
                    st.borrow_mut().set_record = Some(copy);
                    Ok(Value::Null)
                }
                other => unsupported(format!("Page.SetRecord with {other:?}")),
            },
            "Run" => {
                let record = st.borrow().set_record.clone();
                self.run_page_ui(st, record, false)
            }
            "RunModal" => {
                let record = st.borrow().set_record.clone();
                self.run_page_ui(st, record, true)
            }
            "Close" if method.starts_with("Page.") => {
                self.require_open(st)?;
                let a = self.close_action(st, "Close");
                self.close_with(st, &a, false, false)
            }
            "New" => {
                self.require_open(st)?;
                self.new_record(st)
            }
            "ValidationErrorCount" => {
                self.require_open(st)?;
                Ok(Value::Integer(st.borrow().errors.len() as i32))
            }
            "GetValidationError" => {
                self.require_open(st)?;
                let i = match args.first() {
                    Some(Value::Integer(i)) => *i,
                    _ => return unsupported("GetValidationError without an index"),
                };
                let s = st.borrow();
                match usize::try_from(i - 1).ok().and_then(|n| s.errors.get(n)) {
                    Some((_, t)) => Ok(Value::Text(t.clone())),
                    None => Err(error(format!("The index {i} is out of range."))),
                }
            }
            "Update" | "SaveRecord" => {
                self.require_open(st)?;
                // CurrPage.Update() without an argument saves (the parameter defaults to true).
                let save = method.ends_with("SaveRecord") || !matches!(args.first(), Some(Value::Boolean(false)));
                if !method.ends_with("SaveRecord") {
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
            "GoToRecord" => {
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

    fn page_open(&mut self, st: &St, mode: PageMode) -> Result<Value, Signal> {
        // corpus testpageclose/TestPageClosingActions.al:19-24: an open variable cannot be opened.
        if st.borrow().open {
            return Err(error(ALREADY_OPEN));
        }
        // A reopened variable shows a fresh page.
        st.borrow_mut().instance = None;
        st.borrow_mut().modal = false;
        self.open_page(st, mode, None, None).map(|_| Value::Null)
    }

    /// Whether the page has a source table (and so a `Rec`).
    fn has_source(&self, st: &St) -> bool {
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
        st.borrow_mut().instance = Some(inst.clone());
        Ok(inst)
    }

    /// Opens the page: OnInit, OnOpenPage, the control expressions and the first row. `record`
    /// is the record AL code handed to `Page.Run`/`SetRecord`: its filters and position become the
    /// page's view.
    ///
    /// With `bind`, the UI handler for the page is looked up after OnInit and before OnOpenPage
    /// (corpus handlers/TestPageOnInit_Tests.al: an OnInit error surfaces as itself even where no
    /// handler is declared).
    fn open_page(
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
    fn open_page_as(
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
        self.page_instance(st)?;
        {
            let mut s = st.borrow_mut();
            s.open = true;
            s.mode = mode;
            s.cursor = Cursor::None;
            s.draft = false;
            s.dirty = false;
            s.frozen_visible = Vec::new();
            s.window = Vec::new();
            s.focus = None;
            s.focus_in_row = false;
            s.errors = Vec::new();
            s.refused = false;
            s.pending = None;
            s.result = None;
            s.editable_off = false;
            s.opening = true;
            s.parts = Vec::new();
        }
        if let Err(e) = self.create_parts(st) {
            self.mark_closed(st);
            return Err(e);
        }
        let has_source = self.has_source(st);
        // The rowset a page serves from OnFindRecord / OnNextRecord is not modelled: refused rather
        // than walked as the table (corpus pagefindrecord/TestPageFindRecord.al).
        if let Some(t) = page
            .triggers
            .iter()
            .find(|(n, _)| n == "onfindrecord" || n == "onnextrecord")
        {
            self.mark_closed(st);
            return unsupported(format!("a page with an {} trigger (the rowset page code serves)", t.0));
        }
        // SourceTableView is the page's own view of its table (corpus
        // pagefindrecord/TestPageFindRecord.al:192 PlainPage_WithoutTheTriggers_WalksTheTableDescending);
        // how it combines with the view of a record handed to Page.Run is not pinned.
        let source_view = engine.program.objects[st.borrow().object as usize]
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("SourceTableView"))
            .map(|(_, v)| v.trim().to_string());
        // The part control's SubPageView comes first: its filters land in the link group, its
        // sorting and order set the key and direction. The part page's own SourceTableView then
        // sets only the halves it names (corpus handlers/TestPartOwnSortVsSubPageView.al:1-45).
        let sub_view = self.sub_page_view(st);
        if let (Some(view), true) = (&sub_view, has_source) {
            let rec = self.page_rec(st);
            let table = self.table(&rec)?;
            self.set_view_in(&rec, &table, view, Some(parts::LINK_GROUP), false, true)?;
        }
        if let (Some(view), true) = (source_view, has_source) {
            if record.is_some() {
                self.mark_closed(st);
                return unsupported("Page.Run with a record on a page that declares SourceTableView");
            }
            let rec = self.page_rec(st);
            let table = self.table(&rec)?;
            self.set_view_in(&rec, &table, &view, None, sub_view.is_some(), false)?;
        }
        if let (Some(r), true) = (&record, has_source) {
            let rec = self.page_rec(st);
            let src = r.borrow();
            let mut dst = rec.borrow_mut();
            dst.fields = src.fields.clone();
            dst.filters = src.filters.clone();
            dst.key = src.key;
            dst.temporary = src.temporary;
            dst.temp_id = src.temp_id;
            dst.ascending = src.ascending;
        }
        let result = (|| {
            self.page_trigger(st, "oninit", vec![])?;
            if let Some(kind) = bind {
                handler = Some(self.bind_page_handler(kind, st.borrow().object)?);
            }
            self.restore_page_values(st);
            self.check_page_events(st)?;
            let opened = self.page_trigger(st, "onopenpage", vec![]);
            st.borrow_mut().opening = false;
            opened?;
            self.page_event(st, "onopenpageevent", None, None)?;
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
            self.sync_parts(st)
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
    fn load_first_row(&mut self, st: &St, mode: PageMode, with_record: bool) -> Result<(), Signal> {
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
            self.ret_used = true;
            let rec = self.page_rec(st);
            self.record_op("FindLast", &rec, &mut [])?;
            return self.start_draft(st, false);
        }
        if with_record {
            // The page opens on the record the caller handed it, when that row exists
            // (corpus handlers/TestPageRunHandler_Tests.al:PageHandlerReceivesTheRecordPassedToPageRun).
            let rec = self.page_rec(st);
            if self.stored_row(&rec)?.is_some() {
                self.ret_used = true;
                let mut pk = self.table(&rec)?.primary_key(&rec.borrow().fields);
                if matches!(self.record_op("Get", &rec, &mut pk)?, Value::Boolean(true)) {
                    st.borrow_mut().cursor = Cursor::Row;
                    return self.arrive(st);
                }
            }
        }
        self.page_first_row(st)
    }

    fn save_values(&self, st: &St) -> bool {
        let object = st.borrow().object as usize;
        self.engine.program.objects[object]
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("SaveValues") && v.trim().eq_ignore_ascii_case("true"))
    }

    fn var_slots(&self, st: &St) -> Vec<usize> {
        let engine = self.engine();
        let mut slots: Vec<usize> = page_of(&engine, st)
            .controls
            .iter()
            .filter_map(|c| {
                if let ir::Binding::Var(slot) = c.binding {
                    Some(slot)
                } else {
                    None
                }
            })
            .collect();
        slots.sort_unstable();
        slots.dedup();
        slots
    }

    fn store_page_values(&mut self, st: &St) {
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
        self.saved_values.insert(object, saved);
    }

    fn restore_page_values(&mut self, st: &St) {
        let object = st.borrow().object;
        let Some(saved) = self.saved_values.get(&object).cloned() else {
            return;
        };
        let Some(inst) = st.borrow().instance.clone() else {
            return;
        };
        for (slot, v) in saved {
            inst.borrow_mut().globals[slot] = v;
        }
    }

    /// `TestPage.Close()`: the page closes the way a Cancel does (corpus
    /// testpageclose/TestPageClosingActions.al:Dialog arms, `qcp:Cancel` after Close()).
    fn page_close(&mut self, st: &St) -> Result<Value, Signal> {
        let r = self.close_with(st, "Cancel", false, false);
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
    fn close_with(
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
            self.record_op("Insert", &rec, &mut [Value::Boolean(true)])?;
        }
        match self.save_row(st) {
            Err(Signal::Error(e)) if swallow_save_errors => {
                let key_ctl = self.key_control(st);
                st.borrow_mut().errors.push((key_ctl, e.message));
                return Ok(Value::Null);
            }
            other => other?,
        }
        let members: std::rc::Rc<[String]> = al_builtins::value::ACTION_MEMBERS
            .iter()
            .map(|m| m.to_string())
            .collect();
        let ordinal = al_builtins::value::ACTION_MEMBERS
            .iter()
            .position(|m| *m == action)
            .unwrap_or(0) as i32;
        // An AL error raised in OnQueryClosePage is shown as a message and refuses the close
        // (corpus handlers/TestPageQueryCloseError_Tests.al:1-25).
        let veto = match self.page_trigger_answer(st, "onqueryclosepage", vec![Value::Option(ordinal, members)]) {
            Err(Signal::Error(e)) => {
                self.ui_message(e.message)?;
                return Ok(Value::Null);
            }
            other => other?,
        };
        if veto == Some(false) {
            return Ok(Value::Null);
        }
        if !self.page_event(st, "onqueryclosepageevent", Some(true), None)? {
            return Ok(Value::Null);
        }
        self.page_trigger(st, "onclosepage", vec![])?;
        self.page_event(st, "onclosepageevent", None, None)?;
        // SaveValues: what the page's variable-bound controls hold is stored unless the Cancel
        // action closed it (corpus handlers/TestPageSaveValues.al, testpageclose/TestPageClosingActions.al:937-977;
        // Close() and a platform close store too, docs/probes-needed.md).
        if !(via_action && matches!(action, "Cancel" | "LookupCancel" | "No")) {
            self.store_page_values(st);
        }
        self.mark_closed(st);
        st.borrow_mut().result = Some(action.to_string());
        Ok(Value::Null)
    }

    /// The built-in actions a page type offers (corpus handlers/TestPageBuiltInActions_Tests.al:1-30,
    /// testpageclose/TestPageClosingActions.al:25-30: a card or a list offers OK and no Cancel; a
    /// StandardDialog both; a NavigatePage only OK; a PromptDialog Cancel, and OK unless it declares
    /// systemaction(OK); a ConfirmationDialog neither).
    fn builtin_offered(&self, st: &St, which: &str) -> bool {
        if self.is_request_page(st) {
            return self.request_builtin_offered(st, which);
        }
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let page_type = engine.program.objects[object]
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("PageType"))
            .map(|(_, v)| v.trim().to_ascii_lowercase())
            .unwrap_or_default();
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
    fn offers_new_line(&mut self, st: &St) -> Result<bool, Signal> {
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
            && !prop_false("Editable")
            && !prop_false("InsertAllowed"))
    }

    // ---- rows ---------------------------------------------------------------------------

    /// Lands on the first row of the view when the page opens: a stored row, else
    /// the new-row line, else nothing. The expressions of the controls are evaluated for
    /// an empty view too (corpus autoformat/TestAutoFormatExpressionTiming.al:5-12).
    fn page_first_row(&mut self, st: &St) -> Result<(), Signal> {
        let rec = self.page_rec(st);
        self.ret_used = true;
        let found = self.record_op("FindFirst", &rec, &mut [])?;
        if matches!(found, Value::Boolean(true)) {
            st.borrow_mut().cursor = Cursor::Row;
            self.arrive(st)
        } else if st.borrow().new_line {
            self.land_on_new_line(st)?;
            self.populate_expressions(st)
        } else {
            self.populate_expressions(st)
        }
    }

    fn land_on_new_line(&mut self, st: &St) -> Result<(), Signal> {
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
        // handlers/TestPagePartOnNewRecordCount.al). A top-level list is not pinned.
        if st.borrow().host.is_some() {
            self.stamp_from_filters(st, true)?;
            self.page_trigger(st, "onnewrecord", vec![Value::Boolean(true)])?;
            self.page_event(st, "onnewrecordevent", None, Some(true))?;
        }
        Ok(())
    }

    /// Remembers the stored row the cursor stands on as the lower neighbour of the row about to
    /// start, for AutoSplitKey.
    fn note_lower_key(&mut self, st: &St) -> Result<(), Signal> {
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
    fn stamp_from_filters(&mut self, st: &St, validate: bool) -> Result<(), Signal> {
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
            let [Some(v)] = single.as_slice() else { continue };
            stamped.push((pos, v.clone()));
        }
        for (pos, v) in stamped {
            rec.borrow_mut().fields[pos] = v.clone();
            if validate {
                self.ret_used = false;
                self.record_op("Validate", &rec, &mut [Value::Integer(pos as i32), v])?;
            }
        }
        Ok(())
    }

    /// AutoSplitKey: the last field of the primary key of a new line is numbered from its
    /// neighbours when it is saved (corpus handlers/TestPageAutoSplitKey_Tests.al,
    /// TestPageAutoSplitKeyRanges_Tests.al). Only the shapes the corpus pins are numbered; any
    /// other raises Signal::Unsupported.
    fn auto_split_key(&mut self, st: &St) -> Result<(), Signal> {
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
            return unsupported(format!("AutoSplitKey on a key field of type {ty:?}"));
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
                if matches!(self.record_op("FindFirst", &probe, &mut [])?, Value::Boolean(true)) {
                    dec(&probe.borrow().fields[last])
                } else {
                    None
                }
            }
        };
        use rust_decimal::Decimal as D;
        let step = D::from(10000);
        let number = match (lower, upper) {
            // An empty grid burns one interval on the blank placeholder row.
            (None, _) => D::from(20000),
            (Some(l), None) if l >= D::ZERO => l + step,
            (Some(l), None) if l == D::from(-10000) => l + (-l / D::from(3)).trunc(),
            (Some(l), Some(u)) if l >= D::ZERO && u > l => {
                if matches!(ty, RtType::Decimal) {
                    return unsupported("AutoSplitKey between two lines of a Decimal key");
                }
                let half = ((u - l) / D::from(2)).trunc();
                if half < D::ONE {
                    return unsupported("AutoSplitKey between two adjacent lines");
                }
                l + half.min(step)
            }
            (Some(l), Some(u)) if l == D::from(-10000) && u == D::from(10000) => D::from(-1),
            (l, u) => {
                return unsupported(format!(
                    "AutoSplitKey between {l:?} and {u:?} (the corpus does not pin it)"
                ));
            }
        };
        rec.borrow_mut().fields[last] = ops::convert(Value::Decimal(number), &ty)?;
        Ok(())
    }

    /// The expressions every row population evaluates, whatever the control's Visible
    /// (corpus autoformat/TestAutoFormatExpressionTiming.al:5-12): AutoFormatExpression and
    /// CaptionClass. They run after OnOpenPage and before OnAfterGetRecord.
    fn populate_expressions(&mut self, st: &St) -> Result<(), Signal> {
        let engine = self.engine();
        let page = page_of(&engine, st);
        for c in &page.controls {
            for name in ["autoformatexpression", "captionclass"] {
                match c.prop(name) {
                    Some(ir::PropVal::Routine(r)) => {
                        let v = self.page_routine(st, *r)?;
                        if name == "captionclass" {
                            if let Value::Text(t) | Value::Code(t) = &v {
                                if !t.starts_with("3,") {
                                    return unsupported(format!("CaptionClass `{t}` is resolved by the platform"));
                                }
                            }
                        }
                    }
                    // An area no built-in resolver claims goes to the Caption Class codeunit's
                    // OnResolveCaptionClass event (corpus autoformat/TestCaptionClassResolverFailure.al:1-12).
                    Some(ir::PropVal::Lit(l)) if name == "captionclass" && !unquote(l).starts_with("3,") => {
                        return unsupported(format!("CaptionClass `{}` is resolved by the platform", unquote(l)));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// The FlowFields the page shows are calculated when a row loads. A temporary source row
    /// reads them from the database tables (corpus handlers/TestPageRunTempRecordStoredRow_Tests.al:
    /// TempRecordFlowFieldIsCalculatedFromTheDatabase).
    fn calc_row_flowfields(&mut self, st: &St) -> Result<(), Signal> {
        if !self.has_source(st) {
            return Ok(());
        }
        let engine = self.engine();
        let rec = self.page_rec(st);
        let table = rec.borrow().table as usize;
        let Some(def) = engine.program.objects[table].table.as_ref() else {
            return Ok(());
        };
        let mut positions: Vec<usize> = page_of(&engine, st)
            .controls
            .iter()
            .filter_map(|c| match c.binding {
                ir::Binding::Field(p) if def.fields[p].calc.is_some() => Some(p),
                _ => None,
            })
            .collect();
        positions.sort_unstable();
        positions.dedup();
        if positions.is_empty() {
            return Ok(());
        }
        if !rec.borrow().temporary {
            return self.calc_fields(&rec, &positions);
        }
        let probe = {
            let mut p = rec.borrow().clone();
            p.temporary = false;
            p.auto_calc.clear();
            Rc::new(RefCell::new(p))
        };
        for p in positions {
            self.check_calc_field(table, p)?;
            let v = self.calc_value(&probe, p, &mut Vec::new())?;
            rec.borrow_mut().fields[p] = v;
        }
        Ok(())
    }

    /// Populates the current row: the control expressions, then OnAfterGetRecord.
    fn populate_current(&mut self, st: &St) -> Result<(), Signal> {
        st.borrow_mut().dirty = false;
        self.calc_row_flowfields(st)?;
        self.set_xrec(st);
        self.populate_expressions(st)?;
        self.page_trigger(st, "onaftergetrecord", vec![])?;
        Ok(())
    }

    /// PageType = List (a worksheet or ListPlus page with a repeater is not one; corpus
    /// handlers/TestPageControlEditableRoutes.al:Worksheet_RunModal_LookupMode).
    fn is_list_page(&self, st: &St) -> bool {
        let object = st.borrow().object as usize;
        self.engine.program.objects[object]
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("PageType") && v.trim().eq_ignore_ascii_case("list"))
    }

    fn is_list(&self, st: &St) -> bool {
        let engine = self.engine();
        page_of(&engine, st)
            .controls
            .iter()
            .any(|c| c.kind == ir::ControlKind::Repeater)
    }

    fn row_key(&self, rec: &Rec) -> Result<Vec<Value>, Signal> {
        let table = self.table(rec)?;
        Ok(table.primary_key(&rec.borrow().fields))
    }

    /// A repeater populates a window of rows at a time; a row inside the window it already
    /// holds is not populated again (corpus autoformat/TestAutoFormatExpressionTiming.al:5-12:
    /// a failing row 20 of 120 fails the open, row 30 does not, and a move to row 120
    /// populates a new window). The window is 20 rows: the corpus pins it only between 20 and 24.
    const WINDOW: usize = 20;

    fn populate_window(&mut self, st: &St) -> Result<(), Signal> {
        let rec = self.page_rec(st);
        let start = self.row_key(&rec)?;
        let mut keys = Vec::new();
        for i in 0..Self::WINDOW {
            let key = self.row_key(&rec)?;
            let stored = rec.borrow().fields.clone();
            self.populate_current(st)?;
            keys.push(al_builtins::value::WindowRow {
                key,
                stored,
                shown: rec.borrow().fields.clone(),
            });
            if i + 1 == Self::WINDOW {
                break;
            }
            self.ret_used = true;
            if !matches!(
                self.record_op("Next", &rec, &mut [Value::Integer(1)])?,
                Value::Integer(1)
            ) {
                break;
            }
        }
        // Back on the row the cursor is on.
        self.ret_used = true;
        self.record_op("Get", &rec, &mut start.clone())?;
        // The row buffer the page keeps for the row it stands on is what OnAfterGetRecord left.
        if let Some(row) = keys.first() {
            rec.borrow_mut().fields = row.shown.clone();
            self.set_xrec_fields(st, row.stored.clone());
        }
        st.borrow_mut().window = keys;
        Ok(())
    }

    fn in_window(&self, st: &St, key: &[Value]) -> bool {
        st.borrow().window.iter().any(|k| Self::same_key(&k.key, key))
    }

    fn same_key(a: &[Value], b: &[Value]) -> bool {
        a.len() == b.len()
            && a.iter().zip(b).all(|(a, b)| {
                ops::compare(a, b)
                    .map(|o| o == std::cmp::Ordering::Equal)
                    .unwrap_or(false)
            })
    }

    /// The cursor has landed on a stored row: populate it as the page type does, then
    /// OnAfterGetCurrRecord.
    fn arrive(&mut self, st: &St) -> Result<(), Signal> {
        // A stored row is not the started record the cursor may have left (corpus
        // handlers/TestPageOpenNewPartLink.al:OpenNew_LinesInsertedInCode_LastThenWriteLandsOnTheLastLine).
        st.borrow_mut().draft = false;
        if self.is_list(st) {
            let key = self.row_key(&self.page_rec(st))?;
            if !self.in_window(st, &key) {
                self.populate_window(st)?;
            } else {
                // A row of the window shows the buffer OnAfterGetRecord left, as long as the stored
                // row is still the one that buffer was made from.
                let rec = self.page_rec(st);
                let row = st
                    .borrow()
                    .window
                    .iter()
                    .find(|w| Self::same_key(&w.key, &key))
                    .filter(|w| Self::same_values(&w.stored, &rec.borrow().fields))
                    .cloned();
                match row {
                    Some(w) => {
                        rec.borrow_mut().fields = w.shown;
                        self.set_xrec_fields(st, w.stored);
                    }
                    None => self.set_xrec(st),
                }
            }
        } else {
            self.populate_current(st)?;
        }
        self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
        self.page_event(st, "onaftergetcurrrecordevent", None, None)?;
        Ok(())
    }

    /// Moves the cursor; the Boolean the move methods return.
    fn page_move(&mut self, st: &St, how: &str) -> Result<Value, Signal> {
        if !self.has_source(st) {
            return unsupported(format!("{how}() on a page without a source table"));
        }
        // Leaving a row saves it.
        if let Left::Stay = self.leave_row(st, how)? {
            return Ok(Value::Boolean(false));
        }
        let r = self.page_move_inner(st, how);
        self.torn_down_on_error(st, r).map(Value::Boolean)
    }

    /// An error escaping a row population tears the page down. On a card the move reads as
    /// "not open"; on a list it raises the error itself (corpus
    /// autoformat/TestAutoFormatExpressionTiming.al, RowFormat_List_LastOntoAFailingRowBeyondTheWindow).
    fn torn_down_on_error<T>(&mut self, st: &St, r: Result<T, Signal>) -> Result<T, Signal> {
        match r {
            Err(Signal::Error(e)) => {
                self.mark_closed(st);
                if self.is_list(st) {
                    Err(Signal::Error(e))
                } else {
                    Err(error(NOT_OPEN))
                }
            }
            other => other,
        }
    }

    fn page_move_inner(&mut self, st: &St, how: &str) -> Result<bool, Signal> {
        let rec = self.page_rec(st);
        let cursor = st.borrow().cursor;
        self.ret_used = true;
        let found = |v: Value| match v {
            Value::Integer(n) => n != 0,
            Value::Boolean(b) => b,
            _ => false,
        };
        let landed = match how {
            "First" => found(self.record_op("FindFirst", &rec, &mut [])?),
            "Last" => found(self.record_op("FindLast", &rec, &mut [])?),
            "Next" => match cursor {
                Cursor::Row => {
                    if found(self.record_op("Next", &rec, &mut [Value::Integer(1)])?) {
                        true
                    } else if st.borrow().new_line {
                        // corpus handlers/TestPageNewRowLine_Tests.al: Next() past the last data
                        // row lands on the blank line and answers true.
                        self.refuse_read_only_part(st, "Next() onto the new-row line")?;
                        self.land_on_new_line(st)?;
                        return Ok(true);
                    } else {
                        false
                    }
                }
                _ => false,
            },
            "Previous" => match cursor {
                Cursor::NewLine => found(self.record_op("FindLast", &rec, &mut [])?),
                Cursor::Row => found(self.record_op("Next", &rec, &mut [Value::Integer(-1)])?),
                Cursor::None => false,
            },
            _ => unreachable!(),
        };
        if landed {
            st.borrow_mut().cursor = Cursor::Row;
            self.arrive(st)?;
        }
        Ok(landed)
    }

    /// GoToKey / GoToRecord: a forward scan from the first row of the view, populating every row it
    /// passes, so a row whose population fails stops the search there (corpus
    /// autoformat/TestAutoFormatExpressionTiming.al, RowFormat_GoToKeyPastTheFailingRow_ReadsAsTheTestPageNotOpen).
    /// A key no row has leaves the page on the row it stood on
    /// (corpus handlers/TestPageGoToRecordNonKeyRefresh_Tests.al:1-15).
    fn page_go_to_key(&mut self, st: &St, key: &mut [Value]) -> Result<Value, Signal> {
        if !self.has_source(st) {
            return unsupported("GoToKey on a page without a source table");
        }
        let result_used = self.ret_used;
        // A TestPart raises when the key has the wrong number of fields, and when the key is
        // missing and the result is not used (corpus testpart/TestTestPart.al:
        // TestPart_GoToKey_ErrorsWhenTheKeyFieldCountIsWrong, _ErrorsForAMissingKeyWhenTheReturnIsDiscarded).
        // The corpus pins that an error is raised, not its text: refused.
        if st.borrow().host.is_some() {
            let rec = self.page_rec(st);
            let wanted = self.table(&rec)?.keys[0].len();
            if key.len() != wanted {
                return unsupported("TestPart.GoToKey with the wrong number of key values (error text not pinned)");
            }
        }
        self.save_row(st)?;
        let rec = self.page_rec(st);
        let table = self.table(&rec)?;
        let want: Vec<Value> = table.keys[0]
            .iter()
            .enumerate()
            .map(|(n, &f)| match key.get(n) {
                Some(v) => ops::convert(v.clone(), &table.fields[f].ty),
                None => Ok(self.default_value(&table.fields[f].ty)),
            })
            .collect::<Result<_, _>>()?;
        let before_fields = rec.borrow().fields.clone();
        let (before_cursor, before_draft) = (st.borrow().cursor, st.borrow().draft);
        self.ret_used = true;
        let mut at = self.record_op("FindFirst", &rec, &mut [])?;
        let mut found = false;
        while matches!(at, Value::Boolean(true)) {
            st.borrow_mut().cursor = Cursor::Row;
            let r = self.arrive(st);
            self.torn_down_on_error(st, r)?;
            let here = self.row_key(&rec)?;
            if here.len() == want.len()
                && here
                    .iter()
                    .zip(&want)
                    .all(|(a, b)| ops::compare(a, b).map(|o| o.is_eq()).unwrap_or(false))
            {
                found = true;
                break;
            }
            self.ret_used = true;
            at = self.record_op("Next", &rec, &mut [Value::Integer(1)])?;
            at = Value::Boolean(matches!(at, Value::Integer(n) if n != 0));
        }
        if !found {
            rec.borrow_mut().fields = before_fields;
            let mut s = st.borrow_mut();
            s.cursor = before_cursor;
            s.draft = before_draft;
            drop(s);
            if st.borrow().host.is_some() && !result_used {
                return unsupported("TestPart.GoToKey of a missing key with the result unused (error text not pinned)");
            }
        }
        Ok(Value::Boolean(found))
    }

    /// `FindFirstField` / `FindNextField` / `FindPreviousField`: walks the rows of the view from the
    /// first row, the row after the current one, or the row before it, and stops on the first row
    /// whose control shows `value`. The Variant is type-sensitive: a Text never matches an Integer
    /// field (corpus handlers/TestPageAdvanced.al:302 Page_FindPreviousField_StringValue_*). A miss
    /// leaves the cursor where it was (docs/probes-needed.md).
    fn page_find_field(&mut self, st: &St, how: &str, args: &mut [Value]) -> Result<Value, Signal> {
        if !self.has_source(st) {
            return unsupported(format!("{how} on a page without a source table"));
        }
        let Some(Value::TestControl(c)) = args.first() else {
            return unsupported(format!("{how} without a control"));
        };
        let ControlTarget::Control(idx) = c.target else {
            return unsupported(format!("{how} on a built-in control"));
        };
        let wanted = args.get(1).cloned().unwrap_or(Value::Null);
        let origin = self.row_key(&self.page_rec(st))?;
        let (first_step, step) = match how {
            "FindFirstField" => ("First", "Next"),
            "FindNextField" => ("Next", "Next"),
            _ => ("Previous", "Previous"),
        };
        let mut at = self.page_move(st, first_step)?;
        while matches!(at, Value::Boolean(true)) {
            let here = self.control_value(st, idx)?;
            if Self::variant_matches(&here, &wanted) {
                return Ok(Value::Boolean(true));
            }
            at = self.page_move(st, step)?;
        }
        let mut key = origin;
        self.page_go_to_key(st, &mut key)?;
        Ok(Value::Boolean(false))
    }

    fn variant_matches(here: &Value, wanted: &Value) -> bool {
        let class = |v: &Value| match v {
            Value::Text(_) | Value::Code(_) => 1,
            Value::Integer(_) => 2,
            Value::BigInteger(_) => 3,
            Value::Decimal(_) => 4,
            Value::Boolean(_) => 5,
            Value::Date(..) => 6,
            _ => 0,
        };
        class(here) == class(wanted)
            && class(here) != 0
            && ops::compare(here, wanted).map(|o| o.is_eq()).unwrap_or(false)
    }

    /// A `Trap()` set on a TestPage that belongs to a routine ends with the routine; one set through
    /// a `var` parameter belongs to the caller (corpus handlers/TestPageTrapScope_Tests.al:1-15).
    pub(crate) fn end_trap_scope(&mut self, frame: &crate::Frame, params: usize, by_var: &[bool]) {
        let Some(trap) = &self.trap else { return };
        // The trap ends with the last variable holding the page. A by-var parameter hands its page
        // back to the caller, and a page another variable still holds keeps the trap alive (corpus
        // handlers/TestPageTrapSharedScope_Tests.al:87 TrapSharedByTwoLocalsSurvivesWhenOneIsAssignedOut,
        // handlers/TestPageTrapScope_Tests.al TrapAssignedOutOfALocalVariableSurvives).
        let mut local_holders = 0;
        let mut handed_back = false;
        for (i, v) in frame.locals.iter().enumerate() {
            if let Value::TestPage(st) = v {
                if Rc::ptr_eq(st, trap) {
                    local_holders += 1;
                    if i < params && by_var.get(i).copied().unwrap_or(false) {
                        handed_back = true;
                    }
                }
            }
        }
        if local_holders == 0 || handed_back {
            return;
        }
        let outside = Rc::strong_count(trap).saturating_sub(1 + local_holders);
        if outside == 0 {
            trap.borrow_mut().trapping = false;
            self.trap = None;
        }
    }

    // ---- pages opened by AL code ----------------------------------------------------------

    /// The built-in action name `close_with` is asked for, as the `Action` the page reports. A
    /// lookup-mode modal reads OK and Cancel as LookupOK and LookupCancel; a page closing itself
    /// (`CurrPage.Close()`) reads as OK in a plain modal and as LookupCancel in lookup mode (corpus
    /// handlers/TestPageModalQueryClose_Tests.al, TestPageModalSelfClose_Tests.al).
    fn close_action(&self, st: &St, requested: &str) -> String {
        let (modal, lookup) = {
            let s = st.borrow();
            (s.modal, s.lookup)
        };
        match (requested, modal, lookup) {
            ("OK", true, true) => "LookupOK".into(),
            ("Cancel", true, true) => "LookupCancel".into(),
            ("Close", true, true) => "LookupCancel".into(),
            ("Close", true, false) => "OK".into(),
            ("Close", false, _) => "Cancel".into(),
            (other, _, _) => other.into(),
        }
    }

    /// What a modal page reports when its handler invoked nothing: the page type decides (corpus
    /// handlers/TestPageBuiltInActions_Tests.al (i)-(k), TestPageModalQueryClose_Tests.al (b) and (e)).
    fn unattended_action(&self, st: &St) -> &'static str {
        let (lookup, object) = {
            let s = st.borrow();
            (s.lookup, s.object as usize)
        };
        let page_type = self.engine.program.objects[object]
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("PageType"))
            .map(|(_, v)| v.trim().to_ascii_lowercase())
            .unwrap_or_default();
        match page_type.as_str() {
            "standarddialog" | "promptdialog" | "confirmationdialog" => "Cancel",
            _ if lookup => "LookupCancel",
            _ => "OK",
        }
    }

    /// `Page.Run` / `Page.RunModal` (static and on a `Page` variable): opens the page for the
    /// calling AL, hands it to the bound handler (or to the `TestPage.Trap()` variable for a plain
    /// run), and for a modal run reports how it closed.
    pub(crate) fn run_page_ui(&mut self, st: &St, record: Option<Rec>, modal: bool) -> Result<Value, Signal> {
        let object = st.borrow().object;
        let mut st = st.clone();
        let mut trapped = false;
        if !modal {
            if let Some(trap) = self.trap.take() {
                if trap.borrow().object != object {
                    return unsupported("a Trap() for another page than the one Page.Run opens");
                }
                trap.borrow_mut().trapping = false;
                // The trapped variable receives the page and keeps it open for the test.
                trap.borrow_mut().instance = None;
                st = trap;
                trapped = true;
            }
        }
        if st.borrow().open {
            return unsupported("running a page that is already open");
        }
        let kind = if modal { UiKind::ModalPage } else { UiKind::Page };
        st.borrow_mut().modal = modal && !trapped;
        let handler = self.open_page(&st, PageMode::Edit, record, if trapped { None } else { Some(kind) })?;
        let Some(handler) = handler else {
            return Ok(Value::Null);
        };
        self.answer_page(&st, handler, modal)
    }

    /// Hands the open page `st` to its handler, closes it if the handler left it open, and for a
    /// modal page reports how it closed.
    fn answer_page(&mut self, st: &St, handler: Handler, modal: bool) -> Result<Value, Signal> {
        let (hobject, hroutine, hinst) = handler;
        let st = st.clone();
        self.page_handler_depth += 1;
        let handled = self.call(hobject, hroutine, hinst, vec![Value::TestPage(st.clone())]);
        self.page_handler_depth -= 1;
        if let Err(e) = handled {
            self.mark_closed(&st);
            return Err(e);
        }
        if st.borrow().open {
            let a = self.unattended_action(&st);
            self.close_with(&st, a, true, false)?;
            if st.borrow().open {
                return unsupported("a page whose OnQueryClosePage refused the platform's close");
            }
        }
        // A handler that left its page torn down closed it without OK (corpus
        // testpageteardown/TestPageTeardown.al:1930; the non-lookup reading is docs-only).
        if modal && !st.borrow().open && st.borrow().result.is_none() {
            let a = self.close_action(&st, "Cancel");
            st.borrow_mut().result = Some(a);
        }
        let result = st.borrow().result.clone();
        st.borrow_mut().modal = false;
        if !modal {
            return Ok(Value::Null);
        }
        let name = result.unwrap_or_else(|| "None".into());
        let members: std::rc::Rc<[String]> = al_builtins::value::ACTION_MEMBERS
            .iter()
            .map(|m| m.to_string())
            .collect();
        let ordinal = al_builtins::value::ACTION_MEMBERS
            .iter()
            .position(|m| *m == name)
            .unwrap_or(0) as i32;
        Ok(Value::Option(ordinal, members))
    }

    // ---- writing ------------------------------------------------------------------------

    /// `TestPage.New()` (corpus handlers/TestPageInsertAllowed_Tests.al:70 New_OnInsertAllowedFalsePage_IsDenied,
    /// handlers/TestPageOpenNewAfterGetCurrRecord.al New_OnAnOpenCard_RunsOnAfterGetCurrRecordForTheNewRecord).
    fn new_record(&mut self, st: &St) -> Result<Value, Signal> {
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
        match self.leave_row(st, "New")? {
            Left::Stay => Ok(Value::Null),
            Left::Go => self.start_draft(st, true).map(|_| Value::Null),
        }
    }

    /// Saves the row the cursor is leaving. A started line whose insert fails does not raise:
    /// New(), Next() and Last() stay on it with one validation error on its key control; Previous()
    /// and First() move on and the insert fails again at Close()
    /// (corpus handlers/TestPageFailedInsert_Tests.al:1-30).
    fn leave_row(&mut self, st: &St, how: &str) -> Result<Left, Signal> {
        let r = self.save_row(st);
        match r {
            Err(Signal::Error(e)) if st.borrow().draft && st.borrow().dirty => match how {
                "New" | "Next" | "Last" => {
                    let key_ctl = self.key_control(st);
                    let mut s = st.borrow_mut();
                    if !s.refused {
                        s.errors.push((key_ctl, e.message));
                        s.refused = true;
                    }
                    Ok(Left::Stay)
                }
                "Previous" | "First" => {
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
    fn key_control(&self, st: &St) -> usize {
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
    fn set_xrec(&mut self, st: &St) {
        if !self.has_source(st) {
            return;
        }
        let fields = self.page_rec(st).borrow().fields.clone();
        self.set_xrec_fields(st, fields);
    }

    /// xRec holds `fields`: the row as it was read.
    fn set_xrec_fields(&mut self, st: &St, fields: Vec<Value>) {
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

    fn page_xrec_fields(&self, st: &St) -> Vec<Value> {
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

    fn same_values(a: &[Value], b: &[Value]) -> bool {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(x, y)| ops::compare(x, y).map(|o| o.is_eq()).unwrap_or(false))
    }

    /// The stored row with `rec`'s primary key, whatever the view says.
    fn stored_row(&mut self, rec: &Rec) -> Result<Option<Vec<Value>>, Signal> {
        let table = self.table(rec)?;
        let probe = Rc::new(RefCell::new(rec.borrow().clone()));
        probe.borrow_mut().filters.clear();
        let mut pk = table.primary_key(&rec.borrow().fields);
        self.ret_used = true;
        Ok(match self.record_op("Get", &probe, &mut pk)? {
            Value::Boolean(true) => Some(probe.borrow().fields.clone()),
            _ => None,
        })
    }

    /// A new record on the page: xRec stays on the row the cursor was on, Rec starts from the table's
    /// defaults, OnNewRecord runs with BelowxRec true (corpus
    /// handlers/TestPageCreateModeNewRecordArgs.al:243-270), then the row is populated
    /// (handlers/TestPageOpenNewAfterGetCurrRecord.al:1-12: OnAfterGetCurrRecord runs after OnNewRecord).
    fn start_draft(&mut self, st: &St, keep_focus: bool) -> Result<(), Signal> {
        if !self.has_source(st) {
            return unsupported("a new record on a page without a source table");
        }
        let rec = self.page_rec(st);
        self.set_xrec(st);
        self.note_lower_key(st)?;
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
        self.page_trigger(st, "onnewrecord", vec![Value::Boolean(true)])?;
        self.page_event(st, "onnewrecordevent", None, Some(true))?;
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
    fn save_row(&mut self, st: &St) -> Result<(), Signal> {
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
        self.page_xrec = Some(self.page_xrec_fields(st));
        let modified = self.record_op("Modify", &rec, &mut [Value::Boolean(true)]);
        self.page_xrec = None;
        modified?;
        st.borrow_mut().dirty = false;
        self.set_xrec(st);
        Ok(())
    }

    /// OnInsertRecord decides whether the typed record is inserted at all (corpus
    /// handlers/TestPageRecordTriggers.al:76 OnInsertRecord_ReturningFalse_SuppressesTheInsert).
    fn insert_draft(&mut self, st: &St) -> Result<(), Signal> {
        let rec = self.page_rec(st);
        let go = self.page_trigger_answer(st, "oninsertrecord", vec![Value::Boolean(false)])?;
        if go == Some(false) {
            return Ok(());
        }
        if !self.page_event(st, "oninsertrecordevent", Some(true), Some(false))? {
            return Ok(());
        }
        self.auto_split_key(st)?;
        self.ret_used = false;
        self.record_op("Insert", &rec, &mut [Value::Boolean(true)])?;
        {
            let mut s = st.borrow_mut();
            s.draft = false;
            s.dirty = false;
            s.cursor = Cursor::Row;
        }
        self.set_xrec(st);
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
    fn activate(&mut self, st: &St, idx: usize) -> Result<(), Signal> {
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

    /// `TestField.SetValue`: activates the control, turns the value into what the control would
    /// have been typed with, validates the field (the table's OnValidate, then the page control's),
    /// and records a refusal in the control's ledger (corpus handlers/TestPageValidationError_Tests.al,
    /// TestPageReadOnlySetValue.al:1-30, TestPageBooleanSpelling_Tests.al).
    fn control_set_value(&mut self, st: &St, idx: usize, args: &mut [Value]) -> Result<Value, Signal> {
        self.refuse_read_only_part(st, "SetValue")?;
        self.activate(st, idx)?;
        // Only an Update() requested by this call's triggers refreshes at its end.
        st.borrow_mut().refresh = false;
        let engine = self.engine();
        let page = page_of(&engine, st);
        let ctl = &page.controls[idx];
        let current = self.control_value(st, idx)?;
        let Some(ty) = self.control_type(st, idx, &current) else {
            return unsupported("SetValue on a control whose type is not known");
        };
        let rec_bound = matches!(ctl.binding, ir::Binding::Field(_));
        let caption = self.control_caption(st, idx)?;
        let fmt = self.control_format(st, idx)?;
        let arg = args.first().cloned().unwrap_or(Value::Null);
        let text = match &arg {
            Value::Text(t) | Value::Code(t) => t.clone(),
            other => {
                let converted = ops::convert(other.clone(), &ty)?;
                self.control_text(st, idx, &converted)?
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
        let suffix = if rec_bound {
            " (Select Refresh to discard errors)"
        } else {
            ""
        };
        let refuse = |this: &mut Session, msg: String| -> Signal {
            let stored = format!("{msg}{suffix}");
            st.borrow_mut().errors.push((idx, stored.clone()));
            let _ = this;
            error(format!(
                "Validation error for Field: {},  Message = '{stored}'",
                page_of(&engine, st).controls[idx].name
            ))
        };
        let parsed = match al_builtins::pagefmt::parse_control_text(&text, &ty, &fmt, &by_text) {
            Ok(v) => v,
            Err(al_builtins::pagefmt::ParseError::Invalid) => {
                let msg = format!("Your entry of '{text}' is not an acceptable value for '{caption}'.");
                return Err(refuse(self, msg));
            }
            Err(al_builtins::pagefmt::ParseError::Unsupported) => {
                return unsupported(format!(
                    "SetValue of a {ty:?} control: its spelling is not modelled yet"
                ));
            }
        };
        let parsed = ops::convert(parsed, &ty)?;
        // MinValue / MaxValue are enforced on a control write, not on Rec.Validate (corpus
        // handlers/TestPageFieldMinMaxValue_Tests.al:20-110).
        if let Some(f) = self.control_field(st, idx) {
            let bound = |n: &str| {
                f.properties
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case(n))
                    .map(|(_, v)| v.trim().to_string())
            };
            let num = |v: &Value| match v {
                Value::Integer(i) => Some(rust_decimal::Decimal::from(*i)),
                Value::BigInteger(i) => Some(rust_decimal::Decimal::from(*i)),
                Value::Decimal(d) => Some(*d),
                _ => None,
            };
            if let Some(n) = num(&parsed) {
                if let Some(min) = bound("minvalue") {
                    if min.parse::<rust_decimal::Decimal>().is_ok_and(|m| n < m) {
                        return Err(refuse(
                            self,
                            format!("The value must be greater than or equal to {min}"),
                        ));
                    }
                }
                if let Some(max) = bound("maxvalue") {
                    if max.parse::<rust_decimal::Decimal>().is_ok_and(|m| n > m) {
                        return Err(refuse(self, format!("The value must be less than or equal to {max}")));
                    }
                }
            }
        }
        if Self::same_values(std::slice::from_ref(&current), std::slice::from_ref(&parsed)) {
            return Ok(Value::Null);
        }
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
                    self.record_op("Validate", &rec, &mut [Value::Integer(pos as i32), parsed.clone()])?;
                }
                ir::Binding::Var(slot) => {
                    let inst = st.borrow().instance.clone().expect("open page");
                    inst.borrow_mut().globals[slot] = parsed.clone();
                }
                _ => return unsupported("SetValue on a read-only control"),
            }
            if let Some(r) = ctl.trigger("onvalidate") {
                self.page_routine(st, r)?;
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
                } else if let ir::Binding::Var(slot) = ctl.binding {
                    let inst = st.borrow().instance.clone().expect("open page");
                    inst.borrow_mut().globals[slot] = current.clone();
                }
                Err(refuse(self, e.message))
            }
            Err(other) => Err(other),
        }
    }

    // ---- controls -----------------------------------------------------------------------

    fn test_control_op(&mut self, method: &str, c: &ControlRef, args: &mut [Value]) -> Result<Value, Signal> {
        self.require_open(&c.page)?;
        let st = &c.page;
        let m = method.split_once('.').map(|(_, m)| m).unwrap_or(method);
        let idx = match &c.target {
            ControlTarget::Control(i) => *i,
            ControlTarget::BuiltIn(name) if matches!(name.as_str(), "Edit" | "View") => {
                // On a card the built-in Edit and View switch the open page's mode in place; both stay
                // visible, and only the one the current mode does not already have is enabled (corpus
                // handlers/TestPageModeSwitchAction_Tests.al:Claim 1 and 2). A list opens its card
                // instead (a page handler): not modelled yet.
                if self.is_list(st) {
                    return unsupported(format!("the built-in {name} action of a list page"));
                }
                let editable = st.borrow().mode != PageMode::View;
                let applies = (name == "Edit") == !editable;
                return match m {
                    "Visible" => Ok(Value::Boolean(true)),
                    "Enabled" => Ok(Value::Boolean(applies)),
                    "Invoke" => {
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
                let m = m.to_string();
                return self.test_filter_op(&m, st, args);
            }
            ControlTarget::BuiltIn(name) if m == "Invoke" => {
                // OK and Yes confirm, Cancel and No cancel; a refused save keeps the page open
                // (corpus testpageclose/TestPageClosingActions.al).
                return self.close_with(st, name, true, true);
            }
            _ => return unsupported(format!("{method} on a built-in control")),
        };
        // The data items and actions of a report's request page.
        if self.is_request_page(st) {
            let kind = page_of(&self.engine(), st).controls[idx].kind.clone();
            match kind {
                ir::ControlKind::DataItem => return self.request_item_op(method, st, idx, args),
                ir::ControlKind::Action if m == "Invoke" => return self.request_action_invoke(st, idx),
                _ => {}
            }
        }
        // A control whose Visible is the literal false, or that sits in a group whose Visible is,
        // is not on the runtime page at all (corpus handlers/TestPageFieldVisibleGroup_Tests.al:1-30).
        if self.control_eliminated(st, idx) {
            let engine = self.engine();
            let ctl = &page_of(&engine, st).controls[idx];
            return Err(error(format!("The field {} is not found on the page.", ctl.name)));
        }
        // Only Invoke of an actionref is pinned: it runs the action it stands for.
        if m != "Invoke" && page_of(&self.engine(), st).controls[idx].action_ref.is_some() {
            return unsupported(format!("TestAction.{m} of an actionref"));
        }
        match m {
            "Value" => {
                if self.shows_no_row(st, idx) {
                    return Ok(Value::Text(String::new()));
                }
                let v = self.control_value(st, idx)?;
                let text = self.control_text(st, idx, &v)?;
                Ok(Value::Text(text))
            }
            "AsInteger" => self.control_as(st, idx, &RtType::Integer),
            "AsDecimal" => self.control_as(st, idx, &RtType::Decimal),
            "AsBoolean" => self.control_as(st, idx, &RtType::Boolean),
            "AsDate" => self.control_as(st, idx, &RtType::Date),
            "AsTime" => self.control_as(st, idx, &RtType::Time),
            "AsDateTime" => self.control_as(st, idx, &RtType::DateTime),
            "AssertEquals" => self.control_assert_equals(st, idx, args),
            "Caption" => Ok(Value::Text(self.control_caption(st, idx)?)),
            "OptionCount" | "GetOption" => {
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
                if m == "OptionCount" {
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
                Ok(Value::Text(self.control_text(st, idx, &v)?))
            }
            // The properties as written; a computed one is not read (docs only).
            "HideValue" | "ShowMandatory" => {
                let name = if m == "HideValue" { "hidevalue" } else { "showmandatory" };
                let engine = self.engine();
                match page_of(&engine, st).controls[idx].prop(name) {
                    None => Ok(Value::Boolean(false)),
                    Some(ir::PropVal::Lit(l)) => Ok(Value::Boolean(unquote(l).eq_ignore_ascii_case("true"))),
                    Some(_) => unsupported(format!("TestField.{m} of a computed property")),
                }
            }
            "Editable" => Ok(Value::Boolean(self.control_editable(st, idx)?)),
            "Enabled" => Ok(Value::Boolean(self.control_enabled(st, idx)?)),
            "Visible" => Ok(Value::Boolean(self.control_visible(st, idx)?)),
            "Invoke" => self.action_invoke(st, idx),
            "SetValue" => {
                let r = self.control_set_value(st, idx, args)?;
                self.resync_parts_linked_by(st, idx)?;
                Ok(r)
            }
            "Activate" => {
                self.activate(st, idx)?;
                Ok(Value::Null)
            }
            "ValidationErrorCount" => {
                let n = st.borrow().errors.iter().filter(|(i, _)| *i == idx).count();
                Ok(Value::Integer(n as i32))
            }
            "GetValidationError" => {
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
                    Some(t) => Ok(Value::Text((*t).clone())),
                    None => unsupported("GetValidationError past the ledger"),
                }
            }
            other => unsupported(format!("TestField.{other}")),
        }
    }

    fn control_eliminated(&self, st: &St, idx: usize) -> bool {
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

    /// `TestAction.Invoke`: runs the action's OnAction when it is enabled; a disabled action
    /// does nothing and raises nothing (corpus handlers/TestPageActionEnabledSourceField_Tests.al,
    /// TPAE_ActionEnabledBoolField_FalseRow_IsDisabledAndItsTriggerIsSkipped).
    fn action_invoke(&mut self, st: &St, idx: usize) -> Result<Value, Signal> {
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
        if st.borrow().host.is_some() {
            self.save_row(st)?;
        }
        st.borrow_mut().refresh = false;
        if let Some(r) = page_of(&engine, st).controls[idx].trigger("onaction") {
            let r = self.page_routine(st, r);
            if r.is_err() {
                st.borrow_mut().refresh = false;
            }
            r?;
            self.settle_after_action(st)?;
            self.run_refresh(st)?;
        } else if page_of(&engine, st).controls[idx].run_object.is_some() {
            return self.run_object_action(st, idx);
        }
        Ok(Value::Null)
    }

    /// The re-read `CurrPage.Update` asked for, once the call that made it has returned: the
    /// stored current row loads again and raises OnAfterGetRecord and OnAfterGetCurrRecord; a new
    /// row nobody has saved has nothing to re-read (corpus pageupdate/TestPageUpdateNewRow.al:1-30,
    /// TestPageUpdateGoneRow.al StoredRow_OnValidateUpdateFalse_Trace). The row is read from the
    /// table unless the client holds edits of it that are not saved yet.
    fn run_refresh(&mut self, st: &St) -> Result<(), Signal> {
        if !std::mem::take(&mut st.borrow_mut().refresh) {
            return Ok(());
        }
        if !st.borrow().open || !self.has_source(st) || st.borrow().cursor != Cursor::Row || st.borrow().draft {
            return Ok(());
        }
        let rec = self.page_rec(st);
        if self.stored_row(&rec)?.is_none() {
            return Ok(());
        }
        if !st.borrow().dirty {
            self.ret_used = true;
            let mut pk = self.table(&rec)?.primary_key(&rec.borrow().fields);
            self.record_op("Get", &rec, &mut pk)?;
        }
        let r = (|| {
            self.populate_current(st)?;
            self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
            self.page_event(st, "onaftergetcurrrecordevent", None, None)
        })();
        self.torn_down_on_error(st, r)?;
        st.borrow_mut().refresh = false;
        Ok(())
    }

    /// What the page does once an action returns: a page that showed no row may show the one the
    /// action inserted; a row the action deleted closes a card, and moves a list to the next row,
    /// else the previous one, else none (corpus pageupdate/TestPageUpdateGoneRow.al:1-30).
    fn settle_after_action(&mut self, st: &St) -> Result<(), Signal> {
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
            self.page_trigger(st, "onclosepage", vec![])?;
            self.mark_closed(st);
            return Ok(());
        }
        if self.engine.program.objects[st.borrow().object as usize]
            .page
            .as_ref()
            .is_some_and(|p| p.triggers.iter().any(|(n, _)| n == "onfindrecord"))
        {
            return unsupported("a list whose current row an action deleted, with an OnFindRecord trigger");
        }
        self.ret_used = true;
        let moved = matches!(
            self.record_op("Next", &rec, &mut [Value::Integer(1)])?,
            Value::Integer(1)
        ) || matches!(
            self.record_op("Next", &rec, &mut [Value::Integer(-1)])?,
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

    /// A page that showed no row shows one again once page code has inserted a row the view
    /// admits and left Rec on it; code that only changes the buffer, sets a key it never
    /// inserts, or reads a row the view hides shows nothing (corpus
    /// testpart/BnrBlankPart.al:1-30, NoRow_Action_* arms).
    fn show_row_page_code_positioned(&mut self, st: &St) -> Result<(), Signal> {
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
        if matches!(self.record_op("FindFirst", &probe, &mut [])?, Value::Boolean(true)) {
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

    /// The value a control shows: its binding, read now.
    fn control_value(&mut self, st: &St, idx: usize) -> Result<Value, Signal> {
        let engine = self.engine();
        match page_of(&engine, st).controls[idx].binding.clone() {
            ir::Binding::Field(pos) => Ok(self.page_rec(st).borrow().fields[pos].clone()),
            ir::Binding::Var(slot) => {
                let inst = st.borrow().instance.clone().expect("open page");
                let v = inst.borrow().globals[slot].clone();
                Ok(v)
            }
            ir::Binding::Expr(r) => self.page_routine(st, r),
            ir::Binding::None => unsupported("a control without a source"),
        }
    }

    fn control_as(&mut self, st: &St, idx: usize, to: &RtType) -> Result<Value, Signal> {
        if self.shows_no_row(st, idx) {
            return Ok(Value::default_for(to));
        }
        let v = self.control_value(st, idx)?;
        ops::convert(v, to)
    }

    /// A control bound to the page's record reads blank when the page shows no row: whatever its
    /// type, and on a field with an InitValue too (corpus testpart/BnrBlankPart.al:1-30).
    fn shows_no_row(&self, st: &St, idx: usize) -> bool {
        let engine = self.engine();
        st.borrow().cursor == Cursor::None
            && matches!(page_of(&engine, st).controls[idx].binding, ir::Binding::Field(_))
    }

    /// The source field of a control bound to a record field.
    fn control_field(&self, st: &St, idx: usize) -> Option<ir::Field> {
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
    fn control_format(&mut self, st: &St, idx: usize) -> Result<ControlFormat, Signal> {
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
                Some(ir::PropVal::Routine(r)) => match self.page_routine(st, *r)? {
                    Value::Text(t) | Value::Code(t) => Some(t),
                    _ => None,
                },
                None => None,
            };
            f.decimal_places = expr.and_then(|e| pagefmt::parse_precision(&e));
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

    fn control_text(&mut self, st: &St, idx: usize, v: &Value) -> Result<String, Signal> {
        if let Value::Library(name, _) = v {
            return unsupported(format!("control text of a {name} value"));
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
        Ok(pagefmt::control_text(v, &f, &caption))
    }

    /// The type of the value a control holds.
    fn control_type(&self, st: &St, idx: usize, v: &Value) -> Option<RtType> {
        if let Some(f) = self.control_field(st, idx) {
            return Some(f.ty);
        }
        let engine = self.engine();
        if let ir::Binding::Var(slot) = page_of(&engine, st).controls[idx].binding {
            let object = st.borrow().object as usize;
            return Some(engine.program.objects[object].globals[slot].ty.clone());
        }
        let _ = v;
        None
    }

    /// corpus handlers/TestPageDecimalAssertEquals_Tests.al:85, TestPageVariableControlErrorName.al:89:
    /// `AssertEquals for Field: <control> Expected = '<expected>', Actual = '<actual>'`.
    fn control_assert_equals(&mut self, st: &St, idx: usize, args: &mut [Value]) -> Result<Value, Signal> {
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
                self.control_text(st, idx, &converted)?
            }
        };
        if expected_text != actual_text {
            let engine = self.engine();
            let name = page_of(&engine, st).controls[idx].name.clone();
            return Err(error(format!(
                "AssertEquals for Field: {name} Expected = '{expected_text}', Actual = '{actual_text}'"
            )));
        }
        Ok(Value::Null)
    }

    /// The control's caption: its own `Caption`, else the source field's, else its name.
    fn control_caption(&mut self, st: &St, idx: usize) -> Result<String, Signal> {
        let engine = self.engine();
        let ctl = &page_of(&engine, st).controls[idx];
        // CaptionClass `3,<text>` is a caption the platform resolves to <text>
        // (corpus autoformat/TestAutoFormatExpressionTiming.al, WorkingCaptionClass_OpensAndReadsItsCaption).
        let class = match ctl.prop("captionclass") {
            Some(ir::PropVal::Lit(c)) => Some(unquote(c)),
            Some(ir::PropVal::Routine(r)) => match self.page_routine(st, *r)? {
                Value::Text(t) | Value::Code(t) => Some(t),
                _ => None,
            },
            None => None,
        };
        if let Some(c) = class {
            return match c.split_once(',') {
                Some(("3", text)) => Ok(text.to_string()),
                _ => unsupported(format!("CaptionClass `{c}`")),
            };
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
    fn prop_bool_now(&mut self, st: &St, idx: usize, name: &str, default: bool) -> Result<bool, Signal> {
        let engine = self.engine();
        match page_of(&engine, st).controls[idx].prop(name) {
            None => Ok(default),
            Some(ir::PropVal::Lit(v)) => Ok(is_true(v)),
            Some(ir::PropVal::Routine(r)) => ops::truthy(&self.page_routine(st, *r)?),
        }
    }

    fn page_editable(&self, st: &St) -> bool {
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
    fn control_editable(&mut self, st: &St, idx: usize) -> Result<bool, Signal> {
        // A control bound to a page variable stays editable on a read-only page (corpus
        // handlers/TestPageReadOnlySetValue.al:365 OpenView_PageVariableControl_StaysEditable).
        let engine = self.engine();
        let rec_bound = !matches!(page_of(&engine, st).controls[idx].binding, ir::Binding::Var(_))
            || (st.borrow().lookup && self.is_list_page(st))
            || st.borrow().editable_off;
        Ok((!rec_bound || self.page_editable(st)) && self.prop_bool_now(st, idx, "editable", true)?)
    }

    fn control_enabled(&mut self, st: &St, idx: usize) -> Result<bool, Signal> {
        self.prop_bool_now(st, idx, "enabled", true)
    }

    fn control_visible(&mut self, st: &St, idx: usize) -> Result<bool, Signal> {
        let engine = self.engine();
        let page = page_of(&engine, st);
        // Own Visible is the open-time value; a group's is live and so are the groups around the
        // control (corpus handlers/TestPageControlPropertyLiveness_Tests.al:56).
        let own = if page.controls[idx].kind == ir::ControlKind::Group {
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

/// Whether any statement of `body` executes `exit(value)`.
fn has_value_exit(body: &[ir::Stmt]) -> bool {
    use ir::Stmt;
    body.iter().any(|s| match s {
        Stmt::Exit(Some(_)) => true,
        Stmt::If(_, a, b) => has_value_exit(a) || has_value_exit(b),
        Stmt::While(_, b) | Stmt::Repeat(b, _) | Stmt::AssertError(b) => has_value_exit(b),
        Stmt::For { body, .. } | Stmt::Foreach { body, .. } => has_value_exit(body),
        Stmt::Case {
            branches, otherwise, ..
        } => branches.iter().any(|(_, b)| has_value_exit(b)) || has_value_exit(otherwise),
        _ => false,
    })
}
