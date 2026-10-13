//! Value operations: conversion to a slot type, arithmetic, comparison
//! (ADR-010: conversions live next to the value model).

use crate::native::Signal;
use crate::value::{RtType, Value};
use rust_decimal::Decimal;
use std::cmp::Ordering;

fn unsupported<T>(what: String) -> Result<T, Signal> {
    Err(Signal::Unsupported(what))
}

/// Converts `v` for storing in a slot of type `t`.
pub fn convert(v: Value, t: &RtType) -> Result<Value, Signal> {
    use RtType as T;
    Ok(match (v, t) {
        (v, T::Variant) => v,
        (Value::Null, t) => Value::default_for(t),
        (v @ Value::Boolean(_), T::Boolean) => v,
        (v @ Value::Date(..), T::Date) | (v @ Value::Time(_), T::Time) | (v @ Value::DateTime(_), T::DateTime) => v,
        (v @ Value::Duration(_), T::Duration)
        | (v @ Value::Guid(_), T::Guid)
        | (v @ Value::DateFormula(_), T::DateFormula) => v,
        (v @ Value::List(_), T::List(_))
        | (v @ Value::Dictionary(_), T::Dictionary(..))
        | (v @ Value::Array(_), T::Array(..)) => v,
        (v @ Value::Codeunit(_), T::Codeunit(_)) | (v @ Value::Library(..), T::Library(_)) => v,
        (v @ Value::Record(_), T::Record(..)) => v,
        (v @ Value::Enum(..), T::Enum(_)) => v,
        (Value::Option(o, _), T::Option(m)) => Value::Option(o, m.clone().into()),
        (Value::Integer(i), T::Option(m)) => Value::Option(i, m.clone().into()),
        // Numbers.
        (v, T::Integer | T::BigInteger | T::Decimal | T::Byte | T::Char)
            if as_decimal(&v).is_some() && !matches!(v, Value::Text(_) | Value::Code(_)) =>
        {
            let d = as_decimal(&v).expect("numeric");
            match t {
                T::Decimal => Value::Decimal(d),
                _ if d.fract() != Decimal::ZERO => {
                    return unsupported(format!("rounding {d} into {t:?} (not pinned by the corpus yet)"));
                }
                T::Integer => Value::Integer(
                    i32::try_from(d.mantissa() / 10i128.pow(d.scale()))
                        .or_else(|_| unsupported(format!("{d} overflows Integer")))?,
                ),
                T::BigInteger => Value::BigInteger(
                    i64::try_from(d.mantissa() / 10i128.pow(d.scale()))
                        .or_else(|_| unsupported(format!("{d} overflows BigInteger")))?,
                ),
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
        // Text-like.
        (v, T::Text(max)) if crate::native::text_of(&v).is_some() => {
            Value::Text(fit(crate::native::text_of(&v).expect("text"), *max)?)
        }
        (v, T::Code(max)) if crate::native::text_of(&v).is_some() => {
            // Doc (Code data type): upper case, leading and trailing spaces removed.
            let s = crate::native::text_of(&v).expect("text").trim().to_uppercase();
            Value::Code(fit(s, *max)?)
        }
        (Value::Text(s) | Value::Code(s), T::Char) => {
            let u: Vec<u16> = s.encode_utf16().collect();
            match u.as_slice() {
                [c] => Value::Char(*c),
                _ => return unsupported(format!("text of length {} into Char", u.len())),
            }
        }
        (v, t) => return unsupported(format!("conversion of {v:?} to {t:?}")),
    })
}

fn fit(s: String, max: Option<u32>) -> Result<String, Signal> {
    match max {
        Some(m) if Value::text_len(&s) > m as usize => unsupported(format!(
            "text of length {} into a slot of length {m} (error text not pinned yet)",
            Value::text_len(&s)
        )),
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

pub fn arith(op: Arith, l: &Value, r: &Value) -> Result<Value, Signal> {
    use Value as V;
    // Text concatenation.
    if op == Arith::Add {
        if let (Some(a), Some(b)) = (text_like(l), text_like(r)) {
            return Ok(V::Text(a + &b));
        }
    }
    match (l, r) {
        (V::Date(a, _), V::Date(b, _)) if op == Arith::Sub => return Ok(V::Integer(a - b)),
        (V::Date(a, c), n) if matches!(op, Arith::Add | Arith::Sub) && int_of(n).is_some() => {
            let n = int_of(n).expect("int") as i32;
            return Ok(V::Date(if op == Arith::Add { a + n } else { a - n }, *c));
        }
        _ => {}
    }
    let (Some(a), Some(b)) = (as_decimal(l), as_decimal(r)) else {
        return unsupported(format!("{op:?} of {l:?} and {r:?}"));
    };
    let both_int =
        matches!(l, V::Integer(_) | V::Byte(_) | V::Char(_)) && matches!(r, V::Integer(_) | V::Byte(_) | V::Char(_));
    let any_big = matches!(l, V::BigInteger(_)) || matches!(r, V::BigInteger(_));
    let any_dec = matches!(l, V::Decimal(_)) || matches!(r, V::Decimal(_));
    if b.is_zero() && matches!(op, Arith::Div | Arith::IntDiv | Arith::Mod) {
        return unsupported("division by zero (error text not pinned yet)".into());
    }
    let res = match op {
        Arith::Add => a.checked_add(b),
        Arith::Sub => a.checked_sub(b),
        Arith::Mul => a.checked_mul(b),
        Arith::Div => {
            return Ok(V::Decimal(
                a.checked_div(b)
                    .ok_or_else(|| Signal::Unsupported("decimal overflow".into()))?,
            ));
        }
        Arith::IntDiv => (a / b).trunc().into(),
        Arith::Mod => Some(a % b),
    }
    .ok_or_else(|| Signal::Unsupported("decimal overflow".into()))?;
    if any_dec && !matches!(op, Arith::IntDiv | Arith::Mod) {
        return Ok(V::Decimal(res));
    }
    let n = res.trunc().mantissa() / 10i128.pow(res.trunc().scale());
    if any_big {
        return i64::try_from(n)
            .map(V::BigInteger)
            .or_else(|_| unsupported("BigInteger overflow (error text not pinned yet)".into()));
    }
    if both_int || matches!(op, Arith::IntDiv | Arith::Mod) {
        return i32::try_from(n)
            .map(V::Integer)
            .or_else(|_| unsupported("Integer overflow (error text not pinned yet)".into()));
    }
    Ok(V::Decimal(res))
}

fn text_like(v: &Value) -> Option<String> {
    match v {
        Value::Text(s) | Value::Code(s) => Some(s.clone()),
        _ => None,
    }
}

fn int_of(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(i64::from(*i)),
        Value::BigInteger(i) => Some(*i),
        _ => None,
    }
}

/// Ordering for `=`, `<>`, `<`, ... ; `None` when the values are not comparable.
/// Text compares ordinally (case-sensitive): docs/probes-needed.md.
pub fn compare(l: &Value, r: &Value) -> Result<Ordering, Signal> {
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
        (V::Enum(ea, a), V::Enum(eb, b)) if ea == eb => a.cmp(b),
        (V::DateFormula(a), V::DateFormula(b)) => a.cmp(b),
        _ => return unsupported(format!("comparison of {l:?} and {r:?}")),
    })
}

pub fn truthy(v: &Value) -> Result<bool, Signal> {
    match v {
        Value::Boolean(b) => Ok(*b),
        other => unsupported(format!("{other:?} used as a condition")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn d(s: &str) -> Value {
        Value::Decimal(Decimal::from_str(s).unwrap())
    }

    #[test]
    fn convert_numeric() {
        assert!(matches!(convert(Value::Integer(2), &RtType::Decimal), Ok(Value::Decimal(x)) if x == Decimal::from(2)));
        assert!(matches!(
            convert(Value::Integer(2), &RtType::BigInteger),
            Ok(Value::BigInteger(2))
        ));
        assert!(matches!(convert(d("2"), &RtType::Integer), Ok(Value::Integer(2))));
        assert!(
            matches!(convert(d("2.5"), &RtType::Integer), Err(Signal::Unsupported(_))),
            "rounding rule not pinned yet"
        );
    }

    #[test]
    fn convert_text_and_code() {
        // Doc (Code data type): converted to upper case, leading and trailing spaces removed.
        assert!(
            matches!(convert(Value::Text(" ab ".into()), &RtType::Code(None)), Ok(Value::Code(ref s)) if s == "AB")
        );
        assert!(matches!(convert(Value::Code("AB".into()), &RtType::Text(None)), Ok(Value::Text(ref s)) if s == "AB"));
        assert!(
            matches!(
                convert(Value::Text("abc".into()), &RtType::Text(Some(2))),
                Err(Signal::Unsupported(_))
            ),
            "overflow message not pinned yet"
        );
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
            arith(Arith::Sub, &Value::Date(10, false), &Value::Date(4, false)),
            Ok(Value::Integer(6))
        ));
    }

    #[test]
    fn division_by_zero_and_overflow_are_not_guessed() {
        assert!(matches!(
            arith(Arith::Div, &Value::Integer(1), &Value::Integer(0)),
            Err(Signal::Unsupported(_))
        ));
        assert!(matches!(
            arith(Arith::Add, &Value::Integer(i32::MAX), &Value::Integer(1)),
            Err(Signal::Unsupported(_))
        ));
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
    }
}
