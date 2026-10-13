//! Dialogs raised by the code under test (`Message`, `Confirm`, `StrMenu`)
//! and the test handler functions that answer them.
//!
//! BC's test runner binds the handler functions named in a test's
//! `[HandlerFunctions]` for the duration of that test method. A UI call goes to
//! the bound handler of its kind (corpus handlers/TestMessageHandler.al); with
//! none bound it raises "Unhandled UI" (handlers/TestConfirmNoHandler.al); a
//! declared handler that never fired fails the test
//! (handlers/TestPageQueryCloseMessageHandler_Tests.al:51).

use crate::Session;
use al_builtins::native::{AlError, Signal};
use al_builtins::value::{Instance, RtType, Value};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiKind {
    Message,
    Confirm,
    StrMenu,
    /// `[PageHandler]`: answers `Page.Run` (ADR-012). Bound by the page of its TestPage parameter.
    Page,
    /// `[ModalPageHandler]`: answers `Page.RunModal`.
    ModalPage,
}

impl UiKind {
    /// The attribute that marks a handler function of this kind.
    pub fn from_attribute(name: &str) -> Option<UiKind> {
        match name.to_ascii_lowercase().as_str() {
            "messagehandler" => Some(UiKind::Message),
            "confirmhandler" => Some(UiKind::Confirm),
            "strmenuhandler" => Some(UiKind::StrMenu),
            "pagehandler" => Some(UiKind::Page),
            "modalpagehandler" => Some(UiKind::ModalPage),
            _ => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            UiKind::Message => "Message",
            UiKind::Confirm => "Confirm",
            UiKind::StrMenu => "StrMenu",
            UiKind::Page => "Page",
            UiKind::ModalPage => "ModalPage",
        }
    }
}

/// A handler function bound to the running test method.
pub struct UiHandler {
    kind: UiKind,
    /// For page handlers: the page object of the `TestPage` parameter.
    pub page: Option<u32>,
    name: String,
    object: usize,
    routine: usize,
    instance: Rc<RefCell<Instance>>,
    fired: bool,
}

/// The handlers bound while a test method runs.
#[derive(Default)]
pub(crate) struct UiScope {
    handlers: Vec<UiHandler>,
    /// A handler is running: a UI call from inside it is not pinned by the corpus.
    in_handler: bool,
}

impl Session {
    /// Checks that routine `routine` of `object` has the signature BC requires
    /// of a handler of `kind`, and binds it to `instance` (the test codeunit's).
    pub fn ui_handler(
        &self,
        kind: UiKind,
        object: usize,
        routine: usize,
        instance: Rc<RefCell<Instance>>,
    ) -> Result<UiHandler, String> {
        let r = &self.engine.program.objects[object].routines[routine];
        let ty = |i: usize| r.locals.get(i).map(|s| &s.ty);
        let ok = match kind {
            UiKind::Message => r.params == 1 && r.by_var == [false],
            UiKind::Confirm => r.params == 2 && r.by_var == [false, true] && matches!(ty(1), Some(RtType::Boolean)),
            UiKind::StrMenu => {
                r.params == 3 && r.by_var == [false, true, false] && matches!(ty(1), Some(RtType::Integer))
            }
            UiKind::Page | UiKind::ModalPage => {
                r.params == 1 && r.by_var == [true] && matches!(ty(0), Some(RtType::TestPage(_)))
            }
        };
        let page = match ty(0) {
            Some(RtType::TestPage(p)) if matches!(kind, UiKind::Page | UiKind::ModalPage) => Some(*p),
            _ => None,
        };
        if !ok {
            return Err(format!(
                "{} is not shaped like a {} handler function",
                r.name,
                kind.label()
            ));
        }
        Ok(UiHandler {
            kind,
            page,
            name: r.name.clone(),
            object,
            routine,
            instance,
            fired: false,
        })
    }

    /// Starts a test method: from here until `end_ui_scope`, UI calls go to
    /// `handlers`, and any other UI call is "Unhandled UI".
    pub fn begin_ui_scope(&mut self, handlers: Vec<UiHandler>) {
        self.ui = Some(UiScope {
            handlers,
            in_handler: false,
        });
    }

