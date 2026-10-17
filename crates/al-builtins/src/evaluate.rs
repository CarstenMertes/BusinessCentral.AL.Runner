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
    Ok(Parsed::Value(Value::Text(text.to_string().into())))
}

/// A whole number as `Evaluate` reads it into an Integer. Spaces around it are dropped, one sign is allowed and the
/// digits are ASCII; number 0 also reads properly grouped thousands (`1,234`), number 9 does not. Everything else is
/// rejected, an overflow too (types/lt-evaluate-format-lasterror-setview/P86Eval.al, codeunit "LT P86 Eval Integer":
/// `+1`, `007`, `-0`, `1 234`, `1.5`, `1e3`, `0x1F`, Arabic digits, `1-`, `--1`, `2147483648`).
fn integer(text: &str, number: i64) -> Result<Parsed, Signal> {
    if text.chars().any(|c| c.is_whitespace() && c != ' ') {
        return unsupported(format!(
            "Evaluate of `{text}` into Integer with white space other than spaces (not pinned)"
        ));
    }
    let t = text.trim_matches(' ');
    let (negative, digits) = match t.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, t.strip_prefix('+').unwrap_or(t)),
    };
    let ungrouped;
    let digits = if digits.contains(',') {
        let groups: Vec<&str> = digits.split(',').collect();
        let grouped = (1..=3).contains(&groups[0].len())
            && groups.iter().all(|g| g.bytes().all(|b| b.is_ascii_digit()))
            && groups[1..].iter().all(|g| g.len() == 3);
        if number != 0 {
            return Ok(Parsed::Rejected);
        }
        if !grouped {
            return unsupported(format!("Evaluate of `{text}` into Integer (grouping not pinned)"));
        }
        ungrouped = groups.concat();
        ungrouped.as_str()
    } else {
        digits
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(Parsed::Rejected);
    }
    let magnitude = digits.parse::<i64>().ok();
    Ok(
        match magnitude
            .map(|m| if negative { -m } else { m })
            .and_then(|n| i32::try_from(n).ok())
        {
            Some(n) => Parsed::Value(Value::Integer(n)),
            None => Parsed::Rejected,
        },
    )
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
    // More than 18 significant digits do not evaluate, and a value below 1E-28 reads as zero (types/TestProbeStreamContracts.al:
    // WriteDecimal_ManyDigits, `19_digits_text`, `1e_minus_29`).
    let digits: String = format!("{int}{}", frac.unwrap_or(""))
        .trim_start_matches('0')
        .to_string();
    // An integer part of 18 digits does not fit either (`123456789012345678` fails, `123456789012.345678` reads).
    if digits.len() > 18 || int.trim_start_matches('0').len() >= 18 {
        return Ok(Parsed::Rejected);
    }
    match text.parse::<Decimal>() {
        Ok(d) => Ok(Parsed::Value(Value::Decimal(d))),
        Err(_) if digits.is_empty() || frac.is_some_and(|f| f.len() > 28) => {
            Ok(Parsed::Value(Value::Decimal(Decimal::ZERO)))
        }
        Err(_) => unsupported(format!("Evaluate of `{text}` outside the Decimal range")),
    }
}

fn boolean(text: &str) -> Result<Parsed, Signal> {
    // text/TestEvaluate.al: 'true' / 'false'; types/TestFormatBooleanContracts.al:112
    // evaluates Format(true), which is 'Yes' (Format(false) is 'No'). In any case, `1` and `T` are
    // true and anything else, `Ja` for one, is refused
    // (types/TestFormatEvaluateTextContracts.al:Evaluate_Boolean_AcceptsTrueFalseYesNoOneAndT_IgnoringCase).
    // types/TestProbeScalarContracts.al:Evaluate_BooleanTexts: `0 F f 1 T t true TRUE false Yes no Y N y n` read, with
    // surrounding blanks; `2`, the empty text, `on` and `Ja` do not.
    match text.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "1" | "t" | "y" => Ok(Parsed::Value(Value::Boolean(true))),
        "false" | "no" | "0" | "f" | "n" => Ok(Parsed::Value(Value::Boolean(false))),
        // `1.0` reads as true (types/lt-boolean-format/Boolean_EvaluateFromText `1.0 = ok/1`); `0.0` and other numbers are not pinned.
        t if t
            .strip_prefix("1.")
            .is_some_and(|r| !r.is_empty() && r.bytes().all(|c| c == b'0')) =>
        {
            Ok(Parsed::Value(Value::Boolean(true)))
        }
        _ => Ok(Parsed::Rejected),
    }
}

