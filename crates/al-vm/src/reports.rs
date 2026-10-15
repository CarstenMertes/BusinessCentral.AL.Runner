//! Report objects: `Run`, `RunModal`, `Execute`, `SetTableView`, `UseRequestPage`, `CurrReport.Quit`,
//! and the processing a run does (no request page, no layout, no output).
//!
//! Every rule is pinned by a corpus test, cited where it is applied. What no test pins is refused
//! (`Signal::Unsupported`) and listed in docs/probes-needed.md. In particular a run whose request
//! page would open is refused: request pages arrive with the page work.

use crate::dataset::{self, Column, Dataset};
use crate::pages::RequestClosed;
use crate::transactions::{TxModel, al_error};
use crate::{Session, Signal};
use al_builtins::describe::Describe;
use al_builtins::ops;
use al_builtins::value::{Instance, Value};
use al_ir as ir;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

type Inst = Rc<RefCell<Instance>>;
type Rec = Rc<RefCell<al_builtins::value::RecordState>>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

/// What a report trigger asked for with `CurrReport.Skip`, `Break` or `Quit`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Flow {
    Skip,
    Break,
    Quit,
}

/// Which kind of report trigger is running: what `CurrReport.Skip`, `Break` and `Quit` may do there.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TrigKind {
    /// A data item's `OnAfterGetRecord`: Skip, Break and Quit are pinned
    /// (handlers/TestReportRuntimeContracts.al:322-368).
    Row,
    /// `OnPreReport`: Quit is pinned (:355-358).
    Pre,
    /// Every other trigger: what the calls do there is not pinned.
    Other,
}

/// The filter group a `DataItemLink` filter lives in. `GetFilters` reads group 0 and does not show
/// it (handlers/TestReportRuntimeContracts.al:62-90, :279-290: `Child.GetFilters()` is empty in the
/// child's triggers and after the parent). Group 4 is where a part's `SubPageLink` lives.
const LINK_GROUP: i32 = 4;

/// Which method started the run: they differ in whether the request page can open.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Entry {
    /// `Run` and `RunModal`: the request page opens unless the caller or the report switches it off.
    Run,
    /// `Execute` never shows the request page
    /// (record/TestTxModelReportExec.al:195-210, Test09).
    Execute,
}

/// The report instance's object, kept alive independently of the session.
pub(crate) struct Schema {
    engine: Arc<crate::Engine>,
    object: usize,
}

impl Schema {
    pub(crate) fn report(&self) -> &ir::Report {
        self.engine.program.objects[self.object]
            .report
            .as_ref()
            .expect("checked by Session::report_schema")
    }
    pub(crate) fn name(&self) -> &str {
        &self.engine.program.objects[self.object].name
    }
}

impl Session {
    pub(crate) fn report_builtin(
        &mut self,
        id: &str,
        receiver: Option<Value>,
        args: &[Value],
        running: Inst,
    ) -> Result<Value, Signal> {
        match id {
            "Report.Quit#1" => self.report_quit_builtin(&running),
            "Report.Skip#1" => self.report_flow_builtin(&running, Flow::Skip),
            "Report.Break#1" => self.report_flow_builtin(&running, Flow::Break),
            "Report.SetTableView#1" => {
                let inst = self.report_instance(receiver, false)?.0;
                let schema = self.report_schema(&inst)?;
                self.report_set_view(&inst, &schema, args.first())?;
                Ok(Value::Null)
            }
            // `CurrReport.Language := n` and `CurrReport.Language` read it back at once
            // (handlers/TestReportCurrReportLanguage.al:58-84). Language 0 is rejected by BC with an error
            // that names the language: `Invalid language ID supplied: 0.` / Argument, and the language stays as it
            // was (record/ProbeClearAllSetKeyHyperlink.al:341-353, BC 27.5); another language that is not
            // positive is not pinned. A language nobody set reads as the session's, 1033. The runtime's culture
            // is fixed (en-US): the language is kept and changes no formatting (dates and decimals read
            // the same after 1031, ibid.).
            "Report.Language#1" => match args.first() {
                Some(Value::Integer(n)) if *n > 0 => {
                    self.curr_report_language = Some(*n);
                    Ok(Value::Null)
                }
                Some(Value::Integer(0)) => Err(crate::transactions::al_error_code(
                    "Invalid language ID supplied: 0.",
                    "Argument",
                )),
                Some(Value::Integer(n)) => unsupported(format!(
                    "CurrReport.Language set to {n}: only 0 is pinned as invalid (docs/probes-needed.md)"
                )),
                None => Ok(Value::Integer(
                    self.curr_report_language.unwrap_or(self.global_language),
                )),
                Some(other) => unsupported(format!("CurrReport.Language with {}", other.describe())),
            },
            // `FormatRegion` only takes effect when a Word or Excel layout renders; a run that writes the
            // dataset (an Xml export) never does, and the property reads back empty
            // (handlers/TestReportCurrReportLanguage.al:12-18, :76-83).
            "Report.FormatRegion#1" => match args.first() {
                Some(Value::Text(_) | Value::Code(_)) => Ok(Value::Null),
                None => Ok(Value::Text(String::new().into())),
                Some(other) => unsupported(format!("CurrReport.FormatRegion with {}", other.describe())),
            },
            // `ObjectId(UseNames)`: `Report 69730`, or with names (the default) `Report <Caption>`, the object name
            // when the report has no Caption (handlers/PWRefusedBuiltins.al:78-107, BC 27.5; also through a Report
            // variable).
            "Report.ObjectId#1" => {
                // `CurrReport.ObjectId` has no receiver value: it is the running report
                // (handlers/PWRefusedBuiltins.al:ReportObjectId_CurrReportInCaptionedReport).
                let receiver = receiver.or_else(|| Some(Value::Codeunit(running.clone())));
                let inst = self.report_instance(receiver, false)?.0;
                let schema = self.report_schema(&inst)?;
                let o = &schema.engine.program.objects[schema.object];
                let names = match args.first() {
                    None => true,
                    Some(Value::Boolean(b)) => *b,
                    Some(other) => return unsupported(format!("Report.ObjectId with {}", other.describe())),
                };
                if !names {
                    return Ok(Value::Text(format!("Report {}", o.id.unwrap_or(0)).into()));
                }
                let caption = match o.properties.iter().find(|(k, _)| k.eq_ignore_ascii_case("Caption")) {
                    Some((_, v)) => match crate::recmeta::string_literal(v) {
                        Some(c) => c,
                        None => return unsupported("Report.ObjectId of a report whose Caption is not a literal"),
                    },
                    None => o.name.clone(),
                };
                Ok(Value::Text(format!("Report {caption}").into()))
            }
            // `CurrReport.Preview()` is false in a report run by `Report.Run` (with and without the request
            // page) and `RunModal` (handlers/TestReportRequestPageUpdateObjectIdContracts.al:
            // RequestPageUpdate_RefreshesTheOtherControls, ObjectIdAndPreview_WithoutTheRequestPage_AndWithRunModal).
            // Execute, SaveAs and the request page's preview action are not pinned.
            "Report.Preview#1" => match self.report_preview_pinned.last() {
                Some(true) => Ok(Value::Boolean(false)),
                _ => unsupported(
                    "Report.Preview outside a report started by Run or RunModal (whether an Execute, a SaveAs or a request page preview is a preview is not pinned)",
                ),
            },
            "Report.UseRequestPage#1" => {
                let inst = self.report_instance(receiver, false)?.0;
                let schema = self.report_schema(&inst)?;
                let slot = schema.report().request_flag_slot;
                // The getter answers what the next Run does; the setter overrides the report's own
                // property in both directions (handlers/TestReportRuntimeContracts.al:416-440).
                let Some(Value::Boolean(b)) = args.first() else {
                    return Ok(Value::Boolean(Self::report_shows_page(&inst, &schema)));
                };
                inst.borrow_mut().globals[slot] = Value::Integer(if *b { 1 } else { 2 });
                Ok(Value::Null)
            }
            "Report.Run#1" | "Report.RunModal#1" => {
                let (inst, quit) = self.report_instance_for(receiver, true, id == "Report.Run#1")?;
                let schema = self.report_schema(&inst)?;
                let window = Self::report_shows_page(&inst, &schema);
                let result = if quit {
                    Ok(())
                } else {
                    self.report_run(&inst, &schema, Entry::Run, window)
                };
                // "With the Run method, the variable is automatically cleared after the method is
                // run" (handlers/TestReportSetTableView.al:17-22); a global read afterwards belongs
                // to a new instance (handlers/TestReportRunWithRequestPage.al:32-36,
                // handlers/TestReportOnInitReportTimingFixtures.al:3-5).
                self.report_clear(&inst);
                result.map(|()| Value::Null)
            }
            // The static forms run a fresh instance of the report with that id.
            "Report.Run#3" | "Report.RunModal#3" => {
                let (inst, schema) = self.report_by_number(id, args.first())?;
                let window = match args.get(1) {
                    None => true,
                    Some(Value::Boolean(b)) => *b,
                    Some(other) => return unsupported(format!("{id} with RequestWindow {}", other.describe())),
                } && schema.report().use_request_page != Some(false);
                match args.get(2) {
                    None | Some(Value::Boolean(false)) => {}
                    Some(other) => {
                        return unsupported(format!(
                            "{id} with SystemPrinter {} (printing is not supported)",
                            other.describe()
                        ));
                    }
                }
                let quit = self.report_init(&inst)?;
                if let Some(rec) = args.get(3) {
                    self.report_set_view(&inst, &schema, Some(rec))?;
                }
                if quit {
                    return Ok(Value::Null);
                }
                self.report_run(&inst, &schema, Entry::Run, window)
                    .map(|()| Value::Null)
            }
            "Report.Execute#1" => {
                let (inst, quit) = self.report_instance(receiver, true)?;
                let schema = self.report_schema(&inst)?;
                self.report_execute_args(id, args.first(), args.get(1))?;
                // handlers/TestReportRuntimeContracts.al:395-414: Execute keeps the globals, and a
                // second Execute on the same variable is refused.
                if self
                    .report_executed
                    .iter()
                    .any(|w| w.upgrade().is_some_and(|i| Rc::ptr_eq(&i, &inst)))
                {
                    return Err(al_error("The object has been run. Use the function CLEAR(Report)."));
                }
                self.report_executed.retain(|w| w.strong_count() > 0);
                self.report_executed.push(Rc::downgrade(&inst));
                if quit {
                    return Ok(Value::Null);
                }
                self.report_run(&inst, &schema, Entry::Execute, false)
                    .map(|()| Value::Null)
            }
            "Report.Execute#3" => {
                let (inst, schema) = self.report_by_number(id, args.first())?;
                self.report_execute_args(id, args.get(1), args.get(2))?;
                let quit = self.report_init(&inst)?;
                if quit {
                    return Ok(Value::Null);
                }
                self.report_run(&inst, &schema, Entry::Execute, false)
                    .map(|()| Value::Null)
            }
            "Report.Print#1" | "Report.Print#2" => {
                let schema = if id == "Report.Print#1" {
                    let inst = self.report_instance(receiver, false)?.0;
                    self.report_schema(&inst)?
                } else {
                    self.report_by_number(id, args.first())?.1
                };
                // Print never shows the request page, so only the TransactionType term applies
                // (record/TestTxModelReportExec.al:227-239, Test11).
                self.report_enter_world(&schema, false)?;
                unsupported(format!(
                    "Report.Print of \"{}\" (what printing a processing-only report does is not pinned)",
                    schema.name()
                ))
            }
            "Report.SaveAs#1" | "Report.SaveAs#2" => self.report_save_as(id, receiver, args),
            "Report.WordXmlPart#1" | "Report.WordXmlPart#2" => self.report_word_xml_part(id, receiver, args),
            "Report.RunRequestPage#1" | "Report.RunRequestPage#2" => {
                let (inst, schema, seed) = if id == "Report.RunRequestPage#1" {
                    let inst = self.report_instance(receiver, false)?.0;
                    let schema = self.report_schema(&inst)?;
                    (inst, schema, args.first())
                } else {
                    let (inst, schema) = self.report_by_number(id, args.first())?;
                    (inst, schema, args.get(1))
                };
                // Only the empty string and the text this runtime produced are understood; how BC
                // loads a parameters string into the page is pinned only by its result
                // (handlers/TestReportInstanceRunReqPage.al:SeededWithParameters_StillRunsTheHandler).
                let seed = match seed {
                    None => None,
                    Some(Value::Text(s)) if s.is_empty() => None,
                    Some(Value::Text(s)) => Some(s.clone()),
                    Some(other) => return unsupported(format!("{id} seeded with {}", other.describe())),
                };
                // RunRequestPage always shows the page, so it always enters a transaction world
                // (record/TestTxModelReportExec.al:77-102, Test02 and Test03).
                let enters = self.report_enter_world(&schema, true)?;
                self.report_in_world(enters, |s| s.report_request_only(&inst, &schema, seed.as_deref()))
                    .map(|s| Value::Text(s.into()))
            }
            other => unsupported(format!("builtin {other}")),
        }
    }

