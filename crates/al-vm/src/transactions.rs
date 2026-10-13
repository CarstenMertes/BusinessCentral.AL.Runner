//! Transactions as AL code sees them: `Codeunit.Run` (guarded and unguarded), the test
//! method's `TransactionModel`, `Commit` under `AutoRollback` and `[CommitBehavior]`, and
//! `Database.IsInWriteTransaction`.
//!
//! What counts as "a write transaction is open" is "the store holds a change since the last
//! commit" (`MemStore::has_uncommitted`). Every rule below is pinned by a corpus test, cited
//! where it is applied; what the corpus does not pin is refused (`Signal::Unsupported`) and
//! listed in docs/probes-needed.md.

use crate::{AlError, Session, Signal};
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

/// Refusal of a guarded `Codeunit.Run` while the caller holds an uncommitted write. The corpus
/// pins the fragment `the transaction is stopped` (codeunit/TestCodeunitRunWriteTransaction.al:76);
/// the full sentence is quoted from a BC log in corpus docs/handoff-2026-05-25-test-failures.md:242.
pub(crate) const TRANSACTION_STOPPED: &str =
    "An error occurred and the transaction is stopped. Contact your administrator for further assistance.";

/// record/TestTransactionModelAutoRollback.al:ExplicitCommitNotAllowedErr.
const COMMIT_UNDER_AUTO_ROLLBACK: &str =
    "Tests cannot call the Commit function if TransactionModel property is set to AutoRollback.";

/// error-handling/TestCommitBehaviorAttribute.al:CommitBehavior_Error_CommitRaisesAndRowIsRolledBack
/// pins only this fragment of the text.
const COMMIT_PROHIBITED: &str = "Commit is prohibited in the current scope";

/// codeunit/TestWriteTxTestBoundary.al:WriteTxBoundary_Test09_NoneTestCannotWriteDirectly.
pub(crate) const NO_TRANSACTION: &str = "A transaction must be started before changes can be made to the database.";

/// codeunit/TestCodeunitRunNestedTestCodeunit.al: `NestedErr` and `NestedCodeunitErr`, each pinned
/// as a fragment. Joined by one space, which the corpus does not pin (docs/probes-needed.md).
fn nested_test_codeunit(id: u32, name: &str) -> String {
    format!(
        "You cannot nest the execution of test codeunits. Test codeunit {id} {name} was called from another test codeunit."
    )
}

pub(crate) fn al_error(message: &str) -> Signal {
    Signal::Error(AlError {
        message: message.to_string(),
        code: String::new(),
        call_stack: String::new(),
    })
}

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

impl Session {
    /// Whether the session holds an open write transaction
    /// (codeunit/TestWriteTxTestBoundary.al:Test02/Test03, `Database.IsInWriteTransaction`).
    pub(crate) fn in_write_transaction(&self) -> bool {
        self.store.has_uncommitted()
    }

    /// Whether the running test method has no transaction: `TransactionModel::None` outside
    /// anything that begins a transaction of its own (a `Codeunit.Run`, a page).
    pub(crate) fn no_transaction(&self) -> bool {
        self.tx_model == TxModel::None && self.tx_world == 0
    }

    /// `Commit()`. A `[CommitBehavior(Ignore)]` scope turns it into a no-op, and exempts it from
    /// the `AutoRollback` refusal (record/TestTransactionModelAutoRollback.al:Test07); the
    /// `AutoRollback` refusal outranks `[CommitBehavior(Error)]` (:Test08).
    pub(crate) fn commit_builtin(&mut self) -> Result<(), Signal> {
        self.bg_refuse_transaction_call("Commit")?;
        if self.commit_behavior == CommitBehavior::Ignore {
            return Ok(());
        }
        if self.tx_model == TxModel::AutoRollback {
            return Err(al_error(COMMIT_UNDER_AUTO_ROLLBACK));
        }
        if self.commit_behavior == CommitBehavior::Error {
            return Err(al_error(COMMIT_PROHIBITED));
        }
        self.commit_transaction();
        Ok(())
    }

