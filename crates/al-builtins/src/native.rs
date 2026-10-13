//! Native builtin implementations, registered by catalog overload id (ADR-008).

use crate::format::{FormatCtx, try_format};
use crate::value::Value;
use std::rc::Rc;

/// The error state AL code can read back (`GetLastErrorText`, ...).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AlError {
    pub message: String,
    pub code: String,
    pub call_stack: String,
}

/// Non-local control flow (ADR-002, ADR-011).
#[derive(Debug, Clone)]
pub enum Signal {
    Error(AlError),
    Exit(Option<Value>),
    Break,
    /// Behaviour the runtime does not implement yet, met at run time (an edge
    /// case no corpus test pins). The runner reports the test as not run.
    Unsupported(String),
}

/// What builtins need from the running session.
pub trait Ctx: FormatCtx {
    fn last_error(&self) -> AlError;
    fn set_last_error(&mut self, e: AlError);
    /// Record methods (`Record.Insert`, ...) need the store, triggers and
    /// table metadata: the VM implements them. `method` is the catalog name.
    fn record(&mut self, method: &str, rec: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal>;
    /// `TestPage` / `TestField` / `TestAction` methods: the VM's test client
    /// implements them (ADR-012). `method` is `Receiver.Name`.
    fn page(&mut self, method: &str, _recv: Option<&mut Value>, _args: &mut [Value]) -> Result<Value, Signal> {
        Err(Signal::Unsupported(format!("{method} needs a page runtime")))
    }

    /// `RecordRef`, `FieldRef` and `KeyRef` methods need the table schema and
    /// the store, like record methods. `id` is the catalog overload id
    /// (`RecordRef.Open#2`); the VM resolves it to the method.
    fn ref_op(&mut self, id: &str, _receiver: Option<&mut Value>, _args: &mut [Value]) -> Result<Value, Signal> {
        Err(Signal::Unsupported(format!("builtin {id}")))
    }
    /// The declared values of enum object `id` as (ordinal, name), in declaration order with
    /// extension values after them. `None` when the context has no program.
    fn enum_members(&self, _id: u32) -> Option<Vec<(i32, String)>> {
        None
    }
    /// Builtins that read or change session state (number sequences, the transaction state): the
    /// VM implements them. `id` is the catalog overload id (`NumberSequence.Next#1`); `args` holds
    /// the call's arguments, with `var` parameters written back in place.
    fn session_op(&mut self, id: &str, _args: &mut [Value]) -> Result<Value, Signal> {
        Err(Signal::Unsupported(format!("builtin {id} needs a session")))
    }
    /// `Commit()`.
    fn commit(&mut self) -> Result<(), Signal>;
    /// A new, unique Guid (from the host or a deterministic sequence, ADR-006).
    fn new_guid(&mut self) -> [u8; 16];
    /// Whether the result of the builtin being called is used (`[Ok :=]` forms
    /// return false instead of raising when it is).
    fn ret_used(&self) -> bool {
        true
    }
    /// `Message(text)` after argument substitution. The runner decides what a
    /// message does (a test handler, an "Unhandled UI" refusal); a context
    /// with no UI refuses explicitly.
    fn ui_message(&mut self, _text: String) -> Result<(), Signal> {
        Err(Signal::Unsupported("Message: this context has no UI".into()))
    }
    /// `Confirm(question, default)`: the question is NOT substituted
    /// (corpus handlers/TestConfirmFormatArgs.al).
    fn ui_confirm(&mut self, _question: String, _default: bool) -> Result<bool, Signal> {
        Err(Signal::Unsupported("Confirm: this context has no UI".into()))
    }
    /// `StrMenu(options, default, instruction)`: the chosen option, 1-based, 0 for none.
    fn ui_str_menu(&mut self, _options: String, _default: i32, _instruction: String) -> Result<i32, Signal> {
        Err(Signal::Unsupported("StrMenu: this context has no UI".into()))
    }
}

pub type NativeFn = fn(&mut dyn Ctx, Option<&mut Value>, &mut [Value]) -> Result<Value, Signal>;

const REGISTRY: &[(&str, NativeFn)] = &[
    ("System.Format#1", format_1),
    ("System.Format#2", format_2),
    ("System.Evaluate#1", crate::evaluate::evaluate),
    ("System.Abs#1", abs),
    ("System.GetLastErrorText#1", get_last_error_text),
    ("System.GetLastErrorCode#1", get_last_error_code),
    ("System.GetLastErrorCallStack#1", get_last_error_call_stack),
    ("System.ClearLastError#1", clear_last_error),
    ("Dialog.Error#2", error_text),
    ("Dialog.Message#1", message),
    ("Dialog.Confirm#1", confirm),
    ("Dialog.StrMenu#1", str_menu),
    ("Text.StrLen#1", strlen),
    ("Text.StrPos#1", strpos),
    ("Text.CopyStr#1", copystr),
    ("Text.StrSubstNo#1", strsubstno),
    ("Text.UpperCase#1", uppercase),
    ("Text.LowerCase#1", lowercase),
    ("Database.Commit#1", |c, _, _| c.commit().map(|_| Value::Null)),
    ("System.Clear#1", clear),
    ("Enum.AsInteger#1", enum_as_integer),
    ("Enum.Names#1", enum_names),
    ("Enum.Ordinals#1", enum_ordinals),
    ("Enum.FromInteger#1", enum_from_integer),
    ("Record.Init#1", |c, r, a| c.record("Init", r, a)),
    ("Record.Insert#1", |c, r, a| c.record("Insert", r, a)),
    ("Record.Insert#2", |c, r, a| c.record("Insert", r, a)),
    ("Record.Modify#1", |c, r, a| c.record("Modify", r, a)),
    ("Record.Delete#1", |c, r, a| c.record("Delete", r, a)),
    ("Record.DeleteAll#1", |c, r, a| c.record("DeleteAll", r, a)),
    ("Record.Get#1", |c, r, a| c.record("Get", r, a)),
    ("Record.Find#1", |c, r, a| c.record("Find", r, a)),
    ("Record.FindFirst#1", |c, r, a| c.record("FindFirst", r, a)),
    ("Record.FindLast#1", |c, r, a| c.record("FindLast", r, a)),
    ("Record.FindSet#1", |c, r, a| c.record("FindSet", r, a)),
    ("Record.FindSet#2", |c, r, a| c.record("FindSet", r, a)),
    ("Record.Next#1", |c, r, a| c.record("Next", r, a)),
    ("Record.Count#1", |c, r, a| c.record("Count", r, a)),
    ("Record.IsEmpty#1", |c, r, a| c.record("IsEmpty", r, a)),
    ("Record.SetRange#1", |c, r, a| c.record("SetRange", r, a)),
    ("Record.SetFilter#1", |c, r, a| c.record("SetFilter", r, a)),
    // Event subscriptions are session state; the VM intercepts these ids
    // (al-vm dispatch.rs). Registered so lowering accepts them.
    ("Session.BindSubscription#1", vm_only),
    ("Session.UnbindSubscription#1", vm_only),
    ("System.CreateGuid#1", |c, _, _| Ok(Value::Guid(c.new_guid()))),
    ("System.IsNullGuid#1", |_, _, a| {
        Ok(Value::Boolean(matches!(a[0], Value::Guid(g) if g == [0; 16])))
    }),
    ("Record.Reset#1", |c, r, a| c.record("Reset", r, a)),
    ("Record.CalcFields#1", |c, r, a| c.record("CalcFields", r, a)),
    ("Record.SetCurrentKey#1", |c, r, a| c.record("SetCurrentKey", r, a)),
    ("Record.Validate#1", |c, r, a| c.record("Validate", r, a)),
    ("Record.Rename#1", |c, r, a| c.record("Rename", r, a)),
    ("Record.Truncate#1", |c, r, a| c.record("Truncate", r, a)),
    ("Record.CalcSums#1", |c, r, a| c.record("CalcSums", r, a)),
    ("Record.SetAutoCalcFields#1", |c, r, a| {
        c.record("SetAutoCalcFields", r, a)
    }),
    ("Variant.IsBoolean#1", |_, r, _| is(r, "Boolean")),
    ("Variant.IsInteger#1", |_, r, _| is(r, "Integer")),
    ("Variant.IsBigInteger#1", |_, r, _| is(r, "BigInteger")),
    ("Variant.IsDecimal#1", |_, r, _| is(r, "Decimal")),
    ("Variant.IsByte#1", |_, r, _| is(r, "Byte")),
    ("Variant.IsChar#1", |_, r, _| is(r, "Char")),
    ("Variant.IsText#1", |_, r, _| is(r, "Text")),
    ("Variant.IsCode#1", |_, r, _| is(r, "Code")),
    ("Variant.IsDate#1", |_, r, _| is(r, "Date")),
    ("Variant.IsTime#1", |_, r, _| is(r, "Time")),
    ("Variant.IsDateTime#1", |_, r, _| is(r, "DateTime")),
    ("Variant.IsDuration#1", |_, r, _| is(r, "Duration")),
    ("Variant.IsGuid#1", |_, r, _| is(r, "Guid")),
    ("Variant.IsDateFormula#1", |_, r, _| is(r, "DateFormula")),
    ("Variant.IsOption#1", |_, r, _| is(r, "Option")),
    ("Variant.IsList#1", |_, r, _| is(r, "List")),
    ("Variant.IsDictionary#1", |_, r, _| is(r, "Dictionary")),
    ("Variant.IsCodeunit#1", |_, r, _| is(r, "Codeunit")),
    // RecordId, RecordRef and FieldRef variables are library handles named after their type.
    ("Variant.IsNotification#1", |_, r, _| is(r, "Notification")),
    ("Variant.IsTextBuilder#1", |_, r, _| is(r, "TextBuilder")),
    ("Variant.IsRecordId#1", |_, r, _| is(r, "RecordId")),
    ("Variant.IsRecord#1", |_, r, _| is(r, "Record")),
    ("Variant.IsRecordRef#1", |_, r, _| is(r, "RecordRef")),
    ("Variant.IsFieldRef#1", |_, r, _| is(r, "FieldRef")),
    // Kinds the value model has no variant for yet are never true.
    ("Variant.IsTextConstant#1", |_, _, _| Ok(Value::Boolean(false))),
    ("Variant.IsDotNet#1", |_, _, _| Ok(Value::Boolean(false))),
    ("Variant.IsAutomation#1", |_, _, _| Ok(Value::Boolean(false))),
    ("Variant.IsFile#1", |_, _, _| Ok(Value::Boolean(false))),
];

fn all() -> impl Iterator<Item = &'static (&'static str, NativeFn)> {
    REGISTRY
        .iter()
        .chain(crate::collections::REGISTRY)
        .chain(crate::pagebuiltins::REGISTRY)
        .chain(crate::refs::REGISTRY)
        .chain(crate::refs::RECORD_REGISTRY)
        .chain(crate::datefns::REGISTRY)
        .chain(crate::mathfns::REGISTRY)
        .chain(crate::sessionfns::REGISTRY)
}

