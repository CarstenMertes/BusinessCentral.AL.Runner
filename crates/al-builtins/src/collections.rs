//! List and Dictionary methods. The corpus pins the error of `List.Get` outside the list and of
//! `Dictionary.Get` of a missing key; the other out-of-range indexes and `Dictionary.Add` of an
//! existing key raise Unsupported.

use crate::describe::Describe;
use crate::native::{Ctx, NativeFn, Signal};
use crate::ops::compare;
use crate::value::Value;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

pub const REGISTRY: &[(&str, NativeFn)] = &[
    ("List.Add#1", |_, r, a| {
        with_list(r, |l| {
            l.push(a[0].clone());
            Ok(Value::Null)
        })
    }),
    ("List.AddRange#1", |_, r, a| {
        with_list(r, |l| {
            l.extend(a.iter().cloned());
            Ok(Value::Null)
        })
    }),
    ("List.AddRange#2", |_, r, a| {
        let items = list_items(&a[0])?;
        with_list(r, |l| {
            l.extend(items);
            Ok(Value::Null)
        })
    }),
    ("List.Contains#1", |_, r, a| {
        with_list(r, |l| Ok(Value::Boolean(position(l, &a[0]).is_some())))
    }),
    ("List.Count#1", |_, r, _| {
        with_list(r, |l| Ok(Value::Integer(l.len() as i32)))
    }),
    ("List.Get#1", |_, r, a| {
        let i = index(&a[0])?;
        with_list(r, |l| l.get(i).cloned().map_or_else(list_get_error, Ok))
    }),
    ("List.Get#2", |c, r, a| {
        let i = index(&a[0])?;
        let got = with_list(r, |l| Ok(l.get(i).cloned()))?;
        match got {
            Some(v) => {
                a[1] = v;
                Ok(Value::Boolean(true))
            }
            None if c.ret_used() => Ok(Value::Boolean(false)),
            None => list_get_error(),
        }
    }),
    ("List.IndexOf#1", |_, r, a| {
        with_list(r, |l| {
            Ok(Value::Integer(position(l, &a[0]).map_or(0, |p| p as i32 + 1)))
        })
    }),
    ("List.LastIndexOf#1", |_, r, a| {
        with_list(r, |l| {
            Ok(Value::Integer(
                l.iter().rposition(|x| eq(x, &a[0])).map_or(0, |p| p as i32 + 1),
            ))
        })
    }),
    ("List.Insert#1", |_, r, a| {
        let i = index(&a[0])?;
        with_list(r, |l| {
            if i > l.len() {
                return out_of_range();
            }
            l.insert(i, a[1].clone());
            Ok(Value::Boolean(true))
        })
    }),
    ("List.Remove#1", |_, r, a| {
        with_list(r, |l| {
            Ok(Value::Boolean(match position(l, &a[0]) {
                Some(p) => {
                    l.remove(p);
                    true
                }
                None => false,
            }))
        })
    }),
    ("List.RemoveAt#1", |_, r, a| {
        let i = index(&a[0])?;
        with_list(r, |l| {
            if i >= l.len() {
                return out_of_range();
            }
            l.remove(i);
            Ok(Value::Boolean(true))
        })
    }),
    ("List.RemoveRange#1", |_, r, a| {
        let i = index(&a[0])?;
        let n = count(&a[1])?;
        with_list(r, |l| {
            if i + n > l.len() {
                return range_past_end();
            }
            l.drain(i..i + n);
            Ok(Value::Boolean(true))
        })
    }),
    ("List.Reverse#1", |_, r, _| {
        with_list(r, |l| {
            l.reverse();
            Ok(Value::Null)
        })
    }),
    ("List.Set#1", |_, r, a| {
        let i = index(&a[0])?;
        with_list(r, |l| match l.get_mut(i) {
            Some(slot) => {
                *slot = a[1].clone();
                Ok(Value::Boolean(true))
            }
            None => out_of_range(),
        })
    }),
    ("List.GetRange#1", |_, r, a| {
        let i = index(&a[0])?;
        let n = count(&a[1])?;
        with_list(r, |l| {
            if i + n > l.len() {
                return range_past_end();
            }
            Ok(Value::List(Rc::new(RefCell::new(l[i..i + n].to_vec()))))
        })
    }),
    ("Dictionary.Add#1", |c, r, a| {
        let ret_used = c.ret_used();
        with_dict(r, |d| {
            if d.iter().any(|(k, _)| eq(k, &a[0])) {
                return if ret_used {
                    Ok(Value::Boolean(false))
                } else {
                    error_code("An item with the same key has already been added.", "Argument")
                };
            }
            d.push((a[0].clone(), a[1].clone()));
            Ok(Value::Boolean(true))
        })
    }),
    ("Dictionary.ContainsKey#1", |_, r, a| {
        with_dict(r, |d| Ok(Value::Boolean(d.iter().any(|(k, _)| eq(k, &a[0])))))
    }),
    ("Dictionary.Count#1", |_, r, _| {
        with_dict(r, |d| Ok(Value::Integer(d.len() as i32)))
    }),
    ("Dictionary.Get#1", |_, r, a| {
        with_dict(r, |d| {
            d.iter()
                .find(|(k, _)| eq(k, &a[0]))
                .map(|(_, v)| v.clone())
                .map_or_else(key_not_found, Ok)
        })
    }),
    ("Dictionary.Get#2", |c, r, a| {
        let got = with_dict(r, |d| Ok(d.iter().find(|(k, _)| eq(k, &a[0])).map(|(_, v)| v.clone())))?;
        match got {
            Some(v) => {
                a[1] = v;
                Ok(Value::Boolean(true))
            }
            None if c.ret_used() => Ok(Value::Boolean(false)),
            None => key_not_found(),
        }
    }),
    ("Dictionary.Keys#1", |_, r, _| {
        with_dict(r, |d| {
            Ok(Value::List(Rc::new(RefCell::new(
                d.iter().map(|(k, _)| k.clone()).collect(),
            ))))
        })
    }),
    ("Dictionary.Values#1", |_, r, _| {
        with_dict(r, |d| {
            Ok(Value::List(Rc::new(RefCell::new(
                d.iter().map(|(_, v)| v.clone()).collect(),
            ))))
        })
    }),
    ("Dictionary.Remove#1", |_, r, a| {
        with_dict(r, |d| {
            Ok(Value::Boolean(match d.iter().position(|(k, _)| eq(k, &a[0])) {
                Some(p) => {
                    d.remove(p);
                    true
                }
                None => false,
            }))
        })
    }),
    ("Dictionary.Set#1", |_, r, a| {
        with_dict(r, |d| {
            match d.iter_mut().find(|(k, _)| eq(k, &a[0])) {
                Some((_, v)) => *v = a[1].clone(),
                None => d.push((a[0].clone(), a[1].clone())),
            }
            Ok(Value::Null)
        })
    }),
];

