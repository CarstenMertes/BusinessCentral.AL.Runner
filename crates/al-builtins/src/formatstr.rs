//! `Format(Value, Length, FormatString)`: format strings with `<...>` codes.
//!
//! The format-string language is large. Only the codes a corpus test uses are understood, with the
//! widths they are used with: `<Standard Format,N>` (types/TestFormatBooleanContracts.al:36,44),
//! `<Integer>`, `<Integer,N>`, `<Integer Thousand>`, `<Decimals,N>`, `<Sign>`, `<Sign,N>`,
//! `<Filler Character,C>` and `<Precision,A:B>` on numbers, and `<Year>`, `<Year4>`, `<Month>`,
//! `<Month,2>`, `<Month Text>`, `<Day>`, `<Day,2>`, `<Weekday Text>`, `<Closing>`, `<Hours24,2>`,
//! `<Hours12,2>`, `<Minutes,2>`, `<Seconds,2>` and `<AM/PM>` on dates and times
//! (types/TestDateRoundFormatStringContracts.al:Format_PictureCodes; handlers/TestPageTemporalSetValue_Page.al:47-65).
//! Any other code, or one of these on another type or in another width, is refused.

use crate::describe::Describe;
use crate::format::{FormatCtx, civil_from_day, format_value};
use crate::value::Value;
use rust_decimal::{Decimal, RoundingStrategy};

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

/// Marks an error string of [`format_with_string`] as the platform's "invalid field or attribute" error (the caller
/// raises it; text/TestProbeTextDateFormatContracts.al:Format_FormatStrings_Number, Format_OptionFieldAndOptionVariable).
pub const INVALID_FORMAT_MARK: &str = "\u{1}invalid-format:";

/// What the codes that come after a number's digits need to know from the whole string.
struct Numeric {
    value: Decimal,
    /// The character `<Integer,N>` pads with (`<Filler Character,C>` anywhere in the string).
    filler: char,
    /// `<Precision,A:B>`: at least A and at most B decimals.
    precision: Option<(u32, u32)>,
}

/// `Err` names the part of the format string that is not understood.
pub fn format_with_string(v: &Value, length: i32, template: &str, ctx: &dyn FormatCtx) -> Result<String, String> {
    let mut pieces: Vec<(bool, &str)> = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find('<') {
        pieces.push((false, &rest[..open]));
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else {
            return Err(format!("format string `{template}`: `<` without `>`"));
        };
        pieces.push((true, &after[..close]));
        rest = &after[close + 1..];
    }
    pieces.push((false, rest));
    let numeric = numeric_of(v, &pieces).map_err(|e| {
        if e == INVALID_FORMAT_MARK {
            format!("{INVALID_FORMAT_MARK}{template}")
        } else {
            e
        }
    })?;
    let mut out = String::new();
    for (is_code, text) in pieces {
        if is_code {
            out.push_str(&code(v, text, numeric.as_ref(), ctx).map_err(|e| {
                if e == INVALID_FORMAT_MARK {
                    format!("{INVALID_FORMAT_MARK}{template}")
                } else {
                    format!("format string `{template}`: {e}")
                }
            })?);
        } else {
            out.push_str(text);
        }
    }
    Ok(crate::format::pad(out, length))
}

fn numeric_of(v: &Value, pieces: &[(bool, &str)]) -> Result<Option<Numeric>, String> {
    let value = match v {
        Value::Integer(i) => Decimal::from(*i),
        Value::BigInteger(i) => Decimal::from(*i),
        Value::Decimal(d) => *d,
        _ => return Ok(None),
    };
    let mut n = Numeric {
        value,
        filler: ' ',
        precision: None,
    };
    for (is_code, text) in pieces {
        if !is_code {
            continue;
        }
        // `Filler Character 0` (a blank instead of the comma) is no code (Format_FormatStrings_Number).
        if text.starts_with("Filler Character") && !text.starts_with("Filler Character,") {
            return Err(INVALID_FORMAT_MARK.to_string());
        }
        if let Some(c) = text.strip_prefix("Filler Character,") {
            let mut it = c.chars();
            let (Some(f), None) = (it.next(), it.next()) else {
                return Err(format!("code `<{text}>`"));
            };
            n.filler = f;
        } else if let Some(p) = text.strip_prefix("Precision,") {
            let parsed = p
                .split_once(':')
                .and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)))
                .filter(|(a, b)| a <= b && *b <= 18);
            n.precision = Some(parsed.ok_or_else(|| format!("code `<{text}>`"))?);
        }
    }
    Ok(Some(n))
}

fn width(code: &str, name: &str) -> Result<Option<usize>, String> {
    match code.strip_prefix(name).and_then(|r| r.strip_prefix(',')) {
        None => Ok(None),
        Some(w) => w.parse().map(Some).map_err(|_| format!("code `<{code}>`")),
    }
}

