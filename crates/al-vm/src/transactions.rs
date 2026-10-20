//! Transactions as AL code sees them: `Codeunit.Run` (guarded and unguarded), the test
//! method's `TransactionModel`, `Commit` under `AutoRollback` and `[CommitBehavior]`, and
//! `Database.IsInWriteTransaction`.
//!
//! What counts as "a write transaction is open" is "the store holds a change since the last
//! commit" (`MemStore::has_uncommitted`). Every rule below is pinned by a corpus test, cited
//! where it is applied; what the corpus does not pin is refused (`Signal::Unsupported`) and
//! listed in docs/probes-needed.md.

use crate::{AlError, Session, Signal};
use al_builtins::builtin_id::ids;
use al_builtins::record_op::RecordOp;
use al_builtins::value::Value;

/// The transaction model of the running test method (`[TransactionModel(...)]`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TxModel {
    /// The default: the test's work is committed when it passes.
    #[default]
    AutoCommit,
    /// The test's work is rolled back when it finishes; `Commit` is refused.
    AutoRollback,
    /// The test body runs with no transaction at all.
    None,
}

/// The `[CommitBehavior(...)]` in force for the dynamic extent of a procedure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum CommitBehavior {
    #[default]
    Allow,
    Ignore,
    Error,
}

impl TxModel {
    /// The model named by a `[TransactionModel(TransactionModel::X)]` attribute's source text.
    pub fn from_attribute(text: &str) -> Option<TxModel> {
        let t = text.to_ascii_lowercase();
        let (_, value) = t.split_once("::")?;
        match value.trim_end_matches([')', ']', ' ']) {
            "autocommit" => Some(TxModel::AutoCommit),
            "autorollback" => Some(TxModel::AutoRollback),
            "none" => Some(TxModel::None),
            _ => None,
        }
    }
}

impl CommitBehavior {
    /// The behaviour named by a `[CommitBehavior(CommitBehavior::X)]` attribute's source text.
    pub(crate) fn from_attribute(text: &str) -> Option<CommitBehavior> {
        let t = text.to_ascii_lowercase();
        let (_, value) = t.split_once("::")?;
        match value.trim_end_matches([')', ']', ' ']) {
            "ignore" => Some(CommitBehavior::Ignore),
            "error" => Some(CommitBehavior::Error),
            _ => None,
        }
    }
}

/// Refusal of a guarded `Codeunit.Run` while the caller holds an uncommitted write: the full text
/// and the code `DB:Ok` are pinned by codeunit/TestCodeunitRunErrorTextsAndModels.al:75-90.
/// The same text refuses a guarded XmlPort
/// import and a report run with a write pending.
pub(crate) const TRANSACTION_STOPPED: &str =
    "An error occurred and the transaction is stopped. Contact your administrator or partner for further assistance.";
const TRANSACTION_STOPPED_CODE: &str = "DB:Ok";

/// The refusal of a run that needs a transaction of its own while the caller holds a write.
pub(crate) fn transaction_stopped() -> Signal {
    al_error_code(TRANSACTION_STOPPED, TRANSACTION_STOPPED_CODE)
}

/// record/TestTransactionModelAutoRollback.al:ExplicitCommitNotAllowedErr.
const COMMIT_UNDER_AUTO_ROLLBACK: &str =
    "Tests cannot call the Commit function if TransactionModel property is set to AutoRollback.";

/// error-handling/TestCommitBehaviorErrorText.al:34-35: the full text, with code `DB:Ok`.
const COMMIT_PROHIBITED: &str =
    "Commit is prohibited in the current scope. The operation cannot continue. Contact your system administrator.";

/// codeunit/TestWriteTxTestBoundary.al:WriteTxBoundary_Test09_NoneTestCannotWriteDirectly (Insert);
/// codeunit/TestWriteTxNoneModelContracts.al:59-143 pins the same text and the code `DB:BWTMissing`
/// for Modify, Delete, DeleteAll, ModifyAll, Rename and Truncate.
pub(crate) const NO_TRANSACTION: &str = "A transaction must be started before changes can be made to the database.";
const NO_TRANSACTION_CODE: &str = "DB:BWTMissing";

