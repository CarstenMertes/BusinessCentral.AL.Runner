//! The .NET exception `GetLastErrorObject` answers, and the `System.Type` its `GetType()` gives. The runtime
//! provides them itself (ADR-014 section 1, ADR-038), so a run needs no .NET host.
//!
//! Pinned by tests/al-language-onprem-dotnet/lt-dotnet-interop/GetLastErrorObject.al (BC 27, 28, 29):
//! - `Message` is the text `GetLastErrorText` answers (:80), `InnerException` is null for a dialog and a
//!   record-not-found exception and not null for a `NavALException` (:83, :155, :251, :275);
//! - `GetType().FullName` is the exception class (:81-82);
//! - `Format(Exc)` is `Exc.ToString()` (:85); its first line is `<class>: <message>` and the text has `   at `
//!   stack frames (:86-87).
//!
//! Not pinned, so refused (a registry type refuses every member it does not list): `Name`, `Equals`,
//! `StackTrace`, `Source`, `Data`, `HResult`, the members of the inner exception, `GetBaseException`.
//! The stack frames themselves are .NET's and are free to differ (ADR-009: diagnostics): each frame line here is
//! an AL frame of the error, in the form of `GetLastErrorCallStack`.

use super::registry::{MemberKind, Pin, RegistryMember, RegistryType};
use crate::Session;
use al_builtins::dotnet_sigs::{
    AL_EXCEPTION, AL_INNER_EXCEPTION, NCL_DIALOG_EXCEPTION, NULL_EXCEPTION, RECORD_NOT_FOUND_EXCEPTION, RUNTIME_TYPE,
};
use al_builtins::native::{EXC_CALL_STACK, EXC_MESSAGE, Signal, UNTRACKED_CALL_STACK};
use al_builtins::value::{DotNetKind, DotNetObject, Value};
use std::cell::RefCell;
use std::rc::Rc;

const PIN: &str = "tests/al-language-onprem-dotnet/lt-dotnet-interop/GetLastErrorObject.al:79-87";

fn pin() -> Pin {
    Pin::Corpus(PIN.into())
}

fn unsupported<T>(why: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(why.into()))
}

fn parts(this: Option<&Rc<DotNetObject>>) -> Result<(&str, Value, Value), Signal> {
    match this.map(|o| &o.kind) {
        Some(DotNetKind::Internal { type_name, state }) => {
            let s = state.borrow();
            Ok((type_name.as_str(), s[EXC_MESSAGE].clone(), s[EXC_CALL_STACK].clone()))
        }
        _ => unsupported("DotNet exception member on something that is not a last-error exception"),
    }
}

/// Whether `type_name` is one of the exception classes `GetLastErrorObject` answers.
pub(crate) fn is_exception(type_name: &str) -> bool {
    [NCL_DIALOG_EXCEPTION, RECORD_NOT_FOUND_EXCEPTION, AL_EXCEPTION].contains(&type_name)
}

fn message(_: &mut Session, this: Option<&Rc<DotNetObject>>, _: &[Value]) -> Result<Value, Signal> {
    Ok(parts(this)?.1)
}

fn inner_exception(_: &mut Session, this: Option<&Rc<DotNetObject>>, _: &[Value]) -> Result<Value, Signal> {
    let (type_name, ..) = parts(this)?;
    if type_name != AL_EXCEPTION {
        return Ok(Value::Null);
    }
    Ok(Value::DotNet(Rc::new(DotNetObject::new(DotNetKind::Internal {
        type_name: AL_INNER_EXCEPTION.into(),
        state: RefCell::new(Vec::new()),
    }))))
}

fn get_type(_: &mut Session, this: Option<&Rc<DotNetObject>>, _: &[Value]) -> Result<Value, Signal> {
    let (type_name, ..) = parts(this)?;
    Ok(Value::DotNet(Rc::new(DotNetObject::new(DotNetKind::Internal {
        type_name: RUNTIME_TYPE.into(),
        state: RefCell::new(vec![Value::Text(type_name.into())]),
    }))))
}