    /// The report instance behind a receiver, initialised: the first use of a report variable
    /// runs `OnInitReport` (handlers/TestReportOnInitReportTiming.al:50-60).
    /// Returns whether that trigger quit the report; a caller that cannot stop refuses it.
    fn report_instance(&mut self, receiver: Option<Value>, can_stop: bool) -> Result<(Inst, bool), Signal> {
        self.report_instance_for(receiver, can_stop, false)
    }

    fn report_instance_for(
        &mut self,
        receiver: Option<Value>,
        can_stop: bool,
        run: bool,
    ) -> Result<(Inst, bool), Signal> {
        match receiver {
            Some(Value::Codeunit(rc)) => {
                // Used again after a Run: anything but a Run makes it a new instance (OnInitReport runs).
                if std::mem::take(&mut rc.borrow_mut().rerun) && !run {
                    rc.borrow_mut().initialized = false;
                }
                let quit = !rc.borrow().initialized && self.report_init(&rc)?;
                if quit && !can_stop {
                    return unsupported(
                        "CurrReport.Quit in OnInitReport before a method that does not run the report (not pinned)",
                    );
                }
                Ok((rc, quit))
            }
            other => unsupported(format!("a Report method on {}", other.describe())),
        }
    }

    /// The instance of reportextension `ext` that belongs to report variable `base`: made at first use and kept
    /// with the variable, so the extension's globals and added data items keep their state across calls, `SaveAs`
    /// and `Execute`; a `Run` and a `Clear` drop it (reportext/W3RpExtProcedures.al:ExtensionProcedure_*).
    /// The first use of the variable runs `OnInitReport`, as a call of any of its procedures does.
    pub(crate) fn report_extension_instance(&mut self, base: &Inst, ext: usize) -> Result<Inst, Signal> {
        let (base, _) = self.report_instance(Some(Value::Codeunit(base.clone())), false)?;
        if let Some((_, i)) = base.borrow().exts.iter().find(|(e, _)| *e as usize == ext) {
            return Ok(i.clone());
        }
        let i = self.extension_instance(ext, &base);
        base.borrow_mut().exts.push((ext as u32, i.clone()));
        Ok(i)
    }

    /// The schema of `inst`'s object; refuses a report the runtime cannot run.
    fn report_schema(&self, inst: &Inst) -> Result<Schema, Signal> {
        let object = inst.borrow().object as usize;
        let schema = Schema {
            engine: self.engine.clone(),
            object,
        };
        match &schema.engine.program.objects[object].report {
            Some(r) => match r.unsupported.first() {
                None => Ok(schema),
                Some(why) => unsupported(format!("Report \"{}\": {why}", schema.name())),
            },
            None => unsupported(format!("object {object} is not a report")),
        }
    }