fn group(int: &str) -> String {
    let mut out = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn numeric_code(n: &Numeric, code: &str, v: &Value, ctx: &dyn FormatCtx) -> Result<String, String> {
    let abs = n.value.abs();
    let int = abs.trunc().to_string();
    let refuse = || Err(format!("code `<{code}>` on {}", v.describe()));
    if code == "Sign" || code.starts_with("Sign,") {
        let w = width(code, "Sign")?;
        let neg = n.value.is_sign_negative() && !n.value.is_zero();
        return match (neg, w) {
            (true, w) => Ok(format!("{:>1$}", "-", w.unwrap_or(1))),
            (false, Some(w)) => Ok(" ".repeat(w)),
            // A sign code without a width shows nothing for a number that is not negative.
            (false, None) => Ok(String::new()),
        };
    }
    if code == "Integer" || code.starts_with("Integer,") {
        let w = width(code, "Integer")?.unwrap_or(0);
        let pad: String = std::iter::repeat_n(n.filler, w.saturating_sub(int.len())).collect();
        return Ok(format!("{pad}{int}"));
    }
    if code == "Integer Thousand" || code.starts_with("Integer Thousand,") {
        let grouped = group(&int);
        return Ok(match width(code, "Integer Thousand")? {
            // A number that does not fit shows stars (`<Integer Thousand,3>` of 1234567 is `***`).
            Some(w) if grouped.len() > w => "*".repeat(w),
            Some(w) => format!("{grouped:>w$}"),
            None => grouped,
        });
    }
    if code == "Decimals" {
        // Without a width: the decimals the value has (`5.5` shows `.5`, a whole number nothing).
        let frac = abs.fract().normalize().to_string();
        return Ok(frac.strip_prefix('0').map(str::to_string).unwrap_or_default());
    }
    if let Some(w) = width(code, "Decimals")? {
        if w < 2 {
            return refuse();
        }
        let digits = (w - 1) as u32;
        // More decimals than the width holds show stars (`<Decimals,3>` of 1234.567 is `***`); fewer are padded with zeros.
        if abs.fract().normalize().scale() > digits {
            return Ok("*".repeat(w));
        }
        let frac = format!("{:.*}", digits as usize, abs.fract());
        return Ok(frac.trim_start_matches('0').to_string());
    }
    if code.starts_with("Filler Character,") || code.starts_with("Precision,") {
        return Ok(String::new());
    }
    if let Some(std) = code.strip_prefix("Standard Format,") {
        let std: i32 = std.trim().parse().map_err(|_| format!("code `<{code}>`"))?;
        if let Some((min, max)) = n.precision {
            if std != 0 {
                return refuse();
            }
            let rounded = n
                .value
                .round_dp_with_strategy(max, RoundingStrategy::MidpointAwayFromZero);
            let mut text = format_value(&Value::Decimal(rounded), 0, 0, ctx).map_err(fail)?;
            let have = text.split_once('.').map_or(0, |(_, f)| f.len() as u32);
            if have < min {
                if have == 0 {
                    text.push('.');
                }
                text.push_str(&"0".repeat((min - have) as usize));
            }
            return Ok(text);
        }
        return format_value(v, 0, std, ctx).map_err(fail);
    }
    refuse()
}

fn fail(f: crate::format::FormatFail) -> String {
    match f {
        crate::format::FormatFail::Unsupported(s) => s,
        crate::format::FormatFail::Error { message, .. } => message,
    }
}

fn code(v: &Value, code: &str, numeric: Option<&Numeric>, ctx: &dyn FormatCtx) -> Result<String, String> {
    if let Some(n) = numeric {
        return numeric_code(n, code, v, ctx);
    }
    // `<Integer>` is no code of an Option (Format_OptionFieldAndOptionVariable, `integer_cast_default`).
    if matches!(v, Value::Option(..)) && code.starts_with("Integer") {
        return Err(INVALID_FORMAT_MARK.to_string());
    }
    if let Some(n) = code.strip_prefix("Standard Format,") {
        return match n.trim().parse::<i32>() {
            Ok(n) => format_value(v, 0, n, ctx).map_err(fail),
            Err(_) => Err(format!("code `<{code}>`")),
        };
    }
    let (day_no, closing) = match v {
        Value::Date(n, c) if *n > 0 => (Some(*n), *c),
        Value::DateTime(t) if *t > 0 => (Some((t - 1).div_euclid(86_400_000) as i32 + 1), false),
        _ => (None, false),
    };
    let clock = match v {
        Value::Time(t) if *t > 0 => Some(t - 1),
        Value::DateTime(t) if *t > 0 => Some((t - 1).rem_euclid(86_400_000)),
        _ => None,
    };
    if code == "Closing" {
        return match day_no {
            Some(_) => Ok(if closing { "C".into() } else { String::new() }),
            None => Err(format!("code `<Closing>` on {}", v.describe())),
        };
    }
    // A closing date shows its day in the date codes ('5.3.2005'); only `<Closing>` shows the mark.
    let date = day_no.map(civil_from_day);
    let hour12 = |ms: i64| {
        if ms / 3_600_000 % 12 == 0 {
            12
        } else {
            ms / 3_600_000 % 12
        }
    };
    let part = match (code, date, clock) {
        ("Year4", Some((y, _, _)), _) => format!("{y:04}"),
        // A two-digit year below 10 would print as one digit or two: not measured.
        // Two digits for 1999 and for 2000 to 2030, four for 1905, 1929, 1930 and 2105
        // (text/TestProbeTextDateFormatContracts.al:Format_FormatStrings_Date); other years are not measured.
        ("Year", Some((y, _, _)), _) if y == 1999 || (2000..=2030).contains(&y) => format!("{:02}", y.rem_euclid(100)),
        ("Year", Some((y, _, _)), _) if matches!(y, 1905 | 1929 | 1930 | 2105) => format!("{y}"),
        ("Year,2", Some((y, _, _)), _) => format!("{:02}", y.rem_euclid(100)),
        ("Month", Some((_, m, _)), _) => format!("{m}"),
        ("Month,2", Some((_, m, _)), _) => format!("{m:02}"),
        ("Month Text", Some((_, m, _)), _) => MONTHS[m as usize - 1].to_string(),
        ("Day", Some((_, _, d)), _) => format!("{d}"),
        ("Day,2", Some((_, _, d)), _) => format!("{d:02}"),
        ("Weekday Text", Some(_), _) => WEEKDAYS[((day_no.unwrap_or(1) - 1).rem_euclid(7)) as usize].to_string(),
        ("Hours24,2", _, Some(ms)) => format!("{:02}", ms / 3_600_000),
        ("Hours24", _, Some(ms)) => format!("{}", ms / 3_600_000),
        // A minute below 10 without a width is not measured (Dates_CreateDateTimeRoundTrip shows 30).
        ("Minutes", _, Some(ms)) if ms / 60_000 % 60 >= 10 => format!("{}", ms / 60_000 % 60),
        // The twelve-hour clock pads the hour with a space (:Format_PictureCodes).
        ("Hours12,2", _, Some(ms)) => format!("{:>2}", hour12(ms)),
        ("Minutes,2", _, Some(ms)) => format!("{:02}", ms / 60_000 % 60),
        ("Seconds,2", _, Some(ms)) => format!("{:02}", ms / 1000 % 60),
        ("AM/PM", _, Some(ms)) => (if ms / 3_600_000 < 12 { "AM" } else { "PM" }).to_string(),
        _ => return Err(format!("code `<{code}>` on {}", v.describe())),
    };
    Ok(part)
}

#[cfg(test)]
mod tests {
    use crate::format::day_from_civil;
    use crate::native::Signal;
    use crate::native::tests::{call, t};
    use crate::value::Value;

    fn fmt(v: Value, template: &str) -> Result<Value, Signal> {
        call("System.Format#2", vec![v, Value::Integer(0), t(template)])
    }
    fn text(r: Result<Value, Signal>) -> String {
        match r {
            Ok(Value::Text(s)) => s.to_string(),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn standard_format_code() {
        // types/TestFormatBooleanContracts.al:36,44.
        assert_eq!(text(fmt(Value::Boolean(true), "<Standard Format,2>")), "1");
        assert_eq!(text(fmt(Value::Boolean(false), "<Standard Format,2>")), "0");
    }

    #[test]
    fn integer_code() {
        // json/TestJsonXmlDeepContracts.al:71.
        assert_eq!(text(fmt(Value::Integer(10), "<Integer>")), "10");
    }

    #[test]
    fn date_and_time_codes() {
        // handlers/TestPageTemporalSetValue_Page.al:47-65.
        let day = day_from_civil(2024, 3, 7);
        assert_eq!(
            text(fmt(Value::Date(day, false), "<Year4>-<Month,2>-<Day,2>")),
            "2024-03-07"
        );
        let dt = Value::DateTime((i64::from(day) - 1) * 86_400_000 + (14 * 3600 + 30 * 60 + 5) * 1000 + 1);
        assert_eq!(
            text(fmt(dt, "<Year4>-<Month,2>-<Day,2> <Hours24,2>:<Minutes,2>:<Seconds,2>")),
            "2024-03-07 14:30:05"
        );
        let time = Value::Time((14 * 3600 + 30 * 60) * 1000 + 1);
        assert_eq!(text(fmt(time, "<Hours24,2>:<Minutes,2>:<Seconds,2>")), "14:30:00");
    }

    #[test]
    fn integer_thousand_with_a_width_shows_stars_when_the_number_does_not_fit() {
        // text/TestProbeTextDateFormatContracts.al:Format_StringCodes, `integer_thousand_width_3` and `_10`.
        assert_eq!(text(fmt(Value::Integer(1234567), "<Integer Thousand,3>")), "***");
        assert_eq!(
            text(fmt(Value::Integer(1234567), "<Integer Thousand,10>")),
            " 1,234,567"
        );
    }

    #[test]
    fn other_codes_and_values_are_refused() {
        let day = Value::Date(day_from_civil(2024, 3, 7), false);
        for (v, f) in [
            (day.clone(), "<Hours24,2>"),
            (day.clone(), "<Day,3>"),
            (day.clone(), "<Year4"),
            (Value::Date(0, false), "<Year4>"),
            (Value::Integer(1), "<Standard Format,x>"),
        ] {
            assert!(matches!(fmt(v, f), Err(Signal::Unsupported(_))), "{f}");
        }
    }

    #[test]
    fn the_picture_codes_the_probe_pins() {
        // types/TestDateRoundFormatStringContracts.al:Format_PictureCodes.
        let day = Value::Date(day_from_civil(2024, 3, 5), false);
        assert_eq!(text(fmt(day.clone(), "<Day>/<Month>/<Year>")), "5/3/24");
        assert_eq!(text(fmt(day.clone(), "<Month Text> <Day> <Year4>")), "March 5 2024");
        assert_eq!(text(fmt(day.clone(), "<Weekday Text>")), "Tuesday");
        assert_eq!(text(fmt(day, "<Closing><Day,2>")), "05");
        let time = Value::Time(52_205_001);
        assert_eq!(text(fmt(time, "<Hours12,2>:<Minutes,2> <AM/PM>")), " 2:30 PM");
        let dec = |s: &str| Value::Decimal(s.parse().unwrap());
        assert_eq!(text(fmt(dec("1234.5"), "<Integer Thousand><Decimals,3>")), "1,234.50");
        assert_eq!(text(fmt(Value::Integer(5), "<Integer,4><Filler Character,0>")), "0005");
        assert_eq!(
            text(fmt(dec("1234.567"), "<Precision,2:2><Standard Format,0>")),
            "1,234.57"
        );
        assert_eq!(text(fmt(Value::Integer(-5), "<Sign><Integer>")), "-5");
        assert_eq!(text(fmt(Value::Integer(5), "<Sign,1><Integer>")), " 5");
        // text/TestProbeTextDateFormatContracts.al:Format_StringCodes, `sign_pos_dec`.
        assert_eq!(
            text(fmt(Value::Decimal("5.5".parse().unwrap()), "<Sign><Integer><Decimals>")),
            "5.5"
        );
    }

    #[test]
    fn length_pads_the_result() {
        let r = call(
            "System.Format#2",
            vec![Value::Integer(10), Value::Integer(5), t("<Integer>")],
        );
        assert_eq!(text(r), "   10");
    }
    // Mutation-testing additions (docs/status.md debt).

    #[test]
    fn the_last_millisecond_of_a_day_stays_in_that_day() {
        let day = day_from_civil(2024, 3, 7);
        let last = (i64::from(day) - 1) * 86_400_000 + 86_399_999 + 1;
        assert_eq!(
            text(fmt(
                Value::DateTime(last),
                "<Year4>-<Month,2>-<Day,2> <Hours24,2>:<Minutes,2>:<Seconds,2>"
            )),
            "2024-03-07 23:59:59"
        );
        // The first millisecond of the day is 00:00:00.
        let first = (i64::from(day) - 1) * 86_400_000 + 1;
        assert_eq!(
            text(fmt(
                Value::DateTime(first),
                "<Day,2> <Hours24,2>:<Minutes,2>:<Seconds,2>"
            )),
            "07 00:00:00"
        );
        assert_eq!(
            text(fmt(Value::Time(86_399_999 + 1), "<Hours24,2>:<Minutes,2>:<Seconds,2>")),
            "23:59:59"
        );
        assert_eq!(
            text(fmt(Value::Time(1), "<Hours24,2>:<Minutes,2>:<Seconds,2>")),
            "00:00:00"
        );
    }

    #[test]
    fn an_undefined_datetime_or_time_has_no_parts() {
        for (v, f) in [
            (Value::DateTime(0), "<Year4>"),
            (Value::DateTime(0), "<Hours24,2>"),
            (Value::Time(0), "<Hours24,2>"),
            (Value::Time(0), "<Seconds,2>"),
        ] {
            assert!(matches!(fmt(v, f), Err(Signal::Unsupported(_))), "{f}");
        }
    }
}