/// Implementations chosen per catalog signature instead of being listed by id
/// (families with many near-identical overloads: JSON, XML, streams). Built once
/// from the catalog, so a lookup is a hash probe.
fn derived() -> &'static std::collections::HashMap<String, NativeFn> {
    static TABLE: std::sync::OnceLock<std::collections::HashMap<String, NativeFn>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let cat = crate::catalog::Catalog::load();
        cat.signatures
            .iter()
            .filter_map(|sig| {
                crate::json::native_for(sig)
                    .or_else(|| crate::xml::native_for(sig))
                    .or_else(|| crate::stream::native_for(sig))
                    .or_else(|| crate::textfns::native_for(sig))
                    .or_else(|| crate::wsac::native_for(sig))
                    .or_else(|| crate::sessiontypes::native_for(sig))
                    .map(|f| (cat.id(sig), f))
            })
            .collect()
    })
}

/// The implementation registered for overload `id`, if any.
pub fn lookup(id: &str) -> Option<NativeFn> {
    all()
        .find(|(i, _)| *i == id)
        .map(|(_, f)| *f)
        .or_else(|| derived().get(id).copied())
}

/// Every registered id, for the conformance test.
pub fn registered_ids() -> Vec<&'static str> {
    all()
        .map(|(i, _)| *i)
        .chain(derived().keys().map(String::as_str))
        .collect()
}