    /// A fresh instance of the report with the id in `arg`.
    /// `Report.WordXmlPart([Number,] [ExtendedFormat])`: the XML of the data set as text, from the report's
    /// data items and columns alone (nothing runs). Pinned by the corpus staging branch (types/PZMisc.al:
    /// WordXmlPart_BothOverloads_ReportWithoutWordLayout and WordXmlPart_Content_IncludesTheColumns, BC 29.0):
    /// a report with only an RDLC layout answers a text that starts with `<` and holds the data item and
    /// column names and `urn:microsoft-dynamics-nav`, with either flag; an unknown report id fails with the code
    /// AL:MetadataNotFound (its text is not pinned). The rest follows what the owner's Pageworks code reads
    /// (a `NavWordReportXmlPart` root, a `Labels` block, nested data items, `ElementType` DataItem / Column
    /// with the flag): unprobed, docs/probes-needed.md "Report.WordXmlPart".
    fn report_word_xml_part(&mut self, id: &str, receiver: Option<Value>, args: &[Value]) -> Result<Value, Signal> {
        let static_form = id == "Report.WordXmlPart#2";
        let (flag_at, schema) = if static_form {
            let known =
                match args.first() {
                    Some(Value::Integer(n)) => self.engine.program.objects.iter().any(|o| {
                        (o.kind == "report" || o.kind == "uncompiled report") && o.id == u32::try_from(*n).ok()
                    }),
                    _ => true,
                };
            if !known {
                return Err(crate::transactions::al_error_code("", "AL:MetadataNotFound"));
            }
            (1, self.report_by_number(id, args.first())?.1)
        } else {
            let inst = self.report_instance(receiver, false)?.0;
            (0, self.report_schema(&inst)?)
        };
        let extended = match args.get(flag_at) {
            None => false,
            Some(Value::Boolean(b)) => *b,
            Some(other) => return unsupported(format!("{id} with the flag {}", other.describe())),
        };
        let report = schema.report();
        if !report.extensions.is_empty() {
            return unsupported("Report.WordXmlPart of a report that reportextensions change (not pinned)");
        }
        let tag = |name: &str| -> Result<String, Signal> {
            if name
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '_' | '-'))
                && !name.is_empty()
            {
                Ok(name.replace(' ', "_"))
            } else {
                unsupported(format!(
                    "Report.WordXmlPart of the name `{name}`: how BC legalizes it as an XML tag is not pinned"
                ))
            }
        };
        fn items(
            report: &ir::Report,
            parent: Option<usize>,
            extended: bool,
            tag: &dyn Fn(&str) -> Result<String, Signal>,
            out: &mut String,
        ) -> Result<(), Signal> {
            for (i, item) in report.items.iter().enumerate().filter(|(_, it)| it.parent == parent) {
                let name = tag(&item.name)?;
                let kind = if extended { " ElementType=\"DataItem\"" } else { "" };
                out.push_str(&format!("<{name}{kind}>"));
                for c in &item.columns {
                    let cn = tag(&c.name)?;
                    let kind = if extended { " ElementType=\"Column\"" } else { "" };
                    out.push_str(&format!("<{cn}{kind} />"));
                }
                items(report, Some(i), extended, tag, out)?;
                out.push_str(&format!("</{name}>"));
            }
            Ok(())
        }
        let mut body = String::new();
        items(report, None, extended, &tag, &mut body)?;
        let number = self.object_id_of(schema.object as u32);
        Ok(Value::Text(format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\" standalone=\"yes\"?><NavWordReportXmlPart xmlns=\"urn:microsoft-dynamics-nav/reports/{}/{number}/\"><Labels />{body}</NavWordReportXmlPart>",
            tag(schema.name())?
        ).into()))
    }

    fn report_by_number(&mut self, id: &str, arg: Option<&Value>) -> Result<(Inst, Schema), Signal> {
        let Some(Value::Integer(n)) = arg else {
            return unsupported(format!("{id} with an id that is not an integer"));
        };
        let Some(object) = self
            .engine
            .program
            .objects
            .iter()
            .position(|o| o.kind == "report" && o.id == u32::try_from(*n).ok())
        else {
            // A report of a dependency app that nothing reached is not compiled (ADR-013); BC has it.
            if self
                .engine
                .program
                .objects
                .iter()
                .any(|o| o.kind == "uncompiled report" && o.id == u32::try_from(*n).ok())
            {
                return unsupported(format!(
                    "{id} of report {n}, which belongs to a dependency app and was not compiled (no code the workspace reaches uses it)"
                ));
            }
            return unsupported(format!(
                "{id} of id {n}, which no report declares (error text not pinned)"
            ));
        };
        let inst = self.instance(object);
        // `instance` hands out a made instance: OnInitReport has not run on it yet.
        inst.borrow_mut().initialized = false;
        let schema = self.report_schema(&inst)?;
        Ok((inst, schema))
    }

    /// `Execute(Parameters, ?RecordRef)`: only the empty parameter string is pinned
    /// (record/TestTxModelReportExec.al:138-211).
    fn report_execute_args(
        &self,
        id: &str,
        parameters: Option<&Value>,
        record_ref: Option<&Value>,
    ) -> Result<(), Signal> {
        match parameters {
            Some(Value::Text(s)) if s.is_empty() => {}
            other => {
                return unsupported(format!(
                    "{id} with the parameters {} (only '' is pinned)",
                    other.describe()
                ));
            }
        }
        if record_ref.is_some() {
            return unsupported(format!("{id} with a RecordRef (not pinned)"));
        }
        Ok(())
    }

    // ------------------------------------------------------------------ OnInitReport

    /// First use of an instance: its globals are made and, for a report, `OnInitReport` runs once.
    pub(crate) fn first_use(&mut self, inst: &Inst) -> Result<(), Signal> {
        if inst.borrow().initialized {
            return Ok(());
        }
        if self.report_init(inst)? {
            return unsupported("CurrReport.Quit in OnInitReport while a procedure first uses the report (not pinned)");
        }
        Ok(())
    }

    /// Makes the globals of `inst` and, for a report that had none, runs `OnInitReport`. Returns
    /// whether the trigger quit the report.
    fn report_init(&mut self, inst: &Inst) -> Result<bool, Signal> {
        let first = !inst.borrow().initialized;
        self.ensure_initialized(inst);
        let object = inst.borrow().object as usize;
        let engine = self.engine.clone();
        let o = &engine.program.objects[object];
        if !first || o.kind != "report" {
            return Ok(false);
        }
        let Some(on_init) = o.report.as_ref().and_then(|r| r.on_init) else {
            return Ok(false);
        };
        // CurrReport.Quit in OnInitReport stops OnPreReport and the body
        // (handlers/TestReportOnInitReportTiming.al:88-98, QuitInOnInitReport_StopsPreReportAndBody).
        let saved = self.report_init_depth.replace(self.depth + 1);
        self.report_quit = false;
        let r = self.call(object, on_init, inst.clone(), Vec::new());
        self.report_init_depth = saved;
        let quit = std::mem::take(&mut self.report_quit);
        r?;
        Ok(quit)
    }

    /// `CurrReport.Quit`: from `OnInitReport` (handlers/TestReportOnInitReportTiming.al:88-98),
    /// `OnPreReport` and `OnAfterGetRecord` (handlers/TestReportRuntimeContracts.al:355-368); it is
    /// not pinned anywhere else.
    fn report_quit_builtin(&mut self, running: &Inst) -> Result<Value, Signal> {
        let object = running.borrow().object as usize;
        if self.engine.program.objects[object].kind == "report" && self.report_init_depth == Some(self.depth) {
            self.report_quit = true;
            // Leaves the trigger; `report_init` sees the flag.
            return Err(Signal::Exit(None));
        }
        self.report_flow_builtin(running, Flow::Quit)
    }

    /// `CurrReport.Skip`, `Break` and `Quit` called straight from a report trigger: the trigger
    /// ends and the run reads the request (`report_trigger_call`).
    fn report_flow_builtin(&mut self, running: &Inst, flow: Flow) -> Result<Value, Signal> {
        let object = running.borrow().object as usize;
        let on_report = matches!(
            self.engine.program.objects[object].kind.as_str(),
            "report" | "reportextension"
        );
        let allowed = match self.report_trigger {
            Some((depth, kind)) if on_report && depth == self.depth => match flow {
                Flow::Skip | Flow::Break => kind == TrigKind::Row,
                Flow::Quit => kind != TrigKind::Other,
            },
            _ => false,
        };
        if !allowed {
            return unsupported(format!(
                "CurrReport.{flow:?} outside the triggers where it is pinned (what it does there is not pinned)"
            ));
        }
        self.report_flow = Some(flow);
        Err(Signal::Exit(None))
    }

    /// Runs a report trigger and returns what it asked for with Skip, Break or Quit.
    fn report_trigger_call(
        &mut self,
        object: usize,
        routine: usize,
        inst: Inst,
        kind: TrigKind,
    ) -> Result<Option<Flow>, Signal> {
        let saved = self.report_trigger.replace((self.depth + 1, kind));
        self.report_flow = None;
        let r = self.call(object, routine, inst, Vec::new());
        self.report_trigger = saved;
        let flow = self.report_flow.take();
        r?;
        Ok(flow)
    }

    /// Whether a `Run` of `inst` opens the request page: the variable's own `UseRequestPage(..)`
    /// first, else the report's property, which is true when it is not declared
    /// (handlers/TestReportRuntimeContracts.al:416-440).
    fn report_shows_page(inst: &Inst, schema: &Schema) -> bool {
        match inst.borrow().globals[schema.report().request_flag_slot] {
            Value::Integer(1) => true,
            Value::Integer(2) => false,
            _ => schema.report().use_request_page != Some(false),
        }
    }

    /// The variable is reset after `Run`: its globals go back to their defaults, and
    /// `OnInitReport` does not run again (handlers/TestReportRuntimeContracts.al:395-414; a global
    /// read afterwards is a default: handlers/TestReportSetTableView.al:17-22,
    /// handlers/TestReportRunWithRequestPage.al:32-36).
    fn report_clear(&mut self, inst: &Inst) {
        let object = inst.borrow().object as usize;
        let fresh = self.instance(object);
        let globals = std::mem::take(&mut fresh.borrow_mut().globals);
        inst.borrow_mut().globals = globals;
        inst.borrow_mut().rerun = true;
        inst.borrow_mut().exts.clear();
    }

    // ------------------------------------------------------------------ table views

    fn base_item_record(&self, inst: &Inst, item: &ir::ReportItem) -> Result<Rec, Signal> {
        match item.slot.map(|s| inst.borrow().globals[s].clone()) {
            Some(Value::Record(rc)) => Ok(rc),
            other => unsupported(format!(
                "dataitem {} holds {other}",
                item.name,
                other = other.describe()
            )),
        }
    }

    /// `SetTableView` and the record parameter of the static `Run`: the filters of the given record
    /// go to the one data item over its table
    /// (handlers/TestReportSetTableView.al:72-86, handlers/TestReportGetFilterInTriggers.al:58-89).
    fn report_set_view(&mut self, inst: &Inst, schema: &Schema, view: Option<&Value>) -> Result<(), Signal> {
        // A RecordRef handed to `SaveAs` filters the data item over its table the way a view does
        // (handlers/TestReportSaveAsRecordRefFilter.al:TestReport_SaveAs_FilteredRecordRef_LimitsDatasetToThatRecord).
        let view = match view {
            Some(Value::Record(view)) => view.clone(),
            Some(Value::Library(_, h)) => match &*h.borrow() {
                al_builtins::value::LibraryValue::RecordRef(rec) => rec.clone(),
                _ => return unsupported(format!("a table view of {}", h.borrow().describe())),
            },
            other => return unsupported(format!("a table view of {}", other.describe())),
        };
        let table = view.borrow().table;
        let items: Vec<&ir::ReportItem> = schema
            .report()
            .items
            .iter()
            .filter(|i| i.table == Some(table as usize))
            .collect();
        let [item] = items[..] else {
            return unsupported(format!(
                "a table view for report \"{}\", which has {} data items over that table (not pinned)",
                schema.name(),
                items.len()
            ));
        };
        let v = view.borrow();
        if v.temporary || v.marked_only || v.key != 0 || !v.ascending {
            return unsupported("a table view with a temporary, marked or re-sorted record (not pinned)");
        }
        // A view handed to SetTableView stays in force over a DataItemTableView that only sorts
        // (handlers/TestReportRuntimeContracts.al:393-410 "Probe Rep Order"). Over one that filters, both apply
        // (the item's filters live in the report's own filter group): assumed, Whse.-Source - Create Document and its
        // callers need it (registry entry data-item-view-and-table-view).
        if item
            .view
            .as_ref()
            .is_some_and(|v| v.to_ascii_lowercase().contains("where"))
        {
            al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::DataItemViewAndTableView);
        }
        let target = self.base_item_record(inst, item)?;
        let mut t = target.borrow_mut();
        t.filters = v.filters.clone();
        t.filter_group = v.filter_group;
        Ok(())
    }

    // ------------------------------------------------------------------ transactions

    /// The platform's decision, made before the request page is shown: a run enters a transaction
    /// world when it shows the request page or its TransactionType differs from the session's
    /// (`UpdateNoLocks` excepted), and a test under `TransactionModel::AutoRollback` is exempt
    /// (record/TestTxModelReportRun.al:1-35, record/TestTxModelReportExec.al:1-30). Entering one
    /// is refused while the caller holds an uncommitted write (:117-148 Test04/05/08;
    /// TestTxModelReportExec.al:77-102, 138-150, 214-239). Returns whether a world is entered.
    fn report_enter_world(&mut self, schema: &Schema, shows_page: bool) -> Result<bool, Signal> {
        let differs = match schema.report().transaction_type.as_deref() {
            None => false,
            Some("update") => true,
            // Snapshot runs when no write is pending (handlers/TestReportRuntimeContracts.al:
            // 468-482); with a pending write what BC does is not pinned.
            Some("snapshot") => {
                if self.in_write_transaction() {
                    return unsupported(format!(
                        "TransactionType = snapshot on report \"{}\" with a pending write (not pinned)",
                        schema.name()
                    ));
                }
                true
            }
            Some(other) => {
                return unsupported(format!(
                    "TransactionType = {other} on report \"{}\" (only Update and Snapshot are pinned)",
                    schema.name()
                ));
            }
        };
        if !(shows_page || differs) {
            return Ok(false);
        }
        if self.tx_model == TxModel::AutoRollback {
            // A report declaring a TransactionType needs a type change, which BC refuses once a
            // transaction has started; AutoRollback exempts the world, not that rule
            // (record/TestTxModelReportExec.al:243-257, Test12).
            if differs && self.in_write_transaction() {
                return unsupported(
                    "a TransactionType = Update report under AutoRollback with a pending write (not pinned)",
                );
            }
            return Ok(false);
        }
        if self.in_write_transaction() {
            return Err(crate::transactions::transaction_stopped());
        }
        if self.try_depth > 0 {
            return unsupported("a report run that enters a transaction world inside a TryFunction (not pinned)");
        }
        Ok(true)
    }

    // ------------------------------------------------------------------ the run

    fn report_run(&mut self, inst: &Inst, schema: &Schema, entry: Entry, window: bool) -> Result<(), Signal> {
        // `window` says whether the request page opens: the instance's UseRequestPage, which falls
        // back on the report's property (record/TestTxModelReportRun.al:11, :60-61: a report with a
        // request page "defaults to true"), or a static call's RequestWindow. A report without a
        // requestpage section opens its page as well (handlers/TestReportRuntimeContracts.al:416-440).
        let shows_page = entry == Entry::Run && window;
        let enters = self.report_enter_world(schema, shows_page)?;
        self.report_preview_pinned.push(entry == Entry::Run);
        let ran = self.report_in_world(enters, |s| s.report_body(inst, schema, shows_page, None));
        self.report_preview_pinned.pop();
        ran.map(|_| ())
    }

    /// Runs `body` as the platform runs a report: a run that enters no world still begins a
    /// transaction of its own under TransactionModel::None, which ends with the run
    /// (codeunit/TestWriteTxTestBoundary.al:259-279, Test11).
    fn report_in_world<T>(
        &mut self,
        enters: bool,
        body: impl FnOnce(&mut Self) -> Result<T, Signal>,
    ) -> Result<T, Signal> {
        let began_world = self.no_transaction();
        self.tx_world += 1;
        let result = body(self);
        self.tx_world -= 1;
        match result {
            Ok(v) => {
                if (enters || began_world) && self.tx_model != TxModel::AutoRollback {
                    // A world that is entered commits its own writes when the run returns
                    // (record/TestTxModelReportRun.al:177-196, Test07). That `RunRequestPage` does
                    // the same is docs/probes-needed.md.
                    self.commit_transaction();
                }
                Ok(v)
            }
            Err(e) => {
                // A run that raises inside its world does not commit what it wrote
                // (record/TestTxModelReportRun.al:252-269, Test12).
                if enters && matches!(e, Signal::Error(_)) {
                    self.rollback_transaction();
                }
                Err(e)
            }
        }
    }

    /// `RunRequestPage`: the request page and nothing else. A cancelled page returns no parameters
    /// (handlers/TestReportRunRequestPage.al:TestReport_RunRequestPage_CancelledHandlerReturnsNoParameters);
    /// confirming it is not a run, so a `SaveValues` page does not remember what was confirmed
    /// (handlers/TestReportRequestPageSaveValues.al:SourceReport_SaveValues_RunRequestPageReturnsTheValueWithoutSavingIt).
    fn report_request_only(&mut self, inst: &Inst, schema: &Schema, seed: Option<&str>) -> Result<String, Signal> {
        let run = self.report_prepare(inst, schema)?;
        let before = self.saved_values.get(&(schema.object as u32)).cloned();
        let ext: Vec<(usize, Inst)> = run.exts.iter().map(|(i, e)| (*e, i.clone())).collect();
        let closed = self.run_request_page(inst, schema.object, seed, ext, true);
        match before {
            Some(v) => {
                self.saved_values.insert(schema.object as u32, v);
            }
            None => {
                self.saved_values.remove(&(schema.object as u32));
            }
        }
        match closed? {
            (RequestClosed::Cancel, _) => Ok(String::new()),
            (RequestClosed::Ok, parameters) => Ok(parameters.expect("parameters were asked for")),
            (RequestClosed::SavedAsXml, _) => {
                unsupported("SaveAsXml in the request page of a RunRequestPage (not pinned)")
            }
        }
    }

    /// The instances a run executes (the report, and the reportextension that changes it) with
    /// every data item's own view in force. The views are in force before the request page opens
    /// (handlers/TestReportMultiLineDataItemView.al: "the request page applies those views to the
    /// data items before the handler runs") and before OnPreReport, which may refine them
    /// (handlers/TestReportGetFilterInTriggers.al:43-52 reads the view there).
    /// The `Schema` of the report `inst` is an instance of.
    pub(crate) fn report_schema_of(&self, inst: &Inst) -> Result<Schema, Signal> {
        self.report_schema(inst)
    }

    /// A `Run` over instances that already exist and whose items already have their views (the request
    /// page is open): `SaveAsXml` finishes the report from there.
    pub(crate) fn report_run_for(&self, object: usize, base: &Inst, exts: Vec<(Inst, usize)>, dataset: bool) -> Run {
        Run {
            engine: self.engine.clone(),
            object,
            base: base.clone(),
            exts,
            dataset,
        }
    }

    fn report_prepare(&mut self, inst: &Inst, schema: &Schema) -> Result<Run, Signal> {
        let report = schema.report();
        // A reportextension that changes the run gets an instance of its own for this run, whose
        // names for the base items point at the report's own records.
        let mut exts = Vec::new();
        for &e in &report.extensions {
            let ext_inst = self.report_extension_instance(inst, e)?;
            let eir = self.engine.program.objects[e]
                .report_ext
                .as_ref()
                .expect("a report names only extensions that were lowered");
            for &(base_item, slot) in &eir.aliases {
                let base_slot = report.items[base_item].slot.expect("an alias has a record");
                let record = inst.borrow().globals[base_slot].clone();
                ext_inst.borrow_mut().globals[slot] = record;
            }
            exts.push((ext_inst, e));
        }
        let run = Run {
            engine: self.engine.clone(),
            object: schema.object,
            base: inst.clone(),
            exts,
            dataset: false,
        };
        for r in run.all_items() {
            let Some(view) = run.item(r).view.as_deref().map(crate::views::property_view) else {
                continue;
            };
            let rec = self.report_item_record(&run, r)?;
            let t = self.table(&rec)?;
            // A view that only sorts leaves the filters a SetTableView gave the item in force
            // (handlers/TestReportRuntimeContracts.al:393-410).
            let kept = Some(rec.borrow().filters.clone());
            let filtering = view.to_ascii_lowercase().contains("where");
            // The filters of a DataItemTableView are the report's, not the record's: GetFilters does not show them, in
            // OnPostReport either (report/TestProbeReportContracts.al:TriggerOrder_NestedAndSecondItem).
            self.set_view_in(&rec, &t, &view, Some(LINK_GROUP), false, false)?;
            if let Some(mut filters) = kept {
                // A view that filters adds its own filters to the ones a SetTableView gave the item.
                if filtering {
                    filters.extend(rec.borrow().filters.iter().filter(|f| f.group == LINK_GROUP).cloned());
                }
                rec.borrow_mut().filters = filters;
            }
        }
        Ok(run)
    }

    /// `OnPreReport`, every top-level data item, `OnPostReport`; a reportextension's triggers
    /// follow the base report's own. The order of the report triggers among themselves is pinned
    /// by reportextension/TestReportExtensionTriggers.al:244-250 (base pre, extension pre, base
    /// post, extension post); the rest is the order the BC documentation gives
    /// (docs/probes-needed.md).
    ///
    /// `save_as` is the parameters string of a `Report.SaveAs(.., Xml, ..)` run: the run writes the
    /// dataset into `self.dataset` and renders nothing. Returns false when the report was quit.
    fn report_body(
        &mut self,
        inst: &Inst,
        schema: &Schema,
        shows_page: bool,
        save_as: Option<&str>,
    ) -> Result<bool, Signal> {
        let mut run = self.report_prepare(inst, schema)?;
        run.dataset = save_as.is_some();
        if let Some(xml) = save_as.filter(|x| !x.is_empty()) {
            self.apply_run_parameters(inst, schema, &run, xml)?;
        }
        // The request page opens once the items' views are in force; what the handler filters is
        // what the run reads, so the views are not applied again afterwards. A cancelled page ends
        // the run without an error (handlers/TestReportRunWithRequestPage.al:
        // Report_Run_CancelledRequestPageRunsNothingAndRaisesNoError).
        if shows_page {
            let ext: Vec<(usize, Inst)> = run.exts.iter().map(|(i, e)| (*e, i.clone())).collect();
            match self.run_request_page(inst, schema.object, None, ext, false)?.0 {
                RequestClosed::Cancel => return Ok(false),
                // The handler's `SaveAsXml` ran the report there and then
                // (al-language-onprem/handlers/TestRequestPageSaveAsXmlContracts.al:
                // SaveAsXml_RunsTheReportOnce_AndClosesTheRequestPage): it does not run a second time.
                RequestClosed::SavedAsXml => return Ok(true),
                RequestClosed::Ok => {}
            }
        }
        self.report_after_page(&run, schema)
    }

    /// Everything of a run after its request page: the triggers and the data items.
    pub(crate) fn report_after_page(&mut self, run: &Run, schema: &Schema) -> Result<bool, Signal> {
        let report = schema.report();
        // A report that renders output runs up to its request page and no further: there is no
        // renderer (docs/probes-needed.md). The dataset a `SaveAs` writes is not rendered output.
        if let Some(why) = report.output.first().filter(|_| !run.dataset) {
            return unsupported(format!(
                "report \"{}\" renders output, which the runtime does not produce: {why}",
                schema.name()
            ));
        }
        // How a filter on a field a DataItemLink joins combines with the link is not pinned.
        for r in run.all_items() {
            let item = run.item(r);
            if item.link.is_empty() {
                continue;
            }
            let rec = self.report_item_record(run, r)?;
            if rec
                .borrow()
                .filters
                .iter()
                .any(|f| item.link.iter().any(|&(own, _)| f.field == own))
            {
                return unsupported(format!(
                    "data item {} filters a field its DataItemLink joins (how they combine is not pinned)",
                    item.name
                ));
            }
        }
        // A filter an item gets in OnPreReport is discarded: OnPreDataItem sees the view the item
        // had before it (handlers/TestReportRuntimeContracts.al:371-390).
        let mut kept = Vec::new();
        for r in run.all_items() {
            let rec = self.report_item_record(run, r)?;
            let filters = rec.borrow().filters.clone();
            kept.push((rec, filters));
        }
        if let Some(r) = report.on_pre {
            if self.report_trigger_call(run.object, r, run.base.clone(), TrigKind::Pre)? == Some(Flow::Quit) {
                return Ok(false);
            }
        }
        // The base first, then the extensions in object id order (corpus reportextensiontrigger/
        // TestReportExtensionBaseMembersContracts.al:256, ADR-025).
        for k in 0..run.exts.len() {
            let (ext_inst, e) = &run.exts[k];
            if let Some(r) = run.ext_ir(k).and_then(|x| x.on_pre) {
                if self.report_trigger_call(*e, r, ext_inst.clone(), TrigKind::Pre)? == Some(Flow::Quit) {
                    return Ok(false);
                }
            }
        }
        for (rec, filters) in kept {
            rec.borrow_mut().filters = filters;
        }
        for i in 0..report.items.len() {
            if report.items[i].parent.is_none() && self.report_item(run, ItemRef::Base(i), None, &[])? {
                return Ok(false);
            }
        }
        if let Some(r) = report.on_post {
            self.call(run.object, r, run.base.clone(), Vec::new())?;
        }
        for k in 0..run.exts.len() {
            let (ext_inst, e) = &run.exts[k];
            if let Some(r) = run.ext_ir(k).and_then(|x| x.on_post) {
                self.call(*e, r, ext_inst.clone(), Vec::new())?;
            }
        }
        Ok(true)
    }

    fn report_item_record(&self, run: &Run, r: ItemRef) -> Result<Rec, Signal> {
        let item = run.item(r);
        let (owner, _) = run.owner(r);
        match item.slot.map(|s| owner.borrow().globals[s].clone()) {
            Some(Value::Record(rc)) => Ok(rc),
            other => unsupported(format!(
                "dataitem {} holds {other}",
                item.name,
                other = other.describe()
            )),
        }
    }

    /// One data item: `OnPreDataItem`, `OnAfterGetRecord` per row with the nested items run for
    /// each row, `OnPostDataItem`. A nested item runs again for every row of its parent, over the
    /// rows its `DataItemLink` joins to that row
    /// (reportextension/TestReportExtensionTriggers.al:253-259 for the repetition,
    /// handlers/TestReportNestedLinkImplicitParent.al:113-134 for the link). A reportextension's
    /// `modify` triggers bracket the base item's `OnAfterGetRecord`, row by row (:262-268), and
    /// the items it adds run after the item's own nested items.
    ///
    /// Returns whether the report was quit. `CurrReport.Skip` in `OnAfterGetRecord` leaves out the
    /// nested items of that row, `Break` ends this item (its `OnPostDataItem` and the later items
    /// still run), `Quit` ends the whole report without `OnPostDataItem` and `OnPostReport`
    /// (handlers/TestReportRuntimeContracts.al:322-368).
    fn report_item(
        &mut self,
        run: &Run,
        r: ItemRef,
        parent_rec: Option<Rec>,
        enclosing: &[(ItemRef, Rec)],
    ) -> Result<bool, Signal> {
        let item = run.item(r);
        let (owner, object) = run.owner(r);
        let rec = self.report_item_record(run, r)?;
        // A `DataItemLinkReference` names another enclosing item than the parent.
        let parent_rec = match item.link_item {
            Some(li) => enclosing
                .iter()
                .find(|(e, _)| matches!(e, ItemRef::Base(x) if *x == li))
                .map(|(_, rec)| rec.clone())
                .or(parent_rec),
            None => parent_rec,
        };
        if let Some(parent) = parent_rec {
            // The link filter is hidden from GetFilters (handlers/TestReportRuntimeContracts.al:62-90).
            let table = self.table(&rec)?;
            for &(own, other) in &item.link {
                let ty = table.fields[own].ty.clone();
                let v = ops::convert(parent.borrow().fields[other].clone(), &ty)?;
                let mut t = rec.borrow_mut();
                t.filters.retain(|f| !(f.group == LINK_GROUP && f.field == own));
                t.filters.push(al_builtins::filter::FieldFilter {
                    field: own,
                    expr: al_builtins::filter::FilterExpr::Range(Some(v.clone()), Some(v)),
                    group: LINK_GROUP,
                });
            }
        }
        // The `modify` blocks of the extensions that name this item, in object id order.
        let modifies: Vec<(usize, &ir::ReportExtModify)> = match r {
            ItemRef::Base(i) => (0..run.exts.len())
                .filter_map(|k| {
                    run.ext_ir(k)
                        .and_then(|x| x.modifies.iter().find(|m| m.item == i))
                        .map(|m| (k, m))
                })
                .collect(),
            ItemRef::Added(..) => Vec::new(),
        };
        // `OnPreDataItem` of the base, with the extensions' OnBefore/OnAfterPreDataItem around it,
        // each in object id order (corpus reportextensiontrigger/TestReportExtensionBaseMembersContracts.al:
        // "for every trigger the base runs first, then the extensions").
        for &(k, m) in &modifies {
            if let Some(t) = m.before_pre {
                let (ext_inst, e) = &run.exts[k];
                self.call(*e, t, ext_inst.clone(), Vec::new())?;
            }
        }
        if let Some(t) = item.on_pre {
            self.call(object, t, owner.clone(), Vec::new())?;
        }
        for &(k, m) in &modifies {
            if let Some(t) = m.after_pre {
                let (ext_inst, e) = &run.exts[k];
                self.call(*e, t, ext_inst.clone(), Vec::new())?;
            }
        }
        self.ret_used = true;
        let mut more = matches!(self.record_op("FindSet", &rec, &mut [])?, Value::Boolean(true));
        'rows: while more {
            if run.dataset {
                self.dataset_autocalc(run, r, &rec)?;
            }
            for &(k, m) in &modifies {
                if let Some(t) = m.before {
                    let (ext_inst, e) = &run.exts[k];
                    self.call(*e, t, ext_inst.clone(), Vec::new())?;
                }
            }
            let mut flow = None;
            if let Some(t) = item.on_after_get {
                flow = self.report_trigger_call(object, t, owner.clone(), TrigKind::Row)?;
            }
            match flow {
                Some(Flow::Quit) => return Ok(true),
                Some(Flow::Break) => break 'rows,
                _ => {}
            }
            if flow.is_none() {
                for &(k, m) in &modifies {
                    if let Some(t) = m.after {
                        let (ext_inst, e) = &run.exts[k];
                        self.call(*e, t, ext_inst.clone(), Vec::new())?;
                    }
                }
                if run.dataset {
                    self.dataset_begin(run, r, &rec)?;
                }
                let mut inside = enclosing.to_vec();
                inside.push((r, rec.clone()));
                for child in run.children(r) {
                    if self.report_item(run, child, Some(rec.clone()), &inside)? {
                        return Ok(true);
                    }
                }
                if run.dataset {
                    self.dataset.as_mut().expect("a SaveAs run has a dataset").end();
                }
            }
            self.ret_used = true;
            more = !matches!(self.record_op("Next", &rec, &mut [])?, Value::Integer(0));
        }
        // `OnPostDataItem` of the base, with the extensions' OnBefore/OnAfterPostDataItem around it
        // (reportext/W3RpExtProcedures.al:SaveAs_AddedItem_RunsAfterTheParentsNestedItems_Order, BC 27.5 and 29.0).
        for &(k, m) in &modifies {
            if let Some(t) = m.before_post {
                let (ext_inst, e) = &run.exts[k];
                self.call(*e, t, ext_inst.clone(), Vec::new())?;
            }
        }
        if let Some(t) = item.on_post {
            self.call(object, t, owner.clone(), Vec::new())?;
        }
        for &(k, m) in &modifies {
            if let Some(t) = m.after_post {
                let (ext_inst, e) = &run.exts[k];
                self.call(*e, t, ext_inst.clone(), Vec::new())?;
            }
        }
        Ok(false)
    }
}

