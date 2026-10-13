//! `Evaluate(var Variable, String [, Number])`.
//!
//! Only the (type, text form) pairs a corpus test pins are accepted; text that
//! plainly is not a value of the type (no digits for a number, empty) fails the
//! way the corpus shows, and every other form raises `Signal::Unsupported`.

use crate::format::day_from_civil;
use crate::native::{Ctx, Signal};
use crate::value::Value;
use rust_decimal::Decimal;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

/// What parsing one text produced.
enum Parsed {
    Value(Value),
    /// The text is not a value of the type: `Evaluate` returns false.
    Rejected,
}

/// `[-]digits`, or a rejection when there are no digits at all.
fn whole_number(text: &str, ty: &str) -> Result<Parsed, Signal> {
    let digits = text.strip_prefix('-').unwrap_or(text);
    if !text.bytes().any(|b| b.is_ascii_digit()) {
        // text/TestEvaluate.al Evaluate_InvalidInteger_ReturnsFalse ('notanumber') and
        // Evaluate_EmptyString_ReturnsFalse ('').
        return Ok(Parsed::Rejected);
    }
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return unsupported(format!(
            "Evaluate of `{text}` into {ty} (form not pinned by the corpus)"
        ));
    }
    Ok(Parsed::Value(Value::Text(text.to_string())))
}

fn integer(text: &str) -> Result<Parsed, Signal> {
    Ok(match whole_number(text, "Integer")? {
        Parsed::Value(_) => match text.parse::<i32>() {
            Ok(n) => Parsed::Value(Value::Integer(n)),
            Err(_) => return unsupported(format!("Evaluate of `{text}` outside the Integer range")),
        },
        Parsed::Rejected => Parsed::Rejected,
    })
}

fn big_integer(text: &str) -> Result<Parsed, Signal> {
    Ok(match whole_number(text, "BigInteger")? {
        Parsed::Value(_) => match text.parse::<i64>() {
            Ok(n) => Parsed::Value(Value::BigInteger(n)),
            Err(_) => return unsupported(format!("Evaluate of `{text}` outside the BigInteger range")),
        },
        Parsed::Rejected => Parsed::Rejected,
    })
}

fn decimal(text: &str) -> Result<Parsed, Signal> {
    if !text.bytes().any(|b| b.is_ascii_digit()) {
        return Ok(Parsed::Rejected);
    }
    let body = text.strip_prefix('-').unwrap_or(text);
    let (int, frac) = body.split_once('.').map_or((body, Some("")), |(i, f)| (i, Some(f)));
    let well_formed = !int.is_empty()
        && int.bytes().all(|b| b.is_ascii_digit())
        && frac.is_some_and(|f| f.bytes().all(|b| b.is_ascii_digit()))
        && !(body.contains('.') && frac == Some(""));
    if !well_formed {
        return unsupported(format!(
            "Evaluate of `{text}` into Decimal (form not pinned by the corpus)"
        ));
    }
    match text.parse::<Decimal>() {
        Ok(d) => Ok(Parsed::Value(Value::Decimal(d))),
        Err(_) => unsupported(format!("Evaluate of `{text}` outside the Decimal range")),
    }
}

fn boolean(text: &str) -> Result<Parsed, Signal> {
    // text/TestEvaluate.al: 'true' / 'false'; types/TestFormatBooleanContracts.al:112
    // evaluates Format(true), which is 'Yes' (Format(false) is 'No').
    match text {
        "true" | "Yes" => Ok(Parsed::Value(Value::Boolean(true))),
        "false" | "No" => Ok(Parsed::Value(Value::Boolean(false))),
        _ => unsupported(format!(
            "Evaluate of `{text}` into Boolean (form not pinned by the corpus)"
        )),
    }
}

fn guid(text: &str) -> Result<Parsed, Signal> {
    // types/TestGuid.al:78 evaluates Format(Guid) (braces, upper case); the XML form without
    // braces is not pinned.
    let inner = text.strip_prefix('{').and_then(|t| t.strip_suffix('}'));
    let Some(inner) = inner else {
        return unsupported(format!(
            "Evaluate of `{text}` into Guid (form not pinned by the corpus)"
        ));
    };
    let parts: Vec<&str> = inner.split('-').collect();
    let widths = [8, 4, 4, 4, 12];
    let shaped = parts.len() == 5
        && parts
            .iter()
            .zip(widths)
            .all(|(p, w)| p.len() == w && p.bytes().all(|b| b.is_ascii_hexdigit()));
    if !shaped {
        return unsupported(format!(
            "Evaluate of `{text}` into Guid (form not pinned by the corpus)"
        ));
    }
    let hex: String = parts.concat();
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex digits checked");
    }
    Ok(Parsed::Value(Value::Guid(out)))
}

