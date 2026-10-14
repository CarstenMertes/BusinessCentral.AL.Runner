//! Interface variables: casting a value to an interface and calling through one.
//!
//! An interface variable holds the codeunit instance it was assigned, or `Null` while
//! unassigned. The lowering resolved which codeunit answers for an enum value (corpus
//! types/TestEnumDefaultImplTests.Codeunit.al) and lists every implementing routine of an
//! interface procedure; the VM only picks among them by the instance's object.

use crate::{Frame, Session, Signal};
use al_builtins::native::AlError;
use al_builtins::value::{Instance, Value};
use al_ir as ir;
use std::cell::RefCell;
use std::rc::Rc;

/// A call on an interface variable that holds no implementation
/// (codeunit/TestInterfaceRuntimeContracts.al:Interface_CallOnUnassignedVariable_Throws_
/// InterfaceNotInitialized, corpus branch al-runtime-rs, probe on BC 27.5).
fn not_initialized() -> Signal {
    Signal::Error(AlError {
        message: "Interface not initialized".into(),
        code: "InvalidOperation".into(),
        call_stack: String::new(),
    })
}

impl Session {
    /// `value is I` / `value as I`: the instance the interface variable holds is a codeunit
    /// from `objects`. The corpus pins only the instance that does implement `I` (codeunit/
    /// TestInterfaceRuntimeContracts.al:Interface_ExtendedInterface_IsAndAs); `is` of an
    /// instance that does not is a plain `false`, and the failure of `as` and either operator on
    /// an unassigned variable are refused (docs/probes-needed.md).
    pub(crate) fn interface_test(&mut self, value: Value, objects: &[u32], cast: bool) -> Result<Value, Signal> {
        match value {
            Value::Codeunit(i) => {
                let implements = objects.contains(&i.borrow().object);
                match (cast, implements) {
                    (false, hit) => Ok(Value::Boolean(hit)),
                    (true, true) => Ok(Value::Codeunit(i)),
                    (true, false) => Err(Signal::Unsupported(
                        "`as` of an interface value to an interface its instance does not implement (error text not pinned)"
                            .into(),
                    )),
                }
            }
            Value::Null => Err(Signal::Unsupported(
                "`is` or `as` on an interface variable that holds no implementation (not pinned)".into(),
            )),
            other => Err(Signal::Unsupported(format!("`is` or `as` on {other:?}"))),
        }
    }

    /// The routine an interface call runs: the target belonging to the object of the instance
    /// the receiver holds, and that instance.
    pub(crate) fn interface_target(
        &mut self,
        receiver: &ir::Expr,
        targets: &[(usize, usize)],
        f: &mut Frame,
    ) -> Result<(usize, usize, Rc<RefCell<Instance>>), Signal> {
        match self.eval(receiver, f)? {
            Value::Codeunit(i) => {
                let object = i.borrow().object as usize;
                match targets.iter().find(|(o, _)| *o == object) {
                    Some((o, r)) => Ok((*o, *r, self.shared_instance(i))),
                    None => Err(Signal::Unsupported(format!(
                        "interface call on an instance of `{}`, which the interface does not list",
                        self.engine.program.objects[object].name
                    ))),
                }
            }
            // codeunit/TestInterfaceRuntimeContracts.al:Interface_CallOnUnassignedVariable_Throws_
            // InterfaceNotInitialized (corpus branch al-runtime-rs, probe on BC 27.5).
            Value::Null => Err(not_initialized()),
            other => Err(Signal::Unsupported(format!("interface call on {other:?}"))),
        }
    }

    /// An enum value cast to an interface: a fresh, lazily initialised instance of the
    /// implementing codeunit (corpus types/TestInterfaceCodeunitStateField.al: the
    /// implementation's own state must be alive after the cast).
    pub(crate) fn enum_implementation(
        &mut self,
        value: Value,
        by_ordinal: &[(i32, usize)],
        unknown: Option<usize>,
    ) -> Result<Value, Signal> {
        let Value::Enum(_, ordinal) = value else {
            return Err(Signal::Unsupported(format!("enum cast of {value:?}")));
        };
        let object = match by_ordinal.iter().find(|(o, _)| *o == ordinal) {
            Some((_, c)) => *c,
            // An ordinal the enum does not declare and no UnknownValueImplementation to answer for
            // it: codeunit/TestInterfaceRuntimeContracts.al:Interface_CastUndeclaredEnumOrdinal
            // WithoutUnknownImplementation_Throws (corpus branch al-runtime-rs, probe on BC 27.5).
            None => unknown.ok_or_else(|| {
                Signal::Error(AlError {
                    message: format!("Enum does not contain value {ordinal}."),
                    code: "ArgumentOutOfRange".into(),
                    call_stack: String::new(),
                })
            })?,
        };
        Ok(Value::Codeunit(self.instance(object)))
    }
}
