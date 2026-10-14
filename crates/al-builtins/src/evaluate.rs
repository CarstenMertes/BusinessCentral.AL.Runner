//! `Evaluate(var Variable, String [, Number])`.
//!
//! Only the (type, text form) pairs a corpus test pins are accepted; text that
//! plainly is not a value of the type (no digits for a number, empty) fails the
//! way the corpus shows, and every other form raises `Signal::Unsupported`.

use crate::describe::Describe;
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
    // Thousands separators in properly grouped digits are accepted
    // (types/TestFormatEvaluateTextContracts.al:Evaluate_Failed_LeavesTheVariableUnchanged: `1,234.5`).
    let grouped = int.contains(',') && {
        let groups: Vec<&str> = int.split(',').collect();
        (1..=3).contains(&groups[0].len())
            && groups.iter().all(|g| g.bytes().all(|b| b.is_ascii_digit()))
            && groups[1..].iter().all(|g| g.len() == 3)
    };
    let ungrouped = int.replace(',', "");
    let (text, int) = if grouped {
        (text.replacen(int, &ungrouped, 1), ungrouped.as_str())
    } else {
        (text.to_string(), int)
    };
    let text = text.as_str();
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
    // evaluates Format(true), which is 'Yes' (Format(false) is 'No'). In any case, `1` and `T` are
    // true and anything else, `Ja` for one, is refused
    // (types/TestFormatEvaluateTextContracts.al:Evaluate_Boolean_AcceptsTrueFalseYesNoOneAndT_IgnoringCase).
    // `0` and `F` are not measured.
    match text.to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" | "t" => Ok(Parsed::Value(Value::Boolean(true))),
        "false" | "no" => Ok(Parsed::Value(Value::Boolean(false))),
        "0" | "f" => unsupported(format!(
            "Evaluate of `{text}` into Boolean (form not pinned by the corpus)"
        )),
        _ => Ok(Parsed::Rejected),
    }
}

fn guid(text: &str) -> Result<Parsed, Signal> {
    // The braced, the plain (36 characters) and the 32 digit forms; anything else, the empty text
    // included, is rejected (types/TestJsonTokenUnassignedEvaluateGuidContracts.al:EvaluateGuid_Forms_AndFailures).
    let inner = text
        .strip_prefix('{')
        .and_then(|t| t.strip_suffix('}'))
        .unwrap_or(text);
    let parts: Vec<&str> = inner.split('-').collect();
    let hex_only = |p: &str| p.bytes().all(|b| b.is_ascii_hexdigit());
    let hex: String = if parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, w)| p.len() == w && hex_only(p))
    {
        parts.concat()
    } else if parts.len() == 1 && inner.len() == 32 && hex_only(inner) && inner.len() == text.len() {
        inner.to_string()
    } else {
        return Ok(Parsed::Rejected);
    };
    let mut out = [0u8; 16];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex digits checked");
    }
    Ok(Parsed::Value(Value::Guid(out)))
}