/// A data item of the running report: one of its own, or one its reportextension adds.
#[derive(Clone, Copy)]
enum ItemRef {
    Base(usize),
    /// Item `.1` of `ReportExt::added` of extension `.0` of the run (`Run::exts`).
    Added(usize, usize),
}

/// The instances a run executes: the report, and the reportextension that changes it (if any).
pub(crate) struct Run {
    engine: Arc<crate::Engine>,
    /// The report's object index.
    object: usize,
    base: Inst,
    /// The instance and object index of each extension that changes the run, in object id order.
    exts: Vec<(Inst, usize)>,
    /// The run writes the dataset of a `SaveAs` (into `Session::dataset`).
    dataset: bool,
}

impl Run {
    fn report(&self) -> &ir::Report {
        self.engine.program.objects[self.object]
            .report
            .as_ref()
            .expect("a run is of a report")
    }

    fn ext_ir(&self, k: usize) -> Option<&ir::ReportExt> {
        self.exts
            .get(k)
            .and_then(|(_, e)| self.engine.program.objects[*e].report_ext.as_ref())
    }

    fn item(&self, r: ItemRef) -> &ir::ReportItem {
        match r {
            ItemRef::Base(i) => &self.report().items[i],
            ItemRef::Added(k, i) => &self.ext_ir(k).expect("an added item has an extension").added[i].item,
        }
    }