/// codeunit/TestCodeunitRunErrorTextsAndModels.al:48-52 and :64-91: the two sentences joined by
/// one space, with the code `TestCodeUnitNestedInvocation`.
fn nested_test_codeunit(id: u32, name: &str) -> Signal {
    al_error_code(
        &format!(
            "You cannot nest the execution of test codeunits. Test codeunit {id} {name} was called from another test codeunit."
        ),
        "TestCodeUnitNestedInvocation",
    )
}

pub(crate) fn al_error(message: &str) -> Signal {
    al_error_code(message, "")
}

/// `Session::tx_phase`: nothing of the transaction has run yet.
pub(crate) const TX_FRESH: u8 = 0;
/// `Session::tx_phase`: a read call has started the transaction.
pub(crate) const TX_READ: u8 = 1;
/// `Session::tx_phase`: a write call has started the transaction.
pub(crate) const TX_WRITE: u8 = 2;

pub(crate) fn al_error_code(message: &str, code: &str) -> Signal {
    Signal::Error(Box::new(AlError {
        message: message.to_string(),
        code: code.to_string(),
        call_stack: String::new(),
        ..Default::default()
    }))
}

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

/// The emit version BC reports in a missing-object error. Only the measured builds are known
/// (BC 27.5 is 27024 and BC 28.1 is 28014, read off the service tier by AL Runner v2:
/// tests/runner-extras/object-metadata-system-table/OmstTests.Codeunit.al); any other build gets
/// `<major>000`, which keeps the shape the corpus asserts (a number).
fn emit_version(major: u16) -> u32 {
    match major {
        27 => 27024,
        28 => 28014,
        m => u32::from(m) * 1000,
    }
}

