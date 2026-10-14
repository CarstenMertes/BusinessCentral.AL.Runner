//! `ErrorInfo`: a property bag describing an error (corpus errorinfo/TestErrorInfoType.al,
//! TestErrorInfoErrorType.al). Raising one (`Error(ErrorInfo)`), collecting errors and the
//! callback of an action are not here: they need the error pipeline.

use crate::describe::Describe;
use crate::native::{Ctx, NativeFn, Signal};
use crate::signature::Signature;
use crate::value::{ErrorInfoState, LibraryValue, Value};
use std::cell::RefCell;
use std::rc::Rc;

type Out = Result<Value, Signal>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn option(type_name: &str, ordinal: i32) -> Value {
    let members: Rc<[String]> = crate::sysenum::option_members(type_name)
        .expect("a system option")
        .into();
    Value::Option(ordinal, members)
}

fn with_state<R>(r: Option<&mut Value>, f: impl FnOnce(&mut ErrorInfoState) -> R) -> Result<R, Signal> {
    let Some(Value::Library(_, cell)) = r else {
        return unsupported("ErrorInfo method without a receiver");
    };
    match &mut *cell.borrow_mut() {
        LibraryValue::ErrorInfo(s) => Ok(f(s)),
        // A declared but never created ErrorInfo: what its defaults are is not pinned.
        other => unsupported(format!(
            "ErrorInfo method on an ErrorInfo nobody created ({})",
            other.describe()
        )),
    }
}

fn text(a: &[Value], i: usize) -> Result<String, Signal> {
    match a.get(i).and_then(crate::native::text_of) {
        Some(t) => Ok(t),
        None => unsupported(format!("a text argument, got {}", a.get(i).describe())),
    }
}

fn int(a: &[Value], i: usize) -> Result<i32, Signal> {
    match a.get(i) {
        Some(Value::Integer(n)) => Ok(*n),
        other => unsupported(format!("an Integer argument, got {}", other.describe())),
    }
}

fn option_arg(a: &[Value], i: usize) -> Result<i32, Signal> {
    match a.get(i) {
        Some(Value::Option(o, _)) => Ok(*o),
        other => unsupported(format!("an option argument, got {}", other.describe())),
    }
}

fn fresh(message: String, collectible: bool) -> ErrorInfoState {
    ErrorInfoState {
        message,
        title: String::new(),
        detailed_message: String::new(),
        control_name: String::new(),
        // Verbosity::Error and DataClassification::CustomerContent (errorinfo/TestErrorInfoType.al:30-34).
        verbosity: 1,
        data_classification: 0,
        error_type: None,
        collectible,
        table_id: 0,
        record_id: None,
        field_no: 0,
        page_no: 0,
        custom_dimensions: Vec::new(),
    }
}

fn wrap(s: ErrorInfoState) -> Value {
    Value::Library("ErrorInfo".into(), Rc::new(RefCell::new(LibraryValue::ErrorInfo(s))))
}

/// `ErrorInfo.Create()` is collectible; `Create(Message[, Collectible, ...])` is not unless asked
/// (errorinfo/TestErrorInfoType.al:21-60,:225-245).
fn create(ctx: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        return Ok(wrap(fresh(String::new(), true)));
    }
    let collectible = match a.get(1) {
        None => false,
        Some(Value::Boolean(b)) => *b,
        Some(other) => return unsupported(format!("ErrorInfo.Create with Collectible {}", other.describe())),
    };
    let mut s = fresh(text(a, 0)?, collectible);
    if let Some(rec @ Value::Record(_)) = a.get_mut(2) {
        let id = ctx.record("RecordId", Some(rec), &mut [])?;
        if let Value::Library(_, l) = &id {
            if let LibraryValue::RecordId { table_no, .. } = &*l.borrow() {
                s.table_id = *table_no as i32;
            }
        }
        s.record_id = Some(id);
    } else if a.get(2).is_some() {
        return unsupported("ErrorInfo.Create with a record argument that is not a record");
    }
    if a.len() > 3 {
        s.field_no = int(a, 3)?;
    }
    if a.len() > 4 {
        s.page_no = int(a, 4)?;
    }
    if a.len() > 5 {
        s.control_name = text(a, 5)?;
    }
    if a.len() > 6 {
        s.verbosity = option_arg(a, 6)?;
    }
    if a.len() > 7 {
        s.data_classification = option_arg(a, 7)?;
    }
    if a.len() > 8 {
        return unsupported("ErrorInfo.Create with custom dimensions (not pinned)");
    }
    Ok(wrap(s))
}

macro_rules! text_prop {
    ($name:ident, $field:ident) => {
        fn $name(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
            if a.is_empty() {
                return with_state(r, |s| Value::Text(s.$field.clone()));
            }
            let t = text(a, 0)?;
            with_state(r, |s| s.$field = t).map(|()| Value::Null)
        }
    };
}
text_prop!(message, message);
text_prop!(title, title);
text_prop!(detailed_message, detailed_message);
text_prop!(control_name, control_name);

macro_rules! int_prop {
    ($name:ident, $field:ident) => {
        fn $name(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
            if a.is_empty() {
                return with_state(r, |s| Value::Integer(s.$field));
            }
            let n = int(a, 0)?;
            with_state(r, |s| s.$field = n).map(|()| Value::Null)
        }
    };
}
int_prop!(field_no, field_no);
int_prop!(page_no, page_no);
int_prop!(table_id, table_id);