fn vm_only(_: &mut dyn Ctx, _: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    unsupported("a builtin the VM runs itself")
}

fn unsupported<T>(what: &str) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.to_string()))
}

fn int_arg(args: &[Value], i: usize) -> Option<i64> {
    match args.get(i)? {
        Value::Integer(n) => Some(i64::from(*n)),
        Value::BigInteger(n) => Some(*n),
        Value::Byte(n) => Some(i64::from(*n)),
        Value::Char(n) => Some(i64::from(*n)),
        _ => None,
    }
}

/// Stands for a call stack that exists in BC but is not tracked here (a private-use character).
pub const UNTRACKED_CALL_STACK: &str = "\u{E000}";

/// Text of a Text/Code/Char argument (Text parameters accept all three). Text that holds the
/// untracked call stack marker is refused, so its content is never silently wrong.
pub fn text_of(v: &Value) -> Option<String> {
    match v {
        Value::Text(s) | Value::Code(s) if s.contains(UNTRACKED_CALL_STACK) => None,
        Value::Text(s) | Value::Code(s) => Some(s.clone()),
        Value::Char(c) => char::from_u32(u32::from(*c)).map(String::from),
        _ => None,
    }
}

fn text_arg(args: &[Value], i: usize) -> Result<String, Signal> {
    args.get(i).and_then(text_of).map_or_else(
        || unsupported(&format!("text argument {i} of type {:?}", args.get(i))),
        Ok,
    )
}

