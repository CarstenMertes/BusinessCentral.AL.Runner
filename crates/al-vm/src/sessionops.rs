//! Builtins that act on session state: number sequences and the transaction state.

use crate::Session;
use al_builtins::describe::Describe;
use al_builtins::native::{AlError, Signal};
use al_builtins::value::Value;
use std::cell::RefCell;
use std::rc::Rc;

/// One `NumberSequence`.
#[derive(Debug, Clone)]
pub(crate) struct Sequence {
    pub increment: i64,
    /// The value `Next` hands out when nothing was allocated yet.
    pub seed: i64,
    /// The last value handed out (by `Next` or the end of a `Range`).
    pub last: Option<i64>,
}

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn error<T>(message: String) -> Result<T, Signal> {
    Err(Signal::Error(AlError {
        message,
        code: String::new(),
        call_stack: String::new(),
    }))
}

/// An error with a pinned code.
fn error_code<T>(message: String, code: &str) -> Result<T, Signal> {
    Err(Signal::Error(AlError {
        message,
        code: code.into(),
        call_stack: String::new(),
    }))
}

fn big(args: &[Value], i: usize) -> Option<i64> {
    match args.get(i)? {
        Value::BigInteger(n) => Some(*n),
        Value::Integer(n) => Some(i64::from(*n)),
        _ => None,
    }
}

/// The connection string of the platform's own test connection, the one a CRM table connection
/// may use in a test (corpus session/TestDatabaseTableConnectionCrm.al:8-14).
const TEST_CONNECTION: &str = "@@test@@";

impl Session {
    /// `Database.{Register,Unregister,Has,SetDefault,GetDefault}TableConnection`. Only the CRM
    /// test connection is modelled (session/TestDatabaseTableConnectionCrm.al); names match
    /// ignoring case (:Database_HasTableConnection_CrmTest_NameIsCaseInsensitive).
    fn table_connection_op(&mut self, method: &str, args: &[Value]) -> Result<Value, Signal> {
        let kind = match args.first() {
            Some(Value::Option(o, m)) => (*o, m.get(*o as usize).cloned().unwrap_or_default()),
            other => return unsupported(format!("{method}TableConnection with {} as the type", other.describe())),
        };
        let name = match args.get(1) {
            Some(v) => al_builtins::native::text_of(v),
            None => None,
        };
        match (method, kind.1.as_str()) {
            // A Cloud extension may not register an external SQL connection
            // (session/TestMiscComplete.al:185-194: the error mentions a permission). The wording
            // is a documentation-only assumption (docs/probes-needed.md).
            ("Register", "ExternalSQL") => {
                return error(
                    "You do not have permission to register a table connection of type ExternalSQL.".to_string(),
                );
            }
            // Never registered, so none is found and none is the default. The corpus only calls
            // them for ExternalSQL and expects them not to throw
            // (session/TestDatabaseExtended.al:75-92, TestSystemExtended.al:664-672).
            ("Has", "ExternalSQL") => return Ok(Value::Boolean(false)),
            ("GetDefault", "ExternalSQL") => return Ok(Value::Text(String::new())),
            (_, "CRM") => {}
            (m, k) => return unsupported(format!("{m}TableConnection of type {k} (not pinned)")),
        }
        let ord = kind.0;
        match method {
            "GetDefault" => Ok(Value::Text(
                self.default_table_connection.get(&ord).cloned().unwrap_or_default(),
            )),
            "Has" | "Register" | "Unregister" | "SetDefault" => {
                let Some(name) = name else {
                    return unsupported(format!("{method}TableConnection with a name that is not text"));
                };
                let key = (ord, name.to_lowercase());
                match method {
                    "Has" => Ok(Value::Boolean(self.table_connections.contains(&key))),
                    "Register" => {
                        match args.get(2).and_then(al_builtins::native::text_of) {
                            Some(c) if c == TEST_CONNECTION => {}
                            _ => {
                                return unsupported(
                                    "RegisterTableConnection with a connection string other than @@test@@ (no Dataverse in the runtime)",
                                );
                            }
                        }
                        // The name is the pinned fragment of the error
                        // (:Database_RegisterTableConnection_CrmTest_Duplicate_Throws).
                        if !self.table_connections.insert(key) {
                            return error(format!(
                                "A table connection of type CRM named {name} is already registered."
                            ));
                        }
                        Ok(Value::Null)
                    }
                    "Unregister" => {
                        if !self.table_connections.remove(&key) {
                            return unsupported(
                                "UnregisterTableConnection of a connection that is not registered (not pinned)",
                            );
                        }
                        // Unregistering the default clears the default too
                        // (:Database_UnregisterTableConnection_CrmTest_ClearsDefaultAndHas).
                        if self
                            .default_table_connection
                            .get(&ord)
                            .is_some_and(|d| d.to_lowercase() == key.1)
                        {
                            self.default_table_connection.remove(&ord);
                        }
                        Ok(Value::Null)
                    }
                    _ => {
                        if args.len() > 2 {
                            return unsupported("SetDefaultTableConnection with a Scoped argument (not pinned)");
                        }
                        if !self.table_connections.contains(&key) {
                            return unsupported(
                                "SetDefaultTableConnection of a connection that is not registered (not pinned)",
                            );
                        }
                        self.default_table_connection.insert(ord, name);
                        Ok(Value::Null)
                    }
                }
            }
            other => unsupported(format!("{other}TableConnection")),
        }
    }