impl Session {
    /// The error BC raises when code names an object that no app declares
    /// (codeunit/TestCodeunitRunErrorTextsAndModels.al:162-196 for `CodeUnit`,
    /// recordref/TestRecordRefRuntimeContracts.al:30 for `Table`). The text names the outermost running object (the test codeunit when a
    /// helper issues the call).
    pub(crate) fn missing_object(&self, kind: &str, id: i64) -> Signal {
        // The outermost running object is named, not the helper that issued the call: a test that calls a helper
        // codeunit that runs a missing codeunit reads "from the object <the test codeunit>" (misc/W5MissingObject.al:
        // MissingCodeunit_FromHelperCodeunit, BC 27.5 and 29.0).
        let caller = self
            .frames
            .first()
            .map(|&o| self.engine.program.objects[o].name.as_str())
            .unwrap_or_default();
        // A page is opened by the client side of the test runner, which is what the error names
        // (errors/lt-last-error-and-events/P70_06RunModalField.al:RunModal_UnknownPageId: Page.RunModal and Page.Run of
        // an unknown id, and of id 0).
        // Inside a try function the page error names the outermost object as the other kinds do (platform/
        // lt-xmlport-session-navapp-cookie/P94PageRun.al:211 Run_UnknownPage, misc/W5MissingObject.al:52 `run_missing_page`).
        // A codeunit the test method runs itself, outside a try function, names the test runner too
        // (errors/lt-az-error-texts/AzErrorTexts.al:110-134 CodeunitRun_MissingId_RawText). Deeper calls from the test codeunit
        // keep the outermost object: only a helper codeunit is pinned (MissingCodeunit_FromHelperCodeunit above).
        let test_method_itself = kind == "CodeUnit"
            && self.frames.len() == 1
            && self.frames.first().is_some_and(|&o| {
                self.engine.program.objects[o]
                    .properties
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("Subtype") && v.trim().eq_ignore_ascii_case("Test"))
            });
        let caller = if (kind == "Page" || test_method_itself) && self.tx.try_depth == 0 {
            "Test Runner - Mgt"
        } else {
            caller
        };
        let major = self.engine.target.as_ref().map_or(29, |t| t.bc.major);
        al_error_code(
            &format!(
                "You tried to invoke the {kind} object with the ID {id} from the object {caller}. \
                 An object with that ID does not exist in the current application compiled with emit version {}.",
                emit_version(major)
            ),
            "AL:MetadataNotFound",
        )
    }

    /// Whether the session holds an open write transaction
    /// (codeunit/TestWriteTxTestBoundary.al:Test02/Test03, `Database.IsInWriteTransaction`).
    ///
    /// A page background worker runs in a session of its own, outside the caller's write
    /// transaction, and cannot write to a table (`bg_refuse_write`): its transaction never holds
    /// a change, whatever the caller has pending (handlers/TestPageBgTaskTx_Tests.al:
    /// RunPageBackgroundTask_WorkerIsOutsideCallersWriteTransaction).
    pub(crate) fn in_write_transaction(&self) -> bool {
        // A write call starts the transaction even when it changes nothing (record/TestProbeRecordRefContracts.al:
        // WriteTransaction_State: a DeleteAll or ModifyAll of no row).
        self.pages.bg_worker.is_none() && (self.store.has_uncommitted() || self.tx.tx_phase == TX_WRITE)
    }

    /// Whether the running test method has no transaction: `TransactionModel::None` outside
    /// anything that begins a transaction of its own (a `Codeunit.Run`, a page).
    pub(crate) fn no_transaction(&self) -> bool {
        self.tx_model == TxModel::None && self.tx.tx_world == 0
    }

    /// `Commit()`. The next transaction's type is Update until a set changes it
    /// (session/ProbeSystemEnumMembers.al:94-112, TestClearAllTransactionTypeContracts.al:245-253). A `[CommitBehavior(Ignore)]` scope turns it into a no-op, and exempts it from
    /// the `AutoRollback` refusal (record/TestTransactionModelAutoRollback.al:Test07); the
    /// `AutoRollback` refusal outranks `[CommitBehavior(Error)]` (:Test08).
    pub(crate) fn commit_builtin(&mut self) -> Result<(), Signal> {
        self.bg_refuse_transaction_call("Commit")?;
        if self.tx.commit_behavior == CommitBehavior::Ignore {
            return Ok(());
        }
        if self.tx_model == TxModel::AutoRollback {
            return Err(al_error(COMMIT_UNDER_AUTO_ROLLBACK));
        }
        if self.tx.commit_behavior == CommitBehavior::Error {
            return Err(al_error_code(COMMIT_PROHIBITED, "DB:Ok"));
        }
        // A deferred duplicate-key Insert comes out here (corpus record/TestRecordRuntimeTextsAndViews.al:
        // Record_Insert_DuplicateKey_WithoutBlobField_IsDeferred_...: "raised by Commit").
        if let Some((_, e)) = self.tx.deferred_insert.take() {
            return Err(Signal::Error(Box::new(e)));
        }
        self.commit_transaction();
        // A Commit puts LockTimeout back on (misc/lt-probes-needed-2/PnBRecordMisc.al:LastUsedRowVersion_AndLockTimeout
        // `lockTimeoutAfterCommit`). The timeout duration is not pinned.
        self.tx.lock_timeout = true;
        // The next transaction has not started and its type is Update (see `tx_phase`).
        self.tx.tx_phase = TX_FRESH;
        self.tx.transaction_type = 1;
        Ok(())
    }

    /// `Codeunit.Run(id)` in its statement form, as the test runner's own AL code calls it (an error is not trapped).
    pub fn run_codeunit_by_id(&mut self, id: u32) -> Result<(), Signal> {
        let n = i32::try_from(id).map_err(|_| Signal::Unsupported(format!("codeunit id {id} out of range")))?;
        self.codeunit_run(ids::CODEUNIT_RUN_3, None, &[Value::Integer(n)], false)
            .map(|_| ())
    }

    /// `Codeunit.Run`: `Codeunit.Run#1` is the instance form, `#3` the static form by id. The
    /// call is guarded when its result is used (`ret_used`).
    pub(crate) fn codeunit_run(
        &mut self,
        id: al_ir::BuiltinId,
        receiver: Option<Value>,
        args: &[Value],
        guarded: bool,
    ) -> Result<Value, Signal> {
        // Only the guarded form is pinned inside a page background worker
        // (handlers/TestPageBgTaskTx_Tests.al:RunPageBackgroundTask_WorkerIsOutsideCallersWriteTransaction).
        if !guarded {
            self.bg_refuse_transaction_call("Codeunit.Run")?;
        }
        let (inst, record_arg) = match (id, receiver) {
            (ids::CODEUNIT_RUN_1, Some(Value::Codeunit(rc))) => (self.shared_instance(rc), args.first()),
            (ids::CODEUNIT_RUN_3, None) => {
                let Some(Value::Integer(n)) = args.first() else {
                    return unsupported("Codeunit.Run with an id that is not an integer");
                };
                let id = u32::try_from(*n).ok();
                let Some(object) = self.engine.compiled_codeunit(id) else {
                    // A codeunit of a dependency app that nothing reached is not compiled (ADR-013);
                    // BC has it, so this is not the missing-object error. (With no compiled codeunit of the id, the
                    // first codeunit object of the id is a shell.)
                    if self.engine.codeunit_object(id).is_some() {
                        return unsupported(format!(
                            "Codeunit.Run of codeunit {n}, which belongs to a dependency app and was not compiled (no code the workspace reaches uses it)"
                        ));
                    }
                    // The object is resolved before the run begins, so the guarded form raises
                    // too (codeunit/TestCodeunitRunMissing.al). Text and code:
                    // codeunit/TestCodeunitRunErrorTextsAndModels.al:162-196.
                    return Err(self.missing_object("CodeUnit", i64::from(*n)));
                };
                let fresh = self.instance(object);
                (self.shared_instance(fresh), args.get(1))
            }
            (other, _) => return unsupported(format!("builtin {other} with this receiver")),
        };
        let object = inst.borrow().object as usize;
        let engine = self.engine.clone();
        let obj = &engine.program.objects[object];
        let has_property = |name: &str| obj.properties.iter().any(|(k, _)| k.eq_ignore_ascii_case(name));

        // The nested-run refusal comes from the platform before the run begins, so the guarded
        // form does not trap it (codeunit/TestCodeunitRunNestedTestCodeunit.al:
        // CodeunitRun_TestCodeunitFromTest_Guarded_StillThrows, ...VariableFromTest_Throws).
        let is_test = obj
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("Subtype") && v.trim().eq_ignore_ascii_case("Test"));
        if is_test && self.in_test_codeunit {
            return Err(nested_test_codeunit(obj.id.unwrap_or(0), &obj.name));
        }
        // Codeunit.Run of a codeunit with Subtype = Upgrade fails before it runs, with or without upgrade triggers
        // (events/lt-trigger-pins/PUpgradeTriggers.al:PUpgrade_RunWithTriggers line 121, PUpgrade_RunWithoutTriggers line 134; both by id).
        if obj
            .properties
            .iter()
            .any(|(k, v)| k.eq_ignore_ascii_case("Subtype") && v.trim().eq_ignore_ascii_case("Upgrade"))
        {
            return Err(al_error_code(
                &format!(
                    "Upgrade codeunit '{}' cannot be invoked outside the schema synchronization process.",
                    obj.id.unwrap_or(0)
                ),
                "UpgradeCodeunitOnRunCall",
            ));
        }
        // A record passed to a codeunit without TableNo is not touched
        // (codeunit/TestCodeunitRunContracts.al:Codeunit_Run_WithRecord_PassesRecord).
        //
        // A codeunit with TableNo reads the record as its Rec
        // (handlers/TestPageActionRunObjectKinds_Tests.al:ProbeControlEachTargetRecordsItsOwnExecution,
        // :143-150, reads Rec.Descr in OnRun). The record is passed by reference: a change OnRun
        // makes to Rec reaches the caller's variable, in the guarded and the statement form
        // (codeunit/TestCodeunitRunErrorTextsAndModels.al:198-220). What Rec holds when no record is passed is not pinned and is refused
        // before the run (docs/probes-needed.md).
        if has_property("TableNo") {
            let Some(slot) = obj.rec_slot else {
                return unsupported(format!(
                    "Codeunit.Run of {} (its TableNo table is outside the workspace)",
                    obj.name
                ));
            };
            // Without a record Rec is a blank record; a temporary one is handed over by reference like any other
            // (session/TestProbeSessionMiscContracts.al:CodeunitRun_TableNo_NoRecord, CodeunitRun_TableNo_TemporaryRecord).
            self.ensure_initialized(&inst);
            if let Some(Value::Record(rc)) = record_arg {
                inst.borrow_mut().globals[slot] = Value::Record(rc.clone());
            }
        }

        if guarded {
            // The guarded form needs a transaction world of its own, which BC refuses to open
            // while the caller holds an uncommitted write
            // (codeunit/TestCodeunitRunWriteTransaction.al:GuardedRun_WithUncommittedWrite_IsRefused_AndCodeunitNeverRuns).
            // (in_write_transaction is false in a page background worker.)
            if self.in_write_transaction() {
                // The same refusal under AutoRollback, and inside a TryFunction, which traps it
                // and returns false (codeunit/TestCodeunitRunErrorTextsAndModels.al:93-160 and
                // :248-262).
                return Err(transaction_stopped());
            }
        }

        let on_run = obj
            .routines
            .iter()
            .position(|r| r.is_trigger && r.name.eq_ignore_ascii_case("OnRun"));
        // A codeunit without an OnRun runs as a no-op and succeeds
        // (types/TestLanguageEdgeCases.al:CodeunitRun_Success_ReturnsTrue,
        // codeunit/TestCodeunitRunContracts.al:Codeunit_Run_SetsErrorTextOnFailure).
        let Some(on_run) = on_run else {
            return Ok(if guarded { Value::Boolean(true) } else { Value::Null });
        };

        // Codeunit.Run begins a transaction of its own, so a write is allowed inside it even
        // under TransactionModel::None, and the transaction ends with the run
        // (codeunit/TestWriteTxTestBoundary.al:Test10_ARunCodeunitMayWriteUnderNone).
        let began_world = self.tx_model == TxModel::None && self.tx.tx_world == 0;
        self.tx.tx_world += 1;
        let result = self.call(object, on_run, inst, Vec::new());
        self.tx.tx_world -= 1;

        match result {
            Ok(_) if guarded => {
                // A successful guarded run commits its own transaction
                // (TestCodeunitRunWriteTransaction.al:GuardedRun_LeavingUncommittedWrite_CommitsItWithTheRunsOwnTransaction).
                // Under AutoRollback it commits too: the run's row survives the test, and only the
                // test's own writes roll back (codeunit/TestCodeunitRunErrorTextsAndModels.al:
                // 247-275, AutoRollback_Test03/Test04).
                // The refusal above guarantees nothing of the test's own is pending here.
                //
                // In a page background worker the run's transaction is the worker's own and holds
                // no change, so there is nothing to commit: the caller's pending rows stay pending
                // (handlers/TestPageBgTaskTx_Tests.al:RunPageBackgroundTask_WorkerCommitLeavesCallersRowsUncommitted).
                // A duplicate-key Insert that was deferred comes out of that commit: the run fails with its error
                // (xmlport/lt-xmlport-query-datatransfer/XpTImportBasic.al:Import_DuplicateKeyExisting, through a guarded run).
                if let Some((_, e)) = self.tx.deferred_insert.take() {
                    let mut e = Box::new(e);
                    self.add_top_frame(&mut e);
                    if self.pages.bg_worker.is_none() {
                        self.rollback_transaction();
                    }
                    self.error_finals = None;
                    self.last_error = *e;
                    return Ok(Value::Boolean(false));
                }
                if self.pages.bg_worker.is_none() {
                    self.commit_transaction();
                }
                Ok(Value::Boolean(true))
            }
            Ok(_) => {
                if began_world {
                    self.commit_transaction();
                }
                Ok(Value::Null)
            }
            Err(Signal::Error(mut e)) if guarded => {
                self.add_top_frame(&mut e);
                // The run's writes since its last Commit are undone, the error becomes the last
                // error and the call returns false (TestCodeunitRunGuard.al:GuardedRun_TrapsInnerError_ReturnsFalse,
                // :GuardedRun_InstanceForm_CommitThenError_KeepsCommittedRowOnly).
                // (Nothing to undo in a page background worker: the caller's rows are not its own.)
                if self.pages.bg_worker.is_none() {
                    self.rollback_transaction();
                }
                self.error_finals = None;
                self.last_error = *e;
                Ok(Value::Boolean(false))
            }
            // An unguarded run lets the error through (:UnguardedRun_PropagatesInnerError).
            Err(other) => Err(other),
        }
    }

    /// Refuses a write to a persistent table while the test body has no transaction. Only
    /// `Insert` is pinned (codeunit/TestWriteTxTestBoundary.al:Test09_NoneTestCannotWriteDirectly);
    /// the other writes are refused as unsupported rather than given a guessed outcome.
    pub(crate) fn gate_write(&self, method: RecordOp) -> Result<(), Signal> {
        if !self.no_transaction() && !self.report.request_only_page {
            return Ok(());
        }
        match method {
            RecordOp::Insert
            | RecordOp::Modify
            | RecordOp::Delete
            | RecordOp::DeleteAll
            | RecordOp::ModifyAll
            | RecordOp::Rename
            | RecordOp::Truncate => Err(al_error_code(NO_TRANSACTION, NO_TRANSACTION_CODE)),
            m => unsupported(format!(
                "{} under TransactionModel::None (the refusal is not pinned)",
                m.name()
            )),
        }
    }

    /// Runs a TestPage operation. A page begins a transaction of its own, so under
    /// `TransactionModel::None` its writes are allowed and the transaction ends with the
    /// operation (codeunit/TestWriteTxTestBoundary.al:Test13, Test13c, Test13d). That each
    /// operation, not the whole open-to-close sequence, ends it is an assumption
    /// (docs/probes-needed.md).
    pub(crate) fn in_page_transaction<T>(
        &mut self,
        body: impl FnOnce(&mut Session) -> Result<T, Signal>,
    ) -> Result<T, Signal> {
        let began_world = self.no_transaction();
        self.tx.tx_world += 1;
        let r = body(self);
        self.tx.tx_world -= 1;
        if began_world && r.is_ok() {
            self.commit_transaction();
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_transaction_model_attribute_names_its_model() {
        // The three models the corpus uses (record/TestTransactionModelAutoRollback.al, codeunit/TestWriteTxTestBoundary.al).
        assert_eq!(
            TxModel::from_attribute("[TransactionModel(TransactionModel::AutoCommit)]"),
            Some(TxModel::AutoCommit)
        );
        assert_eq!(
            TxModel::from_attribute("[TransactionModel(TransactionModel::AutoRollback)]"),
            Some(TxModel::AutoRollback)
        );
        assert_eq!(
            TxModel::from_attribute("[TransactionModel(TransactionModel::None)]"),
            Some(TxModel::None)
        );
        assert_eq!(
            TxModel::from_attribute("[transactionmodel(transactionmodel::autorollback )]"),
            Some(TxModel::AutoRollback)
        );
        // Anything else, or no member at all, is not a model.
        assert_eq!(
            TxModel::from_attribute("[TransactionModel(TransactionModel::Other)]"),
            None
        );
        assert_eq!(TxModel::from_attribute("[TransactionModel]"), None);
    }

    #[test]
    fn a_commit_behavior_attribute_names_ignore_or_error_only() {
        assert_eq!(
            CommitBehavior::from_attribute("[CommitBehavior(CommitBehavior::Ignore)]"),
            Some(CommitBehavior::Ignore)
        );
        assert_eq!(
            CommitBehavior::from_attribute("[CommitBehavior(CommitBehavior::Error)]"),
            Some(CommitBehavior::Error)
        );
        assert_eq!(
            CommitBehavior::from_attribute("[CommitBehavior(CommitBehavior::Allow)]"),
            None
        );
        assert_eq!(CommitBehavior::from_attribute("[CommitBehavior]"), None);
    }
}
