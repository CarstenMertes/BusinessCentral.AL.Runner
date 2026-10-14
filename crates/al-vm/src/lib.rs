//! Tree-walking interpreter over the typed IR (ADR-011). `Engine` is the
//! immutable program (ADR-001); `Session` holds all mutable state.

use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::value::{Instance, RecordState, RtType, Value};
use al_ir as ir;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;
use std::sync::Arc;

mod autoinc;
mod baseline;
pub use baseline::Baseline;
pub use faultsite::FaultFrame;
mod callstack;
mod dispatch;
pub mod dotnet;
mod errorcollect;
pub mod events;
mod faultsite;
mod filterpage;
mod flowfields;
mod globaltriggers;
mod hostvalues;
mod interfaces;
mod pages;
mod partial;
mod platform;
mod queries;
mod recmeta;
mod records;
mod refs;
mod relations;
mod rename;
mod reports;
mod sessionops;
mod transactions;
mod ui;
mod views;
mod xmlports;
pub use transactions::TxModel;
pub use ui::{UiHandler, UiKind};

pub struct Engine {
    pub program: ir::Program,
    /// Per object, per routine: why it cannot run, if it cannot.
    pub blocked: Vec<Vec<Option<String>>>,
    /// Store schema per table object.
    pub tables: Vec<Option<al_data::TableDef>>,
    /// Event publishers and their subscribers, resolved by name.
    pub events: events::Registry,
    /// The BC version this program runs as; `None` gets the newest behaviour (ADR-015 3.4).
    pub target: Option<al_version::Target>,
    /// The .NET host, the registry of runtime-provided classes and the policy (ADR-014).
    pub dotnet: dotnet::DotNetConfig,
    /// Table objects whose rows the install baseline should have created and did not (ADR-013
    /// decision 5): a `Get` that finds nothing in dependency code is not BC's answer for them.
    pub unseeded: std::collections::HashSet<usize>,
    /// (table object, field position) -> the error BC raises when it evaluates the field's
    /// `InitValue` (`0DT` on a DateTime, a closing-date literal on a Date).
    pub unevaluable_init: std::collections::HashMap<(u32, usize), String>,
}

impl Engine {
    pub fn new(program: ir::Program, blocked: Vec<Vec<Option<String>>>) -> Arc<Engine> {
        Engine::with_config(program, blocked, None, dotnet::DotNetConfig::default())
    }

    /// An engine that runs as `target`: version-gated behaviour follows it (ADR-015 3.4).
    pub fn with_target(
        program: ir::Program,
        blocked: Vec<Vec<Option<String>>>,
        target: Option<al_version::Target>,
    ) -> Arc<Engine> {
        Engine::with_config(program, blocked, target, dotnet::DotNetConfig::default())
    }

    /// An engine whose `DotNet` calls are served by `dotnet` (ADR-014). `new` uses no .NET.
    pub fn with_dotnet(
        program: ir::Program,
        blocked: Vec<Vec<Option<String>>>,
        dotnet: dotnet::DotNetConfig,
    ) -> Arc<Engine> {
        Engine::with_config(program, blocked, None, dotnet)
    }

    /// Target and .NET host together.
    pub fn with_config(
        program: ir::Program,
        blocked: Vec<Vec<Option<String>>>,
        target: Option<al_version::Target>,
        dotnet: dotnet::DotNetConfig,
    ) -> Arc<Engine> {
        let tables = program
            .objects
            .iter()
            .enumerate()
            .map(|(i, o)| {
                o.table.as_ref().map(|t| al_data::TableDef {
                    object: i as u32,
                    name: o.name.clone(),
                    fields: t
                        .fields
                        .iter()
                        .map(|f| al_data::FieldDef {
                            id: f.id,
                            name: f.name.clone(),
                            ty: f.ty.clone(),
                            stored: f.stored,
                        })
                        .collect(),
                    keys: t.keys.clone(),
                    per_company: t.per_company,
                })
            })
            .collect();
        let events = events::Registry::build(&program);
        let mut unevaluable_init = std::collections::HashMap::new();
        for (i, o) in program.objects.iter().enumerate() {
            for (pos, f) in o.table.iter().flat_map(|t| t.fields.iter().enumerate()) {
                if let Some(message) = records::unevaluable_init_value(f) {
                    unevaluable_init.insert((i as u32, pos), message);
                }
            }
        }
        Arc::new(Engine {
            program,
            blocked,
            tables,
            events,
            target,
            dotnet,
            unseeded: Default::default(),
            unevaluable_init,
        })
    }
}

