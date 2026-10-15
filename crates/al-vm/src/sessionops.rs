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
    Err(Signal::Error(Box::new(AlError {
        message,
        code: String::new(),
        call_stack: String::new(),
    })))
}

/// An error with a pinned code.
fn error_code<T>(message: String, code: &str) -> Result<T, Signal> {
    Err(Signal::Error(Box::new(AlError {
        message,
        code: code.into(),
        call_stack: String::new(),
    })))
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
            ("GetDefault", "ExternalSQL") => return Ok(Value::Text(String::new().into())),
            (_, "CRM") => {}
            (m, k) => return unsupported(format!("{m}TableConnection of type {k} (not pinned)")),
        }
        let ord = kind.0;
        match method {
            "GetDefault" => Ok(Value::Text(
                self.default_table_connection
                    .get(&ord)
                    .cloned()
                    .unwrap_or_default()
                    .into(),
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
                        self.default_table_connection.insert(ord, name.to_string());
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
        Ok((company, name.to_string()))
    }

    pub(crate) fn session_op(&mut self, id: &str, args: &mut [Value]) -> Result<Value, Signal> {
        if let Some(answer) = self.host_op(id, args) {
            return answer;
        }
        if let Some(answer) = self.encryption_op(id, args) {
            return answer;
        }
        if id == "SessionInformation.Callstack#1" {
            // Frames innermost first, a backslash after each, the runner's below
            // (session/TestSessionInformationCallstackContracts.al:130-143).
            return match self.live_call_stack() {
                Some(text) => Ok(Value::Text(text.into())),
                None => unsupported(
                    "SessionInformation.Callstack with a frame whose format is not pinned (an event of a table, page or the platform, or an object type not measured)",
                ),
            };
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
            return Ok(Value::Text(self.cx.company.clone().into()));
        }
        if id == "CompanyProperty.ID#1" {
            // A Guid that identifies the company; stable for a name (the Company table's SystemId is not modelled).
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for b in self.cx.company.bytes() {
                h = (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3);
            }
            let mut g = [0u8; 16];
            g[..8].copy_from_slice(&h.to_le_bytes());
            g[8..].copy_from_slice(&h.rotate_left(17).to_le_bytes());
            g[6] = (g[6] & 0x0f) | 0x40;
            g[8] = (g[8] & 0x3f) | 0x80;
            return Ok(Value::Guid(g));
        }
        if id == "CompanyProperty.UrlName#1" {
            // The company name as it stands in a URL: unreserved characters kept, the rest percent-encoded
            // (session/TestProbeSessionMiscContracts.al:Company_Names scrubs both the name and its `%20` form).
            let mut out = String::new();
            for b in self.cx.company.bytes() {
                if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                    out.push(char::from(b));
                } else {
                    out.push_str(&format!("%{b:02X}"));
                }
            }
            return Ok(Value::Text(out.into()));
        }
        if id == "Database.CompanyName#1" {
            // The company of the session, the same name `Record.CurrentCompany` answers
            // (session/TestDatabase.al:69-75, session/TestSessionFunctions.al:25).
            return Ok(Value::Text(self.cx.company.clone().into()));
        }
        match id {
            // The counter has handed out `next_row_version` so far; the database never reports
            // zero (session/TestSystemExtended.al:635-659, TestBCDatabaseContracts.al:100-117).
            // What the database holds committed: a row inserted in the open transaction does not count yet
            // (record/TestProbeRecordRefContracts.al:LastUsedRowVersion_StartsAndGrows, `grows_after_insert` No).
            "Database.LastUsedRowVersion#1" => return Ok(Value::BigInteger(self.committed_row_version.max(1))),
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
            // Set when given a value, read otherwise. Only "positive" is pinned (corpus types/PVLabelSplitForeach.al
            // on the staging branch: positive, 10 ms on the probe tier, "the exact default is a server setting").
            "Database.LockTimeoutDuration#1" => {
                match args.first() {
                    None | Some(Value::Null) => {}
                    Some(Value::Duration(d)) => self.lock_timeout_duration = *d,
                    Some(Value::Integer(i)) => self.lock_timeout_duration = i64::from(*i),
                    Some(other) => return unsupported(format!("LockTimeoutDuration with {}", other.describe())),
                }
                // The catalog types the result as an Integer (a Duration in AL); the id is pinned (pinned_ids.tsv).
                return Ok(Value::Integer(
                    i32::try_from(self.lock_timeout_duration).unwrap_or(i32::MAX),
                ));
            }
            // The type of the transaction (session/ProbeSystemEnumMembers.al:60-161, BC 27.5;
            // TestClearAllTransactionTypeContracts.al:224-253):
            // - a test starts as UpdateNoLocks, and a set before any database call takes effect;
            // - after a Commit() the next transaction's type is Update, and a set takes effect again;
            // - once a write call has run (also a DeleteAll that finds no row) the type stays as it was: a
            //   set to Report or Snapshot, or to the type it has, goes through without effect, and a set
            //   to Update raises DB:CannotChangeTransactionType;
            // - once a read call has run, a set of Report reads back as Update.
            // Every other set after the transaction has started is not pinned and refused.
            "Database.CurrentTransactionType#1" => {
                use crate::transactions::{TX_FRESH, TX_READ, TX_WRITE};
                let members: std::sync::Arc<[String]> = al_builtins::sysenum::option_members("TransactionType")
                    .expect("a system option")
                    .into();
                let new = match args.first() {
                    None | Some(Value::Null) => return Ok(Value::Option(self.transaction_type, members)),
                    Some(Value::Option(t, _)) => *t,
                    Some(other) => return unsupported(format!("CurrentTransactionType set to {}", other.describe())),
                };
                let (update, snapshot, report) = (1, 2, 4);
                match self.tx_phase {
                    TX_FRESH => self.transaction_type = new,
                    TX_WRITE if new == self.transaction_type => {}
                    TX_WRITE if new == snapshot || new == report => {}
                    TX_WRITE if new == update => {
                        let name = |t: i32| members.get(t as usize).cloned().unwrap_or_default();
                        return Err(crate::transactions::al_error_code(
                            &format!(
                                "The transaction type cannot be changed after a transaction has started.\r\\\r\\A transaction starts when the first database operation, such as FIND, NEXT, INSERT, DELETE, MODIFY, SUM, or COUNT, is run.\r\\\r\\The error occurred because the transaction type was changed from {} to {}.",
                                name(self.transaction_type),
                                name(new)
                            ),
                            "DB:CannotChangeTransactionType",
                        ));
                    }
                    TX_READ if new == report => self.transaction_type = update,
                    _ => {
                        return unsupported(format!(
                            "Database.CurrentTransactionType set to {} after a database call (not pinned)",
                            members.get(new as usize).cloned().unwrap_or_default()
                        ));
                    }
                }
                return Ok(Value::Null);
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
            // The user scopes add the session user (session/TestProbeSessionMiscContracts.al:IsolatedStorage_Behaviour:
            // nothing was set there, a Get answers false).
            Some(Value::Option(2, _)) => Ok(format!("app{app}:user:{}", self.host.identity.user_id())),
            Some(Value::Option(3, _)) => Ok(format!(
                "app{app}:company:{}:user:{}",
                self.cx.company,
                self.host.identity.user_id()
            )),
            Some(Value::Option(..)) => unsupported("IsolatedStorage with a data scope the corpus does not name"),
            Some(other) => unsupported(format!("IsolatedStorage with data scope {}", other.describe())),
        }
    }

    /// `IsolatedStorage.<rest>` (`Get#4`, `SetEncrypted#2`, ...): a text store per scope, outside
    /// the transaction (not pinned either way). Corpus: session/TestIsolatedStorage.al:30-125.
    fn isolated_storage_op(&mut self, rest: &str, args: &mut [Value]) -> Result<Value, Signal> {
        let Some(key) = args.first().and_then(al_builtins::native::text_of).map(String::from) else {
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
                    Some(v) => al_builtins::textfns::secret_or_text(v)?.to_string(),
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
                            Value::Text(value.clone().into())
                        };
                        Ok(Value::Boolean(true))
                    }
                    // A missing key empties the target (session/TestProbeSessionMiscContracts.al:IsolatedStorage_Behaviour,
                    // `get_missing` shows `No ` with the variable cleared).
                    None if found => {
                        args[target] = if n == 1 || n == 3 {
                            secret_text(String::new())
                        } else {
                            Value::Text(String::new().into())
                        };
                        Ok(Value::Boolean(false))
                    }
                    None => unsupported("IsolatedStorage.Get of a missing key as a statement (error text not pinned)"),
                }
            }
            other => unsupported(format!("IsolatedStorage.{other}")),
        }
    }
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(bytes: &[u8]) -> String {
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (u32::from(*b) << (16 - 8 * i)));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(BASE64[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl Session {
    /// `EncryptionEnabled`, `EncryptionKeyExists`, `Encrypt` and `Decrypt`, for a tier that has an
    /// encryption key (types/TestVersionCreateEncryptionContracts.al, BC 27.5 on bc-linux). Encrypted text is
    /// 344 characters (256 bytes in base 64) and differs on every call; this session remembers what it
    /// encrypted, so `Decrypt` returns it. Text that is not encrypted data raises the pinned error;
    /// text that looks like encrypted data this session did not make is refused.
    fn encryption_op(&mut self, id: &str, args: &mut [Value]) -> Option<Result<Value, Signal>> {
        Some(match id {
            "System.EncryptionEnabled#1" | "System.EncryptionKeyExists#1" => Ok(Value::Boolean(true)),
            "System.Encrypt#1" => {
                let Some(plain) = args.first().and_then(al_builtins::native::text_of) else {
                    return Some(unsupported("Encrypt of something that is not text"));
                };
                if plain.is_empty() {
                    return Some(Ok(Value::Text(String::new().into())));
                }
                let mut bytes = Vec::with_capacity(256);
                for _ in 0..16 {
                    bytes.extend_from_slice(&self.fresh_guid());
                }
                let cipher = base64(&bytes);
                self.encrypted.insert(cipher.clone(), plain.to_string());
                Ok(Value::Text(cipher.into()))
            }
            "System.Decrypt#1" => {
                let Some(cipher) = args.first().and_then(al_builtins::native::text_of) else {
                    return Some(unsupported("Decrypt of something that is not text"));
                };
                if let Some(plain) = self.encrypted.get(cipher.as_str()) {
                    return Some(Ok(Value::Text(plain.clone().into())));
                }
                let looks_encrypted = cipher.len() == 344
                    && cipher
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'/' | b'='));
                if cipher.is_empty() || looks_encrypted {
                    return Some(unsupported(
                        "Decrypt of text this session did not encrypt (the key and the empty text are not pinned)",
                    ));
                }
                Err(crate::transactions::al_error_code(
                    "Unable to decrypt data. The data was encrypted using a different key.",
                    "Encryption:Format",
                ))
            }
            _ => return None,
        })
    }
}
