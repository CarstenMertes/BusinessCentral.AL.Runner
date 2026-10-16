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
    Signal::Error(Box::new(AlError {
        message: message.into(),
        code: String::new(),
        call_stack: String::new(),
    }))
}

/// Whether `name` (a page's name or id as a table property writes it) is a page of a dependency app
/// that was not compiled.
fn names_uncompiled_page(engine: &Engine, name: &str) -> bool {
    engine.program.objects.iter().any(|o| {
        o.is_shell_of(al_ir::ObjKind::Page)
            && (o.name.eq_ignore_ascii_case(name) || o.id.is_some_and(|i| i.to_string() == name))
    })
}

fn error_code(message: impl Into<String>, code: &str) -> Signal {
    Signal::Error(Box::new(AlError {
        message: message.into(),
        code: code.into(),
        call_stack: String::new(),
    }))
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
mod actions;
mod controls;
mod draft;
mod format;
mod open;
mod pageops;
mod rows;
mod runobject;

impl Session {
    /// `TP.Ctl`: a handle on control `control` of the test page `page`.
    pub(crate) fn page_control(&mut self, page: Value, control: usize, runtime: bool) -> Result<Value, Signal> {
        match page {
            Value::TestPage(p) => {
                // A part control reads as the page it shows (a `TestPart`).
                let is_part = {
                    let engine = self.engine();
                    page_of(&engine, &p).controls[control].kind == ir::ControlKind::Part
                };
                if is_part {
                    return self.part_page(&p, control, runtime).map(Value::TestPage);
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

    /// `OnBeforeValidateEvent` / `OnAfterValidateEvent` of the control `element` of the page: raised around the control's own
    /// OnValidate after the table field's events and trigger have run (corpus
    /// record/lt-loadfields-pagesource-validate/P81PageValidateEvents.al: `T.Name.After > P.Name.Before > P.Name.OnValidate >
    /// P.Name.After`). The parameters are `Rec` and `xRec` (the corpus subscribers take `Rec` only).
    pub(crate) fn page_validate_event(&mut self, st: &St, event: &str, element: &str) -> Result<(), Signal> {
        let engine = self.engine();
        let object = st.borrow().object as usize;
        if !engine.events.is_subscribed(object, event) {
            return Ok(());
        }
        let rec = Value::Record(self.page_rec(st));
        let xrec = {
            let inst = st.borrow().instance.clone().expect("open page");
            let v = engine.program.objects[object]
                .xrec_slot
                .map(|x| inst.borrow().globals[x].clone());
            v.unwrap_or(Value::Null)
        };
        let names = vec!["Rec".to_string(), "xRec".to_string()];
        let mut args = vec![rec, xrec];
        let call = crate::dispatch::EventCall {
            object,
            event,
            element: Some(element),
            isolated: false,
            names: &names,
            sender: None,
        };
        self.raise(&call, &mut args)
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
                // The field events of a page are raised when the client writes the field, so they are refused there
                // (`control_set_value`), not for a page that is merely opened.
                && !matches!(ev.as_str(), "onaftervalidateevent" | "onbeforevalidateevent")
                // The client has no operation that deletes the page's row (an action without code of ours is not on the
                // runtime page, `action_without_behaviour`); a page that is merely
                // opened never raises this event. Base
                // Application subscribes to it on General Journal and the other journals (88 tests of
                // Tests-General Journal were refused at the open).
                && !(ev == "ondeleterecordevent"
                    && al_builtins::assumptions::hit(al_builtins::assumptions::Assumption::PageRowDeleteEventNotRaised))
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

    /// Calls routine `r` of `owner` with arguments.
    fn owner_routine_with(
        &mut self,
        st: &St,
        owner: Option<usize>,
        r: usize,
        args: Vec<Value>,
    ) -> Result<Value, Signal> {
        let object = owner.unwrap_or(st.borrow().object as usize);
        let inst = self.owner_instance(st, owner);
        let result = self.call(object, r, inst, args).map(|(v, _)| v);
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
