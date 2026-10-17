//! Date, Time and DateTime builtins: `DMY2Date`, `CreateDateTime`, `Date2DMY`, `CalcDate`, ...
//!
//! Representations follow `Value`: a Date is a day number (1 = 0001-01-01) and a closing flag, a
//! Time is milliseconds since midnight plus one, a DateTime is milliseconds since 0001-01-01 UTC
//! plus one (0 is the undefined value of each). The session time zone is taken to be UTC, as in
//! `format.rs` (docs/probes-needed.md).
//!
//! Every function accepts the forms a corpus test pins and refuses the rest with
//! `Signal::Unsupported`, never a guess.

use crate::describe::Describe;
use crate::format::{civil_from_day, day_from_civil};
use crate::native::{Ctx, NativeFn, Signal, raise};
use crate::value::Value;

pub const REGISTRY: &[(&str, NativeFn)] = &[
    ("System.DMY2Date#1", dmy2date),
    ("System.CreateDateTime#1", create_datetime),
    ("System.DT2Date#1", dt2date),
    ("System.DT2Time#1", dt2time),
    ("DateTime.Date#1", datetime_date),
    ("DateTime.Time#1", datetime_time),
    ("System.DaTi2Variant#1", create_datetime),
    ("System.Date2DMY#1", date2dmy),
    ("System.Date2DWY#1", date2dwy),
    ("System.NormalDate#1", normal_date),
    ("System.ClosingDate#1", closing_date),
    ("System.RoundDateTime#1", round_datetime),
    ("System.CalcDate#1", calcdate_formula),
    ("System.CalcDate#2", calcdate_text),
    ("Date.DayOfWeek#1", day_of_week),
    ("Date.WeekNo#1", week_no),
    ("Time.Hour#1", |_, r, _| time_part(r, 3_600_000, 24)),
    ("Time.Minute#1", |_, r, _| time_part(r, 60_000, 60)),
    ("Time.Second#1", |_, r, _| time_part(r, 1000, 60)),
    ("Time.Millisecond#1", |_, r, _| time_part(r, 1, 1000)),
];

type Out = Result<Value, Signal>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

const MS_PER_DAY: i64 = 86_400_000;
const LAST_DAY: i32 = 3_652_059; // 9999-12-31

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

fn int_of(v: Option<&Value>) -> Option<i32> {
    match v? {
        Value::Integer(n) => Some(*n),
        _ => None,
    }
}

/// A defined, non-closing date's day number.
fn plain_date(v: &Value, what: &str) -> Result<i32, Signal> {
    match v {
        Value::Date(n, false) if *n > 0 => Ok(*n),
        other => unsupported(format!(
            "{what} of {other}: only a defined, non-closing Date is pinned",
            other = other.describe()
        )),
    }
}

/// `DMY2Date(Day, Month, Year)` with all three parts, which is every form the corpus calls
/// (fieldref/TestFieldRefValue.al:122, record/TestDateVirtualTableFlowField.al:65,83).
/// A part outside the calendar raises in BC with a text no corpus test pins: refused.
fn dmy2date(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    let (Some(d), Some(m), Some(y)) = (int_of(a.first()), int_of(a.get(1)), int_of(a.get(2))) else {
        return unsupported("DMY2Date without all of Day, Month and Year (not pinned by the corpus)");
    };
    if !(1..=9999).contains(&y) || !(1..=12).contains(&m) || d < 1 || d as u32 > days_in_month(y, m as u32) {
        return unsupported(format!(
            "DMY2Date({d}, {m}, {y}) is not a calendar date (error text not pinned)"
        ));
    }
    Ok(Value::Date(day_from_civil(y, m as u32, d as u32), false))
}

/// `CreateDateTime(Date, Time)`. A defined date with `0T` is midnight
/// (handlers/TestPageTemporalSetValue_Tests.al:85). Two blanks give the blank DateTime; a blank
/// date with a time is an error (types/TestDateRoundFormatStringContracts.al:CreateDateTime_MidnightBlankAndInvalid).
fn create_datetime(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    let Value::Time(t) = a[1] else {
        return unsupported(format!("CreateDateTime with {}", a[1].describe()));
    };
    if matches!(a[0], Value::Date(0, false)) {
        if t == 0 {
            return Ok(Value::DateTime(0));
        }
        return raise("The date is not valid.", "DateInvalid");
    }
    // A closing date is its day plus 23:59: with 12:00 PM the result reads 11:59 AM of the next day, with 0T 11:59 PM of the same
    // day (misc/lt-probes-needed-2/PnBDateFormat.al:CreateDateTime_ClosingDate; only the minutes are pinned, the seconds are
    // assumed to be 0, docs/probes-needed.md).
    let (day, closing) = match &a[0] {
        Value::Date(n, c) if *n > 0 => (*n, *c),
        other => return plain_date(other, "CreateDateTime").map(|_| unreachable!()),
    };
    // 0001-01-01 is no date a DateTime can be made of (types/lt-datetime/DateTimeFuncs.al:CreateDateTime_Variants `af32`,
    // RoundDateTime_Edges `af118`).
    if day == 1 {
        return raise("The date is not valid.", "DateInvalid");
    }
    let time_of_day = (t - 1).max(0) + if closing { 23 * 3_600_000 + 59 * 60_000 } else { 0 };
    Ok(Value::DateTime((i64::from(day) - 1) * MS_PER_DAY + time_of_day + 1))
}

/// `RoundDateTime(DateTime [, Precision [, Direction]])` (types/TestRoundDateTimeContracts.al): the
/// precision is in milliseconds (default 1000; a BigInteger, Integer or Duration), the direction `=`
/// (nearest, a tie rounds up; the default), `<` (down) or `>` (up). A precision of 0 or less and an
/// unknown direction raise Argument errors; the undefined DateTime stays undefined.
///
/// Where the multiples of a precision start is pinned only by one case: 12:00:31 on 2024-03-05 with 700000 ms
/// rounds to 12:00:00 and 12:11:40 (types/TestRoundDateTimeContracts.al:61-72, BC 27.5). Every origin at a
/// midnight that fits it is a Saturday (a day shifts the multiples by 300000 ms, a week by none), so the
/// candidates (1899-12-30, 2000-01-01, ...) differ by whole weeks. They give the same multiples exactly when a
/// week is a whole number of precisions; any other precision is refused instead of choosing an origin.
fn round_datetime(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    let invalid = |what: String, n: u32| {
        raise(
            format!("The contents '{what}' of ROUNDDATETIME parameter number {n} are invalid."),
            "Argument",
        )
    };
    let precision = match a.get(1) {
        None => 1000,
        Some(Value::Integer(n)) => i64::from(*n),
        Some(Value::BigInteger(n)) | Some(Value::Duration(n)) => *n,
        Some(other) => return unsupported(format!("RoundDateTime with a precision of {}", other.describe())),
    };
    if precision <= 0 {
        return invalid(precision.to_string(), 2);
    }
    let direction = match a.get(2) {
        None => "=".to_string(),
        Some(v) => match crate::native::text_of(v) {
            Some(t) => t,
            None => return unsupported("RoundDateTime with a direction that is not text"),
        }
        .to_string(),
    };
    if !matches!(direction.as_str(), "=" | "<" | ">") {
        return invalid(direction, 3);
    }
    let Value::DateTime(t) = a[0] else {
        return unsupported(format!("RoundDateTime of {}", a[0].describe()));
    };
    if t == 0 {
        return Ok(Value::DateTime(0));
    }
    if (7 * MS_PER_DAY) % precision != 0 {
        return unsupported(format!(
            "RoundDateTime with a precision of {precision} ms, which a week does not hold a whole number of: where its multiples start is not pinned (docs/probes-needed.md)"
        ));
    }
    // The days from 0001-01-01 to 1899-12-30, a Saturday: any other Saturday gives the same multiples here.
    let origin = 693_593 * MS_PER_DAY;
    let ms = t - 1;
    let below = ms - (ms - origin).rem_euclid(precision);
    let rounded = if below == ms {
        ms
    } else {
        match direction.as_str() {
            "<" => below,
            ">" => below + precision,
            _ if ms - below >= precision - (ms - below) => below + precision,
            _ => below,
        }
    };
    // A result past 9999-12-31 is not a DateTime (DateTimeFuncs.al:RoundDateTime_Edges `af126`, `af130`, RoundDateTime_Errors).
    if rounded >= 3_652_059 * MS_PER_DAY {
        return raise("The date is not valid.", "DateInvalid");
    }
    Ok(Value::DateTime(rounded + 1))
}

