//! Value operations: conversion to a slot type, arithmetic, comparison
//! (ADR-010: conversions live next to the value model).

use crate::describe::Describe;
use crate::native::Signal;
use crate::native::raise;
use crate::value::{LibraryValue, RtType, Value};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

fn unsupported<T>(what: String) -> Result<T, Signal> {
    Err(Signal::Unsupported(what))
}

/// Converts `v` for storing in a slot of type `t`.
pub fn convert(v: Value, t: &RtType) -> Result<Value, Signal> {
    use RtType as T;
    // The conversions between the plain numbers that cannot fail, without the Decimal detour.
    match (&v, t) {
        (Value::Integer(_), T::Integer) | (Value::BigInteger(_), T::BigInteger) | (Value::Decimal(_), T::Decimal) => {
            return Ok(v);
        }
        (Value::Integer(n), T::BigInteger) => return Ok(Value::BigInteger(i64::from(*n))),
        (Value::Integer(n), T::Decimal) => return Ok(Value::Decimal(Decimal::from(*n))),
        (Value::BigInteger(n), T::Decimal) => return Ok(Value::Decimal(Decimal::from(*n))),
        _ => {}
    }
    Ok(match (v, t) {
        (v, T::Variant) => v,
        // A number into its own type is the number (what the general arm below gives, without the round trip through a
        // Decimal; the scale of a Decimal is kept).
        (v @ Value::Integer(_), T::Integer)
        | (v @ Value::BigInteger(_), T::BigInteger)
        | (v @ Value::Decimal(_), T::Decimal) => v,
        (Value::Null, t) => Value::default_for(t),
        (v @ Value::Boolean(_), T::Boolean) => v,
        (v @ Value::Date(..), T::Date) | (v @ Value::Time(_), T::Time) | (v @ Value::DateTime(_), T::DateTime) => v,
        (v @ Value::Duration(_), T::Duration)
        | (v @ Value::Guid(_), T::Guid)
        | (v @ Value::DateFormula(_), T::DateFormula) => v,
        (v @ Value::List(_), T::List(_))
        | (v @ Value::Dictionary(_), T::Dictionary(..))
        | (v @ Value::Array(_), T::Array(..)) => v,
        // A SessionSettings is a value: assignment copies it (session/TestSessionSettings.al:222-262).
        (Value::Library(crate::library_kind::LibraryKind::SessionSettings, cell), T::Library(_)) => {
            let copy = match &*cell.borrow() {
                LibraryValue::SessionSettings(s) => LibraryValue::SessionSettings(s.clone()),
                _ => LibraryValue::Empty,
            };
            Value::Library(
                crate::library_kind::LibraryKind::SessionSettings,
                Rc::new(RefCell::new(copy)),
            )
        }
        // An HttpContent is a value: assignment and a by-value parameter copy its body and headers
        // (misc/W4XmlJsonHttp.al:RequestContent_IsCopyOrReference).
        (Value::Library(n, cell), T::Library(_))
            if n == "HttpContent" && matches!(&*cell.borrow(), LibraryValue::HttpContent(_)) =>
        {
            let copy = match &*cell.borrow() {
                LibraryValue::HttpContent(c) => LibraryValue::HttpContent(c.deep_copy()),
                _ => LibraryValue::Empty,
            };
            Value::Library(n, Rc::new(RefCell::new(copy)))
        }
        (v @ Value::Codeunit(_), T::Codeunit(_) | T::Interface(_)) | (v @ Value::Library(..), T::Library(_)) => v,
        (v @ Value::Record(_), T::Record(..)) => v,
        // A query handed on keeps its state: the callee works on the caller's query.
        (v @ Value::Query(_), T::Query(_)) => v,
        // A DotNet variable shares the object it is assigned (ADR-014; probe pending).
        (v @ Value::DotNet(_), T::DotNet(_)) => v,
        // A DotNet variable holds the AL primitive stored into it, as the .NET value it marshals to
        // (a `DotNet Byte` takes a Byte; ADR-014 section 4, ADR-024). Date and Time have no pinned mapping.
        (
            v @ (Value::Boolean(_)
            | Value::Integer(_)
            | Value::BigInteger(_)
            | Value::Decimal(_)
            | Value::Byte(_)
            | Value::Char(_)
            | Value::Text(_)
            | Value::Code(_)
            | Value::DateTime(_)
            | Value::Duration(_)
            | Value::Guid(_)),
            T::DotNet(_),
        ) => v,
        (v @ Value::TestPage(_), T::TestPage(_) | T::Page(_)) => v,
        (v @ Value::Blob(_), T::Library(n)) if n.eq_ignore_ascii_case("Blob") => v,
        (v @ Value::Guid(_), T::Library(n))
            if n.eq_ignore_ascii_case("Media") || n.eq_ignore_ascii_case("MediaSet") =>
        {
            v
        }
        // A Text (not a literal) assigns into a SecretText (types/TestSecretText.al).
        (Value::Text(s) | Value::Code(s), T::Library(n)) if n.eq_ignore_ascii_case("SecretText") => Value::Library(
            "SecretText".into(),
            Rc::new(RefCell::new(LibraryValue::Text(s.to_string()))),
        ),
        (v @ Value::Enum(from, _), T::Enum(to)) if from == *to => v,
        // An enum of another type converts by its ordinal; an ordinal the target lacks is kept (Format shows
        // the number, AsInteger gives it). The same in a field, a Get key and a SetRange value (corpus
        // types/PXEnumRecordRef.al:7-10, :74-135, probed on BC 27.5). Whether
        // the assignment compiles (the target must state AssignmentCompatibility, AL0122) is the checker's.
        (Value::Enum(_, o), T::Enum(to)) => Value::Enum(*to, o),
        (Value::Option(o, _), T::Option(m)) => Value::Option(o, m.clone().into()),
        // An Enum value into an Option or an Integer is its ordinal, a BigInteger into an Option likewise (an ordinal
        // beyond the members is kept, as for an Integer). Not pinned by the corpus (docs/probes-needed.md).
        (Value::Enum(_, o), T::Option(m))
            if crate::assumptions::hit(crate::assumptions::Assumption::EnumOptionIntegerOrdinal) =>
        {
            Value::Option(o, m.clone().into())
        }
        (Value::Enum(_, o), T::Integer)
            if crate::assumptions::hit(crate::assumptions::Assumption::EnumOptionIntegerOrdinal) =>
        {
            Value::Integer(o)
        }
        (Value::BigInteger(n), T::Option(m)) => match i32::try_from(n) {
            Ok(o) => {
                crate::assumptions::assume(crate::assumptions::Assumption::EnumOptionIntegerOrdinal);
                Value::Option(o, m.clone().into())
            }
            Err(_) => return unsupported(format!("{n} as an Option ordinal (beyond Integer)")),
        },
        (Value::Option(o, _), T::Integer) => Value::Integer(o),
        (Value::Integer(n), T::Duration) => Value::Duration(i64::from(n)),
        (Value::BigInteger(n), T::Duration) => Value::Duration(n),
        // A Duration into an Integer, BigInteger or Decimal is its milliseconds (the reverse of a whole number into a
        // Duration, pinned by record/TestCalcFormulaValidationTests.Codeunit.al:42). Calendar Management CalcTimeDelta
        // does `Result := EndingTime - StartingTime` (Time - Time is a Duration) into an Integer. Not pinned.
        (Value::Duration(ms), T::Integer) if i32::try_from(ms).is_ok() => {
            crate::assumptions::assume(crate::assumptions::Assumption::DurationToNumber);
            Value::Integer(i32::try_from(ms).expect("checked"))
        }
        (Value::Duration(ms), T::BigInteger) => {
            crate::assumptions::assume(crate::assumptions::Assumption::DurationToNumber);
            Value::BigInteger(ms)
        }
        (Value::Duration(ms), T::Decimal) => {
            crate::assumptions::assume(crate::assumptions::Assumption::DurationToNumber);
            Value::Decimal(Decimal::from(ms))
        }
        (Value::Integer(i), T::Option(m)) => Value::Option(i, m.clone().into()),
        // A Duration is a count of milliseconds.
        (v, T::Duration) if as_decimal(&v).is_some() && !matches!(v, Value::Text(_) | Value::Code(_)) => {
            let d = as_decimal(&v).expect("numeric");
            match i64::try_from(d.mantissa() / 10i128.pow(d.scale())) {
                Ok(ms) if d.fract() == Decimal::ZERO => Value::Duration(ms),
                _ => return unsupported(format!("{d} as a Duration (rounding not pinned by the corpus yet)")),
            }
        }
        // Numbers.
        (v, T::Integer | T::BigInteger | T::Decimal | T::Byte | T::Char)
            if as_decimal(&v).is_some() && !matches!(v, Value::Text(_) | Value::Code(_)) =>
        {
            let d = as_decimal(&v).expect("numeric");
            // A Decimal with a fraction, or beyond the range, does not convert into an Integer or a BigInteger: no
            // rounding (types/PRDecimalToInteger.al: ConversionCast, `value 1.5`, `value 3000000000.0`).
            let cast_error = |to: &str| {
                raise::<Value>(
                    format!(
                        "Overflow under conversion of Microsoft.Dynamics.Nav.Runtime.Decimal18 value {d} to System.{to}."
                    ),
                    "ConversionCast",
                )
            };
            match t {
                T::Decimal => Value::Decimal(d),
                T::Integer | T::BigInteger if matches!(v, Value::Decimal(_)) && d.fract() != Decimal::ZERO => {
                    return cast_error(if matches!(t, T::Integer) { "Int32" } else { "Int64" });
                }
                _ if d.fract() != Decimal::ZERO => {
                    return unsupported(format!(
                        "rounding {d} into {} (not pinned by the corpus yet)",
                        t.describe()
                    ));
                }
                T::Integer => match i32::try_from(d.mantissa() / 10i128.pow(d.scale())) {
                    Ok(n) => Value::Integer(n),
                    Err(_) if matches!(v, Value::Decimal(_)) => return cast_error("Int32"),
                    Err(_) => return unsupported(format!("{d} overflows Integer")),
                },
                T::BigInteger => match i64::try_from(d.mantissa() / 10i128.pow(d.scale())) {
                    Ok(n) => Value::BigInteger(n),
                    Err(_) if matches!(v, Value::Decimal(_)) => return cast_error("Int64"),
                    Err(_) => return unsupported(format!("{d} overflows BigInteger")),
                },
                T::Byte => Value::Byte(
                    u8::try_from(d.mantissa() / 10i128.pow(d.scale()))
                        .or_else(|_| unsupported(format!("{d} overflows Byte")))?,
                ),
                T::Char => Value::Char(
                    u16::try_from(d.mantissa() / 10i128.pow(d.scale()))
                        .or_else(|_| unsupported(format!("{d} overflows Char")))?,
                ),
                _ => unreachable!(),
            }
        }
        // The untracked call stack marker moves between Text slots unread (see `native::text_of`).
        (Value::Text(s), T::Text(_)) if s.has_marker() => Value::Text(s),
        // A Date handed to a Text (a Variant from `FieldRef.Value` into a Text parameter: Incoming Document
        // `FindByDocumentNoAndPostingDate(.., PostingDateFieldRef.Value)` and `Evaluate` of it, 474 Tests-ERM tests) is its
        // default `Format` text. Not pinned: assumption date-to-text-is-format.
        (v @ Value::Date(..), T::Text(_) | T::Code(_)) => {
            struct NoEnums;
            impl crate::format::FormatCtx for NoEnums {
                fn enum_name(&self, _: u32, _: i32) -> String {
                    String::new()
                }
            }
            crate::assumptions::assume(crate::assumptions::Assumption::DateToTextIsFormat);
            let text = crate::format::try_format(&v, 0, 0, &NoEnums).map_err(Signal::Unsupported)?;
            match t {
                T::Text(max) => Value::Text(fit_txt(text.into(), *max)?),
                _ => {
                    let T::Code(max) = t else { unreachable!() };
                    Value::Code(fit(upper_code(text.trim()), *max)?.into())
                }
            }
        }
        // Text-like.
        (v, T::Text(max)) if crate::native::text_of(&v).is_some() => {
            Value::Text(fit_txt(crate::native::text_of(&v).expect("text"), *max)?)
        }
        (v, T::Code(max)) if crate::native::text_of(&v).is_some() => {
            // Doc (Code data type): upper case, leading and trailing spaces removed.
            let text = crate::native::text_of(&v).expect("text");
            let s = upper_code(text.trim());
            // The error shows the text as it was given (text/lt-label-textbuilder/TxCLabel.al:Label_MaxLength `toCode2`).
            if max.is_some_and(|m| Value::text_len(&s) > m as usize) {
                fit(text.trim().to_string(), *max)?;
            }
            Value::Code(fit(s, *max)?.into())
        }
        (Value::Text(s) | Value::Code(s), T::Char) => {
            let u: Vec<u16> = s.encode_utf16().collect();
            match u.as_slice() {
                [c] => Value::Char(*c),
                _ => return unsupported(format!("text of length {} into Char", u.len())),
            }
        }
        // A Text assigns into a Guid, with or without braces (record/TestRecordGetBySystemId*.al uses
        // `'00000000-0000-0000-0000-000000000000'`; the system-id tests use the braced form).
        (Value::Text(s) | Value::Code(s), T::Guid) => match parse_guid(&s) {
            Some(g) => Value::Guid(g),
            None => return unsupported(format!("text `{s}` into a Guid (error text not pinned yet)")),
        },
        // An Option converts to an Enum by its ordinal, with or without `AssignmentCompatibility`, blank member
        // ' ' giving 0; an ordinal the enum lacks raises no error, the enum holds that number
        // (corpus types/PEOptionEnumBigText.al:68-141, :171-184, probed on BC 27.5).
        (Value::Option(o, _), T::Enum(id)) => Value::Enum(*id, o),
        // An Integer converts to the enum value of that ordinal, with or without AssignmentCompatibility; an
        // ordinal the enum lacks is kept. The same in a field, a Get key and a SetRange value (corpus
        // types/PXEnumRecordRef.al:11-12, :137-196, probed on BC 27.5).
        (Value::Integer(n), T::Enum(id)) => Value::Enum(*id, n),
        // A BigInteger goes by ordinal as an Integer does (not probed; registry entry enum-option-integer-ordinal).
        (Value::BigInteger(n), T::Enum(id)) => match i32::try_from(n) {
            Ok(o) => {
                crate::assumptions::assume(crate::assumptions::Assumption::EnumOptionIntegerOrdinal);
                Value::Enum(*id, o)
            }
            Err(_) => return unsupported(format!("{n} as an Enum ordinal (beyond Integer)")),
        },
        // A Guid assigns into a Text or Code as its text form `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}`, as
        // Format gives it (`Dimensions.Add('PackageCode', Rec.SystemId)` into a Dictionary of [Text, Text] in Base
        // Application's Config. Package Management). No corpus test assigns a Guid to a Text
        // (docs/probes-needed.md).
        (Value::Guid(g), T::Text(max)) if crate::assumptions::hit(crate::assumptions::Assumption::GuidToText) => {
            Value::Text(fit(crate::format::format_guid(&g), *max)?.into())
        }
        (Value::Guid(g), T::Code(max)) if crate::assumptions::hit(crate::assumptions::Assumption::GuidToText) => {
            Value::Code(fit(crate::format::format_guid(&g), *max)?.into())
        }
        // A Text of digits converts into an Integer or BigInteger where a Variant meets a number (a Get key).
        // Not pinned by the corpus: Microsoft's Incoming Document `Get(Format(Value))`, registry entry text-digits-to-integer.
        (Value::Text(s) | Value::Code(s), T::Integer | T::BigInteger)
            if {
                let d = s.trim();
                let digits = d.strip_prefix(['-', '+']).unwrap_or(d);
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
            } =>
        {
            let n: i64 = s
                .trim()
                .parse()
                .map_err(|_| Signal::Unsupported(format!("text `{s}` into a number (out of range)")))?;
            crate::assumptions::assume(crate::assumptions::Assumption::TextDigitsToInteger);
            match t {
                T::Integer => match i32::try_from(n) {
                    Ok(i) => Value::Integer(i),
                    Err(_) => return unsupported(format!("text `{s}` overflows Integer")),
                },
                _ => Value::BigInteger(n),
            }
        }
        // A Boolean for a Text or Code (a Boolean handed to `Get` of a Code key): as `Format` writes it, Yes or No (the
        // Code upper-cased). Not pinned; Microsoft's tests need the call not to be refused (assumption
        // `boolean-to-text-as-format`).
        (Value::Boolean(b), T::Text(max)) => {
            crate::assumptions::assume(crate::assumptions::Assumption::BooleanToTextAsFormat);
            Value::Text(fit(if b { "Yes" } else { "No" }.to_string(), *max)?.into())
        }
        (Value::Boolean(b), T::Code(max)) => {
            crate::assumptions::assume(crate::assumptions::Assumption::BooleanToTextAsFormat);
            Value::Code(fit(if b { "YES" } else { "NO" }.to_string(), *max)?.into())
        }
        // A Date into a DateTime is that day at midnight, the blank Date the blank DateTime (as CreateDateTime(D, 0T)). Not
        // pinned; Microsoft's production order code assigns a Date to a DateTime (Tests-SCM-Manufacturing, 20 tests;
        // assumption date-to-datetime-is-midnight).
        (v, t) => return convert_rare(v, t),
    })
}

