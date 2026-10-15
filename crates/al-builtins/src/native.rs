//! Native builtin implementations, registered by catalog overload id (ADR-008).

use crate::describe::Describe;
use crate::format::{FormatCtx, FormatFail, format_value, try_format};
use crate::value::Value;
use std::rc::Rc;

/// The error state AL code can read back (`GetLastErrorText`, ...).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlError {
    pub message: String,
    pub code: String,
    pub call_stack: String,
}

/// Raises an AL error with a pinned text and code.
pub fn raise<T>(message: impl Into<String>, code: &str) -> Result<T, Signal> {
    Err(Signal::Error(AlError {
        message: message.into(),
        code: code.into(),
        call_stack: String::new(),
    }))
}

/// Non-local control flow (ADR-002, ADR-011).
#[derive(Debug, Clone)]
pub enum Signal {
    Error(AlError),
    Exit(Option<Value>),
    Break,
    /// `continue`: go on with the next round of the innermost loop.
    Continue,
    /// Behaviour the runtime does not implement yet, met at run time (an edge
    /// case no corpus test pins). The runner reports the test as not run.
    Unsupported(String),
}

/// An app's name and its packaged resources (name relative to the resource folder, content).
pub type AppResources = (String, Vec<(String, Vec<u8>)>);

/// What `NavApp.GetModuleInfo` finds.
pub enum ModuleLookup {
    Found(crate::value::ModuleInfoState),
    /// No app of the workspace has this id and none of its manifests names it.
    NotInstalled,
    /// An app a manifest names as a dependency, which is not part of the workspace.
    OutsideWorkspace,
}