    /// The instance that owns the item's record and triggers, and its object index.
    fn owner(&self, r: ItemRef) -> (&Inst, usize) {
        match r {
            ItemRef::Added(k, _) => (&self.exts[k].0, self.exts[k].1),
            ItemRef::Base(_) => (&self.base, self.object),
        }
    }

    fn all_items(&self) -> Vec<ItemRef> {
        let mut out: Vec<ItemRef> = (0..self.report().items.len()).map(ItemRef::Base).collect();
        for k in 0..self.exts.len() {
            let added = self.ext_ir(k).map_or(0, |x| x.added.len());
            out.extend((0..added).map(|i| ItemRef::Added(k, i)));
        }
        out
    }

    /// The items nested directly under `r`, in the order they run: a base item's own, then the
    /// ones the extension added under it.
    fn children(&self, r: ItemRef) -> Vec<ItemRef> {
        let mut out = Vec::new();
        if let ItemRef::Base(i) = r {
            out.extend(
                (0..self.report().items.len())
                    .filter(|&c| self.report().items[c].parent == Some(i))
                    .map(ItemRef::Base),
            );
        }
        // Each extension in object id order inserts next to its anchor, so the later extension ends
        // nearest it (corpus reportextensiontrigger/TestReportExtensionSamePlaceContracts.al:251:
        // `addfirst` G1 F1 F2, `addbefore` X1 X2 Y1 ChildA, `addafter` ChildA Z2 Z1).
        for k in 0..self.exts.len() {
            let Some(x) = self.ext_ir(k) else { continue };
            let mine: Vec<usize> = (0..x.added.len())
                .filter(|&c| match r {
                    ItemRef::Base(i) => x.added[c].base_parent == Some(i),
                    ItemRef::Added(kk, j) => kk == k && x.added[c].item.parent == Some(j),
                })
                .collect();
            // Position order (reportextensiontrigger/TestReportExtensionDataItemPlacementContracts.al):
            // the items added first, those before a sibling, the sibling, those after it, the items
            // added last. Items next to an item the extension added are placed once that is.
            let at = |out: &[ItemRef], a: ir::ItemAnchor| {
                out.iter().position(|c| match (c, a) {
                    (ItemRef::Base(x), ir::ItemAnchor::Base(y)) => *x == y,
                    (ItemRef::Added(kk, x), ir::ItemAnchor::Added(y)) => *kk == k && *x == y,
                    _ => false,
                })
            };
            let mut pending: Vec<usize> = mine
                .iter()
                .copied()
                .filter(|&c| matches!(x.added[c].place, ir::AddedPlace::Before(_) | ir::AddedPlace::After(_)))
                .collect();
            while !pending.is_empty() {
                let before = pending.len();
                pending.retain(|&c| {
                    let (anchor, after) = match x.added[c].place {
                        ir::AddedPlace::Before(a) => (a, false),
                        ir::AddedPlace::After(a) => (a, true),
                        _ => unreachable!("pending holds only relative places"),
                    };
                    match at(&out, anchor) {
                        Some(p) => {
                            // Items one extension adds after the same anchor keep their declaration
                            // order (ChildA, Z1, Z1b), so a later one goes behind those already
                            // placed there; the later extension's items still stand nearest the
                            // anchor (corpus reportextensiontrigger/TestReportExtensionSamePlaceContracts.al:251).
                            let mut q = p + usize::from(after);
                            if after {
                                while let Some(ItemRef::Added(kk, cc)) = out.get(q) {
                                    if *kk == k && x.added[*cc].place == x.added[c].place {
                                        q += 1;
                                    } else {
                                        break;
                                    }
                                }
                            }
                            out.insert(q, ItemRef::Added(k, c));
                            false
                        }
                        None => true,
                    }
                });
                if pending.len() == before {
                    break;
                }
            }
            let firsts: Vec<ItemRef> = mine
                .iter()
                .filter(|&&c| x.added[c].place == ir::AddedPlace::First)
                .map(|&c| ItemRef::Added(k, c))
                .collect();
            out.splice(0..0, firsts);
            out.extend(
                mine.iter()
                    .filter(|&&c| x.added[c].place == ir::AddedPlace::Last)
                    .map(|&c| ItemRef::Added(k, c)),
            );
        }
        out
    }
}