fn format_1(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let len = int_arg(args, 1).unwrap_or(0) as i32;
    let std = int_arg(args, 2).unwrap_or(0) as i32;
    try_format(&args[0], len, std, ctx)
        .map(Value::Text)
        .map_err(Signal::Unsupported)
}

/// `Format(Value, Length, FormatString)`: see [`crate::formatstr`].
fn format_2(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let len = int_arg(args, 1).unwrap_or(0) as i32;
    let template = text_arg(args, 2)?;
    crate::formatstr::format_with_string(&args[0], len, &template, ctx)
        .map(Value::Text)
        .map_err(Signal::Unsupported)
}

fn abs(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    Ok(match &args[0] {
        Value::Decimal(d) => Value::Decimal(d.abs()),
        Value::Integer(i) => Value::Decimal(i.unsigned_abs().into()),
        Value::BigInteger(i) => Value::Decimal(i.unsigned_abs().into()),
        other => return unsupported(&format!("Abs of {other:?}")),
    })
}

fn get_last_error_text(ctx: &mut dyn Ctx, _: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    Ok(Value::Text(ctx.last_error().message))
}

fn get_last_error_code(ctx: &mut dyn Ctx, _: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    Ok(Value::Text(ctx.last_error().code))
}

fn get_last_error_call_stack(ctx: &mut dyn Ctx, _: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    let e = ctx.last_error();
    if e.call_stack.is_empty() && !(e.message.is_empty() && e.code.is_empty()) {
        // BC returns the AL frames of the error (codeunit/TestCodeunitAlCallStack.al); they are not
        // tracked. The answer is non-empty, as in BC, but its content is a marker that every text
        // function refuses to read ([`text_of`]), so a test that inspects the frames is not run
        // while one that only calls the function (error-handling/TestGetLastError.al) still passes.
        return Ok(Value::Text(UNTRACKED_CALL_STACK.to_string()));
    }
    Ok(Value::Text(e.call_stack))
}

fn clear_last_error(ctx: &mut dyn Ctx, _: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    ctx.set_last_error(AlError::default());
    Ok(Value::Null)
}

/// `Error(Text, Args...)`. The code is `Dialog` (probes-needed: Microsoft's
/// test libraries and the corpus Assert's DialogTxt use it; no corpus test
/// pins it directly).
fn error_text(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let template = text_arg(args, 0)?;
    let message = substitute(&template, &args[1..], ctx)?;
    Err(Signal::Error(AlError {
        message,
        code: "Dialog".into(),
        call_stack: String::new(),
    }))
}