/// Number 0: the inverse of `Format(Date)` as the runtime formats it (`MM/DD/YY`,
/// docs/probes-needed.md); a two-digit year below 50 is 20yy and from 50 on 19yy. Number 9: ISO `yyyy-MM-dd`.
fn date(text: &str, number: i64) -> Result<Parsed, Signal> {
    if number == 9 {
        return match crate::iso::parse_date(text).filter(|_| text.len() == 10) {
            Some(d) => Ok(Parsed::Value(Value::Date(d, false))),
            None => unsupported(format!(
                "Evaluate of `{text}` into Date with format 9 (form not pinned)"
            )),
        };
    }
    // The ISO form is read in format 0 too (types/TestFormatEvaluateTextContracts.al:Evaluate_Date_TwoDigitYear_PivotsAt50).
    if text.len() == 10 && text.as_bytes()[4] == b'-' {
        return match crate::iso::parse_date(text) {
            Some(d) => Ok(Parsed::Value(Value::Date(d, false))),
            None => Ok(Parsed::Rejected),
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
        // types/TestFormatEvaluateTextContracts.al:Evaluate_Date_TwoDigitYear_PivotsAt50.
        let y = if yy < 50 { 2000 + yy } else { 1900 + yy };
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
    // Text with no digit at all is no date (types/TestFormatEvaluateTextContracts.al:
    // Evaluate_Failed_LeavesTheVariableUnchanged: `garbage`).
    if !text.bytes().any(|b| b.is_ascii_digit()) {
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
        Some(other) => return unsupported(format!("Evaluate format number {}", other.describe())),
    };
    // Number 2 is read for a DateFormula only, as number 0 (docs/probes-needed.md).
    if number != 0 && number != 9 && !(number == 2 && matches!(a[0], Value::DateFormula(_))) {
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
        // `Evaluate(V, '1.2.3.4')`: the four-part form (session/TestMiscTypes.al:44-80).
        Value::Library(n, cell) if n == "Version" => {
            let parts: Vec<&str> = text.split('.').collect();
            let nums: Option<Vec<i32>> = parts
                .iter()
                .map(|p| {
                    (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                        .then(|| p.parse().ok())
                        .flatten()
                })
                .collect();
            match nums.filter(|n| n.len() == 4) {
                Some(n) => {
                    *cell.borrow_mut() = crate::value::LibraryValue::Version([n[0], n[1], n[2], n[3]]);
                    return Ok(Value::Boolean(true));
                }
                None => return unsupported(format!("Evaluate of `{text}` into Version (only a.b.c.d is pinned)")),
            }
        }
        // An enum: format 9 reads the ordinal that `Format(Enum, 0, 9)` writes (this runtime's
        // Format, docs/probes-needed.md); format 0 reads a member name, as the InitValue of a field does
        // (types/TestBlankEnumInitTests.Codeunit.al:92). A text that matches no member is rejected.
        Value::Enum(id, _) => {
            let Some(members) = c.enum_members(*id) else {
                return unsupported("Evaluate into an Enum without its members");
            };
            let found = if number == 9 {
                text.parse::<i32>()
                    .ok()
                    .filter(|_| text.bytes().all(|b| b.is_ascii_digit()))
                    .and_then(|n| members.iter().find(|(o, _)| *o == n).map(|(o, _)| *o))
            } else {
                members.iter().find(|(_, n)| *n == text).map(|(o, _)| *o)
            };
            match found {
                Some(o) => Parsed::Value(Value::Enum(*id, o)),
                None => Parsed::Rejected,
            }
        }
        // An ordinal into SecurityFilter: `Evaluate(Filter, '3')` gives Disallowed
        // (recordref/TestPlatformOptionsLockingContracts.al:42-43). Other options are not pinned.
        Value::Option(_, members) if crate::sysenum::is_security_filter(members) => {
            match text.trim().parse::<usize>().ok().filter(|n| *n < members.len()) {
                Some(n) => Parsed::Value(Value::Option(n as i32, members.clone())),
                None => {
                    return unsupported(format!(
                        "Evaluate of `{text}` into SecurityFilter (only an ordinal is pinned)"
                    ));
                }
            }
        }
        // The blank-like texts store the empty formula; garbage fails and leaves the value
        // (types/TestDateFormulaDurationFormatContracts.al:28-74, TestDateFormulaEvaluateLeftoversContracts.al).
        Value::DateFormula(_) => match crate::datefns::evaluate_formula(&text)? {
            crate::datefns::Evaluated::Stored(f) => Parsed::Value(Value::DateFormula(f)),
            crate::datefns::Evaluated::Rejected => Parsed::Rejected,
        },
        other => return unsupported(format!("Evaluate into {} (not pinned by the corpus)", other.describe())),
    };
    match parsed {
        Parsed::Value(v) => {
            a[0] = v;
            Ok(Value::Boolean(true))
        }
        Parsed::Rejected if c.ret_used() => Ok(Value::Boolean(false)),
        Parsed::Rejected if matches!(a[0], Value::Guid(_)) => crate::native::raise(
            "Invalid format of GUID string. The correct format of the GUID string is: CDEF7890-ABCD-0123-1234-567890ABCDEF where 0-9, A-F symbolizes hexadecimal digits.",
            "InvalidGuidFormat",
        ),
        Parsed::Rejected => unsupported(format!(
            "Evaluate of `{text}` failed with the result unused (error text not pinned yet)"
        )),
    }
}
