//! Builtins that act on session state: number sequences and the transaction state.

use crate::Session;
use al_builtins::native::{AlError, Signal};
use al_builtins::value::Value;

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