    /// The key of sequence `args[0]`; `scope` is the position of the optional CompanySpecific
    /// argument (company specific when omitted, as the default overload is: session/TestNumberSequence.al:15-30).
    fn sequence_key(&self, args: &[Value], scope: usize) -> Result<(String, String), Signal> {
        let Some(name) = args.first().and_then(al_builtins::native::text_of) else {
            return unsupported("a number sequence name that is not text");
        };
        let company_specific = match args.get(scope) {
            None => true,
            Some(Value::Boolean(b)) => *b,
            Some(other) => return unsupported(format!("CompanySpecific of {}", other.describe())),
        };
        let company = if company_specific {
            self.cx.company.clone()
        } else {
            String::new()
        };
        Ok((company, name))
    }

    pub(crate) fn session_op(&mut self, id: &str, args: &mut [Value]) -> Result<Value, Signal> {
        if let Some(answer) = self.host_op(id, args) {
            return answer;
        }
        if id == "Database.IsInWriteTransaction#1" {
            // An uncommitted write opens one, a Commit and the test boundary close it
            // (codeunit/TestWriteTxTestBoundary.al:93,103,142). Writes to temporary tables are
            // not counted: they never reach the database (not pinned).
            return Ok(Value::Boolean(self.store.has_uncommitted()));
        }
        if id == "CompanyProperty.DisplayName#1" {
            // Non-empty (session/TestBCPlatformContracts.al:CompanyProperty_DisplayName_NonEmpty). That it is
            // the company's name when no display name is set is an assumption (docs/probes-needed.md); a
            // company that states a display name is not modelled.
            return Ok(Value::Text(self.cx.company.clone()));
        }
        if id == "Database.CompanyName#1" {
            // The company of the session, the same name `Record.CurrentCompany` answers
            // (session/TestDatabase.al:69-75, session/TestSessionFunctions.al:25).
            return Ok(Value::Text(self.cx.company.clone()));
        }
        match id {
            // The counter has handed out `next_row_version` so far; the database never reports
            // zero (session/TestSystemExtended.al:635-659, TestBCDatabaseContracts.al:100-117).
            "Database.LastUsedRowVersion#1" => return Ok(Value::BigInteger(self.next_row_version.max(1))),
            // Set when given a value, read otherwise; the default is the documented one
            // (docs/probes-needed.md). session/TestSystemExtended.al:608-630 only calls it.
            "Database.LockTimeout#1" => {
                match args.first() {
                    None | Some(Value::Null) => {}
                    Some(Value::Boolean(b)) => self.lock_timeout = *b,
                    Some(other) => return unsupported(format!("LockTimeout with {}", other.describe())),
                }
                return Ok(Value::Boolean(self.lock_timeout));
            }
            // The type of the transaction. A test starts as UpdateNoLocks, not Update, and a set before
            // any database access takes effect; once the transaction has written, a read answers
            // Update and a set to another type is ignored without an error
            // (session/TestClearAllTransactionTypeContracts.al:224-253). A set to Update itself after a
            // write is session/TestDatabaseExtended.al:Database_CurrentTransactionType_SetAfterWriteOp_
            // Throws, which expects an error whose text is not pinned: refused. A set after a read only
            // is not pinned either and takes effect (docs/probes-needed.md).
            "Database.CurrentTransactionType#1" => {
                let members: std::rc::Rc<[String]> = al_builtins::sysenum::option_members("TransactionType")
                    .expect("a system option")
                    .into();
                let wrote = self.tx_wrote || self.in_write_transaction();
                match args.first() {
                    None | Some(Value::Null) => {
                        let t = if wrote { 1 } else { self.transaction_type };
                        return Ok(Value::Option(t, members));
                    }
                    Some(Value::Option(t, _)) if !wrote => {
                        self.transaction_type = *t;
                        return Ok(Value::Null);
                    }
                    Some(Value::Option(t, _)) if *t != 1 => return Ok(Value::Null),
                    Some(_) => {
                        return unsupported(
                            "Database.CurrentTransactionType set to Update after a write (BC raises an error whose text is not pinned)",
                        );
                    }
                }
            }
            _ => {}
        }
        if id == "System.CodeCoverageLog#1" {
            // With a state it records or stops and answers the state it was given; without one it
            // answers whether it is recording (codeunit/TestCodeCoverageStartStop.al:30-102). The
            // MultiSession flag changes nothing observable here.
            return match args.first() {
                None => Ok(Value::Boolean(self.code_coverage)),
                Some(Value::Boolean(on)) => {
                    self.code_coverage = *on;
                    if *on {
                        self.coverage_started();
                    }
                    Ok(Value::Boolean(*on))
                }
                Some(other) => unsupported(format!("CodeCoverageLog with {}", other.describe())),
            };
        }
        if let Some(method) = id
            .strip_prefix("Database.")
            .and_then(|m| m.strip_suffix("TableConnection#1"))
        {
            return self.table_connection_op(method, args);
        }
        if let Some(v) = self.error_collection_op(id, args) {
            return v;
        }
        if let Some(rest) = id.strip_prefix("IsolatedStorage.") {
            return self.isolated_storage_op(rest, args);
        }
        let method = id.strip_prefix("NumberSequence.").and_then(|m| m.split('#').next());
        let Some(method) = method else {
            return unsupported(format!("builtin {id}"));
        };
        // The position of the optional CompanySpecific argument per overload.
        let scope = match id {
            "NumberSequence.Insert#1" | "NumberSequence.Range#2" => 3,
            "NumberSequence.Range#1" | "NumberSequence.Restart#1" => 2,
            _ => 1,
        };
        let key = self.sequence_key(args, scope)?;
        let name = key.1.clone();
        // session/TestNumberSequenceRuntimeContracts.al:NumberSequence_MissingSequence_Texts: the name is
        // quoted and the code is `AL`.
        let missing = || error_code::<Value>(format!("The number sequence '{name}' does not exist."), "AL");
        match method {
            "Insert" => {
                // Seed 0 and Increment 1 when omitted
                // (session/TestNumberSequenceRuntimeContracts.al:NumberSequence_Insert_WithoutArguments_StartsAtZeroAndStepsByOne).
                let seed = big(args, 1).unwrap_or(0);
                let increment = big(args, 2).unwrap_or(1);
                // Inserting a name that exists raises, and the sequence is gone afterwards, in this
                // test and the next (:NumberSequence_Insert_OfAnExistingName_RaisesAndLeavesTheSequenceGone).
                if self.sequences.remove(&key).is_some() {
                    return error_code(format!("The number sequence '{name}' already exists."), "AL");
                }
                self.sequences.insert(
                    key,
                    Sequence {
                        increment,
                        seed,
                        last: None,
                    },
                );
                Ok(Value::Null)
            }
            "Exists" => Ok(Value::Boolean(self.sequences.contains_key(&key))),
            // Deleting a sequence that is not there is not an error
            // (error-handling/TestNumberSequenceTrappableErrors.al:117-126).
            "Delete" => {
                self.sequences.remove(&key);
                Ok(Value::Null)
            }
            "Current" => match self.sequences.get(&key) {
                Some(s) => Ok(Value::BigInteger(s.last.unwrap_or(s.seed))),
                None => missing(),
            },
            "Next" | "Range" => {
                let count = if method == "Range" {
                    match args.get(1) {
                        Some(Value::Integer(n)) if *n >= 1 => i64::from(*n),
                        // :NumberSequence_Range_Zero_ThrowsSqlError (the sequence must exist first).
                        Some(Value::Integer(0)) if self.sequences.contains_key(&key) => {
                            return error_code(
                                "An unexpected error occurred after a database command was cancelled.".to_string(),
                                "Sql:Sql",
                            );
                        }
                        other => {
                            return unsupported(format!(
                                "NumberSequence.Range with Count {} (not pinned)",
                                other.describe()
                            ));
                        }
                    }
                } else {
                    1
                };
                let Some(s) = self.sequences.get_mut(&key) else {
                    return missing();
                };
                let first = match s.last {
                    None => Some(s.seed),
                    Some(last) => last.checked_add(s.increment),
                };
                let last = first.and_then(|f| s.increment.checked_mul(count - 1).and_then(|d| f.checked_add(d)));
                let (Some(first), Some(last)) = (first, last) else {
                    return unsupported("a number sequence past the BigInteger range (error not pinned)");
                };
                s.last = Some(last);
                if id == "NumberSequence.Range#2" {
                    args[2] = Value::BigInteger(s.increment);
                }
                Ok(Value::BigInteger(first))
            }
            "Restart" => {
                // Without a Seed the sequence restarts at 0 whatever it started at
                // (:NumberSequence_NextRestartRange_Values).
                let seed = big(args, 1).unwrap_or(0);
                match self.sequences.get_mut(&key) {
                    Some(s) => {
                        s.seed = seed;
                        s.last = None;
                        Ok(Value::Null)
                    }
                    None => missing(),
                }
            }
            other => unsupported(format!("NumberSequence.{other}")),
        }
    }
}