pub struct Session {
    pub engine: Arc<Engine>,
    pub last_error: AlError,
    depth: usize,
    /// Page handler functions running right now (`[PageHandler]`, `[ModalPageHandler]`).
    pub(crate) page_handler_depth: usize,
    /// `CodeCoverageLog` is recording.
    pub(crate) code_coverage: bool,
    /// The objects of the routines on the call stack, innermost last (builtins scoped to an app,
    /// `NavApp.GetCallerModuleInfo`).
    pub(crate) frames: Vec<usize>,
    /// The routine and current statement position of every frame, parallel to `frames`
    /// (`callstack.rs`).
    pub(crate) call_stack: Vec<CallFrame>,
    /// `faultsite.rs`: whether to record where errors and refusals leave from, and the last one.
    fault_trace: bool,
    fault_site: Vec<faultsite::FaultFrame>,
    /// `IsolatedStorage` by (scope, key): the value and whether it was stored encrypted.
    pub(crate) isolated_storage: std::collections::HashMap<(String, String), (String, bool)>,
    /// An install codeunit is running.
    install_scope: bool,
    /// Registered table connections by (type member, lower-case name), and the default name per type.
    pub(crate) table_connections: std::collections::HashSet<(i32, String)>,
    pub(crate) default_table_connection: std::collections::HashMap<i32, String>,
    /// Number sequences by (company, name); a global sequence has an empty company.
    pub(crate) sequences: std::collections::HashMap<(String, String), sessionops::Sequence>,
    /// The one instance of each `SingleInstance = true` codeunit in this session.
    singletons: std::collections::HashMap<usize, Rc<RefCell<Instance>>>,
    pub(crate) store: al_data::memory::MemStore,
    /// What the subscribers of `Global Triggers` answered, per table object (globaltriggers.rs).
    pub(crate) global_setups: std::collections::HashMap<usize, globaltriggers::GlobalSetup>,
    /// Private tables of temporary records, by temp id.
    pub(crate) temps: std::collections::HashMap<u64, al_data::memory::MemStore>,
    next_temp: u64,
    pub(crate) next_guid: u64,
    pub(crate) next_row_version: i64,
    /// Last AutoIncrement value handed out per table (and company). It is not part of the
    /// store: a rollback undoes rows but not the counter (corpus
    /// record/TestAutoIncrementExplicitKey.al:154).
    pub(crate) autoinc: std::collections::HashMap<(String, u32), i64>,
    /// The counter each table had before the first Truncate of the open transaction; a
    /// rollback puts it back (corpus record/TestAutoIncrementEdges.al:36-170).
    pub(crate) truncate_undo: std::collections::HashMap<(String, u32), i64>,
    /// Records inside an OnValidate (by handle) with the field values from before the outermost
    /// Validate, which nested Validates of the same record see as xRec (corpus
    /// record/TestXRecContracts.al:133).
    pub(crate) validating: Vec<(usize, Vec<Value>)>,
    /// Try functions on the call stack (Truncate is refused inside them).
    pub(crate) try_depth: usize,
    pub(crate) cx: al_data::AccessContext,
    /// Whether the builtin being called has its result used (ADR-011 `ret_used`).
    pub(crate) ret_used: bool,
    /// Manual event subscriptions made by `BindSubscription`.
    pub(crate) bindings: Vec<dispatch::Binding>,
    /// The parameters of the routine that just failed with an error, as it left them:
    /// the caller writes its `var` arguments back before it handles the error.
    pub(crate) error_finals: Option<Vec<Value>>,
    /// The highest rowversion handed out when the last `Commit` ran: rows at or below it are durable.
    pub(crate) committed_row_version: i64,
    /// Handler functions bound to the running test method (None outside one).
    pub(crate) ui: Option<ui::UiScope>,
    /// The TestPage variable whose `Trap()` waits for the next plain `Page.Run` (ADR-012).
    /// The variable-bound control values each SaveValues page stored when it closed.
    pub(crate) saved_values: std::collections::HashMap<u32, Vec<(usize, Value)>>,
    pub(crate) trap: Option<Rc<RefCell<al_builtins::value::PageState>>>,
    /// A page background task worker is running: its parameters and the result it set.
    pub(crate) bg_worker: Option<pages::BgWorker>,
    /// The xRec fields the next `Modify` hands the table's triggers instead of the stored row: a
    /// page saving a row passes its own xRec (corpus
    /// handlers/TestPageActionRunObjectPageRec_Tests.al:HostSaveAfterTheTargetEditedTheRowCarriesXRec).
    pub(crate) page_xrec: Option<Vec<Value>>,
    /// A duplicate-key Insert into a table without a Blob field does not fail where it is
    /// called: the table (object index) and the error come out at the next access to the table
    /// or at Commit (corpus record/TestRecordRuntimeTextsAndViews.al:
    /// Record_Insert_DuplicateKey_WithoutBlobField_IsDeferred_AndSurfacesAtTheNextAccess).
    pub(crate) deferred_insert: Option<(usize, AlError)>,
    /// Set while the page client or an XmlPort import writes a record: their duplicate-key
    /// errors are raised at once (their own tests pin it), not deferred like an AL `Insert`.
    pub(crate) immediate_errors: bool,
    /// The transaction model of the running test method (set by the test runner).
    pub tx_model: TxModel,
    /// Nesting depth of constructs that begin a transaction of their own (`Codeunit.Run`,
    /// a TestPage operation); a `TransactionModel::None` test may write inside them.
    pub(crate) tx_world: usize,
    /// The `[CommitBehavior]` of the innermost procedure that declares one.
    pub(crate) commit_behavior: transactions::CommitBehavior,
    /// Whether a test codeunit is executing (a test codeunit cannot be run from one).
    pub in_test_codeunit: bool,
    /// How many times the session made its changes durable (`commit_transaction`). A runner reads
    /// it around a test method to see whether the method committed (test isolation, `isolation.rs`
    /// in al-test-cli).
    pub commits: u64,
    /// The `[ErrorBehavior(Collect)]` scopes on the call stack and the errors they hold
    /// (see `errorcollect.rs`).
    pub(crate) error_scopes: errorcollect::Scopes,
    /// Records (by handle) whose field a TestPage write is validating, with the field's number:
    /// what `CurrFieldNo` reads in the field's OnValidate.
    pub(crate) curr_field_no: Vec<(usize, i32)>,
    /// `Database.LockTimeout`: whether a lock wait ends in a timeout error.
    pub(crate) lock_timeout: bool,
    /// `Database.CurrentTransactionType`: a member index of the TransactionType option.
    pub(crate) transaction_type: i32,
    /// Imported media objects and media sets (the platform tables behind Media fields).
    pub(crate) media: al_builtins::media::MediaStore,
    /// The host session behind `DotNet` values; ends with the session (ADR-014 section 6).
    pub(crate) dotnet: dotnet::DotNetState,
    /// The session's application areas as `ApplicationArea()` reads them: `#Basic,#Suite`, or the
    /// empty text, which enables every area. The test runner empties it before each test method
    /// (corpus session/TestSessionApplicationAreaTestBoundary.al:67552).
    pub application_areas: String,
    /// The clock and the identity (ADR-006).
    pub(crate) host: al_host::Host,
    /// The time `Sleep` has waited, added to every clock reading.
    pub(crate) waited_ms: i64,
    /// Temporary files created so far (`File.CreateTempFile` names them in order).
    pub(crate) temp_files: u32,
    /// The instant of the last system stamp (milliseconds since the Unix epoch).
    pub(crate) last_stamp: i64,
    /// The date `WorkDate(Date)` set; today until then.
    pub(crate) work_date: Option<i32>,
    /// The language `GlobalLanguage(Id)` set.
    pub(crate) global_language: i32,
    /// The call depth of the `OnInitReport` trigger being run, and whether it called
    /// `CurrReport.Quit` (al-vm reports.rs).
    pub(crate) report_init_depth: Option<usize>,
    pub(crate) report_quit: bool,
    /// The call depth of the report trigger being run and what it may ask of the report
    /// (`CurrReport.Skip`, `Break`, `Quit`), and the request it made (al-vm reports.rs).
    pub(crate) report_trigger: Option<(usize, crate::reports::TrigKind)>,
    pub(crate) report_flow: Option<crate::reports::Flow>,
    /// The report instances that `Execute` has run: a second `Execute` on one is refused.
    /// A page opened for a `RunObject` action looks its handler up without failing when there is
    /// none (al-vm pages/runobject.rs).
    pub(crate) lenient_bind: bool,
    pub(crate) report_executed: Vec<std::rc::Weak<RefCell<Instance>>>,
}

/// Nested AL calls allowed before the session gives up. The corpus pins that a chain of 990 calls
/// completes and one of 5000 is refused (codeunit/TestCodeunitRecursionDepth.al); BC's exact limit
/// and the text of its error are not pinned, so exceeding this is Unsupported.
pub const MAX_CALL_DEPTH: usize = 1000;

impl Session {
    /// A session on the deterministic test host: a frozen clock and AL Runner's identity.
    pub fn new(engine: Arc<Engine>) -> Session {
        Session::with_host(engine, al_host::Host::default())
    }

    /// The files AL code wrote through the `File` type, for a test or a front end to inspect.
    pub fn files(&self) -> &dyn al_host::FileSystem {
        &*self.host.files
    }

    /// A session whose clock, identity and file system come from `host`.
    pub fn with_host(engine: Arc<Engine>, host: al_host::Host) -> Session {
        let (company, user) = (host.identity.company_name(), host.identity.user_id());
        let ident = platform::Ident::of(&host);
        let store =
            al_data::memory::MemStore::with_virtual_tables(Rc::new(platform::Providers::new(&engine, ident.clone())));
        let mut session = Session {
            global_language: host.identity.global_language(),
            host,
            waited_ms: 0,
            temp_files: 0,
            last_stamp: i64::MIN / 2,
            work_date: None,
            engine,
            last_error: AlError::default(),
            depth: 0,
            singletons: Default::default(),
            store,
            global_setups: Default::default(),
            temps: Default::default(),
            next_temp: 0,
            next_guid: 0,
            next_row_version: 0,
            autoinc: Default::default(),
            truncate_undo: Default::default(),
            validating: Vec::new(),
            try_depth: 0,
            cx: al_data::AccessContext { company, user },
            ret_used: true,
            bindings: Vec::new(),
            error_finals: None,
            committed_row_version: 0,
            page_handler_depth: 0,
            code_coverage: false,
            frames: Vec::new(),
            call_stack: Vec::new(),
            fault_trace: false,
            fault_site: Vec::new(),
            isolated_storage: Default::default(),
            install_scope: false,
            sequences: Default::default(),
            table_connections: Default::default(),
            default_table_connection: Default::default(),
            ui: None,
            saved_values: std::collections::HashMap::new(),
            trap: None,
            bg_worker: None,
            page_xrec: None,
            deferred_insert: None,
            immediate_errors: false,
            tx_model: TxModel::default(),
            tx_world: 0,
            commit_behavior: Default::default(),
            in_test_codeunit: false,
            commits: 0,
            error_scopes: Default::default(),
            curr_field_no: Vec::new(),
            lock_timeout: true,
            transaction_type: 1,
            media: Default::default(),
            dotnet: Default::default(),
            application_areas: String::new(),
            report_init_depth: None,
            report_quit: false,
            report_trigger: None,
            report_flow: None,
            report_executed: Vec::new(),
            lenient_bind: false,
        };
        session.seed_platform_rows(&ident);
        session
    }