fn eq(a: &Value, b: &Value) -> bool {
    compare(a, b).ok() == Some(Ordering::Equal)
}

fn position(l: &[Value], v: &Value) -> Option<usize> {
    l.iter().position(|x| eq(x, v))
}

fn error_code<T>(message: &str, code: &str) -> Result<T, Signal> {
    Err(Signal::Error(Box::new(crate::native::AlError {
        message: message.into(),
        code: code.into(),
        call_stack: String::new(),
    })))
}

const LIST_INVALID: &str = "An invalid argument was passed to a 'List' data type method.";

/// `List.Get` with an index past the end, in both forms (collections/TestListTrappableError.al:21-45).
fn list_get_error<T>() -> Result<T, Signal> {
    error_code(LIST_INVALID, "ArgumentOutOfRange")
}

/// `Dictionary.Get` of a missing key: pinned for the `Get(Key, var Value)` form
/// (collections/TestCollectionsDictionary.al:42-55); the value form is only pinned to raise
/// (collections/TestCollectionContracts.al:188), and is assumed to say the same (probes-needed).
fn key_not_found<T>() -> Result<T, Signal> {
    error_code("The given key was not present in the dictionary.", "KeyNotFound")
}

/// An index outside the list (session/TestProbeSessionMiscContracts.al:Dictionary_List_ErrorTexts: Get, RemoveAt, Set,
/// Insert and a range starting at 0 or past the count; code ArgumentOutOfRange).
fn out_of_range<T>() -> Result<T, Signal> {
    error_code(LIST_INVALID, "ArgumentOutOfRange")
}

/// A range that reaches past the end of the list (code Argument).
fn range_past_end<T>() -> Result<T, Signal> {
    error_code(LIST_INVALID, "Argument")
}

/// AL list indexes are 1-based.
fn index(v: &Value) -> Result<usize, Signal> {
    match v {
        Value::Integer(i) if *i >= 1 => Ok(*i as usize - 1),
        _ => out_of_range(),
    }
}

fn count(v: &Value) -> Result<usize, Signal> {
    match v {
        Value::Integer(i) if *i >= 0 => Ok(*i as usize),
        _ => out_of_range(),
    }
}

fn list_items(v: &Value) -> Result<Vec<Value>, Signal> {
    match v {
        Value::List(l) => Ok(l.borrow().clone()),
        other => Err(Signal::Unsupported(format!("{} is not a list", other.describe()))),
    }
}

fn with_list<T>(r: Option<&mut Value>, f: impl FnOnce(&mut Vec<Value>) -> Result<T, Signal>) -> Result<T, Signal> {
    match r {
        Some(Value::List(l)) => f(&mut l.borrow_mut()),
        other => Err(Signal::Unsupported(format!("list method on {}", other.describe()))),
    }
}

fn with_dict<T>(
    r: Option<&mut Value>,
    f: impl FnOnce(&mut Vec<(Value, Value)>) -> Result<T, Signal>,
) -> Result<T, Signal> {
    match r {
        Some(Value::Dictionary(d)) => f(&mut d.borrow_mut()),
        other => Err(Signal::Unsupported(format!(
            "dictionary method on {}",
            other.describe()
        ))),
    }
}

#[allow(dead_code)]
fn _ctx(_: &dyn Ctx) {}
