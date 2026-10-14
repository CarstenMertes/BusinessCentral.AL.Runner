//! Report objects: `Run`, `RunModal`, `Execute`, `SetTableView`, `UseRequestPage`, `CurrReport.Quit`,
//! and the processing a run does (no request page, no layout, no output).
//!
//! Every rule is pinned by a corpus test, cited where it is applied. What no test pins is refused
//! (`Signal::Unsupported`) and listed in docs/probes-needed.md. In particular a run whose request
//! page would open is refused: request pages arrive with the page work.

use crate::pages::RequestClosed;
use crate::transactions::{TRANSACTION_STOPPED, TxModel, al_error};
use crate::{Session, Signal};
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
struct Schema {
    engine: Arc<crate::Engine>,
    object: usize,
}

impl Schema {
    fn report(&self) -> &ir::Report {
        self.engine.program.objects[self.object]
            .report
            .as_ref()
            .expect("checked by Session::report_schema")
    }
    fn name(&self) -> &str {
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
                let (inst, quit) = self.report_instance(receiver, true)?;
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
                    Some(other) => return unsupported(format!("{id} with RequestWindow {other:?}")),
                } && schema.report().use_request_page != Some(false);
                match args.get(2) {
                    None | Some(Value::Boolean(false)) => {}
                    Some(other) => {
                        return unsupported(format!("{id} with SystemPrinter {other:?} (printing is not supported)"));
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
                if self.report_executed.iter().any(|w| w.upgrade().is_some_and(|i| Rc::ptr_eq(&i, &inst))) {
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
                    Some(other) => return unsupported(format!("{id} seeded with {other:?}")),
                };
                // RunRequestPage always shows the page, so it always enters a transaction world
                // (record/TestTxModelReportExec.al:77-102, Test02 and Test03).
                let enters = self.report_enter_world(&schema, true)?;
                self.report_in_world(enters, |s| s.report_request_only(&inst, &schema, seed.as_deref()))
                    .map(Value::Text)
            }
            other => unsupported(format!("builtin {other}")),
        }
    }

    /// The report instance behind a receiver, initialised: the first use of a report variable
    /// runs `OnInitReport` (handlers/TestReportOnInitReportTiming.al:50-60).
    /// Returns whether that trigger quit the report; a caller that cannot stop refuses it.
    fn report_instance(&mut self, receiver: Option<Value>, can_stop: bool) -> Result<(Inst, bool), Signal> {
        match receiver {
            Some(Value::Codeunit(rc)) => {
                let quit = !rc.borrow().initialized && self.report_init(&rc)?;
                if quit && !can_stop {
                    return unsupported(
                        "CurrReport.Quit in OnInitReport before a method that does not run the report (not pinned)",
                    );
                }
                Ok((rc, quit))
            }
            other => unsupported(format!("a Report method on {other:?}")),
        }
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
            other => return unsupported(format!("{id} with the parameters {other:?} (only '' is pinned)")),
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
        let on_report = matches!(self.engine.program.objects[object].kind.as_str(), "report" | "reportextension");
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
    }

    // ------------------------------------------------------------------ table views

    fn base_item_record(&self, inst: &Inst, item: &ir::ReportItem) -> Result<Rec, Signal> {
        match item.slot.map(|s| inst.borrow().globals[s].clone()) {
            Some(Value::Record(rc)) => Ok(rc),
            other => unsupported(format!("dataitem {} holds {other:?}", item.name)),
        }
    }

    /// `SetTableView` and the record parameter of the static `Run`: the filters of the given record
    /// go to the one data item over its table
    /// (handlers/TestReportSetTableView.al:72-86, handlers/TestReportGetFilterInTriggers.al:58-89).
    fn report_set_view(&mut self, inst: &Inst, schema: &Schema, view: Option<&Value>) -> Result<(), Signal> {
        let Some(Value::Record(view)) = view else {
            return unsupported(format!("a table view of {view:?}"));
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
        // (handlers/TestReportRuntimeContracts.al:393-410 "Probe Rep Order"); how it combines with
        // a DataItemTableView that filters is not pinned.
        if item.view.as_ref().is_some_and(|v| v.to_ascii_lowercase().contains("where")) {
            return unsupported(format!(
                "a table view for data item {}, which declares a DataItemTableView (not pinned)",
                item.name
            ));
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
            return Err(al_error(TRANSACTION_STOPPED));
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
        self.report_in_world(enters, |s| s.report_body(inst, schema, shows_page))
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
        let ext = run.ext.as_ref().map(|(i, e)| (*e, i.clone()));
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
        }
    }

    /// The instances a run executes (the report, and the reportextension that changes it) with
    /// every data item's own view in force. The views are in force before the request page opens
    /// (handlers/TestReportMultiLineDataItemView.al: "the request page applies those views to the
    /// data items before the handler runs") and before OnPreReport, which may refine them
    /// (handlers/TestReportGetFilterInTriggers.al:43-52 reads the view there).
    fn report_prepare(&mut self, inst: &Inst, schema: &Schema) -> Result<Run, Signal> {
        let report = schema.report();
        // A reportextension that changes the run gets an instance of its own for this run, whose
        // names for the base items point at the report's own records.
        let ext = match report.extension {
            None => None,
            Some(e) => {
                let ext_inst = self.instance(e);
                let eir = self.engine.program.objects[e]
                    .report_ext
                    .as_ref()
                    .expect("a report names only extensions that were lowered");
                for &(base_item, slot) in &eir.aliases {
                    let base_slot = report.items[base_item].slot.expect("an alias has a record");
                    let record = inst.borrow().globals[base_slot].clone();
                    ext_inst.borrow_mut().globals[slot] = record;
                }
                Some((ext_inst, e))
            }
        };
        let run = Run {
            engine: self.engine.clone(),
            object: schema.object,
            base: inst.clone(),
            ext,
        };
        for r in run.all_items() {
            let Some(view) = run.item(r).view.clone() else { continue };
            let rec = self.report_item_record(&run, r)?;
            let t = self.table(&rec)?;
            // A view that only sorts leaves the filters a SetTableView gave the item in force
            // (handlers/TestReportRuntimeContracts.al:393-410).
            let kept = (!view.to_ascii_lowercase().contains("where")).then(|| rec.borrow().filters.clone());
            self.set_view(&rec, &t, &view)?;
            if let Some(filters) = kept {
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
    fn report_body(&mut self, inst: &Inst, schema: &Schema, shows_page: bool) -> Result<(), Signal> {
        let report = schema.report();
        let run = self.report_prepare(inst, schema)?;
        // The request page opens once the items' views are in force; what the handler filters is
        // what the run reads, so the views are not applied again afterwards. A cancelled page ends
        // the run without an error (handlers/TestReportRunWithRequestPage.al:
        // Report_Run_CancelledRequestPageRunsNothingAndRaisesNoError).
        if shows_page {
            let ext = run.ext.as_ref().map(|(i, e)| (*e, i.clone()));
            if self.run_request_page(inst, schema.object, None, ext, false)?.0 == RequestClosed::Cancel {
                return Ok(());
            }
        }
        // A report that renders output runs up to its request page and no further: there is no
        // renderer (docs/probes-needed.md).
        if let Some(why) = report.output.first() {
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
            let rec = self.report_item_record(&run, r)?;
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
            let rec = self.report_item_record(&run, r)?;
            let filters = rec.borrow().filters.clone();
            kept.push((rec, filters));
        }
        if let Some(r) = report.on_pre {
            if self.report_trigger_call(run.object, r, run.base.clone(), TrigKind::Pre)? == Some(Flow::Quit) {
                return Ok(());
            }
        }
        if let Some((ext_inst, e)) = &run.ext {
            if let Some(r) = run.ext_ir().and_then(|x| x.on_pre) {
                if self.report_trigger_call(*e, r, ext_inst.clone(), TrigKind::Pre)? == Some(Flow::Quit) {
                    return Ok(());
                }
            }
        }
        for (rec, filters) in kept {
            rec.borrow_mut().filters = filters;
        }
        for i in 0..report.items.len() {
            if report.items[i].parent.is_none() && self.report_item(&run, ItemRef::Base(i), None)? {
                return Ok(());
            }
        }
        if let Some(r) = report.on_post {
            self.call(run.object, r, run.base.clone(), Vec::new())?;
        }
        if let Some((ext_inst, e)) = &run.ext {
            if let Some(r) = run.ext_ir().and_then(|x| x.on_post) {
                self.call(*e, r, ext_inst.clone(), Vec::new())?;
            }
        }
        Ok(())
    }

    fn report_item_record(&self, run: &Run, r: ItemRef) -> Result<Rec, Signal> {
        let item = run.item(r);
        let (owner, _) = run.owner(r);
        match item.slot.map(|s| owner.borrow().globals[s].clone()) {
            Some(Value::Record(rc)) => Ok(rc),
            other => unsupported(format!("dataitem {} holds {other:?}", item.name)),
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
    fn report_item(&mut self, run: &Run, r: ItemRef, parent_rec: Option<Rec>) -> Result<bool, Signal> {
        let item = run.item(r);
        let (owner, object) = run.owner(r);
        let rec = self.report_item_record(run, r)?;
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
        if let Some(t) = item.on_pre {
            self.call(object, t, owner.clone(), Vec::new())?;
        }
        let modify = match r {
            ItemRef::Base(i) => run.ext_ir().and_then(|x| x.modifies.iter().find(|m| m.item == i)),
            ItemRef::Added(_) => None,
        };
        self.ret_used = true;
        let mut more = matches!(self.record_op("FindSet", &rec, &mut [])?, Value::Boolean(true));
        'rows: while more {
            if let (Some(t), Some((ext_inst, e))) = (modify.and_then(|m| m.before), &run.ext) {
                self.call(*e, t, ext_inst.clone(), Vec::new())?;
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
                if let (Some(t), Some((ext_inst, e))) = (modify.and_then(|m| m.after), &run.ext) {
                    self.call(*e, t, ext_inst.clone(), Vec::new())?;
                }
                for child in run.children(r) {
                    if self.report_item(run, child, Some(rec.clone()))? {
                        return Ok(true);
                    }
                }
            }
            self.ret_used = true;
            more = !matches!(self.record_op("Next", &rec, &mut [])?, Value::Integer(0));
        }
        if let Some(t) = item.on_post {
            self.call(object, t, owner.clone(), Vec::new())?;
        }
        Ok(false)
    }
}

/// A data item of the running report: one of its own, or one its reportextension adds.
#[derive(Clone, Copy)]
enum ItemRef {
    Base(usize),
    Added(usize),
}

/// The instances a run executes: the report, and the reportextension that changes it (if any).
struct Run {
    engine: Arc<crate::Engine>,
    /// The report's object index.
    object: usize,
    base: Inst,
    /// The extension's instance and object index.
    ext: Option<(Inst, usize)>,
}

impl Run {
    fn report(&self) -> &ir::Report {
        self.engine.program.objects[self.object]
            .report
            .as_ref()
            .expect("a run is of a report")
    }

    fn ext_ir(&self) -> Option<&ir::ReportExt> {
        self.ext
            .as_ref()
            .and_then(|(_, e)| self.engine.program.objects[*e].report_ext.as_ref())
    }

    fn item(&self, r: ItemRef) -> &ir::ReportItem {
        match r {
            ItemRef::Base(i) => &self.report().items[i],
            ItemRef::Added(i) => &self.ext_ir().expect("an added item has an extension").added[i].item,
        }
    }

    /// The instance that owns the item's record and triggers, and its object index.
    fn owner(&self, r: ItemRef) -> (&Inst, usize) {
        match (r, &self.ext) {
            (ItemRef::Added(_), Some((inst, e))) => (inst, *e),
            _ => (&self.base, self.object),
        }
    }

    fn all_items(&self) -> Vec<ItemRef> {
        let added = self.ext_ir().map_or(0, |x| x.added.len());
        (0..self.report().items.len())
            .map(ItemRef::Base)
            .chain((0..added).map(ItemRef::Added))
            .collect()
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
        if let Some(x) = self.ext_ir() {
            out.extend(
                (0..x.added.len())
                    .filter(|&c| match r {
                        ItemRef::Base(i) => x.added[c].base_parent == Some(i),
                        ItemRef::Added(k) => x.added[c].item.parent == Some(k),
                    })
                    .map(ItemRef::Added),
            );
        }
        out
    }
}