// ---------------------------------------------------------------------- Report.SaveAs

/// The text BC raises when the default layout cannot produce the format asked for
/// (handlers/TestReportSaveAsDatasetContracts.al:SaveAs_Html_WithAnRdlcLayout_IsRefused).
const FORMAT_NOT_SUPPORTED_BY_LAYOUT: &str =
    "The Html output format is not supported by the current report layout format.";

impl Session {
    /// `Report.SaveAs(Parameters, Format, OutStream[, RecordRef])` and the static form with the
    /// report number first. Only `ReportFormat::Xml` is implemented: it runs the report with no
    /// request page and writes the dataset the run produces. `Html` of a report with an RDLC layout
    /// is the one refusal BC answers with `false` that a test pins; Pdf, Word and Excel render a
    /// layout, which this runtime does not do (docs/probes-needed.md).
    fn report_save_as(&mut self, id: &str, receiver: Option<Value>, args: &[Value]) -> Result<Value, Signal> {
        let result_used = self.ret_used;
        let at = usize::from(id == "Report.SaveAs#2");
        let parameters = match args.get(at) {
            Some(Value::Text(s)) => s.clone(),
            other => return unsupported(format!("{id} with the parameters {}", other.describe())),
        };
        let format = match args.get(at + 1) {
            Some(Value::Option(n, names)) => names.get(*n as usize).cloned().unwrap_or_default(),
            other => return unsupported(format!("{id} with the format {}", other.describe())),
        };
        let blob = match args.get(at + 2) {
            Some(Value::Library(_, h)) => match &*h.borrow() {
                al_builtins::value::LibraryValue::OutStream(blob, _) => blob.clone(),
                other => return unsupported(format!("{id} to {}, which is not an OutStream", other.describe())),
            },
            other => return unsupported(format!("{id} to {}", other.describe())),
        };
        let record_ref = args.get(at + 3);
        let (inst, schema, quit) = if at == 0 {
            let (inst, quit) = self.report_instance(receiver, true)?;
            let schema = self.report_schema(&inst)?;
            (inst, schema, quit)
        } else {
            let (inst, schema) = self.report_by_number(id, args.first())?;
            (inst, schema, false)
        };
        match format.as_str() {
            "Xml" => {}
            // handlers/TestReportSaveAsDatasetContracts.al:SaveAs_Html_WithAnRdlcLayout_IsRefused: false, the
            // text in GetLastErrorText, nothing written. Whether the report's triggers run first is not pinned,
            // so none does.
            // report/PRSaveAsFormats.al SaveAs_Formats_OfReportWithoutLayoutFile: Pdf, Html, Word and Excel of a report
            // with `DefaultLayout` and no layout file answer false, write nothing and set this text. With the answer
            // dropped BC's behaviour is not pinned.
            "Pdf" | "Html" | "Word" | "Excel" if schema.report().layout_file_missing && result_used => {
                self.last_error = al_builtins::native::AlError {
                    message: format!(
                        "Report {} does not have a valid layout. Valid layout types are: Word, RDLC, Excel, and Custom.",
                        self.object_id_of(schema.object as u32)
                    ),
                    code: "Dialog".into(),
                    call_stack: String::new(),
                };
                return Ok(Value::Boolean(false));
            }
            "Html" if schema.report().layout == ir::ReportLayout::Rdlc => {
                self.last_error = al_builtins::native::AlError {
                    message: FORMAT_NOT_SUPPORTED_BY_LAYOUT.into(),
                    code: "Dialog".into(),
                    call_stack: String::new(),
                };
                return Ok(Value::Boolean(false));
            }
            other => {
                return unsupported(format!(
                    "Report.SaveAs of \"{}\" as {other}: only the Xml dataset is produced (a rendered format needs a layout \
                     renderer, and the Linux tier the corpus was measured on does not render them either)",
                    schema.name()
                ));
            }
        }
        // handlers/TestReportSaveAsColumnTypesContracts.al:372-388: false with the text and the code
        // `ReportProcessingOnly`, no trigger runs and nothing is written. With the answer dropped
        // BC's behaviour is not pinned.
        if schema.report().processing_only {
            let error = al_builtins::native::AlError {
                message: format!(
                    "Report {} is a processing-only report. Output cannot be rendered and saved to a file.",
                    self.object_id_of(schema.object as u32)
                ),
                code: "ReportProcessingOnly".into(),
                call_stack: String::new(),
            };
            // The statement form raises the same error (handlers/TestReportSaveAsColumnDetailsContracts.al:
            // ProcessingOnlyReport_StatementFormRaises, handlers/PWReportSaveAsColumns.al:
            // ProcessingOnly_AnswerDropped_AsserterrorForm); with the answer used it is returned as false.
            if !result_used {
                return Err(Signal::Error(error));
            }
            self.last_error = error;
            return Ok(Value::Boolean(false));
        }
        if let Some(why) = schema.report().dataset_unsupported.first() {
            return unsupported(format!("Report.SaveAs of \"{}\": {why}", schema.name()));
        }
        if schema.report().transaction_type.is_some() {
            return unsupported(format!(
                "Report.SaveAs of \"{}\", which declares a TransactionType (what SaveAs does with it is not pinned)",
                schema.name()
            ));
        }
        if self.global_language != 1033 {
            return unsupported("Report.SaveAs with a session language other than en-US (only en-US is pinned)");
        }
        let quit = if at == 0 { quit } else { self.report_init(&inst)? };
        if quit {
            return unsupported("CurrReport.Quit in OnInitReport of a SaveAs (what the dataset holds is not pinned)");
        }
        if at == 0 {
            // `Execute` refuses a second run on one variable with an error of its own; that this holds
            // for `SaveAs` is not pinned.
            self.report_executed.retain(|w| w.strong_count() > 0);
            if self
                .report_executed
                .iter()
                .any(|w| w.upgrade().is_some_and(|i| Rc::ptr_eq(&i, &inst)))
            {
                return unsupported("a second SaveAs on one report variable (not pinned)");
            }
            self.report_executed.push(Rc::downgrade(&inst));
        }
        if let Some(rr) = record_ref {
            self.report_set_view(&inst, &schema, Some(rr))?;
        }
        let enters = self.report_enter_world(&schema, false)?;
        let outer = self.dataset.replace(Dataset::default());
        self.dataset_labels(&schema);
        self.report_preview_pinned.push(false);
        let ran = self.report_in_world(enters, |s| s.report_body(&inst, &schema, false, Some(&parameters)));
        self.report_preview_pinned.pop();
        let written = std::mem::replace(&mut self.dataset, outer);
        match ran {
            Ok(true) => {
                let header = self.dataset_header(&schema)?;
                let bytes = written.expect("a SaveAs run has a dataset").finish(&header)?;
                al_builtins::stream::write_bytes(&blob, &bytes);
                Ok(Value::Boolean(true))
            }
            Ok(false) => unsupported("CurrReport.Quit during a SaveAs (what the dataset holds is not pinned)"),
            // An error a trigger raises: with no layout it reaches the caller
            // (reportextension/TestReportExtensionTriggers.al:SaveAsXml_ExtPreReportError_FailsTheSave); in a
            // report with an RDLC layout SaveAs answers false and leaves the stream alone
            // (handlers/TestReportSaveAsRecordRefFilter.al:TestReport_SaveAs_NoRecordRef_LeavesDataItemUnfiltered).
            // That GetLastErrorText then holds the error is not pinned.
            Err(Signal::Error(e)) => {
                let layout = schema.report().layout;
                let extended = !schema.report().extensions.is_empty();
                match (layout, extended) {
                    (ir::ReportLayout::None, _) => Err(Signal::Error(e)),
                    // An error of a trigger reaches a SaveAs whose result is not used, also with a layout
                    // (handlers/TestReportCurrReportLanguage.al:TestReport_SettingCurrReportLanguageToZero_IsRejectedAsInvalid:
                    // `asserterror Probe.SaveAs(..)` on a report with an RDLC layout).
                    (ir::ReportLayout::Rdlc, false) if !result_used => Err(Signal::Error(e)),
                    (ir::ReportLayout::Rdlc, false) => {
                        self.last_error = e;
                        Ok(Value::Boolean(false))
                    }
                    _ => unsupported(format!(
                        "an error in a SaveAs of report \"{}\", which has a layout of this kind or an extension \
                         (whether SaveAs raises it or answers false is not pinned): {}",
                        schema.name(),
                        e.message
                    )),
                }
            }
            Err(other) => Err(other),
        }
    }

