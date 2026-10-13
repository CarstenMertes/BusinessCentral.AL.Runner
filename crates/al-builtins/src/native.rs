//! Native builtin implementations, registered by catalog overload id (ADR-008).

use crate::format::{FormatCtx, format};
use crate::value::Value;

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
    /// `Commit()`.
    fn commit(&mut self) -> Result<(), Signal>;
    /// A new, unique Guid (from the host or a deterministic sequence, ADR-006).
    fn new_guid(&mut self) -> [u8; 16];
    /// Whether the result of the builtin being called is used (`[Ok :=]` forms
    /// return false instead of raising when it is).
    fn ret_used(&self) -> bool {
        true
    }
}

pub type NativeFn = fn(&mut dyn Ctx, Option<&mut Value>, &mut [Value]) -> Result<Value, Signal>;

const REGISTRY: &[(&str, NativeFn)] = &[
    ("System.Format#1", format_1),
    ("System.Abs#1", abs),
    ("System.GetLastErrorText#1", get_last_error_text),
    ("System.GetLastErrorCode#1", get_last_error_code),
    ("System.GetLastErrorCallStack#1", get_last_error_call_stack),
    ("System.ClearLastError#1", clear_last_error),
    ("Dialog.Error#2", error_text),
    ("Text.StrLen#1", strlen),
    ("Text.StrPos#1", strpos),
    ("Text.CopyStr#1", copystr),
    ("Text.StrSubstNo#1", strsubstno),
    ("Text.UpperCase#1", uppercase),
    ("Text.LowerCase#1", lowercase),
    ("Database.Commit#1", |c, _, _| c.commit().map(|_| Value::Null)),
    ("System.Clear#1", clear),
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
    ("System.CreateGuid#1", |c, _, _| Ok(Value::Guid(c.new_guid()))),
    ("System.IsNullGuid#1", |_, _, a| {
        Ok(Value::Boolean(matches!(a[0], Value::Guid(g) if g == [0; 16])))
    }),
    ("Record.Reset#1", |c, r, a| c.record("Reset", r, a)),
    ("Record.SetCurrentKey#1", |c, r, a| c.record("SetCurrentKey", r, a)),
    ("Record.Validate#1", |c, r, a| c.record("Validate", r, a)),
    ("Record.Rename#1", |c, r, a| c.record("Rename", r, a)),
    ("Record.CalcFields#1", |c, r, a| c.record("CalcFields", r, a)),
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
    // Kinds the value model has no variant for yet are never true.
    ("Variant.IsRecordId#1", |_, _, _| Ok(Value::Boolean(false))),
    ("Variant.IsTextConstant#1", |_, _, _| Ok(Value::Boolean(false))),
    ("Variant.IsRecord#1", |_, r, _| is(r, "Record")),
    ("Variant.IsRecordRef#1", |_, _, _| Ok(Value::Boolean(false))),
    ("Variant.IsFieldRef#1", |_, _, _| Ok(Value::Boolean(false))),
    ("Variant.IsDotNet#1", |_, _, _| Ok(Value::Boolean(false))),
    ("Variant.IsAutomation#1", |_, _, _| Ok(Value::Boolean(false))),
    ("Variant.IsFile#1", |_, _, _| Ok(Value::Boolean(false))),
];

fn all() -> impl Iterator<Item = &'static (&'static str, NativeFn)> {
    REGISTRY.iter().chain(crate::collections::REGISTRY)
}

/// The implementation registered for overload `id`, if any.
pub fn lookup(id: &str) -> Option<NativeFn> {
    all().find(|(i, _)| *i == id).map(|(_, f)| *f)
}

/// Every registered id, for the conformance test.
pub fn registered_ids() -> Vec<&'static str> {
    all().map(|(i, _)| *i).collect()
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

/// Text of a Text/Code/Char argument (Text parameters accept all three).
pub fn text_of(v: &Value) -> Option<String> {
    match v {
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
    Ok(Value::Text(format(&args[0], len, std, ctx)))
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
    Ok(Value::Text(ctx.last_error().call_stack))
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
                    Some(v) => out.push_str(&format(v, 0, 0, ctx)),
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
    if sub.is_empty() {
        return unsupported("StrPos with an empty substring");
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
        other => return unsupported(&format!("Clear of {other:?}")),
    };
    Ok(Value::Null)
}

fn is(receiver: Option<&mut Value>, kind: &str) -> Result<Value, Signal> {
    Ok(Value::Boolean(receiver.is_some_and(|v| v.is(kind))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;

    #[derive(Default)]
    struct TestCtx {
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

    fn call(id: &str, args: Vec<Value>) -> Result<Value, Signal> {
        let f = lookup(id).unwrap_or_else(|| panic!("{id} not registered"));
        let mut args = args;
        f(&mut TestCtx::default(), None, &mut args)
    }
    fn t(s: &str) -> Value {
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