    /// Makes every change so far durable (and a Truncate of this transaction final).
    pub fn commit_transaction(&mut self) {
        self.deferred_insert = None;
        al_data::Store::commit(&mut self.store);
        self.commits += 1;
        self.truncate_undo.clear();
        // Rows at or below this rowversion are durable: a buffer older than one of them is stale.
        self.committed_row_version = self.next_row_version;
    }

    /// Discards every change since the last commit. The AutoIncrement counters are not
    /// part of the data and stay (corpus record/TestAutoIncrementExplicitKey.al:154), except
    /// those a Truncate of this transaction changed.
    pub fn rollback_transaction(&mut self) {
        self.deferred_insert = None;
        al_data::Store::rollback(&mut self.store);
        for (slot, value) in std::mem::take(&mut self.truncate_undo) {
            self.autoinc.insert(slot, value);
        }
    }

    /// The session's database, for runners that manage transactions around tests.
    pub fn store_mut(&mut self) -> &mut al_data::memory::MemStore {
        &mut self.store
    }

    /// The initial value of a slot of type `t`.
    pub fn default_value(&mut self, t: &RtType) -> Value {
        match t {
            RtType::Record(table, temporary) => {
                // A table declared `TableType = Temporary` is temporary for every variable
                // (corpus record 60198 TempOnly_IsTemporary_AlwaysTrue).
                let temporary = &(*temporary
                    || self.engine.program.objects[*table as usize]
                        .properties
                        .iter()
                        .any(|(k, v)| {
                            k.eq_ignore_ascii_case("TableType") && v.trim().eq_ignore_ascii_case("Temporary")
                        }));
                let temp_id = temporary.then(|| {
                    self.next_temp += 1;
                    self.next_temp
                });
                Value::Record(Rc::new(RefCell::new(RecordState {
                    table: *table,
                    temporary: *temporary,
                    temp_id,
                    fields: self.initial_fields(*table as usize),
                    filters: Vec::new(),
                    filter_group: 0,
                    marks: Vec::new(),
                    marked_only: false,
                    read_isolation: 0,
                    // A record in a test run is Validated (corpus record/TestSecurityFilteringContracts.al:161-175).
                    security_filtering: 2,
                    key: 0,
                    ascending: true,
                    field_descending: false,
                    position: None,
                    auto_calc: Vec::new(),
                    load: Default::default(),
                    touched: false,
                })))
            }
            other => Value::default_for(other),
        }
    }

    /// A fresh instance of codeunit `object` with default globals.
    /// What `ModuleInfo` reports for app `index` of the program, from its manifest.
    fn module_of_app(&self, index: usize) -> Option<al_builtins::value::ModuleInfoState> {
        let app = self.engine.program.apps.get(index)?;
        let version = {
            let parts: Vec<Option<i32>> = app.version.split('.').map(|p| p.parse().ok()).collect();
            match parts.as_slice() {
                [Some(a), Some(b), Some(c), Some(d)] => Some([*a, *b, *c, *d]),
                _ => None,
            }
        };
        Some(al_builtins::value::ModuleInfoState {
            id: ops::parse_guid(&app.id)?,
            name: app.name.clone(),
            publisher: app.publisher.clone(),
            version,
            dependencies: app.dependencies.iter().filter_map(|d| ops::parse_guid(d)).collect(),
        })
    }

    /// The runner marks the time install codeunits run: `DataTransfer` is usable in it.
    pub fn set_install_scope(&mut self, on: bool) {
        self.install_scope = on;
    }

    pub fn instance(&mut self, object: usize) -> Rc<RefCell<Instance>> {
        let tys: Vec<RtType> = self.engine.program.objects[object]
            .globals
            .iter()
            .map(|g| g.ty.clone())
            .collect();
        let globals = tys.iter().map(|t| self.default_value(t)).collect();
        Rc::new(RefCell::new(Instance {
            object: object as u32,
            globals,
            initialized: true,
        }))
    }

    /// A record passed by value is a copy (ADR-010). A temporary record's copy
    /// gets an independent, empty table of its own (corpus
    /// record/TestTempTableContracts.al:TempVar_PassedByValue_CallerNotAffected).
    pub(crate) fn record_by_value(&mut self, rc: &Rc<RefCell<RecordState>>) -> Value {
        let mut st = rc.borrow().clone();
        if st.temporary {
            self.next_temp += 1;
            st.temp_id = Some(self.next_temp);
        }
        Value::Record(Rc::new(RefCell::new(st)))
    }

    pub(crate) fn ensure_initialized(&mut self, inst: &Rc<RefCell<Instance>>) {
        if inst.borrow().initialized {
            return;
        }
        let object = inst.borrow().object as usize;
        let fresh = self.instance(object);
        let globals = std::mem::take(&mut fresh.borrow_mut().globals);
        let mut i = inst.borrow_mut();
        i.globals = globals;
        i.initialized = true;
    }

    /// Calls routine `routine` of `object` on `instance`. Returns the return
    /// value and the final values of all parameters (for `var` write-back).
    pub fn call(
        &mut self,
        object: usize,
        routine: usize,
        instance: Rc<RefCell<Instance>>,
        args: Vec<Value>,
    ) -> Result<(Value, Vec<Value>), Signal> {
        if let Some(Some(reason)) = self.engine.blocked.get(object).and_then(|o| o.get(routine)) {
            return Err(Signal::Unsupported(reason.clone()));
        }
        if self.depth >= MAX_CALL_DEPTH {
            return Err(Signal::Unsupported(format!(
                "call depth over {MAX_CALL_DEPTH} (BC's limit and error text are not pinned)"
            )));
        }
        self.depth += 1;
        // The builtin being run (a record write that runs triggers and events) keeps its own
        // `ret_used`: the callee's builtins must not change it.
        let ret_used = self.ret_used;
        let r = if self.engine.events.publishers.contains_key(&(object, routine)) {
            self.publish(object, routine, instance, args)
        } else {
            self.call_inner(object, routine, instance, args)
        };
        self.ret_used = ret_used;
        self.depth -= 1;
        r
    }

