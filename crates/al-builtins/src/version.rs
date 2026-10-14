//! `Version`: four numbers read by `Evaluate` (corpus session/TestMiscTypes.al:44-80).
//! `Create` and `ToText` are not pinned and are not implemented.

use crate::describe::Describe;
use crate::native::{Ctx, NativeFn, Signal};
use crate::signature::Signature;
use crate::value::{LibraryValue, Value};

fn part(r: Option<&mut Value>, i: usize) -> Result<Value, Signal> {
    match r {
        Some(Value::Library(_, cell)) => match &*cell.borrow() {
            LibraryValue::Version(v) => Ok(Value::Integer(v[i])),
            other => Err(Signal::Unsupported(format!(
                "a Version part of {other} (never evaluated)",
                other = other.describe()
            ))),
        },
        other => Err(Signal::Unsupported(format!("a Version part of {}", other.describe()))),
    }
}

pub fn native_for(sig: &Signature) -> Option<NativeFn> {
    if sig.receiver != "Version" || !sig.instance {
        return None;
    }
    let f: NativeFn = match sig.name.as_str() {
        "Major" => |_: &mut dyn Ctx, r, _| part(r, 0),
        "Minor" => |_: &mut dyn Ctx, r, _| part(r, 1),
        "Build" => |_: &mut dyn Ctx, r, _| part(r, 2),
        "Revision" => |_: &mut dyn Ctx, r, _| part(r, 3),
        _ => return None,
    };
    Some(f)
}
