//! The test client for pages (ADR-012): the part of BC's test framework that
//! opens a page, moves on its rows, reads and writes its controls and closes
//! it again. The page itself runs as ordinary AL (triggers, procedures,
//! property routines) on an `Instance` in this session.
//!
//! Every rule below cites the corpus test that pins it; behaviour no test
//! pins raises `Signal::Unsupported`.

use crate::{Engine, Session, UiKind};
use al_builtins::describe::Describe;
use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::pagefmt::{self, Blank, ControlFormat};
use al_builtins::value::{ControlRef, ControlTarget, Cursor, PageMode, PageState, RecordState, RtType, Value};
use al_data::Store as _;
use al_ir as ir;
use rust_decimal::prelude::ToPrimitive;
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

/// Whether `name` (a page's name or id as a table property writes it) is a page of a dependency app
/// that was not compiled.
fn names_uncompiled_page(engine: &Engine, name: &str) -> bool {
    engine.program.objects.iter().any(|o| {
        o.kind == "uncompiled page"
            && (o.name.eq_ignore_ascii_case(name) || o.id.is_some_and(|i| i.to_string() == name))
    })
}

fn error_code(message: impl Into<String>, code: &str) -> Signal {
    Signal::Error(AlError {
        message: message.into(),
        code: code.into(),
        call_stack: String::new(),
    })
}