fn secret_text(s: String) -> Value {
    Value::Library(
        "SecretText".into(),
        Rc::new(RefCell::new(al_builtins::value::LibraryValue::Text(s))),
    )
}

impl Session {
    /// The key a data scope names. Module is the default and is one app's store; Company adds the
    /// session company, and the app boundary holds there too (session/TestIsolatedStoragePerApp.al:
    /// 100-120). The user scopes need a user identity, which the runtime does not have.
    fn storage_scope(&self, scope: Option<&Value>) -> Result<String, Signal> {
        // The module is the app that owns the code calling IsolatedStorage, not the app that raised
        // the event it handles (session/TestIsolatedStoragePerApp.al:140-160).
        let app = self.frames.last().map_or(0, |o| self.engine.program.objects[*o].app);
        match scope {
            None => Ok(format!("app{app}")),
            Some(Value::Option(0, _)) => Ok(format!("app{app}")),
            Some(Value::Option(1, _)) => Ok(format!("app{app}:company:{}", self.cx.company)),
            Some(Value::Option(..)) => {
                unsupported("IsolatedStorage with a user data scope (needs an identity, ADR-013)")
            }
            Some(other) => unsupported(format!("IsolatedStorage with data scope {}", other.describe())),
        }
    }

    /// `IsolatedStorage.<rest>` (`Get#4`, `SetEncrypted#2`, ...): a text store per scope, outside
    /// the transaction (not pinned either way). Corpus: session/TestIsolatedStorage.al:30-125.
    fn isolated_storage_op(&mut self, rest: &str, args: &mut [Value]) -> Result<Value, Signal> {
        let Some(key) = args.first().and_then(al_builtins::native::text_of) else {
            return unsupported("an IsolatedStorage key that is not text");
        };
        let (method, n) = rest.split_once('#').unwrap_or((rest, "1"));
        let n: u32 = n.parse().unwrap_or(1);
        // Where the optional DataScope sits, and where the value goes, per overload.
        let scope_at = match (method, n) {
            ("Get", 3 | 4) => None,
            ("Get", _) => Some(1),
            ("Set" | "SetEncrypted", _) => Some(2),
            _ => Some(1),
        };
        let scope = self.storage_scope(scope_at.and_then(|i| args.get(i)))?;
        let slot = (scope, key);
        let found = self.ret_used;
        match method {
            "Set" | "SetEncrypted" => {
                let value = match args.get(1) {
                    Some(v) => al_builtins::textfns::secret_or_text(v)?,
                    None => return unsupported("IsolatedStorage.Set without a value"),
                };
                self.isolated_storage.insert(slot, (value, method == "SetEncrypted"));
                Ok(Value::Boolean(true))
            }
            "Contains" => {
                let hit = self.isolated_storage.get(&slot).cloned();
                if n == 2 {
                    args[2] = Value::Boolean(hit.as_ref().is_some_and(|(_, enc)| *enc));
                }
                Ok(Value::Boolean(hit.is_some()))
            }
            "Delete" => match self.isolated_storage.remove(&slot) {
                Some(_) => Ok(Value::Boolean(true)),
                None if found => Ok(Value::Boolean(false)),
                None => unsupported("IsolatedStorage.Delete of a missing key as a statement (error text not pinned)"),
            },
            "Get" => {
                let target = if n <= 2 { 2 } else { 1 };
                match self.isolated_storage.get(&slot) {
                    Some((value, _)) => {
                        args[target] = if n == 1 || n == 3 {
                            secret_text(value.clone())
                        } else {
                            Value::Text(value.clone())
                        };
                        Ok(Value::Boolean(true))
                    }
                    None if found => Ok(Value::Boolean(false)),
                    None => unsupported("IsolatedStorage.Get of a missing key as a statement (error text not pinned)"),
                }
            }
            other => unsupported(format!("IsolatedStorage.{other}")),
        }
    }
}