/// What builtins need from the running session.
pub trait Ctx: FormatCtx {
    fn last_error(&self) -> AlError;
    fn set_last_error(&mut self, e: AlError);
    /// Record methods (`Record.Insert`, ...) need the store, triggers and
    /// table metadata: the VM implements them. `method` is the catalog name.
    fn record(&mut self, method: &str, rec: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal>;
    /// `TestPage` / `TestField` / `TestAction` methods: the VM's test client
    /// implements them (ADR-012). `method` is `Receiver.Name`.
    fn page(&mut self, method: &str, _recv: Option<&mut Value>, _args: &mut [Value]) -> Result<Value, Signal> {
        Err(Signal::Unsupported(format!("{method} needs a page runtime")))
    }

    /// `Query` variable methods: the VM executes the query (`al-vm/src/queries.rs`).
    /// `method` is the catalog name (`Open`, `SetRange`, ...).
    fn query(&mut self, method: &str, _recv: Option<&mut Value>, _args: &mut [Value]) -> Result<Value, Signal> {
        Err(Signal::Unsupported(format!("Query.{method} needs a query runtime")))
    }

    /// `RecordRef`, `FieldRef` and `KeyRef` methods need the table schema and
    /// the store, like record methods. `id` is the catalog overload id
    /// (`RecordRef.Open#2`); the VM resolves it to the method.
    fn ref_op(&mut self, id: &str, _receiver: Option<&mut Value>, _args: &mut [Value]) -> Result<Value, Signal> {
        Err(Signal::Unsupported(format!("builtin {id}")))
    }
    /// Today's day number (1 = 0001-01-01) on the host's clock; `None` where there is no host.
    fn today_day(&self) -> Option<i32> {
        None
    }
    /// `Format` of a record variable, a `RecordRef` or a `FieldRef` (standard format 0): needs the
    /// table's fields. `None` where the context has no program.
    fn format_composite(&mut self, _v: &Value) -> Option<Result<String, Signal>> {
        None
    }
    /// `Format` of a `DotNet` object or enum: its `ToString()` (ADR-024). `None` where the context
    /// has no .NET host.
    fn format_dotnet(&mut self, _v: &Value) -> Option<Result<String, Signal>> {
        None
    }
    /// `CopyStream` where one side is a `DotNet` stream (ADR-024). `None` where the context has no
    /// .NET host.
    fn copy_stream_dotnet(&mut self, _out: &Value, _src: &Value, _count: Option<i64>) -> Option<Result<(), Signal>> {
        None
    }
    /// The declared values of enum object `id` as (ordinal, name), in declaration order with
    /// extension values after them. `None` when the context has no program.
    fn enum_members(&self, _id: u32) -> Option<Vec<(i32, String)>> {
        None
    }
    /// The captions the values of enum object `id` declare, as (ordinal, caption).
    fn enum_captions(&self, _id: u32) -> Vec<(i32, String)> {
        Vec::new()
    }
    /// Builtins that read or change session state (number sequences, the transaction state): the
    /// VM implements them. `id` is the catalog overload id (`NumberSequence.Next#1`); `args` holds
    /// the call's arguments, with `var` parameters written back in place.
    fn session_op(&mut self, id: &str, _args: &mut [Value]) -> Result<Value, Signal> {
        Err(Signal::Unsupported(format!("builtin {id} needs a session")))
    }
    /// The language id, locale id and Windows time zone name of the running session, which
    /// `SessionSettings.Init` reads. `None` where the context has no host.
    fn session_locale(&self) -> Option<(i32, i32, String)> {
        None
    }
    /// The app of the routine `depth` frames out on the call stack: 0 is the routine that runs the
    /// builtin, 1 its caller. `None` when there is no such frame (the caller of a test is the test
    /// runner) or the app's manifest has no readable id.
    fn module_at_frame(&self, _depth: usize) -> Option<crate::value::ModuleInfoState> {
        None
    }
    /// The distinct apps of the frames on the call stack, innermost first, leaving out the
    /// `skip` innermost frames.
    fn module_callstack(&self, _skip: usize) -> Vec<crate::value::ModuleInfoState> {
        Vec::new()
    }
    /// The frames of the test runner below the test, in `Callstack` form (backslash after each
    /// frame); empty where the context has no runner (al-vm/callstack.rs).
    fn runner_call_stack(&self) -> String {
        String::new()
    }
    /// The name and packaged resources of the app that owns the running code.
    fn app_resources(&self) -> Option<AppResources> {
        None
    }
    /// The module BC answers `GetModuleInfo` with for the null app id: the installed application.
    fn application_module(&self) -> crate::value::ModuleInfoState {
        crate::value::ModuleInfoState {
            id: [0; 16],
            name: String::new(),
            publisher: String::new(),
            version: None,
            dependencies: Vec::new(),
            synthesized: true,
        }
    }
    /// The installed app with this id.
    fn module_by_id(&self, _id: &[u8; 16]) -> ModuleLookup {
        ModuleLookup::NotInstalled
    }
    /// Whether table `number` (`Database::"Name"`) is declared in the workspace.
    fn workspace_table(&self, _number: i32) -> bool {
        false
    }
    /// Whether an install codeunit is running (DataTransfer works there and only there).
    fn install_scope(&self) -> bool {
        false
    }
    /// Whether a test codeunit is being run by the test runner (under TestIsolation = Codeunit).
    fn in_test_run(&self) -> bool {
        false
    }
    /// `Error(ErrorInfo)` of a collectible error: inside a `[ErrorBehavior(Collect)]` scope the
    /// session keeps a copy and the call returns normally (`true`); outside one it is raised like
    /// any other error (`false`).
    fn collect_error(&mut self, _info: &Value) -> bool {
        false
    }
    /// The session's media objects and media sets (`Media` and `MediaSet` fields).
    fn media_store(&mut self) -> Option<&mut crate::media::MediaStore> {
        None
    }
    /// Whether the host file system has a file with this path (`File` data type).
    fn fs_exists(&mut self, _path: &str) -> Result<bool, Signal> {
        Err(Signal::Unsupported("File: this context has no file system".into()))
    }
    /// The bytes of the file at `path`, `None` if there is none.
    fn fs_read(&mut self, _path: &str) -> Result<Option<Vec<u8>>, Signal> {
        Err(Signal::Unsupported("File: this context has no file system".into()))
    }
    /// Creates the file at `path` or replaces its content; the host's reason if it cannot.
    fn fs_write(&mut self, _path: &str, _bytes: &[u8]) -> Result<(), String> {
        Err("this context has no file system".into())
    }
    /// Deletes the file at `path`; `false` if there was none.
    fn fs_remove(&mut self, _path: &str) -> Result<bool, Signal> {
        Err(Signal::Unsupported("File: this context has no file system".into()))
    }
    /// A new name for a temporary file, under [`crate::file::TEMP_DIR`].
    fn fs_temp_name(&mut self) -> String {
        format!("{}0.tmp", crate::file::TEMP_DIR)
    }
    /// `Commit()`.
    fn commit(&mut self) -> Result<(), Signal>;
    /// A new, unique Guid (from the host or a deterministic sequence, ADR-006).
    fn new_guid(&mut self) -> [u8; 16];
    /// Whether the result of the builtin being called is used (`[Ok :=]` forms
    /// return false instead of raising when it is).
    fn ret_used(&self) -> bool {
        true
    }
    /// `Message(text)` after argument substitution. The runner decides what a
    /// message does (a test handler, an "Unhandled UI" refusal); a context
    /// with no UI refuses explicitly.
    fn ui_message(&mut self, _text: String) -> Result<(), Signal> {
        Err(Signal::Unsupported("Message: this context has no UI".into()))
    }
    /// `Confirm(question, default)`: the question is NOT substituted
    /// (corpus handlers/TestConfirmFormatArgs.al).
    fn ui_confirm(&mut self, _question: String, _default: bool) -> Result<bool, Signal> {
        Err(Signal::Unsupported("Confirm: this context has no UI".into()))
    }
    /// `Notification.Send()`: hands a copy of the notification (with its id) to the bound
    /// `[SendNotificationHandler]`, if there is one, and answers the notification as the handler
    /// left it. With no handler nothing is shown and `None` comes back.
    fn ui_send_notification(&mut self, _notification: Value, _id: [u8; 16]) -> Result<Option<Value>, Signal> {
        Err(Signal::Unsupported("Notification.Send: this context has no UI".into()))
    }
    /// `Notification.Recall()` of the notification with this id: false when it was never sent.
    /// A bound `[RecallNotificationHandler]` gets a copy of the notification.
    fn ui_recall_notification(&mut self, _notification: Value, _id: [u8; 16]) -> Result<bool, Signal> {
        Err(Signal::Unsupported(
            "Notification.Recall: this context has no UI".into(),
        ))
    }
    /// `SessionSettings.RequestSessionUpdate(false)`: hands a copy of the settings to the bound
    /// `[SessionSettingsHandler]`.
    fn ui_session_settings(&mut self, _settings: Value) -> Result<(), Signal> {
        Err(Signal::Unsupported(
            "SessionSettings.RequestSessionUpdate: this context has no UI".into(),
        ))
    }
    /// `StrMenu(options, default, instruction)`: the chosen option, 1-based, 0 for none.
    fn ui_str_menu(&mut self, _options: String, _default: i32, _instruction: String) -> Result<i32, Signal> {
        Err(Signal::Unsupported("StrMenu: this context has no UI".into()))
    }
}

pub type NativeFn = fn(&mut dyn Ctx, Option<&mut Value>, &mut [Value]) -> Result<Value, Signal>;

const REGISTRY: &[(&str, NativeFn)] = &[
    ("System.Format#1", format_1),
    ("System.Format#2", format_2),
    ("System.Evaluate#1", crate::evaluate::evaluate),
    ("System.Abs#1", abs),
    ("System.GetLastErrorText#1", get_last_error_text),
    ("System.GetLastErrorCode#1", get_last_error_code),
    ("System.GetLastErrorCallStack#1", get_last_error_call_stack),
    ("System.ClearLastError#1", clear_last_error),
    ("Dialog.Error#2", error_text),
    ("Dialog.Error#1", crate::errorinfo::raise),
    ("Session.GetExecutionContext#1", execution_context),
    ("Session.GetCurrentModuleExecutionContext#1", execution_context),
    ("Session.GetModuleExecutionContext#1", module_execution_context),
    ("Dialog.Message#1", message),
    ("Dialog.Confirm#1", confirm),
    ("Dialog.StrMenu#1", str_menu),
    ("Text.StrLen#1", strlen),
    ("Text.StrPos#1", strpos),
    ("Text.StrCheckSum#1", strchecksum),
    ("Session.LogMessage#1", log_message),
    ("Session.LogMessage#2", log_message),
    ("Session.LogSecurityAudit#1", log_message),
    ("Session.LogAuditMessage#1", log_message),
    ("System.Hyperlink#1", hyperlink),
    ("System.Variant2Date#1", variant_2_date),
    ("System.Variant2Time#1", variant_2_time),
    // Whether the host can create tasks is not pinned (corpus session/TestTaskSchedulerCloudBehavior.al
    // pins only that CreateTask returns a task id): a run that reaches the question is refused.
    // The URL of the web client depends on the server's address, which a host has not been asked
    // for: a run that builds one is refused (session/TestFinalCoverage.al:105-125 asks only that
    // GetUrl does not throw).
    ("System.GetUrl#1", |_, _, _| {
        unsupported("GetUrl (the server address is not a host value)")
    }),
    ("System.GetUrl#2", |_, _, _| {
        unsupported("GetUrl (the server address is not a host value)")
    }),
    ("System.GetUrl#3", |_, _, _| {
        unsupported("GetUrl (the server address is not a host value)")
    }),
    ("System.GetUrl#4", |_, _, _| {
        unsupported("GetUrl (the server address is not a host value)")
    }),
    // Measured on real BC: session/TestEnvironmentUserTenantContracts.al:237-241.
    ("Session.CurrentExecutionMode#1", |_, _, _| {
        let members: std::sync::Arc<[String]> = crate::sysenum::option_members("ExecutionMode")
            .expect("a system option")
            .into();
        Ok(Value::Option(0, members))
    }),
    ("Session.ApplicationIdentifier#1", |_, _, _| {
        Ok(Value::Text("NAV".into()))
    }),
    ("ProductName.Full#1", |_, _, _| {
        Ok(Value::Text("Dynamics 365 Business Central".into()))
    }),
    ("ProductName.Short#1", |_, _, _| {
        Ok(Value::Text("Business Central".into()))
    }),
    ("ProductName.Marketing#1", |_, _, _| {
        Ok(Value::Text("Microsoft Dynamics 365 Business Central".into()))
    }),
    // False in a test: measured on real BC by session/TestEnvironmentUserTenantContracts.al:238
    // (Assert.IsFalse(TaskScheduler.CanCreateTask())).
    ("TaskScheduler.CanCreateTask#1", |_, _, _| Ok(Value::Boolean(false))),
    // CreateTask hands back a new task id (session/TestTaskSchedulerCloudBehavior.al:9-20 pins that
    // and nothing more). A task starts in a session of its own after the creating transaction
    // commits, which a test does not wait for, so the task is not run (docs/probes-needed.md).
    ("TaskScheduler.CreateTask#1", |c, _, _| Ok(Value::Guid(c.new_guid()))),
    ("TaskScheduler.CreateTask#2", |c, _, _| Ok(Value::Guid(c.new_guid()))),
    ("TaskScheduler.SetTaskReady#1", |_, _, _| {
        unsupported("TaskScheduler.SetTaskReady (not pinned)")
    }),
    ("TaskScheduler.TaskExists#1", |_, _, _| {
        unsupported("TaskScheduler.TaskExists (not pinned)")
    }),
    ("TaskScheduler.CancelTask#1", |_, _, _| {
        unsupported("TaskScheduler.CancelTask (not pinned)")
    }),
    ("Session.StartSession#1", start_session),
    ("Session.StartSession#2", start_session),
    ("Session.StartSession#3", start_session),
    ("Text.CopyStr#1", copystr),
    ("Text.StrSubstNo#1", strsubstno),
    ("Text.UpperCase#1", uppercase),
    ("Text.LowerCase#1", lowercase),
    ("Database.Commit#1", |c, _, _| c.commit().map(|_| Value::Null)),
    ("Database.SID#1", database_sid),
    ("System.Clear#1", clear),
    // A progress dialog is a headless no-op in a test (handlers/TestHandlersDialogProgressNoOp.al:24-110).
    ("Dialog.Open#1", |_, _, _| Ok(Value::Null)),
    ("Dialog.Update#1", |_, _, _| Ok(Value::Null)),
    ("Dialog.Close#1", |_, _, _| Ok(Value::Null)),
    // Reading the latest committed data is what this runtime always does (session/TestBCDatabaseContracts.al:170-187).
    // Telemetry goes to the server's log, which AL cannot read (ADR-009).
    ("Session.LogMessage#1", |_, _, _| Ok(Value::Null)),
    ("Session.LogMessage#2", |_, _, _| Ok(Value::Null)),
    ("Database.SelectLatestVersion#1", |_, _, _| Ok(Value::Null)),
    ("Database.SelectLatestVersion#2", |_, _, _| Ok(Value::Null)),
    // Unique and not null is all the corpus asks (session/TestSystemExtended.al:306-328).
    ("Guid.CreateSequentialGuid#1", |c, _, _| Ok(Value::Guid(c.new_guid()))),
    // A TextBuilder is bounded by the Integer range; the corpus needs at least a million
    // (session/TestSystemExtended.al:450-473).
    ("TextBuilder.MaxCapacity#1", |_, _, _| Ok(Value::Integer(i32::MAX))),
    ("Enum.AsInteger#1", enum_as_integer),
    ("Enum.Names#1", enum_names),
    ("Enum.Ordinals#1", enum_ordinals),
    ("Enum.FromInteger#1", enum_from_integer),
    ("Record.Init#1", |c, r, a| c.record("Init", r, a)),
    ("Record.Insert#1", |c, r, a| c.record("Insert", r, a)),
    ("Record.Insert#2", |c, r, a| c.record("Insert", r, a)),
    ("Record.Modify#1", |c, r, a| c.record("Modify", r, a)),
    ("Record.Delete#1", |c, r, a| c.record("Delete", r, a)),
    ("Record.DeleteAll#1", |c, r, a| c.record("DeleteAll", r, a)),
    ("Record.Get#1", |c, r, a| c.record("Get", r, a)),
    ("Record.Find#1", |c, r, a| c.record("Find", r, a)),
    ("Record.FindFirst#1", |c, r, a| c.record("FindFirst", r, a)),
    ("Record.FindLast#1", |c, r, a| c.record("FindLast", r, a)),
    ("Record.FindSet#1", |c, r, a| c.record("FindSet", r, a)),
    ("Record.FindSet#2", |c, r, a| c.record("FindSet", r, a)),
    ("Record.Next#1", |c, r, a| c.record("Next", r, a)),
    ("Record.Count#1", |c, r, a| c.record("Count", r, a)),
    ("Record.IsEmpty#1", |c, r, a| c.record("IsEmpty", r, a)),
    ("Record.SetRange#1", |c, r, a| c.record("SetRange", r, a)),
    ("Record.SetFilter#1", |c, r, a| c.record("SetFilter", r, a)),
    // Event subscriptions are session state; the VM intercepts these ids
    // (al-vm dispatch.rs). Registered so lowering accepts them.
    ("Session.ApplicationArea#1", vm_only),
    // ClearAll resets the running object's globals: the VM runs it (al-vm lib.rs).
    ("System.ClearAll#1", vm_only),
    ("Session.BindSubscription#1", vm_only),
    ("Session.UnbindSubscription#1", vm_only),
    // Codeunit.Run runs a codeunit and `IsInWriteTransaction` reads the transaction: both are
    // session state, run by the VM (al-vm transactions.rs).
    ("Codeunit.Run#1", vm_only),
    ("Codeunit.Run#2", vm_only),
    ("Codeunit.Run#3", vm_only),
    ("Database.IsInWriteTransaction#1", vm_only),
    // XmlPort import and export run triggers and write records: the VM runs them
    // (al-vm xmlports.rs).
    ("Xmlport.Export#1", vm_only),
    ("Xmlport.Export#2", vm_only),
    ("Xmlport.Import#1", vm_only),
    ("Xmlport.Import#2", vm_only),
    ("Xmlport.SetSource#1", vm_only),
    ("Xmlport.SetDestination#1", vm_only),
    ("Xmlport.SetTableView#1", vm_only),
    // Report runs execute triggers and write records: the VM runs them (al-vm reports.rs). The
    // ids not listed (SaveAsPdf, SaveAsXml and the other file forms, Preview, the layout accessors, Skip, Break, ...) need
    // output or behaviour the corpus does not pin and are refused in lowering.
    // A FilterPageBuilder keeps record buffers and renders views: the VM runs it (al-vm filterpage.rs).
    ("FilterPageBuilder.AddTable#1", vm_only),
    ("FilterPageBuilder.AddRecord#1", vm_only),
    ("FilterPageBuilder.AddRecordRef#1", vm_only),
    ("FilterPageBuilder.AddField#1", vm_only),
    ("FilterPageBuilder.AddField#2", vm_only),
    ("FilterPageBuilder.AddFieldNo#1", vm_only),
    ("FilterPageBuilder.Count#1", vm_only),
    ("FilterPageBuilder.GetView#1", vm_only),
    ("FilterPageBuilder.SetView#1", vm_only),
    ("FilterPageBuilder.Name#1", vm_only),
    ("FilterPageBuilder.PageCaption#1", vm_only),
    ("FilterPageBuilder.RunModal#1", vm_only),
    ("Report.Run#1", vm_only),
    ("Report.Run#3", vm_only),
    ("Report.RunModal#1", vm_only),
    ("Report.RunModal#3", vm_only),
    ("Report.Execute#1", vm_only),
    ("Report.Execute#3", vm_only),
    ("Report.WordXmlPart#1", vm_only),
    ("Report.WordXmlPart#2", vm_only),
    ("Report.Print#1", vm_only),
    ("Report.Print#2", vm_only),
    ("Report.RunRequestPage#1", vm_only),
    ("Report.RunRequestPage#2", vm_only),
    ("Report.SetTableView#1", vm_only),
    ("Report.UseRequestPage#1", vm_only),
    ("Report.ObjectId#1", vm_only),
    ("Report.Preview#1", vm_only),
    ("RequestPage.Update#1", vm_only),
    ("Report.Language#1", vm_only),
    ("Report.FormatRegion#1", vm_only),
    // SaveAs with a stream runs the report and writes its dataset (al-vm reports.rs); SaveAs to a file
    // name (SaveAsXml, SaveAsPdf, ...) writes a file no corpus test pins and stays refused.
    ("Report.SaveAs#1", vm_only),
    ("Report.SaveAs#2", vm_only),
    ("Report.Quit#1", vm_only),
    ("Report.Skip#1", vm_only),
    ("Report.Break#1", vm_only),
    ("System.CreateGuid#1", |c, _, _| Ok(Value::Guid(c.new_guid()))),
    // `IsNull(DotNet)`: an unassigned DotNet variable, or a .NET call that returned null, holds
    // `Null` (ADR-014). Microsoft's Azure AD Tenant Impl. and Json Impl. ask it
    // (session/TestEnvironmentUserTenantContracts.al:204-232 reaches them).
    ("System.IsNull#1", |_, _, a| {
        Ok(Value::Boolean(matches!(a.first(), Some(Value::Null))))
    }),
    ("System.IsNullGuid#1", |_, _, a| {
        Ok(Value::Boolean(matches!(a[0], Value::Guid(g) if g == [0; 16])))
    }),
    ("Record.Reset#1", |c, r, a| c.record("Reset", r, a)),
    ("Record.CalcFields#1", |c, r, a| c.record("CalcFields", r, a)),
    ("Record.SetCurrentKey#1", |c, r, a| c.record("SetCurrentKey", r, a)),
    ("Record.Validate#1", |c, r, a| c.record("Validate", r, a)),
    ("Record.Rename#1", |c, r, a| c.record("Rename", r, a)),
    ("Record.Truncate#1", |c, r, a| c.record("Truncate", r, a)),
    ("Record.CalcSums#1", |c, r, a| c.record("CalcSums", r, a)),
    ("Record.SetLoadFields#1", |c, r, a| c.record("SetLoadFields", r, a)),
    ("Record.AddLoadFields#1", |c, r, a| c.record("AddLoadFields", r, a)),
    ("Record.LoadFields#1", |c, r, a| c.record("LoadFields", r, a)),
    ("Record.SetBaseLoadFields#1", |c, r, a| {
        c.record("SetBaseLoadFields", r, a)
    }),
    ("Record.AreFieldsLoaded#1", |c, r, a| c.record("AreFieldsLoaded", r, a)),
    ("Record.SetAutoCalcFields#1", |c, r, a| {
        c.record("SetAutoCalcFields", r, a)
    }),
    ("Variant.IsDotNet#1", |_, r, _| is(r, "DotNet")),
    ("Variant.IsBoolean#1", |_, r, _| is(r, "Boolean")),
    ("Variant.IsInteger#1", |_, r, _| is(r, "Integer")),
    ("Variant.IsBigInteger#1", |_, r, _| is(r, "BigInteger")),
    ("Variant.IsDecimal#1", |_, r, _| is(r, "Decimal")),
    ("Variant.IsByte#1", |_, r, _| is(r, "Byte")),
    ("Variant.IsChar#1", |_, r, _| is(r, "Char")),
    ("Variant.IsText#1", |_, r, _| is(r, "Text")),
    ("Variant.IsCode#1", |_, r, _| is(r, "Code")),
    ("Variant.IsDate#1", |_, r, _| is(r, "Date")),
    ("Variant.IsTime#1", |_, r, _| is(r, "Time")),
    ("Variant.IsDateTime#1", |_, r, _| is(r, "DateTime")),
    ("Variant.IsDuration#1", |_, r, _| is(r, "Duration")),
    ("Variant.IsGuid#1", |_, r, _| is(r, "Guid")),
    ("Variant.IsDateFormula#1", |_, r, _| is(r, "DateFormula")),
    ("Variant.IsOption#1", |_, r, _| is(r, "Option")),
    ("Variant.IsList#1", |_, r, _| is(r, "List")),
    ("Variant.IsDictionary#1", |_, r, _| is(r, "Dictionary")),
    ("Variant.IsCodeunit#1", |_, r, _| is(r, "Codeunit")),
    // RecordId, RecordRef and FieldRef variables are library handles named after their type.
    ("Variant.IsNotification#1", |_, r, _| is(r, "Notification")),
    ("Variant.IsTextBuilder#1", |_, r, _| is(r, "TextBuilder")),
    ("Variant.IsRecordId#1", |_, r, _| is(r, "RecordId")),
    ("Variant.IsRecord#1", |_, r, _| is(r, "Record")),
    ("Variant.IsRecordRef#1", |_, r, _| is(r, "RecordRef")),
    ("Variant.IsFieldRef#1", |_, r, _| is(r, "FieldRef")),
    // Permission enforcement is not modelled (`Permissions Mock`, docs/probes-needed.md): the session
    // user reads every row, so the permission filter filters nothing. Record: record/TestRecordFilter.al:
    // 406-413 pins that it does not throw. RecordRef is the same call on the same model, not pinned itself.
    ("Record.SetPermissionFilter#1", |_, _, _| Ok(Value::Null)),
    ("RecordRef.SetPermissionFilter#1", |_, _, _| Ok(Value::Null)),
    // Kinds the value model has no variant for yet are never true.
    ("Variant.IsTextConstant#1", |_, _, _| Ok(Value::Boolean(false))),
    ("Variant.IsAutomation#1", |_, _, _| Ok(Value::Boolean(false))),
    ("Variant.IsFile#1", |_, _, _| Ok(Value::Boolean(false))),
];

/// Builtins that are refused on purpose, with the reason. No corpus test pins what they answer, so
/// the lowering says so instead of "is not implemented" (hard rule 3; each has a line in
/// `docs/probes-needed.md`, "Builtins refused until a probe pins them").
pub fn refusal(id: &str) -> Option<&'static str> {
    Some(match id {
        "System.GetDotNetType#1" => {
            "GetDotNetType is in no corpus test, so the .NET type value it returns, and what a cast or a comparison with it does, are not pinned"
        }
        "Xmlport.Run#1" | "Xmlport.Run#2" => {
            "Xmlport.Run is in no corpus test, so where a run without a stream writes or reads (a file on the service tier a test cannot read) and what the request window does are not pinned"
        }
        "System.GetLastErrorText#2" => {
            "GetLastErrorText(ExcludeCustomerContent) is in no corpus test, so what the flag removes from the text of an error (and whether it changes the text of one that carries no customer content) is not pinned"
        }
        "Media.ImportFile#1" | "MediaSet.ImportFile#1" => {
            "Media.ImportFile and MediaSet.ImportFile read a file of the service tier and are in no corpus test, so which file a test can name, the error for a missing file and what the Tenant Media row holds are not pinned"
        }
        "RecordRef.Copy#1" | "RecordRef.Copy#2" => {
            "RecordRef.Copy is in no corpus test, so what it copies (the buffer, the filters, the key, the table view) and what ShareTable changes are not pinned"
        }
        _ => return None,
    })
}

