//! Builtins that act on session state: number sequences and the transaction state.

use crate::Session;
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

fn big(args: &[Value], i: usize) -> Option<i64> {
    match args.get(i)? {
        Value::BigInteger(n) => Some(*n),
        Value::Integer(n) => Some(i64::from(*n)),
        _ => None,
    }
}

impl Session {
    /// The key of sequence `args[0]`; `scope` is the position of the optional CompanySpecific
    /// argument (company specific when omitted, as the default overload is: session/TestNumberSequence.al:15-30).
    fn sequence_key(&self, args: &[Value], scope: usize) -> Result<(String, String), Signal> {
        let Some(name) = args.first().and_then(al_builtins::native::text_of) else {
            return unsupported("a number sequence name that is not text");
        };
        let company_specific = match args.get(scope) {
            None => true,
            Some(Value::Boolean(b)) => *b,
            Some(other) => return unsupported(format!("CompanySpecific of {other:?}")),
        };
        let company = if company_specific {
            self.cx.company.clone()
        } else {
            String::new()
        };
        Ok((company, name))
    }

    pub(crate) fn session_op(&mut self, id: &str, args: &mut [Value]) -> Result<Value, Signal> {
        if id == "Database.IsInWriteTransaction#1" {
            // An uncommitted write opens one, a Commit and the test boundary close it
            // (codeunit/TestWriteTxTestBoundary.al:93,103,142). Writes to temporary tables are
            // not counted: they never reach the database (not pinned).
            return Ok(Value::Boolean(self.store.has_uncommitted()));
        }
        if id == "Database.CompanyName#1" {
            // The company of the session, the same name `Record.CurrentCompany` answers
            // (session/TestDatabase.al:69-75, session/TestSessionFunctions.al:25).
            return Ok(Value::Text(self.cx.company.clone()));
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
                Some(other) => unsupported(format!("CodeCoverageLog with {other:?}")),
            };
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
        let missing = || error::<Value>(format!("The number sequence {name} does not exist."));
        match method {
            // Documentation-only texts: the corpus pins the fragments "already exists" and "does
            // not exist" (session/TestNumberSequence.al:177-182,188-200).
            "Insert" => {
                // Seed and Increment have defaults in the docs, but every corpus call passes both.
                let (Some(seed), Some(increment)) = (big(args, 1), big(args, 2)) else {
                    return unsupported("NumberSequence.Insert without a Seed and an Increment (defaults not pinned)");
                };
                if self.sequences.contains_key(&key) {
                    return error(format!("The number sequence {name} already exists."));
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
                        other => return unsupported(format!("NumberSequence.Range with Count {other:?} (not pinned)")),
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
                let Some(seed) = big(args, 1) else {
                    return unsupported("NumberSequence.Restart without a Seed (default not pinned)");
                };
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
            Some(other) => unsupported(format!("IsolatedStorage with data scope {other:?}")),
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