/// `Message(String, Values...)`: the text is substituted like `StrSubstNo`
/// before it reaches the UI (corpus handlers/TestMessageHandler.al
/// Message_FormattedString_HandlerGetsFormatted).
fn message(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let text = substitute(&text_arg(args, 0)?, &args[1..], ctx)?;
    ctx.ui_message(text)?;
    Ok(Value::Null)
}

/// `Confirm(String, Default, Values...)`. Values are not substituted into the
/// question a handler sees (corpus handlers/TestConfirmFormatArgs.al). An
/// omitted Default is false (docs/probes-needed.md).
fn confirm(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let question = text_arg(args, 0)?;
    let default = match args.get(1) {
        None => false,
        Some(Value::Boolean(b)) => *b,
        Some(other) => return unsupported(&format!("Confirm default {other:?}")),
    };
    Ok(Value::Boolean(ctx.ui_confirm(question, default)?))
}

/// `StrMenu(Options, Default, Instruction)`. An omitted Default is 1 and an
/// omitted Instruction is empty (docs/probes-needed.md).
fn str_menu(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let options = text_arg(args, 0)?;
    let default = match args.get(1) {
        None => 1,
        Some(_) => int_arg(args, 1).map_or_else(|| unsupported("StrMenu default"), |n| Ok(n as i32))?,
    };
    let instruction = match args.get(2) {
        None => String::new(),
        Some(_) => text_arg(args, 2)?,
    };
    Ok(Value::Integer(ctx.ui_str_menu(options, default, instruction)?))
}

/// `%1`..`%9` replaced by `Format(arg)`. A placeholder without an argument
/// and `#n` fields are not pinned by the corpus yet: unsupported.
pub fn substitute(template: &str, args: &[Value], ctx: &dyn FormatCtx) -> Result<String, Signal> {
    let mut out = String::new();
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '%' {
            if let Some(d) = chars.peek().and_then(|d| d.to_digit(10)).filter(|d| *d >= 1) {
                chars.next();
                match args.get(d as usize - 1) {
                    Some(v) => out.push_str(&try_format(v, 0, 0, ctx).map_err(Signal::Unsupported)?),
                    None => return unsupported("StrSubstNo placeholder without an argument"),
                }
                continue;
            }
        }
        out.push(c);
    }
    Ok(out)
}

fn strlen(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    Ok(Value::Integer(Value::text_len(&text_arg(args, 0)?) as i32))
}

fn strpos(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let (s, sub) = (text_arg(args, 0)?, text_arg(args, 1)?);
    // text/TestTextProcessingContracts.al StrPos_EmptySubstring_ReturnsZero.
    if sub.is_empty() {
        return Ok(Value::Integer(0));
    }
    let hay: Vec<u16> = s.encode_utf16().collect();
    let needle: Vec<u16> = sub.encode_utf16().collect();
    let pos = hay
        .windows(needle.len())
        .position(|w| w == needle.as_slice())
        .map_or(0, |p| p + 1);
    Ok(Value::Integer(pos as i32))
}

fn copystr(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let s: Vec<u16> = text_arg(args, 0)?.encode_utf16().collect();
    let pos = int_arg(args, 1).unwrap_or(1);
    if pos < 1 {
        return unsupported("CopyStr with Position < 1");
    }
    let start = (pos as usize - 1).min(s.len());
    let end = match int_arg(args, 2) {
        Some(len) if len < 0 => return unsupported("CopyStr with a negative Length"),
        Some(len) => (start + len as usize).min(s.len()),
        None => s.len(),
    };
    Ok(Value::Text(String::from_utf16_lossy(&s[start..end])))
}

fn strsubstno(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let template = text_arg(args, 0)?;
    Ok(Value::Text(substitute(&template, &args[1..], ctx)?))
}

fn uppercase(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    Ok(Value::Text(text_arg(args, 0)?.to_uppercase()))
}

fn lowercase(_: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    Ok(Value::Text(text_arg(args, 0)?.to_lowercase()))
}