fn defined_datetime(v: &Value, what: &str) -> Result<i64, Signal> {
    match v {
        Value::DateTime(n) if *n > 0 => Ok(*n - 1),
        other => unsupported(format!(
            "{what} of {}: only a defined DateTime is pinned",
            other.describe()
        )),
    }
}

/// handlers/TestPageTemporalSetValue_Tests.al:118.
fn dt2date(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    // The undefined DateTime gives the undefined Date (text/TestProbeTextDateFormatContracts.al:Dates_CreateDateTimeRoundTrip).
    if matches!(a[0], Value::DateTime(0)) {
        return Ok(Value::Date(0, false));
    }
    let ms = defined_datetime(&a[0], "DT2Date")?;
    Ok(Value::Date(ms.div_euclid(MS_PER_DAY) as i32 + 1, false))
}

/// `DateTime.Date()` is `DT2Date` of the receiver (assumed, docs/probes-needed.md).
fn datetime_date(c: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    crate::assumptions::assume(crate::assumptions::Assumption::DateTimeDateTimeMethods);
    let Some(v) = r else {
        return unsupported("DateTime.Date without a receiver");
    };
    dt2date(c, None, &mut [v.clone()])
}

/// `DateTime.Time()` is `DT2Time` of the receiver (assumed, docs/probes-needed.md).
fn datetime_time(c: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    crate::assumptions::assume(crate::assumptions::Assumption::DateTimeDateTimeMethods);
    let Some(v) = r else {
        return unsupported("DateTime.Time without a receiver");
    };
    dt2time(c, None, &mut [v.clone()])
}

/// handlers/TestPageTemporalSetValue_Tests.al:117.
fn dt2time(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    if matches!(a[0], Value::DateTime(0)) {
        return Ok(Value::Time(0));
    }
    let ms = defined_datetime(&a[0], "DT2Time")?;
    Ok(Value::Time(ms.rem_euclid(MS_PER_DAY) + 1))
}

/// `Date2DMY(Date, 1|2|3)`: day, month, year (session/TestCalcDateNotificationContracts.al:21,22,32).
/// Any other part raises (types/TestDateRoundFormatStringContracts.al:Date2DMY_Part4_Throws, measured for 4).
fn date2dmy(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    let day = match &a[0] {
        // Only Date2DWY of the undefined date is pinned (text/TestProbeTextDateFormatContracts.al:Dates_Date2DWY_BadPart).
        Value::Date(0, _) => return unsupported("Date2DMY of an undefined date (not pinned)"),
        Value::Date(n, _) if *n > 0 => *n,
        other => return plain_date(other, "Date2DMY").map(|_| unreachable!()),
    };
    let (y, m, d) = civil_from_day(day);
    match int_of(a.get(1)) {
        Some(1) => Ok(Value::Integer(d as i32)),
        Some(2) => Ok(Value::Integer(m as i32)),
        Some(3) => Ok(Value::Integer(y)),
        Some(n) => raise(
            format!(
                "The value of DATE2DMY parameter 2 is outside of the permitted range. The current value is: {n}. The permitted range is: from 1 to 3."
            ),
            "OutsidePermittedRange",
        ),
        None => unsupported("Date2DMY with a part that is not an Integer"),
    }
}

/// `Date2DWY(Date, 1|2|3)`: the weekday (Monday is 1), the ISO 8601 week number
/// (types/TestDateRoundFormatStringContracts.al:Date2DWY_WeekNumbers_AreIso8601) and the year. The year is
/// the ISO week-year (text/TestProbeTextDateFormatContracts.al:Dates_Date2DWY_WeekAndYear).
fn date2dwy(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    // text/TestProbeTextDateFormatContracts.al:Dates_Date2DWY_BadPart: the undefined date raises, a closing date counts as
    // its day, a part outside 1 to 3 raises.
    let day = match &a[0] {
        Value::Date(0, _) => return raise("The date is not valid.", "DateInvalid"),
        Value::Date(n, _) if *n > 0 => *n,
        other => return plain_date(other, "Date2DWY").map(|_| unreachable!()),
    };
    let thursday = day + (4 - weekday(day));
    let (iso_year, _, _) = civil_from_day(thursday);
    match int_of(a.get(1)) {
        Some(1) => Ok(Value::Integer(weekday(day))),
        Some(2) => Ok(Value::Integer((thursday - day_from_civil(iso_year, 1, 1)) / 7 + 1)),
        // The ISO year of the week (text/TestProbeTextDateFormatContracts.al:Dates_Date2DWY_WeekAndYear: 2021-01-01 is in 2020).
        Some(3) => Ok(Value::Integer(iso_year)),
        Some(n) => raise(
            format!(
                "The value of DATE2DWY parameter 2 is outside of the permitted range. The current value is: {n}. The permitted range is: from 1 to 3."
            ),
            "OutsidePermittedRange",
        ),
        None => unsupported("Date2DWY with a part that is not an Integer"),
    }
}

/// record/TestDateVirtualTable.al:188-190: a closing date gives its plain day, a plain date is unchanged.
fn normal_date(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    match &a[0] {
        Value::Date(n, _) if *n > 0 => Ok(Value::Date(*n, false)),
        other => unsupported(format!("NormalDate of {}", other.describe())),
    }
}

/// record/TestFilterContracts.al:56,58.
fn closing_date(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    match &a[0] {
        Value::Date(n, _) if *n > 0 => Ok(Value::Date(*n, true)),
        // streams/TestProbeStreamContracts.al:WriteDate_ClosingDates, `closing_0D`.
        Value::Date(0, _) => raise("The date is not valid.".to_string(), "DateInvalid"),
        other => unsupported(format!("ClosingDate of {}", other.describe())),
    }
}

/// 1 = Monday (session/TestSystemExtended.al:278). Day 1 (0001-01-01) is a Monday.
fn weekday(day: i32) -> i32 {
    (day - 1).rem_euclid(7) + 1
}

