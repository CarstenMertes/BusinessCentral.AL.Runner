//! `NavApp.GetCurrentModuleInfo` and friends, and the `ModuleInfo` type: what an app's manifest
//! says about it (corpus session/TestNavAppExtended.al, TestNavAppModuleInfoCallerIdentity.al,
//! TestNavAppCallerCallstackModuleInfos.al, TestBCPlatformContracts.al, TestMiscTypes.al).
//!
//! Resources, entitlements and archive data of an app are not here.

use crate::native::{AlError, Ctx, ModuleLookup, NativeFn, Signal};
use crate::signature::Signature;
use crate::value::{LibraryValue, ModuleInfoState, Value};
use std::cell::RefCell;
use std::rc::Rc;

type Out = Result<Value, Signal>;

pub const REGISTRY: &[(&str, NativeFn)] = &[
    ("NavApp.GetCurrentModuleInfo#1", get_current),
    ("NavApp.GetCallerModuleInfo#1", get_caller),
    ("NavApp.GetModuleInfo#1", get_module),
    ("NavApp.GetCallstackModuleInfos#1", |c, _, _| callstack(c, 0)),
    ("NavApp.GetCallerCallstackModuleInfos#1", |c, _, _| callstack(c, 1)),
    // True while an install codeunit runs (session/TestNavAppExtended.al, TestBCPlatformContracts.al: false in a test).
    ("NavApp.IsInstalling#1", |c, _, _| Ok(Value::Boolean(c.install_scope()))),
];

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn module_value(m: ModuleInfoState) -> Value {
    Value::Library("ModuleInfo".into(), Rc::new(RefCell::new(LibraryValue::ModuleInfo(m))))
}

fn set_info(args: &mut [Value], m: ModuleInfoState) -> Out {
    match args.first_mut() {
        Some(slot) => {
            *slot = module_value(m);
            Ok(Value::Boolean(true))
        }
        None => unsupported("a module info call without its var parameter"),
    }
}

fn get_current(c: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    match c.module_at_frame(0) {
        Some(m) => set_info(a, m),
        None => unsupported("the app of the running code has no readable manifest id"),
    }
}

/// The immediate caller's app, even when it is the same app
/// (session/TestNavAppModuleInfoCallerIdentity.al:15-20).
fn get_caller(c: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    match c.module_at_frame(1) {
        Some(m) => set_info(a, m),
        None => unsupported("GetCallerModuleInfo with no caller in the workspace (the test runner calls a test)"),
    }
}

fn guid_arg(v: &Value) -> Option<[u8; 16]> {
    match v {
        Value::Guid(g) => Some(*g),
        other => crate::native::text_of(other).and_then(|t| crate::ops::parse_guid(&t)),
    }
}

fn plain_guid(g: &[u8; 16]) -> String {
    let h: String = g.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// `GetModuleInfo(AppId, var Info)`: an installed app, else false when the result is used, else an
/// error that names the id (session/TestNavAppExtended.al:118-150: "No installed extension was
/// found with ID" and the id). The null id answers with a Microsoft module there: refused.
fn get_module(c: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    let Some(id) = a.first().and_then(guid_arg) else {
        return unsupported(format!("GetModuleInfo with {:?} as the app id", a.first()));
    };
    if id == [0; 16] {
        return unsupported("GetModuleInfo of the null id answers a synthesized Microsoft module (not modelled)");
    }
    match c.module_by_id(&id) {
        ModuleLookup::Found(m) => {
            if a.len() < 2 {
                return unsupported("GetModuleInfo without its var parameter");
            }
            a[1] = module_value(m);
            Ok(Value::Boolean(true))
        }
        ModuleLookup::OutsideWorkspace => {
            unsupported("GetModuleInfo of an app outside the workspace (Microsoft apps, ADR-013)")
        }
        ModuleLookup::NotInstalled if c.ret_used() => Ok(Value::Boolean(false)),
        ModuleLookup::NotInstalled => Err(Signal::Error(AlError {
            message: format!("No installed extension was found with ID {}.", plain_guid(&id)),
            code: String::new(),
            call_stack: String::new(),
        })),
    }
}

/// The apps on the call stack, each once, innermost first
/// (session/TestNavAppCallerCallstackModuleInfos.al).
fn callstack(c: &mut dyn Ctx, skip: usize) -> Out {
    let items = c.module_callstack(skip).into_iter().map(module_value).collect();
    Ok(Value::List(Rc::new(RefCell::new(items))))
}

fn state(r: Option<&mut Value>) -> Result<ModuleInfoState, Signal> {
    match r {
        Some(Value::Library(_, cell)) => match &*cell.borrow() {
            LibraryValue::ModuleInfo(m) => Ok(m.clone()),
            other => unsupported(format!("a ModuleInfo nobody filled ({other:?})")),
        },
        other => unsupported(format!("a ModuleInfo method on {other:?}")),
    }
}

fn version_value(v: [i32; 4]) -> Value {
    Value::Library("Version".into(), Rc::new(RefCell::new(LibraryValue::Version(v))))
}

fn app_version(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    match state(r)?.version {
        Some(v) => Ok(version_value(v)),
        None => unsupported("the manifest of this app has no version the runtime can read"),
    }
}

/// A freshly installed app's data version is its app version (not pinned further: the corpus asks
/// only that a version comes back, session/TestMiscTypes.al).
fn data_version(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    app_version(c, r, a)
}

/// Not null: the package id is a second id the manifest does not carry; it is the app id with its last
/// byte flipped, which differs from it and is stable (session/TestMiscTypes.al, TestBCPlatformContracts.al
/// ask only that it is not null).
fn package_id(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    let mut id = state(r)?.id;
    id[15] ^= 0x01;
    Ok(Value::Guid(id))
}

fn dependencies(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    let items = state(r)?
        .dependencies
        .iter()
        .map(|d| {
            Value::Library(
                "ModuleDependencyInfo".into(),
                Rc::new(RefCell::new(LibraryValue::Text(plain_guid(d)))),
            )
        })
        .collect();
    Ok(Value::List(Rc::new(RefCell::new(items))))
}

pub fn native_for(sig: &Signature) -> Option<NativeFn> {
    if sig.receiver != "ModuleInfo" || !sig.instance {
        return None;
    }
    let f: NativeFn = match sig.name.as_str() {
        "Id" => |_, r, _| Ok(Value::Guid(state(r)?.id)),
        "Name" => |_, r, _| Ok(Value::Text(state(r)?.name)),
        "Publisher" => |_, r, _| Ok(Value::Text(state(r)?.publisher)),
        "AppVersion" => app_version,
        "DataVersion" => data_version,
        "PackageId" => package_id,
        "Dependencies" => dependencies,
        _ => return None,
    };
    Some(f)
}