    /// `RequestOptionsPage.Update(SaveRecord)` in a request page trigger. With `false` the controls show
    /// the variables as they are now, which they do on every read here, so nothing is left to do
    /// (handlers/TestReportRequestPageUpdateObjectIdContracts.al:RequestPageUpdate_RefreshesTheOtherControls:
    /// a handler sets Source to 21 and reads Derived 42 that the trigger set before it called Update(false)).
    pub(crate) fn request_page_update(&mut self, args: &[Value]) -> Result<Value, Signal> {
        match args.first() {
            // `true` shows the same: the handler reads the variables the trigger set, and the run sees them
            // (handlers/PRRequestPageUpdate.al:RequestPage_OnValidate_UpdateTrue).
            Some(Value::Boolean(_)) => Ok(Value::Null),
            other => unsupported(format!(
                "RequestPage.Update with SaveRecord {}",
                other.map_or("missing".to_string(), |v| v.describe())
            )),
        }
    }

    /// Loads the parameters string of a `SaveAs` into the data items' records, the way a request
    /// page does before the run (handlers/TestReportRunRequestPage.al:TestReport_CapturedParameters_
    /// ReplayThroughSaveAs_FilterTheDataset). Only the text this runtime's `RunRequestPage` hands out
    /// is understood (`apply_request_parameters`).
    fn apply_run_parameters(&mut self, inst: &Inst, schema: &Schema, run: &Run, xml: &str) -> Result<(), Signal> {
        let engine = self.engine.clone();
        let Some(page) = engine.program.objects[schema.object].page.as_ref() else {
            return unsupported(format!(
                "parameters for report \"{}\", which has no request page",
                schema.name()
            ));
        };
        if let Some(why) = page.unsupported.first() {
            return unsupported(format!("the request page of report \"{}\": {why}", schema.name()));
        }
        let mut exts = Vec::new();
        for &e in &page.extensions {
            let ext = match run.exts.iter().find(|(_, re)| *re == e) {
                Some((i, _)) => i.clone(),
                None => self.extension_instance(e, inst),
            };
            exts.push((e, ext));
        }
        self.apply_request_parameters(inst, &exts, schema.object, xml)
    }

    /// A column bound to a FlowField of the data item's table is calculated for every row before
    /// `OnAfterGetRecord`, because a column's AutoCalcField property defaults to true
    /// (handlers/TestReportFlowFieldColumnAutoCalc.al).
    fn dataset_autocalc(&mut self, run: &Run, r: ItemRef, rec: &Rec) -> Result<(), Signal> {
        let table = rec.borrow().table as usize;
        let engine = self.engine.clone();
        let Some(def) = engine.program.objects[table].table.as_ref() else {
            return Ok(());
        };
        let mut calc: Vec<usize> = Vec::new();
        let mut columns: Vec<&ir::ReportColumn> = run.item(r).columns.iter().collect();
        if let ItemRef::Base(i) = r {
            for k in 0..run.exts.len() {
                if let Some(x) = run.ext_ir(k) {
                    columns.extend(x.adds.iter().filter(|a| a.item == i).flat_map(|a| a.columns.iter()));
                }
            }
        }
        for c in columns {
            if let Some(p) = c.field {
                let f = &def.fields[p];
                if !f.stored && !f.flow_filter && !calc.contains(&p) {
                    calc.push(p);
                }
            }
        }
        if calc.is_empty() {
            return Ok(());
        }
        self.calc_fields(rec, &calc)
    }

    /// Opens the `DataItem` element of the current row of item `r` with its columns, evaluated now:
    /// after `OnAfterGetRecord` and before the nested items.
    fn dataset_begin(&mut self, run: &Run, r: ItemRef, rec: &Rec) -> Result<(), Signal> {
        let item = run.item(r);
        let name = dataset::dataset_name(&item.name)?;
        let (owner, object) = run.owner(r);
        let owner = owner.clone();
        let table = rec.borrow().table as usize;
        let mut columns = Vec::new();
        for c in &item.columns {
            columns.push(self.dataset_column(c, &owner, object, table)?);
        }
        if let ItemRef::Base(i) = r {
            for k in 0..run.exts.len() {
                let (ext_inst, e) = &run.exts[k];
                let Some(x) = run.ext_ir(k) else { continue };
                for add in x.adds.iter().filter(|a| a.item == i) {
                    for c in &add.columns {
                        columns.push(self.dataset_column(c, ext_inst, *e, table)?);
                    }
                }
            }
        }
        // From BC 28 each DataItem names its table and a GUID that differs per element
        // (handlers/TestReportSaveAsDatasetContracts.al:253-300, `#if BC28PLUS`). Whether the GUID is the
        // record's SystemId is not pinned; the record's own is used, which is distinct per row.
        let identity = if al_version::gate_on(
            self.engine.target.as_ref(),
            al_version::Gate::ReportSaveAsDataItemIdentity,
        ) {
            Some(self.dataset_identity(rec)?)
        } else {
            None
        };
        // An item that declares nested items writes its `DataItems` element even when no row reaches them
        // (report/W3ReportDataset.al:Rows_ParentsWithoutChildRows, Rows_ParentsWithChildRowsAndSiblingItem).
        let declares_children = match r {
            ItemRef::Base(i) => run.report().items.iter().any(|it| it.parent == Some(i)),
            ItemRef::Added(..) => false,
        };
        self.dataset.as_mut().expect("a SaveAs run has a dataset").begin(
            &name,
            identity.as_ref().map(|(t, g)| (*t, g.as_str())),
            &columns,
            declares_children,
        );
        Ok(())
    }