/// Number 0: the inverse of `Format(Date)` as the runtime formats it (`MM/DD/YY`,
/// docs/probes-needed.md); a two-digit year is read as 1930-2029. Number 9: ISO `yyyy-MM-dd`.
fn date(text: &str, number: i64) -> Result<Parsed, Signal> {
    if number == 9 {
        return match crate::iso::parse_date(text).filter(|_| text.len() == 10) {
            Some(d) => Ok(Parsed::Value(Value::Date(d, false))),
            None => unsupported(format!(
                "Evaluate of `{text}` into Date with format 9 (form not pinned)"
            )),
        };
    }
    let parts: Vec<&str> = text.split('/').collect();
    let two = |s: &str| s.len() == 2 && s.bytes().all(|b| b.is_ascii_digit());
    if parts.len() == 3 && parts.iter().all(|p| two(p)) {
        let (m, d, yy): (u32, u32, i32) = (
            parts[0].parse().unwrap(),
            parts[1].parse().unwrap(),
            parts[2].parse().unwrap(),
        );
        let y = if yy < 30 { 2000 + yy } else { 1900 + yy };
        let valid = (1..=12).contains(&m) && d >= 1 && {
            let first = day_from_civil(y, m, 1);
            let next = if m == 12 {
                day_from_civil(y + 1, 1, 1)
            } else {
                day_from_civil(y, m + 1, 1)
            };
            (d as i32) <= next - first
        };
        if valid {
            return Ok(Parsed::Value(Value::Date(day_from_civil(y, m, d), false)));
        }
        return Ok(Parsed::Rejected);
    }
    unsupported(format!(
        "Evaluate of `{text}` into Date (form not pinned by the corpus)"
    ))
}

/// Only the XML round-trip form with format number 9 is pinned
/// (session/TestSessionEvaluateRegionalPatterns.al:38).
fn datetime(text: &str, number: i64) -> Result<Parsed, Signal> {
    if number == 9 {
        if let Some(v) = crate::iso::parse_datetime(text) {
            return Ok(Parsed::Value(Value::DateTime(v)));
        }
    }
    unsupported(format!(
        "Evaluate of `{text}` into DateTime with format {number} (form not pinned)"
    ))
}

pub fn evaluate(c: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Result<Value, Signal> {
    let Some(text) = crate::native::text_of(&a[1]) else {
        return unsupported("Evaluate of a source that is not text");
    };
    let number = match a.get(2) {
        None => 0,
        Some(Value::Integer(n)) => i64::from(*n),
        Some(other) => return unsupported(format!("Evaluate format number {other:?}")),
    };
    if number != 0 && number != 9 {
        return unsupported(format!(
            "Evaluate with format number {number} (not pinned by the corpus)"
        ));
    }
    let parsed = match &a[0] {
        Value::Integer(_) => integer(&text)?,
        Value::BigInteger(_) => big_integer(&text)?,
        Value::Decimal(_) => decimal(&text)?,
        Value::Boolean(_) => boolean(&text)?,
        Value::Guid(_) => guid(&text)?,
        Value::Date(..) => date(&text, number)?,
        Value::DateTime(_) => datetime(&text, number)?,
        // The text is kept as written once it is a form the runtime can calculate with
        // (types/TestDateFormulaContracts.al:21-139); what Format prints is docs/probes-needed.md.
        Value::DateFormula(_) => {
            crate::datefns::check_formula(&text)?;
            Parsed::Value(Value::DateFormula(text.clone()))
        }
        other => return unsupported(format!("Evaluate into {other:?} (not pinned by the corpus)")),
    };
    match parsed {
        Parsed::Value(v) => {
            a[0] = v;
            Ok(Value::Boolean(true))
        }
        Parsed::Rejected if c.ret_used() => Ok(Value::Boolean(false)),
        Parsed::Rejected => unsupported(format!(
            "Evaluate of `{text}` failed with the result unused (error text not pinned yet)"
        )),
    }
}
