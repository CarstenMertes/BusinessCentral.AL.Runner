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
use al_builtins::builtin_id::ids;
use al_builtins::describe::Describe;
use al_builtins::ops;
use al_builtins::record_op::RecordOp;
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
///
/// Pinned by handlers/TestReportRuntimeContracts.al:322-368 (a data item's `OnAfterGetRecord`) and
/// report/W3ReportSkipBreakQuit.al (the others, also from a procedure the trigger calls):
///
/// | trigger | Skip | Break | Quit |
/// |---|---|---|---|
/// | OnAfterGetRecord | skips the row | ends the rows | ends the run |
/// | OnPreReport | no effect | no effect | ends the run |
/// | OnPreDataItem | no effect | no rows, OnPostDataItem still runs | ends the run |
/// | OnPostDataItem | no effect | no effect | ends the run |
/// | OnPostReport | no effect | no effect | no effect |
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TrigKind {
    Row,
    Pre,
    PreItem,
    PostItem,
    PostReport,
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
        id: al_ir::BuiltinId,
        receiver: Option<Value>,
        args: &[Value],
        running: Inst,
    ) -> Result<Value, Signal> {
        match id {
            ids::REPORT_QUIT_1 => self.report_quit_builtin(&running),
            ids::REPORT_SKIP_1 => self.report_flow_builtin(&running, Flow::Skip),
            ids::REPORT_BREAK_1 => self.report_flow_builtin(&running, Flow::Break),
            ids::REPORT_SETTABLEVIEW_1 => {
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
            ids::REPORT_LANGUAGE_1 => match args.first() {
                Some(Value::Integer(0)) => Err(crate::transactions::al_error_code(
                    "Invalid language ID supplied: 0.",
                    "Argument",
                )),
                Some(Value::Integer(n)) if *n > 0 && *n <= 0xFFFF => {
                    self.report.curr_report_language = Some(*n);
                    Ok(Value::Null)
                }
                // Only 99999 is pinned as an unsupported language (misc/lt-probes-needed-3/PnC06ReportLanguage.al:Language_Invalid);
                // the rest of the range above 65535 is refused (docs/probes-needed.md).
                Some(Value::Integer(n @ 99999)) => Err(crate::transactions::al_error_code(
                    &format!(
                        "An unsupported language was selected ('{n}' - 'Requested culture is not supported by the current platform')."
                    ),
                    "ReportLanguageNotFound",
                )),
                // A negative language raises nothing and changes nothing (PnC06ReportLanguage.al:Language_Invalid, `-1`: no
                // error, `post:1033`). That is measured for a report that had not set a language; after one it is not pinned.
                Some(Value::Integer(n)) if *n > 0xFFFF => unsupported(format!(
                    "CurrReport.Language set to {n}: only 99999 is pinned as an unsupported language (docs/probes-needed.md)"
                )),
                Some(Value::Integer(-1)) if self.report.curr_report_language.is_none() => Ok(Value::Null),
                Some(Value::Integer(n)) => unsupported(format!(
                    "CurrReport.Language set to {n} after a language was set: whether a negative language keeps it is not pinned (docs/probes-needed.md)"
                )),
                None => Ok(Value::Integer(
                    self.report.curr_report_language.unwrap_or(self.global_language),
                )),
                Some(other) => unsupported(format!("CurrReport.Language with {}", other.describe())),
            },
            // `FormatRegion` only takes effect when a Word or Excel layout renders; a run that writes the
            // dataset (an Xml export) never does, and the property reads back empty
            // (handlers/TestReportCurrReportLanguage.al:12-18, :76-83). In a report started by `Run` the region is kept,
            // reads back, and `Format` of a date and a decimal follows it (misc/lt-probes-needed-3/PnC06ReportLanguage.al:
            // Language_SetAndRead, `region0:|region1:de-DE|date2:05.03.24|num2:1.234,5`).
            ids::REPORT_FORMATREGION_1 => match args.first() {
                Some(Value::Text(t)) if self.report.report_preview_pinned.last() == Some(&true) => {
                    self.report.format_region = Some(t.to_string());
                    Ok(Value::Null)
                }
                Some(Value::Code(t)) if self.report.report_preview_pinned.last() == Some(&true) => {
                    self.report.format_region = Some(t.to_string());
                    Ok(Value::Null)
                }
                Some(Value::Text(_) | Value::Code(_)) => Ok(Value::Null),
                None if self.report.report_preview_pinned.last() == Some(&true) => Ok(Value::Text(
                    self.report.format_region.clone().unwrap_or_default().into(),
                )),
                None => Ok(Value::Text(String::new().into())),
                Some(other) => unsupported(format!("CurrReport.FormatRegion with {}", other.describe())),
            },
            // `ObjectId(UseNames)`: `Report 69730`, or with names (the default) `Report <Caption>`, the object name
            // when the report has no Caption (handlers/PWRefusedBuiltins.al:78-107, BC 27.5; also through a Report
            // variable).
            ids::REPORT_OBJECTID_1 => {
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
            ids::REPORT_PREVIEW_1 => match self.report.report_preview_pinned.last() {
                Some(true) => Ok(Value::Boolean(false)),
                _ => unsupported(
                    "Report.Preview outside a report started by Run or RunModal (whether an Execute, a SaveAs or a request page preview is a preview is not pinned)",
                ),
            },
            ids::REPORT_USEREQUESTPAGE_1 => {
                // `CurrReport.UseRequestPage()` has no receiver value: it is the running report, as for ObjectId (Copy Sales
                // Document OnPreReport, 186 Tests-ERM tests; the getter of a report run by Run is pinned by
                // handlers/TestReportRuntimeContracts.al:416-440 for a variable).
                let receiver = receiver.or_else(|| Some(Value::Codeunit(running.clone())));
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
            ids::REPORT_RUN_1 | ids::REPORT_RUNMODAL_1 => {
                let (inst, quit) = self.report_instance_for(receiver, true, id == ids::REPORT_RUN_1)?;
                // An instance that already ran (one a variable gave up after its Run) cannot run again, and the attempt resets it
                // (report/lt-report-this-passed-on/P141ThisStored.al:P141_StoredAfterRun_RunAgain: the error, then a blank marker).
                if id == ids::REPORT_RUN_1
                    && self
                        .report
                        .report_executed
                        .iter()
                        .any(|w| w.upgrade().is_some_and(|i| Rc::ptr_eq(&i, &inst)))
                {
                    self.report_clear(&inst);
                    return Err(crate::transactions::al_error_code(
                        "The object has been run. Use the function CLEAR(Report).",
                        "ObjectHasAlreadyBeenRun",
                    ));
                }
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
                // `RunModal` does not clear it: a getter called after it reads what the run collected (corpus
                // reportextensiontrigger/TestPrecompiledReportExtensionDataItems.al: "Run frees the variable's report
                // instance when it ends ... RunModal keeps it", as Base Application's Reservation Worksheet relies on).
                if id == ids::REPORT_RUN_1 && !self.report.rebind_after_run {
                    self.report_clear(&inst);
                }
                result.map(|()| Value::Null)
            }
            // The static forms run a fresh instance of the report with that id.
            ids::REPORT_RUN_3 | ids::REPORT_RUNMODAL_3 => {
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
            ids::REPORT_EXECUTE_1 => {
                let (inst, quit) = self.report_instance(receiver, true)?;
                let schema = self.report_schema(&inst)?;
                self.report_execute_args(id, args.first(), args.get(1))?;
                // handlers/TestReportRuntimeContracts.al:395-414: Execute keeps the globals, and a
                // second Execute on the same variable is refused.
                if self
                    .report
                    .report_executed
                    .iter()
                    .any(|w| w.upgrade().is_some_and(|i| Rc::ptr_eq(&i, &inst)))
                {
                    return Err(al_error("The object has been run. Use the function CLEAR(Report)."));
                }
                self.report.report_executed.retain(|w| w.strong_count() > 0);
                self.report.report_executed.push(Rc::downgrade(&inst));
                if quit {
                    return Ok(Value::Null);
                }
                self.report_run(&inst, &schema, Entry::Execute, false)
                    .map(|()| Value::Null)
            }
            ids::REPORT_EXECUTE_3 => {
                let (inst, schema) = self.report_by_number(id, args.first())?;
                self.report_execute_args(id, args.get(1), args.get(2))?;
                let quit = self.report_init(&inst)?;
                if quit {
                    return Ok(Value::Null);
                }
                self.report_run(&inst, &schema, Entry::Execute, false)
                    .map(|()| Value::Null)
            }
            ids::REPORT_PRINT_1 | ids::REPORT_PRINT_2 => {
                let schema = if id == ids::REPORT_PRINT_1 {
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
            ids::REPORT_SAVEAS_1 | ids::REPORT_SAVEAS_2 => self.report_save_as(id, receiver, args),
            ids::REPORT_SAVEASPDF_1 => self.report_save_as_file(id, receiver, args, "Pdf", false),
            ids::REPORT_SAVEASPDF_2 => self.report_save_as_file(id, receiver, args, "Pdf", true),
            ids::REPORT_SAVEASEXCEL_1 => self.report_save_as_file(id, receiver, args, "Excel", false),
            ids::REPORT_SAVEASEXCEL_2 => self.report_save_as_file(id, receiver, args, "Excel", true),
            ids::REPORT_SAVEASWORD_1 => self.report_save_as_file(id, receiver, args, "Word", false),
            ids::REPORT_SAVEASWORD_2 => self.report_save_as_file(id, receiver, args, "Word", true),
            ids::REPORT_SAVEASHTML_1 => self.report_save_as_file(id, receiver, args, "Html", false),
            ids::REPORT_SAVEASHTML_2 => self.report_save_as_file(id, receiver, args, "Html", true),
            ids::REPORT_WORDXMLPART_1 | ids::REPORT_WORDXMLPART_2 => self.report_word_xml_part(id, receiver, args),
            ids::REPORT_RUNREQUESTPAGE_1 | ids::REPORT_RUNREQUESTPAGE_2 => {
                let (inst, schema, seed) = if id == ids::REPORT_RUNREQUESTPAGE_1 {
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
    fn report_word_xml_part(
        &mut self,
        id: al_ir::BuiltinId,
        receiver: Option<Value>,
        args: &[Value],
    ) -> Result<Value, Signal> {
        let static_form = id == ids::REPORT_WORDXMLPART_2;
        let (flag_at, schema) = if static_form {
            let known = match args.first() {
                Some(Value::Integer(n)) => self.engine.program.objects.iter().any(|o| {
                    (o.is_kind(al_ir::ObjKind::Report) || o.is_shell_of(al_ir::ObjKind::Report))
                        && o.id == u32::try_from(*n).ok()
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

    fn report_by_number(&mut self, id: al_ir::BuiltinId, arg: Option<&Value>) -> Result<(Inst, Schema), Signal> {
        let Some(Value::Integer(n)) = arg else {
            return unsupported(format!("{id} with an id that is not an integer"));
        };
        let Some(object) = self
            .engine
            .program
            .objects
            .iter()
            .position(|o| o.is_kind(al_ir::ObjKind::Report) && o.id == u32::try_from(*n).ok())
        else {
            // A report of a dependency app that nothing reached is not compiled (ADR-013); BC has it.
            if self
                .engine
                .program
                .objects
                .iter()
                .any(|o| o.is_shell_of(al_ir::ObjKind::Report) && o.id == u32::try_from(*n).ok())
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
        id: al_ir::BuiltinId,
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
        // A report variable a `Run` just reset starts again with its next call of anything but `Run`: `OnInitReport`
        // runs again, so a procedure called after a Run sees what OnInitReport set (corpus
        // pages/lt-page-report-events/P84ReportSender.al:Triggers_Run_SenderInEachTrigger: `GetMarker()` after `Run()` is
        // `init`; report/TestProbeReportContracts.al:ReportVariable_RunTwice_GlobalsReset for the builtins).
        if std::mem::take(&mut inst.borrow_mut().rerun) {
            inst.borrow_mut().initialized = false;
        }
        if inst.borrow().initialized {
            return Ok(());
        }
        if self.report_init(inst)? {
            return unsupported("CurrReport.Quit in OnInitReport while a procedure first uses the report (not pinned)");
        }
        // A procedure the caller runs on the variable before the run (`SetParameters`) may set what the request page
        // shows (`restore_page_values`).
        if self.report.report_trigger.is_none()
            && self.engine.program.objects[inst.borrow().object as usize]
                .report
                .is_some()
        {
            self.report.prepared.push(Rc::downgrade(inst));
        }
        Ok(())
    }

    /// Makes the globals of `inst` and, for a report that had none, runs `OnInitReport`. Returns
    /// whether the trigger quit the report.
    fn report_init(&mut self, inst: &Inst) -> Result<bool, Signal> {
        let first = !inst.borrow().initialized;
        if first {
            self.report
                .prepared
                .retain(|w| w.upgrade().is_some_and(|i| !Rc::ptr_eq(&i, inst)));
        }
        self.ensure_initialized(inst);
        let object = inst.borrow().object as usize;
        let engine = self.engine.clone();
        let o = &engine.program.objects[object];
        if !first || !o.is_kind(al_ir::ObjKind::Report) {
            return Ok(false);
        }
        let Some(on_init) = o.report.as_ref().and_then(|r| r.on_init) else {
            return Ok(false);
        };
        // CurrReport.Quit in OnInitReport stops OnPreReport and the body
        // (handlers/TestReportOnInitReportTiming.al:88-98, QuitInOnInitReport_StopsPreReportAndBody).
        let saved = self.report.report_init_depth.replace(self.depth + 1);
        self.report.report_quit = false;
        let r = self.call(object, on_init, inst.clone(), Vec::new());
        self.report.report_init_depth = saved;
        let quit = std::mem::take(&mut self.report.report_quit);
        r?;
        Ok(quit)
    }

    /// `CurrReport.Quit`: from `OnInitReport` (handlers/TestReportOnInitReportTiming.al:88-98),
    /// `OnPreReport` and `OnAfterGetRecord` (handlers/TestReportRuntimeContracts.al:355-368); it is
    /// not pinned anywhere else.
    fn report_quit_builtin(&mut self, running: &Inst) -> Result<Value, Signal> {
        let object = running.borrow().object as usize;
        if self.engine.program.objects[object].is_kind(al_ir::ObjKind::Report)
            && self.report.report_init_depth == Some(self.depth)
        {
            self.report.report_quit = true;
            // Leaves the trigger; `report_init` sees the flag.
            return Err(Signal::Exit(None));
        }
        self.report_flow_builtin(running, Flow::Quit)
    }

    /// `CurrReport.Skip`, `Break` and `Quit` called straight from a report trigger: the trigger
    /// ends and the run reads the request (`report_trigger_call`).
    fn report_flow_builtin(&mut self, running: &Inst, flow: Flow) -> Result<Value, Signal> {
        let object = running.borrow().object as usize;
        let on_report = {
            let o = &self.engine.program.objects[object];
            o.is_kind(al_ir::ObjKind::Report) || o.is_kind(al_ir::ObjKind::ReportExtension)
        };
        // From the trigger itself or from a procedure it calls (report/W3ReportSkipBreakQuit.al:
        // *_InProcedureCalledFromOnAfterGetRecord); every other trigger kind is not pinned.
        let allowed = match self.report.report_trigger {
            Some((depth, kind)) if on_report && self.depth >= depth => kind != TrigKind::Other,
            _ => false,
        };
        if !allowed {
            return unsupported(format!(
                "CurrReport.{flow:?} outside the triggers where it is pinned (what it does there is not pinned)"
            ));
        }
        self.report.report_flow = Some(flow);
        // The trigger ends, and the procedures between it and the call end with it (`call_inner`).
        if let Some((depth, _)) = self.report.report_trigger {
            self.report.flow_unwind = Some(depth);
        }
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
        let saved = self.report.report_trigger.replace((self.depth + 1, kind));
        self.report.report_flow = None;
        let r = self.call(object, routine, inst, Vec::new());
        self.report.report_trigger = saved;
        self.report.flow_unwind = None;
        let flow = self.report.report_flow.take();
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
    /// A variable after `Run` holds a new instance; whoever kept the instance that ran (a SingleInstance codeunit that stored `this`) still
    /// reads its state (report/lt-report-this-passed-on/P141ThisStored.al:P141_StoredAfterRun_ReadAndChange `ok:post`, _SecondRunReplaces).
    pub(crate) fn report_fresh_instance(&mut self, old: &Inst) -> Inst {
        let object = old.borrow().object as usize;
        let fresh = self.instance(object);
        {
            let mut f = fresh.borrow_mut();
            f.initialized = true;
            f.rerun = true;
        }
        fresh
    }

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
        // No data item over the table is an error; with several the view filters the first
        // (corpus errors/lt-ms-refusals-136/P136Report.al:P136_Report_ViewOverTableWithoutDataItem and
        // P136_Report_ViewWithTwoDataItemsOverTheTable: `A:a;A:b;B:a;B:b;B:c;`).
        let Some(&item) = items.first() else {
            let (id, name) = {
                let o = &self.engine.program.objects[table as usize];
                (o.id.unwrap_or(0), o.name.clone())
            };
            return Err(Signal::Error(Box::new(al_builtins::native::AlError {
                message: format!(
                    "The {} report does not have a DataItem that uses the table (Table {id} {name}) specified in the function SetTableView.",
                    schema.name()
                ),
                code: "ReportNoDataItem".into(),
                call_stack: String::new(),
                ..Default::default()
            })));
        };
        let v = view.borrow();
        // The data item reads the table itself, so a temporary record's rows do not count, only its filters do; the marks of a
        // marked-only record, its key and its direction carry over (corpus errors/lt-ms-refusals-136/P136Report.al:
        // temporary, temporaryFiltered, markedOnly, markedNotOnly, keyQty, keyQtyDescending, descendingPrimary, keyQtyFiltered).
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
        let key = Rc::as_ptr(&target) as usize;
        let origin: Vec<crate::state::ViewFilterKey> = v
            .filters
            .iter()
            .map(|f| Ok((f.field, f.group, self.filter_text(&f.expr)?)))
            .collect::<Result<_, Signal>>()?;
        self.report.set_view_filters.retain(|(k, _)| *k != key);
        self.report.set_view_filters.push((key, origin));
        t.filters = v.filters.clone();
        t.filter_group = v.filter_group;
        t.key = v.key;
        t.adhoc = v.adhoc.clone();
        t.ascending = v.ascending;
        t.field_descending = v.field_descending;
        if !v.temporary {
            t.marks = v.marks.clone();
            t.marked_only = v.marked_only;
        }
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
        if self.tx.try_depth > 0 {
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
        self.report.report_preview_pinned.push(entry == Entry::Run);
        // The language and region a trigger sets belong to this run (PnC06ReportLanguage.al:Language_Invalid starts three
        // runs with no language set after Language_SetAndRead set one).
        let outer_region = self.report.format_region.take();
        let outer_language = self.report.curr_report_language.take();
        let ran = self.report_in_world(enters, |s| s.report_body(inst, schema, shows_page, None));
        self.report.format_region = outer_region;
        self.report.curr_report_language = outer_language;
        self.report.report_preview_pinned.pop();
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
        self.tx.tx_world += 1;
        let result = body(self);
        self.tx.tx_world -= 1;
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
        let before = self.pages.saved_values.get(&(schema.object as u32)).cloned();
        let ext: Vec<(usize, Inst)> = run.exts.iter().map(|(i, e)| (*e, i.clone())).collect();
        let closed = self.run_request_page(inst, schema.object, seed, ext, true);
        match before {
            Some(v) => {
                self.pages.saved_values.insert(schema.object as u32, v);
            }
            None => {
                self.pages.saved_values.remove(&(schema.object as u32));
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
            // A view that only filters keeps the key and direction a SetTableView gave the item (corpus
            // report/lt-report-set-table/ReportSetTableViewOverWhere.al:P163_SetTableView_OverWhere_KeyOfTheView: key Txt, 4|3|2).
            let kept_order = {
                let r = rec.borrow();
                (r.key, r.adhoc.clone(), r.ascending, r.field_descending)
            };
            let sorts = view.to_ascii_lowercase().contains("sorting");
            // The filters of a DataItemTableView are the report's, not the record's: GetFilters does not show them, in
            // OnPostReport either (report/TestProbeReportContracts.al:TriggerOrder_NestedAndSecondItem).
            self.set_view_in(&rec, &t, &view, Some(LINK_GROUP), false, false)?;
            if !sorts {
                let mut r = rec.borrow_mut();
                (r.key, r.adhoc, r.ascending, r.field_descending) = kept_order;
            }
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

    /// A filter a procedure of the report set on a data item (`RowDi.SetFilter`) before the request page opens is not on the page: the
    /// page shows the filters of a `SetTableView` and what was stored (report/lt-requestpage-savevalues-date-text-setvalue/
    /// P147SaveValuesFilter.al:P147_Filter_ProcedureFilterVsStored, `procedure filter B* code filter = []`; the view of
    /// P147_Filter_SetTableViewVsStored is shown). The page's own filters replace the item's user filters when it closes with OK, so what a
    /// procedure set is gone for that run; with a cancelled page there is no run. Whether the run keeps it is not pinned (registry entry
    /// report-procedure-filter-under-request-page).
    fn hide_procedure_filters(&mut self, run: &Run) -> Result<bool, Signal> {
        let mut any = false;
        for r in run.all_items() {
            let rec = self.report_item_record(run, r)?;
            let key = Rc::as_ptr(&rec) as usize;
            let from_view: Vec<crate::state::ViewFilterKey> = self
                .report
                .set_view_filters
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.clone())
                .unwrap_or_default();
            let mut dropped = false;
            let mut kept = Vec::new();
            let filters = rec.borrow().filters.clone();
            for f in filters {
                let keep = f.group != 0 || from_view.contains(&(f.field, f.group, self.filter_text(&f.expr)?));
                dropped |= !keep;
                if keep {
                    kept.push(f);
                }
            }
            rec.borrow_mut().filters = kept;
            any |= dropped;
        }
        Ok(any)
    }

    /// The error of a rendering run of a report with columns and no layout of any kind (none of DefaultLayout, the layout properties,
    /// a rendering section), `None` when the report has a layout, is processing only or has no column to render.
    fn no_valid_layout(&self, schema: &Schema) -> Result<Option<al_builtins::native::AlError>, Signal> {
        let report = schema.report();
        if report.processing_only
            || report.layout != ir::ReportLayout::None
            || report.layout_file_missing
            || report.output.iter().any(|r| r.contains("(a layout)"))
        {
            return Ok(None);
        }
        if !report.items.iter().any(|i| !i.columns.is_empty()) {
            return Ok(None);
        }
        Ok(Some(al_builtins::native::AlError {
            message: format!(
                "Report {} does not have a valid layout. Valid layout types are: Word, RDLC, Excel, and Custom.",
                self.object_id_of(schema.object as u32)
            ),
            code: "Dialog".into(),
            call_stack: String::new(),
            ..Default::default()
        }))
    }

    /// `OnPreReport`, every top-level data item, `OnPostReport`; a reportextension's triggers
    /// follow the base report's own. The order of the report triggers among themselves is pinned
    /// by reportextension/TestReportExtensionTriggers.al:244-250 (base pre, extension pre, base
    /// post, extension post); the rest is the order the BC documentation gives
    /// (docs/probes-needed.md).
    ///
    /// `save_as` is the parameters string of a `Report.SaveAs(.., Xml, ..)` run: the run writes the
    /// dataset into `self.report.dataset` and renders nothing. Returns false when the report was quit.
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
            let hidden = self.hide_procedure_filters(&run)?;
            let ext: Vec<(usize, Inst)> = run.exts.iter().map(|(i, e)| (*e, i.clone())).collect();
            match self.run_request_page(inst, schema.object, None, ext, false)?.0 {
                RequestClosed::Cancel => return Ok(false),
                // The handler's `SaveAsXml` ran the report there and then
                // (al-language-onprem/handlers/TestRequestPageSaveAsXmlContracts.al:
                // SaveAsXml_RunsTheReportOnce_AndClosesTheRequestPage): it does not run a second time.
                RequestClosed::SavedAsXml => return Ok(true),
                RequestClosed::Ok => {
                    if hidden {
                        al_builtins::assumptions::assume(
                            al_builtins::assumptions::Assumption::ReportProcedureFilterUnderRequestPage,
                        );
                    }
                }
            }
        }
        self.report_after_page(&run, schema)
    }

    /// Everything of a run after its request page: the triggers and the data items.
    pub(crate) fn report_after_page(&mut self, run: &Run, schema: &Schema) -> Result<bool, Signal> {
        let report = schema.report();
        // A report that renders and declares no layout at all stops after OnInitReport and the request page with an error: no
        // OnPreReport, no data item (report/lt-report-pre-rendering/ReportPreRenderingRun.al: P195_ReportRun_PreRenderingOrder,
        // P195_ReportExecute_PreRenderingOrder; a cancelled request page ends the run first, pages/lt-page-presentation-properties/
        // P119ReportColumn.al).
        if !run.dataset {
            if let Some(e) = self.no_valid_layout(schema)? {
                return Err(Signal::Error(Box::new(e)));
            }
        }
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
                // The corpus pins filters a trigger sets on a link field (misc/lt-probes-needed-8/PnH3Link.al:
                // Link_ChildSetsOwnFilters_InPreDataItem: both apply). One the view or the caller sets before the run is in
                // the report's filter group, where the link replaces the filter on its field: an assumption.
                al_builtins::assumptions::assume(
                    al_builtins::assumptions::Assumption::ReportItemFilterOnLinkFieldReplaced,
                );
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
        // OnPreRendering runs once the data items are done and before OnPostReport, with an empty payload, in a run that writes the dataset
        // (report/lt-report-pre-rendering/ReportPreRenderingSaveAsXml.al: P195_SaveAsXml_PreRenderingOrder `OnPreRendering keys=0`). A
        // run that renders is refused above or stops on its missing layout; the trigger of such a run is not pinned.
        if let Some(r) = report.on_pre_rendering {
            let payload = Value::Library(
                "JsonObject".into(),
                std::rc::Rc::new(std::cell::RefCell::new(al_builtins::value::LibraryValue::Empty)),
            );
            self.call(run.object, r, run.base.clone(), vec![payload])?;
        }
        if let Some(r) = report.on_post {
            self.report_trigger_call(run.object, r, run.base.clone(), TrigKind::PostReport)?;
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
        let mut rec = self.report_item_record(run, r)?;
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
        let mut no_rows = false;
        if let Some(t) = item.on_pre {
            match self.report_trigger_call(object, t, owner.clone(), TrigKind::PreItem)? {
                Some(Flow::Quit) => return Ok(true),
                Some(Flow::Break) => no_rows = true,
                _ => {}
            }
        }
        for &(k, m) in &modifies {
            if let Some(t) = m.after_pre {
                let (ext_inst, e) = &run.exts[k];
                self.call(*e, t, ext_inst.clone(), Vec::new())?;
            }
        }
        // A `ClearAll` in `OnPreDataItem` gave the item a fresh record, with no filters; the rows are read into that one
        // (corpus errors/lt-ms-refusals-135/P135ClearAllReport.al:P135_ClearAll_In_OnPreDataItem: `row 1`, `row 2`).
        rec = self.report_item_record(run, r)?;
        self.ret_used = true;
        let mut more = !no_rows && matches!(self.record_op(RecordOp::FindSet, &rec, &mut [])?, Value::Boolean(true));
        let mut iterations = 0u32;
        'rows: while more {
            iterations += 1;
            // The CalcFields property: the listed FlowFields of the row, before its triggers. Pinned for one field of a
            // top-level item (misc/lt-probes-needed-8/PnH3Link.al:CalcFields_Property_CalculatesTheFlowField); a nested
            // item or a list of several fields is the same on an assumption.
            if !item.calc_fields.is_empty() {
                if item.parent.is_some() || item.calc_fields.len() > 1 {
                    al_builtins::assumptions::assume(
                        al_builtins::assumptions::Assumption::ReportItemCalcFieldsNestedOrList,
                    );
                }
                self.calc_fields(&rec, &item.calc_fields)?;
            }
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
                    self.report
                        .dataset
                        .as_mut()
                        .expect("a SaveAs run has a dataset")
                        .end_row(item.print_only_if_detail);
                }
            }
            // MaxIteration stops an Integer item after that many rows (misc/lt-probes-needed-8/PnH3Link.al:MaxIteration_*).
            if item.max_iteration.is_some_and(|m| iterations >= m) {
                break 'rows;
            }
            self.ret_used = true;
            more = !matches!(self.record_op(RecordOp::Next, &rec, &mut [])?, Value::Integer(0));
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
            if self.report_trigger_call(object, t, owner.clone(), TrigKind::PostItem)? == Some(Flow::Quit) {
                return Ok(true);
            }
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

/// Where a rendering save puts its output.
enum SaveTarget {
    Stream(al_builtins::stream::BlobRef),
    File(String),
}

/// A report whose rendered output nothing reads (ADR-040, section 3). A rendering save of one runs the report and
/// writes no output. The list grows by evidence only: a Microsoft test that discards the output, cited here and in
/// docs/probes-needed.md (rule 3 exception of 2026-10-15). Layout smoke tests, `CheckFileNotEmpty` tests and Excel
/// cell tests are never on it.
pub(crate) struct IgnoredOutput {
    /// The id of the app that declares the report (Base Application).
    pub app: &'static str,
    pub id: u32,
    pub name: &'static str,
    /// Why the output is not the point, and the Microsoft tests that need the report to run.
    #[allow(dead_code)] // read by the unit test of the list and by whoever extends it
    pub why: &'static str,
}

pub(crate) const IGNORED_OUTPUT: &[IgnoredOutput] = &[
    IgnoredOutput {
        app: "437dbf0e-84ff-417a-965d-ed2bb9650972",
        id: 1002,
        name: "Post Inventory Cost to G/L",
        why: "posts to G/L in its data item triggers; Library - Costing.PostInvtCostToGL and Library - Assembly.PostInvtCostToGL \
              save it to a file nobody reads (SCM Kitting D3 Part 1 and 2, SCM WIP Costing Production I and II, SCM Costing Batch)",
    },
    IgnoredOutput {
        app: "437dbf0e-84ff-417a-965d-ed2bb9650972",
        id: 1401,
        name: "Check",
        why: "prints and posts check ledger entries; the file name is a GUID or a temp name nobody reads (ERM Cash Manager, \
              Match Bank Rec. Scenarios)",
    },
];

impl Session {
    /// Whether the report is on the allow-list of reports whose rendered output is not read (ADR-040).
    pub(crate) fn output_is_ignored(&self, schema: &Schema) -> bool {
        let program = &self.engine.program;
        let obj = &program.objects[schema.object];
        let Some(app) = program.apps.get(obj.app as usize) else {
            return false;
        };
        IGNORED_OUTPUT
            .iter()
            .any(|e| e.id == obj.id.unwrap_or(0) && e.name == obj.name && e.app.eq_ignore_ascii_case(&app.id))
    }

    /// `Report.SaveAs(Parameters, Format, OutStream[, RecordRef])` and the static form with the
    /// report number first. Only `ReportFormat::Xml` is implemented: it runs the report with no
    /// request page and writes the dataset the run produces. `Html` of a report with an RDLC layout
    /// is the one refusal BC answers with `false` that a test pins; Pdf, Word and Excel render a
    /// layout, which this runtime does not do (docs/probes-needed.md).
    fn report_save_as(
        &mut self,
        id: al_ir::BuiltinId,
        receiver: Option<Value>,
        args: &[Value],
    ) -> Result<Value, Signal> {
        let at = usize::from(id == ids::REPORT_SAVEAS_2);
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
        self.report_save_core(
            id,
            receiver,
            args,
            at,
            parameters,
            &format,
            SaveTarget::Stream(blob),
            record_ref,
        )
    }

    /// `SaveAsPdf`, `SaveAsExcel`, `SaveAsWord` and `SaveAsHtml` with a file name, on a report variable (`FileName`) or by
    /// number (`Number, FileName`). Only a report on the allow-list runs (ADR-040); the others stay refused.
    fn report_save_as_file(
        &mut self,
        id: al_ir::BuiltinId,
        receiver: Option<Value>,
        args: &[Value],
        format: &str,
        by_number: bool,
    ) -> Result<Value, Signal> {
        let at = usize::from(by_number);
        let name = match args.get(at) {
            Some(Value::Text(s) | Value::Code(s)) => s.to_string(),
            other => return unsupported(format!("{id} with the file name {}", other.describe())),
        };
        if args.get(at + 1).is_some() {
            return unsupported(format!("{id} with a record argument (what it filters is not pinned)"));
        }
        self.report_save_core(
            id,
            receiver,
            args,
            at,
            String::new().into(),
            format,
            SaveTarget::File(name),
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn report_save_core(
        &mut self,
        id: al_ir::BuiltinId,
        receiver: Option<Value>,
        args: &[Value],
        at: usize,
        parameters: al_builtins::text::Txt,
        format: &str,
        target: SaveTarget,
        record_ref: Option<&Value>,
    ) -> Result<Value, Signal> {
        let result_used = self.ret_used;
        let format = format.to_string();
        // ADR-040: a rendering save of a listed report runs it and writes nothing.
        let mut discard = false;
        let (inst, schema, quit) = if at == 0 {
            let (inst, quit) = self.report_instance(receiver, true)?;
            let schema = self.report_schema(&inst)?;
            (inst, schema, quit)
        } else {
            let (inst, schema) = self.report_by_number(id, args.first())?;
            (inst, schema, false)
        };
        let custom = if format == "Xml" {
            None
        } else {
            self.custom_layout(&schema)?
        };
        // A report with columns and no layout at all: the rendering formats stop after OnInitReport with the error, which a used result
        // turns into false (report/lt-report-pre-rendering/ReportPreRenderingSaveAsPdf.al: P195_SaveAsPdf_PreRenderingOrder and its
        // Word, Excel and Html siblings, the call in a TryFunction).
        if matches!(format.as_str(), "Pdf" | "Html" | "Word" | "Excel") {
            if let Some(e) = self.no_valid_layout(&schema)? {
                if self.report_init(&inst)? {
                    return unsupported("CurrReport.Quit in OnInitReport of a report with no layout (not pinned)");
                }
                if result_used {
                    self.last_error = e;
                    return Ok(Value::Boolean(false));
                }
                return Err(Signal::Error(Box::new(e)));
            }
        }
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
                    ..Default::default()
                };
                return Ok(Value::Boolean(false));
            }
            // A layout of Type = Custom is rendered by whoever subscribes to the platform's CustomDocumentMergerEx; the
            // event is raised below, once the dataset is written (assumption custom-layout-merger).
            "Pdf" | "Word" | "Excel" | "Html" if custom.is_some() => {}
            "Html" if schema.report().layout == ir::ReportLayout::Rdlc => {
                self.last_error = al_builtins::native::AlError {
                    message: FORMAT_NOT_SUPPORTED_BY_LAYOUT.into(),
                    code: "Dialog".into(),
                    call_stack: String::new(),
                    ..Default::default()
                };
                return Ok(Value::Boolean(false));
            }
            "Pdf" | "Html" | "Word" | "Excel" if self.output_is_ignored(&schema) => discard = true,
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
                ..Default::default()
            };
            // The statement form raises the same error (handlers/TestReportSaveAsColumnDetailsContracts.al:
            // ProcessingOnlyReport_StatementFormRaises, handlers/PWReportSaveAsColumns.al:
            // ProcessingOnly_AnswerDropped_AsserterrorForm); with the answer used it is returned as false.
            if !result_used {
                return Err(Signal::Error(Box::new(error)));
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
            self.report.report_executed.retain(|w| w.strong_count() > 0);
            if self
                .report
                .report_executed
                .iter()
                .any(|w| w.upgrade().is_some_and(|i| Rc::ptr_eq(&i, &inst)))
            {
                return unsupported("a second SaveAs on one report variable (not pinned)");
            }
            self.report.report_executed.push(Rc::downgrade(&inst));
        }
        if let Some(rr) = record_ref {
            self.report_set_view(&inst, &schema, Some(rr))?;
        }
        let enters = self.report_enter_world(&schema, false)?;
        let outer = self.report.dataset.replace(Dataset::default());
        if let Err(e) = self.dataset_labels(&schema) {
            self.report.dataset = outer;
            return Err(e);
        }
        self.report.report_preview_pinned.push(false);
        let ran = self.report_in_world(enters, |s| s.report_body(&inst, &schema, false, Some(&parameters)));
        self.report.report_preview_pinned.pop();
        let written = std::mem::replace(&mut self.report.dataset, outer);
        match ran {
            Ok(true) => {
                let header = self.dataset_header(&schema)?;
                let bytes = written.expect("a SaveAs run has a dataset").finish(&header)?;
                if let Some(layout) = custom {
                    let SaveTarget::Stream(blob) = target else {
                        return unsupported(format!(
                            "{id} to a file of a report with a Custom layout (only a stream is handed to the merger)"
                        ));
                    };
                    return self.render_custom_layout(&schema, &format, layout, bytes, &blob);
                }
                match target {
                    SaveTarget::Stream(blob) if !discard => al_builtins::stream::write_bytes(&blob, &bytes),
                    SaveTarget::Stream(blob) => {
                        // The dataset is dropped; the buffer is marked and any look at it is refused.
                        blob.borrow_mut().render_pending = Some(schema.name().into());
                        al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::ReportRenderDiscarded);
                    }
                    SaveTarget::File(name) => {
                        self.render_pending_files
                            .insert(crate::ctx::pending_key(&name), schema.name().into());
                        al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::ReportRenderDiscarded);
                    }
                }
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
                        self.last_error = *e;
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

    /// The layout a rendered `SaveAs` of `schema` uses when it is of `Type = Custom`: the report's
    /// `DefaultRenderingLayout`, with its file's bytes. `None` for a report whose layout is not a custom one; a custom
    /// layout whose file the app does not hold is refused.
    fn custom_layout(&self, schema: &Schema) -> Result<Option<(ir::RenderingLayout, Vec<u8>)>, Signal> {
        let report = schema.report();
        let Some(default) = &report.default_rendering_layout else {
            return Ok(None);
        };
        let Some(layout) = report
            .rendering_layouts
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(default))
            .filter(|l| l.kind == "custom")
        else {
            return Ok(None);
        };
        let app = self.engine.program.objects[schema.object].app as usize;
        let bytes = self
            .engine
            .program
            .apps
            .get(app)
            .and_then(|a| a.layout_files.iter().find(|(p, _)| *p == layout.path))
            .map(|(_, b)| b.clone());
        match bytes {
            Some(b) => Ok(Some((layout.clone(), b))),
            None => unsupported(format!(
                "Report.SaveAs of \"{}\" with the custom layout {}, whose file {} the app does not hold",
                schema.name(),
                layout.name,
                layout.path
            )),
        }
    }

    /// Raises the platform's `CustomDocumentMergerEx` for a custom layout (assumption custom-layout-merger) and answers
    /// whether a subscriber rendered the document into `blob`.
    fn render_custom_layout(
        &mut self,
        schema: &Schema,
        format: &str,
        (layout, layout_bytes): (ir::RenderingLayout, Vec<u8>),
        dataset: Vec<u8>,
        blob: &al_builtins::stream::BlobRef,
    ) -> Result<Value, Signal> {
        use al_builtins::stream::{Encoding, InState};
        use al_builtins::value::LibraryValue;
        if !self
            .engine
            .events
            .is_subscribed(crate::events::REPORTING_TRIGGERS, "customdocumentmergerex")
        {
            return unsupported(format!(
                "Report.SaveAs of \"{}\" with the custom layout {}: no subscriber to CustomDocumentMergerEx (what BC does then is not pinned)",
                schema.name(),
                layout.name
            ));
        }
        al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::CustomLayoutMerger);
        let members: std::sync::Arc<[String]> = [
            "SaveAsPdf",
            "SaveAsWord",
            "SaveAsExcel",
            "Preview",
            "Print",
            "SaveAsHtml",
        ]
        .iter()
        .map(|m| m.to_string())
        .collect();
        let action = match format {
            "Pdf" => 0,
            "Word" => 1,
            "Excel" => 2,
            _ => 5,
        };
        let stream = |name: &str, v: LibraryValue| Value::Library(name.into(), Rc::new(RefCell::new(v)));
        let names: Vec<String> = [
            "ObjectID",
            "ReportAction",
            "ObjectPayload",
            "XmlData",
            "LayoutData",
            "DocumentStream",
            "Success",
        ]
        .iter()
        .map(|n| n.to_string())
        .collect();
        // The payload keys are not pinned (corpus topic #176): any use of the object is refused.
        let payload = al_builtins::json::unpinned_object(
            "the ObjectPayload of a custom layout merger is not pinned (corpus topic #176)",
        );
        let mut args = vec![
            Value::Integer(self.object_id_of(schema.object as u32) as i32),
            Value::Option(action, members),
            payload,
            stream(
                "InStream",
                LibraryValue::InStream(InState::snapshot(dataset, Encoding::Utf8)),
            ),
            stream(
                "InStream",
                LibraryValue::InStream(InState::snapshot(layout_bytes, Encoding::Utf8)),
            ),
            stream("OutStream", LibraryValue::OutStream(blob.clone(), Encoding::Utf8)),
            Value::Boolean(false),
        ];
        let call = crate::dispatch::EventCall {
            object: crate::events::REPORTING_TRIGGERS,
            event: "customdocumentmergerex",
            element: None,
            isolated: false,
            names: &names,
            sender: None,
        };
        self.raise(&call, &mut args)?;
        if matches!(args[6], Value::Boolean(true)) {
            Ok(Value::Boolean(true))
        } else {
            unsupported(format!(
                "Report.SaveAs of \"{}\" with the custom layout {}: no subscriber rendered it (what SaveAs does then is not pinned)",
                schema.name(),
                layout.name
            ))
        }
    }

    /// `RequestOptionsPage.Update(SaveRecord)` in a request page trigger. With `false` the controls show
    /// the variables as they are now, which they do on every read here, so nothing is left to do
    /// (handlers/TestReportRequestPageUpdateObjectIdContracts.al:RequestPageUpdate_RefreshesTheOtherControls:
    /// a handler sets Source to 21 and reads Derived 42 that the trigger set before it called Update(false)).
    /// `RequestOptionsPage.Caption := x` in the code of a report (Base Application's Financial Report). The request page of
    /// a run has no page object of its own to hold it, so the assignment is dropped; whether a request page handler
    /// reads it back is not modelled (assumption request-page-caption-set).
    pub(crate) fn request_page_caption(&mut self, args: &[Value]) -> Result<Value, Signal> {
        match args {
            [Value::Text(_) | Value::Code(_)] => {
                al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::RequestPageCaptionSet);
                Ok(Value::Null)
            }
            _ => unsupported("RequestPage.Caption read, or set with something that is not text"),
        }
    }

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
        self.report.dataset.as_mut().expect("a SaveAs run has a dataset").begin(
            &name,
            identity.as_ref().map(|(t, g)| (*t, g.as_str())),
            &columns,
            declares_children,
        );
        Ok(())
    }

    /// The Labels block of every data item, nested ones too, is written even when no row reaches the column
    /// (handlers/TestReportSaveAsColumnDetailsContracts.al:LabelsAreWrittenForAnEmptyDataItem,
    /// handlers/PWReportSaveAsColumns.al:Labels_NoRowAtAll, report/PTSaveAsEmpty.al:SaveAs_NoRowAtAll_ExactLines
    /// `ParentIdColCaption` then `ChildIdColCaption`).
    fn dataset_labels(&mut self, schema: &Schema) -> Result<(), Signal> {
        let engine = self.engine.clone();
        // The labels the report and its extensions declare come first, base report then extensions, in declaration
        // order (report/PZReportLabels.al: Labels_AreWrittenIntoTheDataset). Whether a column's caption label comes
        // before or after them is not pinned, so a report with both is refused.
        let mut declared: Vec<(String, String)> = schema.report().labels.clone();
        for &e in &schema.report().extensions {
            if let Some(x) = engine.program.objects[e].report_ext.as_ref() {
                declared.extend(x.labels.iter().cloned());
            }
        }
        let has_declared = !declared.is_empty();
        self.report
            .dataset
            .as_mut()
            .expect("a SaveAs run has a dataset")
            .labels
            .extend(declared);
        for item in schema.report().items.iter() {
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
                if has_declared {
                    return unsupported(
                        "a report with declared labels and a column with IncludeCaption (the order of the two kinds of label is not pinned)",
                    );
                }
                let ds = self.report.dataset.as_mut().expect("a SaveAs run has a dataset");
                if !ds.labels.contains(&label) {
                    ds.labels.push(label);
                }
            }
        }
        Ok(())
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
    /// undefined), `h:mm:ss` plus a space plus AM/PM for a Time, both for a DateTime, an Option and an Enum
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
            let ds = self.report.dataset.as_mut().expect("a SaveAs run has a dataset");
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
            company_name: self.cx.company.to_string(),
            company_id: id,
            user_name: self.host.identity.user_id(),
            now: (y, mo, d, (in_day / 3_600_000) as u32, (in_day / 60_000 % 60) as u32),
            company_block: self.has(al_version::Gate::ReportSaveAsXmlHeaderHasCompanyBlock),
        })
    }
}

#[cfg(test)]
mod ignored_output_tests {
    use super::*;

    /// ADR-040 section 3: the list grows by evidence only. Each entry names the app, the object, and the Microsoft tests
    /// that need the report to run; no entry is listed twice; a layout smoke test, a `CheckFileNotEmpty` test or an
    /// Excel cell test is never a reason.
    #[test]
    fn the_allow_list_cites_its_tests_and_has_no_duplicates() {
        assert_eq!(
            IGNORED_OUTPUT.iter().map(|e| e.name).collect::<Vec<_>>(),
            ["Post Inventory Cost to G/L", "Check"]
        );
        for (i, e) in IGNORED_OUTPUT.iter().enumerate() {
            assert!(e.app.len() == 36 && e.id > 0 && !e.name.is_empty(), "{}", e.name);
            assert!(
                e.why.contains('(') && e.why.contains("tests") == e.why.contains("tests"),
                "{}",
                e.name
            );
            assert!(
                IGNORED_OUTPUT[..i].iter().all(|o| (o.app, o.id) != (e.app, e.id)),
                "{} is listed twice",
                e.name
            );
            for banned in ["CheckFileNotEmpty", "VerifyCellValue", "Layout Test", "Report Layout"] {
                assert!(
                    !e.why.contains(banned) || e.why.contains("never"),
                    "{}: {banned}",
                    e.name
                );
            }
        }
    }
}