fn receiver_date(r: Option<&mut Value>, what: &str) -> Result<i32, Signal> {
    match r {
        Some(v) => plain_date(v, what),
        None => unsupported(format!("{what} without a receiver")),
    }
}

fn day_of_week(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    Ok(Value::Integer(weekday(receiver_date(r, "Date.DayOfWeek")?)))
}

/// The ISO 8601 week: the week that holds the year's first Thursday is week 1. The corpus pins
/// the range and the first week of a year (session/TestSystemExtended.al:285-300); the rule
/// itself is the documented one (docs/probes-needed.md).
fn week_no(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    let day = receiver_date(r, "Date.WeekNo")?;
    let thursday = day + (4 - weekday(day));
    let (y, _, _) = civil_from_day(thursday);
    Ok(Value::Integer((thursday - day_from_civil(y, 1, 1)) / 7 + 1))
}

/// `Time.Hour/Minute/Second/Millisecond` (session/TestMiscTypes.al:22-42). The undefined time is not pinned.
fn time_part(r: Option<&mut Value>, unit_ms: i64, modulo: i64) -> Out {
    match r {
        Some(Value::Time(t)) if *t > 0 => Ok(Value::Integer((((*t - 1) / unit_ms) % modulo) as i32)),
        other => unsupported(format!(
            "a Time part of {}: only a defined Time is pinned",
            other.describe()
        )),
    }
}

// ---- CalcDate ------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Day,
    Week,
    Month,
    Quarter,
    Year,
}

/// One term of a date formula.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Term {
    /// `+nD`, `-nW`, `nM`, `nQ`, `nY`: move by `n` units.
    Step(Unit, i32),
    /// `CD`, `CW`, `CM`, `CQ`, `CY`: the end of the current period; `-C..`: its start. A week runs
    /// from Monday to Sunday.
    Edge { unit: Unit, end: bool },
    /// `WDn`: the next day (`-WDn`: the previous one) that is weekday `n`, Monday being 1.
    WeekDay { n: i32, forward: bool },
}

/// A formula as stored or typed, without the angle brackets: how `Format` prints it
/// (types/TestDateRoundFormatStringContracts.al:DateFormula_Evaluate_ThenFormat_PrintsTheNormalizedFormula).
pub fn formula_text(text: &str) -> String {
    let t = text.strip_prefix('<').unwrap_or(text);
    t.strip_suffix('>').unwrap_or(t).to_string()
}

/// Parses `<term...>` or the same without brackets (types/TestDateRoundFormatStringContracts.al:
/// CalcDate_WeeksQuartersWeekDaysAndCurrentPeriods), where a term is `[+|-][digits](D|W|M|Q|Y)`,
/// `[+|-]C(D|W|M|Q|Y)` or `[+|-]WD(1-7)`. After the first term every term carries its sign. A formula
/// with no term, lower case letters and the like are not pinned.
fn parse_formula(text: &str) -> Result<Vec<Term>, Signal> {
    let refuse = || unsupported(format!("date formula `{text}`: this form is not pinned by the corpus"));
    // The empty formula, `<>` and a text of white space are formulas without a term; CalcDate with
    // them returns the date (types/TestDateFormulaDurationFormatContracts.al:28-74).
    if is_blank_formula(text) {
        return Ok(Vec::new());
    }
    // Brackets are optional on either side, and the letters may be lower case
    // (text/TestProbeTextDateFormatContracts.al:Dates_CalcDate_Formulas, DateFormula_EvaluateForms).
    let upper = text.trim().to_ascii_uppercase();
    // Spaces are ignored: inside the brackets (types/lt-datetime/Evaluate.al:Format_Evaluate `< 1D >`) and between the terms
    // (types/lt-date-formula-spaces/DateFormulaSpaces.al: CalcDate_Spaces, EvaluateDateFormula_Spaces).
    let upper = if upper.contains(' ') {
        upper.replace(' ', "")
    } else {
        upper
    };
    let body = upper.strip_prefix('<').unwrap_or(&upper);
    let body = body.strip_suffix('>').unwrap_or(body);
    let b = body.as_bytes();
    let unit_of = |c: Option<&u8>| match c {
        Some(b'D') => Some(Unit::Day),
        Some(b'W') => Some(Unit::Week),
        Some(b'M') => Some(Unit::Month),
        Some(b'Q') => Some(Unit::Quarter),
        Some(b'Y') => Some(Unit::Year),
        _ => None,
    };
    // `1WD` evaluates and CalcDate gives the next day from Friday 2024-03-15 (types/W7EvaluateTests.al:
    // EvaluateDateFormula_Format2_NonEnglishFormulas), which fits no term this parser knows; what it means is not pinned.
    if (1..b.len()).any(|k| {
        b[k] == b'W'
            && b.get(k + 1) == Some(&b'D')
            && b[k - 1].is_ascii_digit()
            && !b.get(k + 2).is_some_and(u8::is_ascii_digit)
    }) {
        return refuse();
    }
    let mut terms = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if i > 0 && !matches!(b[i], b'+' | b'-') {
            // Two terms need a sign on the second (Dates_CalcDate_Formulas, `<1D1W>`).
            return raise(
                format!("The date formula '{body}' must include plus (+) or minus (-)."),
                "Evaluate",
            );
        }
        let negative = match b[i] {
            b'-' => {
                i += 1;
                true
            }
            b'+' => {
                i += 1;
                false
            }
            _ => false,
        };
        if b.get(i) == Some(&b'C') {
            let Some(unit) = unit_of(b.get(i + 1)) else {
                return refuse();
            };
            terms.push(Term::Edge { unit, end: !negative });
            i += 2;
            continue;
        }
        if b.get(i) == Some(&b'W') && b.get(i + 1) == Some(&b'D') {
            let Some(n) = b.get(i + 2).filter(|c| (b'1'..=b'7').contains(c)) else {
                // Only `WD8` is pinned (Dates_CalcDate_Formulas, `wd_with_number_8`); `WD0` is not.
                if b.get(i + 2).is_some_and(|c| *c == b'8' || *c == b'9') {
                    return raise(
                        format!(
                            "The date formula 'WD{}' contains a number that is too large.\\\\The number must be in the range 1 - 7.",
                            b[i + 2] as char
                        ),
                        "Evaluate",
                    );
                }
                return refuse();
            };
            terms.push(Term::WeekDay {
                n: i32::from(n - b'0'),
                forward: !negative,
            });
            i += 3;
            continue;
        }
        // A `D` before its number is a day step (`D1`, types/TestDateFormulaEvaluateLeftoversContracts.al:
        // Evaluate_Accepted). The other letters before a number are not pinned.
        if b.get(i) == Some(&b'D') && b.get(i + 1).is_some_and(u8::is_ascii_digit) {
            let start = i + 1;
            i = start;
            while b.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            let Ok(n) = body[start..i].parse::<i32>() else {
                return refuse();
            };
            terms.push(Term::Step(Unit::Day, if negative { -n } else { n }));
            continue;
        }
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if start == i {
            return refuse();
        }
        let Ok(n) = body[start..i].parse::<i32>() else {
            return refuse();
        };
        let Some(unit) = unit_of(b.get(i)) else {
            return refuse();
        };
        i += 1;
        terms.push(Term::Step(unit, if negative { -n } else { n }));
    }
    if terms.is_empty() {
        return refuse();
    }
    Ok(terms)
}