    /// The Labels block of the top-level data items is written even when no row reaches the column
    /// (handlers/TestReportSaveAsColumnDetailsContracts.al:LabelsAreWrittenForAnEmptyDataItem,
    /// handlers/PWReportSaveAsColumns.al:Labels_NoRowAtAll). Items nested in another are added when a
    /// row reaches them: whether an unreached nested item writes its labels is not pinned.
    fn dataset_labels(&mut self, schema: &Schema) {
        let engine = self.engine.clone();
        for item in schema.report().items.iter().filter(|i| i.parent.is_none()) {
            let Some(t) = item.table.and_then(|t| engine.tables.get(t)).and_then(|t| t.clone()) else {
                continue;
            };
            for c in item
                .columns
                .iter()
                .filter(|c| c.include_caption && c.unsupported.is_none())
            {
                let (Some(pos), Ok(name)) = (c.field, dataset::dataset_name(&c.name)) else {
                    continue;
                };
                let label = (format!("{name}Caption"), self.field_caption(&t, pos));
                let ds = self.dataset.as_mut().expect("a SaveAs run has a dataset");
                if !ds.labels.contains(&label) {
                    ds.labels.push(label);
                }
            }
        }
    }

    /// The table id and the lower case GUID of the row `rec` stands on.
    fn dataset_identity(&self, rec: &Rec) -> Result<(u32, String), Signal> {
        let object = rec.borrow().table as usize;
        let id = self.object_id_of(object as u32);
        let Some(t) = self.engine.tables.get(object).and_then(|t| t.clone()) else {
            return unsupported("a data item over a table the program did not compile");
        };
        let Some(i) = t.fields.iter().position(|f| f.id == 2_000_000_000) else {
            return unsupported("a data item over a table without a SystemId");
        };
        let Value::Guid(g) = rec.borrow().fields[i].clone() else {
            return unsupported("a data item row whose SystemId is not a Guid");
        };
        let hex: String = g.iter().map(|b| format!("{b:02x}")).collect();
        Ok((
            id,
            format!(
                "{}-{}-{}-{}-{}",
                &hex[0..8],
                &hex[8..12],
                &hex[12..16],
                &hex[16..20],
                &hex[20..32]
            ),
        ))
    }

    /// One column of a row: the value of its source, as the dataset writes it
    /// (handlers/TestReportSaveAsColumnTypesContracts.al:12-22 and :167-248, probed on BC 27.5): digits for
    /// Integer, BigInteger and Code, escaped text, `True`/`False`, `m/d/yyyy` for a Date (blank when
    /// undefined), `h:mm:ss` plus U+202F plus AM/PM for a Time, both for a DateTime, an Option and an Enum
    /// as their captions, a Duration as words, a Guid without braces, and a Decimal in full with a
    /// `decimalformatter` attribute from `DecimalPlaces` of the column, else of the field. Every other
    /// type is refused. A column that asks for its caption adds to the Labels block.
    fn dataset_column(
        &mut self,
        c: &ir::ReportColumn,
        owner: &Inst,
        object: usize,
        table: usize,
    ) -> Result<Column, Signal> {
        let name = dataset::dataset_name(&c.name)?;
        if let Some(why) = &c.unsupported {
            return unsupported(format!("report column {}: {why}", c.name));
        }
        let routine = c.routine.expect("a column without a routine is unsupported");
        let (value, _) = self.call(object, routine, owner.clone(), Vec::new())?;
        let t = self.engine.tables[table].clone();
        if c.include_caption {
            // The caption of the field the column reads (the corpus label is the field's Caption).
            let (Some(t), Some(pos)) = (&t, c.field) else {
                return unsupported(format!(
                    "report column {}: IncludeCaption on a column that is not one field (not pinned)",
                    c.name
                ));
            };
            let label = (format!("{name}Caption"), self.field_caption(t, pos));
            let ds = self.dataset.as_mut().expect("a SaveAs run has a dataset");
            if !ds.labels.contains(&label) {
                ds.labels.push(label);
            }
        }
        let mut attribute = None;
        let mut schema_type = None;
        let text = match &value {
            Value::Integer(n) => {
                schema_type = Some("xs:int");
                n.to_string()
            }
            Value::BigInteger(n) => n.to_string(),
            Value::Text(s) | Value::Code(s) => {
                schema_type = Some("xs:string");
                dataset::escape(s)?
            }
            Value::Boolean(b) => if *b { "True" } else { "False" }.to_string(),
            Value::Date(0, false) => String::new(),
            Value::Date(d, false) => dataset::date_text(*d),
            // An undefined Time or DateTime is blank like an undefined Date (docs/probes-needed.md).
            Value::Time(0) | Value::DateTime(0) => String::new(),
            Value::Time(t) => dataset::time_text(t - 1)?,
            Value::DateTime(t) => {
                let ms = t - 1;
                let day = ms.div_euclid(86_400_000) as i32 + 1;
                format!(
                    "{} {}",
                    dataset::date_text(day),
                    dataset::time_text(ms.rem_euclid(86_400_000))?
                )
            }
            Value::Decimal(_) => {
                let from_field = match (&t, c.field) {
                    (Some(t), Some(pos)) => self.field_property(t, pos, "DecimalPlaces"),
                    _ => None,
                };
                let places = c.decimal_places.clone().or(from_field);
                attribute = Some(("decimalformatter", dataset::decimal_formatter(places.as_deref())?));
                // The decimal as it is, scale and all: `12.345678 * 1.5` is `18.5185170`
                // (handlers/TestReportSaveAsColumnTypesContracts.al:174-180); Format would trim the zero.
                let Value::Decimal(d) = &value else {
                    unreachable!("matched a Decimal")
                };
                d.to_string()
            }
            // An Option field shows its OptionCaption (`Beta cap`), the way Format(Opt) does.
            Value::Option(n, _) if t.is_some() && c.field.is_some() => {
                let (t, pos) = (t.as_ref().expect("checked"), c.field.expect("checked"));
                match self.members_of_field(t, pos)? {
                    Some((_, members)) => match members.iter().find(|(ord, _, _)| ord == n) {
                        Some((_, _, caption)) => dataset::escape(caption)?,
                        None => n.to_string(),
                    },
                    None => n.to_string(),
                }
            }
            Value::Option(..) | Value::Enum(..) | Value::Duration(_) => {
                dataset::escape(&al_builtins::format::try_format(&value, 0, 0, self).map_err(Signal::Unsupported)?)?
            }
            Value::Guid(_) => {
                let g = al_builtins::format::try_format(&value, 0, 0, self).map_err(Signal::Unsupported)?;
                // Lower case, braces dropped (handlers/TestReportSaveAsColumnDetailsContracts.al:
                // UndefinedTimesGuidsHoursAndDecimalPlaces, handlers/PWReportSaveAsColumns.al:
                // Undefined_TimeDateTime_SingleDigitHour_GuidWithHexLetters); the null Guid is zeros, not blank.
                g.trim_matches(|ch| ch == '{' || ch == '}').to_ascii_lowercase()
            }
            other => {
                return unsupported(format!(
                    "report column {} holds {other}, a type the dataset has no pinned text for",
                    c.name,
                    other = other.describe()
                ));
            }
        };
        Ok(Column {
            name,
            attribute,
            text,
            schema_type,
        })
    }

    /// The header of the dataset: what the report, its app, the session and the clock say.
    fn dataset_header(&self, schema: &Schema) -> Result<dataset::Header, Signal> {
        let engine = &schema.engine;
        let object = &engine.program.objects[schema.object];
        let report = schema.report();
        let app = engine.program.apps.get(object.app as usize);
        let ms = self.now_millis();
        let day = ms.div_euclid(86_400_000) + i64::from(al_builtins::format::day_from_civil(1970, 1, 1));
        let (y, mo, d) = al_builtins::format::civil_from_day(day as i32);
        let in_day = ms.rem_euclid(86_400_000);
        let id: String = {
            let g = self.host.identity.company_id();
            let h: String = g.iter().map(|b| format!("{b:02x}")).collect();
            format!(
                "{}-{}-{}-{}-{}",
                &h[0..8],
                &h[8..12],
                &h[12..16],
                &h[16..20],
                &h[20..32]
            )
        };
        Ok(dataset::Header {
            extension_id: app.map(|a| a.id.clone()).unwrap_or_default(),
            extension_name: app.map(|a| a.name.clone()).unwrap_or_default(),
            extension_publisher: app.map(|a| a.publisher.clone()).unwrap_or_default(),
            extension_version: app.map(|a| a.version.clone()).unwrap_or_default(),
            report_id: object.id.map_or(String::new(), |i| i.to_string()),
            report_name: object.name.clone(),
            report_caption: report.caption.clone().unwrap_or_else(|| object.name.clone()),
            about_title: report.about_title.clone().unwrap_or_default(),
            about_text: report.about_text.clone().unwrap_or_default(),
            // The manifest of both apps the corpus measured states no contextSensitiveHelpUrl, and the
            // page then states none either (handlers/TestReportDatasetHelpLink.al).
            help_link: String::new(),
            company_name: self.cx.company.clone(),
            company_id: id,
            user_name: self.host.identity.user_id(),
            now: (y, mo, d, (in_day / 3_600_000) as u32, (in_day / 60_000 % 60) as u32),
            company_block: self.has(al_version::Gate::ReportSaveAsXmlHeaderHasCompanyBlock),
        })
    }
}