    /// `Codeunit.Run`: `Codeunit.Run#1` is the instance form, `#3` the static form by id. The
    /// call is guarded when its result is used (`ret_used`).
    pub(crate) fn codeunit_run(
        &mut self,
        id: &str,
        receiver: Option<Value>,
        args: &[Value],
        guarded: bool,
    ) -> Result<Value, Signal> {
        self.bg_refuse_transaction_call("Codeunit.Run")?;
        let (inst, record_arg) = match (id, receiver) {
            ("Codeunit.Run#1", Some(Value::Codeunit(rc))) => (self.shared_instance(rc), args.first()),
            ("Codeunit.Run#3", None) => {
                let Some(Value::Integer(n)) = args.first() else {
                    return unsupported("Codeunit.Run with an id that is not an integer");
                };
                let Some(object) = self
                    .engine
                    .program
                    .objects
                    .iter()
                    .position(|o| o.kind == "codeunit" && o.id == u32::try_from(*n).ok())
                else {
                    // codeunit/TestCodeunitRunMissing.al pins two fragments of the text, not the
                    // middle of it (the calling object and an emit version).
                    return unsupported(format!(
                        "Codeunit.Run of id {n}, which no object declares (error text not pinned)"
                    ));
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
            return Err(al_error(&nested_test_codeunit(obj.id.unwrap_or(0), &obj.name)));
        }
        if has_property("TableNo") {
            return unsupported(format!(
                "Codeunit.Run of {} (TableNo: passing Rec is not pinned)",
                obj.name
            ));
        }
        // A record passed to a codeunit without TableNo is not touched
        // (codeunit/TestCodeunitRunContracts.al:Codeunit_Run_WithRecord_PassesRecord).
        let _ = record_arg;

        if guarded {
            // The guarded form needs a transaction world of its own, which BC refuses to open
            // while the caller holds an uncommitted write
            // (codeunit/TestCodeunitRunWriteTransaction.al:GuardedRun_WithUncommittedWrite_IsRefused_AndCodeunitNeverRuns).
            if self.in_write_transaction() {
                if self.tx_model == TxModel::AutoRollback {
                    return unsupported(
                        "guarded Codeunit.Run with a pending write under TransactionModel::AutoRollback (not pinned)",
                    );
                }
                return Err(al_error(TRANSACTION_STOPPED));
            }
            if self.try_depth > 0 {
                return unsupported("guarded Codeunit.Run inside a TryFunction (not pinned)");
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
        let began_world = self.tx_model == TxModel::None && self.tx_world == 0;
        self.tx_world += 1;
        let result = self.call(object, on_run, inst, Vec::new());
        self.tx_world -= 1;

        match result {
            Ok(_) if guarded => {
                // A successful guarded run commits its own transaction
                // (TestCodeunitRunWriteTransaction.al:GuardedRun_LeavingUncommittedWrite_CommitsItWithTheRunsOwnTransaction).
                // Under AutoRollback nothing may become durable; whether BC commits here is not
                // pinned (docs/probes-needed.md), so the run joins the test's transaction.
                if self.tx_model != TxModel::AutoRollback {
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
            Err(Signal::Error(e)) if guarded => {
                // The run's writes since its last Commit are undone, the error becomes the last
                // error and the call returns false (TestCodeunitRunGuard.al:GuardedRun_TrapsInnerError_ReturnsFalse,
                // :GuardedRun_InstanceForm_CommitThenError_KeepsCommittedRowOnly).
                self.rollback_transaction();
                self.error_finals = None;
                self.last_error = e;
                Ok(Value::Boolean(false))
            }
            // An unguarded run lets the error through (:UnguardedRun_PropagatesInnerError).
            Err(other) => Err(other),
        }
    }

    /// Refuses a write to a persistent table while the test body has no transaction. Only
    /// `Insert` is pinned (codeunit/TestWriteTxTestBoundary.al:Test09_NoneTestCannotWriteDirectly);
    /// the other writes are refused as unsupported rather than given a guessed outcome.
    pub(crate) fn gate_write(&self, method: &str) -> Result<(), Signal> {
        if !self.no_transaction() {
            return Ok(());
        }
        match method {
            "Insert" => Err(al_error(NO_TRANSACTION)),
            m => unsupported(format!("{m} under TransactionModel::None (the refusal is not pinned)")),
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
        self.tx_world += 1;
        let r = body(self);
        self.tx_world -= 1;
        if began_world && r.is_ok() {
            self.commit_transaction();
        }
        r
    }
}