fn all() -> impl Iterator<Item = &'static (&'static str, NativeFn)> {
    REGISTRY
        .iter()
        .chain(crate::collections::REGISTRY)
        .chain(crate::pagebuiltins::REGISTRY)
        .chain(crate::querybuiltins::REGISTRY)
        .chain(crate::refs::REGISTRY)
        .chain(crate::refs::RECORD_REGISTRY)
        .chain(crate::datefns::REGISTRY)
        .chain(crate::mathfns::REGISTRY)
        .chain(crate::sessionfns::REGISTRY)
        .chain(crate::media::REGISTRY)
        .chain(crate::moduleinfo::REGISTRY)
}

/// Implementations chosen per catalog signature instead of being listed by id
/// (families with many near-identical overloads: JSON, XML, streams). Built once
/// from the catalog, so a lookup is a hash probe.
fn derived() -> &'static std::collections::HashMap<String, NativeFn> {
    static TABLE: std::sync::OnceLock<std::collections::HashMap<String, NativeFn>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let cat = crate::catalog::Catalog::load();
        cat.signatures
            .iter()
            .filter_map(|sig| {
                crate::json::native_for(sig)
                    .or_else(|| crate::xml::native_for(sig))
                    .or_else(|| crate::stream::native_for(sig))
                    .or_else(|| crate::file::native_for(sig))
                    .or_else(|| crate::textfns::native_for(sig))
                    .or_else(|| crate::wsac::native_for(sig))
                    .or_else(|| crate::errorinfo::native_for(sig))
                    .or_else(|| crate::version::native_for(sig))
                    .or_else(|| crate::datatransfer::native_for(sig))
                    .or_else(|| crate::moduleinfo::native_for(sig))
                    .or_else(|| crate::sessiontypes::native_for(sig))
                    .or_else(|| crate::http::native_for(sig))
                    .map(|f| (cat.id(sig), f))
            })
            .collect()
    })
}