/// `Clear(var X)`: the default of the value's own kind. Records go to the VM.
fn clear(ctx: &mut dyn Ctx, _: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let v = &mut args[0];
    *v = match v {
        Value::Record(_) => return ctx.record("Clear", Some(v), &mut []),
        // Clearing a RecordRef/FieldRef/KeyRef closes it: a fresh, unopened handle.
        Value::Library(name, _)
            if matches!(
                name.as_str(),
                "RecordRef"
                    | "FieldRef"
                    | "KeyRef"
                    | "RecordId"
                    | "WebServiceActionContext"
                    | "SessionSettings"
                    | "Notification"
            ) =>
        {
            Value::Library(name.clone(), Default::default())
        }
        Value::Boolean(_) => Value::Boolean(false),
        Value::Integer(_) => Value::Integer(0),
        Value::BigInteger(_) => Value::BigInteger(0),
        Value::Decimal(_) => Value::Decimal(Default::default()),
        Value::Byte(_) => Value::Byte(0),
        Value::Char(_) => Value::Char(0),
        Value::Text(_) => Value::Text(String::new()),
        Value::Code(_) => Value::Code(String::new()),
        Value::Date(..) => Value::Date(0, false),
        Value::Time(_) => Value::Time(0),
        Value::DateTime(_) => Value::DateTime(0),
        Value::Duration(_) => Value::Duration(0),
        Value::Guid(_) => Value::Guid([0; 16]),
        Value::Option(_, m) => Value::Option(0, m.clone()),
        // A cleared Blob is a new, empty buffer (streams/TestBlobCalcFieldsUncommittedWrite.al).
        Value::Blob(_) => Value::Blob(crate::stream::new_blob()),
        other => return unsupported(&format!("Clear of {other:?}")),
    };
    Ok(Value::Null)
}

/// `Enum.AsInteger()`: the ordinal (types/TestEnum.al:27-40).
fn enum_as_integer(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    match r {
        Some(Value::Enum(_, o)) => Ok(Value::Integer(*o)),
        other => unsupported(&format!("AsInteger of {other:?}")),
    }
}

/// The enum a type-level call (`"Name".Names()`) or a call on a variable names: lowering passes
/// the enum object as the receiver value.
fn enum_receiver(ctx: &dyn Ctx, r: Option<&mut Value>, what: &str) -> Result<(u32, Vec<(i32, String)>), Signal> {
    let Some(Value::Enum(id, _)) = r else {
        return unsupported(&format!("{what} without an enum receiver"));
    };
    let id = *id;
    ctx.enum_members(id).map(|m| (id, m)).map_or_else(
        || unsupported(&format!("{what}: this context has no enum metadata")),
        Ok,
    )
}

fn list_of(items: Vec<Value>) -> Value {
    Value::List(Rc::new(std::cell::RefCell::new(items)))
}

/// `Enum.Names()`: every declared name, the blank ordinal included
/// (types/TestEnumNamesContracts.al:19-60). The order is the declaration order, which matches
/// `Ordinals()` position by position; BC's order is not pinned (docs/probes-needed.md).
fn enum_names(ctx: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    let (_, members) = enum_receiver(ctx, r, "Enum.Names")?;
    Ok(list_of(members.into_iter().map(|(_, n)| Value::Text(n)).collect()))
}

/// `Enum.Ordinals()` (session/TestSystemExtended.al:381-390, types/TestLanguageEdgeCases.al:91-105).
fn enum_ordinals(ctx: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
    let (_, members) = enum_receiver(ctx, r, "Enum.Ordinals")?;
    Ok(list_of(members.into_iter().map(|(o, _)| Value::Integer(o)).collect()))
}

/// `Enum.FromInteger(n)`: any number is carried, declared or not (types/TestEnum.al:83-91,
/// types/TestEnumDefaultImplTests.Codeunit.al:70).
fn enum_from_integer(ctx: &mut dyn Ctx, r: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
    let (id, _) = enum_receiver(ctx, r, "Enum.FromInteger")?;
    match int_arg(args, 0).and_then(|n| i32::try_from(n).ok()) {
        Some(n) => Ok(Value::Enum(id, n)),
        None => unsupported("Enum.FromInteger with a number outside the Integer range"),
    }
}