/// `8-4-4-4-12` hexadecimal digits, with or without surrounding braces.
pub fn parse_guid(text: &str) -> Option<[u8; 16]> {
    let t = text.trim();
    let inner = match (t.strip_prefix('{'), t.strip_suffix('}')) {
        (Some(_), Some(_)) => &t[1..t.len() - 1],
        (None, None) => t,
        _ => return None,
    };
    let parts: Vec<&str> = inner.split('-').collect();
    let widths = [8, 4, 4, 4, 12];
    if parts.len() != 5
        || !parts
            .iter()
            .zip(widths)
            .all(|(p, w)| p.len() == w && p.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return None;
    }
    let hex: String = parts.concat();
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// The upper case of a Code: a character whose upper case is several characters keeps its own (`ß` stays `ß`,
/// types/TestProbeScalarContracts.al:CodeAssign_UpperAndTrim, `umlaut_sharp_s`).
pub fn upper_code(s: &str) -> String {
    s.chars()
        .flat_map(|c| {
            let mut u = c.to_uppercase();
            if u.len() == 1 { vec![u.next().unwrap()] } else { vec![c] }
        })
        .collect()
}

/// [`fit`] for a shared text: no copy when it fits.
fn fit_txt(s: crate::text::Txt, max: Option<u32>) -> Result<crate::text::Txt, Signal> {
    match max {
        Some(m) if s.utf16_len() > m as usize => fit(s.into_string(), max).map(Into::into),
        _ => Ok(s),
    }
}

/// The value an over-long text error shows: the first 200 characters and `...` after them (recordref/lt-refs/RfTValues.al:
/// Value_Truncation_And_Length `text250_over`, misc/lt-system-functions/ErrorMessage.al `af235`).
pub fn shown_in_length_error(s: &str) -> String {
    if s.chars().count() > 200 {
        format!("{}...", s.chars().take(200).collect::<String>())
    } else {
        s.to_string()
    }
}

fn fit(s: String, max: Option<u32>) -> Result<String, Signal> {
    match max {
        // types/TestProbeScalarContracts.al:CodeAssign_UpperAndTrim, `too_long_11` (a Code[10] given 11 characters).
        Some(m) if Value::text_len(&s) > m as usize => crate::native::raise(
            format!(
                "The length of the string is {}, but it must be less than or equal to {m} characters. Value: {}",
                Value::text_len(&s),
                shown_in_length_error(&s)
            ),
            "StringLengthExceeded",
        ),
        _ => Ok(s),
    }
}

fn as_decimal(v: &Value) -> Option<Decimal> {
    Some(match v {
        Value::Integer(i) => Decimal::from(*i),
        Value::BigInteger(i) => Decimal::from(*i),
        Value::Decimal(d) => *d,
        Value::Byte(b) => Decimal::from(*b),
        Value::Char(c) => Decimal::from(*c),
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arith {
    Add,
    Sub,
    Mul,
    Div,
    IntDiv,
    Mod,
}

/// Integer with Integer for `+ - * div mod`, in 32 bits. The one body the general path, the bytecode's `BinI` and its typed
/// Integer ops all use, so an overflow or a division by zero gives the same error everywhere. `Div` is a Decimal quotient and
/// is not an Integer operation (the caller keeps it off this function).
#[inline(always)]
pub fn int_arith(op: Arith, a: i32, b: i32) -> Result<i32, Signal> {
    let n = match op {
        Arith::Add => a.checked_add(b),
        Arith::Sub => a.checked_sub(b),
        Arith::Mul => a.checked_mul(b),
        Arith::IntDiv | Arith::Mod if b == 0 => return raise("Attempted to divide by zero.", "AL:DivideByZero"),
        Arith::IntDiv => a.checked_div(b),
        // The smallest Integer mod -1 overflows, as div does (types/lt-scalars/ScT70056.al:Integer_Overflow `min_mod_neg`).
        Arith::Mod => a.checked_rem(b),
        Arith::Div => return Err(Signal::Unsupported("int_arith of the quotient operator".into())),
    };
    n.ok_or_else(arith_overflow)
}

#[cold]
fn arith_overflow() -> Signal {
    match raise::<Value>("Arithmetic operation resulted in an overflow.", "AL:Overflow") {
        Err(s) => s,
        Ok(_) => unreachable!(),
    }
}

pub fn arith(op: Arith, l: &Value, r: &Value) -> Result<Value, Signal> {
    use Value as V;
    // Integer with Integer is done in 32 bits: the same results and errors as the general path below, which goes
    // through a Decimal (session/TestProbeSessionMiscContracts.al:Arithmetic_ErrorTexts for the two error texts). `/` is a
    // Decimal quotient and stays below. `MIN mod -1` is 0, as the Decimal remainder is.
    if let (V::Integer(a), V::Integer(b)) = (l, r) {
        if op != Arith::Div {
            return int_arith(op, *a, *b).map(V::Integer);
        }
    }
    arith_general(op, l, r)
}

/// The conversions no hot path reaches: a Date into a DateTime, else the refusal.
#[cold]
#[inline(never)]
fn convert_rare(v: Value, t: &RtType) -> Result<Value, Signal> {
    match (&v, t) {
        (Value::Date(n, false), RtType::DateTime) if *n >= 0 => Ok(date_to_datetime(*n)),
        // A RecordRef a Variant holds, into a record of its own table: the record with the RecordRef's row and filters, a copy
        // (Base Application's Approvals Mgmt.InformUserOnStatusChange passes its Variant to a `Record "Sales Header"` parameter;
        // 73 Tests-Workflow tests; assumption recordref-into-record). Another table is not pinned.
        (Value::Library(_, h), RtType::Record(table, _)) => {
            let copy = match &*h.borrow() {
                LibraryValue::RecordRef(rec) if rec.borrow().table == *table => Some(rec.borrow().clone()),
                _ => None,
            };
            match copy {
                Some(state) => {
                    crate::assumptions::assume(crate::assumptions::Assumption::RecordRefIntoRecord);
                    Ok(Value::Record(std::rc::Rc::new(std::cell::RefCell::new(state))))
                }
                None => unsupported(format!("conversion of {} to {t}", v.describe(), t = t.describe())),
            }
        }
        // A Variant holding an Integer assigned to a Text variable raises TypeConversion (corpus
        // types/lt-variant/VaTCompare.al:Concat_After_Conversion, af117). The other numbers and a Code target are not pinned.
        (Value::Integer(_), RtType::Text(_)) => raise(
            "The expression Microsoft.Dynamics.Nav.Runtime.NavIndirectValue cannot be type-converted to a Microsoft.Dynamics.Nav.Runtime.NavText value.",
            "TypeConversion",
        ),
        _ => unsupported(format!("conversion of {} to {t}", v.describe(), t = t.describe())),
    }
}

/// A number a Variant holds (`FieldRef.Value()`) assigned to a Text or Code field of a record: its default `Format` text. Not pinned:
/// assumption number-to-text-is-format. Base Application's Workflow Table Relation Value.CreateNew does `Value := FieldRef.Value()` into
/// a Text[250] field (114 Tests-Workflow tests). A Variant into a Text *variable* is not this: it raises (VaTCompare.al:af117).
/// `None` when the value is no number or the target no Text or Code.
#[cold]
#[inline(never)]
pub fn number_into_text_field(
    v: &Value,
    t: &RtType,
    ctx: &dyn crate::format::FormatCtx,
) -> Option<Result<Value, Signal>> {
    if !matches!(
        v,
        Value::Integer(_) | Value::BigInteger(_) | Value::Decimal(_) | Value::Enum(..)
    ) {
        return None;
    }
    let (max, code) = match t {
        RtType::Text(max) => (max, false),
        RtType::Code(max) => (max, true),
        _ => return None,
    };
    crate::assumptions::assume(crate::assumptions::Assumption::NumberToTextIsFormat);
    let text = match crate::format::try_format(v, 0, 0, ctx) {
        Ok(text) => text,
        Err(e) => return Some(Err(Signal::Unsupported(e))),
    };
    Some(if code {
        fit(upper_code(text.trim()), *max).map(|c| Value::Code(c.into()))
    } else {
        fit_txt(text.into(), *max).map(Value::Text)
    })
}

/// A Date into a DateTime is that day at midnight, the blank Date the blank DateTime (as CreateDateTime(D, 0T)). Not pinned;
/// Microsoft's production order code assigns a Date to a DateTime (Tests-SCM-Manufacturing, 20 tests; assumption
/// date-to-datetime-is-midnight). Kept off the conversion's hot path.
#[cold]
#[inline(never)]
fn date_to_datetime(n: i32) -> Value {
    if n == 0 {
        return Value::DateTime(0);
    }
    crate::assumptions::assume(crate::assumptions::Assumption::DateToDateTime);
    Value::DateTime((i64::from(n) - 1) * 86_400_000 + 1)
}

/// A whole number and a Duration add and subtract as milliseconds, into a BigInteger (types/lt-duration-arithmetic/DurationPlusInteger.al
/// `sum_format_default = 2000`, IntegerPlusDuration.al, IntegerMinusDuration.al); the amount is not pinned for overflow. Used to be assumed; Microsoft's routing code
/// adds a day in milliseconds to a Duration (Tests-SCM-Manufacturing, 20 tests; assumption duration-arithmetic-with-number).
#[cold]
#[inline(never)]
fn duration_with_number(op: Arith, duration_first: bool, d: i64, k: i64) -> Result<Value, Signal> {
    let (a, b) = if duration_first { (d, k) } else { (k, d) };
    match op {
        Arith::Add => a.checked_add(b),
        _ => a.checked_sub(b),
    }
    .map(Value::BigInteger)
    .ok_or_else(|| Signal::Unsupported("Duration overflow (error text not pinned yet)".into()))
}

/// Duration `/`, `div` and `mod` by a whole number, and Duration / Duration (pinned by corpus
/// types/lt-filter-enum-relation-duration-guid/D_Duration.al:354-370 with 90000 ms: `/ 4` is 22500 ms, `div 7` is 12,857, `mod 7` is 1,
/// Duration / Duration is the Decimal 3, `/ 7` raises ConversionCast because the quotient is no whole number of milliseconds).
#[cold]
#[inline(never)]
fn duration_divide(op: Arith, a: i64, r: &Value) -> Result<Value, Signal> {
    let (b, by_duration) = match r {
        Value::Duration(ms) => (*ms, true),
        other => (int_of(other).unwrap_or(0), false),
    };
    if b == 0 {
        return raise("Attempted to divide by zero.", "AL:DivideByZero");
    }
    if by_duration {
        // Only the quotient of two Durations is pinned.
        if op != Arith::Div {
            return unsupported(format!("{} of two Durations (not pinned)", op.describe()));
        }
        let q = Decimal::from(a)
            .checked_div(Decimal::from(b))
            .and_then(|q| q.round_sf_with_strategy(18, rust_decimal::RoundingStrategy::MidpointAwayFromZero))
            .ok_or_else(|| Signal::Unsupported("decimal overflow".into()))?;
        return Ok(Value::Decimal(q));
    }
    match op {
        Arith::IntDiv | Arith::Mod => {
            // The result formats as a Decimal (`D1 div 7` is '12,857' in Format; D_Duration.al:369).
            Ok(Value::Decimal(Decimal::from(if op == Arith::IntDiv {
                a / b
            } else {
                a % b
            })))
        }
        Arith::Div if a % b == 0 => Ok(Value::Duration(a / b)),
        Arith::Div => {
            let q = Decimal::from(a)
                .checked_div(Decimal::from(b))
                .and_then(|q| q.round_sf_with_strategy(18, rust_decimal::RoundingStrategy::MidpointAwayFromZero))
                .ok_or_else(|| Signal::Unsupported("decimal overflow".into()))?;
            raise(
                format!(
                    "Overflow under conversion of Microsoft.Dynamics.Nav.Runtime.Decimal18 value {q} to System.Int64."
                ),
                "ConversionCast",
            )
        }
        _ => unsupported("Duration arithmetic (not pinned)".to_string()),
    }
}

fn arith_general(op: Arith, l: &Value, r: &Value) -> Result<Value, Signal> {
    use Value as V;
    // An Option takes part in arithmetic as its ordinal, an Integer (types/PROptionInteger.al:OptionMember_AsInteger:
    // `Opt + 1`, `Opt * 2`).
    if matches!(l, V::Option(..)) || matches!(r, V::Option(..)) {
        let ord = |v: &Value| match v {
            V::Option(n, _) => V::Integer(*n),
            other => other.clone(),
        };
        return arith(op, &ord(l), &ord(r));
    }
    // Text concatenation.
    if op == Arith::Add {
        if let (Some(a), Some(b)) = (text_like(l), text_like(r)) {
            // An empty side leaves the other as it is (shared, no copy).
            if b.is_empty() {
                return Ok(V::Text(a.clone()));
            }
            let mut out = String::with_capacity(a.len() + b.len());
            out.push_str(a);
            out.push_str(b);
            return Ok(V::Text(out.into()));
        }
        // A null string (a FileUpload's file name before a client filled it) joins as nothing (types/PRFileUpload.al:
        // Unfilled_AssignClearAndPass).
        match (l, r) {
            (V::Text(t) | V::Code(t), V::Null) | (V::Null, V::Text(t) | V::Code(t)) => return Ok(V::Text(t.clone())),
            _ => {}
        }
        // A Char next to a text is a character of it, not a number: `'line1' + S[1] + 'line2'`
        // (corpus json/TestJsonXmlDeepContracts.al:71, which round-trips the result through JSON).
        // Two Chars add as numbers. A lone surrogate is not a character a Rust string can hold.
        match (l, r) {
            (V::Text(t) | V::Code(t), V::Char(c)) | (V::Char(c), V::Text(t) | V::Code(t)) => {
                let Some(ch) = char::from_u32(u32::from(*c)) else {
                    return unsupported("a text joined with half of a surrogate pair".to_string());
                };
                let mut out = String::new();
                if matches!(l, V::Char(_)) {
                    out.push(ch);
                    out.push_str(t);
                } else {
                    out.push_str(t);
                    out.push(ch);
                }
                return Ok(V::Text(out.into()));
            }
            _ => {}
        }
    }
    match (l, r) {
        // A whole number of Durations (milliseconds); other Duration arithmetic is not pinned.
        (V::Duration(ms), n) | (n, V::Duration(ms)) if op == Arith::Mul && int_of(n).is_some() => {
            return int_of(n)
                .and_then(|n| n.checked_mul(*ms))
                .map(V::Duration)
                .ok_or_else(|| Signal::Unsupported("Duration overflow (error text not pinned yet)".into()));
        }
        // Duration +/- Duration is a BigInteger of milliseconds: `Format(D1 + D2)` answers '4000', not a duration text, and `R := D1 + D2`
        // converts it back (corpus types/lt-duration-arithmetic/DurationAddDuration.al:Add_TwoVariables_Result, sum_format_default).
        (V::Duration(a), V::Duration(b)) if matches!(op, Arith::Add | Arith::Sub) => {
            let n = if op == Arith::Add {
                a.checked_add(*b)
            } else {
                a.checked_sub(*b)
            };
            return n
                .map(V::BigInteger)
                .ok_or_else(|| Signal::Unsupported("Duration overflow (error text not pinned yet)".into()));
        }
        // A whole number and a Duration add and subtract as milliseconds, into a BigInteger (`duration_with_number`).
        (V::Duration(d), n) | (n, V::Duration(d)) if matches!(op, Arith::Add | Arith::Sub) && int_of(n).is_some() => {
            return duration_with_number(op, matches!(l, V::Duration(_)), *d, int_of(n).unwrap_or(0));
        }
        // DateTime + Duration is a DateTime and DateTime - DateTime a Duration, in milliseconds
        // (types/TestDurationContracts.al:44-171, session/TestDateTimeContracts.al:52-90). The undefined
        // DateTime is not pinned.
        (V::DateTime(a), V::Duration(d)) | (V::Duration(d), V::DateTime(a))
            if *a > 0 && (op == Arith::Add || (op == Arith::Sub && matches!(l, V::DateTime(_)))) =>
        {
            let moved = if op == Arith::Add {
                a.checked_add(*d)
            } else {
                a.checked_sub(*d)
            };
            return moved
                .filter(|m| *m > 0)
                .map(V::DateTime)
                .ok_or_else(|| Signal::Unsupported("DateTime out of range (error text not pinned yet)".into()));
        }
        // DateTime +/- an Integer, BigInteger or integer constant moves by that many milliseconds; a
        // Decimal with a fraction raises a conversion error (types/TestDateTimeIntegerCalcDateContracts.al).
        // The undefined DateTime is not pinned.
        (V::DateTime(a), n) | (n, V::DateTime(a))
            if *a > 0
                && (int_of(n).is_some() || matches!(n, V::Decimal(_)))
                && (op == Arith::Add || (op == Arith::Sub && matches!(l, V::DateTime(_)))) =>
        {
            let ms = match n {
                V::Decimal(d) if d.fract().is_zero() => d.to_i64(),
                V::Decimal(d) => {
                    return crate::native::raise(
                        format!(
                            "Overflow under conversion of Microsoft.Dynamics.Nav.Runtime.Decimal18 value {} to System.Int64.",
                            d.normalize()
                        ),
                        "ConversionCast",
                    );
                }
                other => int_of(other),
            };
            let moved = ms.and_then(|ms| {
                if op == Arith::Add {
                    a.checked_add(ms)
                } else {
                    a.checked_sub(ms)
                }
            });
            return moved
                .filter(|m| *m > 0)
                .map(V::DateTime)
                .ok_or_else(|| Signal::Unsupported("DateTime out of range (error text not pinned yet)".into()));
        }
        // Time +/- an Integer or a Duration moves by that many milliseconds and wraps around midnight; Time - Time is
        // a Duration; the undefined time (0T) cannot be moved ("Adding to an undefined (0T) time." for both
        // directions of a Time and a number, "Subtracting from an undefined (0T) time." for 0T - Time, code
        // InvalidTime); a Decimal with a fraction raises the conversion error (types/PUTimeArithmetic.al, BC 27.5
        // and 29.0; json/TestProbeJsonContracts.al:ValueTypes_DurationTimeDateTime `100000T + 123`).
        (V::Time(a), n) | (n, V::Time(a))
            if matches!(op, Arith::Add | Arith::Sub)
                && (op == Arith::Add || matches!(l, V::Time(_)))
                && (int_of(n).is_some() || matches!(n, V::Duration(_))) =>
        {
            if *a == 0 {
                return raise("Adding to an undefined (0T) time.", "InvalidTime");
            }
            let ms = match n {
                V::Duration(d) => *d,
                other => int_of(other).expect("an integer"),
            };
            let moved = if op == Arith::Add {
                (a - 1).checked_add(ms)
            } else {
                (a - 1).checked_sub(ms)
            };
            return moved
                .map(|m| V::Time(m.rem_euclid(86_400_000) + 1))
                .ok_or_else(|| Signal::Unsupported("Time moved by an amount that overflows (not pinned)".into()));
        }
        // A whole-number Decimal moves a Time like an Integer (Calendar Management's `Time + Decimal`); a fraction raises
        // below. Assumed, as for a Date (registry entry time-add-whole-decimal).
        (V::Time(_), V::Decimal(d)) | (V::Decimal(d), V::Time(_))
            if matches!(op, Arith::Add | Arith::Sub)
                && d.fract().is_zero()
                && (op == Arith::Add || matches!(l, V::Time(_)))
                && i32::try_from(d.mantissa() / 10i128.pow(d.scale())).is_ok() =>
        {
            crate::assumptions::assume(crate::assumptions::Assumption::TimeAddWholeDecimal);
            let n = V::Integer(i32::try_from(d.mantissa() / 10i128.pow(d.scale())).expect("checked"));
            return match (l, r) {
                (V::Time(_), _) => arith(op, l, &n),
                _ => arith(op, &n, r),
            };
        }
        (V::Time(a), V::Decimal(d)) | (V::Decimal(d), V::Time(a))
            if matches!(op, Arith::Add | Arith::Sub) && *a > 0 && !d.fract().is_zero() =>
        {
            return raise(
                format!(
                    "Overflow under conversion of Microsoft.Dynamics.Nav.Runtime.Decimal18 value {} to System.Int32.",
                    d.normalize()
                ),
                "ConversionCast",
            );
        }
        (V::Time(a), V::Time(b)) if op == Arith::Sub => {
            if *a == 0 {
                return raise("Subtracting from an undefined (0T) time.", "InvalidTime");
            }
            if *b == 0 {
                return unsupported("subtraction of an undefined Time from a Time (not pinned)".to_string());
            }
            return Ok(V::Duration(a - b));
        }
        (V::DateTime(a), V::DateTime(b)) if op == Arith::Sub && *a > 0 && *b > 0 => return Ok(V::Duration(a - b)),
        // An undefined date has no difference, and the whole range is too wide a difference (types/lt-datetime/
        // Arithmetic.al:Date_Plus_Minus_Integer `af30`, `af32`, `af34`).
        (V::Date(a, _), V::Date(b, _)) if op == Arith::Sub => {
            if *a == 0 || *b == 0 || (a - b).abs() >= 3_652_058 {
                return date_invalid();
            }
            return Ok(V::Integer(a - b));
        }
        // A whole-number Decimal adds days like an Integer (`StartDate + Days` with `Days: Decimal` holding
        // `(EndDate - StartDate + 1) div 2` in Base Application's Phys. Invt. Count.-Management). A fraction
        // would round or truncate: not pinned, so it stays refused below (docs/probes-needed.md).
        (V::Date(a, c), n) if matches!(op, Arith::Add | Arith::Sub) && whole_days(n).is_some() => {
            if matches!(n, V::Decimal(_)) {
                crate::assumptions::assume(crate::assumptions::Assumption::DateAddWholeDecimal);
            }
            // The result stays within 0001-01-01 and 9999-12-31, an undefined date takes no days, and the sum of days is
            // computed in 32 bits (Arithmetic.al:Date_Arithmetic_Errors, Date_Plus_Minus_Integer `af39`, `af41`).
            let n = whole_days(n).expect("whole");
            let Ok(n) = i32::try_from(n) else {
                return raise("Arithmetic operation resulted in an overflow.", "AL:Overflow");
            };
            let sum = if op == Arith::Add {
                a.checked_add(n)
            } else {
                a.checked_sub(n)
            };
            let Some(sum) = sum else {
                return raise("Arithmetic operation resulted in an overflow.", "AL:Overflow");
            };
            if *a == 0 || !(1..=3_652_059).contains(&sum) {
                return date_invalid();
            }
            return Ok(V::Date(sum, *c));
        }
        _ => {}
    }
    let (Some(a), Some(b)) = (as_decimal(l), as_decimal(r)) else {
        // Duration `/`, `div` and `mod` a whole number, and Duration / Duration (types/lt-filter-enum-relation-duration-guid/
        // D_Duration.al:Duration_Arithmetic). Here, off the path of the numbers.
        if let V::Duration(d) = l {
            if matches!(op, Arith::Div | Arith::IntDiv | Arith::Mod)
                && (int_of(r).is_some() || matches!(r, V::Duration(_)))
            {
                return duration_divide(op, *d, r);
            }
        }
        return unsupported(format!(
            "{} of {l} and {r}",
            op.describe(),
            l = l.describe(),
            r = r.describe()
        ));
    };
    let both_int =
        matches!(l, V::Integer(_) | V::Byte(_) | V::Char(_)) && matches!(r, V::Integer(_) | V::Byte(_) | V::Char(_));
    let any_big = matches!(l, V::BigInteger(_)) || matches!(r, V::BigInteger(_));
    let any_dec = matches!(l, V::Decimal(_)) || matches!(r, V::Decimal(_));
    if b.is_zero() && matches!(op, Arith::Div | Arith::IntDiv | Arith::Mod) {
        // session/TestProbeSessionMiscContracts.al:Arithmetic_ErrorTexts.
        return raise("Attempted to divide by zero.", "AL:DivideByZero");
    }
    let res = match op {
        Arith::Add => a.checked_add(b),
        Arith::Sub => a.checked_sub(b),
        Arith::Mul => a.checked_mul(b),
        Arith::Div => {
            // A quotient keeps 18 significant digits, rounded at the last one
            // (types/TestDateRoundFormatStringContracts.al:Decimal_Division_KeepsEighteenSignificantDigits).
            return Ok(V::Decimal(
                a.checked_div(b)
                    .and_then(|q| q.round_sf_with_strategy(18, rust_decimal::RoundingStrategy::MidpointAwayFromZero))
                    // A quotient carries no trailing zeros (types/lt-scalars/ScT70056.al:Integer_DivisionResult `value 3.5`).
                    .map(|q| q.normalize())
                    .ok_or_else(|| Signal::Unsupported("decimal overflow".into()))?,
            ));
        }
        Arith::IntDiv => (a / b).trunc().into(),
        Arith::Mod => Some(a % b),
    }
    .ok_or_else(|| Signal::Unsupported("decimal overflow".into()))?;
    // A Decimal mod keeps the fraction (types/lt-scalars/ScT70058.al:Decimal_Basic `mod_int`, Decimal_DivModTruncation `mod2`).
    if any_dec && !matches!(op, Arith::IntDiv) {
        return Ok(V::Decimal(res));
    }
    let n = res.trunc().mantissa() / 10i128.pow(res.trunc().scale());
    if any_big {
        return i64::try_from(n)
            .map(V::BigInteger)
            .or_else(|_| raise("Arithmetic operation resulted in an overflow.", "AL:Overflow"));
    }
    if both_int || matches!(op, Arith::IntDiv | Arith::Mod) {
        return i32::try_from(n)
            .map(V::Integer)
            .or_else(|_| raise("Arithmetic operation resulted in an overflow.", "AL:Overflow"));
    }
    Ok(V::Decimal(res))
}

fn text_like(v: &Value) -> Option<&crate::text::Txt> {
    match v {
        Value::Text(s) | Value::Code(s) => Some(s),
        _ => None,
    }
}

fn whole_days(v: &Value) -> Option<i64> {
    match v {
        Value::Decimal(d) if d.fract().is_zero() => d.trunc().to_i64(),
        other => int_of(other),
    }
}

fn int_of(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(i64::from(*i)),
        Value::BigInteger(i) => Some(*i),
        _ => None,
    }
}

fn date_invalid<T>() -> Result<T, Signal> {
    raise("The date is not valid.", "DateInvalid")
}

/// `=`, `<>`, `<`, ... on two values in AL code: as [`compare`], except that an undefined date against 0001-01-01 raises
/// (types/lt-datetime/Arithmetic.al:Date_Comparisons `af72`, `af75`, `af79`).
pub fn compare_operator(l: &Value, r: &Value, ordered: bool) -> Result<Ordering, Signal> {
    if let (Value::Date(a, _), Value::Date(b, _)) = (l, r) {
        if (*a == 0 && *b == 1) || (*a == 1 && *b == 0) {
            return date_invalid();
        }
    }
    if ordered { compare_order(l, r) } else { compare(l, r) }
}

/// Ordering for `<`, `<=`, `>` and `>=`: as [`compare`], except that text follows the culture order
/// (types/TestFormatEvaluateTextContracts.al:Operators_TextComparison_IsCultureOrderAndCaseSensitiveEquality).
pub fn compare_order(l: &Value, r: &Value) -> Result<Ordering, Signal> {
    // Two Codes of digits only (leading zeros and the empty Code included) are ordered by length first, then as text: `'9' < '10'`, `'0010'
    // > '10'`, `'9' < '0010'`, `'0009' > '10'`, `'' < '0'` (types/lt-code-digits-page-run-view/P145CodeCompare.al:P145_CodeVars_DigitsOnly,
    // P145_CodeVars_NumberSeriesPair; text/lt-text-functions/TextCompare.Codeunit.al:CodeComparison `tenlt`). A Text on either side, and a Code
    // with a letter, keep the culture order (P145_CodeVars_WithLetters, P145_TextVsText_DigitStrings, P145_TextVsCode_Mixed).
    if let (Value::Code(a), Value::Code(b)) = (l, r) {
        if let Some(o) = digit_code_order(a, b) {
            return Ok(o);
        }
    }
    if let (Value::Text(_) | Value::Code(_), Value::Text(_) | Value::Code(_)) = (l, r) {
        if let (Some(a), Some(b)) = (crate::native::text_of(l), crate::native::text_of(r)) {
            return crate::collation::compare(&a, &b).map_err(Signal::Unsupported);
        }
    }
    compare(l, r)
}

/// The order of two Codes that are digits only: the shorter is the smaller, else the text order. `None` for anything else.
fn digit_code_order(a: &str, b: &str) -> Option<Ordering> {
    let digits = |t: &str| t.bytes().all(|c| c.is_ascii_digit());
    (digits(a) && digits(b)).then(|| a.len().cmp(&b.len()).then_with(|| a.cmp(b)))
}

/// The order of two text key values: trailing blanks do not count, the letters compare without regard to their case first
/// (collation order), and two texts that differ only in case are ordered by it (`a` before `A`), so they are two keys. A
/// character the collation has no weight for falls back to the code point order of the trimmed texts
/// (record/lt-record-read/RcAGetFind.al:Get_Text_Key, Find_Text_Key_Case_Insensitive).
pub fn text_key_order(a: &str, b: &str) -> Ordering {
    // The common case first, without trimming or copying: byte-equal texts are equal, and two texts that first differ
    // at an upper-case letter or a digit are ordered by that byte (the collation agrees with the code points there), or,
    // when one is the start of the other, by what the longer one adds besides blanks.
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    let n = ab.iter().zip(bb).take_while(|(x, y)| x == y).count();
    if n == ab.len() && n == bb.len() {
        return Ordering::Equal;
    }
    let plain = |c: u8| c.is_ascii_uppercase() || c.is_ascii_digit();
    if n < ab.len() && n < bb.len() && plain(ab[n]) && plain(bb[n]) {
        return ab[n].cmp(&bb[n]);
    }
    if n == ab.len() || n == bb.len() {
        let tail = if n == ab.len() { &bb[n..] } else { &ab[n..] };
        if tail.iter().all(|c| *c == b' ') {
            return Ordering::Equal;
        }
        if tail.iter().all(|c| c.is_ascii_graphic() || *c == b' ') && tail.iter().any(|c| *c != b' ') {
            return if n == ab.len() {
                Ordering::Less
            } else {
                Ordering::Greater
            };
        }
    }
    let (a, b) = (a.trim_end_matches(' '), b.trim_end_matches(' '));
    if a == b {
        return Ordering::Equal;
    }
    // Letters and digits only: the case-folded byte order is the collation's, and the first difference in case decides a tie.
    let plain = |t: &str| t.bytes().all(|c| c.is_ascii_alphanumeric());
    if plain(a) && plain(b) {
        let folded = a
            .bytes()
            .map(|c| c.to_ascii_uppercase())
            .cmp(b.bytes().map(|c| c.to_ascii_uppercase()));
        if folded != Ordering::Equal {
            return folded;
        }
        for (x, y) in a.bytes().zip(b.bytes()) {
            if x != y {
                return if x.is_ascii_lowercase() {
                    Ordering::Less
                } else {
                    Ordering::Greater
                };
            }
        }
        return Ordering::Equal;
    }
    crate::collation::compare(a, b).unwrap_or_else(|_| a.cmp(b))
}

/// [`compare`] for the values of a key: text follows [`text_key_order`].
#[inline]
pub fn compare_key_value(l: &Value, r: &Value) -> Result<Ordering, Signal> {
    match (l, r) {
        (Value::Integer(a), Value::Integer(b)) => Ok(a.cmp(b)),
        (Value::Text(a) | Value::Code(a), Value::Text(b) | Value::Code(b)) if !a.has_marker() && !b.has_marker() => {
            Ok(text_key_order(a, b))
        }
        _ => compare(l, r),
    }
}

/// Ordering for `=`, `<>`, `<`, ... ; `None` when the values are not comparable.
/// Text compares ordinally (case-sensitive): docs/probes-needed.md.
pub fn compare(l: &Value, r: &Value) -> Result<Ordering, Signal> {
    use Value as V;
    // The cases a scan compares millions of times (keys of integers, decimals and text) answer without
    // converting to a Decimal or copying a text; the result is the one of the general path below.
    match (l, r) {
        (V::Integer(a), V::Integer(b)) => return Ok(a.cmp(b)),
        (V::BigInteger(a), V::BigInteger(b)) => return Ok(a.cmp(b)),
        (V::Integer(a), V::BigInteger(b)) => return Ok(i64::from(*a).cmp(b)),
        (V::BigInteger(a), V::Integer(b)) => return Ok(a.cmp(&i64::from(*b))),
        (V::Decimal(a), V::Decimal(b)) => return Ok(a.cmp(b)),
        (V::Boolean(a), V::Boolean(b)) => return Ok(a.cmp(b)),
        (V::Date(a, ca), V::Date(b, cb)) => return Ok((a, ca).cmp(&(b, cb))),
        (V::Time(a), V::Time(b)) | (V::DateTime(a), V::DateTime(b)) | (V::Duration(a), V::Duration(b)) => {
            return Ok(a.cmp(b));
        }
        (V::Guid(a), V::Guid(b)) => return Ok(a.cmp(b)),
        (V::Option(a, _), V::Option(b, _)) => return Ok(a.cmp(b)),
        (V::Text(a) | V::Code(a), V::Text(b) | V::Code(b)) if !a.has_marker() && !b.has_marker() => {
            return Ok(a.cmp(b));
        }
        // A call stack with an untracked frame holds the marker `CALL_STACK_MARK`, so it is not empty: against the empty text the
        // answer is exact (Assert.ExpectedError: `GetLastErrorCallstack = ''`; Tests-ERM, 92 tests). Against any other
        // text it stays refused.
        (V::Text(a) | V::Code(a), V::Text(b) | V::Code(b)) if (a.has_marker() && b.as_str().is_empty()) => {
            return Ok(Ordering::Greater);
        }
        (V::Text(a) | V::Code(a), V::Text(b) | V::Code(b)) if (b.has_marker() && a.as_str().is_empty()) => {
            return Ok(Ordering::Less);
        }
        _ => {}
    }
    compare_general(l, r)
}

/// The rest of [`compare`]: kept out of line so that the fast cases above stay small.
#[inline(never)]
fn compare_general(l: &Value, r: &Value) -> Result<Ordering, Signal> {
    use Value as V;
    if let (Some(a), Some(b)) = (as_decimal(l), as_decimal(r)) {
        if !matches!(l, V::Char(_)) || !matches!(r, V::Text(_) | V::Code(_)) {
            return Ok(a.cmp(&b));
        }
    }
    if let (Some(a), Some(b)) = (crate::native::text_of(l), crate::native::text_of(r)) {
        return Ok(a.cmp(&b));
    }
    Ok(match (l, r) {
        (V::Boolean(a), V::Boolean(b)) => a.cmp(b),
        (V::Date(a, ca), V::Date(b, cb)) => (a, ca).cmp(&(b, cb)),
        (V::Time(a), V::Time(b)) | (V::DateTime(a), V::DateTime(b)) | (V::Duration(a), V::Duration(b)) => a.cmp(b),
        (V::Guid(a), V::Guid(b)) => a.cmp(b),
        (V::Option(a, _), V::Option(b, _)) => a.cmp(b),
        (V::Option(a, _), V::Integer(b)) | (V::Integer(a), V::Option(b, _)) => a.cmp(b),
        // A Duration compares with a whole number of milliseconds (json/TestJsonComplete.al: `Dur > 0`).
        (V::Duration(a), V::Integer(b)) => a.cmp(&i64::from(*b)),
        (V::Integer(a), V::Duration(b)) => i64::from(*a).cmp(b),
        (V::Duration(a), V::BigInteger(b)) => a.cmp(b),
        (V::BigInteger(a), V::Duration(b)) => a.cmp(b),
        (V::Enum(ea, a), V::Enum(eb, b)) if ea == eb => a.cmp(b),
        // Two enum types compare by ordinal: an ordinal converted into an enum that lacks it equals the
        // source (corpus types/PXEnumRecordRef.al:93-97,106, both enums with
        // AssignmentCompatibility; other pairs are an assumption, docs/probes-needed.md).
        (V::Enum(_, a), V::Enum(_, b)) => a.cmp(b),
        (V::DateFormula(a), V::DateFormula(b)) => crate::datefns::formula_text(a).cmp(&crate::datefns::formula_text(b)),
        // SessionSettings compare by value (session/TestSessionSettings.al:513-545).
        (
            V::Library(crate::library_kind::LibraryKind::SessionSettings, a),
            V::Library(crate::library_kind::LibraryKind::SessionSettings, b),
        ) => {
            use crate::value::LibraryValue as L;
            let state = |l: &L| match l {
                L::SessionSettings(s) => s.clone(),
                _ => Default::default(),
            };
            let (a, b) = (state(&a.borrow()), state(&b.borrow()));
            if a == b { Ordering::Equal } else { Ordering::Less }
        }
        (V::Library(_, a), V::Library(_, b)) => {
            use crate::value::LibraryValue as L;
            match (&*a.borrow(), &*b.borrow()) {
                (L::Version(a), L::Version(b)) => a.cmp(b),
                (L::Empty, L::Empty) => Ordering::Equal,
                (L::Empty, L::RecordId { .. }) => Ordering::Less,
                (L::RecordId { .. }, L::Empty) => Ordering::Greater,
                (
                    L::RecordId {
                        table_no: ta, key: ka, ..
                    },
                    L::RecordId {
                        table_no: tb, key: kb, ..
                    },
                ) => {
                    let mut o = ta.cmp(tb);
                    for (x, y) in ka.iter().zip(kb) {
                        if o != Ordering::Equal {
                            break;
                        }
                        o = compare(x, y)?;
                    }
                    o.then(ka.len().cmp(&kb.len()))
                }
                _ => return unsupported(format!("comparison of {} and {r}", l.describe(), r = r.describe())),
            }
        }
        _ => return unsupported(format!("comparison of {} and {r}", l.describe(), r = r.describe())),
    })
}

/// Whether two primary keys are the same value for value.
pub fn compare_keys(a: &[Value], b: &[Value]) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b)
            .all(|(x, y)| compare(x, y).ok() == Some(Ordering::Equal))
}

pub fn truthy(v: &Value) -> Result<bool, Signal> {
    match v {
        Value::Boolean(b) => Ok(*b),
        other => unsupported(format!("{} used as a condition", other.describe())),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn text_keys_are_equal_without_trailing_blanks_and_ordered_by_case_last() {
        use super::text_key_order;
        use std::cmp::Ordering::{Equal, Greater, Less};
        // record/lt-record-read/RcAGetFind.al:Get_Text_Key: 'abc ' is the key 'abc', 'ABC' is another one.
        assert_eq!(text_key_order("abc", "abc  "), Equal);
        assert_ne!(text_key_order("abc", "ABC"), Equal);
        assert_eq!(text_key_order("abc", "ABC"), Less);
        // :Find_Text_Key_Case_Insensitive: 'abc' < 'abc x' < 'ABD' = 'abd' (case last) < 'b'.
        assert_eq!(text_key_order("abc x", "ABD"), Less);
        assert_eq!(text_key_order("abd", "ABD"), Less);
        assert_eq!(text_key_order("ABD", "b"), Less);
        assert_eq!(text_key_order("a_b", "a1"), Less);
        assert_eq!(text_key_order("b", "ABD"), Greater);
        // The fast paths: a first difference at a capital or digit, and a text that starts the other.
        assert_eq!(text_key_order("A1", "A10"), Less);
        assert_eq!(text_key_order("A9", "A10"), Greater);
        assert_eq!(text_key_order("ABC", "ABC   "), Equal);
        assert_eq!(text_key_order("ABC   ", "ABC"), Equal);
        assert_eq!(text_key_order("ABC", "ABC X"), Less);
        assert_eq!(text_key_order("ABC X", "ABC"), Greater);
    }

    use super::*;

    // Microsoft test exception (no corpus pin): Match Bank Payments adds the Durations of its telemetry timers.
    #[test]
    fn two_durations_add_and_subtract_as_milliseconds() {
        let add = arith(Arith::Add, &Value::Duration(1500), &Value::Duration(500)).unwrap();
        assert!(matches!(add, Value::BigInteger(2000)));
        let sub = arith(Arith::Sub, &Value::Duration(500), &Value::Duration(1500)).unwrap();
        assert!(matches!(sub, Value::BigInteger(-1000)));
    }

    #[test]
    fn a_text_with_the_call_stack_marker_is_not_equal_to_the_empty_text() {
        let marked = Value::Text("a\u{FDD0}b".into());
        let empty = Value::Text("".into());
        assert_eq!(compare(&marked, &empty).unwrap(), Ordering::Greater);
        assert_eq!(compare(&empty, &marked).unwrap(), Ordering::Less);
        assert!(compare(&marked, &Value::Text("x".into())).is_err());
    }

    #[test]
    fn a_date_converts_into_a_text_as_its_default_format() {
        let d = Value::Date(crate::format::day_from_civil(2024, 3, 5), false);
        let want = crate::format::try_format(&d, 0, 0, &Plain).unwrap();
        assert!(matches!(convert(d.clone(), &RtType::Text(None)), Ok(Value::Text(t)) if t.as_str() == want));
        assert!(convert(d, &RtType::Text(Some(3))).is_err());
    }

    struct Plain;
    impl crate::format::FormatCtx for Plain {
        fn enum_name(&self, _: u32, _: i32) -> String {
            String::new()
        }
    }

    // Base Application's Incoming Document `Get(Format(FieldRef.Value))` hands a Text to an Integer key (Tests-ERM,
    // about 400 tests behind the Incoming Doc. Attach. FactBox). A text of digits is the number; other text stays refused.
    #[test]
    fn a_text_of_digits_converts_into_an_integer_key() {
        let t = |s: &str| convert(Value::Text(s.into()), &RtType::Integer);
        assert!(matches!(t("0"), Ok(Value::Integer(0))));
        assert!(matches!(t(" -12 "), Ok(Value::Integer(-12))));
        assert!(t("1.5").is_err() && t("abc").is_err() && t("").is_err() && t("99999999999").is_err());
        assert!(matches!(
            convert(Value::Code("7".into()), &RtType::BigInteger),
            Ok(Value::BigInteger(7))
        ));
    }

    /// What Integer arithmetic gives, worked out in 128 bits: the definition the fast path must keep
    /// (session/TestProbeSessionMiscContracts.al:Arithmetic_ErrorTexts for the two error texts).
    fn integer_reference(op: Arith, a: i32, b: i32) -> Option<Result<i32, (&'static str, &'static str)>> {
        const OVERFLOW: (&str, &str) = ("Arithmetic operation resulted in an overflow.", "AL:Overflow");
        const ZERO: (&str, &str) = ("Attempted to divide by zero.", "AL:DivideByZero");
        let (x, y) = (i128::from(a), i128::from(b));
        let wide = match op {
            Arith::Add => x + y,
            Arith::Sub => x - y,
            Arith::Mul => x * y,
            Arith::IntDiv | Arith::Mod if y == 0 => return Some(Err(ZERO)),
            Arith::IntDiv => x / y,
            // The smallest Integer mod -1 overflows (types/lt-scalars/ScT70056.al:Integer_Overflow `min_mod_neg`).
            Arith::Mod if a == i32::MIN && b == -1 => return Some(Err(OVERFLOW)),
            Arith::Mod => x % y,
            Arith::Div => return None,
        };
        Some(i32::try_from(wide).map_err(|_| OVERFLOW))
    }

    /// Tiny deterministic generator: the property test needs no crate.
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            self.0 >> 17
        }
        fn i32(&mut self) -> i32 {
            const EDGES: [i32; 8] = [0, 1, -1, 2, -2, i32::MAX, i32::MIN, 46341];
            match self.next() % 4 {
                0 => EDGES[(self.next() % 8) as usize],
                1 => (self.next() % 2001) as i32 - 1000,
                _ => self.next() as u32 as i32,
            }
        }
    }

    #[test]
    fn integer_arithmetic_gives_what_the_definition_gives_for_random_and_edge_operands() {
        let mut rng = Lcg(7);
        let mut cases: Vec<(i32, i32)> = vec![];
        for a in [0, 1, -1, i32::MAX, i32::MIN] {
            for b in [0, 1, -1, i32::MAX, i32::MIN] {
                cases.push((a, b));
            }
        }
        cases.extend((0..20_000).map(|_| (rng.i32(), rng.i32())));
        for (a, b) in cases {
            for op in [Arith::Add, Arith::Sub, Arith::Mul, Arith::IntDiv, Arith::Mod] {
                let got = arith(op, &Value::Integer(a), &Value::Integer(b));
                let want = integer_reference(op, a, b).expect("not a division");
                match (got, want) {
                    (Ok(Value::Integer(g)), Ok(w)) => assert_eq!(g, w, "{a} {op:?} {b}"),
                    (Err(Signal::Error(e)), Err((message, code))) => {
                        assert_eq!((e.message.as_str(), e.code.as_str()), (message, code), "{a} {op:?} {b}")
                    }
                    (got, want) => panic!("{a} {op:?} {b}: got {got:?}, want {want:?}"),
                }
            }
        }
    }

    /// Regression guard (the behavior is unchanged by moving the body out of `arith`): `int_arith`, which the bytecode's
    /// typed Integer ops call, gives the definition's results and error texts on the edge grid.
    #[test]
    fn int_arith_gives_what_the_definition_gives_on_the_edge_grid() {
        let edges = [0, 1, -1, 2, -2, 46341, i32::MAX, i32::MIN];
        for a in edges {
            for b in edges {
                for op in [Arith::Add, Arith::Sub, Arith::Mul, Arith::IntDiv, Arith::Mod] {
                    match (
                        int_arith(op, a, b),
                        integer_reference(op, a, b).expect("not a division"),
                    ) {
                        (Ok(g), Ok(w)) => assert_eq!(g, w, "{a} {op:?} {b}"),
                        (Err(Signal::Error(e)), Err((m, c))) => {
                            assert_eq!((e.message.as_str(), e.code.as_str()), (m, c), "{a} {op:?} {b}")
                        }
                        (got, want) => panic!("{a} {op:?} {b}: got {got:?}, want {want:?}"),
                    }
                }
            }
        }
    }

    #[test]
    fn integer_division_with_a_slash_stays_a_decimal() {
        for (a, b) in [(7, 2), (-7, 2), (6, 3), (i32::MIN, -1)] {
            match arith(Arith::Div, &Value::Integer(a), &Value::Integer(b)) {
                Ok(Value::Decimal(d)) => assert_eq!(d, Decimal::from(a) / Decimal::from(b), "{a} / {b}"),
                other => panic!("{a} / {b}: {other:?}"),
            }
        }
        assert!(matches!(
            arith(Arith::Div, &Value::Integer(1), &Value::Integer(0)),
            Err(Signal::Error(_))
        ));
    }

    #[test]
    fn converting_a_value_into_its_own_type_gives_the_value() {
        for i in [0, 1, -1, i32::MAX, i32::MIN] {
            assert!(matches!(convert(Value::Integer(i), &RtType::Integer), Ok(Value::Integer(g)) if g == i));
        }
        for i in [0i64, i64::MAX, i64::MIN] {
            assert!(matches!(convert(Value::BigInteger(i), &RtType::BigInteger), Ok(Value::BigInteger(g)) if g == i));
        }
        for s in ["0", "1.50", "-0.000001", "79228162514264337593543950335"] {
            let d = Decimal::from_str(s).unwrap();
            // The scale is kept: 1.50 stays 1.50.
            match convert(Value::Decimal(d), &RtType::Decimal) {
                Ok(Value::Decimal(g)) => assert_eq!((g, g.scale()), (d, d.scale())),
                other => panic!("{s}: {other:?}"),
            }
        }
    }

    use std::str::FromStr;

    fn d(s: &str) -> Value {
        Value::Decimal(Decimal::from_str(s).unwrap())
    }

    // types/PUTimeArithmetic.al: a Time moves by milliseconds and wraps around midnight; 0T cannot be moved.
    #[test]
    fn time_arithmetic_wraps_and_refuses_the_undefined_time() {
        let noon = Value::Time(43_200_001);
        assert!(matches!(
            arith(Arith::Sub, &noon, &Value::Integer(1000)),
            Ok(Value::Time(43_199_001))
        ));
        // 23:59:59.999 + 1 ms is 00:00:00.
        assert!(matches!(
            arith(Arith::Add, &Value::Time(86_400_000), &Value::Integer(1)),
            Ok(Value::Time(1))
        ));
        assert!(matches!(
            arith(Arith::Sub, &noon, &Value::Duration(3_600_000)),
            Ok(Value::Time(39_600_001))
        ));
        assert!(matches!(
            arith(Arith::Sub, &Value::Time(46_800_001), &noon),
            Ok(Value::Duration(3_600_000))
        ));
        for (op, l, r, text) in [
            (
                Arith::Sub,
                Value::Time(0),
                Value::Integer(1),
                "Adding to an undefined (0T) time.",
            ),
            (
                Arith::Sub,
                Value::Time(0),
                noon.clone(),
                "Subtracting from an undefined (0T) time.",
            ),
        ] {
            let Err(Signal::Error(e)) = arith(op, &l, &r) else {
                panic!("expected an error")
            };
            assert_eq!((e.message.as_str(), e.code.as_str()), (text, "InvalidTime"));
        }
        let Err(Signal::Error(e)) = arith(Arith::Add, &noon, &d("1500.5")) else {
            panic!("expected an error")
        };
        assert_eq!(e.code, "ConversionCast");
    }

    // corpus types/PXEnumRecordRef.al:74-108, :137-196: an Enum of another
    // type and an Integer become the target enum's value of that ordinal, a lacking ordinal kept.
    #[test]
    fn an_enum_of_another_type_and_an_integer_convert_by_ordinal() {
        assert!(matches!(
            convert(Value::Enum(5057, 1), &RtType::Enum(5056)),
            Ok(Value::Enum(5056, 1))
        ));
        assert!(matches!(
            convert(Value::Enum(5057, 7), &RtType::Enum(5056)),
            Ok(Value::Enum(5056, 7))
        ));
        assert!(matches!(
            convert(Value::Enum(5056, 1), &RtType::Enum(5056)),
            Ok(Value::Enum(5056, 1))
        ));
        assert!(matches!(
            convert(Value::Integer(5), &RtType::Enum(5056)),
            Ok(Value::Enum(5056, 5))
        ));
        // A BigInteger is not probed; Microsoft's tests assign one (Transfer Order posting, `Enum := BigIntegerField`),
        // so it goes by ordinal as an Integer does (registry entry enum-option-integer-ordinal).
        assert!(matches!(
            convert(Value::BigInteger(5), &RtType::Enum(5056)),
            Ok(Value::Enum(5056, 5))
        ));
        assert!(matches!(
            convert(Value::BigInteger(i64::MAX), &RtType::Enum(5056)),
            Err(Signal::Unsupported(_))
        ));
    }

    // An Enum value into an Option or an Integer, and a BigInteger into an Option, go by ordinal. Nothing in the corpus
    // pins them (docs/probes-needed.md); Microsoft's tests need them: the FlowField filter
    // `"Source Subtype" = field("Document Type")` of Sales Line "Reserved Quantity" (Option column, Enum field),
    // `DequeueInteger` of an enqueued enum in Library - Variable Storage, and the loop
    // `for FromSalesLineDisc."Sales Type" := ...::Customer to ...::Campaign` in Sales Price Calc. Mgt.
    #[test]
    fn an_enum_converts_to_an_option_and_an_integer_by_ordinal_and_a_biginteger_to_an_option() {
        let members = || RtType::Option(vec!["A".into(), "B".into(), "C".into()]);
        assert!(matches!(convert(Value::Enum(37, 2), &members()), Ok(Value::Option(2, m)) if m.len() == 3));
        assert!(matches!(
            convert(Value::Enum(37, 9), &members()),
            Ok(Value::Option(9, _))
        ));
        assert!(matches!(
            convert(Value::Enum(37, 2), &RtType::Integer),
            Ok(Value::Integer(2))
        ));
        assert!(matches!(
            convert(Value::BigInteger(1), &members()),
            Ok(Value::Option(1, _))
        ));
        assert!(matches!(
            convert(Value::BigInteger(0), &members()),
            Ok(Value::Option(0, _))
        ));
        assert!(matches!(
            convert(Value::BigInteger(i64::MAX), &members()),
            Err(Signal::Unsupported(_))
        ));
    }

    // Calendar Management CalcTimeDelta: `Result := EndingTime - StartingTime` into an Integer (about 750 Microsoft tests
    // in service and manufacturing set-ups reach it).
    #[test]
    fn a_duration_converts_to_a_number_of_milliseconds() {
        assert!(matches!(
            convert(Value::Duration(63_565_866), &RtType::Integer),
            Ok(Value::Integer(63_565_866))
        ));
        assert!(matches!(
            convert(Value::Duration(-5), &RtType::BigInteger),
            Ok(Value::BigInteger(-5))
        ));
        assert!(
            matches!(convert(Value::Duration(7), &RtType::Decimal), Ok(Value::Decimal(d)) if d == Decimal::from(7))
        );
        assert!(matches!(
            convert(Value::Duration(i64::MAX), &RtType::Integer),
            Err(Signal::Unsupported(_))
        ));
    }

    // Calendar Management: `Time + Decimal` with a whole Decimal (about 820 Microsoft tests through the service set-up).
    #[test]
    fn a_whole_decimal_moves_a_time_like_an_integer() {
        let t = |ms: i64| Value::Time(ms + 1);
        let dec = |n: i64| Value::Decimal(Decimal::from(n));
        assert!(matches!(arith(Arith::Add, &t(5000), &dec(1000)), Ok(Value::Time(6001))));
        assert!(matches!(arith(Arith::Add, &dec(1000), &t(5000)), Ok(Value::Time(6001))));
        assert!(matches!(arith(Arith::Sub, &t(5000), &dec(1000)), Ok(Value::Time(4001))));
        assert!(matches!(arith(Arith::Add, &t(5000), &dec(0)), Ok(Value::Time(5001))));
    }

    /// `ItemTrackingCode.Get(Serial)` with a Boolean for a Code key (Microsoft's Tests-SCM kitting tests): the Boolean
    /// converts as `Format` writes it. Not pinned (assumption `boolean-to-text-as-format`).
    #[test]
    fn a_boolean_converts_to_text_and_code_as_format_writes_it() {
        let c = convert(Value::Boolean(true), &RtType::Code(Some(10))).unwrap();
        assert_eq!(crate::native::text_of(&c).unwrap().to_string(), "YES");
        let t = convert(Value::Boolean(false), &RtType::Text(None)).unwrap();
        assert_eq!(crate::native::text_of(&t).unwrap().to_string(), "No");
    }

    #[test]
    fn convert_numeric() {
        assert!(matches!(convert(Value::Integer(2), &RtType::Decimal), Ok(Value::Decimal(x)) if x == Decimal::from(2)));
        assert!(matches!(
            convert(Value::Integer(2), &RtType::BigInteger),
            Ok(Value::BigInteger(2))
        ));
        assert!(matches!(convert(d("2"), &RtType::Integer), Ok(Value::Integer(2))));
        // A Decimal with a fraction does not convert into an Integer, it is not rounded (types/PRDecimalToInteger.al).
        let Err(Signal::Error(e)) = convert(d("2.5"), &RtType::Integer) else {
            panic!("2.5 into an Integer must raise");
        };
        assert_eq!(e.code, "ConversionCast");
    }

    // corpus record/TestCalcFormulaValidationTests.Codeunit.al:42: `exit(60 * 60 * 1000)` from a
    // procedure returning Duration; a Duration is a count of milliseconds.
    #[test]
    fn whole_numbers_convert_to_a_duration_in_milliseconds() {
        assert!(matches!(
            convert(Value::Integer(3_600_000), &RtType::Duration),
            Ok(Value::Duration(3_600_000))
        ));
        assert!(matches!(
            convert(Value::BigInteger(5), &RtType::Duration),
            Ok(Value::Duration(5))
        ));
        assert!(matches!(
            convert(d("1500"), &RtType::Duration),
            Ok(Value::Duration(1500))
        ));
        assert!(matches!(
            convert(d("1.5"), &RtType::Duration),
            Err(Signal::Unsupported(_))
        ));
    }

    // corpus record/TestCalcFormulaValidationTests.Codeunit.al:60,129: `1 * OneHour()` and
    // `10 * OneHour()` with a Duration in milliseconds.
    #[test]
    fn a_whole_number_times_a_duration_is_a_duration() {
        let hour = Value::Duration(3_600_000);
        assert!(matches!(
            arith(Arith::Mul, &Value::Integer(10), &hour),
            Ok(Value::Duration(36_000_000))
        ));
        assert!(matches!(
            arith(Arith::Mul, &hour, &Value::Integer(2)),
            Ok(Value::Duration(7_200_000))
        ));
        assert!(
            matches!(
                arith(Arith::Mul, &hour, &Value::Duration(2)),
                Err(Signal::Unsupported(_))
            ),
            "other Duration arithmetic is not pinned"
        );
    }

    /// Duration `/`, `div` and `mod` with a whole number, and Duration / Duration, as corpus
    /// types/lt-filter-enum-relation-duration-guid/D_Duration.al:Duration_Arithmetic pins them (D1 = 90000 ms, D2 = 30000 ms).
    /// A Variant's number into a record's Text or Code field is its Format text (assumption number-to-text-is-format; a BigInteger has
    /// no thousands separator in Format); into a Text variable an Integer raises TypeConversion (corpus VaTCompare.al af117).
    #[test]
    fn a_variant_number_becomes_text_in_a_field_and_raises_in_a_variable() {
        use crate::value::RtType as T;
        struct NoEnums;
        impl crate::format::FormatCtx for NoEnums {
            fn enum_name(&self, _: u32, _: i32) -> String {
                String::new()
            }
        }
        let got = number_into_text_field(&Value::BigInteger(2645), &T::Text(Some(250)), &NoEnums);
        assert!(matches!(&got, Some(Ok(Value::Text(t))) if &**t == "2645"), "{got:?}");
        let got = number_into_text_field(&Value::Integer(7), &T::Code(Some(20)), &NoEnums);
        assert!(matches!(&got, Some(Ok(Value::Code(t))) if &**t == "7"), "{got:?}");
        assert!(number_into_text_field(&Value::Boolean(true), &T::Text(None), &NoEnums).is_none());
        let Err(Signal::Error(e)) = convert(Value::Integer(5), &T::Text(Some(10))) else {
            panic!("an Integer into a Text variable raises");
        };
        assert_eq!(e.code, "TypeConversion");
    }

    #[test]
    fn a_duration_divides_and_takes_the_remainder_by_a_whole_number() {
        let d1 = Value::Duration(90_000);
        assert!(matches!(
            arith(Arith::Div, &d1, &Value::Integer(4)),
            Ok(Value::Duration(22_500))
        ));
        assert!(matches!(
            arith(Arith::IntDiv, &d1, &Value::Integer(7)),
            Ok(Value::Decimal(x)) if x == Decimal::from(12_857)
        ));
        assert!(matches!(
            arith(Arith::Mod, &d1, &Value::Integer(7)),
            Ok(Value::Decimal(x)) if x == Decimal::from(1)
        ));
        assert!(matches!(
            arith(Arith::Div, &d1, &Value::Duration(30_000)),
            Ok(Value::Decimal(x)) if x == Decimal::from(3)
        ));
        let Err(Signal::Error(e)) = arith(Arith::Div, &d1, &Value::Integer(7)) else {
            panic!("a fraction of a millisecond raises");
        };
        assert_eq!(e.code, "ConversionCast");
        assert_eq!(
            e.message,
            "Overflow under conversion of Microsoft.Dynamics.Nav.Runtime.Decimal18 value 12857.1428571428571 to System.Int64."
        );
    }

    #[test]
    fn convert_option_to_integer_and_integer_to_duration() {
        // json/TestJsonComplete.al JsonObject_GetOption_ReturnsInteger: `Opt: Integer; Opt := JObj.GetOption('o')`;
        // JsonArray_GetDuration_ReturnsDuration: `Dur: Duration; Dur := 86400000`.
        assert!(matches!(
            convert(Value::Option(2, std::sync::Arc::from(vec![])), &RtType::Integer),
            Ok(Value::Integer(2))
        ));
        assert!(matches!(
            convert(Value::Integer(86_400_000), &RtType::Duration),
            Ok(Value::Duration(86_400_000))
        ));
        assert!(matches!(
            convert(Value::BigInteger(5), &RtType::Duration),
            Ok(Value::Duration(5))
        ));
    }

    #[test]
    fn convert_text_and_code() {
        // Doc (Code data type): converted to upper case, leading and trailing spaces removed.
        assert!(
            matches!(convert(Value::Text(" ab ".into()), &RtType::Code(None)), Ok(Value::Code(ref s)) if s == "AB")
        );
        assert!(matches!(convert(Value::Code("AB".into()), &RtType::Text(None)), Ok(Value::Text(ref s)) if s == "AB"));
        // types/TestProbeScalarContracts.al:CodeAssign_UpperAndTrim: a text past the declared length raises.
        let Err(Signal::Error(e)) = convert(Value::Text("abc".into()), &RtType::Text(Some(2))) else {
            panic!("a text longer than its type must raise");
        };
        assert_eq!(e.code, "StringLengthExceeded");
        assert!(matches!(
            convert(Value::Text("a".into()), &RtType::Char),
            Ok(Value::Char(97))
        ));
    }

    #[test]
    fn variant_slots_keep_the_value() {
        assert!(matches!(
            convert(Value::Integer(3), &RtType::Variant),
            Ok(Value::Integer(3))
        ));
        assert!(matches!(
            convert(Value::Code("X".into()), &RtType::Variant),
            Ok(Value::Code(_))
        ));
    }

    #[test]
    fn same_type_is_identity() {
        assert!(matches!(
            convert(Value::Boolean(true), &RtType::Boolean),
            Ok(Value::Boolean(true))
        ));
        assert!(matches!(
            convert(Value::Date(5, false), &RtType::Date),
            Ok(Value::Date(5, false))
        ));
    }

    #[test]
    fn arithmetic_with_promotion() {
        assert!(matches!(
            arith(Arith::Add, &Value::Integer(2), &Value::Integer(3)),
            Ok(Value::Integer(5))
        ));
        assert!(
            matches!(arith(Arith::Add, &Value::Integer(2), &d("0.5")), Ok(Value::Decimal(x)) if x == Decimal::from_str("2.5").unwrap())
        );
        assert!(
            matches!(arith(Arith::Div, &Value::Integer(7), &Value::Integer(2)), Ok(Value::Decimal(x)) if x == Decimal::from_str("3.5").unwrap())
        );
        assert!(matches!(
            arith(Arith::IntDiv, &Value::Integer(7), &Value::Integer(2)),
            Ok(Value::Integer(3))
        ));
        assert!(matches!(
            arith(Arith::Mod, &Value::Integer(7), &Value::Integer(2)),
            Ok(Value::Integer(1))
        ));
        assert!(
            matches!(arith(Arith::Add, &Value::Text("a".into()), &Value::Code("B".into())), Ok(Value::Text(ref s)) if s == "aB")
        );
        assert!(matches!(
            arith(Arith::Add, &Value::Date(10, false), &Value::Integer(2)),
            Ok(Value::Date(12, false))
        ));
        assert!(matches!(
            arith(
                Arith::Add,
                &Value::Date(10, false),
                &Value::Decimal(rust_decimal::Decimal::from(3))
            ),
            Ok(Value::Date(13, false))
        ));
        assert!(
            arith(
                Arith::Add,
                &Value::Date(10, false),
                &Value::Decimal(rust_decimal::Decimal::new(25, 1))
            )
            .is_err()
        );
        assert!(matches!(
            arith(Arith::Sub, &Value::Date(10, false), &Value::Date(4, false)),
            Ok(Value::Integer(6))
        ));
    }

    // types/TestDurationContracts.al:44-171 and session/TestDateTimeContracts.al:52-90.
    #[test]
    fn datetime_and_duration_arithmetic() {
        let dt = |n: i64| Value::DateTime(n);
        let dur = |n: i64| Value::Duration(n);
        assert!(matches!(
            arith(Arith::Add, &dt(1_000), &dur(3_600_000)),
            Ok(Value::DateTime(3_601_000))
        ));
        assert!(matches!(
            arith(Arith::Add, &dur(3_600_000), &dt(1_000)),
            Ok(Value::DateTime(3_601_000))
        ));
        assert!(matches!(
            arith(Arith::Sub, &dt(3_601_000), &dur(3_600_000)),
            Ok(Value::DateTime(1_000))
        ));
        assert!(matches!(
            arith(Arith::Sub, &dt(3_601_000), &dt(1_000)),
            Ok(Value::Duration(3_600_000))
        ));
        // The undefined DateTime and a Duration taken from a DateTime are not pinned.
        assert!(matches!(
            arith(Arith::Add, &dt(0), &dur(1)),
            Err(Signal::Unsupported(_))
        ));
        assert!(matches!(arith(Arith::Sub, &dt(5), &dt(0)), Err(Signal::Unsupported(_))));
        assert!(matches!(
            arith(Arith::Sub, &dur(5), &dt(5)),
            Err(Signal::Unsupported(_))
        ));
    }

    // fieldref/..., record/TestRecordGetBySystemId*.al: a Text assigns into a Guid, with or without braces.
    #[test]
    fn a_text_converts_to_a_guid() {
        let g = |s: &str| convert(Value::Text(s.into()), &RtType::Guid);
        assert!(matches!(g("00000000-0000-0000-0000-000000000000"), Ok(Value::Guid(x)) if x == [0; 16]));
        let braced = g("{11111111-2222-3333-4444-555555555555}").unwrap();
        let plain = g("11111111-2222-3333-4444-555555555555").unwrap();
        assert!(
            matches!((&braced, &plain), (Value::Guid(a), Value::Guid(b)) if a == b && a[0] == 0x11 && a[15] == 0x55)
        );
        assert!(matches!(g("not a guid"), Err(Signal::Unsupported(_))));
        assert!(matches!(
            convert(
                Value::Code("11111111-1111-1111-1111-111111111111".into()),
                &RtType::Guid
            ),
            Ok(Value::Guid(_))
        ));
    }

    #[test]
    fn division_by_zero_and_overflow_raise_the_pinned_errors() {
        // session/TestProbeSessionMiscContracts.al:Arithmetic_ErrorTexts.
        let Err(Signal::Error(e)) = arith(Arith::Div, &Value::Integer(1), &Value::Integer(0)) else {
            panic!("1 / 0 must raise");
        };
        assert_eq!(
            (e.message.as_str(), e.code.as_str()),
            ("Attempted to divide by zero.", "AL:DivideByZero")
        );
        let Err(Signal::Error(e)) = arith(Arith::Add, &Value::Integer(i32::MAX), &Value::Integer(1)) else {
            panic!("an Integer overflow must raise");
        };
        assert_eq!(
            (e.message.as_str(), e.code.as_str()),
            ("Arithmetic operation resulted in an overflow.", "AL:Overflow")
        );
    }

    #[test]
    fn comparisons() {
        assert_eq!(compare(&Value::Integer(1), &d("1.0")).unwrap(), Ordering::Equal);
        assert_eq!(compare(&Value::Integer(1), &Value::Integer(2)).unwrap(), Ordering::Less);
        assert_eq!(
            compare(&Value::Text("a".into()), &Value::Text("a".into())).unwrap(),
            Ordering::Equal
        );
        assert_eq!(
            compare(&Value::Code("A".into()), &Value::Text("A".into())).unwrap(),
            Ordering::Equal
        );
        assert_eq!(
            compare(&Value::Boolean(false), &Value::Boolean(true)).unwrap(),
            Ordering::Less
        );
        assert_eq!(
            compare(&Value::Enum(0, 7), &Value::Enum(0, 7)).unwrap(),
            Ordering::Equal
        );
        assert!(compare(&Value::Integer(1), &Value::Text("1".into())).is_err());
        // json/TestJsonComplete.al JsonObject_GetDuration_ReturnsDuration: `Dur > 0`.
        assert_eq!(
            compare(&Value::Duration(3_600_000), &Value::Integer(0)).unwrap(),
            Ordering::Greater
        );
        assert_eq!(
            compare(&Value::Integer(0), &Value::Duration(5)).unwrap(),
            Ordering::Less
        );
    }
}