/// The formulas without a term: the empty text, `<>` and white space. `Evaluate` accepts them and
/// stores the empty formula, which `Format` prints as an empty text
/// (types/TestDateFormulaDurationFormatContracts.al:28-74: `''`, `'<>'`, `' '`).
pub fn is_blank_formula(text: &str) -> bool {
    text == "<>" || text.trim().is_empty()
}

/// Whether `Evaluate` rejects `text` as a date formula: it holds a letter that no formula uses
/// (the letters are C, D, W, M, Q and Y). Only `xyz` is pinned
/// (types/TestDateFormulaDurationFormatContracts.al:68); the rest of the texts the parser does not
/// know stay refused, not rejected (docs/probes-needed.md).
pub fn is_garbage_formula(text: &str) -> bool {
    text.bytes()
        .any(|b| b.is_ascii_alphabetic() && !matches!(b.to_ascii_uppercase(), b'C' | b'D' | b'W' | b'M' | b'Q' | b'Y'))
}

/// What `Evaluate` makes of a text for a DateFormula.
pub enum Evaluated {
    /// The formula to store, in the normal form `<TERMS>` (empty for the empty formula).
    Stored(String),
    /// `Evaluate` answers false.
    Rejected,
}

/// `Evaluate` into a DateFormula (types/TestDateFormulaEvaluateLeftoversContracts.al:35-69, BC 27.5):
/// lower case letters are read, angle brackets and white space are ignored (`< >` is the empty
/// formula), a sign in front is kept. A `D` or `WD` before its number is a formula (`D1`, `WD1`).
/// Rejected: a letter no formula uses (`1X`, `LW`), a `D` alone, a number alone, a decimal point,
/// and two parts without a sign (`1D1W`). Any other text stays refused (docs/probes-needed.md).
pub fn evaluate_formula(text: &str) -> Result<Evaluated, Signal> {
    // Only a pair of brackets around the whole text is pinned.
    let trimmed = text.trim();
    // A bracket on one side only is read as well (DateFormula_EvaluateForms: `<1D` and `1D>`).
    let inner = trimmed.strip_prefix('<').unwrap_or(trimmed);
    let inner = inner.strip_suffix('>').unwrap_or(inner);
    if inner.contains(['<', '>']) {
        return unsupported(format!(
            "date formula `{text}`: angle brackets that do not wrap it are not pinned"
        ));
    }
    let n: String = inner
        .chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_uppercase())
        .collect();
    if n.is_empty() {
        return Ok(Evaluated::Stored(String::new()));
    }
    // A number of five digits does not fit (`99999D`).
    let long_number = n.as_bytes().windows(5).any(|w| w.iter().all(u8::is_ascii_digit));
    if n.contains('.') || is_garbage_formula(&n) || long_number || n == "D" || n.bytes().all(|b| b.is_ascii_digit()) {
        return Ok(Evaluated::Rejected);
    }
    // A step (`digits` then a unit) followed by a digit starts a second part with no sign.
    let b = n.as_bytes();
    for i in 0..b.len().saturating_sub(2) {
        let after_step = b[i].is_ascii_digit()
            && matches!(b[i + 1], b'D' | b'W' | b'M' | b'Q' | b'Y')
            && b[i + 2].is_ascii_digit()
            && !(b[i + 1] == b'D' && i > 0 && b[i - 1] == b'W');
        if after_step {
            return Ok(Evaluated::Rejected);
        }
    }
    parse_formula(&n)?;
    Ok(Evaluated::Stored(format!("<{n}>")))
}

/// Whether `text` is a date formula `Evaluate` can store (see [`parse_formula`]).
pub fn check_formula(text: &str) -> Result<(), Signal> {
    parse_formula(text).map(|_| ())
}

/// The day `months` months from `(y, m, d)`; the day of the month is cut back to the month's length
/// (types/TestDateRoundFormatStringContracts.al:CalcDate_MonthYearClamping).
fn add_months(y: i32, m: u32, d: u32, months: i64) -> Result<i32, Signal> {
    let total = i64::from(y) * 12 + i64::from(m) - 1 + months;
    let (ny, nm) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
    if !(1..=9999).contains(&ny) {
        return unsupported("CalcDate result outside 0001-9999 (error text not pinned)");
    }
    Ok(day_from_civil(ny as i32, nm, d.min(days_in_month(ny as i32, nm))))
}

fn apply(terms: &[Term], from: i32) -> Result<i32, Signal> {
    let mut day = from;
    for term in terms {
        let (y, m, d) = civil_from_day(day);
        day = match *term {
            Term::Step(Unit::Day, n) => day.checked_add(n).unwrap_or(0),
            Term::Step(Unit::Week, n) => n.checked_mul(7).and_then(|n| day.checked_add(n)).unwrap_or(0),
            Term::Step(Unit::Month, n) => add_months(y, m, d, i64::from(n))?,
            Term::Step(Unit::Quarter, n) => add_months(y, m, d, i64::from(n) * 3)?,
            Term::Step(Unit::Year, n) => add_months(y, m, d, i64::from(n) * 12)?,
            Term::Edge { unit: Unit::Day, .. } => day,
            Term::Edge { unit: Unit::Week, end } => {
                if end {
                    day + (7 - weekday(day))
                } else {
                    day - (weekday(day) - 1)
                }
            }
            Term::Edge { unit: Unit::Month, end } => day_from_civil(y, m, if end { days_in_month(y, m) } else { 1 }),
            Term::Edge { unit: Unit::Year, end } => {
                if end {
                    day_from_civil(y, 12, 31)
                } else {
                    day_from_civil(y, 1, 1)
                }
            }
            Term::Edge {
                unit: Unit::Quarter,
                end,
            } => {
                let first = (m - 1) / 3 * 3 + 1;
                if end {
                    day_from_civil(y, first + 2, days_in_month(y, first + 2))
                } else {
                    day_from_civil(y, first, 1)
                }
            }
            Term::WeekDay { n, forward } => {
                let wd = weekday(day);
                // On the weekday itself the term moves a whole week (Dates_CalcDate_Formulas: `<WD1>` on a Monday).
                if forward {
                    day + (n - wd - 1).rem_euclid(7) + 1
                } else {
                    day - ((wd - n - 1).rem_euclid(7) + 1)
                }
            }
        };
        if !(1..=LAST_DAY).contains(&day) {
            return unsupported("CalcDate result outside 0001-9999 (error text not pinned)");
        }
    }
    Ok(day)
}

