//! Interface variables: casting a value to an interface and calling through one.
//!
//! An interface variable holds the codeunit instance it was assigned, or `Null` while
//! unassigned. The lowering resolved which codeunit answers for an enum value (corpus
//! types/TestEnumDefaultImplTests.Codeunit.al) and lists every implementing routine of an
//! interface procedure; the VM only picks among them by the instance's object.

use crate::{Frame, Session, Signal};
use al_builtins::value::{Instance, Value};
use al_ir as ir;
use std::cell::RefCell;
use std::rc::Rc;

impl Session {
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
            // The error text of calling an unassigned interface is not pinned by the corpus
            // (docs/probes-needed.md).
            Value::Null => Err(Signal::Unsupported(
                "a call on an interface variable that holds no implementation (error text not pinned)".into(),
            )),
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
            None => unknown.ok_or_else(|| {
                Signal::Unsupported(format!(
                    "interface cast of enum ordinal {ordinal}, which the enum does not declare and has no \
                     UnknownValueImplementation (behaviour not pinned)"
                ))
            })?,
        };
        Ok(Value::Codeunit(self.instance(object)))
    }
}