    /// Ends the test method; returns the names of the declared handlers that
    /// never fired.
    pub fn end_ui_scope(&mut self) -> Vec<String> {
        self.ui
            .take()
            .map(|s| s.handlers.into_iter().filter(|h| !h.fired).map(|h| h.name).collect())
            .unwrap_or_default()
    }

    /// Runs the handler of `kind` with `args`; returns the handler's final
    /// parameter values.
    fn dispatch_ui(&mut self, kind: UiKind, shown: &str, args: Vec<Value>) -> Result<Vec<Value>, Signal> {
        let Some(scope) = self.ui.as_mut() else {
            return Err(Signal::Unsupported(format!(
                "{} outside a test method (what the runner does then is not pinned)",
                kind.label()
            )));
        };
        if scope.in_handler {
            return Err(Signal::Unsupported(format!(
                "{} raised from inside a handler function (not pinned)",
                kind.label()
            )));
        }
        let Some(h) = scope.handlers.iter_mut().find(|h| h.kind == kind) else {
            // Texts: corpus handlers/TestConfirmNoHandler.al (Unhandled UI, the handler type
            // and the question), pageextensiontrigger/TestModifiedControlValidateTriggers_Tests.al:162
            // ("Unhandled UI: Message  <text>"). The error code is not pinned.
            return Err(Signal::Error(AlError {
                message: format!("Unhandled UI: {}  {shown}", kind.label()),
                code: String::new(),
                call_stack: String::new(),
            }));
        };
        h.fired = true;
        let (object, routine, instance) = (h.object, h.routine, h.instance.clone());
        scope.in_handler = true;
        let result = self.call(object, routine, instance, args);
        if let Some(scope) = self.ui.as_mut() {
            scope.in_handler = false;
        }
        result.map(|(_, finals)| finals)
    }

    /// The bound handler of `kind` for `page`, marked as used: `Unhandled UI` when there is none
    /// (corpus handlers/TestPageModalHandler_Tests.al:105, TestPageRunHandler_Tests.al:
    /// NonModalPageRunWithoutAHandlerIsRefused). BC looks the handler up before the page opens, so
    /// a refused page never ran its OnOpenPage (handlers/TestPageActionRunObjectNoHandler_Tests.al) and a
    /// handler whose page then fails to open still counts as executed
    /// (autoformat/TestAutoFormatExpressionTiming.al, FailingFormat_RunModal_RaisesBeforeAnyHandler).
    pub(crate) fn bind_page_handler(
        &mut self,
        kind: UiKind,
        page: u32,
    ) -> Result<(usize, usize, Rc<RefCell<Instance>>), Signal> {
        let Some(scope) = self.ui.as_mut() else {
            return Err(Signal::Unsupported(format!(
                "{} outside a test method (what the runner does then is not pinned)",
                kind.label()
            )));
        };
        let Some(h) = scope
            .handlers
            .iter_mut()
            .find(|h| h.kind == kind && h.page == Some(page))
        else {
            return Err(Signal::Error(AlError {
                message: format!("Unhandled UI: {}", kind.label()),
                code: String::new(),
                call_stack: String::new(),
            }));
        };
        h.fired = true;
        Ok((h.object, h.routine, h.instance.clone()))
    }

    pub(crate) fn ui_message(&mut self, text: String) -> Result<(), Signal> {
        self.dispatch_ui(UiKind::Message, &text.clone(), vec![Value::Text(text)])
            .map(|_| ())
    }

    pub(crate) fn ui_confirm(&mut self, question: String, default: bool) -> Result<bool, Signal> {
        let finals = self.dispatch_ui(
            UiKind::Confirm,
            &question.clone(),
            vec![Value::Text(question), Value::Boolean(default)],
        )?;
        match finals.get(1) {
            Some(Value::Boolean(b)) => Ok(*b),
            other => Err(Signal::Unsupported(format!("Confirm handler returned {other:?}"))),
        }
    }

    pub(crate) fn ui_str_menu(&mut self, options: String, default: i32, instruction: String) -> Result<i32, Signal> {
        let finals = self.dispatch_ui(
            UiKind::StrMenu,
            &options.clone(),
            vec![Value::Text(options), Value::Integer(default), Value::Text(instruction)],
        )?;
        match finals.get(1) {
            Some(Value::Integer(n)) => Ok(*n),
            other => Err(Signal::Unsupported(format!("StrMenu handler returned {other:?}"))),
        }
    }
}