fn calcdate(ctx: &dyn Ctx, formula: &str, base: Option<&Value>) -> Out {
    // `%1` in `+%1D` counts as the day count 1: CalcDate('<+%1D>') and CalcDate('<+%1D') (what Format('<+%1D>', 5) cuts the text to)
    // both add one day (misc/lt-protected-var-abs-query-views/P146FormatCalcDate.al:P146_CalcDate_FormatBuilt_Formula,
    // `literal placeholder` and `format 5`). The brackets are optional for CalcDate (Dates_CalcDate_Formulas), which carries the
    // pin to the bare `+%1D`. Another count behind `%`, another unit and Evaluate of such a text are not pinned.
    if matches!(formula.trim(), "<+%1D>" | "<+%1D" | "+%1D") {
        return calcdate(ctx, "<+1D>", base);
    }
    // Evaluate takes a formula with one bracket missing (DateFormula_EvaluateForms, `<1D`); CalcDate of one is not pinned.
    let t = formula.trim();
    if t.starts_with('<') != t.ends_with('>') && !t.is_empty() {
        return unsupported(format!(
            "CalcDate of `{formula}`, a formula with one angle bracket (not pinned)"
        ));
    }
    let terms = parse_formula(formula)?;
    // Without a date the calculation counts from TODAY, not from WorkDate (types/
    // TestDateTimeIntegerCalcDateContracts.al:CalcDateWithoutADate_UsesToday_NotWorkDate).
    let today = base
        .is_none()
        .then(|| ctx.today_day())
        .flatten()
        .map(|d| Value::Date(d, false));
    let Some(base) = base.or(today.as_ref()) else {
        return unsupported("CalcDate without a date needs the host's clock");
    };
    if matches!(base, Value::Date(0, _)) {
        // types/TestDateRoundFormatStringContracts.al:CalcDate_UndefinedDate_Throws (CR and backslash line breaks).
        return raise(
            format!(
                "You cannot base a date calculation on an undefined date.\\\\Date: 0D\\Formula: {}.",
                formula_text(formula)
            ),
            "Argument",
        );
    }
    // A closing date is its day (Dates_CalcDate_Formulas, `closing_date_base`).
    let from = match base {
        Value::Date(n, _) if *n > 0 => *n,
        other => return plain_date(other, "CalcDate").map(|_| unreachable!()),
    };
    Ok(Value::Date(apply(&terms, from)?, false))
}

fn calcdate_text(ctx: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    match crate::native::text_of(&a[0]) {
        Some(f) => calcdate(ctx, &f, a.get(1)),
        None => unsupported("CalcDate with a formula that is not text"),
    }
}

fn calcdate_formula(ctx: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    match &a[0] {
        Value::DateFormula(f) => calcdate(ctx, f, a.get(1)),
        other => unsupported(format!("CalcDate with {}", other.describe())),
    }
}

#[cfg(test)]
mod tests {
    use crate::format::day_from_civil;
    use crate::native::tests::{TestCtx, call, t};
    use crate::native::{Signal, lookup};
    use crate::value::Value;

