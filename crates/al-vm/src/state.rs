//! Sub-states of a [`Session`] (ADR-037 item 3): fields that belong together, so `Session` stays readable.

use super::*;

/// Transactions: the depth of transaction-beginning constructs, and where the open transaction stands.
pub(crate) struct TxState {
    /// Nesting depth of constructs that begin a transaction of their own (`Codeunit.Run`,
    /// a TestPage operation); a `TransactionModel::None` test may write inside them.
    pub(crate) tx_world: usize,
    /// The `[CommitBehavior]` of the innermost procedure that declares one.
    pub(crate) commit_behavior: transactions::CommitBehavior,
    /// How far the transaction has started since the test method began or the last `Commit()`:
    /// `TX_FRESH` (nothing yet), `TX_READ` (a read call) or `TX_WRITE` (a write call, also a DeleteAll that
    /// finds no row). `CurrentTransactionType` depends on it (`sessionops.rs`).
    pub(crate) tx_phase: u8,
    /// `Database.CurrentTransactionType`: a member index of the TransactionType option; a test
    /// method starts at 0, UpdateNoLocks (`start_test_method`).
    pub(crate) transaction_type: i32,
    /// `Database.LockTimeout`: whether a lock wait ends in a timeout error.
    pub(crate) lock_timeout: bool,
    /// `Database.LockTimeoutDuration` in milliseconds.
    pub(crate) lock_timeout_duration: i64,
    /// The highest rowversion handed out when the last `Commit` ran: rows at or below it are durable.
    pub(crate) committed_row_version: i64,
    /// A duplicate-key Insert into a table without a Blob field does not fail where it is
    /// called: the table (object index) and the error come out at the next access to the table
    /// or at Commit (corpus record/TestRecordRuntimeTextsAndViews.al:
    /// Record_Insert_DuplicateKey_WithoutBlobField_IsDeferred_AndSurfacesAtTheNextAccess).
    pub(crate) deferred_insert: Option<(usize, AlError)>,
    /// Set while the page client or an XmlPort import writes a record: their duplicate-key
    /// errors are raised at once (their own tests pin it), not deferred like an AL `Insert`.
    pub(crate) immediate_errors: bool,
    /// Try functions on the call stack (Truncate is refused inside them).
    pub(crate) try_depth: usize,
}

/// Handler functions and notifications of the running test method.
pub(crate) struct UiState {
    /// Handler functions bound to the running test method (None outside one).
    pub(crate) scope: Option<ui::UiScope>,
    /// Page handler functions running right now (`[PageHandler]`, `[ModalPageHandler]`).
    pub(crate) page_handler_depth: usize,
    /// Ids of the notifications a `[SendNotificationHandler]` took in this session.
    pub(crate) sent_notifications: Vec<[u8; 16]>,
}

/// What the TestPage client keeps between calls.
pub(crate) struct PageClients {
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
    /// Records whose `CurrFieldNo` is set (a TestPage write validating a field, or an assignment in table
    /// code), with the field's number: what `CurrFieldNo` reads. Held weakly: the entry keeps the record's
    /// allocation, so a later record cannot get the same address and inherit the number.
    pub(crate) curr_field_no: Vec<(std::rc::Weak<RefCell<RecordState>>, i32)>,
    /// The report instances that `Execute` has run: a second `Execute` on one is refused.
    /// A page opened for a `RunObject` action looks its handler up without failing when there is
    /// none (al-vm pages/runobject.rs).
    pub(crate) lenient_bind: bool,
}

/// Event subscriptions and what the Event Subscription table reports.
pub(crate) struct EventState {
    /// Manual event subscriptions made by `BindSubscription`.
    pub(crate) bindings: Vec<dispatch::Binding>,
    /// What the Event Subscription table reports (calls handled, bound instances).
    pub(crate) event_stats: Rc<platform::EventStats>,
    /// `publish` is about to raise its event: the next `raise` has an event frame on the stack.
    pub(crate) publish_frame: bool,
    /// Events being raised that have no publisher routine (table, page and Global Triggers
    /// events): BC puts frames between the subscriber and its caller that are not pinned, so the
    /// live call stack is not answered while one runs.
    pub(crate) unpinned_event_depth: usize,
    /// What the subscribers of `Global Triggers` answered, per table object (globaltriggers.rs).
    pub(crate) global_setups: std::collections::HashMap<usize, globaltriggers::GlobalSetup>,
}

/// Code coverage recording.
pub(crate) struct CoverageState {
    /// `CodeCoverageLog` is recording.
    pub(crate) code_coverage: bool,
    /// `alrunner test --coverage`: runs per statement position (`Program::positions`) since the last
    /// [`Session::take_coverage`]. `None` when coverage is off, which costs one branch per statement.
    pub(crate) coverage_counts: Option<(Vec<u32>, Vec<u32>)>,
    /// What recording noted (`platform/coverage.rs`).
    pub(crate) recorder: platform::coverage::Recorder,
}

/// The report being run: language, trigger flow, dataset.
pub(crate) struct ReportState {
    /// `CurrReport.Language` of the running report (`reports.rs`); `None` until a trigger sets it.
    pub(crate) curr_report_language: Option<i32>,
    /// The call depth of the `OnInitReport` trigger being run, and whether it called
    /// `CurrReport.Quit` (al-vm reports.rs).
    pub(crate) report_init_depth: Option<usize>,
    pub(crate) report_quit: bool,
    /// The call depth of the report trigger being run and what it may ask of the report
    /// (`CurrReport.Skip`, `Break`, `Quit`), and the request it made (al-vm reports.rs).
    pub(crate) report_trigger: Option<(usize, crate::reports::TrigKind)>,
    pub(crate) report_flow: Option<crate::reports::Flow>,
    /// A `CurrReport.Skip`, `Break` or `Quit` is leaving the procedures down to the report trigger at this depth.
    pub(crate) flow_unwind: Option<usize>,
    /// The request page of a `RunRequestPage` is open: a table write in its handler finds no transaction
    /// (report/TestProbeReportContracts.al:RequestPage_TableWriteInsideHandler).
    pub(crate) request_only_page: bool,
    pub(crate) report_executed: Vec<std::rc::Weak<RefCell<Instance>>>,
    /// Report variables a procedure of the caller initialized before their run (`SetParameters` and the like).
    pub(crate) prepared: Vec<std::rc::Weak<RefCell<Instance>>>,
    /// For each report body running, innermost last: whether `CurrReport.Preview()` is pinned to be
    /// false for it (it was started by `Run` or `RunModal`).
    pub(crate) report_preview_pinned: Vec<bool>,
    /// The dataset a `Report.SaveAs(.., Xml, ..)` run is writing (al-vm reports.rs).
    pub(crate) dataset: Option<crate::dataset::Dataset>,
}