/// The implementation registered for overload `id`, if any.
pub fn lookup(id: &str) -> Option<NativeFn> {
    all()
        .find(|(i, _)| *i == id)
        .map(|(_, f)| *f)
        .or_else(|| derived().get(id).copied())
        .or_else(|| crate::platform_native::lookup(id))
}

/// Every registered id, for the conformance test.
pub fn registered_ids() -> Vec<&'static str> {
    all()
        .map(|(i, _)| *i)
        .chain(derived().keys().map(String::as_str))
        .collect()
}

fn vm_only(_: &mut dyn Ctx, _: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    unsupported("a builtin the VM runs itself")
}

fn unsupported<T>(what: &str) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.to_string()))
}

fn int_arg(args: &[Value], i: usize) -> Option<i64> {
    match args.get(i)? {
        Value::Integer(n) => Some(i64::from(*n)),
        Value::BigInteger(n) => Some(*n),
        Value::Byte(n) => Some(i64::from(*n)),
        Value::Char(n) => Some(i64::from(*n)),
        _ => None,
    }
}

/// Stands for a call stack that exists in BC but is not tracked here (a private-use character).
pub const UNTRACKED_CALL_STACK: &str = "\u{E000}";

/// Stands for the text and code of an error BC raises but whose wording the corpus does not pin.
/// `GetLastErrorText` and `GetLastErrorCode` refuse to answer while it is the last error, so a
/// test that reads it is not run, and one that only needs the call to fail still passes.
pub const UNPINNED_ERROR: &str = "\u{E001}";

/// The last error to set when a call fails with an error whose text is not pinned.
pub fn unpinned_error() -> AlError {
    AlError {
        message: UNPINNED_ERROR.into(),
        code: UNPINNED_ERROR.into(),
        call_stack: String::new(),
    }
}

/// Text of a Text/Code/Char argument (Text parameters accept all three). Text that holds the
/// untracked call stack marker is refused, so its content is never silently wrong.
/// `Database.SID(UserName)`: a Windows-style security identifier as Text. Probed on BC 27.5 (corpus
/// branch al-runtime-rs-staging, ae3fe88, tests/al-language-onprem/session/PXDatabaseSid.al:31-46):
/// `S-1-5-21-<a>-<b>-<c>-<rid>`, eight dash-separated parts, all but the first numeric (each read by
/// `Evaluate` into an Integer), the same for the same name, not the text of `UserSecurityId()`, and
/// a SID of that shape for a name no user has. The numbers are the machine's and are not pinned:
/// the domain part is fixed and the rid is a stable hash of the name (docs/probes-needed.md).
/// Without a name, which user is meant is not pinned: refused.
fn database_sid(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let Some(name) = args.first().and_then(text_of) else {
        return unsupported("Database.SID without a user name (whose SID it answers is not pinned)");
    };
    crate::assumptions::assume(crate::assumptions::Assumption::DatabaseSidNumbers);
    // FNV-1a over the name's UTF-8 bytes: the same on every platform and run.
    let hash = name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    let rid = 1000 + hash % (i32::MAX as u64 - 1000);
    Ok(Value::Text(format!("S-1-5-21-1004336348-1177238915-682003330-{rid}")))
}

pub fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::Text(s) | Value::Code(s) if s.contains(UNTRACKED_CALL_STACK) => None,
        Value::Text(s) | Value::Code(s) => Some(s.clone()),
        Value::Char(c) => char::from_u32(u32::from(*c)).map(String::from),
        _ => None,
    }
}

fn text_arg(args: &[Value], i: usize) -> Result<String, Signal> {
    // A Guid where a Text is expected is its text form, as in an assignment (`CopyStr(CreateGuid(), 1, 50)` in
    // Microsoft's Match General Jnl Lines UT, 61 tests; unprobed, docs/probes-needed.md "A Guid assigned to a Text
    // or Code"; hard rule 3 exception, owner 2026-10-15).
    if let Some(Value::Guid(g)) = args.get(i) {
        return Ok(crate::format::format_guid(g));
    }
    args.get(i).and_then(text_of).map_or_else(
        || unsupported(&format!("text argument {i} of type {}", args.get(i).describe())),
        Ok,
    )
}

fn format_1(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let len = int_arg(args, 1).unwrap_or(0) as i32;
    let std = int_arg(args, 2).unwrap_or(0) as i32;
    // The text of a DotNet object is its ToString(); the length and the format number change
    // nothing for the numbers 0 and 2 (tests/al-language-onprem-dotnet/TestDotNetValueKindsContracts.al,
    // codeunit 61400, FormatOfDotNetObjects_IsToString). Other format numbers are not pinned.
    if matches!(&args[0], Value::DotNet(_)) && matches!(std, 0 | 2) {
        if let Some(r) = ctx.format_dotnet(&args[0]) {
            return r.map(Value::Text);
        }
    }
    // A record as a whole, a RecordRef and a FieldRef need the table's schema.
    let composite = matches!(&args[0], Value::Record(_))
        || matches!(&args[0], Value::Library(n, _) if n == "RecordRef" || n == "FieldRef");
    if composite && std == 0 {
        if let Some(r) = ctx.format_composite(&args[0]) {
            return r.map(|t| Value::Text(crate::format::pad(t, len)));
        }
    }
    format_value(&args[0], len, std, ctx)
        .map(Value::Text)
        .map_err(|f| match f {
            FormatFail::Unsupported(s) => Signal::Unsupported(s),
            FormatFail::Error { message, code } => Signal::Error(AlError {
                message,
                code,
                call_stack: String::new(),
            }),
        })
}

/// `Format(Value, Length, FormatString)`: see [`crate::formatstr`].
fn format_2(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let len = int_arg(args, 1).unwrap_or(0) as i32;
    let template = text_arg(args, 2)?;
    crate::formatstr::format_with_string(&args[0], len, &template, ctx)
        .map(Value::Text)
        .map_err(|e| match e.strip_prefix(crate::formatstr::INVALID_FORMAT_MARK) {
            Some(template) => Signal::Error(AlError {
                message: format!(
                    "An invalid field or attribute has been specified for the 'Format' property.\r\\\r\\{template}"
                ),
                code: "InvalidFormatString".into(),
                call_stack: String::new(),
            }),
            None => Signal::Unsupported(e),
        })
}