fn is(receiver: Option<&mut Value>, kind: &str) -> Result<Value, Signal> {
    Ok(Value::Boolean(receiver.is_some_and(|v| v.is(kind))))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::rc::Rc;

    #[derive(Default)]
    pub(crate) struct TestCtx {
        err: AlError,
    }
    impl FormatCtx for TestCtx {
        fn enum_name(&self, _id: u32, o: i32) -> String {
            format!("E{o}")
        }
    }
    impl Ctx for TestCtx {
        fn last_error(&self) -> AlError {
            self.err.clone()
        }
        fn set_last_error(&mut self, e: AlError) {
            self.err = e;
        }
        fn record(&mut self, m: &str, _: Option<&mut Value>, _: &mut [Value]) -> Result<Value, Signal> {
            Err(Signal::Unsupported(m.into()))
        }
        fn commit(&mut self) -> Result<(), Signal> {
            Ok(())
        }
        fn new_guid(&mut self) -> [u8; 16] {
            [1; 16]
        }
    }

    pub(crate) fn call(id: &str, args: Vec<Value>) -> Result<Value, Signal> {
        let f = lookup(id).unwrap_or_else(|| panic!("{id} not registered"));
        let mut args = args;
        f(&mut TestCtx::default(), None, &mut args)
    }
    pub(crate) fn t(s: &str) -> Value {
        Value::Text(s.into())
    }
    fn text(r: Result<Value, Signal>) -> String {
        match r {
            Ok(Value::Text(s)) => s,
            other => panic!("{other:?}"),
        }
    }
    fn int(r: Result<Value, Signal>) -> i32 {
        match r {
            Ok(Value::Integer(i)) => i,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn every_registered_id_is_in_the_catalog() {
        let cat = crate::catalog::Catalog::load();
        let ids: Vec<String> = cat.signatures.iter().map(|s| cat.id(s)).collect();
        for id in registered_ids() {
            assert!(ids.iter().any(|i| i == id), "{id} is not a catalog overload id");
        }
    }

    #[test]
    fn format_with_length_and_standard_format() {
        assert_eq!(text(call("System.Format#1", vec![Value::Integer(42)])), "42");
        assert_eq!(
            text(call("System.Format#1", vec![Value::Integer(42), Value::Integer(5)])),
            "   42"
        );
        assert_eq!(
            text(call(
                "System.Format#1",
                vec![Value::Boolean(true), Value::Integer(0), Value::Integer(2)]
            )),
            "1"
        );
    }

    // The formats of Time, DateTime and Duration are not pinned by the corpus yet: Format of
    // them is a refusal the test can report, not a panic that takes the whole codeunit down.
    #[test]
    fn format_of_a_value_without_a_pinned_format_is_a_refusal() {
        for v in [Value::Duration(3_600_000), Value::Time(43_200_001), Value::DateTime(5)] {
            assert!(
                matches!(call("System.Format#1", vec![v]), Err(Signal::Unsupported(_))),
                "Format must refuse"
            );
        }
    }

    #[test]
    fn formats_that_are_not_pinned_are_refused_not_panics() {
        // Format of a Duration is not pinned by the corpus yet: Unsupported, so the test is
        // reported as not run instead of a panic taking the whole codeunit down.
        assert!(matches!(
            call("System.Format#1", vec![Value::Duration(86_400_000)]),
            Err(Signal::Unsupported(_))
        ));
        assert!(matches!(
            call("Text.StrSubstNo#1", vec![t("%1"), Value::Duration(5)]),
            Err(Signal::Unsupported(_))
        ));
    }

    #[test]
    fn the_call_stack_of_a_recorded_error_is_refused_not_invented() {
        // codeunit/TestCodeunitAlCallStack.al expects AL frames such as `(CodeUnit 60211)`; call
        // stacks are not tracked: after an error the answer is a marker no text function reads.
        let mut ctx = TestCtx::default();
        let f = lookup("System.GetLastErrorCallStack#1").unwrap();
        assert!(matches!(f(&mut ctx, None, &mut []), Ok(Value::Text(ref s)) if s.is_empty()));
        ctx.set_last_error(AlError {
            message: "boom".into(),
            code: "Dialog".into(),
            call_stack: String::new(),
        });
        let Ok(stack @ Value::Text(_)) = f(&mut ctx, None, &mut []) else {
            panic!("a call stack is returned after an error")
        };
        assert!(matches!(&stack, Value::Text(s) if !s.is_empty()));
        assert!(text_of(&stack).is_none(), "its content cannot be inspected");
    }

    #[test]
    fn strlen_counts_utf16() {
        assert_eq!(int(call("Text.StrLen#1", vec![t("abc")])), 3);
        assert_eq!(int(call("Text.StrLen#1", vec![t("😀")])), 2);
    }

    #[test]
    fn strpos_is_one_based_and_zero_when_absent() {
        assert_eq!(int(call("Text.StrPos#1", vec![t("hello"), t("ll")])), 3);
        assert_eq!(int(call("Text.StrPos#1", vec![t("hello"), t("x")])), 0);
    }

    #[test]
    fn copystr() {
        assert_eq!(
            text(call(
                "Text.CopyStr#1",
                vec![t("Hello World"), Value::Integer(1), Value::Integer(5)]
            )),
            "Hello"
        );
        assert_eq!(text(call("Text.CopyStr#1", vec![t("Hello"), Value::Integer(3)])), "llo");
        assert_eq!(
            text(call(
                "Text.CopyStr#1",
                vec![t("Hello"), Value::Integer(4), Value::Integer(10)]
            )),
            "lo"
        );
        assert!(matches!(
            call("Text.CopyStr#1", vec![t("Hello"), Value::Integer(0)]),
            Err(Signal::Unsupported(_))
        ));
    }

    #[test]
    fn strsubstno_formats_its_arguments() {
        assert_eq!(
            text(call(
                "Text.StrSubstNo#1",
                vec![t("%1 and %2"), Value::Integer(7), Value::Boolean(true)]
            )),
            "7 and Yes"
        );
        assert_eq!(text(call("Text.StrSubstNo#1", vec![t("no args")])), "no args");
    }

    #[test]
    fn case_conversion() {
        assert_eq!(text(call("Text.UpperCase#1", vec![t("abc")])), "ABC");
        assert_eq!(text(call("Text.LowerCase#1", vec![t("ABC")])), "abc");
    }

    #[test]
    fn error_substitutes_and_raises() {
        let Err(Signal::Error(e)) = call("Dialog.Error#2", vec![t("bad %1"), Value::Integer(3)]) else {
            panic!()
        };
        assert_eq!(e.message, "bad 3");
    }

    #[test]
    fn last_error_round_trip() {
        let mut ctx = TestCtx::default();
        ctx.set_last_error(AlError {
            message: "boom".into(),
            code: "Dialog".into(),
            call_stack: String::new(),
        });
        let get = lookup("System.GetLastErrorText#1").unwrap();
        assert!(matches!(get(&mut ctx, None, &mut []), Ok(Value::Text(ref s)) if s == "boom"));
        let code = lookup("System.GetLastErrorCode#1").unwrap();
        assert!(matches!(code(&mut ctx, None, &mut []), Ok(Value::Text(ref s)) if s == "Dialog"));
        lookup("System.ClearLastError#1").unwrap()(&mut ctx, None, &mut []).unwrap();
        assert_eq!(ctx.last_error(), AlError::default());
    }

    #[test]
    fn abs() {
        let r = call("System.Abs#1", vec![Value::Decimal((-2).into())]);
        assert!(matches!(r, Ok(Value::Decimal(d)) if d == 2.into()));
    }

    #[test]
    fn variant_type_tests_use_the_receiver() {
        let f = lookup("Variant.IsInteger#1").unwrap();
        let mut v = Value::Integer(1);
        assert!(matches!(
            f(&mut TestCtx::default(), Some(&mut v), &mut []),
            Ok(Value::Boolean(true))
        ));
        let mut v = Value::Option(0, Rc::from(vec![]));
        let f = lookup("Variant.IsOption#1").unwrap();
        assert!(matches!(
            f(&mut TestCtx::default(), Some(&mut v), &mut []),
            Ok(Value::Boolean(true))
        ));
    }
}