    fn call_inner(
        &mut self,
        object: usize,
        routine: usize,
        instance: Rc<RefCell<Instance>>,
        args: Vec<Value>,
    ) -> Result<(Value, Vec<Value>), Signal> {
        self.first_use(&instance)?;
        let engine = self.engine.clone();
        let r = &engine.program.objects[object].routines[routine];
        let mut locals: Vec<Value> = Vec::with_capacity(r.locals.len());
        for s in &r.locals {
            locals.push(self.default_value(&s.ty));
        }
        for (i, a) in args.into_iter().enumerate().take(r.params) {
            locals[i] = match (a, r.by_var.get(i).copied().unwrap_or(false)) {
                (Value::Record(rc), false) => self.record_by_value(&rc),
                (a, _) => ops::convert(a, &r.locals[i].ty)?,
            };
        }
        let mut frame = Frame {
            object,
            routine,
            instance,
            locals,
        };
        let is_try = r.attributes.iter().any(|a| a.name.eq_ignore_ascii_case("TryFunction"));
        // [CommitBehavior] governs the whole dynamic extent of the procedure and ends when it
        // returns (error-handling/TestCommitBehaviorAttribute.al:CommitBehavior_Ignore_ScopeIsPoppedOnReturn).
        let saved_commit_behavior = self.commit_behavior;
        for a in r
            .attributes
            .iter()
            .filter(|a| a.name.eq_ignore_ascii_case("CommitBehavior"))
        {
            match transactions::CommitBehavior::from_attribute(&a.text) {
                Some(b) => self.commit_behavior = b,
                None => {
                    return Err(Signal::Unsupported(format!(
                        "{} (only Ignore and Error are pinned)",
                        a.text
                    )));
                }
            }
        }
        let collecting = errorcollect::declares_collect(&r.attributes)?;
        if collecting {
            self.error_scopes.open();
        }
        self.try_depth += usize::from(is_try);
        self.frames.push(object);
        self.call_stack.push(CallFrame {
            routine,
            pos: ir::NO_POS,
        });
        let mut outcome = self.exec_block(&r.body, &mut frame);
        if self.fault_trace && matches!(outcome, Err(Signal::Error(_) | Signal::Unsupported(_))) {
            self.note_fault();
        }
        // An error that leaves the routine takes the routine's frame into its call stack.
        if let (Err(Signal::Error(e)), Some(&top)) = (&mut outcome, self.call_stack.last()) {
            self.add_frame(e, object, top);
        }
        self.call_stack.pop();
        self.frames.pop();
        if collecting {
            outcome = self.close_error_scope(outcome);
        }
        self.try_depth -= usize::from(is_try);
        self.commit_behavior = saved_commit_behavior;
        self.end_trap_scope(&frame, r.params, &r.by_var);
        let ret = match outcome {
            Ok(()) => None,
            Err(Signal::Exit(v)) => v,
            Err(Signal::Break) => None,
            Err(e) => {
                // Variables set before the error stay set for the caller that catches it
                // (error-handling/TestTryFunctionContracts.al, codeunit/TestScopeIsolationContracts.al).
                if matches!(e, Signal::Error(_)) {
                    self.error_finals = Some(frame.locals[..r.params].to_vec());
                }
                return Err(e);
            }
        };
        let value = match (&r.ret, ret, r.ret_local) {
            (Some(t), Some(v), _) => ops::convert(v, t)?,
            (Some(_), None, Some(l)) => frame.locals[l].clone(),
            (Some(t), None, None) => Value::default_for(t),
            (None, _, _) => Value::Null,
        };
        frame.locals.truncate(r.params);
        Ok((value, frame.locals))
    }

    fn exec_block(&mut self, stmts: &[ir::Stmt], f: &mut Frame) -> Result<(), Signal> {
        for s in stmts {
            self.exec(s, f)?;
        }
        Ok(())
    }

    fn exec(&mut self, s: &ir::Stmt, f: &mut Frame) -> Result<(), Signal> {
        use ir::Stmt as S;
        match s {
            S::Assign(p, e) => {
                let v = self.eval(e, f)?;
                self.write(p, v, f)
            }
            S::Expr(e) => self.eval(e, f).map(|_| ()),
            S::If(c, t, e) => {
                if ops::truthy(&self.eval(c, f)?)? {
                    self.exec_block(t, f)
                } else {
                    self.exec_block(e, f)
                }
            }
            S::While(c, body) => {
                let here = self.call_stack.last().map(|t| t.pos);
                while ops::truthy(&self.eval(c, f)?)? {
                    match self.exec_block(body, f) {
                        Err(Signal::Break) => break,
                        r => r?,
                    }
                    // The condition belongs to the loop's line, not to the body's last statement.
                    self.restore_pos(here);
                }
                Ok(())
            }
            S::Repeat(body, until) => {
                let here = self.call_stack.last().map(|t| t.pos);
                loop {
                    match self.exec_block(body, f) {
                        Err(Signal::Break) => break,
                        r => r?,
                    }
                    self.restore_pos(here);
                    if ops::truthy(&self.eval(until, f)?)? {
                        break;
                    }
                }
                Ok(())
            }
            S::For {
                var,
                from,
                to,
                down,
                body,
            } => {
                let start = int(&self.eval(from, f)?)?;
                let end = int(&self.eval(to, f)?)?;
                let mut i = start;
                while if *down { i >= end } else { i <= end } {
                    let slot_ty = self.place_type(var, f);
                    self.write(var, ops::convert(Value::BigInteger(i), &slot_ty)?, f)?;
                    match self.exec_block(body, f) {
                        Err(Signal::Break) => break,
                        r => r?,
                    }
                    i = int(&self.read(var, f)?)?;
                    i += if *down { -1 } else { 1 };
                }
                Ok(())
            }
            S::Foreach { var, collection, body } => {
                let items: Vec<Value> = match self.eval(collection, f)? {
                    Value::List(l) => l.borrow().clone(),
                    other => return Err(Signal::Unsupported(format!("foreach over {other:?}"))),
                };
                for item in items {
                    self.write(var, item, f)?;
                    match self.exec_block(body, f) {
                        Err(Signal::Break) => break,
                        r => r?,
                    }
                }
                Ok(())
            }
            S::Case {
                scrutinee,
                branches,
                otherwise,
            } => {
                let v = self.eval(scrutinee, f)?;
                for (labels, body) in branches {
                    if self.matches_any(&v, labels, f)? {
                        return self.exec_block(body, f);
                    }
                }
                self.exec_block(otherwise, f)
            }
            S::Exit(e) => {
                let v = match e {
                    Some(e) => Some(self.eval(e, f)?),
                    None => None,
                };
                Err(Signal::Exit(v))
            }
            S::Break => Err(Signal::Break),
            S::Pos(p) => {
                if let Some(top) = self.call_stack.last_mut() {
                    top.pos = *p;
                }
                Ok(())
            }
            S::AssertError(body) => {
                // A caught error rolls the transaction back to the last commit, also inside a modal
                // page handler: the write pending before the page opened and the handler's own
                // earlier writes are lost with it (corpus branch al-runtime-rs, pending push:
                // record/TestValidateRaisesAndHandlerAsserterrorContracts.al:122-143,
                // Asserterror_InAModalHandler_RollsBackToTheLastCommit, probe on BC 27.5).
                match self.exec_block(body, f) {
                    Err(Signal::Error(mut e)) => {
                        // The caught error rolls the transaction back to the last commit
                        // (record/TestRecordRenameRollback.al).
                        self.rollback_transaction();
                        // The routine that catches it is the last frame of its call stack.
                        self.add_top_frame(&mut e);
                        self.last_error = e;
                        Ok(())
                    }
                    // Text and code: types/TestFormatEvaluateTextContracts.al:Asserterror_NothingRaised_ReportsAssertError.
                    Ok(()) => Err(Signal::Error(AlError {
                        message: "An error was expected inside an ASSERTERROR statement.".into(),
                        code: "AssertError".into(),
                        call_stack: String::new(),
                    })),
                    Err(other) => Err(other),
                }
            }
        }
    }

    fn restore_pos(&mut self, pos: Option<u32>) {
        if let (Some(p), Some(top)) = (pos, self.call_stack.last_mut()) {
            top.pos = p;
        }
    }