/// `<class>: <message>`, then one `   at <frame>` line per AL frame of the error.
pub(crate) fn exception_text(this: &Rc<DotNetObject>) -> Result<String, Signal> {
    let (type_name, msg, stack) = parts(Some(this))?;
    let (Value::Text(msg), Value::Text(stack)) = (msg, stack) else {
        return unsupported("a last-error exception without its message");
    };
    let mut out = format!("{type_name}: {msg}");
    let frames: Vec<&str> = stack
        .split('\n')
        .filter(|f| !f.is_empty() && *f != UNTRACKED_CALL_STACK)
        .collect();
    if frames.is_empty() {
        out.push_str("\n   at (the AL call stack of this error is not tracked)");
    }
    for f in frames {
        out.push_str("\n   at ");
        out.push_str(f);
    }
    Ok(out)
}

fn to_string(_: &mut Session, this: Option<&Rc<DotNetObject>>, _: &[Value]) -> Result<Value, Signal> {
    let Some(this) = this else {
        return unsupported("DotNet exception ToString without an object");
    };
    Ok(Value::Text(exception_text(this)?.into()))
}

fn full_name(_: &mut Session, this: Option<&Rc<DotNetObject>>, _: &[Value]) -> Result<Value, Signal> {
    match this.map(|o| &o.kind) {
        Some(DotNetKind::Internal { state, .. }) => Ok(state.borrow()[0].clone()),
        _ => unsupported("DotNet Type member on something that is not a runtime type"),
    }
}

fn member(name: &str, kind: MemberKind, f: super::registry::MemberFn) -> RegistryMember {
    RegistryMember {
        name: name.into(),
        kind,
        pin: pin(),
        call: f,
        call_ref: None,
    }
}

fn exception(clr_name: &str) -> RegistryType {
    RegistryType {
        clr_name: clr_name.into(),
        members: vec![
            member("Message", MemberKind::Get, message),
            member("InnerException", MemberKind::Get, inner_exception),
            member("GetType", MemberKind::Call, get_type),
            member("ToString", MemberKind::Call, to_string),
        ],
    }
}

/// A member of the "no last error" object: the error of an unassigned DotNet variable, naming the declared
/// type System.Exception (GetLastErrorObject.al:296-299, `Message`, `GetType` and `ToString`).
fn not_instantiated(s: &mut Session, member: &str) -> Result<Value, Signal> {
    let Some((&object, &frame)) = s.frames.last().zip(s.call_stack.last()) else {
        return unsupported(format!(
            "System.Exception.{member} on a null exception outside a routine"
        ));
    };
    let o = &s.engine.program.objects[object];
    if o.kind != al_ir::ObjKind::Codeunit || o.uncompiled {
        return unsupported(format!(
            "System.Exception.{member} on a null exception outside a codeunit (the object kind's spelling in the error is not pinned)"
        ));
    }
    let routine = o.routines.get(frame.routine).map_or("", |r| r.name.as_str());
    let message = format!(
        "A DotNet variable has not been instantiated. Attempting to call System.Exception.{member} in CodeUnit {}: {routine}",
        o.name
    );
    al_builtins::native::raise(message, "DotNetVariableNotCreated")
}

fn null_message(s: &mut Session, _: Option<&Rc<DotNetObject>>, _: &[Value]) -> Result<Value, Signal> {
    not_instantiated(s, "Message")
}

fn null_get_type(s: &mut Session, _: Option<&Rc<DotNetObject>>, _: &[Value]) -> Result<Value, Signal> {
    not_instantiated(s, "GetType")
}

fn null_to_string(s: &mut Session, _: Option<&Rc<DotNetObject>>, _: &[Value]) -> Result<Value, Signal> {
    not_instantiated(s, "ToString")
}

pub fn types() -> Vec<RegistryType> {
    vec![
        exception(NCL_DIALOG_EXCEPTION),
        exception(RECORD_NOT_FOUND_EXCEPTION),
        exception(AL_EXCEPTION),
        // The inner exception of a NavALException: only that it exists is pinned.
        RegistryType {
            clr_name: AL_INNER_EXCEPTION.into(),
            members: Vec::new(),
        },
        RegistryType {
            clr_name: NULL_EXCEPTION.into(),
            members: vec![
                member("Message", MemberKind::Get, null_message),
                member("GetType", MemberKind::Call, null_get_type),
                member("ToString", MemberKind::Call, null_to_string),
            ],
        },
        RegistryType {
            clr_name: RUNTIME_TYPE.into(),
            members: vec![member("FullName", MemberKind::Get, full_name)],
        },
    ]
}