macro_rules! option_prop {
    ($name:ident, $field:ident, $ty:literal) => {
        fn $name(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
            if a.is_empty() {
                return with_state(r, |s| option($ty, s.$field));
            }
            let o = option_arg(a, 0)?;
            with_state(r, |s| s.$field = o).map(|()| Value::Null)
        }
    };
}
option_prop!(verbosity, verbosity, "Verbosity");
option_prop!(data_classification, data_classification, "DataClassification");

fn collectible(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        return with_state(r, |s| Value::Boolean(s.collectible));
    }
    let Some(Value::Boolean(b)) = a.first() else {
        return unsupported(format!("Collectible with {}", a.first().describe()));
    };
    let b = *b;
    with_state(r, |s| s.collectible = b).map(|()| Value::Null)
}

/// Both members round-trip (errorinfo/TestErrorInfoErrorType.al:25-45); the default is not pinned.
fn error_type(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        return with_state(r, |s| s.error_type)?
            .map(|o| option("ErrorType", o))
            .ok_or_else(|| {
                Signal::Unsupported("ErrorInfo.ErrorType before one was set (the default is not pinned)".into())
            });
    }
    let o = option_arg(a, 0)?;
    with_state(r, |s| s.error_type = Some(o)).map(|()| Value::Null)
}

fn record_id(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if !a.is_empty() {
        return unsupported("setting ErrorInfo.RecordId (not pinned)");
    }
    Ok(with_state(r, |s| s.record_id.clone())?.unwrap_or_else(|| Value::Library("RecordId".into(), Rc::default())))
}

/// A fresh ErrorInfo carries no custom dimensions; an assigned dictionary comes back entry for entry
/// (errorinfo/TestErrorInfoType.al:313-345). Both directions copy.
fn custom_dimensions(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    if a.is_empty() {
        let entries = with_state(r, |s| s.custom_dimensions.clone())?;
        let pairs = entries
            .into_iter()
            .map(|(k, v)| (Value::Text(k), Value::Text(v)))
            .collect();
        return Ok(Value::Dictionary(Rc::new(RefCell::new(pairs))));
    }
    let Some(Value::Dictionary(d)) = a.first() else {
        return unsupported(format!("CustomDimensions with {}", a.first().describe()));
    };
    let mut entries = Vec::new();
    for (k, v) in d.borrow().iter() {
        let (Some(k), Some(v)) = (crate::native::text_of(k), crate::native::text_of(v)) else {
            return unsupported("CustomDimensions with a key or value that is not text");
        };
        entries.push((k, v));
    }
    with_state(r, |s| s.custom_dimensions = entries).map(|()| Value::Null)
}

/// Actions are stored for the client; there is none in a test, so they only have to bind and not
/// disturb the object (errorinfo/TestErrorInfoType.al:523-557).
fn add_action(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> Out {
    text(a, 0)?;
    int(a, 1)?;
    text(a, 2)?;
    with_state(r, |_| ()).map(|()| Value::Null)
}

fn add_navigation_action(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    with_state(r, |_| ()).map(|()| Value::Null)
}

/// `Error(ErrorInfo)`: raises the message as it is for a client error
/// (errorinfo/TestErrorInfoErrorType.al:50-65). An internal error hides its message behind a text
/// the corpus does not pin: refused. A collectible one is collected inside a collecting scope.
pub fn raise(c: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    let Some(info @ Value::Library(_, cell)) = a.first() else {
        return unsupported(format!("Error with {} as an ErrorInfo", a.first().describe()));
    };
    let (message, internal, collectible) = match &*cell.borrow() {
        LibraryValue::ErrorInfo(s) => (s.message.clone(), s.error_type == Some(1), s.collectible),
        _ => return unsupported("Error with an ErrorInfo nobody created"),
    };
    if internal {
        return unsupported("Error with ErrorType::Internal (the masking text is not pinned)");
    }
    // A collectible error inside a `[ErrorBehavior(Collect)]` scope is kept and execution goes on
    // (errorinfo/TestErrorInfoType.al:448-459); a non-collectible one unwinds even there (:474-490).
    if collectible && c.collect_error(info) {
        return Ok(Value::Null);
    }
    Err(Signal::Error(crate::native::AlError {
        message,
        code: String::new(),
        call_stack: String::new(),
    }))
}

/// A copy of an `ErrorInfo` value that shares nothing with the original.
pub fn copy_of(v: &Value) -> Option<Value> {
    let Value::Library(name, cell) = v else { return None };
    match &*cell.borrow() {
        LibraryValue::ErrorInfo(s) => Some(Value::Library(
            name.clone(),
            Rc::new(RefCell::new(LibraryValue::ErrorInfo(s.clone()))),
        )),
        _ => None,
    }
}

/// The message of an `ErrorInfo` value.
pub fn message_of(v: &Value) -> Option<String> {
    let Value::Library(_, cell) = v else { return None };
    match &*cell.borrow() {
        LibraryValue::ErrorInfo(s) => Some(s.message.clone()),
        _ => None,
    }
}

pub fn native_for(sig: &Signature) -> Option<NativeFn> {
    if sig.receiver != "ErrorInfo" {
        return None;
    }
    Some(match sig.name.as_str() {
        "Create" => create,
        "Message" => message,
        "Title" => title,
        "DetailedMessage" => detailed_message,
        "ControlName" => control_name,
        "FieldNo" => field_no,
        "PageNo" => page_no,
        "TableId" => table_id,
        "Verbosity" => verbosity,
        "DataClassification" => data_classification,
        "Collectible" => collectible,
        "ErrorType" => error_type,
        "RecordId" => record_id,
        "CustomDimensions" => custom_dimensions,
        "AddAction" => add_action,
        "AddNavigationAction" => add_navigation_action,
        _ => return None,
    })
}