    fn matches_any(&mut self, v: &Value, labels: &[ir::SetElem], f: &mut Frame) -> Result<bool, Signal> {
        for l in labels {
            let hit = match l {
                ir::SetElem::One(e) => ops::compare(v, &self.eval(e, f)?)? == Ordering::Equal,
                ir::SetElem::Range(lo, hi) => {
                    let above = match lo {
                        Some(lo) => ops::compare(v, &self.eval(lo, f)?)? != Ordering::Less,
                        None => true,
                    };
                    let below = match hi {
                        Some(hi) => ops::compare(v, &self.eval(hi, f)?)? != Ordering::Greater,
                        None => true,
                    };
                    above && below
                }
            };
            if hit {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn eval(&mut self, e: &ir::Expr, f: &mut Frame) -> Result<Value, Signal> {
        use ir::Expr as E;
        match e {
            E::Const(c) => Ok(constant(c)),
            E::Load(p) => self.read(p, f),
            E::Convert(t, e) => {
                let v = self.eval(e, f)?;
                ops::convert(v, t)
            }
            E::Unary(op, e) => {
                let v = self.eval(e, f)?;
                match op {
                    ir::UnOp::Not => Ok(Value::Boolean(!ops::truthy(&v)?)),
                    ir::UnOp::Plus => Ok(v),
                    ir::UnOp::Neg => ops::arith(ops::Arith::Sub, &Value::Integer(0), &v).map(|r| match (&v, r) {
                        (Value::Decimal(_), r) => r,
                        (_, r) => r,
                    }),
                }
            }
            E::Binary(op, l, r) => {
                // AL evaluates both operands of and/or (no short circuit):
                // docs/probes-needed.md.
                let l = self.eval(l, f)?;
                let r = self.eval(r, f)?;
                use ir::BinOp as B;
                Ok(match op {
                    B::Add => ops::arith(ops::Arith::Add, &l, &r)?,
                    B::Sub => ops::arith(ops::Arith::Sub, &l, &r)?,
                    B::Mul => ops::arith(ops::Arith::Mul, &l, &r)?,
                    B::Div => ops::arith(ops::Arith::Div, &l, &r)?,
                    B::IntDiv => ops::arith(ops::Arith::IntDiv, &l, &r)?,
                    B::Mod => ops::arith(ops::Arith::Mod, &l, &r)?,
                    B::And => Value::Boolean(ops::truthy(&l)? & ops::truthy(&r)?),
                    B::Or => Value::Boolean(ops::truthy(&l)? | ops::truthy(&r)?),
                    B::Xor => Value::Boolean(ops::truthy(&l)? ^ ops::truthy(&r)?),
                    B::Eq => Value::Boolean(ops::compare(&l, &r)? == Ordering::Equal),
                    B::Ne => Value::Boolean(ops::compare(&l, &r)? != Ordering::Equal),
                    B::Lt => Value::Boolean(ops::compare_order(&l, &r)? == Ordering::Less),
                    B::Le => Value::Boolean(ops::compare_order(&l, &r)? != Ordering::Greater),
                    B::Gt => Value::Boolean(ops::compare_order(&l, &r)? == Ordering::Greater),
                    B::Ge => Value::Boolean(ops::compare_order(&l, &r)? != Ordering::Less),
                })
            }
            E::In(v, set) => {
                let v = self.eval(v, f)?;
                Ok(Value::Boolean(self.matches_any(&v, set, f)?))
            }
            E::Ternary(c, a, b) => {
                if ops::truthy(&self.eval(c, f)?)? {
                    self.eval(a, f)
                } else {
                    self.eval(b, f)
                }
            }
            E::Call {
                object,
                routine,
                instance,
                args,
                try_call,
            } => {
                let inst = match instance {
                    None => f.instance.clone(),
                    Some(e) => match self.eval(e, f)? {
                        Value::Codeunit(i) => self.shared_instance(i),
                        // A table procedure runs with the record as its Rec.
                        Value::Record(rc) => self.record_instance(*object, rc, None)?,
                        // The page a part shows (`CurrPage.<Part>.Page`), or a `Page` variable, which
                        // is constructed at first use.
                        Value::TestPage(st) => {
                            let i = self.construct_page(&st)?;
                            // A procedure a page extension declares runs on the extension's instance.
                            let ext = st
                                .borrow()
                                .ext_instances
                                .iter()
                                .find(|(e, _)| e == object)
                                .map(|(_, i)| i.clone());
                            self.shared_instance(ext.unwrap_or(i))
                        }
                        other => {
                            return Err(Signal::Unsupported(format!(
                                "a procedure call on a {} value",
                                value_kind(&other)
                            )));
                        }
                    },
                };
                self.finish_call(*object, *routine, inst, args, *try_call, f)
            }
            E::InterfaceCall {
                receiver,
                targets,
                args,
            } => {
                let (object, routine, inst) = self.interface_target(receiver, targets, f)?;
                self.finish_call(object, routine, inst, args, false, f)
            }
            E::InterfaceTest { value, objects, cast } => {
                let v = self.eval(value, f)?;
                self.interface_test(v, objects, *cast)
            }
            E::EnumImpl {
                value,
                by_ordinal,
                unknown,
            } => {
                let v = self.eval(value, f)?;
                self.enum_implementation(v, by_ordinal, *unknown)
            }
            E::Control { page, control } => {
                let p = self.eval(page, f)?;
                self.page_control(p, *control)
            }
            E::QueryColumn { query, column } => {
                let q = self.eval(query, f)?;
                self.query_column(q, *column)
            }
            E::DotNet { ty, receiver, op, args } => self.eval_dotnet(*ty, receiver.as_deref(), op, args, f),
            E::CharAt(text, index) => {
                let t = self.eval(text, f)?;
                let Some(t) = al_builtins::native::text_of(&t) else {
                    return Err(Signal::Unsupported(format!("indexing {t:?} as a text")));
                };
                let i = int(&self.eval(index, f)?)?;
                usize::try_from(i - 1)
                    .ok()
                    .and_then(|i| t.encode_utf16().nth(i))
                    .map(Value::Char)
                    .ok_or_else(|| {
                        Signal::Unsupported("a text indexed outside its length (error text not pinned)".into())
                    })
            }
            E::DotNetCoerce(t, e) => {
                let v = self.eval(e, f)?;
                self.dotnet_coerce(v, t)
            }
            E::Builtin {
                id,
                receiver,
                args,
                ret_used,
            } => {
                // Event subscriptions are session state: the VM runs them (dispatch.rs).
                if let "Session.BindSubscription#1" | "Session.UnbindSubscription#1" = id.as_str() {
                    let Some(ir::Arg::Value(e)) = args.first() else {
                        return Err(Signal::Unsupported(format!("{id} with this argument")));
                    };
                    let cu = self.eval(e, f)?;
                    return self.subscription(id.starts_with("Session.Bind"), cu, *ret_used);
                }
                // `CurrFieldNo` is a property of the implicit Rec, not a catalog method: the lowerer
                // emits this id with the Rec as receiver.
                if id == "Record.CurrFieldNo#1" {
                    let Some(ir::Arg::Ref(p)) = receiver.as_deref() else {
                        return Err(Signal::Unsupported("CurrFieldNo without a record".into()));
                    };
                    let Value::Record(rec) = self.read(p, f)? else {
                        return Err(Signal::Unsupported("CurrFieldNo of something that is no record".into()));
                    };
                    return Ok(Value::Integer(self.curr_field_of(&rec)));
                }
                // `ClearAll()` resets every global variable of the running object to its default;
                // locals are untouched (docs/probes-needed.md "ClearAll"). A codeunit has nothing
                // else in its globals; an object with an implicit Rec or page state is refused.
                if id == "System.ClearAll#1" {
                    let object = f.instance.borrow().object as usize;
                    let o = &self.engine.program.objects[object];
                    if o.kind != "codeunit" || o.rec_slot.is_some() {
                        return Err(Signal::Unsupported(
                            "ClearAll outside a codeunit without a source table (the implicit record is not pinned)"
                                .into(),
                        ));
                    }
                    let tys: Vec<RtType> = o.globals.iter().map(|g| g.ty.clone()).collect();
                    let fresh: Vec<Value> = tys.iter().map(|t| self.default_value(t)).collect();
                    f.instance.borrow_mut().globals = fresh;
                    return Ok(Value::Null);
                }
                if id == "Session.ApplicationArea#1" {
                    // `ApplicationArea()` reads the session's areas, `ApplicationArea(Text)` sets them
                    // (corpus pageapplicationarea/TestPageApplicationAreaControlRemoval.al).
                    return match args.first() {
                        None => Ok(Value::Text(self.application_areas.clone())),
                        Some(ir::Arg::Value(e)) => match self.eval(e, f)? {
                            Value::Text(t) | Value::Code(t) => {
                                self.application_areas = t;
                                Ok(Value::Null)
                            }
                            other => Err(Signal::Unsupported(format!("ApplicationArea({other:?})"))),
                        },
                        Some(_) => Err(Signal::Unsupported("ApplicationArea with this argument".into())),
                    };
                }
                if id.starts_with("Codeunit.Run#")
                    || id.starts_with("Xmlport.")
                    || id.starts_with("Report.")
                    || id.starts_with("FilterPageBuilder.")
                    || id == "Database.IsInWriteTransaction#1"
                {
                    let recv = match receiver.as_deref() {
                        Some(ir::Arg::Value(e)) => Some(self.eval(e, f)?),
                        Some(ir::Arg::Ref(p)) => Some(self.read(p, f)?),
                        None => None,
                    };
                    let mut values = Vec::with_capacity(args.len());
                    for a in args {
                        values.push(match a {
                            ir::Arg::Value(e) => self.eval(e, f)?,
                            ir::Arg::Ref(p) => self.read(p, f)?,
                        });
                    }
                    if id == "Database.IsInWriteTransaction#1" {
                        return Ok(Value::Boolean(self.in_write_transaction()));
                    }
                    if id.starts_with("Xmlport.") {
                        return self.xmlport_builtin(id, recv, &values, *ret_used);
                    }
                    if id.starts_with("Report.") {
                        return self.report_builtin(id, recv, &values, f.instance.clone());
                    }
                    if id.starts_with("FilterPageBuilder.") {
                        return self.filter_page_builder_op(id, recv, &values, *ret_used);
                    }
                    return self.codeunit_run(id, recv, &values, *ret_used);
                }
                let native =
                    al_builtins::native::lookup(id).ok_or_else(|| Signal::Unsupported(format!("builtin {id}")))?;
                let mut recv = match receiver.as_deref() {
                    Some(ir::Arg::Value(e)) => Some(self.eval(e, f)?),
                    Some(ir::Arg::Ref(p)) => Some(self.read(p, f)?),
                    None => None,
                };
                let mut values = Vec::with_capacity(args.len());
                for a in args {
                    values.push(match a {
                        ir::Arg::Value(e) => self.eval(e, f)?,
                        ir::Arg::Ref(p) => self.read(p, f)?,
                    });
                }
                self.ret_used = *ret_used;
                let ret = native(self, recv.as_mut(), &mut values)?;
                if let (Some(ir::Arg::Ref(p)), Some(v)) = (receiver.as_deref(), recv) {
                    self.write(p, v, f)?;
                }
                for (a, v) in args.iter().zip(values) {
                    if let ir::Arg::Ref(p) = a {
                        self.write(p, v, f)?;
                    }
                }
                Ok(ret)
            }
        }
    }

    /// What `CurrFieldNo` reads on `rec`: the field number a TestPage write is validating on this
    /// record, 0 anywhere else (handlers/TestPageCurrFieldNo_Tests.al:36-123).
    fn curr_field_of(&self, rec: &Rc<RefCell<RecordState>>) -> i32 {
        let handle = Rc::as_ptr(rec) as usize;
        self.curr_field_no
            .iter()
            .rev()
            .find(|(h, _)| *h == handle)
            .map_or(0, |(_, n)| *n)
    }

    /// Evaluates the arguments of a resolved call and runs it on `inst`, writing back the
    /// by-reference arguments.
    fn finish_call(
        &mut self,
        object: usize,
        routine: usize,
        inst: Rc<RefCell<Instance>>,
        args: &[ir::Arg],
        try_call: bool,
        f: &mut Frame,
    ) -> Result<Value, Signal> {
        let mut values = Vec::with_capacity(args.len());
        for a in args {
            values.push(match a {
                ir::Arg::Value(e) => self.eval(e, f)?,
                ir::Arg::Ref(p) => self.read(p, f)?,
            });
        }
        let result = self.call(object, routine, inst, values);
        if let Err(Signal::Error(_)) = &result {
            if let Some(finals) = self.error_finals.take() {
                for (a, v) in args.iter().zip(finals) {
                    if let ir::Arg::Ref(p) = a {
                        self.write(p, v, f)?;
                    }
                }
            }
        }
        let (ret, finals) = match (result, try_call) {
            (Err(Signal::Error(mut e)), true) => {
                self.add_top_frame(&mut e);
                self.last_error = e;
                return Ok(Value::Boolean(false));
            }
            (Ok((_, finals)), true) => (Value::Boolean(true), finals),
            (r, _) => r?,
        };
        for (a, v) in args.iter().zip(finals) {
            if let ir::Arg::Ref(p) = a {
                self.write(p, v, f)?;
            }
        }
        Ok(ret)
    }

    /// The instance a call on codeunit value `i` runs on: the session's
    /// singleton for `SingleInstance = true` codeunits, `i` itself otherwise.
    pub(crate) fn shared_instance(&mut self, i: Rc<RefCell<Instance>>) -> Rc<RefCell<Instance>> {
        let object = i.borrow().object as usize;
        let single = self.engine.program.objects[object]
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("SingleInstance") && v.trim().eq_ignore_ascii_case("true"));
        if !single {
            return i;
        }
        if !self.singletons.contains_key(&object) {
            let inst = self.instance(object);
            self.singletons.insert(object, inst);
        }
        self.singletons[&object].clone()
    }

    fn place_type(&self, p: &ir::Place, f: &Frame) -> RtType {
        let r = &self.engine.program.objects[f.object].routines[f.routine];
        match p {
            ir::Place::Local(i) => r.locals[*i].ty.clone(),
            ir::Place::Global(i) => {
                let o = f.instance.borrow().object as usize;
                self.engine.program.objects[o].globals[*i].ty.clone()
            }
            ir::Place::Index(base, _) => match self.place_type(base, f) {
                RtType::Array(_, el) => *el,
                other => other,
            },
            ir::Place::Field(base, pos) => match self.place_type(base, f) {
                RtType::Record(t, _) => self.engine.tables[t as usize]
                    .as_ref()
                    .map_or(RtType::Variant, |td| td.fields[*pos].ty.clone()),
                _ => RtType::Variant,
            },
        }
    }

    fn read(&mut self, p: &ir::Place, f: &mut Frame) -> Result<Value, Signal> {
        match p {
            ir::Place::Local(i) => Ok(f.locals[*i].clone()),
            ir::Place::Global(i) => Ok(f.instance.borrow().globals[*i].clone()),
            ir::Place::Index(base, idx) => {
                let (arr, flat) = self.element(base, idx, f)?;
                let v = arr.borrow()[flat].clone();
                Ok(v)
            }
            ir::Place::Field(base, pos) => match self.read(base, f)? {
                Value::Record(rc) => {
                    let r = rc.borrow();
                    if !self.engine.unevaluable_init.is_empty() && !r.touched {
                        // The buffer of a record nothing has written is built by this read, which
                        // evaluates the InitValues (record/TestDateTimeInitValue.Codeunit.al:
                        // DateTimeInitValue_Table_UntouchedRecordRaisesOnReadNotOnInit).
                        if let Some(message) = self.engine.unevaluable_init.get(&(r.table, *pos)) {
                            return Err(Signal::Error(AlError {
                                message: message.clone(),
                                code: String::new(),
                                call_stack: String::new(),
                            }));
                        }
                    }
                    if !self.engine.unevaluable_init.is_empty() && r.touched {
                        let engine = self.engine.clone();
                        if let Some(message) = engine.unevaluable_init.get(&(r.table, *pos)) {
                            let pristine = self.initial_fields(r.table as usize);
                            if ops::compare(&r.fields[*pos], &pristine[*pos]).ok() == Some(std::cmp::Ordering::Equal) {
                                return Err(Signal::Unsupported(format!(
                                    "a read of the field whose InitValue BC cannot evaluate (`{message}`), in a record where another field was written (not pinned)"
                                )));
                            }
                        }
                    }
                    let v = r.fields[*pos].clone();
                    if r.load.partial {
                        drop(r);
                        rc.borrow_mut().load.touch(*pos);
                    }
                    Ok(v)
                }
                other => Err(Signal::Unsupported(format!("field of {other:?}"))),
            },
        }
    }

    fn write(&mut self, p: &ir::Place, v: Value, f: &mut Frame) -> Result<(), Signal> {
        // `:=` between FilterPageBuilders copies the control collection and shares the records
        // (corpus filterpage/TestFilterPageBuilder.al:FilterPageBuilder_Assignment_*).
        let v = match v {
            Value::Library(n, cell) if n == "FilterPageBuilder" => {
                Value::Library(n, filterpage::copy_for_assignment(&cell))
            }
            v => v,
        };
        // `:=` between records copies the fields into the target record (ADR-010).
        if let Value::Record(src) = &v {
            if !matches!(p, ir::Place::Field(..)) {
                if let Ok(Value::Record(dst)) = self.read(p, f) {
                    if !Rc::ptr_eq(src, &dst) {
                        let fields = src.borrow().fields.clone();
                        if fields.iter().any(|f| matches!(f, Value::Blob(_))) {
                            // Whether a copied record shares or copies its Blob is not pinned.
                            return Err(Signal::Unsupported("record assignment with a Blob field".into()));
                        }
                        let load = src.borrow().load.clone();
                        let mut d = dst.borrow_mut();
                        d.fields = fields;
                        // The copy holds what the source's buffer held, if that is known.
                        d.load.buffer = load.buffer;
                        d.load.partial = load.partial;
                    }
                    return Ok(());
                }
            }
        }
        if let ir::Place::Field(base, pos) = p {
            let ty = self.place_type(p, f);
            let v = ops::convert(v, &ty)?;
            return match self.read(base, f)? {
                Value::Record(rc) => {
                    let mut r = rc.borrow_mut();
                    r.fields[*pos] = v;
                    r.touched = true;
                    if r.load.partial {
                        r.load.written(*pos);
                    }
                    Ok(())
                }
                other => Err(Signal::Unsupported(format!("field of {other:?}"))),
            };
        }
        match p {
            ir::Place::Local(i) => f.locals[*i] = v,
            ir::Place::Global(i) => f.instance.borrow_mut().globals[*i] = v,
            ir::Place::Index(base, idx) if matches!(self.place_type(base, f), RtType::Text(_)) => {
                return self.write_char(base, idx, v, f);
            }
            ir::Place::Index(base, idx) => {
                let el_ty = self.place_type(p, f);
                let (arr, flat) = self.element(base, idx, f)?;
                arr.borrow_mut()[flat] = ops::convert(v, &el_ty)?;
            }
            ir::Place::Field(..) => unreachable!("handled above"),
        }
        Ok(())
    }

    /// `Text[i] := c`: sets one character of a text. Pinned only for an empty or growing text at
    /// the next position within the declared length (handlers/TestPageFieldFormats_Tests.al:58-66);
    /// a position outside that, a value that is not one UTF-16 character, and a Code are refused.
    fn write_char(&mut self, base: &ir::Place, idx: &[ir::Expr], v: Value, f: &mut Frame) -> Result<(), Signal> {
        let max = match self.place_type(base, f) {
            RtType::Text(m) => m,
            _ => None,
        };
        let [e] = idx else {
            return Err(Signal::Unsupported("a text indexed with several positions".into()));
        };
        let i = int(&self.eval(e, f)?)?;
        let unit = match int(&v) {
            Ok(c) => u16::try_from(c).ok().filter(|u| !(0xD800..0xE000).contains(u)),
            Err(_) => None,
        };
        let Value::Text(s) = self.read(base, f)? else {
            return Err(Signal::Unsupported(
                "assigning a character into a value that is no text".into(),
            ));
        };
        let mut units: Vec<u16> = s.encode_utf16().collect();
        let in_range = i >= 1 && (i as usize) <= units.len() + 1 && max.is_none_or(|m| i as usize <= m as usize);
        let (Some(unit), true) = (unit, in_range) else {
            return Err(Signal::Unsupported(format!(
                "assigning {v:?} to character {i} of a text (only the next position within the length is pinned)"
            )));
        };
        let i = i as usize - 1;
        if i == units.len() {
            units.push(unit);
        } else {
            units[i] = unit;
        }
        let text = String::from_utf16(&units)
            .map_err(|_| Signal::Unsupported("a text that ends up with a lone surrogate".into()))?;
        self.write(base, Value::Text(text), f)
    }

    /// The array behind `base` and the flat position of `idx` (1-based, row-major).
    fn element(&mut self, base: &ir::Place, idx: &[ir::Expr], f: &mut Frame) -> Result<(Cells, usize), Signal> {
        let RtType::Array(dims, _) = self.place_type(base, f) else {
            return Err(Signal::Unsupported("indexing a non-array".into()));
        };
        let arr = match self.read(base, f)? {
            Value::Array(a) => a,
            other => return Err(Signal::Unsupported(format!("indexing {other:?}"))),
        };
        let mut flat = 0usize;
        for (d, e) in dims.iter().zip(idx) {
            let i = int(&self.eval(e, f)?)?;
            if i < 1 || i > i64::from(*d) {
                return Err(Signal::Unsupported(
                    "array index out of bounds (error text not pinned yet)".into(),
                ));
            }
            flat = flat * (*d as usize) + (i as usize - 1);
        }
        Ok((arr, flat))
    }
}

/// The shared storage of an array value.
type Cells = Rc<RefCell<Vec<Value>>>;

/// A routine on the call stack and the position of the statement it is running.
#[derive(Clone, Copy)]
pub(crate) struct CallFrame {
    pub(crate) routine: usize,
    /// `Program::positions` id of the current statement; `NO_POS` before the first one.
    pub(crate) pos: u32,
}

struct Frame {
    object: usize,
    routine: usize,
    instance: Rc<RefCell<Instance>>,
    locals: Vec<Value>,
}

fn int(v: &Value) -> Result<i64, Signal> {
    match v {
        Value::Integer(i) => Ok(i64::from(*i)),
        Value::BigInteger(i) => Ok(*i),
        Value::Byte(b) => Ok(i64::from(*b)),
        Value::Char(c) => Ok(i64::from(*c)),
        other => Err(Signal::Unsupported(format!("{other:?} used as an integer"))),
    }
}

/// The kind of a value for a refusal reason: the variant name, never its contents.
pub(crate) fn value_kind(v: &Value) -> String {
    let d = format!("{v:?}");
    d.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("").to_string()
}

fn constant(c: &ir::Const) -> Value {
    match c {
        ir::Const::Boolean(b) => Value::Boolean(*b),
        ir::Const::Integer(i) => Value::Integer(*i),
        ir::Const::BigInteger(i) => Value::BigInteger(*i),
        ir::Const::Decimal(s) => Value::Decimal(s.parse().expect("decimal literal from the lowerer")),
        ir::Const::Text(s) => Value::Text(s.clone()),
        ir::Const::Date(d) => Value::Date(*d, false),
        ir::Const::Time(t) => Value::Time(*t),
        ir::Const::DateTime(t) => Value::DateTime(*t),
        ir::Const::Option(i, m) => Value::Option(*i, m.clone().into()),
        ir::Const::Enum(o, v) => Value::Enum(*o, *v),
        ir::Const::FieldPos(p) => Value::Integer(*p as i32),
    }
}

impl al_builtins::format::FormatCtx for Session {
    /// The caption a value declares, else its name (types/TestEnumCaption.al).
    fn enum_name(&self, id: u32, value: i32) -> String {
        let o = &self.engine.program.objects[id as usize];
        if let Some((_, c)) = o.enum_captions.iter().find(|(v, _)| *v == value) {
            return c.clone();
        }
        o.enum_values
            .iter()
            .find(|(v, _)| *v == value)
            .map(|(_, n)| n.clone())
            .unwrap_or_else(|| value.to_string())
    }
}

impl al_builtins::native::Ctx for Session {
    fn last_error(&self) -> AlError {
        self.last_error.clone()
    }
    fn set_last_error(&mut self, e: AlError) {
        self.last_error = e;
    }
    fn record(&mut self, method: &str, rec: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
        match rec {
            Some(Value::Record(rc)) => {
                let rc = rc.clone();
                self.record_op(method, &rc, args)
            }
            other => Err(Signal::Unsupported(format!("record method {method} on {other:?}"))),
        }
    }
    fn page(&mut self, method: &str, recv: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
        self.page_op(method, recv, args)
    }

    fn query(&mut self, method: &str, recv: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
        self.query_op(method, recv, args)
    }

    fn ref_op(&mut self, id: &str, receiver: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
        Session::ref_op(self, id, receiver, args)
    }
    fn session_op(&mut self, id: &str, args: &mut [Value]) -> Result<Value, Signal> {
        Session::session_op(self, id, args)
    }
    fn media_store(&mut self) -> Option<&mut al_builtins::media::MediaStore> {
        Some(&mut self.media)
    }
    fn collect_error(&mut self, info: &Value) -> bool {
        Session::collect_error(self, info)
    }
    fn fs_exists(&mut self, path: &str) -> Result<bool, Signal> {
        Ok(self.host.files.exists(path))
    }
    fn fs_read(&mut self, path: &str) -> Result<Option<Vec<u8>>, Signal> {
        Ok(self.host.files.read(path))
    }
    fn fs_write(&mut self, path: &str, bytes: &[u8]) -> Result<(), String> {
        self.host.files.write(path, bytes)
    }
    fn fs_remove(&mut self, path: &str) -> Result<bool, Signal> {
        Ok(self.host.files.remove(path))
    }
    fn fs_temp_name(&mut self) -> String {
        self.temp_files += 1;
        format!("{}{}.tmp", al_builtins::file::TEMP_DIR, self.temp_files)
    }
    fn session_locale(&self) -> Option<(i32, i32, String)> {
        Some((
            self.global_language,
            self.host.identity.locale_id(),
            self.host.identity.time_zone(),
        ))
    }
    fn module_at_frame(&self, depth: usize) -> Option<al_builtins::value::ModuleInfoState> {
        let at = self.frames.len().checked_sub(1 + depth)?;
        self.module_of_app(self.engine.program.objects[self.frames[at]].app as usize)
    }
    fn app_resources(&self) -> Option<al_builtins::native::AppResources> {
        let obj = *self.frames.last()?;
        let app = self
            .engine
            .program
            .apps
            .get(self.engine.program.objects[obj].app as usize)?;
        Some((app.name.clone(), app.resources.clone()))
    }
    fn module_callstack(&self, skip: usize) -> Vec<al_builtins::value::ModuleInfoState> {
        let mut seen = Vec::new();
        let mut out = Vec::new();
        for obj in self.frames.iter().rev().skip(skip) {
            let app = self.engine.program.objects[*obj].app as usize;
            if !seen.contains(&app) {
                seen.push(app);
                out.extend(self.module_of_app(app));
            }
        }
        out
    }
    fn module_by_id(&self, id: &[u8; 16]) -> al_builtins::native::ModuleLookup {
        use al_builtins::native::ModuleLookup;
        let apps = &self.engine.program.apps;
        let parse = |s: &str| ops::parse_guid(s);
        if let Some(i) = apps.iter().position(|a| parse(&a.id).as_ref() == Some(id)) {
            if let Some(m) = self.module_of_app(i) {
                return ModuleLookup::Found(m);
            }
        }
        if apps
            .iter()
            .any(|a| a.dependencies.iter().any(|d| parse(d).as_ref() == Some(id)))
        {
            return ModuleLookup::OutsideWorkspace;
        }
        ModuleLookup::NotInstalled
    }
    fn workspace_table(&self, number: i32) -> bool {
        self.engine
            .program
            .objects
            .iter()
            .any(|o| o.kind == "table" && o.id.is_some_and(|id| i64::from(id) == i64::from(number)))
    }
    fn install_scope(&self) -> bool {
        self.install_scope
    }
    fn in_test_run(&self) -> bool {
        self.in_test_codeunit
    }
    fn enum_members(&self, id: u32) -> Option<Vec<(i32, String)>> {
        Some(self.engine.program.objects[id as usize].enum_values.clone())
    }
    fn commit(&mut self) -> Result<(), Signal> {
        self.commit_builtin()
    }
    fn ret_used(&self) -> bool {
        self.ret_used
    }
    fn ui_message(&mut self, text: String) -> Result<(), Signal> {
        Session::ui_message(self, text)
    }
    fn ui_confirm(&mut self, question: String, default: bool) -> Result<bool, Signal> {
        Session::ui_confirm(self, question, default)
    }
    fn ui_str_menu(&mut self, options: String, default: i32, instruction: String) -> Result<i32, Signal> {
        Session::ui_str_menu(self, options, default, instruction)
    }
    fn ui_session_settings(&mut self, settings: Value) -> Result<(), Signal> {
        Session::ui_session_settings(self, settings)
    }
    fn new_guid(&mut self) -> [u8; 16] {
        // Deterministic ids (ADR-006: no OS randomness in the core).
        self.next_guid += 1;
        let mut g = [0u8; 16];
        g[8..].copy_from_slice(&self.next_guid.to_be_bytes());
        g[6] = 0x40;
        g
    }
}

#[cfg(test)]
mod conformance {
    /// ADR-001: the compiled program is shared across threads; sessions are not.
    #[test]
    fn engine_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<super::Engine>();
    }
}