/// The id a TestPage error names a control by: FNV-1a over the UTF-16 text of the declaring
/// object's id followed by the control name, read as a signed 32-bit number and made positive.
/// It reproduces every id the corpus pins: part `Hidden` of page 60344 (1318487454), `Name` of
/// page 329 (301776846), `User Defined` of page 5361 (104384132) and the extension control
/// `Ignore Status on Service Docs.` of page extension 6478 (1790009997).
pub(crate) fn control_id(object_id: u32, name: &str) -> i64 {
    request::action_id(object_id, name)
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

mod areas;
mod background;
pub(crate) use background::BgWorker;
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
            other => Err(Signal::Unsupported(format!("control of {}", other.describe()))),
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
            other => panic!("Rec is {}", other.describe()),
        }
    }

    fn require_open(&self, st: &St) -> Result<(), Signal> {
        if st.borrow().open {
            Ok(())
        } else {
            Err(error_code(NOT_OPEN, "TestPageNotOpened"))
        }
    }

    /// Runs page trigger `name` (lower case) if the page declares it, then the same trigger of
    /// each page extension that declares it. The page's own trigger runs first (assumed, see
    /// docs/probes-needed.md "Page extensions"); the lowering refuses two extensions that
    /// declare one trigger and extension triggers that answer a Boolean. The answer returned
    /// is the page's own.
    fn page_trigger(&mut self, st: &St, name: &str, args: Vec<Value>) -> Result<Option<Value>, Signal> {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let page = page_of(&engine, st);
        let mut answer = None;
        let mut result = Ok(());
        if let Some(r) = page.triggers.iter().find(|(n, _)| n == name).map(|(_, r)| *r) {
            let inst = st.borrow().instance.clone().expect("open page");
            match self.call(object, r, inst, args.clone()) {
                Ok((v, _)) => answer = Some(v),
                Err(e) => result = Err(e),
            }
        }
        if result.is_ok() {
            for (n, e, r) in &page.ext_triggers {
                if n == name {
                    let inst = self.owner_instance(st, Some(*e));
                    if let Err(err) = self.call(*e, *r, inst, args.clone()) {
                        result = Err(err);
                        break;
                    }
                }
            }
        }
        self.end_page_call(st);
        result.map(|()| answer)
    }

    /// The instance that runs the code of `owner`: the page itself, or one of its extensions.
    fn owner_instance(&self, st: &St, owner: Option<usize>) -> Rc<RefCell<al_builtins::value::Instance>> {
        let s = st.borrow();
        match owner {
            None => s.instance.clone().expect("open page"),
            Some(e) => s
                .ext_instances
                .iter()
                .find(|(o, _)| *o == e)
                .map(|(_, i)| i.clone())
                .expect("an extension of the page has an instance"),
        }
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

    /// `OnQueryClosePage` of a page and of the extensions that declare it: all of them run, the page's first
    /// and then each extension's in application order, whatever the earlier ones answered. The page closes
    /// only when every trigger that states an answer says true (corpus
    /// handlers/TestPageCloseTriggersSelectionCardPageContracts.al:
    /// OnQueryClosePage_PageAndExtension_BothRun_AndBothMustAgreeToClose). `Some(false)` vetoes; `None` when
    /// none states an answer.
    fn query_close_answer(&mut self, st: &St, args: Vec<Value>) -> Result<Option<bool>, Signal> {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        let page = page_of(&engine, st);
        let mut answers = Vec::new();
        let mut calls: Vec<(usize, usize, Option<usize>)> = Vec::new();
        if let Some(r) = page
            .triggers
            .iter()
            .find(|(n, _)| n == "onqueryclosepage")
            .map(|(_, r)| *r)
        {
            calls.push((object, r, None));
        }
        for (n, e, r) in &page.ext_triggers {
            if n == "onqueryclosepage" {
                calls.push((*e, *r, Some(*e)));
            }
        }
        let mut failure = None;
        for (obj, routine, owner) in calls {
            let states = has_value_exit(&engine.program.objects[obj].routines[routine].body);
            let inst = self.owner_instance(st, owner);
            match self.call(obj, routine, inst, args.clone()) {
                Ok((Value::Boolean(b), _)) if states => answers.push(b),
                Ok(_) => {}
                Err(e) => {
                    failure = Some(e);
                    break;
                }
            }
        }
        self.end_page_call(st);
        if let Some(e) = failure {
            return Err(e);
        }
        Ok(if answers.contains(&false) {
            Some(false)
        } else if answers.is_empty() {
            None
        } else {
            Some(true)
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
        // Subscribers that are compiled and subscribers that are declared and not (both would run
        // in BC).
        let declared = engine
            .events
            .subscribers
            .iter()
            .filter(|(_, subs)| !subs.is_empty())
            .map(|((o, ev), _)| (o, ev))
            .chain(engine.events.missing.keys().map(|(o, ev)| (o, ev)));
        for (o, ev) in declared {
            if *o == object
                && ev.starts_with("on")
                && ev.ends_with("event")
                && !al_ir::RAISED_PAGE_EVENTS.contains(&ev.as_str())
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

    /// Calls routine `r` of `owner` (the page, or one of its extensions) on its instance.
    fn owner_routine(&mut self, st: &St, owner: Option<usize>, r: usize) -> Result<Value, Signal> {
        let object = owner.unwrap_or(st.borrow().object as usize);
        let inst = self.owner_instance(st, owner);
        let result = self.call(object, r, inst, vec![]).map(|(v, _)| v);
        self.end_page_call(st);
        result
    }

    /// Calls routine `r` written in control `idx`: in the extension that declares the control, else the page.
    fn control_routine(&mut self, st: &St, idx: usize, r: usize) -> Result<Value, Signal> {
        let engine = self.engine();
        let owner = page_of(&engine, st).controls[idx].owner;
        self.owner_routine(st, owner, r)
    }

    /// The value of the routine behind property value `v` of control `idx`; `None` for a literal.
    fn prop_routine_value(&mut self, st: &St, idx: usize, v: &ir::PropVal) -> Result<Option<Value>, Signal> {
        match v {
            ir::PropVal::Lit(_) => Ok(None),
            ir::PropVal::Routine(r) => self.control_routine(st, idx, *r).map(Some),
            ir::PropVal::Ext(e, r) => self.owner_routine(st, Some(*e), *r).map(Some),
        }
    }

    /// The `TestPage.*` / `TestField.*` methods (ADR-012).
    pub(crate) fn page_op(
        &mut self,
        method: &str,
        recv: Option<&mut Value>,
        args: &mut [Value],
    ) -> Result<Value, Signal> {
        // A TestPage begins a transaction of its own (see `in_page_transaction`). Page.Run and Page.RunModal with a handler that writes are allowed under None, and the
        // transaction the page began has ended afterwards
        // (codeunit/TestWriteTxNoneModelContracts.al:200-216 and :163-185).
        if recv.is_none() && self.no_transaction() {
            if matches!(method, "Page.RunStatic" | "Page.RunModalStatic") {
                return self.in_page_transaction(|s| s.page_op_inner(method, recv, args));
            }
            return unsupported(format!("{method} under TransactionModel::None (not pinned)"));
        }
        if recv.is_some() && self.no_transaction() {
            return self.in_page_transaction(|s| s.page_op_inner(method, recv, args));
        }
        self.page_op_inner(method, recv, args)
    }

    fn page_op_inner(&mut self, method: &str, recv: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
        match recv {
            None if matches!(method, "Page.GetBackgroundParameters" | "Page.SetBackgroundTaskResult") => {
                self.bg_static(method, args)
            }
            None if method == "Page.RunStatic" || method == "Page.RunModalStatic" => {
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
                    .position(|o| o.kind == "page" && o.id == Some(id))
                else {
                    // A page of a dependency app that nothing reached is not compiled (ADR-013); BC has it.
                    if engine
                        .program
                        .objects
                        .iter()
                        .any(|o| o.kind == "uncompiled page" && o.id == Some(id))
                    {
                        return unsupported(format!(
                            "page {n} belongs to a dependency app and was not compiled (no code the workspace reaches uses it)"
                        ));
                    }
                    return Err(error(format!("You tried to invoke the Page object with the ID {n}")));
                };
                let st = Rc::new(RefCell::new(PageState::closed(object as u32)));
                // The static RunModal reports the lookup values, with or without a record: OK reads
                // LookupOK, Cancel LookupCancel (corpus handlers/TestPageModalHandlerStatic_Tests.al:48-72
                // for the record overload;
                // handlers/TestPageModalResultsAndTriggersContracts.al:206 RunModal_Dialog_StaticVersusVariable_ReportedAction
                // and :240 RunModal_Card_StaticWithAndWithoutRecord_VersusVariable_ReportedAction for the others). A page variable reports OK and Cancel.
                if method == "Page.RunModalStatic" {
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
            other => unsupported(format!("{method} on {}", other.describe())),
        }
    }

    fn test_page_op(&mut self, method: &str, st: &St, args: &mut [Value]) -> Result<Value, Signal> {
        let name = method.split_once('.').map(|(_, m)| m).unwrap_or(method);
        let r = self.test_page_op_inner(method, st, args)?;
        // The client runs the page's background tasks before the call returns (corpus
        // handlers/TestPageBgTask_Tests.al:EnqueueBackgroundTask_CompletesBeforeOpenViewReturns).
        self.bg_drain(st)?;
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
                    // Code: handlers/TestPageRuntimeContracts.al:274-275 (page and part).
                    Some(Value::Integer(n)) => Err(error_code(
                        format!("The field with ID = {n} is not found on the page."),
                        "TestFieldNotFound",
                    )),
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
            "SaveAsXml" => self.request_save_as_xml(st, args),
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
            // `ObjectId(UseNames)`: `Page <id>`, or with names (the default) `Page <Caption>`, the object name when
            // the page has no Caption. Taken from the probed report form (handlers/PWRefusedBuiltins.al:78-107,
            // BC 27.5); for a page it is a documented assumption (docs/probes-needed.md). It reads the object, so
            // a page that has not run answers too.
            "ObjectId" if method.starts_with("Page.") => {
                al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::PageObjectId);
                let names = match args.first() {
                    None => true,
                    Some(Value::Boolean(b)) => *b,
                    Some(other) => return unsupported(format!("Page.ObjectId with {}", other.describe())),
                };
                let engine = self.engine();
                let o = &engine.program.objects[st.borrow().object as usize];
                if !names {
                    return Ok(Value::Text(format!("Page {}", o.id.unwrap_or(0))));
                }
                let caption = match o.properties.iter().find(|(k, _)| k.eq_ignore_ascii_case("Caption")) {
                    Some((_, v)) => match crate::recmeta::string_literal(v) {
                        Some(c) => c,
                        None => return unsupported("Page.ObjectId of a page whose Caption is not a literal"),
                    },
                    None => o.name.clone(),
                };
                Ok(Value::Text(format!("Page {caption}")))
            }
            // `CurrPage.Caption := x` sets the page's caption for as long as it stays open; without an
            // argument it answers the caption in force (handlers/TestPageCaption_Page.al:44).
            "Caption" if method.starts_with("Page.") => {
                self.require_open(st)?;
                match args.first() {
                    Some(v) => match al_builtins::native::text_of(v) {
                        Some(t) => {
                            st.borrow_mut().caption = Some(t);
                            Ok(Value::Null)
                        }
                        None => unsupported(format!("CurrPage.Caption with {}", v.describe())),
                    },
                    None => Ok(Value::Text(self.page_caption(st))),
                }
            }
            // `ObjectId(UseNames)` (CurrPage and a Page variable alike, open or not): `Page <id>`, or with names, the
            // default, `Page <Caption>` and the object name when the page has no Caption (handlers/PRPageObjectId.al).
            "ObjectId" if method.starts_with("Page.") => {
                let names = match args.first() {
                    None => true,
                    Some(v) => ops::truthy(v)?,
                };
                let engine = self.engine();
                let o = &engine.program.objects[st.borrow().object as usize];
                if !names {
                    return Ok(Value::Text(format!("Page {}", o.id.unwrap_or(0))));
                }
                let caption = o
                    .properties
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("Caption"))
                    .map(|(_, v)| unquote(v))
                    .unwrap_or_else(|| o.name.clone());
                Ok(Value::Text(format!("Page {caption}")))
            }
            "Caption" => {
                self.require_open(st)?;
                Ok(Value::Text(self.page_caption(st)))
            }
            // `CurrPage.PromptMode := PromptMode::Prompt` states what a PromptDialog already starts in:
            // the pages of handlers/TbaPromptDeclared.Page.al and TbaPromptBare.Page.al do it in OnOpenPage
            // and the tests around them (TestPageBuiltInActions_Tests.al (a)-(j)) see an ordinary dialog.
            // The other modes are not pinned.
            "PromptMode" => match args.first() {
                Some(Value::Option(n, members)) if members.get(*n as usize).is_some_and(|m| m == "Prompt") => {
                    Ok(Value::Null)
                }
                Some(Value::Enum(..)) | Some(Value::Option(..)) => {
                    unsupported("CurrPage.PromptMode other than Prompt (not pinned)")
                }
                other => unsupported(format!("CurrPage.PromptMode with {}", other.describe())),
            },
            "Editable" if method.starts_with("Page.") => {
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
                    st.borrow_mut().lookup_set = true;
                    Ok(Value::Null)
                }
                None => Ok(Value::Boolean(st.borrow().lookup)),
            },
            // `P.SetRecord(Rec)` hands the page variable the record it shows when it runs (corpus
            // handlers/TestPageSetRecord.al).
            "SetRecord" => match args.first() {
                Some(Value::Record(r)) => {
                    // Inside a Page-typed modal handler the page holds the record already
                    // (handlers/PSModalPageHandlerTyped.al: GetRecord answers the record SetRecord handed).
                    if st.borrow().typed_handler && self.has_source(st) {
                        let rec = self.page_rec(st);
                        rec.borrow_mut().fields = r.borrow().fields.clone();
                        return Ok(Value::Null);
                    }
                    let copy = Rc::new(RefCell::new(r.borrow().clone()));
                    st.borrow_mut().set_record = Some(copy);
                    Ok(Value::Null)
                }
                other => unsupported(format!("Page.SetRecord with {}", other.describe())),
            },
            // `CurrPage.SetSelectionFilter(var Rec)` on a list the test client stands on a row of: the record is
            // filtered to that row (handlers/TestPageCloseTriggersSelectionCardPageContracts.al:239-250). A
            // record that already has filters, and a page on no row, are not pinned.
            "SetSelectionFilter" => {
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
                self.record_op("SetRecFilter", dst, &mut [])?;
                Ok(Value::Null)
            }
            // Which record a page variable reports after it ran is not pinned: no corpus test calls GetRecord
            // (docs/probes-needed.md). Refused when reached, like SetSelectionFilter.
            // `P.GetRecord(var Rec)` after the page ran: the row it stood on when it closed, the picked row after
            // OK and the last positioned row after Cancel too, without filters
            // (handlers/TestPageCloseTriggersSelectionCardPageContracts.al:250-285).
            "GetRecord" => {
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
            "SetTableView" => match args.first() {
                Some(Value::Record(r)) => {
                    let copy = Rc::new(RefCell::new(r.borrow().clone()));
                    let table = copy.borrow().table as usize;
                    let blank = self.initial_fields(table);
                    copy.borrow_mut().fields = blank;
                    st.borrow_mut().set_record = Some(copy);
                    Ok(Value::Null)
                }
                other => unsupported(format!("Page.SetTableView with {}", other.describe())),
            },
            "EnqueueBackgroundTask" => {
                self.require_open(st)?;
                self.bg_enqueue(st, args)
            }
            "CancelBackgroundTask" => {
                self.require_open(st)?;
                self.bg_cancel(st, args)
            }
            "RunPageBackgroundTask" => {
                self.require_open(st)?;
                self.bg_run_page_task(st, args)
            }
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
                    // Index 0 is pinned (handlers/TestPageRuntimeContracts.al:224-236: text and code);
                    // an index past the last error is assumed to raise the same (docs/probes-needed.md).
                    None => Err(error_code("Index out of bounds.", "IndexOutOfBounds")),
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
        st.borrow_mut().inited = false;
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
    fn adopt_instance(&mut self, from: &St, to: &St) {
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
        let mut t = to.borrow_mut();
        t.instance = instance;
        t.ext_instances = exts;
        t.inited = inited;
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
        for a in &page.assumes {
            al_builtins::assumptions::assume(*a);
        }
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
            s.window_from_start = false;
            s.window_eof = false;
            s.focus = None;
            s.focus_in_row = false;
            s.errors = Vec::new();
            s.refused = false;
            s.pending = None;
            s.result = None;
            s.editable_off = false;
            s.caption = None;
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
        // how it combines with the view of a record handed to Page.Run is not pinned.
        let source_view = engine.program.objects[st.borrow().object as usize]
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("SourceTableView"))
            .map(|(_, v)| crate::views::property_view(v.trim()));
        // The part control's SubPageView comes first: its filters land in the link group, its
        // sorting and order set the key and direction. The part page's own SourceTableView then
        // sets only the halves it names (corpus handlers/TestPartOwnSortVsSubPageView.al:1-45).
        let sub_view = self.sub_page_view(st).map(|v| crate::views::property_view(&v));
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
            // The platform applies the view in filter group 2: the page's own AL reads it with
            // FilterGroup(2) and finds group 0 empty (corpus handlers/TestPageSourceTableView.al:
            // FiltersAreInFilterGroup2WhenOnOpenPageRuns).
            self.set_view_in(&rec, &table, &view, Some(2), sub_view.is_some(), false)?;
        }
        if let (Some(r), true) = (&record, has_source) {
            let rec = self.page_rec(st);
            let src = r.borrow();
            let mut dst = rec.borrow_mut();
            dst.fields = src.fields.clone();
            dst.filters = src.filters.clone();
            dst.key = src.key;
            dst.adhoc = src.adhoc.clone();
            dst.temporary = src.temporary;
            dst.temp_id = src.temp_id.clone();
            dst.ascending = src.ascending;
        }
        let result = (|| {
            // A `Page` variable whose procedure ran before this open was constructed then, and its
            // OnInit has run (handlers/TestPageOnInit_Tests.al:188).
            if !std::mem::take(&mut st.borrow_mut().inited) {
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
                if self.lenient_bind {
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
            if self.declares(st, "onfindrecord") {
                return unsupported("OpenNew on a page with an OnFindRecord trigger (not pinned)");
            }
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

    fn var_slots(&self, st: &St) -> Vec<usize> {
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
        // Card, List, Document, Worksheet and ListPlus report OK (handlers/TestPageCloseActionByPageTypeContracts.al:
        // TestPageClose_IsOk_ForEveryPageType; a Card also in handlers/TestPageCloseTriggersSelectionCardPageContracts.al:
        // OnQueryClosePage_PageAndExtension_BothRun_AndBothMustAgreeToClose); the dialogs report Cancel.
        // Other page types are not pinned and keep Cancel.
        let object = st.borrow().object as usize;
        let ok_type = self.engine.program.objects[object].properties.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("PageType")
                && ["card", "list", "document", "worksheet", "listplus"]
                    .iter()
                    .any(|t| v.trim().eq_ignore_ascii_case(t))
        });
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
            self.immediate_errors = true;
            let inserted = self.record_op("Insert", &rec, &mut [Value::Boolean(true)]);
            self.immediate_errors = false;
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
            // A list handed over in lookup mode has no new-row line (corpus
            // handlers/TestPageModalNewRowLine_Tests.al:209 Plain_LookupModeTrue_HasNoNewRowLine). Only a
            // LookupMode(true) call does that: Page.RunModal(id, Rec) keeps the line (:180).
            && !(st.borrow().lookup_set && st.borrow().lookup && self.is_list_page(st))
            && !prop_false("Editable")
            && !prop_false("InsertAllowed"))
    }

    // ---- rows ---------------------------------------------------------------------------

    /// Whether the page declares trigger `name` (lower case) of its own.
    fn declares(&self, st: &St, name: &str) -> bool {
        let engine = self.engine();
        page_of(&engine, st).triggers.iter().any(|(n, _)| n == name)
    }

    /// FindFirst (`which` "-") or FindLast ("+") of the page's rowset. A page that declares
    /// OnFindRecord serves its rows from the trigger, which receives `Which` and answers whether it
    /// found a row (corpus pagefindrecord/TestPageFindRecord.al:112, :160). The strings the client
    /// really passes are `=`, `=>` and `=><` forms the corpus pins only for a deleted row
    /// (pageupdate/TestPageUpdateGoneRow.al:390); `-` and `+` are assumed (docs/probes-needed.md).
    fn nav_find(&mut self, st: &St, which: &str) -> Result<bool, Signal> {
        if self.declares(st, "onfindrecord") {
            let v = self.page_trigger(st, "onfindrecord", vec![Value::Text(which.into())])?;
            return Ok(matches!(v, Some(Value::Boolean(true))));
        }
        let rec = self.page_rec(st);
        self.ret_used = true;
        Ok(matches!(
            match which {
                "-" => self.record_op("FindFirst", &rec, &mut [])?,
                "+" => self.record_op("FindLast", &rec, &mut [])?,
                w => self.record_op("Find", &rec, &mut [Value::Text(w.into())])?,
            },
            Value::Boolean(true)
        ))
    }

    /// Next(steps) of the page's rowset: OnNextRecord when the page declares it, which answers how
    /// many rows it moved (corpus pagefindrecord/TestPageFindRecord.al:129).
    fn nav_next(&mut self, st: &St, steps: i32) -> Result<bool, Signal> {
        if self.declares(st, "onnextrecord") {
            let v = self.page_trigger(st, "onnextrecord", vec![Value::Integer(steps)])?;
            return Ok(matches!(v, Some(Value::Integer(n)) if n != 0));
        }
        let rec = self.page_rec(st);
        self.ret_used = true;
        let moved = self.record_op("Next", &rec, &mut [Value::Integer(steps)])?;
        Ok(matches!(moved, Value::Integer(n) if n != 0))
    }

    /// Lands on the first row of the view when the page opens: a stored row, else
    /// the new-row line, else nothing. The expressions of the controls are evaluated for
    /// an empty view too (corpus autoformat/TestAutoFormatExpressionTiming.al:5-12).
    fn page_first_row(&mut self, st: &St) -> Result<(), Signal> {
        // A page that serves its rows from OnFindRecord is asked `=><` when it opens (corpus
        // pageext/PCCurrRecordReads.al:FindList_OnFindRecordAndOnNextRecord, `open`).
        // That is pinned for the default order; a page with a SourceTableView (a descending order
        // among them) keeps the `-` assumed so far, as the client's blank key would sit at the other end.
        let object = st.borrow().object as usize;
        let has_view = self.engine.program.objects[object]
            .properties
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("SourceTableView"));
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
            r?;
            // The window starts at the first row of the view.
            let from_start = self.is_list(st);
            st.borrow_mut().window_from_start = from_start;
            Ok(())
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
        // handlers/TestPagePartOnNewRecordCount.al). A top-level list does the same, and both then
        // run OnAfterGetCurrRecord on the blank row (corpus testpart/TestPartTriggerOrderAndRefresh.al:
        // 258-273 TopLevelList_BlankLine_RunsOnNewRecord, Part_BlankLine_RunsOnNewRecord).
        self.stamp_from_filters(st, true)?;
        self.page_trigger(st, "onnewrecord", vec![Value::Boolean(true)])?;
        self.page_event(st, "onnewrecordevent", None, Some(true))?;
        self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
        self.page_event(st, "onaftergetcurrrecordevent", None, None)?;
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
                if matches!(self.record_op("FindFirst", &probe, &mut [])?, Value::Boolean(true)) {
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

    /// The caption a CaptionClass text stands for. Area 3 is the literal text. Other numeric areas
    /// are the platform's resolvers, which are not modelled. An area that is not a number, or no
    /// area at all, goes to no built-in resolver: the System Application's Caption Class codeunit
    /// raises OnResolveCaptionClass for it, and when no subscriber claims it the text stays as
    /// written (corpus handlers/TestPageCaptionClass.al:90
    /// TestPageField_Caption_UnresolvedCaptionClass_ReturnsExpressionUnchanged;
    /// autoformat/TestCaptionClassResolverFailure.al: an error the subscriber raises reaches the
    /// open, and a subscriber that claims nothing changes nothing). The language handed to the
    /// event is the session language (types/PSCaptionClass.al).
    fn caption_class_text(&mut self, st: &St, text: &str) -> Result<String, Signal> {
        if let Some(rest) = text.strip_prefix("3,") {
            return Ok(rest.to_string());
        }
        let (area, expr) = text.split_once(',').unwrap_or(("", text));
        if text.contains(',') && area.trim().parse::<u32>().is_ok() {
            return unsupported(format!("CaptionClass `{text}` is resolved by the platform"));
        }
        let _ = st;
        self.raise_resolve_caption_class(area, expr, text)
    }

    /// `CaptionClassTranslate(Text)`: area 3 gives the text after the first comma without an event ('3,One,Two' is
    /// `One,Two`, '3,' is empty); a text with no comma and the empty text come back unchanged without an event; any other
    /// area raises OnResolveCaptionClass of the Caption Class codeunit with the area, the rest after the first comma and the
    /// session language, and comes back unchanged when no subscriber resolves it (corpus types/PSCaptionClass.al:5-8,
    /// CaptionClassTranslate_Expressions).
    pub(crate) fn caption_class_translate(&mut self, text: &str) -> Result<String, Signal> {
        let Some((area, expr)) = text.split_once(',') else {
            return Ok(text.to_string());
        };
        if area == "3" {
            return Ok(expr.to_string());
        }
        self.raise_resolve_caption_class(area, expr, text)
    }

    fn raise_resolve_caption_class(&mut self, area: &str, expr: &str, text: &str) -> Result<String, Signal> {
        if !self
            .engine
            .events
            .is_subscribed(crate::events::CAPTION_CLASS, "onresolvecaptionclass")
        {
            return Ok(text.to_string());
        }
        let names: Vec<String> = ["CaptionArea", "CaptionExpr", "Language", "Caption", "Resolved"]
            .iter()
            .map(|n| n.to_string())
            .collect();
        let mut args = vec![
            Value::Text(area.to_string()),
            Value::Text(expr.to_string()),
            Value::Integer(self.global_language),
            Value::Text(String::new()),
            Value::Boolean(false),
        ];
        let call = crate::dispatch::EventCall {
            object: crate::events::CAPTION_CLASS,
            event: "onresolvecaptionclass",
            element: None,
            isolated: false,
            names: &names,
            sender: None,
        };
        self.raise(&call, &mut args)?;
        Ok(match (&args[4], &args[3]) {
            (Value::Boolean(true), Value::Text(t) | Value::Code(t)) => t.clone(),
            _ => text.to_string(),
        })
    }

    /// The expressions every row population evaluates, whatever the control's Visible
    /// (corpus autoformat/TestAutoFormatExpressionTiming.al:5-12): AutoFormatExpression and
    /// CaptionClass. They run after OnOpenPage and before OnAfterGetRecord.
    fn populate_expressions(&mut self, st: &St) -> Result<(), Signal> {
        let engine = self.engine();
        let page = page_of(&engine, st);
        for (idx, c) in page.controls.iter().enumerate() {
            for name in ["autoformatexpression", "captionclass"] {
                match c.prop(name) {
                    Some(routine @ (ir::PropVal::Routine(_) | ir::PropVal::Ext(..))) => {
                        let v = self
                            .prop_routine_value(st, idx, routine)?
                            .expect("a routine-valued property");
                        if name == "captionclass" {
                            if let Value::Text(t) | Value::Code(t) = &v {
                                self.caption_class_text(st, t)?;
                            }
                        }
                    }
                    // An area no built-in resolver claims goes to the Caption Class codeunit's
                    // OnResolveCaptionClass event (corpus autoformat/TestCaptionClassResolverFailure.al:1-12).
                    Some(ir::PropVal::Lit(l)) if name == "captionclass" => {
                        self.caption_class_text(st, &unquote(l))?;
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
        st.borrow_mut().window_from_start = false;
        st.borrow_mut().window_eof = false;
        let rec = self.page_rec(st);
        let start = self.row_key(&rec)?;
        let mut keys = Vec::new();
        let mut walked = 0;
        let mut end_reached = false;
        let opening = std::mem::take(&mut st.borrow_mut().landing_first);
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
            if !self.nav_next(st, 1)? {
                end_reached = true;
                break;
            }
            walked += 1;
        }
        // A view that ends inside the window: the client puts its first row back without asking
        // the page, reads it again, then asks OnNextRecord(-1) and OnFindRecord(`=`) and reads it
        // once more (corpus pageext/PCCurrRecordReads.al:List_ReadsAfterEachAction and
        // FindList_OnFindRecordAndOnNextRecord, `open` of a list of 3 rows: five reads). Only the
        // count is pinned, for a table of 3 rows.
        let find_page = self.declares(st, "onnextrecord") || self.declares(st, "onfindrecord");
        let end_reached = end_reached && opening;
        if end_reached && find_page && self.declares(st, "onaftergetrecord") {
            if let Some(first) = keys.first() {
                rec.borrow_mut().fields = first.stored.clone();
            }
            self.page_trigger(st, "onaftergetrecord", vec![])?;
            if self.declares(st, "onnextrecord") {
                self.nav_next(st, -1)?;
            }
            if self.declares(st, "onfindrecord") {
                self.nav_find(st, "=")?;
            }
            if let Some(row) = keys.first() {
                rec.borrow_mut().fields = row.shown.clone();
                self.set_xrec_fields(st, row.stored.clone());
            }
            st.borrow_mut().window = keys;
            self.page_trigger(st, "onaftergetrecord", vec![])?;
            return Ok(());
        }
        // Back on the row the cursor is on. A page that serves its rows from triggers steps back
        // through them: the row is not necessarily in the table.
        if self.declares(st, "onnextrecord") || self.declares(st, "onfindrecord") {
            for _ in 0..walked {
                if !self.nav_next(st, -1)? {
                    return unsupported("OnNextRecord did not step back over the rows it served");
                }
            }
        } else {
            self.ret_used = true;
            self.record_op("Get", &rec, &mut start.clone())?;
        }
        // The row buffer the page keeps for the row it stands on is what OnAfterGetRecord left.
        if let Some(row) = keys.first() {
            rec.borrow_mut().fields = row.shown.clone();
            self.set_xrec_fields(st, row.stored.clone());
        }
        st.borrow_mut().window = keys;
        // The row the cursor stands on is read once more after the window: a list of 60 rows
        // raises OnAfterGetRecord 21 times when it opens, rows 1 to 20 and then row 1 again
        // (corpus handlers/TestPageRuntimeContracts.al:121-136 TestPage_List_OpenReads21Rows).
        self.page_trigger(st, "onaftergetrecord", vec![])?;
        if end_reached {
            self.page_trigger(st, "onaftergetrecord", vec![])?;
        }
        Ok(())
    }

    /// A GoToKey that goes past the rows a list has read reads on in blocks: the last two rows of the
    /// block before, then 20 new rows (fewer at the end of the view). The cursor stands on the first
    /// unread row `X` and the window holds every row before it. Returns whether the block was full, the key
    /// of its last row, and leaves the cursor on `X` again with the whole block in the window.
    /// Pinned on a real service tier by corpus handlers/TestPageSystemPartListWindowContracts.al:194
    /// (List_Window_AfterGoToKeyOutsideTheRowsRead), :210 (List_Window_SmallTable) and
    /// handlers/TestPageRuntimeContracts.al:331.
    fn read_window_block(&mut self, st: &St) -> Result<(bool, Vec<Value>), Signal> {
        let rec = self.page_rec(st);
        let start = self.row_key(&rec)?;
        let prefix = st.borrow().window.len().min(2);
        let trigger_rows = self.declares(st, "onnextrecord") || self.declares(st, "onfindrecord");
        if prefix > 0 && !self.nav_next(st, -(prefix as i32))? {
            return unsupported("a list that cannot step back over the rows it has read");
        }
        let mut walked = 0usize;
        let mut new_rows = 0usize;
        let mut last_key = start.clone();
        for i in 0..prefix + Self::WINDOW {
            let key = self.row_key(&rec)?;
            let stored = rec.borrow().fields.clone();
            self.populate_current(st)?;
            let row = al_builtins::value::WindowRow {
                key: key.clone(),
                stored,
                shown: rec.borrow().fields.clone(),
            };
            {
                let mut s = st.borrow_mut();
                match s.window.iter().position(|w| Self::same_key(&w.key, &key)) {
                    Some(at) => s.window[at] = row,
                    None => s.window.push(row),
                }
            }
            if i >= prefix {
                new_rows += 1;
                last_key = key;
            }
            if i + 1 == prefix + Self::WINDOW {
                break;
            }
            if !self.nav_next(st, 1)? {
                break;
            }
            walked += 1;
        }
        // Back on `X`, which is `prefix` rows into the walk.
        if trigger_rows {
            for _ in prefix..walked {
                if !self.nav_next(st, -1)? {
                    return unsupported("OnNextRecord did not step back over the rows it served");
                }
            }
        } else {
            self.ret_used = true;
            self.record_op("Get", &rec, &mut start.clone())?;
        }
        Ok((new_rows == Self::WINDOW, last_key))
    }

    /// The last two rows of the window are read once more: what a read that reached the end of the view
    /// does (corpus handlers/TestPageSystemPartListWindowContracts.al:194, handlers/TestPageRuntimeContracts.al:331).
    /// The cursor is on the last row of the view and stays there.
    fn reread_last_rows(&mut self, st: &St) -> Result<(), Signal> {
        let rec = self.page_rec(st);
        let n = st.borrow().window.len();
        if n >= 2 {
            if !self.nav_next(st, -1)? {
                return unsupported("a list that cannot step back over its last row");
            }
            self.populate_current(st)?;
            self.nav_next(st, 1)?;
        }
        self.populate_current(st)?;
        let _ = rec;
        st.borrow_mut().window_eof = true;
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
        st.borrow_mut().stale = false;
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
        let anchor = st.borrow_mut().after_new.take();
        if let Left::Stay = self.leave_row(st, how)? {
            return Ok(Value::Boolean(false));
        }
        // A new line saved in the middle of a list: Next() goes to the row below the one it was
        // started under, not to the row after its own key (corpus
        // handlers/ProbeTestPageForms.al:227-253, where a row with the last key, added below row 2,
        // is followed by row 3 and not by the blank line).
        let cursor_now = st.borrow().cursor;
        if let ("Next", Some(anchor), Cursor::Row) = (how, anchor, cursor_now) {
            if self.next_below_anchor(st, anchor)? {
                st.borrow_mut().cursor = Cursor::Row;
                self.arrive(st)?;
                return Ok(Value::Boolean(true));
            }
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
                    Err(error_code(NOT_OPEN, "TestPageNotOpened"))
                }
            }
            other => other,
        }
    }

    /// Moves the page's record to the row after `anchor` in key order, when there is one and it is
    /// not the row just saved; else leaves the record as it was and answers false.
    fn next_below_anchor(&mut self, st: &St, anchor: Vec<Value>) -> Result<bool, Signal> {
        if self.declares(st, "onnextrecord") || self.declares(st, "onfindrecord") {
            return Ok(false);
        }
        let rec = self.page_rec(st);
        let saved = rec.borrow().fields.clone();
        rec.borrow_mut().fields = anchor;
        self.ret_used = true;
        let mut pk = self.table(&rec)?.primary_key(&rec.borrow().fields);
        let found = matches!(self.record_op("Get", &rec, &mut pk)?, Value::Boolean(true));
        let moved = found && {
            self.ret_used = true;
            matches!(self.record_op("Next", &rec, &mut [Value::Integer(1)])?, Value::Integer(n) if n != 0)
        };
        let table = self.table(&rec)?;
        let same = moved && Self::same_values(&table.primary_key(&rec.borrow().fields), &table.primary_key(&saved));
        if moved && !same {
            return Ok(true);
        }
        rec.borrow_mut().fields = saved;
        Ok(false)
    }

    /// A move of a list whose whole view the client has read (fewer rows than a window) is served
    /// from the client's rows: the new row is read, a page with OnFindRecord is asked `=` for it,
    /// the row is read again, and OnAfterGetCurrRecord ends the move; `Last` on a page without
    /// OnFindRecord starts with an OnAfterGetCurrRecord (corpus pageext/PCCurrRecordReads.al:
    /// List_ReadsAfterEachAction `next`, `last`, `first`; FindList_OnFindRecordAndOnNextRecord
    /// `next`, `last`, `previous`, `first`, pinned for a table of 3 rows). `None` when the move is not
    /// one of those.
    fn move_in_client_rows(&mut self, st: &St, how: &str) -> Result<Option<bool>, Signal> {
        let find_page = self.declares(st, "onfindrecord");
        if !self.declares(st, "onaftergetrecord")
            || !self.is_list(st)
            || st.borrow().cursor != Cursor::Row
            || st.borrow().draft
            || st.borrow().stale
            || !st.borrow().window_from_start
            || st.borrow().window.is_empty()
            || st.borrow().window.len() >= Self::WINDOW
        {
            return Ok(None);
        }
        let rec = self.page_rec(st);
        let key = self.row_key(&rec)?;
        let window = st.borrow().window.clone();
        let Some(at) = window.iter().position(|w| Self::same_key(&w.key, &key)) else {
            return Ok(None);
        };
        let target = match how {
            "First" => 0,
            "Last" => window.len() - 1,
            "Next" if at + 1 < window.len() => at + 1,
            "Previous" if at > 0 => at - 1,
            _ => return Ok(None),
        };
        if how == "Last" && !find_page {
            self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
            self.page_event(st, "onaftergetcurrrecordevent", None, None)?;
        }
        self.read_client_row(st, target)?;
        Ok(Some(true))
    }

    /// The reads of a move onto row `target` of the rows the client holds (see `move_in_client_rows`).
    fn read_client_row(&mut self, st: &St, target: usize) -> Result<(), Signal> {
        let rec = self.page_rec(st);
        let stored = st.borrow().window[target].stored.clone();
        rec.borrow_mut().fields = stored;
        self.populate_current(st)?;
        if self.declares(st, "onfindrecord") {
            self.nav_find(st, "=")?;
        }
        self.populate_current(st)?;
        let shown = rec.borrow().fields.clone();
        if let Some(w) = st.borrow_mut().window.get_mut(target) {
            w.shown = shown;
        }
        self.page_trigger(st, "onaftergetcurrrecord", vec![])?;
        self.page_event(st, "onaftergetcurrrecordevent", None, None)?;
        Ok(())
    }

    fn page_move_inner(&mut self, st: &St, how: &str) -> Result<bool, Signal> {
        if let Some(r) = self.move_in_client_rows(st, how)? {
            return Ok(r);
        }
        let cursor = st.borrow().cursor;
        let landed = match how {
            "First" => self.nav_find(st, "-")?,
            "Last" => self.nav_find(st, "+")?,
            "Next" => match cursor {
                Cursor::Row => {
                    if self.nav_next(st, 1)? {
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
                Cursor::NewLine => self.nav_find(st, "+")?,
                Cursor::Row => self.nav_next(st, -1)?,
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
        // Texts and codes: handlers/TestPageRuntimeContracts.al:300-312 (TestPart_GoToKey_Arity_AndMissingKey).
        if st.borrow().host.is_some() {
            let rec = self.page_rec(st);
            let wanted = self.table(&rec)?.keys[0].len();
            if key.len() != wanted {
                return Err(error_code(
                    "The number of arguments does not match the number of primary key fields.",
                    "TestInvalidNumberOfKeyFieldValues",
                ));
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
        // A list that serves its rows from OnFindRecord and holds its whole view goes to a row it
        // holds as a move does (corpus pageext/PCCurrRecordReads.al:FindList_OnFindRecordAndOnNextRecord
        // `gotokey_2`, a table of 3 rows).
        if self.declares(st, "onfindrecord")
            && self.declares(st, "onaftergetrecord")
            && self.is_list(st)
            && st.borrow().cursor == Cursor::Row
            && !st.borrow().draft
            && !st.borrow().stale
        {
            let held = {
                let s = st.borrow();
                (s.window_from_start && s.window.len() < Self::WINDOW)
                    .then(|| s.window.iter().position(|w| Self::same_key(&w.key, &want)))
                    .flatten()
            };
            if let Some(at) = held {
                self.read_client_row(st, at)?;
                return Ok(Value::Boolean(true));
            }
        }
        let before_fields = rec.borrow().fields.clone();
        let (before_cursor, before_draft) = (st.borrow().cursor, st.borrow().draft);
        let mut at = self.nav_find(st, "-")?;
        let mut found = false;
        // The last block this scan read: whether it was full and the key of its last row.
        let mut block: Option<(bool, Vec<Value>)> = None;
        while at {
            st.borrow_mut().cursor = Cursor::Row;
            if self.is_list(st) && st.borrow().window_from_start && !st.borrow().window.is_empty() {
                let key = self.row_key(&rec)?;
                if !self.in_window(st, &key) {
                    let r = self.read_window_block(st);
                    block = Some(self.torn_down_on_error(st, r)?);
                }
            }
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
            at = self.nav_next(st, 1)?;
        }
        if found {
            // A full block whose last row is the target: the read goes on. At the end of the view it reads
            // the last two rows once more (handlers/TestPageRuntimeContracts.al:331, row 60). Where more
            // rows follow, nothing pins what is read.
            if let Some((true, last)) = &block {
                if Self::same_key(&self.row_key(&rec)?, last) {
                    if self.nav_next(st, 1)? {
                        self.nav_next(st, -1)?;
                        return unsupported(
                            "GoToKey onto the last row of a full block of rows read, with more rows after it (not pinned)",
                        );
                    }
                    let r = self.reread_last_rows(st);
                    self.torn_down_on_error(st, r)?;
                }
            }
        }
        if !found {
            // A miss reads on to the end of the view and then the last two rows once more
            // (handlers/TestPageSystemPartListWindowContracts.al:194, key 100).
            if block.is_some() && !st.borrow().window_eof {
                let r = self.reread_last_rows(st);
                self.torn_down_on_error(st, r)?;
            }
            rec.borrow_mut().fields = before_fields;
            let mut s = st.borrow_mut();
            s.cursor = before_cursor;
            s.draft = before_draft;
            drop(s);
            if st.borrow().host.is_some() && !result_used {
                return Err(error_code("The row does not exist on the TestPage.", "TestRowNotFound"));
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
        let result_used = self.ret_used;
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
        // A FindFirstField whose result is not used raises when no row carries the value
        // (handlers/TestPagePlatformPageSourceTable_Tests.al:125-131, the whole message). The same
        // for FindNextField and FindPreviousField is not pinned: they keep answering false.
        if how == "FindFirstField" && !result_used {
            return Err(error_code("The row does not exist on the TestPage.", "TestRowNotFound"));
        }
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
                // The trapped variable receives the page and keeps it open for the test. A `Page`
                // variable that was set up first hands its instance over
                // (handlers/TestPageOnInit_Tests.al:223 POI_SetterBeforeTrappedRun_...).
                self.adopt_instance(&st, &trap);
                st = trap;
                trapped = true;
            }
        }
        if st.borrow().open {
            return unsupported("running a page that is already open");
        }
        if modal {
            if let Some(handler) = self.take_typed_modal_handler(object) {
                return self.answer_typed_modal(&st, record, handler);
            }
        }
        let kind = if modal { UiKind::ModalPage } else { UiKind::Page };
        st.borrow_mut().modal = modal && !trapped;
        let handler = self.open_page(&st, PageMode::Edit, record, if trapped { None } else { Some(kind) })?;
        let Some(handler) = handler else {
            return Ok(Value::Null);
        };
        self.answer_page(&st, handler, modal)
    }

    /// A `[ModalPageHandler]` typed as the Page itself (handlers/PSModalPageHandlerTyped.al): the handler gets the page
    /// object the code ran, never opened. `GetRecord` gives the record `RunModal` was given (an empty one when none),
    /// `LookupMode` is Yes, `SetRecord` and `GetRecord` work on it, `Response` arrives as None, and the value the
    /// handler leaves in it is what `Page.RunModal` answers. Whether the page's own triggers run is not pinned, so a
    /// page that declares one is refused.
    fn answer_typed_modal(&mut self, st: &St, record: Option<Rec>, handler: Handler) -> Result<Value, Signal> {
        for t in [
            "oninit",
            "onopenpage",
            "onclosepage",
            "onqueryclosepage",
            "onaftergetrecord",
            "onaftergetcurrrecord",
            "onfindrecord",
            "onnextrecord",
        ] {
            if self.declares(st, t) {
                return unsupported(format!(
                    "a Page-typed ModalPageHandler for a page that declares {t} (whether its triggers run is not pinned)"
                ));
            }
        }
        self.page_instance(st)?;
        if self.has_source(st) {
            if let Some(r) = &record {
                let rec = self.page_rec(st);
                rec.borrow_mut().fields = r.borrow().fields.clone();
            }
        }
        {
            let mut s = st.borrow_mut();
            s.lookup = true;
            s.typed_handler = true;
            s.cursor = Cursor::Row;
        }
        let (hobject, hroutine, hinst) = handler;
        let none = al_builtins::value::ACTION_MEMBERS
            .iter()
            .position(|m| *m == "None")
            .unwrap_or(0) as i32;
        let members: std::sync::Arc<[String]> = al_builtins::value::ACTION_MEMBERS
            .iter()
            .map(|m| m.to_string())
            .collect();
        self.page_handler_depth += 1;
        let handled = self.call(
            hobject,
            hroutine,
            hinst,
            vec![Value::TestPage(st.clone()), Value::Option(none, members)],
        );
        self.page_handler_depth -= 1;
        {
            let mut s = st.borrow_mut();
            s.typed_handler = false;
            s.cursor = Cursor::None;
        }
        let (_, finals) = handled?;
        match finals.get(1) {
            Some(v @ Value::Option(..)) => Ok(v.clone()),
            other => unsupported(format!(
                "a Page-typed ModalPageHandler left {} in Response",
                other.describe()
            )),
        }
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
                // The platform closes the page the handler left open, and OnQueryClosePage
                // raises again: a second message, a second refusal. The caller gets control back
                // with the page gone and no action completed; the page's writes stay (corpus
                // handlers/TestPageQueryCloseMessageHandler_Tests.al:
                // ErrorInQueryClosePage_MessageConsumed_CallerRegainsControl, ..._SettlesThePagesUncommittedWrite).
                // A refusal by a plain `false` is not pinned for a modal page.
                if !(modal && st.borrow().close_error) {
                    return unsupported("a page whose OnQueryClosePage refused the platform's close");
                }
                self.mark_closed(&st);
                st.borrow_mut().result = Some("None".into());
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
        let members: std::sync::Arc<[String]> = al_builtins::value::ACTION_MEMBERS
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
        // The page saves the row on fresh table globals: what an action set in them is gone (corpus
        // record/PWTableGlobals.al:408-431: OnModify reads Counter 0 though the action set 7).
        self.reset_table_globals(&rec);
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
        // BelowxRec is Yes for a row added with New() at the end, as it is for OnNewRecord (corpus
        // handlers/TestPageRuntimeContracts.al:197-211 TestPage_New_AtTheEnd_PassesBelowxRecTrueToOnInsertRecord).
        // A row added in the middle is not pinned; it is given the same Yes as OnNewRecord gets.
        let go = self.page_trigger_answer(st, "oninsertrecord", vec![Value::Boolean(true)])?;
        if go == Some(false) {
            return Ok(());
        }
        if !self.page_event(st, "oninsertrecordevent", Some(true), Some(true))? {
            return Ok(());
        }
        self.auto_split_key(st)?;
        self.ret_used = false;
        self.immediate_errors = true;
        let inserted = self.record_op("Insert", &rec, &mut [Value::Boolean(true)]);
        self.immediate_errors = false;
        inserted?;
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
            ..self.control_format(st, idx)?
        };
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
                let msg = format!("Your entry of '{text}' is not an acceptable value for '{caption}'.");
                return Err(refuse(self, msg));
            }
            Err(al_builtins::pagefmt::ParseError::Unsupported) => {
                return unsupported(format!(
                    "SetValue of a {ty} control: its spelling is not modelled yet",
                    ty = ty.describe()
                ));
            }
        };
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
                    self.curr_field_no.push((Rc::as_ptr(&rec) as usize, number));
                    // A validation through the page runs the table's OnValidate on fresh table
                    // globals, and leaves the page's record with fresh globals (ADR-021 decision 8;
                    // corpus probes TableGlobals_PageFieldValidate_*, TableGlobals_PageRec_ActionGlobalsAreLostAfterPageFieldValidation).
                    self.reset_table_globals(&rec);
                    let validated = self.record_op("Validate", &rec, &mut [Value::Integer(pos as i32), parsed.clone()]);
                    self.reset_table_globals(&rec);
                    self.curr_field_no.pop();
                    validated?;
                }
                ir::Binding::Var(_) | ir::Binding::BaseVar(_) => {
                    let (slot, owner) = ctl.binding.var_slot(ctl.owner).expect("a variable binding");
                    let inst = self.owner_instance(st, owner);
                    inst.borrow_mut().globals[slot] = parsed.clone();
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
                    let enabled = match (edit, card) {
                        (false, _) => true,
                        (true, None) => false,
                        (true, Some(target)) => self.card_allows_edit(target)?,
                    };
                    return match m {
                        "Visible" => Ok(Value::Boolean(true)),
                        "Enabled" => Ok(Value::Boolean(enabled)),
                        "Invoke" if !enabled || card.is_none() => Ok(Value::Null),
                        "Invoke" => self.list_card_action(st, edit),
                        other => unsupported(format!(
                            "TestAction.{other} of the built-in {name} action of a list page"
                        )),
                    };
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
            // The built-in OK and Cancel are visible (report/TestProbeReportContracts.al:RequestPage_InvokeActionNotFound).
            ControlTarget::BuiltIn(_) if m == "Visible" => return Ok(Value::Boolean(true)),
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
        // The compiler emits no call for a `Value()` whose result is thrown away: nothing is
        // evaluated, on a control that is on the page or not (corpus
        // pageapplicationarea/TestPageDiscardedFieldValueCall.al: RemovedControl_DiscardedValueCall_RaisesNothing,
        // FoundControl_DiscardedValueCall_RaisesNothing).
        if m == "Value" && args.is_empty() && !self.ret_used {
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
        if m != "Invoke" {
            let engine = self.engine();
            let page = page_of(&engine, st);
            let ctl = &page.controls[idx];
            if let Some(action_ref) = &ctl.action_ref {
                let target_plain = match action_ref {
                    ir::ActionRef::Target(t) => page.controls[*t].prop("visible").is_none(),
                    ir::ActionRef::Unresolved(_) => false,
                };
                if m == "Visible" && target_plain && ctl.prop("visible").is_none() {
                    return Ok(Value::Boolean(true));
                }
                // handlers/TestPageRunObjectContracts.al:392-404: Visible and Enabled of an actionref answer Yes.
                let target_enabled_plain = match action_ref {
                    ir::ActionRef::Target(t) => page.controls[*t].prop("enabled").is_none(),
                    ir::ActionRef::Unresolved(_) => false,
                };
                if m == "Enabled" && target_enabled_plain && ctl.prop("enabled").is_none() {
                    return Ok(Value::Boolean(true));
                }
                return unsupported(format!("TestAction.{m} of an actionref"));
            }
        }
        match m {
            // `Ctl.Value := x` is the setter form of the property, and does what SetValue does
            // (corpus handlers/ProbeTestPageForms.al:109-131: OnValidate runs with the assigned text).
            "Value" if !args.is_empty() && self.in_cue_group(st, idx) => Ok(Value::Null),
            "Value" if !args.is_empty() => {
                let r = self.control_set_value(st, idx, args)?;
                self.resync_parts_linked_by(st, idx)?;
                Ok(r)
            }
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
                    // ShowMandatory is true when the page says nothing (handlers/TestProbePageHandlerContracts.al:
                    // TestPage_Members_ShowMandatoryHideValue_Defaults); HideValue is false.
                    None => Ok(Value::Boolean(m == "ShowMandatory")),
                    Some(ir::PropVal::Lit(l)) => Ok(Value::Boolean(unquote(l).eq_ignore_ascii_case("true"))),
                    Some(_) => unsupported(format!("TestField.{m} of a computed property")),
                }
            }
            "Editable" => Ok(Value::Boolean(self.control_editable(st, idx)?)),
            "Enabled" => Ok(Value::Boolean(self.control_enabled(st, idx)?)),
            "Visible" => Ok(Value::Boolean(self.control_visible(st, idx)?)),
            "Invoke" => self.action_invoke(st, idx),
            "DrillDown" | "AssistEdit" | "Lookup" => self.control_navigate(st, idx, m),
            // SetValue on a field of a cuegroup does nothing and raises nothing
            // (handlers/TestPageCueGroupFixedGridContracts.al:CueGroup_SetValue_IsSilentlyIgnored).
            "SetValue" if self.in_cue_group(st, idx) => Ok(Value::Null),
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

    /// An action with no `OnAction` trigger (its own or a page extension's) and no `RunObject` is not
    /// on the TestPage: naming it raises TestActionNotFound (handlers/TestPageLabelControlContracts.al:
    /// 131-145, BC 27.5). Whether other behaviour (a `RunPageLink` without a `RunObject`, a report
    /// action) also counts is not pinned.
    fn action_without_behaviour(&self, st: &St, idx: usize) -> bool {
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

    /// `TestField.DrillDown`, `AssistEdit` and `Lookup`: run the control's trigger of that name, the
    /// one it states itself or the one a page extension's `modify` block adds (corpus
    /// pageextensiontrigger/TestModifiedControlValidateTriggers_Tests.al: AModifyBlocksOnDrillDownRunsForTheModifiedControl,
    /// AModifyBlocksOnLookupRunsForTheModifiedControl, ABaseControlsOwnOnAssistEditRuns,
    /// AModifyBlocksOnAssistEditRunsForTheModifiedControl, OnAssistEditIsRaisedOnlyForTheControlItWasCalledOn).
    /// A DrillDown on a control with no trigger raises a fixed platform error (handlers/TestPageFieldDrillDown_Tests.al:
    /// FieldDrillDownWithNoTriggerIsRefused). What AssistEdit and Lookup do without a trigger (a table
    /// relation's lookup page) is not modelled.
    fn control_navigate(&mut self, st: &St, idx: usize, method: &str) -> Result<Value, Signal> {
        let engine = self.engine();
        let ctl = &page_of(&engine, st).controls[idx];
        let name = method.to_ascii_lowercase();
        let name = match name.as_str() {
            "drilldown" => "ondrilldown",
            "assistedit" => "onassistedit",
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
            [] if method == "DrillDown" => {
                return Err(error("The NavDrilldownAction method is not supported."));
            }
            [] if method == "Lookup" => return self.field_lookup(st, idx),
            [] => return unsupported(format!("TestField.{method} on a control with no {name} trigger")),
            _ => {
                return unsupported(format!(
                    "TestField.{method} on a control with more than one {name} trigger"
                ));
            }
        };
        st.borrow_mut().refresh = false;
        let result = if method == "Lookup" {
            // OnLookup(var Text): Boolean. The text it writes back replaces the field's value, and
            // the Boolean says whether the user picked anything (corpus
            // handlers/TestPageFieldTableLookup.al:72 Lookup_ControlWithTrigger_WinsOverTheTableFieldsOnLookup,
            // handlers/TestPageModalHandler_Tests.al:132). The text the trigger starts with is the
            // control's current text (assumed, docs/probes-needed.md).
            let object = owner.unwrap_or(st.borrow().object as usize);
            let inst = self.owner_instance(st, owner);
            let current = self.control_value(st, idx)?;
            let shown = self.control_text(st, idx, &current)?;
            self.call(object, routine, inst, vec![Value::Text(shown)])
                .and_then(|(v, finals)| match (v, finals.first()) {
                    (Value::Boolean(true), Some(Value::Text(t))) => {
                        // Written back as the client would type it.
                        self.control_set_value(st, idx, &mut [Value::Text(t.clone())])
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
    fn field_lookup(&mut self, st: &St, idx: usize) -> Result<Value, Signal> {
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
                o.kind == "page" && (o.name.eq_ignore_ascii_case(&name) || o.id.is_some_and(|i| i.to_string() == name))
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
    fn list_card_page(&self, st: &St) -> Result<Option<usize>, Signal> {
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
                o.kind == "page" && (o.name.eq_ignore_ascii_case(&card) || o.id.is_some_and(|i| i.to_string() == card))
            })
            .map(Some)
            .ok_or_else(|| Signal::Unsupported("CardPageId names a page outside the workspace".into()))
    }

    /// Whether the card a list's Edit action would open can be edited. Only `Editable = false`
    /// is pinned (corpus handlers/TestPageModeSwitchAction_Tests.al:
    /// ListWhoseCardIsReadOnlyLeavesTheEditActionVisibleButNotEnabled); any other restriction is refused.
    fn card_allows_edit(&self, card: usize) -> Result<bool, Signal> {
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
    fn list_card_action(&mut self, st: &St, edit: bool) -> Result<Value, Signal> {
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
            o.kind == "page" && (o.name.eq_ignore_ascii_case(&card) || o.id.is_some_and(|i| i.to_string() == card))
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
            && !self.declares(st, "onfindrecord")
            && !self.declares(st, "onnextrecord")
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
        let reads_rows = (!was_dirty || dirty_reread_only)
            && self.declares(st, "onaftergetrecord")
            && !self.declares(st, "onfindrecord")
            && !self.declares(st, "onnextrecord")
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
                if !Self::same_values(&rec.borrow().fields, &before) {
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
            if !self.declares(st, "onfindrecord") && !self.declares(st, "onnextrecord") {
                // The reads the page does when it opens come first, then the refresh's own: a card
                // reads its row twice, a list the rows of its window as it opens plus one (corpus
                // pageext/PCCurrRecordReads.al:Card_ReadsAfterEachAction, List_ReadsAfterEachAction,
                // pinned for a card and a list of 3 rows with the cursor on the first).
                if reads_rows && !st.borrow().refresh {
                    self.read_page_again(st)?;
                }
                st.borrow_mut().refresh = true;
            }
            self.run_refresh(st)?;
        }
        Ok(Value::Null)
    }

    /// The reads of an action that did not call `CurrPage.Update`, before the refresh's own (see
    /// `action_invoke`).
    fn read_page_again(&mut self, st: &St) -> Result<(), Signal> {
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
        let first = {
            let s = st.borrow();
            s.window_from_start
                && s.window.len() < Self::WINDOW
                && s.window.first().is_some_and(|w| Self::same_key(&w.key, &key))
        };
        // With the cursor off the first row, or more rows than a window, the reads are not pinned: the
        // refresh alone stays (docs/probes-needed.md).
        if !first {
            return Ok(());
        }
        st.borrow_mut().landing_first = true;
        let r = self.populate_window(st);
        st.borrow_mut().landing_first = false;
        r?;
        st.borrow_mut().window_from_start = true;
        Ok(())
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
        // A page that serves its rows from OnFindRecord asks the trigger to settle the client on a row
        // again (corpus pageupdate/TestPageUpdateGoneRow.al:390 pins the Which strings of a deleted
        // row only). Which row that is, is not modelled: nothing that shows it may be read before the
        // cursor moves, and the page's row triggers must not matter.
        if self.declares(st, "onfindrecord") || self.declares(st, "onnextrecord") {
            if self.declares(st, "onaftergetrecord") || self.declares(st, "onaftergetcurrrecord") {
                return unsupported(
                    "CurrPage.Update on a page with OnFindRecord and row triggers (re-anchoring not pinned)",
                );
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
            self.record_op("Get", &rec, &mut pk)?;
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
    fn settle_gone_row_with_find(&mut self, st: &St, rec: &Rec) -> Result<(), Signal> {
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
        if matches!(self.record_op("FindFirst", &probe, &mut [])?, Value::Boolean(true)) {
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
            ir::Binding::Caption => Ok(Value::Text(self.control_caption(st, idx)?)),
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
                Some(routine) => match self.prop_routine_value(st, idx, routine)? {
                    Some(Value::Text(t) | Value::Code(t)) => Some(t),
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
    fn auto_format_places(&mut self, kind: &str, expr: &str) -> Result<(u32, u32), Signal> {
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
        let Ok(key) = ops::convert(Value::Text(expr.to_string()), &t.fields[0].ty) else {
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

    fn control_text(&mut self, st: &St, idx: usize, v: &Value) -> Result<String, Signal> {
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
        Ok(pagefmt::control_text(v, &f, &caption))
    }

    /// The type of the value a control holds.
    fn control_type(&self, st: &St, idx: usize, v: &Value) -> Option<RtType> {
        if let Some(f) = self.control_field(st, idx) {
            return Some(f.ty);
        }
        let engine = self.engine();
        let ctl = &page_of(&engine, st).controls[idx];
        if let Some((slot, owner)) = ctl.binding.var_slot(ctl.owner) {
            let object = owner.unwrap_or(st.borrow().object as usize);
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
            Some(routine) => match self.prop_routine_value(st, idx, routine)? {
                Some(Value::Text(t) | Value::Code(t)) => Some(t),
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
    fn prop_bool_now(&mut self, st: &St, idx: usize, name: &str, default: bool) -> Result<bool, Signal> {
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
    fn page_caption(&self, st: &Rc<RefCell<PageState>>) -> String {
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
    fn in_cue_group(&self, st: &St, idx: usize) -> bool {
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

    fn control_enabled(&mut self, st: &St, idx: usize) -> Result<bool, Signal> {
        self.prop_bool_now(st, idx, "enabled", true)
    }

    fn control_visible(&mut self, st: &St, idx: usize) -> Result<bool, Signal> {
        let engine = self.engine();
        let page = page_of(&engine, st);
        // Own Visible is the open-time value; a group's is live and so are the groups around the
        // control (corpus handlers/TestPageControlPropertyLiveness_Tests.al:56).
        let own = if matches!(
            page.controls[idx].kind,
            ir::ControlKind::Group | ir::ControlKind::CueGroup
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