fn abs(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    Ok(match &args[0] {
        Value::Decimal(d) => Value::Decimal(d.abs()),
        Value::Integer(i) => Value::Decimal(i.unsigned_abs().into()),
        Value::BigInteger(i) => Value::Decimal(i.unsigned_abs().into()),
        other => return unsupported(&format!("Abs of {}", other.describe())),
    })
}

fn get_last_error_text(ctx: &mut dyn Ctx, _: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    let e = ctx.last_error();
    if e.message == UNPINNED_ERROR {
        return unsupported("GetLastErrorText of an error whose text is not pinned");
    }
    Ok(Value::Text(e.message))
}

fn get_last_error_code(ctx: &mut dyn Ctx, _: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    let e = ctx.last_error();
    if e.code == UNPINNED_ERROR {
        return unsupported("GetLastErrorCode of an error whose text is not pinned");
    }
    Ok(Value::Text(e.code))
}

fn get_last_error_call_stack(ctx: &mut dyn Ctx, _: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    let e = ctx.last_error();
    if e.call_stack.is_empty() && !(e.message.is_empty() && e.code.is_empty()) {
        // BC returns the AL frames of the error (codeunit/TestCodeunitAlCallStack.al). The VM adds
        // them as the error unwinds (al-vm/src/callstack.rs), so an error with no frame at all
        // (raised outside any AL routine) is the only one that gets here. The answer is non-empty,
        // as in BC, but its content is a marker that every text function refuses to read
        // ([`text_of`]), so a test that inspects the frames is not run while one that only calls
        // the function (error-handling/TestGetLastError.al) still passes.
        return Ok(Value::Text(UNTRACKED_CALL_STACK.to_string()));
    }
    if e.call_stack.is_empty() {
        return Ok(Value::Text(String::new()));
    }
    // The frames are separated by a backslash and the text ends with one, the runner's frames
    // below the test's (session/TestSessionInformationCallstackContracts.al:200-218).
    let mut out = String::with_capacity(e.call_stack.len() + 64);
    for frame in e.call_stack.split('\n') {
        out.push_str(frame);
        out.push('\\');
    }
    out.push_str(&ctx.runner_call_stack());
    Ok(Value::Text(out))
}

fn clear_last_error(ctx: &mut dyn Ctx, _: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    ctx.set_last_error(AlError::default());
    Ok(Value::Null)
}

/// `Error(Text, Args...)`. The code is `Dialog` (probes-needed: Microsoft's
/// test libraries and the corpus Assert's DialogTxt use it; no corpus test
/// pins it directly).
fn error_text(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let template = text_arg(args, 0)?;
    let message = substitute(&template, &args[1..], ctx)?;
    Err(Signal::Error(AlError {
        message,
        code: "Dialog".into(),
        call_stack: String::new(),
    }))
}

/// `Message(String, Values...)`: the text is substituted like `StrSubstNo`
/// before it reaches the UI (corpus handlers/TestMessageHandler.al
/// Message_FormattedString_HandlerGetsFormatted).
fn message(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let text = substitute(&text_arg(args, 0)?, &args[1..], ctx)?;
    ctx.ui_message(text)?;
    Ok(Value::Null)
}

/// `Confirm(String, Default, Values...)`. Values are not substituted into the
/// question a handler sees (corpus handlers/TestConfirmFormatArgs.al). An
/// omitted Default is false (docs/probes-needed.md).
fn confirm(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let question = text_arg(args, 0)?;
    let default = match args.get(1) {
        None => false,
        Some(Value::Boolean(b)) => *b,
        Some(other) => return unsupported(&format!("Confirm default {}", other.describe())),
    };
    Ok(Value::Boolean(ctx.ui_confirm(question, default)?))
}

/// `StrMenu(Options, Default, Instruction)`. An omitted Default is 1 and an
/// omitted Instruction is empty (docs/probes-needed.md).
fn str_menu(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let options = text_arg(args, 0)?;
    let default = match args.get(1) {
        None => 1,
        Some(_) => int_arg(args, 1).map_or_else(|| unsupported("StrMenu default"), |n| Ok(n as i32))?,
    };
    let instruction = match args.get(2) {
        None => String::new(),
        Some(_) => text_arg(args, 2)?,
    };
    Ok(Value::Integer(ctx.ui_str_menu(options, default, instruction)?))
}

/// `%1`..`%9` replaced by `Format(arg)`. A placeholder without an argument
/// and `#n` fields are not pinned by the corpus yet: unsupported.
pub fn substitute(template: &str, args: &[Value], ctx: &dyn FormatCtx) -> Result<String, Signal> {
    let mut out = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            if let Some(d) = chars.peek().and_then(|d| d.to_digit(10)).filter(|d| *d >= 1) {
                chars.next();
                match args.get(d as usize - 1) {
                    Some(v) => out.push_str(&try_format(v, 0, 0, ctx).map_err(Signal::Unsupported)?),
                    None => return unsupported("StrSubstNo placeholder without an argument"),
                }
                continue;
            }
        }
        out.push(c);
    }
    Ok(out)
}

fn strlen(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    Ok(Value::Integer(Value::text_len(&text_arg(args, 0)?) as i32))
}

fn strpos(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let (s, sub) = (text_arg(args, 0)?, text_arg(args, 1)?);
    // text/TestTextProcessingContracts.al StrPos_EmptySubstring_ReturnsZero.
    if sub.is_empty() {
        return Ok(Value::Integer(0));
    }
    // A match at a char boundary is the same match unit by unit; the position counts UTF-16 units. No copy of the
    // text: a loop that calls StrPos on a long text (Pageworks' asset parsing) would copy it each time.
    let pos = s.find(sub.as_str()).map_or(0, |b| {
        if s.is_ascii() {
            b + 1
        } else {
            s[..b].encode_utf16().count() + 1
        }
    });
    Ok(Value::Integer(pos as i32))
}

/// `Session.LogMessage`, `Session.LogSecurityAudit` and `Session.LogAuditMessage`: telemetry and the security audit log have
/// no effect AL code can see in a test, so the call returns (session/TestSessionExtended.al:76-89
/// only needs LogMessage callable; session/TestEnvironmentUserTenantContracts.al:315-374 runs
/// Change Log Management, which logs a security audit entry, and reads only the change log).
fn log_message(_: &mut dyn Ctx, _: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    Ok(Value::Null)
}

/// `Hyperlink(Url)` asks the client to open a page. A test has no handler for it, so the platform raises
/// `Unhandled UI: Hyperlink <url>` (record/ProbeClearAllSetKeyHyperlink.al:322-338, BC 27.5); a routine
/// that only mentions the call still runs.
fn hyperlink(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let url = text_arg(args, 0)?;
    Err(Signal::Error(AlError {
        message: format!("Unhandled UI: Hyperlink {url}"),
        code: "MissingUIHandler".into(),
        call_stack: String::new(),
    }))
}

/// `Variant2Date(Variant)`: the Date a Variant holds (session/TestFinalCoverage.al:79-88). Another
/// content raises a conversion error whose text is not pinned: refused.
fn variant_2_date(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    match args.first() {
        Some(d @ Value::Date(..)) => Ok(d.clone()),
        other => unsupported(&format!(
            "Variant2Date of {other} (the conversion error is not pinned)",
            other = other.describe()
        )),
    }
}

/// `Variant2Time(Variant)`: the Time a Variant holds (session/TestFinalCoverage.al:91-100).
fn variant_2_time(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    match args.first() {
        Some(t @ Value::Time(_)) => Ok(t.clone()),
        other => unsupported(&format!(
            "Variant2Time of {other} (the conversion error is not pinned)",
            other = other.describe()
        )),
    }
}

/// `StartSession`, every overload. While an app installs it starts nothing, leaves `SessionId` alone
/// and answers false (or raises nothing when the result is ignored): install/TestInstallStartSession_Tests.al,
/// ALSession.ALStartSessionAsyncImpl. In a test run under TestIsolation = Codeunit it raises the
/// refusal below, also for the overload with a record, and `SessionId` stays 0
/// (session/TestStartSessionRecord.al:StartSession_FromATestCodeunit_IsRefusedUnderCodeunitIsolation).
/// Anywhere else a session would start, which this runtime does not do: refused.
fn start_session(c: &mut dyn Ctx, _: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    if c.install_scope() {
        return Ok(if c.ret_used() {
            Value::Boolean(false)
        } else {
            Value::Null
        });
    }
    if c.in_test_run() {
        return Err(Signal::Error(AlError {
            message: "Sessions can only be started in tests that are run by a TestRunner that has TestIsolation set to Disabled."
                .into(),
            code: String::new(),
            call_stack: String::new(),
        }));
    }
    unsupported("StartSession outside an install trigger and a test run (a second session is not modelled)")
}