    fn date(y: i32, m: u32, d: u32) -> Value {
        Value::Date(day_from_civil(y, m, d), false)
    }
    fn is_date(r: Result<Value, Signal>, y: i32, m: u32, d: u32) {
        match r {
            Ok(Value::Date(n, false)) => assert_eq!(n, day_from_civil(y, m, d), "wrong date"),
            other => panic!("{other:?}"),
        }
    }
    #[track_caller]
    fn refused(r: Result<Value, Signal>) {
        assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?} should be a refusal");
    }
    fn int(r: Result<Value, Signal>) -> i32 {
        match r {
            Ok(Value::Integer(i)) => i,
            other => panic!("{other:?}"),
        }
    }
    fn on(id: &str, recv: Value) -> Result<Value, Signal> {
        let f = lookup(id).unwrap_or_else(|| panic!("{id} not registered"));
        let mut recv = recv;
        f(&mut TestCtx::default(), Some(&mut recv), &mut [])
    }
    fn calc(formula: &str, from: Value) -> Result<Value, Signal> {
        call("System.CalcDate#2", vec![t(formula), from])
    }

    // types/TestRoundDateTimeContracts.al:61-72: a precision of 700000 ms does not divide a day but a week.
    #[test]
    fn round_date_time_counts_a_precision_that_divides_a_week_but_not_a_day() {
        let at = |h: u32, m: u32, s: u32| {
            let Value::Date(d, _) = date(2024, 3, 5) else {
                unreachable!()
            };
            Value::DateTime((i64::from(d) - 1) * 86_400_000 + i64::from(h * 3600 + m * 60 + s) * 1000 + 1)
        };
        let round = |dir: &str| {
            call(
                "System.RoundDateTime#1",
                vec![at(12, 0, 31), Value::Integer(700_000), t(dir)],
            )
        };
        for (dir, want) in [("=", at(12, 0, 0)), ("<", at(12, 0, 0)), (">", at(12, 11, 40))] {
            let (Ok(Value::DateTime(got)), Value::DateTime(want)) = (round(dir), want) else {
                panic!("{dir}")
            };
            assert_eq!(got, want, "{dir}");
        }
    }

    #[test]
    fn round_date_time_refuses_a_precision_whose_multiples_depend_on_the_origin() {
        let r = call(
            "System.RoundDateTime#1",
            vec![Value::DateTime(86_400_000 * 700_000 + 1), Value::Integer(1_100_000)],
        );
        assert!(
            matches!(&r, Err(Signal::Unsupported(m)) if m.contains("not pinned")),
            "{r:?}"
        );
    }

    #[test]
    fn dmy2date_knows_the_length_of_every_month_and_the_leap_year_rules() {
        let d = |a: i32, b: i32, c: i32| {
            call(
                "System.DMY2Date#1",
                vec![Value::Integer(a), Value::Integer(b), Value::Integer(c)],
            )
        };
        for (month, days) in [
            (1, 31),
            (2, 28),
            (3, 31),
            (4, 30),
            (5, 31),
            (6, 30),
            (7, 31),
            (8, 31),
            (9, 30),
            (10, 31),
            (11, 30),
            (12, 31),
        ] {
            is_date(d(days, month, 2023), 2023, month as u32, days as u32);
            refused(d(days + 1, month, 2023));
        }
        // Leap years: divisible by 4, not by 100 unless by 400.
        is_date(d(29, 2, 2024), 2024, 2, 29);
        is_date(d(29, 2, 2000), 2000, 2, 29);
        refused(d(29, 2, 1900));
        refused(d(29, 2, 2023));
        refused(d(30, 2, 2024));
    }

    #[test]
    fn dmy2date_builds_a_date() {
        // fieldref/TestFieldRefValue.al:122 `DMY2Date(1, 1, 2024)`; record/TestDateVirtualTableFlowField.al:65
        // `DMY2Date(3, 1, 1)` and :83 `DMY2Date(31, 12, 9999)` are the ends of the range.
        let d = |a: i32, b: i32, c: i32| {
            call(
                "System.DMY2Date#1",
                vec![Value::Integer(a), Value::Integer(b), Value::Integer(c)],
            )
        };
        is_date(d(1, 1, 2024), 2024, 1, 1);
        is_date(d(31, 12, 9999), 9999, 12, 31);
        is_date(d(3, 1, 1), 1, 1, 3);
        is_date(d(29, 2, 2024), 2024, 2, 29);
    }

    #[test]
    fn dmy2date_refuses_what_the_corpus_does_not_pin() {
        let d = |args: Vec<i32>| call("System.DMY2Date#1", args.into_iter().map(Value::Integer).collect());
        refused(d(vec![30, 2, 2024]));
        refused(d(vec![1, 13, 2024]));
        refused(d(vec![1, 1, 0]));
        refused(d(vec![1, 1, 10000]));
        refused(d(vec![1]));
        refused(d(vec![1, 1]));
    }

    #[test]
    fn create_datetime_joins_date_and_time() {
        // json/TestJsonTypedGetters.al:351 `CreateDateTime(Today(), 120000T)`;
        // handlers/TestPageTemporalSetValue_Tests.al:85 pairs a date with 0T (midnight).
        let day = i64::from(day_from_civil(2024, 1, 15)) - 1;
        let noon = Value::Time(43_200_000 + 1);
        assert!(matches!(
            call("System.CreateDateTime#1", vec![date(2024, 1, 15), noon]),
            Ok(Value::DateTime(n)) if n == day * 86_400_000 + 43_200_000 + 1
        ));
        assert!(matches!(
            call("System.CreateDateTime#1", vec![date(2024, 1, 15), Value::Time(0)]),
            Ok(Value::DateTime(n)) if n == day * 86_400_000 + 1
        ));
        // types/TestDateRoundFormatStringContracts.al:CreateDateTime_MidnightBlankAndInvalid.
        assert!(matches!(
            call("System.CreateDateTime#1", vec![Value::Date(0, false), Value::Time(0)]),
            Ok(Value::DateTime(0))
        ));
        let Err(Signal::Error(e)) = call("System.CreateDateTime#1", vec![Value::Date(0, false), Value::Time(1)]) else {
            panic!("a blank date with a time must raise");
        };
        assert_eq!(
            (e.message.as_str(), e.code.as_str()),
            ("The date is not valid.", "DateInvalid")
        );
        // A closing date reads as its day (types/TestProbeScalarContracts.al is silent; text/TestProbeTextDateFormatContracts.al:
        // closing dates are dates here), which this build reads as the plain date.
        assert!(matches!(
            call("System.CreateDateTime#1", vec![Value::Date(5, true), Value::Time(1)]),
            Ok(Value::DateTime(_))
        ));
    }

    #[test]
    fn dt2date_and_dt2time_split_a_datetime() {
        // handlers/TestPageTemporalSetValue_Tests.al:117-118.
        let dt = call(
            "System.CreateDateTime#1",
            vec![date(2024, 3, 17), Value::Time(52_200_000 + 1)],
        )
        .unwrap();
        is_date(call("System.DT2Date#1", vec![dt.clone()]), 2024, 3, 17);
        assert!(matches!(
            call("System.DT2Time#1", vec![dt]),
            Ok(Value::Time(52_200_001))
        ));
        // text/TestProbeTextDateFormatContracts.al:Dates_DateTime_DT2DateTime, `dt2date_of_0DT`, `dt2time_of_0DT`: both blank.
        assert!(matches!(
            call("System.DT2Date#1", vec![Value::DateTime(0)]),
            Ok(Value::Date(0, false))
        ));
        assert!(matches!(
            call("System.DT2Time#1", vec![Value::DateTime(0)]),
            Ok(Value::Time(0))
        ));
    }

    // The instance forms give the same as the functions (assumed, registry entry datetime-date-time-methods).
    #[test]
    fn the_datetime_instance_methods_split_like_dt2date_and_dt2time() {
        let dt = call(
            "System.CreateDateTime#1",
            vec![date(2024, 3, 17), Value::Time(52_200_000 + 1)],
        )
        .unwrap();
        is_date(on("DateTime.Date#1", dt.clone()), 2024, 3, 17);
        assert!(matches!(on("DateTime.Time#1", dt), Ok(Value::Time(52_200_001))));
        assert!(matches!(
            on("DateTime.Date#1", Value::DateTime(0)),
            Ok(Value::Date(0, false))
        ));
        assert!(matches!(on("DateTime.Time#1", Value::DateTime(0)), Ok(Value::Time(0))));
    }

    #[test]
    fn dati2variant_is_a_datetime() {
        // session/TestSystemExtended.al:168.
        assert!(matches!(
            call("System.DaTi2Variant#1", vec![date(2024, 1, 1), Value::Time(43_200_001)]),
            Ok(Value::DateTime(_))
        ));
    }

    #[test]
    fn date2dmy_picks_a_part() {
        // session/TestCalcDateNotificationContracts.al:61 `Date2DMY(D, 1)` is the day, :79 `..., 2)` the
        // month; :32 `..., 3)` the year.
        let part = |n: i32| int(call("System.Date2DMY#1", vec![date(2024, 3, 15), Value::Integer(n)]));
        assert_eq!((part(1), part(2), part(3)), (15, 3, 2024));
        // types/TestDateRoundFormatStringContracts.al:Date2DMY_Part4_Throws.
        let Err(Signal::Error(e)) = call("System.Date2DMY#1", vec![date(2024, 3, 15), Value::Integer(4)]) else {
            panic!("part 4 must raise");
        };
        assert_eq!(e.code, "OutsidePermittedRange");
        assert!(
            e.message
                .ends_with("The current value is: 4. The permitted range is: from 1 to 3.")
        );
        // Date2DMY of the undefined date is not pinned (Date2DWY's is).
        refused(call(
            "System.Date2DMY#1",
            vec![Value::Date(0, false), Value::Integer(1)],
        ));
    }

    #[test]
    fn date2dwy_is_iso_8601() {
        // types/TestDateRoundFormatStringContracts.al:Date2DWY_WeekNumbers_AreIso8601.
        let part = |y, m, d, n| int(call("System.Date2DWY#1", vec![date(y, m, d), Value::Integer(n)]));
        assert_eq!(part(2024, 1, 1, 3), 2024);
        assert_eq!(part(2024, 1, 1, 2), 1);
        assert_eq!(part(2024, 12, 29, 2), 52);
        assert_eq!(part(2024, 12, 30, 2), 1);
        assert_eq!(part(2021, 1, 1, 2), 53);
        assert_eq!(part(2021, 1, 3, 2), 53);
        assert_eq!(part(2020, 12, 31, 2), 53);
        assert_eq!(part(2024, 3, 5, 1), 2);
        // The year is the ISO week-year (text/TestProbeTextDateFormatContracts.al:Dates_Date2DWY_WeekAndYear: 2027-01-01 is
        // week 53 of 2026).
        assert_eq!(part(2021, 1, 1, 3), 2020);
        assert_eq!(part(2027, 1, 1, 3), 2026);
    }

    #[test]
    fn normal_date_and_closing_date() {
        // record/TestDateVirtualTable.al:188-190 and record/TestFilterContracts.al:56.
        assert!(matches!(
            call("System.ClosingDate#1", vec![date(2024, 12, 31)]),
            Ok(Value::Date(n, true)) if n == day_from_civil(2024, 12, 31)
        ));
        let closing = call("System.ClosingDate#1", vec![date(2026, 3, 31)]).unwrap();
        is_date(call("System.NormalDate#1", vec![closing]), 2026, 3, 31);
        is_date(call("System.NormalDate#1", vec![date(2026, 3, 1)]), 2026, 3, 1);
        assert!(matches!(
            call("System.ClosingDate#1", vec![Value::Date(0, false)]),
            Err(Signal::Error(e)) if e.code == "DateInvalid"
        ));
    }

    #[test]
    fn day_of_week_counts_from_monday() {
        // session/TestSystemExtended.al:278 `20240101D.DayOfWeek()` is 1 (Monday).
        assert_eq!(int(on("Date.DayOfWeek#1", date(2024, 1, 1))), 1);
        assert_eq!(int(on("Date.DayOfWeek#1", date(2024, 1, 7))), 7);
        assert_eq!(int(on("Date.DayOfWeek#1", date(2024, 1, 3))), 3);
        refused(on("Date.DayOfWeek#1", Value::Date(0, false)));
    }

    #[test]
    fn week_no_is_the_iso_week() {
        // session/TestSystemExtended.al:285-300 pins the range 1..53 and the first week of a year.
        assert_eq!(int(on("Date.WeekNo#1", date(2024, 1, 1))), 1);
        assert_eq!(int(on("Date.WeekNo#1", date(2024, 1, 15))), 3);
        assert_eq!(int(on("Date.WeekNo#1", date(2021, 1, 3))), 53);
        assert_eq!(int(on("Date.WeekNo#1", date(2024, 12, 30))), 1);
    }

    #[test]
    fn time_parts() {
        // session/TestMiscTypes.al:22-42.
        let t = Value::Time(((12 * 60 + 30) * 60 + 5) * 1000 + 250 + 1);
        assert_eq!(int(on("Time.Hour#1", t.clone())), 12);
        assert_eq!(int(on("Time.Minute#1", t.clone())), 30);
        assert_eq!(int(on("Time.Second#1", t.clone())), 5);
        assert_eq!(int(on("Time.Millisecond#1", t)), 250);
        refused(on("Time.Hour#1", Value::Time(0)));
    }

    #[test]
    fn calcdate_day_month_year_steps() {
        // types/TestDate.al:28-38 (`<+1D>`, `<-1D>`), session/TestDateTimeContracts.al:29 (`<-1M>`),
        // :38 and :47 (`<1Y>` clamps Feb 29 to Feb 28).
        is_date(calc("<+1D>", date(2024, 1, 1)), 2024, 1, 2);
        is_date(calc("<-1D>", date(2024, 1, 2)), 2024, 1, 1);
        is_date(calc("<1D>", date(2024, 2, 28)), 2024, 2, 29);
        is_date(calc("<+1D>", date(2024, 2, 29)), 2024, 3, 1);
        is_date(calc("<-1M>", date(2024, 3, 1)), 2024, 2, 1);
        is_date(calc("<1M>", date(2024, 1, 1)), 2024, 2, 1);
        is_date(calc("<1Y>", date(2024, 2, 29)), 2025, 2, 28);
        is_date(calc("<1Y>", date(2024, 2, 28)), 2025, 2, 28);
        is_date(calc("<-1M>", date(2024, 1, 15)), 2023, 12, 15);
    }

    #[test]
    fn calcdate_ends_of_periods() {
        // session/TestCalcDateNotificationContracts.al:16-80.
        is_date(calc("<CM>", date(2024, 3, 15)), 2024, 3, 31);
        is_date(calc("<CM>", date(2024, 2, 15)), 2024, 2, 29);
        is_date(calc("<-CM>", date(2024, 3, 15)), 2024, 3, 1);
        is_date(calc("<CY>", date(2024, 3, 15)), 2024, 12, 31);
        is_date(calc("<-CY>", date(2024, 3, 15)), 2024, 1, 1);
        is_date(calc("<CQ>", date(2024, 2, 15)), 2024, 3, 31);
    }

    #[test]
    fn calcdate_terms_apply_left_to_right() {
        // session/TestCalcDateNotificationContracts.al:30 `<1M-1D>` from the first of a month stays in it.
        is_date(calc("<1M-1D>", date(2024, 3, 1)), 2024, 3, 31);
        is_date(calc("<CM+1D>", date(2024, 3, 10)), 2024, 4, 1);
    }

    #[test]
    fn calcdate_refuses_forms_the_corpus_does_not_pin() {
        let from = date(2024, 3, 15);
        for f in ["<MON>", "<1W+MON>", "<2CM>", "<1X>", "<D>", "<+>", "<1D", "<WD0>"] {
            refused(calc(f, from.clone()));
        }
        // A result off the calendar.
        refused(calc("<1Y>", date(9999, 6, 1)));
        refused(calc("<-1Y>", date(1, 6, 1)));
        refused(call("System.CalcDate#2", vec![t("<1D>")]));
    }

    #[test]
    fn calcdate_probe_forms_from_the_text_probe() {
        // text/TestProbeTextDateFormatContracts.al:Dates_CalcDate_Formulas.
        let mon = date(2024, 1, 15);
        let wed = date(2024, 1, 17);
        // A weekday formula on a day that already is that weekday moves a week.
        is_date(calc("<WD1>", mon.clone()), 2024, 1, 22);
        is_date(calc("<-WD1>", mon.clone()), 2024, 1, 8);
        is_date(calc("<WD3>", wed.clone()), 2024, 1, 24);
        is_date(calc("<cm>", mon.clone()), 2024, 1, 31);
        is_date(calc("<1d>", mon.clone()), 2024, 1, 16);
        is_date(calc("<1D+1W>", mon.clone()), 2024, 1, 23);
        // Spaces between the terms are ignored (Microsoft's Tests-SCM Item.TestField("Next Counting Start Date",
        // CalcDate('<CM + 1D>', WorkDate())) and Base Application's `<CM + 1D>`; the corpus pins `< 1D >`).
        is_date(calc("<1D + 1W>", mon.clone()), 2024, 1, 23);
        // A second term without a sign.
        let Err(Signal::Error(e)) = calc("<1D1W>", mon.clone()) else {
            panic!("a second term without a sign must raise");
        };
        assert_eq!(
            (e.message.as_str(), e.code.as_str()),
            (
                "The date formula '1D1W' must include plus (+) or minus (-).",
                "Evaluate"
            )
        );
        // A closing date as the base: `<1D>` from the closing date of 2023-12-31 is 2024-01-01.
        is_date(
            calc("<1D>", Value::Date(day_from_civil(2023, 12, 31), true)),
            2024,
            1,
            1,
        );
        is_date(
            calc("<CM>", Value::Date(day_from_civil(2023, 12, 31), true)),
            2023,
            12,
            31,
        );
        // `WD8` is too large.
        let Err(Signal::Error(e)) = calc("<WD8>", mon) else {
            panic!("WD8 must raise");
        };
        assert_eq!(e.code, "Evaluate");
        assert!(
            e.message
                .starts_with("The date formula 'WD8' contains a number that is too large.")
        );
    }

    #[test]
    fn calcdate_forms_the_probe_pins() {
        // types/TestDateRoundFormatStringContracts.al:CalcDate_MonthYearClamping and
        // CalcDate_WeeksQuartersWeekDaysAndCurrentPeriods.
        let tue = date(2024, 3, 5);
        is_date(calc("<1M>", date(2024, 1, 31)), 2024, 2, 29);
        is_date(calc("<1M>", date(2024, 11, 30)), 2024, 12, 30);
        is_date(calc("<-1M>", date(2024, 3, 31)), 2024, 2, 29);
        is_date(calc("<1Y>", date(2024, 2, 29)), 2025, 2, 28);
        is_date(calc("<1W>", tue.clone()), 2024, 3, 12);
        is_date(calc("<CW>", tue.clone()), 2024, 3, 10);
        is_date(calc("<-CW>", tue.clone()), 2024, 3, 4);
        is_date(calc("<CD>", tue.clone()), 2024, 3, 5);
        is_date(calc("<1Q>", tue.clone()), 2024, 6, 5);
        is_date(calc("<-CQ>", tue.clone()), 2024, 1, 1);
        is_date(calc("<CQ>", tue.clone()), 2024, 3, 31);
        is_date(calc("<WD1>", tue.clone()), 2024, 3, 11);
        is_date(calc("<-WD1>", tue.clone()), 2024, 3, 4);
        is_date(calc("<WD7>", tue.clone()), 2024, 3, 10);
        is_date(calc("1M", tue.clone()), 2024, 4, 5);
        is_date(calc("<1M+CM>", tue.clone()), 2024, 4, 30);
        is_date(calc("<CM+1D>", tue.clone()), 2024, 4, 1);
        is_date(calc("<-1D-1M>", tue), 2024, 2, 4);
    }

    #[test]
    fn calcdate_of_an_undefined_date_raises() {
        // types/TestDateRoundFormatStringContracts.al:CalcDate_UndefinedDate_Throws.
        let Err(Signal::Error(e)) = calc("<1D>", Value::Date(0, false)) else {
            panic!("must raise");
        };
        assert_eq!(
            e.message,
            "You cannot base a date calculation on an undefined date.\\\\Date: 0D\\Formula: 1D."
        );
        assert_eq!(e.code, "Argument");
    }

    #[test]
    fn calcdate_takes_a_date_formula() {
        // types/TestDateFormulaContracts.al:45-125.
        let c = |f: &str, d| call("System.CalcDate#1", vec![Value::DateFormula(f.into()), d]);
        is_date(c("<1D>", date(2024, 1, 1)), 2024, 1, 2);
        is_date(c("<1M>", date(2024, 1, 1)), 2024, 2, 1);
        is_date(c("<-1D>", date(2024, 1, 2)), 2024, 1, 1);
        is_date(c("<1Y>", date(2024, 1, 1)), 2025, 1, 1);
        // types/TestDateFormulaDurationFormatContracts.al:30-43: the empty formula moves nothing.
        is_date(c("", date(2024, 1, 1)), 2024, 1, 1);
        is_date(call("System.CalcDate#2", vec![t(""), date(2024, 3, 5)]), 2024, 3, 5);
        // The empty formula on the undefined date raises the same error as any other formula
        // (types/TestDateFormulaEvaluateLeftoversContracts.al:CalcDate_EmptyFormula).
        let Err(Signal::Error(e)) = c("", Value::Date(0, false)) else {
            panic!("expected an AL error")
        };
        assert!(
            e.message
                .starts_with("You cannot base a date calculation on an undefined date."),
            "{}",
            e.message
        );
    }

    // types/TestDateFormulaDurationFormatContracts.al:28-74.
    #[test]
    fn evaluate_reads_the_blank_like_formulas_and_rejects_garbage() {
        let f = lookup("System.Evaluate#1").unwrap();
        let eval = |old: &str, text: &str| {
            let mut args = vec![Value::DateFormula(old.into()), t(text)];
            let mut ctx = TestCtx::default();
            let r = f(&mut ctx, None, &mut args);
            let Value::DateFormula(now) = &args[0] else { panic!() };
            (matches!(r, Ok(Value::Boolean(true))), super::formula_text(now))
        };
        assert_eq!(eval("<1D>", ""), (true, String::new()));
        assert_eq!(eval("<1D>", "<>"), (true, String::new()));
        assert_eq!(eval("<1D>", " "), (true, String::new()));
        assert_eq!(eval("", "0D"), (true, "0D".into()));
        assert_eq!(eval("", "<0D>"), (true, "0D".into()));
        // A failed Evaluate leaves the value.
        assert_eq!(eval("<0D>", "xyz"), (false, "0D".into()));
    }

    // types/TestDateFormulaEvaluateLeftoversContracts.al:35-69 (BC 27.5): what Format prints back and what
    // CalcDate(formula, 2024-01-31) gives, for the inputs the other tests leave open.
    #[test]
    fn evaluate_reads_lower_case_brackets_white_space_signs_and_a_letter_before_its_number() {
        let f = lookup("System.Evaluate#1").unwrap();
        let eval = |text: &str| {
            let mut args = vec![Value::DateFormula("<9D>".into()), t(text)];
            let r = f(&mut TestCtx::default(), None, &mut args);
            let Value::DateFormula(now) = &args[0] else { panic!() };
            match r {
                Ok(Value::Boolean(true)) => {
                    let shown = super::formula_text(now);
                    let d = call("System.CalcDate#1", vec![args[0].clone(), date(2024, 1, 31)]);
                    Ok((shown, d))
                }
                Ok(Value::Boolean(false)) => Err(()),
                other => panic!("{text}: {other:?}"),
            }
        };
        let ok = |text: &str, shown: &str, y, m, d| {
            let (s, r) = eval(text).unwrap_or_else(|()| panic!("{text} was rejected"));
            assert_eq!(s, shown, "{text}");
            is_date(r, y, m, d);
        };
        ok("1d", "1D", 2024, 2, 1);
        ok("1D", "1D", 2024, 2, 1);
        ok("< >", "", 2024, 1, 31);
        ok("-1W", "-1W", 2024, 1, 24);
        ok("CM", "CM", 2024, 1, 31);
        ok("<1M+CM>", "1M+CM", 2024, 2, 29);
        ok("+1D", "+1D", 2024, 2, 1);
        ok("D1", "D1", 2024, 2, 1);
        ok("1Y-1D", "1Y-1D", 2025, 1, 30);
        ok("WD1", "WD1", 2024, 2, 5);
        ok("<-WD1>", "-WD1", 2024, 1, 29);
        ok("CQ", "CQ", 2024, 3, 31);
        ok(" 1D ", "1D", 2024, 2, 1);
        ok("1 D", "1D", 2024, 2, 1);
        for bad in ["1X", "D", "<D>", "1d1w", "1D1W", "1", "LW", "1.5D"] {
            assert!(eval(bad).is_err(), "{bad} should be rejected");
        }
    }

    #[test]
    fn evaluate_reads_a_date_formula() {
        // types/TestDateFormulaContracts.al:21,31,43,139: Evaluate then Format, CalcDate.
        let mut args = vec![Value::DateFormula(String::new()), t("<1D>")];
        let f = lookup("System.Evaluate#1").unwrap();
        assert!(matches!(
            f(&mut TestCtx::default(), None, &mut args),
            Ok(Value::Boolean(true))
        ));
        assert!(matches!(&args[0], Value::DateFormula(s) if s == "<1D>"));
        // A text with letters no formula uses is rejected (types/TestDateFormulaDurationFormatContracts.al:68).
        let mut args = vec![Value::DateFormula(String::new()), t("not a formula")];
        assert!(matches!(
            f(&mut TestCtx::default(), None, &mut args),
            Ok(Value::Boolean(false))
        ));
        // A formula with one bracket missing is accepted (text/TestProbeTextDateFormatContracts.al:DateFormula_EvaluateForms, `<1D`).
        let mut args = vec![Value::DateFormula(String::new()), t("<1D")];
        assert!(matches!(
            f(&mut TestCtx::default(), None, &mut args),
            Ok(Value::Boolean(true))
        ));
    }
}