fn guid(text: &str) -> Result<Parsed, Signal> {
    // Blanks around the text do not count (types/lt-scalars/ScT70054.al:Guid_EvaluateInvalid `spaces`).
    let text = text.trim_matches(' ');
    // The braced, the plain (36 characters) and the 32 digit forms; anything else, the empty text
    // included, is rejected (types/TestJsonTokenUnassignedEvaluateGuidContracts.al:EvaluateGuid_Forms_AndFailures).
    // Parentheses wrap it like braces (types/TestProbeScalarContracts.al:Guid_EvaluateAndFormat).
    let inner = text
        .strip_prefix('{')
        .and_then(|t| t.strip_suffix('}'))
        .or_else(|| text.strip_prefix('(').and_then(|t| t.strip_suffix(')')))
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
/// `today` is the host's current day, for the text `T`.
///
/// Pinned by types/lt-evaluate-format-lasterror-setview/P86Eval.al, codeunit "LT P86 Eval Date": number 0 reads
/// `yyyy-mm-dd`, `m/d/yyyy`, `m/d/yy` and `T`; it rejects `15.01.2024`, `150124`, `15012024`, `20240115`,
/// `2024-01-15T00:00:00Z`, `0D` and `15-01-2024`. Number 9 reads `yyyy-mm-dd` with spaces around it and the empty text
/// (which leaves the date empty) and rejects everything else, `T` and the slash forms included.
fn date(text: &str, number: i64, today: Option<i32>) -> Result<Parsed, Signal> {
    if number == 9 {
        let t = text.trim_matches(' ');
        if t.is_empty() {
            return Ok(Parsed::Value(Value::Date(0, false)));
        }
        // The first day of the calendar reads as the empty date, and year 0 as the day after it
        // (types/lt-datetime/Evaluate.al:Date_Format9 `iso min`, `iso 0D` and `iso year 0`).
        if t == "0001-01-01" {
            return Ok(Parsed::Value(Value::Date(0, false)));
        }
        if t == "0000-01-01" {
            return Ok(Parsed::Value(Value::Date(2, false)));
        }
        return Ok(
            match crate::iso::parse_date(t).filter(|_| t.len() == 10 && t.as_bytes()[4] == b'-') {
                Some(d) => Parsed::Value(Value::Date(d, false)),
                None => Parsed::Rejected,
            },
        );
    }
    // The empty text leaves the date empty in format 0 as in format 9 (types/lt-datetime/Evaluate.al:Date_Default_Evaluate,
    // oracle: Yes and an empty date).
    if text.is_empty() {
        return Ok(Parsed::Value(Value::Date(0, false)));
    }
    if text.eq_ignore_ascii_case("t") {
        return match today {
            Some(d) => Ok(Parsed::Value(Value::Date(d, false))),
            None => unsupported("Evaluate of `T` into a Date without a clock"),
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
    let digits = |s: &str, lens: std::ops::RangeInclusive<usize>| {
        lens.contains(&s.len()) && s.bytes().all(|b| b.is_ascii_digit())
    };
    if parts.len() == 3
        && digits(parts[0], 1..=2)
        && digits(parts[1], 1..=2)
        && (parts[2].len() == 2 || parts[2].len() == 4)
        && digits(parts[2], 2..=4)
    {
        let (m, d, yy): (u32, u32, i32) = (
            parts[0].parse().unwrap(),
            parts[1].parse().unwrap(),
            parts[2].parse().unwrap(),
        );
        // types/TestFormatEvaluateTextContracts.al:Evaluate_Date_TwoDigitYear_PivotsAt50.
        let y = if parts[2].len() == 4 {
            yy
        } else if yy < 50 {
            2000 + yy
        } else {
            1900 + yy
        };
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
    let all_digits = text.bytes().all(|b| b.is_ascii_digit());
    let dotted = text.split('.').count() == 3 && text.split('.').all(|p| digits(p, 1..=4));
    let dashed = text.split('-').count() == 3 && text.split('-').all(|p| digits(p, 1..=4));
    let iso_with_time = text.len() > 10 && text.as_bytes()[4] == b'-' && crate::iso::parse_date(&text[..10]).is_some();
    if (all_digits && matches!(text.len(), 6 | 8))
        || dotted
        || dashed
        || iso_with_time
        || text.eq_ignore_ascii_case("0d")
    {
        return Ok(Parsed::Rejected);
    }
    unsupported(format!(
        "Evaluate of `{text}` into Date (form not pinned by the corpus)"
    ))
}

/// A time of day, as a `Value::Time` holds it (milliseconds since midnight, plus one).
///
/// Pinned by types/lt-evaluate-format-lasterror-setview/P86Eval.al, codeunit "LT P86 Eval Time". Number 0 reads
/// `h:m[:s[.f]]` with one or two digits per part, `h.m.s`, `hhmm`, and `h:m[:s] AM|PM`; it rejects hours from 24, minutes
/// or seconds from 60, a minus sign, a trailing `Z` and `0T`. Number 9 reads exactly `hh:mm:ss[.f]` with an optional `Z`
/// and rejects everything else. Spaces around the text are dropped and the empty text leaves the time empty in both.
fn time(text: &str, number: i64) -> Result<Parsed, Signal> {
    let t = text.trim_matches(' ');
    if t.is_empty() {
        return Ok(Parsed::Value(Value::Time(0)));
    }
    let value = |h: i64, m: i64, s: i64, f: i64| Parsed::Value(Value::Time(((h * 60 + m) * 60 + s) * 1000 + f + 1));
    let num = |s: &str, max_len: usize| -> Option<i64> {
        (!s.is_empty() && s.len() <= max_len && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    // `.f`: one to three digits, as tenths, hundredths or milliseconds.
    let fraction = |s: &str| -> Option<i64> {
        let n = num(s, 3)?;
        Some(n * 10_i64.pow(3 - s.len() as u32))
    };
    let in_range = |h: i64, m: i64, s: i64| h < 24 && m < 60 && s < 60;
    if number == 9 {
        let body = t.strip_suffix('Z').unwrap_or(t);
        let (clock, frac) = match body.split_once('.') {
            Some((c, f)) => (c, Some(f)),
            None => (body, None),
        };
        let parts: Vec<&str> = clock.split(':').collect();
        let strict = parts.len() == 3 && parts.iter().all(|p| p.len() == 2);
        let f = match frac {
            Some(f) => fraction(f),
            None => Some(0),
        };
        return Ok(
            match (
                strict,
                f,
                parts.iter().map(|p| num(p, 2)).collect::<Vec<_>>()[..].to_vec(),
            ) {
                (true, Some(f), nums) if nums.iter().all(Option::is_some) => {
                    let (h, m, s) = (nums[0].unwrap_or(0), nums[1].unwrap_or(0), nums[2].unwrap_or(0));
                    if in_range(h, m, s) {
                        value(h, m, s, f)
                    } else {
                        Parsed::Rejected
                    }
                }
                _ => Parsed::Rejected,
            },
        );
    }
    if t.starts_with('-') || t.ends_with(['Z', 'z']) || t.eq_ignore_ascii_case("0t") {
        return Ok(Parsed::Rejected);
    }
    if !t.bytes().any(|b| b.is_ascii_digit()) {
        return Ok(Parsed::Rejected);
    }
    let (body, meridiem) = match (t.strip_suffix(" AM"), t.strip_suffix(" PM")) {
        (Some(b), _) => (b, Some(false)),
        (_, Some(b)) => (b, Some(true)),
        _ => (t, None),
    };
    let nums: Option<(i64, i64, i64, i64)> = if body.contains(':') {
        let (clock, frac) = match body.split_once('.') {
            Some((c, f)) => (c, Some(f)),
            None => (body, None),
        };
        let parts: Vec<&str> = clock.split(':').collect();
        match (parts.len(), frac) {
            (2, None) => num(parts[0], 2).zip(num(parts[1], 2)).map(|(h, m)| (h, m, 0, 0)),
            (3, f) => {
                let f = match f {
                    Some(f) => fraction(f),
                    None => Some(0),
                };
                num(parts[0], 2)
                    .zip(num(parts[1], 2))
                    .zip(num(parts[2], 2))
                    .zip(f)
                    .map(|(((h, m), s), f)| (h, m, s, f))
            }
            _ => None,
        }
    } else if body.contains('.') {
        let parts: Vec<&str> = body.split('.').collect();
        match parts[..] {
            [h, m, s] => num(h, 2).zip(num(m, 2)).zip(num(s, 2)).map(|((h, m), s)| (h, m, s, 0)),
            _ => None,
        }
    } else if body.len() == 4 {
        num(&body[..2], 2).zip(num(&body[2..], 2)).map(|(h, m)| (h, m, 0, 0))
    } else {
        None
    };
    let Some((mut h, m, s, f)) = nums else {
        return unsupported(format!(
            "Evaluate of `{text}` into Time (form not pinned by the corpus)"
        ));
    };
    if let Some(pm) = meridiem {
        if !(1..=12).contains(&h) {
            return unsupported(format!(
                "Evaluate of `{text}` into Time (hour with AM or PM not pinned)"
            ));
        }
        h = h % 12 + if pm { 12 } else { 0 };
    }
    Ok(if in_range(h, m, s) {
        value(h, m, s, f)
    } else {
        Parsed::Rejected
    })
}

/// A date and time in UTC (`Value::DateTime`).
///
/// Pinned by types/lt-evaluate-format-lasterror-setview/P86Eval.al, codeunit "LT P86 Eval DateTime". Number 9 reads
/// `yyyy-mm-dd[Thh:mm:ss[.f][Z|+hh:mm]]` (no zone means UTC) and the empty text, and rejects everything else. Number 0
/// reads the same with `Z` or an offset, the empty text, and `m/d/y h:mm[:ss] [AM|PM]`; it rejects a time past 23:59:59,
/// a day that does not exist and `0D`, `0T`, `0DT`. Not read: the ISO form without a zone in number 0 (BC raises
/// `The date is not valid.`) and `10:20`, which BC completes with the current year.
fn datetime(text: &str, number: i64) -> Result<Parsed, Signal> {
    let t = text.trim_matches(' ');
    if t.is_empty() {
        return Ok(Parsed::Value(Value::DateTime(0)));
    }
    let iso = t.len() >= 10 && t.as_bytes()[4] == b'-';
    if number == 9 {
        if t.len() == 10 && t.as_bytes()[4] == b'-' {
            return Ok(match crate::iso::parse_date(t) {
                Some(d) => Parsed::Value(Value::DateTime((i64::from(d) - 1) * 86_400_000 + 1)),
                None => Parsed::Rejected,
            });
        }
        return Ok(
            match crate::iso::parse_datetime(t).filter(|_| iso && t.as_bytes().get(10) == Some(&b'T')) {
                Some(v) => Parsed::Value(Value::DateTime(v)),
                None => Parsed::Rejected,
            },
        );
    }
    if ["0d", "0t", "0dt"].iter().any(|k| t.eq_ignore_ascii_case(k)) || !t.bytes().any(|b| b.is_ascii_digit()) {
        return Ok(Parsed::Rejected);
    }
    if iso && t.as_bytes().get(10) == Some(&b'T') {
        if !(t.ends_with(['Z', 'z']) || t[11..].contains(['+', '-'])) {
            return unsupported(format!(
                "Evaluate of `{text}` into DateTime without a zone (BC raises another error)"
            ));
        }
        return Ok(match crate::iso::parse_datetime(t) {
            Some(v) => Parsed::Value(Value::DateTime(v)),
            None => Parsed::Rejected,
        });
    }
    if let Some((d, clock)) = t.split_once(' ') {
        if d.contains('/') {
            return Ok(match (date(d, 0, None)?, time(clock, 0)?) {
                (Parsed::Value(Value::Date(day, _)), Parsed::Value(Value::Time(ms))) => {
                    Parsed::Value(Value::DateTime((i64::from(day) - 1) * 86_400_000 + ms))
                }
                _ => Parsed::Rejected,
            });
        }
    }
    unsupported(format!(
        "Evaluate of `{text}` into DateTime with format {number} (form not pinned)"
    ))
}

/// The value of a Date or a Guid in the form `Evaluate` reads it (and `Format` writes it), for a reader that has no
/// variable to evaluate into: `None` for any other type, a text that is no such value, or a form not pinned.
pub fn read_text(ty: &crate::value::RtType, text: &str) -> Option<Value> {
    use crate::value::RtType as T;
    // `Format(ClosingDate(D))` is `C` and the date (corpus types/lt-datetime/DateFuncs.al:47 `C12/31/23`,
    // misc/lt-system-functions/RandomFormat.al:432): the closing date of that day.
    if let (T::Date, Some(rest)) = (ty, text.strip_prefix('C')) {
        return match date(rest, 0, None).ok()? {
            Parsed::Value(Value::Date(d, false)) => Some(Value::Date(d, true)),
            _ => None,
        };
    }
    let parsed = match ty {
        T::Date => date(text, 0, None).ok()?,
        T::Guid => guid(text).ok()?,
        _ => return None,
    };
    match parsed {
        Parsed::Value(v) => Some(v),
        Parsed::Rejected => None,
    }
}

pub fn evaluate(c: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Result<Value, Signal> {
    evaluate_with(c, a, false)
}

/// `Evaluate` into an Option variable, which has no OptionCaption to name a member by.
pub fn evaluate_variable_option(c: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Result<Value, Signal> {
    evaluate_with(c, a, true)
}

fn evaluate_with(c: &mut dyn Ctx, a: &mut [Value], variable_option: bool) -> Result<Value, Signal> {
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
        Value::Integer(_) => integer(&text, number)?,
        Value::BigInteger(_) => big_integer(&text)?,
        Value::Decimal(_) => decimal(&text)?,
        Value::Boolean(_) => boolean(&text)?,
        Value::Guid(_) => guid(&text)?,
        Value::Date(..) => date(&text, number, c.today_day())?,
        Value::Time(_) => time(&text, number)?,
        Value::DateTime(_) => datetime(&text, number)?,
        // The text is kept as written once it is a form the runtime can calculate with
        // (types/TestDateFormulaContracts.al:21-139); what Format prints is docs/probes-needed.md.
        // `Evaluate(V, '1.2.3.4')`: the four-part form (session/TestMiscTypes.al:44-80).
        Value::Library(crate::library_kind::LibraryKind::Version, cell) => {
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
        // An enum reads an ordinal (also one no member has), a member name in any case or a caption, with
        // format 0 and with format 9 (types/ProbeTypeFormsAndFilters.al:Enum_Evaluate_Forms, BC 27.5; the
        // InitValue of a field reads a name the same way, types/TestBlankEnumInitTests.Codeunit.al:92). A
        // text that is none of these is rejected.
        Value::Enum(id, _) => {
            let Some(members) = c.enum_members(*id) else {
                return unsupported("Evaluate into an Enum without its members");
            };
            // White space around the text is ignored (types/W7EvaluateTests.al:EvaluateIntoAnEnum_DefaultFormat,
            // ` Second ` gives Second).
            let original = text.clone();
            let text = text.trim();
            let digits = text.strip_prefix('+').unwrap_or(text);
            let ordinal = (!digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
                .then(|| digits.parse::<i32>().ok())
                .flatten();
            // A sign is allowed before the digits and an ordinal no member has is kept, with format 9 too
            // (P86Eval.al, codeunit "LT P86 Eval Enum": `+5`, `00`, `11`).
            let captions = c.enum_captions(*id);
            let found = ordinal
                .or_else(|| {
                    members
                        .iter()
                        // A blank member matches the blank text (types/TestBlankEnumInitTests.Codeunit.al:
                        // EvaluateMatchesBlankMemberAndRejectsNonMember).
                        .find(|(_, n)| n.trim().eq_ignore_ascii_case(text))
                        .map(|(o, _)| *o)
                })
                .or_else(|| {
                    captions
                        .iter()
                        .find(|(_, n)| !text.is_empty() && n.eq_ignore_ascii_case(text))
                        .map(|(o, _)| *o)
                });
            match found {
                Some(o) => Parsed::Value(Value::Enum(*id, o)),
                None if c.ret_used() => Parsed::Rejected,
                // The error names every member by its caption, in ordinal order, and quotes the text as given.
                None => {
                    let options: Vec<String> = members
                        .iter()
                        .map(|(o, n)| {
                            captions
                                .iter()
                                .find(|(co, _)| co == o)
                                .map_or(n.clone(), |(_, cap)| cap.clone())
                        })
                        .collect();
                    return crate::native::raise(
                        format!(
                            "'{original}' is not an option. The existing options are: {}",
                            options.join(",")
                        ),
                        "InvalidOptionString",
                    );
                }
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
        // ClientType has no OptionCaption: a number below 16 or a member name evaluates, any other text answers false
        // (types/PWMisc.al:ClientType_OrdinalsAndNames: 16 to 30 are rejected).
        Value::Option(_, members) if crate::sysenum::is_client_type(members) => {
            let t = text.trim();
            let by_name = members.iter().position(|m| m.eq_ignore_ascii_case(t));
            match (t.parse::<usize>().ok().filter(|n| *n < members.len()), by_name) {
                (Some(n), _) | (None, Some(n)) => Parsed::Value(Value::Option(n as i32, members.clone())),
                _ => Parsed::Rejected,
            }
        }
        // An Option reads an ordinal or a member name (types/PROptionInteger.al:Option_InCaseAndLoopBounds: `1` is B, `C` is C).
        // A variable: an ordinal of any size is kept (`3` of three members formats as `3`), a text that is neither an ordinal
        // nor a member name is rejected, and the error names the members (P86Eval.al, codeunit "LT P86 Eval Option").
        Value::Option(_, members) if variable_option => {
            let t = text.trim();
            // An empty text names no member, not even one declared as "" (types/lt-scalars/ScT70062.al:Option_EvaluateByName `empty`).
            let by_name = members.iter().position(|m| !t.is_empty() && m.eq_ignore_ascii_case(t));
            let ordinal = if t.starts_with(|ch: char| ch.is_ascii_digit() || ch == '+') {
                t.parse::<u64>().ok()
            } else {
                None
            };
            match (ordinal, by_name) {
                (Some(n), _) => match i32::try_from(n) {
                    Ok(n) => Parsed::Value(Value::Option(n, members.clone())),
                    Err(_) => {
                        return unsupported(format!("Evaluate of the ordinal `{text}` into an Option (not pinned)"));
                    }
                },
                (None, Some(n)) => Parsed::Value(Value::Option(n as i32, members.clone())),
                _ if c.ret_used() => return Ok(Value::Boolean(false)),
                _ => {
                    return crate::native::raise(
                        format!(
                            "'{text}' is not an option. The existing options are: {}",
                            members.join(",")
                        ),
                        "InvalidOptionString",
                    );
                }
            }
        }
        Value::Option(_, members) => {
            let t = text.trim();
            let by_name = members.iter().position(|m| m.eq_ignore_ascii_case(t));
            match (t.parse::<usize>().ok().filter(|n| *n < members.len()), by_name) {
                (Some(n), _) | (None, Some(n)) => Parsed::Value(Value::Option(n as i32, members.clone())),
                // An OptionCaption may name it (fieldref/TestPrecompiledFieldOptionCaption.al:Evaluate_Caption_SelectsThatMember);
                // the value does not carry the captions.
                _ => {
                    return unsupported(format!(
                        "Evaluate of `{text}` into an Option: neither an ordinal nor a member name (an OptionCaption could name it, not pinned)"
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
        // The error of a failed statement form (session/TestProbeSessionMiscContracts.al:Arithmetic_ErrorTexts,
        // `evaluate_int_overflow_statement`; types/TestProbeScalarContracts.al:Evaluate_BooleanFailureError).
        Parsed::Rejected
            if matches!(
                a[0],
                Value::Integer(_)
                    | Value::BigInteger(_)
                    | Value::Decimal(_)
                    | Value::Boolean(_)
                    | Value::Time(_)
                    | Value::DateTime(_)
            ) =>
        {
            let ty = match a[0] {
                Value::Integer(_) => "Integer",
                Value::BigInteger(_) => "BigInteger",
                Value::Decimal(_) => "Decimal",
                Value::Time(_) => "Time",
                Value::DateTime(_) => "DateTime",
                _ => "Boolean",
            };
            crate::native::raise(
                format!("The value \"{text}\" can't be evaluated into type {ty}."),
                "Evaluate",
            )
        }
        // The error of a rejected date that looks like one (P86Eval.al:Dflt3_Error_FNone, Fmt03_Error_F0, Fmt93_Error_F9: the same
        // text as a date filter gives, SetFilter on a Date).
        Parsed::Rejected
            if matches!(a[0], Value::Date(..))
                && text
                    .bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'-' | b'.' | b'/')) =>
        {
            crate::native::raise(
                format!(
                    "The format of the Date '{text}' does not match your device's date settings.\\\\Make sure that the day, month, and year are in the correct order and use the correct separator (such as - or /). Alternatively, you can always use the format 'yyyy-mm-dd'."
                ),
                "Evaluate",
            )
        }
        Parsed::Rejected if matches!(a[0], Value::Guid(_)) => crate::native::raise(
            "Invalid format of GUID string. The correct format of the GUID string is: CDEF7890-ABCD-0123-1234-567890ABCDEF where 0-9, A-F symbolizes hexadecimal digits.",
            "InvalidGuidFormat",
        ),
        Parsed::Rejected => unsupported(format!(
            "Evaluate of `{text}` failed with the result unused (error text not pinned yet)"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(p: Result<Parsed, Signal>) -> Option<Value> {
        match p {
            Ok(Parsed::Value(v)) => Some(v),
            _ => None,
        }
    }

    // types/lt-evaluate-format-lasterror-setview/P86Eval.al, codeunit "LT P86 Eval Integer"
    #[test]
    fn integers_read_spaces_signs_and_grouping_by_format_number() {
        let int = |s: &str, n: i64| value(integer(s, n)).map(|v| matches!(v, Value::Integer(_)));
        assert!(matches!(value(integer("  +12 ", 0)), Some(Value::Integer(12))));
        assert!(matches!(value(integer("-0", 9)), Some(Value::Integer(0))));
        assert!(matches!(value(integer("1,234", 0)), Some(Value::Integer(1234))));
        assert_eq!(int("1,234", 9), None);
        assert_eq!(int("2147483648", 9), None);
        assert_eq!(int("1.5", 0), None);
        assert_eq!(int("1 234", 0), None);
    }

    // codeunit "LT P86 Eval Time"
    #[test]
    fn times_read_the_forms_of_each_format_number() {
        let ms = |s: &str, n: i64| match value(time(s, n)) {
            Some(Value::Time(t)) => Some(t - 1),
            _ => None,
        };
        assert_eq!(ms("12:00 PM", 0), Some(43_200_000));
        assert_eq!(ms("12:00:00 AM", 0), Some(0));
        assert_eq!(ms("1200", 0), Some(43_200_000));
        assert_eq!(ms("12.00.00", 0), Some(43_200_000));
        assert_eq!(ms("12:00:00.5", 9), Some(43_200_500));
        assert_eq!(ms("12:00", 9), None);
        assert_eq!(ms("24:00:00", 0), None);
        assert_eq!(ms("12:00:60", 0), None);
    }

    // types/lt-datetime/Evaluate.al:Date_Default_Evaluate: the empty text reads as the empty date in format 0 too.
    #[test]
    fn the_empty_text_reads_as_the_empty_date_in_format_0() {
        assert!(matches!(value(date("", 0, Some(5))), Some(Value::Date(0, _))));
    }

    // codeunit "LT P86 Eval Date"
    #[test]
    fn dates_read_slash_forms_in_format_0_and_only_iso_in_format_9() {
        let day = |s: &str, n: i64| match value(date(s, n, Some(5))) {
            Some(Value::Date(d, _)) => Some(d),
            _ => None,
        };
        assert_eq!(day("1/15/24", 0), day("2024-01-15", 0));
        assert_eq!(day("01/15/2024", 0), day("2024-01-15", 9));
        assert_eq!(day("T", 0), Some(5));
        assert_eq!(day("T", 9), None);
        assert_eq!(day("01/15/2024", 9), None);
        assert_eq!(day("20240115", 0), None);
    }
}