/// `StrCheckSum(String [, WeightString] [, Modulus])`: `(M - (sum of digit * weight) mod M) mod M`, the
/// formula of the documentation (methods-auto/text/text-strchecksum-method.md, its example gives 0
/// for '4378' with weights '1234' and modulus 7). The corpus pins only that the result is not
/// negative, is below the modulus and is not 0 for '12345' (session/TestSystemExtended.al:218-256);
/// values and the four errors: types/TestProbeScalarContracts.al:25-123.
fn strchecksum(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let digits = |s: &str| -> Option<Vec<i64>> { s.chars().map(|c| c.to_digit(10).map(i64::from)).collect() };
    let source = text_arg(args, 0)?;
    let Some(source) = digits(&source) else {
        return raise("The value string must be numeric.", "ALSystemString");
    };
    let mut weights = match args.get(1) {
        Some(_) => {
            let Some(w) = digits(&text_arg(args, 1)?) else {
                return raise("The weight string must be numeric.", "ALSystemString");
            };
            w
        }
        None => Vec::new(),
    };
    // An empty source is 0 whatever the weights (types/TestProbeScalarContracts.al:StrCheckSum_Values, `empty_w1`).
    if source.is_empty() {
        return Ok(Value::Integer(0));
    }
    if weights.len() > source.len() {
        return raise(
            "The weight parameter cannot be longer than the value string.",
            "ALSystemString",
        );
    }
    // A short weight string is padded with 1 up to the length of the source.
    weights.resize(source.len(), 1);
    let modulus = int_arg(args, 2).unwrap_or(10);
    if modulus < 1 {
        return raise("The modulus parameter cannot be less than 1.", "ALSystemString");
    }
    if source.is_empty() {
        return Ok(Value::Integer(0));
    }
    let sum: i64 = source.iter().zip(&weights).map(|(d, w)| d * w).sum();
    Ok(Value::Integer(((modulus - sum % modulus) % modulus) as i32))
}

fn copystr(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let whole = text_arg(args, 0)?;
    copystr_of(&whole, int_arg(args, 1).unwrap_or(1), int_arg(args, 2))
}

/// `CopyStr` of a text the caller holds by reference (the VM's fast path reads a variable without copying it).
pub fn copystr_of(whole: &str, pos: i64, len: Option<i64>) -> Result<Value, Signal> {
    // A text of ASCII only is indexed by byte, with no copy: a loop of CopyStr over a long text stays linear in
    // the loop (Pageworks' asset parsing).
    let ascii = whole.is_ascii();
    let s: Vec<u16> = if ascii {
        Vec::new()
    } else {
        whole.encode_utf16().collect()
    };
    let total = if ascii { whole.len() } else { s.len() };
    // text/TestProbeTextDateFormatContracts.al:Text_SubstringBounds, `copystr_start0`, `copystr_neg_count`.
    if pos < 1 {
        return raise(
            format!(
                "The value of COPYSTR parameter 2 is outside of the permitted range. The current value is: {pos}. The permitted range is: from 1 to 2,147,483,647."
            ),
            "OutsidePermittedRange",
        );
    }
    let start = (pos as usize - 1).min(total);
    let end = match len {
        Some(len) if len < 0 => {
            return raise(
                format!(
                    "The value of COPYSTR parameter 3 is outside of the permitted range. The current value is: {len}. The permitted range is: from 0 to 2,147,483,647."
                ),
                "OutsidePermittedRange",
            );
        }
        Some(len) => (start + len as usize).min(total),
        None => total,
    };
    if ascii {
        return Ok(Value::Text(whole[start..end].to_string()));
    }
    Ok(Value::Text(String::from_utf16_lossy(&s[start..end])))
}

fn strsubstno(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let template = text_arg(args, 0)?;
    Ok(Value::Text(substitute(&template, &args[1..], ctx)?))
}

fn uppercase(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    Ok(Value::Text(text_arg(args, 0)?.to_uppercase()))
}

fn lowercase(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    Ok(Value::Text(text_arg(args, 0)?.to_lowercase()))
}

/// `Clear(var X)`: the default of the value's own kind. Records go to the VM.
fn clear(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let v = &mut args[0];
    *v = match v {
        Value::Record(_) => return ctx.record("Clear", Some(v), &mut []),
        // Clearing a RecordRef/FieldRef/KeyRef closes it: a fresh, unopened handle.
        Value::Library(name, _)
            if matches!(
                name.as_str(),
                "RecordRef"
                    | "FieldRef"
                    | "KeyRef"
                    | "RecordId"
                    | "WebServiceActionContext"
                    | "SessionSettings"
                    | "Notification"
                    | "JsonObject"
                    | "JsonArray"
                    | "JsonValue"
                    | "JsonToken"
            ) =>
        {
            Value::Library(name.clone(), Default::default())
        }
        // Clear empties the controls and leaves an assigned page caption alone
        // (filterpage/TestFilterPageBuilder.al:FilterPageBuilder_Clear_KeepsTheAssignedCaption).
        Value::Library(name, cell) if name == "FilterPageBuilder" => {
            let caption = match &*cell.borrow() {
                crate::value::LibraryValue::FilterPageBuilder(s) => s.caption.clone(),
                _ => None,
            };
            let state = crate::value::LibraryValue::FilterPageBuilder(crate::value::FilterPageState {
                controls: Vec::new(),
                caption,
            });
            Value::Library(name.clone(), std::rc::Rc::new(std::cell::RefCell::new(state)))
        }
        Value::Boolean(_) => Value::Boolean(false),
        Value::Integer(_) => Value::Integer(0),
        Value::BigInteger(_) => Value::BigInteger(0),
        Value::Decimal(_) => Value::Decimal(Default::default()),
        Value::Byte(_) => Value::Byte(0),
        Value::Char(_) => Value::Char(0),
        Value::Text(_) => Value::Text(String::new()),
        Value::Code(_) => Value::Code(String::new()),
        Value::Date(..) => Value::Date(0, false),
        Value::Time(_) => Value::Time(0),
        Value::DateTime(_) => Value::DateTime(0),
        Value::Duration(_) => Value::Duration(0),
        Value::Guid(_) => Value::Guid([0; 16]),
        // The empty formula (types/TestDateFormulaDurationFormatContracts.al:38-42).
        Value::DateFormula(_) => Value::DateFormula(String::new()),
        Value::Option(_, m) => Value::Option(0, m.clone()),
        // A cleared List or Dictionary is a new, empty one (types/TestClearContracts.al:130-150,
        // collections/TestListByRefContracts.al:155 through a `var` parameter).
        Value::List(_) => Value::List(Default::default()),
        Value::Dictionary(_) => Value::Dictionary(Default::default()),
        // A cleared Blob is a new, empty buffer (streams/TestBlobCalcFieldsUncommittedWrite.al).
        Value::Blob(_) => Value::Blob(crate::stream::new_blob()),
        // A cleared TestPage or Page variable is a fresh, closed one (corpus
        // handlers/TestPageEditable_Tests.al:93 EditableBoundToPageVariable_FollowsTheRow). A page
        // that is open, waits for Trap(), or is a part is not cleared: nothing pins what follows.
        Value::TestPage(st) => {
            let object = {
                let s = st.borrow();
                if s.open || s.trapping || s.host.is_some() {
                    return unsupported("Clear of a TestPage that is open, trapping or a part (not pinned)");
                }
                s.object
            };
            Value::TestPage(std::rc::Rc::new(std::cell::RefCell::new(
                crate::value::PageState::closed(object),
            )))
        }
        // A cleared Query is a fresh one: no filters, no row cap, not open
        // (query/TestQueryRuntimeContracts.al:Query_GetFilters_ThreeFilters_OrderIsNotColumnOrder).
        Value::Query(q) => {
            let object = q.borrow().object;
            Value::Query(Rc::new(std::cell::RefCell::new(crate::value::QueryState {
                object,
                filters: Vec::new(),
                top: None,
                security_filtering: None,
                run: None,
                instance: None,
            })))
        }
        // A codeunit or report variable nothing has used yet has no instance to release, so
        // clearing it changes nothing (handlers/TestReportRunWithRequestPage.al: `Clear(Probe)`
        // before `Probe.Run()`). Clearing one in use gives a fresh instance: after `Clear(TempBlob)` the
        // blob is empty again (streams/TestStreamsRuntimeContracts.al:WriteText_ExplicitEncodings_Bytes).
        Value::Codeunit(rc) if !rc.borrow().initialized => return Ok(Value::Null),
        Value::Codeunit(rc) => {
            let object = rc.borrow().object;
            Value::Codeunit(std::rc::Rc::new(std::cell::RefCell::new(crate::value::Instance {
                object,
                ..Default::default()
            })))
        }
        // An array goes back to its initial value element by element, in place: a `var` parameter
        // that shares it sees the change (docs/probes-needed.md; the oracle runs
        // pageupdate/PageUpdateFacts.Page.al:58 `Clear(Derived)` on an array of Decimal).
        Value::Array(a) => {
            let a = a.clone();
            for e in a.borrow_mut().iter_mut() {
                clear(ctx, None, std::slice::from_mut(e))?;
            }
            return Ok(Value::Null);
        }
        // An unassigned DotNet variable holds nothing, so clearing it changes nothing. One that holds
        // an object is refused: whether BC calls Dispose on it, and so whether a stream the object
        // wraps is flushed, is not pinned (docs/probes-needed.md, "Clear of a DotNet variable").
        Value::Null => return Ok(Value::Null),
        Value::DotNet(o) => {
            let ty = match &o.kind {
                crate::value::DotNetKind::Host { clr_type, .. } => clr_type.as_str(),
                crate::value::DotNetKind::Enum { clr_type, .. } => clr_type.as_str(),
                crate::value::DotNetKind::Internal { type_name, .. } => type_name.as_str(),
            };
            return unsupported(&format!(
                "Clear of a DotNet variable that holds a {ty}: whether BC disposes the object is not pinned"
            ));
        }
        // A cleared stream is one that was never created again (streams/TestProbeStreamContracts.al:RecordAssign_CopiesBlobBytes).
        Value::Library(name, _) if matches!(name.as_str(), "OutStream" | "InStream" | "FileUpload") => Value::Library(
            name.clone(),
            std::rc::Rc::new(std::cell::RefCell::new(crate::value::LibraryValue::Empty)),
        ),
        other => return unsupported(&format!("Clear of a {} value", value_kind(other))),
    };
    Ok(Value::Null)
}

