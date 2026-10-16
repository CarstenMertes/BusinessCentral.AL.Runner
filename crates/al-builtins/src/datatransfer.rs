//! `DataTransfer`: copies table data during upgrade and install code only. The corpus pins what it
//! does anywhere else (install/TestDataTransferContracts.al): `SetTables` is accepted and staged;
//! a call before it asks for `SetTables`; a call after it says the type is for upgrade and install
//! code. What it does inside such code is not implemented.

use crate::native::{AlError, Ctx, NativeFn, Signal};
use crate::signature::Signature;
use crate::value::{LibraryValue, Value};

type Out = Result<Value, Signal>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn error<T>(message: &str) -> Result<T, Signal> {
    Err(Signal::Error(Box::new(AlError {
        message: message.to_string(),
        // xmlport/lt-xmlport-query-datatransfer/XpTDataTransfer.al, misc/W5DataTransferOutside.al: the code is DB:Ok.
        code: "DB:Ok".into(),
        call_stack: String::new(),
    })))
}

fn set_tables(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    let (Some(Value::Integer(src)), Some(Value::Integer(dst))) = (a.first(), a.get(1)) else {
        return unsupported("DataTransfer.SetTables with table numbers that are not Integers");
    };
    let (src, dst) = (*src, *dst);
    if c.install_scope() {
        return unsupported("DataTransfer inside install code is not implemented");
    }
    if !c.workspace_table(src) || !c.workspace_table(dst) {
        return unsupported("DataTransfer.SetTables on a table outside the workspace (validation not pinned)");
    }
    let Some(Value::Library(_, cell)) = r else {
        return unsupported("DataTransfer method without a receiver");
    };
    *cell.borrow_mut() = LibraryValue::DataTransfer(Some((src, dst)));
    Ok(Value::Null)
}

/// Every staging and copying call: refused, with the text that says why.
fn other_call(c: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    let Some(Value::Library(_, cell)) = r else {
        return unsupported("DataTransfer method without a receiver");
    };
    let staged = matches!(&*cell.borrow(), LibraryValue::DataTransfer(Some(_)));
    if c.install_scope() {
        return unsupported("DataTransfer inside install code is not implemented");
    }
    if staged {
        error("DataTransfer is only usable during upgrade and installation code.")
    } else {
        error("SetTables must first be called before calling other methods on DataTransfer.")
    }
}

/// `UpdateAuditFields(Value)` and `UpdateAuditFields := Value`: accepted after `SetTables` even outside upgrade and install code,
/// where the other staging calls raise (misc/W5DataTransferOutside.al:SetTables_Twice_AndUpdateAuditFields,
/// misc/lt-probes-needed-2/PnBErrorsCollections.al:DataTransfer_OutsideInstall `op6`). Before `SetTables` and inside install code
/// it is not pinned.
fn update_audit_fields(c: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    let Some(Value::Library(_, cell)) = r else {
        return unsupported("DataTransfer method without a receiver");
    };
    let staged = matches!(&*cell.borrow(), LibraryValue::DataTransfer(Some(_)));
    if c.install_scope() || !staged {
        return unsupported("DataTransfer.UpdateAuditFields before SetTables or inside install code (not pinned)");
    }
    Ok(Value::Null)
}

pub fn native_for(sig: &Signature) -> Option<NativeFn> {
    if sig.receiver != "DataTransfer" {
        return None;
    }
    Some(match sig.name.as_str() {
        "SetTables" => set_tables,
        "UpdateAuditFields" => update_audit_fields,
        "AddConstantValue"
        | "AddDestinationFilter"
        | "AddFieldValue"
        | "AddJoin"
        | "AddSourceFilter"
        | "CopyFields"
        | "CopyRows" => other_call,
        _ => return None,
    })
}