fn context_value(c: &dyn Ctx) -> Value {
    let members: std::sync::Arc<[String]> = crate::sysenum::option_members("ExecutionContext")
        .expect("a system option")
        .into();
    // Install inside an install codeunit, Normal anywhere else (install/TestInstallEnvVisible_Tests.al:90-127).
    Value::Option(i32::from(c.install_scope()), members)
}

fn execution_context(c: &mut dyn Ctx, _: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    Ok(context_value(c))
}

/// Of this app (no argument) or of an app of the workspace; another app's context while an
/// installer runs is not pinned.
fn module_execution_context(c: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Result<Value, Signal> {
    if let Some(id) = a.first() {
        if c.install_scope() {
            return unsupported("GetModuleExecutionContext(AppId) inside install code (not pinned)");
        }
        let Value::Guid(g) = id else {
            return unsupported(&format!("GetModuleExecutionContext with {}", id.describe()));
        };
        if !matches!(c.module_by_id(g), ModuleLookup::Found(_)) {
            return unsupported("GetModuleExecutionContext of an app the workspace does not hold (not pinned)");
        }
    }
    Ok(context_value(c))
}

/// `Enum.AsInteger()`: the ordinal (types/TestEnum.al:27-40).
fn enum_as_integer(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    match r {
        Some(Value::Enum(_, o)) => Ok(Value::Integer(*o)),
        other => unsupported(&format!("AsInteger of {}", other.describe())),
    }
}

/// The enum a type-level call (`"Name".Names()`) or a call on a variable names: lowering passes
/// the enum object as the receiver value.
fn enum_receiver(ctx: &dyn Ctx, r: Option<&mut Value>, what: &str) -> Result<(u32, Vec<(i32, String)>), Signal> {
    let Some(Value::Enum(id, _)) = r else {
        return unsupported(&format!("{what} without an enum receiver"));
    };
    let id = *id;
    ctx.enum_members(id).map(|m| (id, m)).map_or_else(
        || unsupported(&format!("{what}: this context has no enum metadata")),
        Ok,
    )
}

fn list_of(items: Vec<Value>) -> Value {
    Value::List(Rc::new(std::cell::RefCell::new(items)))
}

/// `Enum.Names()`: every declared name, the blank ordinal included
/// (types/TestEnumNamesContracts.al:19-60). The order is the declaration order, which matches
/// `Ordinals()` position by position; BC's order is not pinned (docs/probes-needed.md).
fn enum_names(ctx: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    let (_, members) = enum_receiver(ctx, r, "Enum.Names")?;
    Ok(list_of(members.into_iter().map(|(_, n)| Value::Text(n)).collect()))
}

/// `Enum.Ordinals()` (session/TestSystemExtended.al:381-390, types/TestLanguageEdgeCases.al:91-105).
fn enum_ordinals(ctx: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    let (_, members) = enum_receiver(ctx, r, "Enum.Ordinals")?;
    Ok(list_of(members.into_iter().map(|(o, _)| Value::Integer(o)).collect()))
}

/// `Enum.FromInteger(n)`: any number is carried, declared or not (types/TestEnum.al:83-91,
/// types/TestEnumDefaultImplTests.Codeunit.al:70).
fn enum_from_integer(ctx: &mut dyn Ctx, r: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let (id, _) = enum_receiver(ctx, r, "Enum.FromInteger")?;
    // An Option argument converts to its ordinal, as any Integer parameter takes it.
    let n = match args.first() {
        Some(Value::Option(o, _)) => Some(*o),
        _ => int_arg(args, 0).and_then(|n| i32::try_from(n).ok()),
    };
    match n {
        Some(n) => Ok(Value::Enum(id, n)),
        None => unsupported("Enum.FromInteger with a number outside the Integer range"),
    }
}

fn is(receiver: Option<&mut Value>, kind: &str) -> Result<Value, Signal> {
    Ok(Value::Boolean(receiver.is_some_and(|v| v.is(kind))))
}

/// The kind of a value for a refusal reason: the variant name, never its contents.
fn value_kind(v: &Value) -> String {
    let d = v.describe();
    d.split(|c: char| !c.is_alphanumeric()).next().unwrap_or("").to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    // TestField with an ErrorInfo is implemented (issue 41): a check that holds raises nothing, a failing one
    // is refused at run time (al-vm recmeta.rs `test_field`, al-vm/tests/scm_service_41.rs). Issue 42 refused
    // it at lowering; that entry is gone because `lookup` now finds the ids.
    #[test]
    fn test_field_with_an_error_info_is_implemented_not_refused_at_lowering() {
        for id in ["Record.TestField#2", "Record.TestField#4"] {
            assert!(refusal(id).is_none(), "{id}");
            assert!(lookup(id).is_some(), "{id}");
        }
    }

    // Base Application's Web Request Helper casts a caught .NET exception with `GetDotNetType(WebException)`
    // after a web request failed; no corpus test calls GetDotNetType (issue 34).
    #[test]
    fn get_dot_net_type_is_refused_with_its_reason() {
        let why = refusal("System.GetDotNetType#1").unwrap_or_default();
        assert!(why.contains("GetDotNetType") && why.contains("not pinned"), "{why}");
    }

    #[derive(Default)]
    pub(crate) struct TestCtx {
        err: AlError,
        install: bool,
        in_test: bool,
        result_ignored: bool,
    }
    impl FormatCtx for TestCtx {
        fn enum_name(&self, _id: u32, o: i32) -> String {
            format!("E{o}")
        }
    }
    impl Ctx for TestCtx {
        fn last_error(&self) -> AlError {
            self.err.clone()
        }
        fn set_last_error(&mut self, e: AlError) {
            self.err = e;
        }
        fn record(&mut self, m: &str, _: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
            Err(Signal::Unsupported(m.into()))
        }
        fn commit(&mut self) -> Result<(), Signal> {
            Ok(())
        }
        fn new_guid(&mut self) -> [u8; 16] {
            [1; 16]
        }
        fn install_scope(&self) -> bool {
            self.install
        }
        fn in_test_run(&self) -> bool {
            self.in_test
        }
        fn ret_used(&self) -> bool {
            !self.result_ignored
        }
    }

    #[test]
    fn variant_conversions_and_log_message() {
        assert!(matches!(
            call("System.Variant2Date#1", vec![Value::Date(5, false)]),
            Ok(Value::Date(5, false))
        ));
        assert!(matches!(
            call("System.Variant2Time#1", vec![Value::Time(7)]),
            Ok(Value::Time(7))
        ));
        assert!(matches!(
            call("System.Variant2Date#1", vec![Value::Integer(1)]),
            Err(Signal::Unsupported(_))
        ));
        assert!(matches!(
            call("Session.LogMessage#2", vec![t("E"), t("m")]),
            Ok(Value::Null)
        ));
        // The security audit log has no effect AL can see, like LogSecurityAudit.
        assert!(matches!(
            call(
                "Session.LogAuditMessage#1",
                vec![
                    t("D"),
                    Value::Integer(0),
                    Value::Integer(0),
                    Value::Integer(1),
                    Value::Integer(0)
                ]
            ),
            Ok(Value::Null)
        ));
    }

    #[test]
    fn start_session_answers_false_in_install_and_is_refused_in_a_test_run() {
        let run = |ctx: &mut TestCtx| {
            let mut args = vec![Value::Integer(777), Value::Integer(60448)];
            lookup("Session.StartSession#1").unwrap()(ctx, None, &mut args)
        };
        // install/TestInstallStartSession_Tests.al:51-65
        let mut install = TestCtx {
            install: true,
            ..Default::default()
        };
        assert!(matches!(run(&mut install), Ok(Value::Boolean(false))));
        let mut plain = TestCtx {
            install: true,
            result_ignored: true,
            ..Default::default()
        };
        assert!(matches!(run(&mut plain), Ok(Value::Null)));
        // session/TestStartSessionRecord.al:57-66
        let mut test = TestCtx {
            in_test: true,
            ..Default::default()
        };
        assert!(matches!(run(&mut test), Err(Signal::Error(e)) if e.message.contains(
            "can only be started in tests that are run by a TestRunner that has TestIsolation set to Disabled")));
        assert!(matches!(run(&mut TestCtx::default()), Err(Signal::Unsupported(_))));
    }

    pub(crate) fn call(id: &str, args: Vec<Value>) -> Result<Value, Signal> {
        let f = lookup(id).unwrap_or_else(|| panic!("{id} not registered"));
        let mut args = args;
        f(&mut TestCtx::default(), None, &mut args)
    }
    pub(crate) fn t(s: &str) -> Value {
        Value::Text(s.into())
    }
    fn text(r: Result<Value, Signal>) -> String {
        match r {
            Ok(Value::Text(s)) => s,
            other => panic!("{other:?}"),
        }
    }
    fn int(r: Result<Value, Signal>) -> i32 {
        match r {
            Ok(Value::Integer(i)) => i,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn every_registered_id_is_in_the_catalog() {
        let cat = crate::catalog::Catalog::load();
        let ids: Vec<String> = cat.signatures.iter().map(|s| cat.id(s)).collect();
        for id in registered_ids() {
            assert!(ids.iter().any(|i| i == id), "{id} is not a catalog overload id");
        }
    }

    #[test]
    fn format_with_length_and_standard_format() {
        assert_eq!(text(call("System.Format#1", vec![Value::Integer(42)])), "42");
        assert_eq!(
            text(call("System.Format#1", vec![Value::Integer(42), Value::Integer(5)])),
            "   42"
        );
        assert_eq!(
            text(call(
                "System.Format#1",
                vec![Value::Boolean(true), Value::Integer(0), Value::Integer(2)]
            )),
            "1"
        );
    }

    // A standard format number a type does not have raises the pinned error, not a panic that takes the whole codeunit down
    // (text/TestProbeTextDateFormatContracts.al:Dates_Format_Time, Dates_Format_DateTime, Dates_Format_Duration).
    #[test]
    fn format_number_a_type_does_not_have_raises_the_pinned_error() {
        for (v, ty) in [
            (Value::Duration(3_600_000), "Duration"),
            (Value::Time(43_200_001), "Time"),
            (Value::DateTime(5), "DateTime"),
        ] {
            let args = vec![v, Value::Integer(0), Value::Integer(7)];
            let Err(Signal::Error(e)) = call("System.Format#1", args) else {
                panic!("{ty}: format 7 must raise");
            };
            assert_eq!(
                (e.message.as_str(), e.code.as_str()),
                (
                    format!("Standard format number 7 does not exist for the type '{ty}'.").as_str(),
                    "InvalidStandardFormat"
                )
            );
        }
    }

    #[test]
    fn a_duration_format_number_that_does_not_exist_raises() {
        // text/TestProbeTextDateFormatContracts.al:Dates_Format_Duration, `duration_fmt_5`.
        let Err(Signal::Error(e)) = call(
            "System.Format#1",
            vec![Value::Duration(86_400_000), Value::Integer(0), Value::Integer(5)],
        ) else {
            panic!("format 5 of a Duration must raise");
        };
        assert_eq!(e.code, "InvalidStandardFormat");
    }

    #[test]
    fn the_call_stack_of_a_recorded_error_is_refused_not_invented() {
        // codeunit/TestCodeunitAlCallStack.al expects AL frames such as `(CodeUnit 60211)`. The VM
        // fills them in (al-vm/tests/call_stack.rs); an error that has no frame gets a marker no
        // text function reads.
        let mut ctx = TestCtx::default();
        let f = lookup("System.GetLastErrorCallStack#1").unwrap();
        assert!(matches!(f(&mut ctx, None, &mut []), Ok(Value::Text(ref s)) if s.is_empty()));
        ctx.set_last_error(AlError {
            message: "boom".into(),
            code: "Dialog".into(),
            call_stack: String::new(),
        });
        let Ok(stack @ Value::Text(_)) = f(&mut ctx, None, &mut []) else {
            panic!("a call stack is returned after an error")
        };
        assert!(matches!(&stack, Value::Text(s) if !s.is_empty()));
        assert!(text_of(&stack).is_none(), "its content cannot be inspected");
    }

    // session/TestSystemExtended.al:218-256 pins the range; the values are the documentation's formula.
    #[test]
    fn strchecksum_follows_the_documented_formula() {
        let n = |v: Result<Value, Signal>| int(v);
        assert_eq!(n(call("Text.StrCheckSum#1", vec![t("12345")])), 5);
        assert_eq!(n(call("Text.StrCheckSum#1", vec![t("12345"), t("5432")])), 5); // weights 54321: 35 mod 10 = 5
        assert_eq!(
            n(call(
                "Text.StrCheckSum#1",
                vec![t("12345"), t("5432"), Value::Integer(10)]
            )),
            5
        );
        // The example of the documentation: (7 - (4*1 + 3*2 + 7*3 + 8*4) mod 7) mod 7 = 0.
        assert_eq!(
            n(call(
                "Text.StrCheckSum#1",
                vec![t("4378"), t("1234"), Value::Integer(7)]
            )),
            0
        );
        assert_eq!(n(call("Text.StrCheckSum#1", vec![t("")])), 0);
        // types/TestProbeScalarContracts.al:StrCheckSum_NonDigitSource and the StrCheckSum tests after it.
        for (args, message) in [
            (vec![t("12a")], "The value string must be numeric."),
            (
                vec![t("12"), t("123")],
                "The weight parameter cannot be longer than the value string.",
            ),
        ] {
            let Err(Signal::Error(e)) = call("Text.StrCheckSum#1", args) else {
                panic!("{message}");
            };
            assert_eq!((e.message.as_str(), e.code.as_str()), (message, "ALSystemString"));
        }
    }

    #[test]
    fn strlen_counts_utf16() {
        assert_eq!(int(call("Text.StrLen#1", vec![t("abc")])), 3);
        assert_eq!(int(call("Text.StrLen#1", vec![t("😀")])), 2);
    }

    #[test]
    fn strpos_is_one_based_and_zero_when_absent() {
        assert_eq!(int(call("Text.StrPos#1", vec![t("hello"), t("ll")])), 3);
        assert_eq!(int(call("Text.StrPos#1", vec![t("hello"), t("x")])), 0);
    }

    #[test]
    fn copystr() {
        assert_eq!(
            text(call(
                "Text.CopyStr#1",
                vec![t("Hello World"), Value::Integer(1), Value::Integer(5)]
            )),
            "Hello"
        );
        assert_eq!(text(call("Text.CopyStr#1", vec![t("Hello"), Value::Integer(3)])), "llo");
        assert_eq!(
            text(call(
                "Text.CopyStr#1",
                vec![t("Hello"), Value::Integer(4), Value::Integer(10)]
            )),
            "lo"
        );
        // text/TestProbeTextDateFormatContracts.al:Text_SubstringBounds, `copystr_start0`.
        let Err(Signal::Error(e)) = call("Text.CopyStr#1", vec![t("Hello"), Value::Integer(0)]) else {
            panic!("CopyStr from position 0 must raise");
        };
        assert_eq!(e.code, "OutsidePermittedRange");
    }

    #[test]
    fn strsubstno_formats_its_arguments() {
        assert_eq!(
            text(call(
                "Text.StrSubstNo#1",
                vec![t("%1 and %2"), Value::Integer(7), Value::Boolean(true)]
            )),
            "7 and Yes"
        );
        assert_eq!(text(call("Text.StrSubstNo#1", vec![t("no args")])), "no args");
    }

    #[test]
    fn case_conversion() {
        assert_eq!(text(call("Text.UpperCase#1", vec![t("abc")])), "ABC");
        assert_eq!(text(call("Text.LowerCase#1", vec![t("ABC")])), "abc");
    }

    #[test]
    fn error_substitutes_and_raises() {
        let Err(Signal::Error(e)) = call("Dialog.Error#2", vec![t("bad %1"), Value::Integer(3)]) else {
            panic!()
        };
        assert_eq!(e.message, "bad 3");
    }

    #[test]
    fn last_error_round_trip() {
        let mut ctx = TestCtx::default();
        ctx.set_last_error(AlError {
            message: "boom".into(),
            code: "Dialog".into(),
            call_stack: String::new(),
        });
        let get = lookup("System.GetLastErrorText#1").unwrap();
        assert!(matches!(get(&mut ctx, None, &mut []), Ok(Value::Text(ref s)) if s == "boom"));
        let code = lookup("System.GetLastErrorCode#1").unwrap();
        assert!(matches!(code(&mut ctx, None, &mut []), Ok(Value::Text(ref s)) if s == "Dialog"));
        lookup("System.ClearLastError#1").unwrap()(&mut ctx, None, &mut []).unwrap();
        assert_eq!(ctx.last_error(), AlError::default());
    }

    #[test]
    fn abs() {
        let r = call("System.Abs#1", vec![Value::Decimal((-2).into())]);
        assert!(matches!(r, Ok(Value::Decimal(d)) if d == 2.into()));
    }

    #[test]
    fn variant_type_tests_use_the_receiver() {
        let f = lookup("Variant.IsInteger#1").unwrap();
        let mut v = Value::Integer(1);
        assert!(matches!(
            f(&mut TestCtx::default(), Some(&mut v), &mut []),
            Ok(Value::Boolean(true))
        ));
        let mut v = Value::Option(0, std::sync::Arc::from(vec![]));
        let f = lookup("Variant.IsOption#1").unwrap();
        assert!(matches!(
            f(&mut TestCtx::default(), Some(&mut v), &mut []),
            Ok(Value::Boolean(true))
        ));
    }
}
